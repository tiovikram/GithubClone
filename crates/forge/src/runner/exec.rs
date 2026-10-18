//! Executes one job on the runner: steps (`run:` scripts with bash/sh/python or a custom
//! shell), built-in actions (see `builtin.rs`), composite actions from repositories on this
//! forge, env files (GITHUB_OUTPUT/ENV/PATH/STEP_SUMMARY/STATE), workflow commands, secret
//! masking, step/job timeouts, cancellation, job outputs, and log streaming.
//!
//! Executors: `shell` runs processes directly on the runner host (in a per-job work
//! directory, with a per-job HOME and a minimal environment: the pod/VM is the sandbox);
//! `docker` runs every `run:` step inside a per-job container (the job's `container:` image,
//! or an image mapped from `runs-on`), with the job directory bind-mounted at the same path.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use super::expr::{self, Ctx, Status};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Executor {
    Shell,
    Docker,
}

/// HTTP client for the runner protocol, authenticated with the job token.
#[derive(Clone)]
pub struct Api {
    pub http: reqwest::Client,
    /// forge base URL as seen by the runner (`FORGE_RUNNER_URL`)
    pub base: String,
    /// runner credential: authenticates the runner protocol calls
    pub credential: String,
    /// the job's GITHUB_TOKEN: git and REST calls made for the job
    pub token: String,
    pub job_id: i64,
}

impl Api {
    pub fn url(&self, path: &str) -> String {
        format!("{}/api/forge/runner/jobs/{}{path}", self.base, self.job_id)
    }
    pub async fn post_json(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let r = self.http.post(self.url(path)).bearer_auth(&self.credential).json(body).send().await?;
        let st = r.status();
        let text = r.text().await.unwrap_or_default();
        if !st.is_success() {
            anyhow::bail!("{path}: HTTP {st}: {}", text.chars().take(300).collect::<String>());
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }
    pub async fn post_bytes(&self, path: &str, body: Vec<u8>) -> anyhow::Result<(reqwest::StatusCode, Value)> {
        let r = self.http.post(self.url(path)).bearer_auth(&self.credential).body(body).send().await?;
        let st = r.status();
        let text = r.text().await.unwrap_or_default();
        Ok((st, serde_json::from_str(&text).unwrap_or(Value::String(text))))
    }
    pub async fn get(&self, path: &str) -> anyhow::Result<reqwest::Response> {
        Ok(self.http.get(self.url(path)).bearer_auth(&self.credential).send().await?)
    }
}

/// Streams log lines to the server in chunks (per step), masking secrets.
pub struct Logger {
    tx: mpsc::UnboundedSender<(i32, Vec<u8>)>,
    pub masks: Arc<Mutex<Vec<String>>>,
    pub step: AtomicI32,
}

impl Logger {
    pub fn mask(&self, s: &str) -> String {
        let masks = self.masks.lock().unwrap();
        let mut out = s.to_string();
        for m in masks.iter() {
            if !m.is_empty() {
                out = out.replace(m.as_str(), "***");
            }
        }
        out
    }
    pub fn line(&self, text: &str) {
        let step = self.step.load(Ordering::SeqCst);
        for l in text.split('\n') {
            let l = l.strip_suffix('\r').unwrap_or(l);
            let masked = self.mask(l);
            let _ = self.tx.send((step, format!("{} {masked}\n", super::logs::stamp()).into_bytes()));
        }
    }
    pub fn add_mask(&self, v: &str) {
        let v = v.trim();
        if v.len() >= 2 {
            let mut m = self.masks.lock().unwrap();
            if !m.iter().any(|x| x == v) {
                m.push(v.to_string());
                m.sort_by_key(|x| std::cmp::Reverse(x.len()));
            }
        }
    }
}

/// Uploads log chunks; sets `cancel` when the server says the job was cancelled.
fn spawn_uploader(api: Api, mut rx: mpsc::UnboundedReceiver<(i32, Vec<u8>)>, cancel: Arc<AtomicBool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut cur: Option<(i32, Vec<u8>)> = None;
        let mut tick = tokio::time::interval(Duration::from_millis(800));
        let flush = |api: Api, step: i32, buf: Vec<u8>, cancel: Arc<AtomicBool>| async move {
            for _ in 0..3 {
                match api.post_bytes(&format!("/logs?step={step}"), buf.clone()).await {
                    Ok((st, v)) if st.is_success() => {
                        if v["cancel"].as_bool() == Some(true) {
                            cancel.store(true, Ordering::SeqCst);
                        }
                        return;
                    }
                    _ => tokio::time::sleep(Duration::from_millis(500)).await,
                }
            }
        };
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Some((step, data)) => {
                        match &mut cur {
                            Some((s, buf)) if *s == step && buf.len() < 64 * 1024 => buf.extend_from_slice(&data),
                            _ => {
                                if let Some((s, buf)) = cur.take() {
                                    flush(api.clone(), s, buf, cancel.clone()).await;
                                }
                                cur = Some((step, data));
                            }
                        }
                    }
                    None => {
                        if let Some((s, buf)) = cur.take() {
                            flush(api.clone(), s, buf, cancel.clone()).await;
                        }
                        return;
                    }
                },
                _ = tick.tick() => {
                    if let Some((s, buf)) = cur.take() {
                        flush(api.clone(), s, buf, cancel.clone()).await;
                    }
                }
            }
        }
    })
}

