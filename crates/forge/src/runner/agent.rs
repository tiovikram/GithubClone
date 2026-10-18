//! `forge runner`: a separate process (a Deployment in Kubernetes) that registers with forge,
//! long-polls for jobs and runs them. It only talks HTTP to forge (no database or object
//! storage credentials), so untrusted job code never runs inside the API server.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::builtin::{self, Scope};
use super::exec::{Api, Executor, Job};
use super::expr::{self, Ctx, Status};

#[derive(clap::Args, Debug, Clone)]
pub struct RunnerArgs {
    /// forge base URL as reachable from the runner (env FORGE_RUNNER_URL)
    #[arg(long, env = "FORGE_RUNNER_URL")]
    pub url: Option<String>,
    /// registration token (repository, organization or enterprise; env FORGE_RUNNER_TOKEN)
    #[arg(long, env = "FORGE_RUNNER_TOKEN")]
    pub token: Option<String>,
    /// runner name (default: host name)
    #[arg(long, env = "FORGE_RUNNER_NAME")]
    pub name: Option<String>,
    /// extra labels, comma separated (self-hosted, OS and architecture are implicit)
    #[arg(long, env = "FORGE_RUNNER_LABELS", default_value = "")]
    pub labels: String,
    /// `shell` (default; the host/pod is the sandbox) or `docker`
    #[arg(long, env = "FORGE_RUNNER_EXECUTOR", default_value = "shell")]
    pub executor: String,
    /// directory for job work directories
    #[arg(long, env = "FORGE_RUNNER_WORKDIR")]
    pub work_dir: Option<PathBuf>,
    /// file holding the runner credential (written at registration, reused on restart)
    #[arg(long, env = "FORGE_RUNNER_CONFIG")]
    pub config: Option<PathBuf>,
    /// base64 JIT configuration from generate-jitconfig (runs one job, then exits)
    #[arg(long, env = "FORGE_RUNNER_JITCONFIG")]
    pub jitconfig: Option<String>,
    /// register as ephemeral: take one job, then exit (the server removes the runner)
    #[arg(long, env = "FORGE_RUNNER_EPHEMERAL")]
    pub ephemeral: bool,
    /// exit after one job
    #[arg(long)]
    pub once: bool,
    /// number of jobs to run concurrently
    #[arg(long, env = "FORGE_RUNNER_SLOTS", default_value_t = 1)]
    pub slots: usize,
    /// unregister with this remove token and exit
    #[arg(long)]
    pub remove_token: Option<String>,
    /// keep job directories after completion (debugging)
    #[arg(long, env = "FORGE_RUNNER_KEEP")]
    pub keep: bool,
}

#[derive(Clone, Debug)]
struct Registered {
    url: String,
    name: String,
    credential: String,
    once: bool,
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| format!("forge-runner-{}", crate::auth::random_string(6).to_lowercase()))
}

async fn register(http: &reqwest::Client, a: &RunnerArgs) -> anyhow::Result<Registered> {
    if let Some(j) = &a.jitconfig {
        let raw = base64::engine::general_purpose::STANDARD.decode(j.trim())?;
        let v: Value = serde_json::from_slice(&raw)?;
        let url = a.url.clone().or_else(|| v["url"].as_str().map(str::to_string)).ok_or_else(|| anyhow::anyhow!("jitconfig has no url"))?;
        return Ok(Registered {
            url: url.trim_end_matches('/').to_string(),
            name: v["name"].as_str().unwrap_or("jit").to_string(),
            credential: v["credential"].as_str().unwrap_or("").to_string(),
            once: true,
        });
    }
    let url = a.url.clone().ok_or_else(|| anyhow::anyhow!("--url (FORGE_RUNNER_URL) is required"))?.trim_end_matches('/').to_string();
    let name = a.name.clone().unwrap_or_else(hostname);
    if let Some(cfg) = &a.config {
        if let Ok(text) = std::fs::read_to_string(cfg) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v["url"].as_str() == Some(url.as_str()) && v["credential"].as_str().is_some() {
                    return Ok(Registered { url, name: v["name"].as_str().unwrap_or(&name).to_string(), credential: v["credential"].as_str().unwrap().into(), once: a.once || a.ephemeral });
                }
            }
        }
    }
    let token = a.token.clone().ok_or_else(|| anyhow::anyhow!("--token (FORGE_RUNNER_TOKEN, a registration token) is required"))?;
    let labels: Vec<String> = a.labels.split(',').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let body = json!({ "token": token, "name": name, "labels": labels, "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
                       "version": env!("CARGO_PKG_VERSION"), "ephemeral": a.ephemeral });
    let r = http.post(format!("{url}/api/forge/runner/register")).json(&body).send().await?;
    let st = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if !st.is_success() {
        anyhow::bail!("registration failed: HTTP {st}: {}", v["message"].as_str().unwrap_or(""));
    }
    let reg = Registered { url: url.clone(), name: v["name"].as_str().unwrap_or(&name).to_string(), credential: v["credential"].as_str().unwrap_or("").to_string(), once: a.once || a.ephemeral };
    if let Some(cfg) = &a.config {
        let _ = std::fs::write(cfg, json!({ "url": url, "name": reg.name, "id": v["id"], "credential": reg.credential }).to_string());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(cfg, std::fs::Permissions::from_mode(0o600));
        }
    }
    tracing::info!(runner = %reg.name, id = %v["id"], labels = %v["labels"], "registered");
    Ok(reg)
}