/// A step as reported to the server (the `steps` of the job JSON).
#[derive(Clone, Debug)]
pub struct StepReport {
    pub number: i32,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

impl StepReport {
    fn json(&self) -> Value {
        json!({ "number": self.number, "name": self.name, "status": self.status, "conclusion": self.conclusion,
                "started_at": self.started_at, "completed_at": self.completed_at })
    }
}

fn now() -> String {
    crate::json::ts(chrono::Utc::now())
}

/// Result of one step.
#[derive(Clone, Debug)]
pub struct StepResult {
    pub outcome: String,
    pub outputs: Map<String, Value>,
}

/// A post step registered by an action (run after the main steps, in reverse order).
#[derive(Clone)]
pub struct PostStep {
    pub name: String,
    pub kind: String,
    pub data: Value,
    /// condition: `always` or `success`
    pub when: &'static str,
}

/// Everything a running job needs.
pub struct Job {
    pub api: Api,
    pub msg: Value,
    pub executor: Executor,
    pub dir: PathBuf,
    pub workspace: PathBuf,
    pub temp: PathBuf,
    pub home: PathBuf,
    pub log: Arc<Logger>,
    pub cancel: Arc<AtomicBool>,
    pub timed_out: Arc<AtomicBool>,
    pub status: Status,
    /// base contexts (github, needs, matrix, strategy, inputs, vars, secrets)
    pub contexts: Value,
    /// GITHUB_ENV additions
    pub env_added: BTreeMap<String, String>,
    /// GITHUB_PATH additions (prepended, newest first)
    pub path_added: Vec<String>,
    pub steps_ctx: Map<String, Value>,
    pub reports: Arc<Mutex<Vec<StepReport>>>,
    pub post: Vec<PostStep>,
    pub container: Option<String>,
    pub runner_name: String,
    pub summary: String,
}

pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// Docker image for a job without `container:` on the docker executor.
fn image_for_labels(labels: &[String]) -> String {
    for l in labels {
        let l = l.to_ascii_lowercase();
        if let Some(v) = l.strip_prefix("ubuntu-") {
            if v == "latest" {
                return "ubuntu:24.04".into();
            }
            if v.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return format!("ubuntu:{v}");
            }
        }
    }
    std::env::var("FORGE_RUNNER_DEFAULT_IMAGE").unwrap_or_else(|_| "ubuntu:24.04".into())
}

pub async fn run_cmd(cmd: &mut tokio::process::Command) -> anyhow::Result<(bool, String)> {
    let out = cmd.stdin(std::process::Stdio::null()).output().await?;
    Ok((out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
}

impl Job {
    /// Builds a job from the server's message and starts its log uploader.
    pub fn new(api: Api, msg: Value, executor: Executor, root: &Path, runner_name: &str) -> anyhow::Result<(Job, tokio::task::JoinHandle<()>)> {
        let job_id = msg["job_id"].as_i64().unwrap_or(0);
        let dir = root.join(format!("job-{job_id}-{}", crate::auth::random_string(6).to_lowercase()));
        let repo_name = msg.pointer("/repository/name").and_then(Value::as_str).unwrap_or("repo").to_string();
        let workspace = dir.join("work").join(&repo_name);
        let temp = dir.join("temp");
        let home = dir.join("home");
        for d in [&workspace, &temp, &home, &dir.join("tool")] {
            std::fs::create_dir_all(d)?;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let up = spawn_uploader(api.clone(), rx, cancel.clone());
        let log = Arc::new(Logger { tx, masks: Arc::new(Mutex::new(Vec::new())), step: AtomicI32::new(1) });
        if let Some(Value::Object(secrets)) = msg.pointer("/contexts/secrets") {
            for v in secrets.values() {
                if let Some(s) = v.as_str() {
                    log.add_mask(s);
                    for l in s.lines() {
                        log.add_mask(l);
                    }
                }
            }
        }
        let mut contexts = msg["contexts"].clone();
        let ws = workspace.to_string_lossy().to_string();
        contexts["github"]["workspace"] = json!(ws);
        contexts["runner"] = json!({
            "name": runner_name,
            "os": msg.pointer("/runner/os").cloned().unwrap_or(json!("Linux")),
            "arch": msg.pointer("/runner/arch").cloned().unwrap_or(json!("X64")),
            "temp": temp.to_string_lossy(),
            "tool_cache": dir.join("tool").to_string_lossy(),
            "environment": "self-hosted",
            "debug": "",
        });
        contexts["job"] = json!({ "status": "success", "container": { "id": null, "network": null }, "services": {} });
        Ok((
            Job {
                api,
                msg,
                executor,
                dir,
                workspace,
                temp,
                home,
                log,
                cancel,
                timed_out: Arc::new(AtomicBool::new(false)),
                status: Status::Success,
                contexts,
                env_added: BTreeMap::new(),
                path_added: Vec::new(),
                steps_ctx: Map::new(),
                reports: Arc::new(Mutex::new(Vec::new())),
                post: Vec::new(),
                container: None,
                runner_name: runner_name.to_string(),
                summary: String::new(),
            },
            up,
        ))
    }

    pub fn def(&self) -> &Value {
        &self.msg["job"]
    }

    fn status_str(&self) -> &'static str {
        match self.status {
            Status::Success => "success",
            Status::Failure => "failure",
            Status::Cancelled => "cancelled",
        }
    }

    /// Contexts for expressions in a step (`env` = the env the step will see).
    pub fn ctx_value(&self, env: &BTreeMap<String, String>) -> Value {
        let mut c = self.contexts.clone();
        c["env"] = json!(env);
        c["steps"] = Value::Object(self.steps_ctx.clone());
        c["job"]["status"] = json!(self.status_str());
        c
    }

    fn eval_ctx<'a>(&'a self, c: &'a Value) -> Ctx<'a> {
        Ctx { contexts: c, status: self.current_status(), workspace: Some(&self.workspace) }
    }

    pub fn current_status(&self) -> Status {
        if self.cancel.load(Ordering::SeqCst) {
            Status::Cancelled
        } else {
            self.status
        }
    }

    /// Workflow + job env (interpolated) + GITHUB_ENV additions.
    pub fn job_env(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        let base = self.ctx_value(&BTreeMap::new());
        for src in [self.msg.pointer("/workflow/env"), self.def().get("env")] {
            if let Some(Value::Object(m)) = src {
                let mut c = base.clone();
                c["env"] = json!(env);
                for (k, v) in m {
                    let s = expr::interpolate(&self.eval_ctx(&c), &expr::to_string(v)).unwrap_or_else(|_| expr::to_string(v));
                    env.insert(k.clone(), s);
                }
            }
        }
        for (k, v) in &self.env_added {
            env.insert(k.clone(), v.clone());
        }
        env
    }

    /// Adds a step to the reported list and returns its number.
    pub fn begin_step(&self, name: &str) -> i32 {
        let mut r = self.reports.lock().unwrap();
        let n = r.len() as i32 + 1;
        r.push(StepReport { number: n, name: name.to_string(), status: "in_progress".into(), conclusion: None, started_at: Some(now()), completed_at: None });
        self.log.step.store(n, Ordering::SeqCst);
        n
    }

    pub fn end_step(&self, n: i32, conclusion: &str) {
        let mut r = self.reports.lock().unwrap();
        if let Some(s) = r.iter_mut().find(|s| s.number == n) {
            s.status = "completed".into();
            s.conclusion = Some(conclusion.to_string());
            if conclusion == "skipped" {
                s.started_at = None;
            } else {
                s.completed_at = Some(now());
            }
            if conclusion == "skipped" {
                s.completed_at = None;
            }
        }
    }

    pub fn steps_json(&self) -> Value {
        Value::Array(self.reports.lock().unwrap().iter().map(StepReport::json).collect())
    }

    /// Process environment for a step.
    pub fn process_env(&self, step_env: &BTreeMap<String, String>, files: &EnvFiles, action: &str) -> BTreeMap<String, String> {
        let mut e: BTreeMap<String, String> = BTreeMap::new();
        for k in ["LANG", "LC_ALL", "USER", "LOGNAME", "SHELL", "TZ", "DOCKER_HOST"] {
            if let Ok(v) = std::env::var(k) {
                e.insert(k.into(), v);
            }
        }
        let mut path: Vec<String> = self.path_added.clone();
        if self.container.is_some() {
            path.push("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into());
        } else {
            path.push(std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()));
        }
        e.insert("PATH".into(), path.join(":"));
        e.insert("HOME".into(), self.home.to_string_lossy().into());
        e.insert("TMPDIR".into(), self.temp.to_string_lossy().into());
        e.insert("CI".into(), "true".into());
        e.insert("GITHUB_ACTIONS".into(), "true".into());
        e.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        e.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        let gh = &self.contexts["github"];
        let g = |k: &str| expr::to_string(&gh[k]);
        for (env_name, key) in [
            ("GITHUB_ACTOR", "actor"), ("GITHUB_ACTOR_ID", "actor_id"), ("GITHUB_API_URL", "api_url"), ("GITHUB_BASE_REF", "base_ref"),
            ("GITHUB_EVENT_NAME", "event_name"), ("GITHUB_GRAPHQL_URL", "graphql_url"), ("GITHUB_HEAD_REF", "head_ref"),
            ("GITHUB_JOB", "job"), ("GITHUB_REF", "ref"), ("GITHUB_REF_NAME", "ref_name"), ("GITHUB_REF_PROTECTED", "ref_protected"),
            ("GITHUB_REF_TYPE", "ref_type"), ("GITHUB_REPOSITORY", "repository"), ("GITHUB_REPOSITORY_ID", "repository_id"),
            ("GITHUB_REPOSITORY_OWNER", "repository_owner"), ("GITHUB_REPOSITORY_OWNER_ID", "repository_owner_id"),
            ("GITHUB_RETENTION_DAYS", "retention_days"), ("GITHUB_RUN_ATTEMPT", "run_attempt"), ("GITHUB_RUN_ID", "run_id"),
            ("GITHUB_RUN_NUMBER", "run_number"), ("GITHUB_SERVER_URL", "server_url"), ("GITHUB_SHA", "sha"),
            ("GITHUB_TRIGGERING_ACTOR", "triggering_actor"), ("GITHUB_WORKFLOW", "workflow"), ("GITHUB_WORKFLOW_REF", "workflow_ref"),
            ("GITHUB_WORKFLOW_SHA", "workflow_sha"), ("GITHUB_WORKSPACE", "workspace"),
        ] {
            e.insert(env_name.into(), g(key));
        }
        e.insert("GITHUB_EVENT_PATH".into(), self.temp.join("_github_workflow/event.json").to_string_lossy().into());
        e.insert("GITHUB_ACTION".into(), action.into());
        e.insert("GITHUB_OUTPUT".into(), files.output.to_string_lossy().into());
        e.insert("GITHUB_ENV".into(), files.env.to_string_lossy().into());
        e.insert("GITHUB_PATH".into(), files.path.to_string_lossy().into());
        e.insert("GITHUB_STEP_SUMMARY".into(), files.summary.to_string_lossy().into());
        e.insert("GITHUB_STATE".into(), files.state.to_string_lossy().into());
        let r = &self.contexts["runner"];
        for (k, key) in [("RUNNER_OS", "os"), ("RUNNER_ARCH", "arch"), ("RUNNER_NAME", "name"), ("RUNNER_TEMP", "temp"), ("RUNNER_TOOL_CACHE", "tool_cache"), ("RUNNER_ENVIRONMENT", "environment")] {
            e.insert(k.into(), expr::to_string(&r[key]));
        }
        e.insert("RUNNER_WORKSPACE".into(), self.workspace.parent().unwrap_or(&self.workspace).to_string_lossy().into());
        for (k, v) in step_env {
            e.insert(k.clone(), v.clone());
        }
        e
    }

    pub fn err(&self, msg: &str) {
        self.log.line(&format!("##[error]{msg}"));
    }
}

/// Per-step env files.
pub struct EnvFiles {
    pub output: PathBuf,
    pub env: PathBuf,
    pub path: PathBuf,
    pub summary: PathBuf,
    pub state: PathBuf,
}

impl EnvFiles {
    pub fn create(dir: &Path, n: &str) -> anyhow::Result<EnvFiles> {
        let d = dir.join("_runner_file_commands");
        std::fs::create_dir_all(&d)?;
        let f = |p: &str| -> anyhow::Result<PathBuf> {
            let path = d.join(format!("{p}_{n}"));
            std::fs::write(&path, b"")?;
            Ok(path)
        };
        Ok(EnvFiles { output: f("set_output")?, env: f("set_env")?, path: f("add_path")?, summary: f("step_summary")?, state: f("save_state")? })
    }
}

/// Parses `k=v` and `k<<DELIM ... DELIM` lines of GITHUB_OUTPUT / GITHUB_ENV.
pub fn parse_env_file(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        let heredoc = line.find("<<").filter(|h| line.find('=').is_none_or(|e| *h < e));
        if let Some(h) = heredoc {
            let key = line[..h].to_string();
            let delim = line[h + 2..].trim().to_string();
            if delim.is_empty() {
                return Err(format!("invalid heredoc delimiter in line: {line}"));
            }
            let mut val: Vec<&str> = Vec::new();
            let mut closed = false;
            for l in lines.by_ref() {
                if l == delim {
                    closed = true;
                    break;
                }
                val.push(l);
            }
            if !closed {
                return Err(format!("matching delimiter not found '{delim}'"));
            }
            out.push((key, val.join("\n")));
        } else if let Some((k, v)) = line.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        } else {
            return Err(format!("invalid format '{line}'"));
        }
    }
    Ok(out)
}