/// Entry point of `forge runner`.
pub async fn main(a: RunnerArgs) -> anyhow::Result<()> {
    let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build()?;
    let executor = match a.executor.as_str() {
        "docker" => Executor::Docker,
        "shell" => Executor::Shell,
        other => anyhow::bail!("unknown executor {other} (shell or docker)"),
    };
    let reg = register(&http, &a).await?;
    if let Some(rt) = &a.remove_token {
        let r = http.post(format!("{}/api/forge/runner/remove", reg.url)).bearer_auth(&reg.credential).json(&json!({ "token": rt })).send().await?;
        if !r.status().is_success() {
            anyhow::bail!("remove failed: HTTP {}", r.status());
        }
        if let Some(cfg) = &a.config {
            let _ = std::fs::remove_file(cfg);
        }
        println!("runner {} removed", reg.name);
        return Ok(());
    }
    let work = a.work_dir.clone().unwrap_or_else(|| std::env::temp_dir().join("forge-runner"));
    std::fs::create_dir_all(&work)?;
    let work = work.canonicalize().unwrap_or(work);
    tracing::info!(runner = %reg.name, url = %reg.url, executor = %a.executor, work = %work.display(), "waiting for jobs");
    let slots = if reg.once { 1 } else { a.slots.max(1) };
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..slots {
        let (http, reg, work, keep) = (http.clone(), reg.clone(), work.clone(), a.keep);
        set.spawn(async move { worker(http, reg, executor, work, keep).await });
    }
    // SIGTERM/ctrl-c: stop taking jobs, let running jobs finish (FORGE_RUNNER_DRAIN_SECS).
    let term = async {
        #[cfg(unix)]
        if let Ok(mut t) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            tokio::select! { _ = t.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            return;
        }
        let _ = tokio::signal::ctrl_c().await;
    };
    tokio::select! {
        r = set.join_next() => {
            if let Some(Ok(Err(e))) = r { return Err(e); }
            while let Some(r) = set.join_next().await {
                if let Ok(Err(e)) = r { return Err(e); }
            }
        }
        _ = term => {
            DRAINING.store(true, std::sync::atomic::Ordering::SeqCst);
            let secs = std::env::var("FORGE_RUNNER_DRAIN_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(600u64);
            tracing::info!(secs, "shutting down: finishing running jobs");
            let _ = tokio::time::timeout(Duration::from_secs(secs), async { while set.join_next().await.is_some() {} }).await;
        }
    }
    Ok(())
}

/// Set on SIGTERM: workers finish their current job and stop polling.
static DRAINING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn worker(http: reqwest::Client, reg: Registered, executor: Executor, work: PathBuf, keep: bool) -> anyhow::Result<()> {
    let mut backoff = 1u64;
    loop {
        if DRAINING.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let r = http
            .post(format!("{}/api/forge/runner/acquire", reg.url))
            .bearer_auth(&reg.credential)
            .json(&json!({ "wait": 25 }))
            .timeout(Duration::from_secs(60))
            .send()
            .await;
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "poll failed");
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
                continue;
            }
        };
        backoff = 1;
        match r.status().as_u16() {
            204 => continue,
            200 => {}
            401 | 403 => anyhow::bail!("the runner credential was rejected (runner deleted?)"),
            s => {
                tracing::warn!(status = s, "unexpected poll response");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }
        let msg: Value = r.json().await?;
        let job_id = msg["job_id"].as_i64().unwrap_or(0);
        tracing::info!(job = job_id, name = %msg["name"], repo = %msg["repository"]["full_name"], "running job");
        let api = Api { http: http.clone(), base: reg.url.clone(), credential: reg.credential.clone(), token: msg["token"].as_str().unwrap_or("").to_string(), job_id };
        let concl = run_job(api, msg, executor, &work, &reg.name, keep).await;
        tracing::info!(job = job_id, conclusion = %concl, "job finished");
        if reg.once {
            return Ok(());
        }
    }
}

/// Runs one job end to end and reports it; returns the conclusion.
pub async fn run_job(api: Api, msg: Value, executor: Executor, work: &std::path::Path, runner_name: &str, keep: bool) -> String {
    let (mut job, uploader) = match Job::new(api.clone(), msg, executor, work, runner_name) {
        Ok(x) => x,
        Err(e) => {
            let _ = api.post_json("/complete", &json!({ "conclusion": "failure", "steps": [] })).await;
            tracing::error!(error = %e, "could not prepare job directory");
            return "failure".into();
        }
    };
    // heartbeat: progress of steps, cancellation
    let hb = {
        let (api, reports, cancel) = (api.clone(), job.reports.clone(), job.cancel.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let steps: Vec<Value> = reports.lock().unwrap().iter().map(|s| json!({ "number": s.number, "name": s.name, "status": s.status,
                    "conclusion": s.conclusion, "started_at": s.started_at, "completed_at": s.completed_at })).collect();
                if let Ok(v) = api.post_json("/heartbeat", &json!({ "steps": steps })).await {
                    if v["cancel"].as_bool() == Some(true) {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
            }
        })
    };
    let timeout_min = job.msg["timeout_minutes"].as_f64().unwrap_or(360.0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout_min * 60.0);
    // Set up job
    let n = job.begin_step("Set up job");
    job.log.line(&format!("Current runner version: '{}'", env!("CARGO_PKG_VERSION")));
    job.log.line(&format!("Runner name: '{runner_name}'"));
    job.log.line(&format!("Machine name: '{}'", hostname()));
    job.log.line(&format!("Job: {} ({})", job.msg["name"].as_str().unwrap_or(""), job.msg.pointer("/workflow/path").and_then(Value::as_str).unwrap_or("")));
    job.log.line(&format!("Executor: {}", if executor == Executor::Docker { "docker" } else { "shell" }));
    let mut setup_ok = true;
    let event_dir = job.temp.join("_github_workflow");
    let _ = std::fs::create_dir_all(&event_dir);
    let _ = std::fs::write(event_dir.join("event.json"), serde_json::to_vec_pretty(&job.contexts["github"]["event"]).unwrap_or_default());
    if job.def().get("services").is_some_and(|s| s.as_object().is_some_and(|m| !m.is_empty())) {
        job.err("Service containers (jobs.<id>.services) are not supported by forge runner");
        setup_ok = false;
    }
    if setup_ok {
        if let Err(e) = job.start_container().await {
            job.err(&format!("{e:#}"));
            setup_ok = false;
        }
    }
    job.end_step(n, if setup_ok { "success" } else { "failure" });
    if !setup_ok {
        job.status = Status::Failure;
    } else {
        let defaults = |k: &str| -> String {
            job.def()
                .pointer(&format!("/defaults/run/{k}"))
                .or_else(|| job.msg.pointer(&format!("/workflow/defaults/run/{k}")))
                .map(expr::to_string)
                .unwrap_or_default()
        };
        let scope = Scope { depth: 0, shell: defaults("shell"), working_directory: defaults("working-directory"), action_path: None, report: true };
        let steps: Vec<Value> = job.def().get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
        builtin::run_steps(&mut job, &steps, &scope, Some(deadline)).await;
        builtin::run_post_steps(&mut job).await;
    }
    if job.timed_out.load(Ordering::SeqCst) {
        job.err(&format!("The job running on runner {runner_name} has exceeded the maximum execution time of {timeout_min} minutes."));
        job.status = Status::Failure;
    }
    // job outputs
    let mut outputs = Map::new();
    if let Some(Value::Object(o)) = job.def().get("outputs") {
        let env = job.job_env();
        let c = job.ctx_value(&env);
        let ctx = Ctx { contexts: &c, status: job.current_status(), workspace: Some(&job.workspace) };
        for (k, v) in o {
            let val = expr::interpolate(&ctx, &expr::to_string(v)).unwrap_or_default();
            // GitHub drops outputs that contain secrets
            if job.log.mask(&val) != val {
                job.log.line(&format!("##[warning]Skip output '{k}' since it may contain secret."));
                continue;
            }
            outputs.insert(k.clone(), Value::String(val));
        }
    }
    let n = job.begin_step("Complete job");
    if !job.summary.is_empty() {
        job.log.line("Job summary:");
        let summary = job.summary.clone();
        job.log.line(&summary);
    }
    job.stop_container().await;
    let conclusion = if job.cancel.load(Ordering::SeqCst) && !job.timed_out.load(Ordering::SeqCst) {
        "cancelled"
    } else {
        match job.status {
            Status::Success => "success",
            Status::Failure => "failure",
            Status::Cancelled => "cancelled",
        }
    };
    job.log.line("Cleaning up orphan processes");
    job.end_step(n, "success");
    hb.abort();
    // flush logs before completing (the job token stops working afterwards)
    let steps = job.steps_json();
    let annotations = job.annotations.lock().unwrap().clone();
    let dir = job.dir.clone();
    drop(job);
    let _ = uploader.await;
    for _ in 0..5 {
        match api.post_json("/complete", &json!({ "conclusion": conclusion, "outputs": outputs, "steps": steps, "annotations": annotations })).await {
            Ok(_) => break,
            Err(e) => {
                tracing::warn!(error = %e, "could not report completion; retrying");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    if !keep {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let _: BTreeMap<String, String> = BTreeMap::new();
    conclusion.to_string()
}