/// Handles `::command::` lines; returns the text to log (or None to suppress).
fn workflow_command(job: &Job, line: &str, outputs: &mut Map<String, Value>) -> Option<String> {
    let rest = line.strip_prefix("::")?;
    let (head, msg) = rest.split_once("::")?;
    let (cmd, props) = head.split_once(' ').unwrap_or((head, ""));
    let unescape = |s: &str| s.replace("%0A", "\n").replace("%0D", "\r").replace("%25", "%");
    match cmd {
        "add-mask" => {
            job.log.add_mask(&unescape(msg));
            None
        }
        "set-output" => {
            let name = props.trim().strip_prefix("name=").unwrap_or("").to_string();
            if !name.is_empty() {
                outputs.insert(name, Value::String(unescape(msg)));
            }
            Some(format!("##[warning]The `set-output` command is deprecated. Use environment files instead."))
        }
        "group" => Some(format!("##[group]{msg}")),
        "endgroup" => Some("##[endgroup]".into()),
        "error" | "warning" | "notice" => Some(format!("##[{cmd}]{}", unescape(msg))),
        "debug" => {
            if job.contexts.pointer("/secrets/ACTIONS_STEP_DEBUG").and_then(Value::as_str) == Some("true") {
                Some(format!("##[debug]{}", unescape(msg)))
            } else {
                None
            }
        }
        "save-state" | "echo" | "stop-commands" | "add-matcher" | "remove-matcher" => None,
        _ => Some(line.to_string()),
    }
}

/// Runs a process, streaming stdout/stderr into the log. Returns Ok(exit success).
pub async fn run_process(job: &Job, mut cmd: tokio::process::Command, timeout: Option<Duration>, outputs: &mut Map<String, Value>) -> anyhow::Result<bool> {
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(out);
            let mut buf = Vec::new();
            while let Ok(n) = r.read_until(b'\n', &mut buf).await {
                if n == 0 {
                    break;
                }
                let _ = tx.send(String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string());
                buf.clear();
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(err);
            let mut buf = Vec::new();
            while let Ok(n) = r.read_until(b'\n', &mut buf).await {
                if n == 0 {
                    break;
                }
                let _ = tx.send(String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string());
                buf.clear();
            }
        });
    }
    drop(tx);
    let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
    let kill = |pid: Option<u32>| {
        #[cfg(unix)]
        if let Some(p) = pid {
            unsafe {
                libc::kill(-(p as i32), libc::SIGKILL);
            }
        }
    };
    let mut stream_open = true;
    let status = loop {
        let sleep = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            line = rx.recv(), if stream_open => match line {
                Some(l) => {
                    if let Some(t) = workflow_command(job, &l, outputs) {
                        job.log.line(&t);
                    } else if !l.starts_with("::") {
                        job.log.line(&l);
                    }
                }
                None => stream_open = false,
            },
            st = child.wait(), if !stream_open => break st?,
            _ = sleep => {
                job.timed_out.store(true, Ordering::SeqCst);
                kill(pid);
                let _ = child.kill().await;
                anyhow::bail!("timeout");
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if job.cancel.load(Ordering::SeqCst) {
                    kill(pid);
                    let _ = child.kill().await;
                    if let Some(c) = &job.container {
                        let _ = run_cmd(tokio::process::Command::new("docker").args(["exec", c, "sh", "-c", "kill -9 -1"])).await;
                    }
                    anyhow::bail!("cancelled");
                }
            }
        }
    };
    // drain anything the readers still hold
    while let Ok(l) = rx.try_recv() {
        job.log.line(&l);
    }
    if !status.success() {
        let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
        job.err(&format!("Process completed with exit code {code}."));
    }
    Ok(status.success())
}

/// Shell command line for a `run:` step: (program, args before the script path, extension).
fn shell_command(shell: &str, has_bash: bool) -> Result<(Vec<String>, String), String> {
    let s = shell.trim();
    let (tmpl, ext): (String, &str) = match s {
        "" if has_bash => ("bash --noprofile --norc -eo pipefail {0}".into(), "sh"),
        "" | "sh" => ("sh -e {0}".into(), "sh"),
        "bash" => ("bash --noprofile --norc -eo pipefail {0}".into(), "sh"),
        "python" => ("python3 {0}".into(), "py"),
        "pwsh" => ("pwsh -command \". '{0}'\"".into(), "ps1"),
        "powershell" => ("powershell -command \". '{0}'\"".into(), "ps1"),
        "cmd" => return Err("shell 'cmd' is not supported on this runner".into()),
        custom if custom.contains("{0}") => (custom.to_string(), "sh"),
        other => return Err(format!("Invalid shell option '{other}'. Shell must be a valid built-in (bash, sh, python, pwsh) or a format string containing '{{0}}'")),
    };
    let parts: Vec<String> = shell_words(&tmpl);
    Ok((parts, ext.to_string()))
}

/// Splits a command template on whitespace, honouring simple quotes.
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl Job {
    /// Starts the job container (docker executor, or a job with `container:`).
    pub async fn start_container(&mut self) -> anyhow::Result<()> {
        let image = match self.def().get("container") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Object(o)) => o.get("image").map(expr::to_string),
            _ => None,
        };
        let image = match (image, self.executor) {
            (Some(i), _) => {
                let c = self.ctx_value(&BTreeMap::new());
                expr::interpolate(&self.eval_ctx(&c), &i).map_err(|e| anyhow::anyhow!(e))?
            }
            (None, Executor::Docker) => image_for_labels(&self.msg["labels"].as_array().map(|a| a.iter().map(expr::to_string).collect::<Vec<_>>()).unwrap_or_default()),
            (None, Executor::Shell) => return Ok(()),
        };
        if which("docker").is_none() {
            anyhow::bail!("this job needs a container ({image}) but docker is not available on runner '{}'", self.runner_name);
        }
        let name = format!("forge-job-{}-{}", self.api.job_id, crate::auth::random_string(5).to_lowercase());
        self.log.line(&format!("##[group]Starting job container {image}"));
        let (ok, out) = run_cmd(tokio::process::Command::new("docker").args(["pull", "-q", &image])).await?;
        self.log.line(out.trim());
        if !ok {
            // a locally built image may not be pullable
            let (present, _) = run_cmd(tokio::process::Command::new("docker").args(["image", "inspect", &image])).await?;
            if !present {
                anyhow::bail!("could not pull image {image}");
            }
        }
        let dir = self.dir.to_string_lossy().to_string();
        let mut args: Vec<String> = vec!["run".into(), "-d".into(), "--name".into(), name.clone(), "-v".into(), format!("{dir}:{dir}"), "-w".into(), self.workspace.to_string_lossy().into(), "--entrypoint".into(), "tail".into()];
        if let Some(Value::Object(o)) = self.def().get("container") {
            if let Some(Value::Object(env)) = o.get("env") {
                for (k, v) in env {
                    args.push("-e".into());
                    args.push(format!("{k}={}", expr::to_string(v)));
                }
            }
        }
        args.push(image.clone());
        args.extend(["-f".into(), "/dev/null".into()]);
        let (ok, out) = run_cmd(tokio::process::Command::new("docker").args(&args)).await?;
        if !ok {
            anyhow::bail!("docker run failed: {}", out.trim());
        }
        self.log.line(&format!("Container {name} started"));
        self.log.line("##[endgroup]");
        self.contexts["job"]["container"]["id"] = json!(name);
        self.container = Some(name);
        Ok(())
    }

    pub async fn stop_container(&mut self) {
        if let Some(c) = self.container.take() {
            let _ = run_cmd(tokio::process::Command::new("docker").args(["rm", "-f", &c])).await;
        }
    }

    async fn container_has(&self, bin: &str) -> bool {
        match &self.container {
            Some(c) => run_cmd(tokio::process::Command::new("docker").args(["exec", c, "sh", "-c", &format!("command -v {bin}")])).await.map(|(ok, _)| ok).unwrap_or(false),
            None => which(bin).is_some(),
        }
    }

    /// Runs a `run:` script. `defaults` is the effective `defaults.run`.
    pub async fn run_script(&self, script: &str, shell: &str, workdir: &Path, env: &BTreeMap<String, String>, timeout: Option<Duration>, action: &str) -> anyhow::Result<(bool, Map<String, Value>, EnvFiles)> {
        let has_bash = self.container_has("bash").await;
        let (parts, ext) = shell_command(shell, has_bash).map_err(|e| anyhow::anyhow!(e))?;
        let id = crate::auth::random_string(10).to_lowercase();
        let script_path = self.temp.join(format!("{id}.{ext}"));
        std::fs::write(&script_path, script)?;
        let files = EnvFiles::create(&self.temp, &id)?;
        let penv = self.process_env(env, &files, action);
        let sp = script_path.to_string_lossy().to_string();
        let argv: Vec<String> = parts.iter().map(|p| p.replace("{0}", &sp)).collect();
        self.log.line(&format!("##[group]Run {}", script.lines().next().unwrap_or("")));
        for l in script.lines().take(20) {
            self.log.line(l);
        }
        self.log.line(&format!("shell: {}", parts.join(" ")));
        self.log.line("##[endgroup]");
        if !workdir.exists() {
            anyhow::bail!("working directory {} does not exist", workdir.display());
        }
        let cmd = match &self.container {
            Some(c) => {
                let mut cmd = tokio::process::Command::new("docker");
                cmd.args(["exec", "-i", "-w", &workdir.to_string_lossy()]);
                for (k, v) in &penv {
                    if k == "SHELL" || k == "USER" || k == "LOGNAME" || k == "DOCKER_HOST" {
                        continue;
                    }
                    cmd.arg("-e").arg(format!("{k}={v}"));
                }
                cmd.arg(c).args(&argv);
                cmd
            }
            None => {
                let mut cmd = tokio::process::Command::new(&argv[0]);
                cmd.args(&argv[1..]).env_clear().envs(&penv).current_dir(workdir);
                cmd
            }
        };
        let mut outputs = Map::new();
        let ok = run_process(self, cmd, timeout, &mut outputs).await?;
        Ok((ok, outputs, files))
    }

    /// Applies a finished step's env files: outputs, env, path, summary.
    pub fn apply_files(&mut self, files: &EnvFiles, outputs: &mut Map<String, Value>) -> Result<(), String> {
        let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
        for (k, v) in parse_env_file(&read(&files.output))? {
            outputs.insert(k, Value::String(v));
        }
        for (k, v) in parse_env_file(&read(&files.env))? {
            if k == "NODE_OPTIONS" {
                continue;
            }
            self.env_added.insert(k, v);
        }
        for l in read(&files.path).lines().map(str::trim).filter(|l| !l.is_empty()) {
            self.path_added.insert(0, l.to_string());
        }
        self.summary.push_str(&read(&files.summary));
        Ok(())
    }
}
