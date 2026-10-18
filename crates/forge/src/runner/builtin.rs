//! Steps: the step runner (job steps and composite action steps), the built-in actions
//! (`actions/checkout`, `actions/upload-artifact`, `actions/download-artifact`,
//! `actions/cache[/restore|/save]`) and composite actions from repositories on this forge.
//! Other actions fail with a clear message: forge never downloads actions from github.com.
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};

use super::exec::{run_cmd, Job, PostStep, StepResult};
use super::expr::{self, Ctx, Status};

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

fn s(v: Option<&Value>) -> String {
    v.map(expr::to_string).unwrap_or_default()
}

fn input(with: &Map<String, Value>, k: &str) -> String {
    with.iter().find(|(kk, _)| kk.eq_ignore_ascii_case(k)).map(|(_, v)| expr::to_string(v)).unwrap_or_default()
}

fn truthy_input(with: &Map<String, Value>, k: &str, default: bool) -> bool {
    match input(with, k).trim().to_ascii_lowercase().as_str() {
        "" => default,
        "true" => true,
        _ => false,
    }
}

/// What a step runs in: the job itself or a composite action.
pub struct Scope {
    pub depth: usize,
    /// `defaults.run` (shell, working-directory)
    pub shell: String,
    pub working_directory: String,
    /// composite: the action directory (GITHUB_ACTION_PATH)
    pub action_path: Option<PathBuf>,
    /// report each step in the job's step list (top level only)
    pub report: bool,
}

/// Runs a list of steps. Returns the scope's final status.
pub fn run_steps<'a>(job: &'a mut Job, steps: &'a [Value], scope: &'a Scope, deadline: Option<tokio::time::Instant>) -> BoxFut<'a, ()> {
    Box::pin(async move {
        for (i, step) in steps.iter().enumerate() {
            let res = run_step(job, step, i, scope, deadline).await;
            let id = step.get("id").map(expr::to_string).unwrap_or_else(|| format!("__run_{i}"));
            job.steps_ctx.insert(id, json!({ "outputs": res.0.outputs, "outcome": res.0.outcome, "conclusion": res.1 }));
        }
    })
}

fn step_name(job: &Job, step: &Value, c: &Value) -> String {
    if let Some(n) = step.get("name") {
        let raw = expr::to_string(n);
        return expr::interpolate(&Ctx { contexts: c, status: job.current_status(), workspace: Some(&job.workspace) }, &raw).unwrap_or(raw);
    }
    if let Some(r) = step.get("run") {
        return format!("Run {}", expr::to_string(r).lines().next().unwrap_or("").trim());
    }
    format!("Run {}", s(step.get("uses")))
}

/// Runs one step: returns (result, conclusion).
async fn run_step(job: &mut Job, step: &Value, idx: usize, scope: &Scope, deadline: Option<tokio::time::Instant>) -> (StepResult, String) {
    let mut env = job.job_env();
    if let Some(dir) = &scope.action_path {
        env.insert("GITHUB_ACTION_PATH".into(), dir.to_string_lossy().into());
    }
    let base = job.ctx_value(&env);
    if let Some(Value::Object(m)) = step.get("env") {
        let mut c = base.clone();
        for (k, v) in m {
            c["env"] = json!(env);
            let ctx = Ctx { contexts: &c, status: job.current_status(), workspace: Some(&job.workspace) };
            let val = expr::interpolate(&ctx, &expr::to_string(v)).unwrap_or_else(|_| expr::to_string(v));
            env.insert(k.clone(), val);
        }
    }
    let c = job.ctx_value(&env);
    let name = step_name(job, step, &c);
    let status = job.current_status();
    let ctx = Ctx { contexts: &c, status, workspace: Some(&job.workspace) };
    let run_it = match expr::eval_if(&ctx, step.get("if")) {
        Ok(b) => b,
        Err(e) => {
            let n = if scope.report { job.begin_step(&name) } else { 0 };
            job.err(&format!("The step's 'if' expression is invalid: {e}"));
            job.status = Status::Failure;
            if scope.report {
                job.end_step(n, "failure");
            }
            return (StepResult { outcome: "failure".into(), outputs: Map::new() }, "failure".into());
        }
    };
    if !run_it {
        if scope.report {
            let n = job.begin_step(&name);
            job.end_step(n, "skipped");
        }
        return (StepResult { outcome: "skipped".into(), outputs: Map::new() }, "skipped".into());
    }
    let n = if scope.report { job.begin_step(&name) } else { 0 };
    let continue_on_error = match step.get("continue-on-error") {
        Some(Value::Bool(b)) => *b,
        Some(v) => expr::truthy(&expr::interpolate_value(&ctx, v).unwrap_or(Value::Bool(false))) && expr::to_string(v) != "false",
        None => false,
    };
    let step_timeout = step.get("timeout-minutes").and_then(|v| expr::to_string(v).parse::<f64>().ok()).map(|m| Duration::from_secs_f64(m * 60.0));
    let remaining = deadline.map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
    let timeout = match (step_timeout, remaining) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let result = if let Some(run) = step.get("run") {
        run_run_step(job, step, &expr::to_string(run), &env, &c, scope, timeout, idx).await
    } else if let Some(uses) = step.get("uses") {
        let with = match step.get("with") {
            Some(Value::Object(m)) => {
                let mut o = Map::new();
                for (k, v) in m {
                    o.insert(k.clone(), Value::String(expr::interpolate(&ctx, &expr::to_string(v)).unwrap_or_else(|_| expr::to_string(v))));
                }
                o
            }
            _ => Map::new(),
        };
        run_uses(job, &expr::to_string(uses), with, &env, scope, deadline, &name).await
    } else {
        Err("Step must have either 'run' or 'uses'".to_string())
    };
    let (outcome, outputs) = match result {
        Ok(o) => ("success".to_string(), o),
        Err(e) => {
            let cancelled = job.cancel.load(Ordering::SeqCst) && !job.timed_out.load(Ordering::SeqCst);
            if e == "cancelled" || cancelled {
                job.err("The operation was canceled.");
                ("cancelled".to_string(), Map::new())
            } else if e == "timeout" {
                let mins = timeout.map(|t| (t.as_secs_f64() / 60.0).round() as i64).unwrap_or(0);
                job.err(&format!("The action '{name}' has timed out after {mins} minutes."));
                ("failure".to_string(), Map::new())
            } else {
                if !e.is_empty() {
                    job.err(&e);
                }
                ("failure".to_string(), Map::new())
            }
        }
    };
    let conclusion = if outcome == "failure" && continue_on_error { "success".to_string() } else { outcome.clone() };
    match conclusion.as_str() {
        "failure" => job.status = Status::Failure,
        "cancelled" => job.status = Status::Cancelled,
        _ => {}
    }
    // a step timeout that exhausted the job's time
    if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
        job.timed_out.store(true, Ordering::SeqCst);
        job.cancel.store(true, Ordering::SeqCst);
    }
    if scope.report {
        job.end_step(n, &conclusion);
    }
    (StepResult { outcome, outputs }, conclusion)
}

fn resolve_dir(job: &Job, wd: &str) -> PathBuf {
    if wd.is_empty() {
        job.workspace.clone()
    } else if Path::new(wd).is_absolute() {
        PathBuf::from(wd)
    } else {
        job.workspace.join(wd)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_run_step(job: &mut Job, step: &Value, run: &str, env: &BTreeMap<String, String>, c: &Value, scope: &Scope, timeout: Option<Duration>, idx: usize) -> Result<Map<String, Value>, String> {
    let ctx = Ctx { contexts: c, status: job.current_status(), workspace: Some(&job.workspace) };
    let script = expr::interpolate(&ctx, run).map_err(|e| format!("Invalid expression in 'run': {e}"))?;
    let shell = step.get("shell").map(expr::to_string).unwrap_or_else(|| scope.shell.clone());
    let shell = expr::interpolate(&ctx, &shell).unwrap_or(shell);
    if scope.action_path.is_some() && step.get("shell").is_none() && scope.shell.is_empty() {
        return Err("Required property is missing: shell (composite action steps must set 'shell')".into());
    }
    let wd = step.get("working-directory").map(expr::to_string).unwrap_or_else(|| scope.working_directory.clone());
    let wd = expr::interpolate(&ctx, &wd).unwrap_or(wd);
    let dir = resolve_dir(job, &wd);
    let action = step.get("id").map(expr::to_string).unwrap_or_else(|| format!("__run_{idx}"));
    let (ok, mut outputs, files) = job.run_script(&script, &shell, &dir, env, timeout, &action).await.map_err(|e| format!("{e:#}"))?;
    job.apply_files(&files, &mut outputs).map_err(|e| format!("Unable to process file command: {e}"))?;
    if ok {
        Ok(outputs)
    } else {
        Err(String::new())
    }
}

// ---------------------------------------------------------------------------------------------
// `uses:`

/// Splits `owner/repo[/path]@ref`.
fn parse_uses(uses: &str) -> Option<(String, String, String, String)> {
    let (left, git_ref) = uses.rsplit_once('@')?;
    let mut parts = left.splitn(3, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    let path = parts.next().unwrap_or("").to_string();
    if owner.is_empty() || repo.is_empty() || git_ref.is_empty() {
        return None;
    }
    Some((owner, repo, path, git_ref.to_string()))
}

async fn run_uses(job: &mut Job, uses: &str, with: Map<String, Value>, env: &BTreeMap<String, String>, scope: &Scope, deadline: Option<tokio::time::Instant>, name: &str) -> Result<Map<String, Value>, String> {
    if scope.depth > 9 {
        return Err("Composite actions are nested too deeply (limit 10)".into());
    }
    if uses.starts_with("docker://") {
        return Err(format!("'{uses}': docker container actions are not supported by forge runner"));
    }
    if let Some(local) = uses.strip_prefix("./") {
        let dir = job.workspace.join(local);
        return run_action_dir(job, &dir, uses, with, env, scope, deadline).await;
    }
    let Some((owner, repo, path, git_ref)) = parse_uses(uses) else {
        return Err(format!("Invalid action reference '{uses}': expected owner/repo[/path]@ref, ./path or docker://image"));
    };
    let key = format!("{}/{}{}", owner.to_lowercase(), repo.to_lowercase(), if path.is_empty() { String::new() } else { format!("/{}", path.to_lowercase()) });
    job.log.line(&format!("##[group]Run {uses}"));
    for (k, v) in &with {
        job.log.line(&format!("  {k}: {}", expr::to_string(v)));
    }
    job.log.line("##[endgroup]");
    match key.as_str() {
        "actions/checkout" => return checkout(job, &with).await,
        "actions/upload-artifact" => return upload_artifact(job, &with).await,
        "actions/download-artifact" => return download_artifact(job, &with).await,
        "actions/cache" => return cache_restore(job, &with, true, name).await,
        "actions/cache/restore" => return cache_restore(job, &with, false, name).await,
        "actions/cache/save" => return cache_save(job, &with).await,
        _ => {}
    }
    // A repository on this forge (e.g. a mirror of the action).
    let dir = job.dir.join("_actions").join(&owner).join(&repo).join(&git_ref);
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let url = format!("{}/{owner}/{repo}.git", job.api.base);
        let fetched = git(job, &dir, &["init", "-q"]).await.is_ok()
            && git(job, &dir, &["-c", &auth_header(job, &job.api.token), "fetch", "-q", "--depth=1", &url, &git_ref]).await.is_ok()
            && git(job, &dir, &["checkout", "-q", "--force", "FETCH_HEAD"]).await.is_ok();
        if !fetched {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!(
                "Unable to resolve action `{uses}`: repository {owner}/{repo} (ref {git_ref}) was not found on this forge. forge runner does not download actions from github.com; mirror the action repository to this forge (only composite actions are supported) or replace the step with a run: script."
            ));
        }
    }
    let adir = if path.is_empty() { dir } else { dir.join(&path) };
    run_action_dir(job, &adir, uses, with, env, scope, deadline).await
}

async fn run_action_dir(job: &mut Job, dir: &Path, uses: &str, with: Map<String, Value>, env: &BTreeMap<String, String>, scope: &Scope, deadline: Option<tokio::time::Instant>) -> Result<Map<String, Value>, String> {
    let text = ["action.yml", "action.yaml"]
        .iter()
        .find_map(|f| std::fs::read_to_string(dir.join(f)).ok())
        .ok_or_else(|| format!("Can't find 'action.yml' or 'action.yaml' for action '{uses}'"))?;
    let action = super::workflow::parse_yaml(&text).map_err(|e| format!("Invalid action.yml of '{uses}': {e}"))?;
    let using = s(action.pointer("/runs/using"));
    if using != "composite" {
        return Err(format!("Action '{uses}' uses runs.using '{using}'; forge runner only runs composite actions (no node or docker actions)"));
    }
    // inputs: given values, else declared defaults (which may contain expressions)
    let c = job.ctx_value(env);
    let ctx = Ctx { contexts: &c, status: job.current_status(), workspace: Some(&job.workspace) };
    let mut inputs = Map::new();
    if let Some(Value::Object(decl)) = action.get("inputs") {
        for (k, spec) in decl {
            let v = with.iter().find(|(kk, _)| kk.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
            let v = match v {
                Some(v) => v,
                None => {
                    let d = s(spec.get("default"));
                    Value::String(expr::interpolate(&ctx, &d).unwrap_or(d))
                }
            };
            inputs.insert(k.clone(), v);
        }
    }
    for (k, v) in &with {
        inputs.entry(k.clone()).or_insert(v.clone());
    }
    let steps: Vec<Value> = action.pointer("/runs/steps").and_then(Value::as_array).cloned().unwrap_or_default();
    // run in a nested scope: own `steps` and `inputs` contexts and status
    let saved_steps = std::mem::take(&mut job.steps_ctx);
    let saved_inputs = std::mem::replace(&mut job.contexts["inputs"], Value::Object(inputs));
    let saved_status = job.status;
    let saved_env = job.env_added.clone();
    let mut env_for_action = env.clone();
    for (k, v) in &job.env_added {
        env_for_action.insert(k.clone(), v.clone());
    }
    let inner = Scope { depth: scope.depth + 1, shell: String::new(), working_directory: String::new(), action_path: Some(dir.to_path_buf()), report: false };
    run_steps(job, &steps, &inner, deadline).await;
    let inner_status = job.status;
    // outputs
    let c = job.ctx_value(&env_for_action);
    let octx = Ctx { contexts: &c, status: job.current_status(), workspace: Some(&job.workspace) };
    let mut outputs = Map::new();
    if let Some(Value::Object(o)) = action.get("outputs") {
        for (k, spec) in o {
            let v = s(spec.get("value"));
            outputs.insert(k.clone(), Value::String(expr::interpolate(&octx, &v).unwrap_or_default()));
        }
    }
    job.steps_ctx = saved_steps;
    job.contexts["inputs"] = saved_inputs;
    job.status = saved_status;
    let _ = saved_env;
    match inner_status {
        Status::Success => Ok(outputs),
        Status::Cancelled => Err("cancelled".into()),
        Status::Failure => Err(String::new()),
    }
}

// ---------------------------------------------------------------------------------------------
// git helpers

fn auth_header(job: &Job, token: &str) -> String {
    let b = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    job.log.add_mask(&b);
    job.log.add_mask(token);
    format!("http.{}/.extraheader=AUTHORIZATION: basic {b}", job.api.base)
}

async fn git(job: &Job, dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(std::env::var("FORGE_GIT_BIN").unwrap_or_else(|_| "git".into()));
    cmd.args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &job.home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    let (ok, out) = run_cmd(&mut cmd).await.map_err(|e| format!("git: {e}"))?;
    let shown: Vec<&str> = args
        .iter()
        .map(|a| if a.contains("extraheader=") { "http.extraheader=***" } else if a.to_ascii_uppercase().starts_with("AUTHORIZATION:") { "***" } else { a })
        .collect();
    job.log.line(&format!("[command]git {}", shown.join(" ")));
    let t = out.trim_end();
    if !t.is_empty() {
        job.log.line(t);
    }
    if ok {
        Ok(out)
    } else {
        Err(format!("git {} failed", shown.first().copied().unwrap_or("")))
    }
}

/// `actions/checkout`: fetches the repository at `ref` (default: the run's commit) into
/// `path`, authenticated with the job token; persists the credential for later git commands.
async fn checkout(job: &mut Job, with: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let gh = job.contexts["github"].clone();
    let own = expr::to_string(&gh["repository"]);
    let repository = Some(input(with, "repository")).filter(|r| !r.is_empty()).unwrap_or_else(|| own.clone());
    let token = Some(input(with, "token")).filter(|t| !t.is_empty()).unwrap_or_else(|| job.api.token.clone());
    let path = input(with, "path");
    let dir = resolve_dir(job, &path);
    let depth: i64 = input(with, "fetch-depth").parse().unwrap_or(1);
    let same = repository.eq_ignore_ascii_case(&own);
    let given_ref = input(with, "ref");
    let (git_ref, sha) = if !given_ref.is_empty() {
        (given_ref.clone(), None)
    } else if same {
        (expr::to_string(&gh["ref"]), Some(expr::to_string(&gh["sha"])))
    } else {
        ("HEAD".to_string(), None)
    };
    if truthy_input(with, "clean", true) && dir.exists() && dir != job.workspace {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let url = format!("{}/{repository}.git", job.api.base);
    job.log.line(&format!("Syncing repository: {repository}"));
    if !dir.join(".git").exists() {
        git(job, &dir, &["init", "-q"]).await?;
        git(job, &dir, &["remote", "add", "origin", &url]).await?;
    }
    let header = auth_header(job, &token);
    let (k, v) = header.split_once('=').unwrap_or((&header, ""));
    git(job, &dir, &["config", "--local", k, v]).await?;
    let depth_arg = format!("--depth={depth}");
    let mut fetch: Vec<&str> = vec!["-c", "protocol.version=2", "fetch", "--no-tags", "--prune", "--no-recurse-submodules"];
    if depth > 0 {
        fetch.push(&depth_arg);
    }
    let is_sha = |r: &str| r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit());
    // branch name for a local branch (GitHub's checkout creates one for branch refs)
    let branch = git_ref.strip_prefix("refs/heads/").map(str::to_string).or_else(|| {
        (!git_ref.starts_with("refs/") && !is_sha(&git_ref) && git_ref != "HEAD" && !given_ref.is_empty()).then(|| git_ref.clone())
    });
    let mut fetched = false;
    if let Some(sha) = sha.as_deref().filter(|s| is_sha(s)) {
        let refspec = match &branch {
            Some(b) => format!("+{sha}:refs/remotes/origin/{b}"),
            None => sha.to_string(),
        };
        let mut a = fetch.clone();
        a.extend(["origin", refspec.as_str()]);
        fetched = git(job, &dir, &a).await.is_ok();
    }
    if !fetched {
        let refspec = if is_sha(&git_ref) {
            git_ref.clone()
        } else if let Some(b) = &branch {
            format!("+refs/heads/{b}:refs/remotes/origin/{b}")
        } else if git_ref.starts_with("refs/") {
            format!("+{git_ref}:{git_ref}")
        } else if git_ref == "HEAD" {
            "HEAD".to_string()
        } else {
            format!("+refs/tags/{git_ref}:refs/tags/{git_ref}")
        };
        let mut a = fetch.clone();
        a.extend(["origin", refspec.as_str()]);
        if git(job, &dir, &a).await.is_err() {
            // e.g. a short sha or an unknown ref: fetch everything
            let mut a = vec!["fetch", "--no-tags", "origin", "+refs/heads/*:refs/remotes/origin/*", "+refs/tags/*:refs/tags/*"];
            if is_sha(&git_ref) || git_ref.len() < 40 {
                a.push("+refs/pull/*:refs/remotes/pull/*");
            }
            git(job, &dir, &a).await.map_err(|_| format!("The repository {repository} or ref '{git_ref}' could not be fetched"))?;
        }
    }
    let target = match (&sha, &branch) {
        (Some(s), _) if is_sha(s) => s.clone(),
        (_, Some(b)) => format!("refs/remotes/origin/{b}"),
        _ if is_sha(&git_ref) => git_ref.clone(),
        _ => "FETCH_HEAD".to_string(),
    };
    match &branch {
        Some(b) => git(job, &dir, &["checkout", "-q", "--force", "-B", b, &target]).await?,
        None => git(job, &dir, &["checkout", "-q", "--force", "--detach", &target]).await?,
    };
    if !truthy_input(with, "persist-credentials", true) {
        let _ = git(job, &dir, &["config", "--local", "--unset-all", k]).await;
    }
    let head = git(job, &dir, &["rev-parse", "HEAD"]).await?.trim().to_string();
    let mut out = Map::new();
    out.insert("ref".into(), json!(git_ref));
    out.insert("commit".into(), json!(head));
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Artifacts

fn has_glob(p: &str) -> bool {
    p.contains(['*', '?', '['])
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&p, out),
            Ok(t) if t.is_file() || t.is_symlink() => out.push(p),
            _ => {}
        }
    }
}

fn hidden(rel: &Path) -> bool {
    rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

/// Files matched by an upload-artifact `path` input and the root they are stored under.
fn collect_files(job: &Job, spec: &str, include_hidden: bool) -> (Vec<PathBuf>, PathBuf) {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut excludes: Vec<regex::Regex> = Vec::new();
    for line in spec.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(ex) = line.strip_prefix('!') {
            let abs = resolve_path(job, ex);
            if let Some(r) = super::glob::to_regex(&abs.to_string_lossy()) {
                excludes.push(r);
            }
            continue;
        }
        let abs = resolve_path(job, line);
        let s = abs.to_string_lossy().to_string();
        if has_glob(&s) {
            let prefix: String = s.split(['*', '?', '[']).next().unwrap_or("").to_string();
            let base = PathBuf::from(prefix.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or(prefix.clone()));
            let re = super::glob::to_regex(&s);
            let mut all = Vec::new();
            walk(&base, &mut all);
            for f in all {
                if re.as_ref().is_some_and(|r| r.is_match(&f.to_string_lossy())) && (include_hidden || !hidden(f.strip_prefix(&base).unwrap_or(&f))) {
                    files.push(f);
                }
            }
            roots.push(base);
        } else if abs.is_dir() {
            let mut all = Vec::new();
            walk(&abs, &mut all);
            files.extend(all.into_iter().filter(|f| include_hidden || !hidden(f.strip_prefix(&abs).unwrap_or(f))));
            roots.push(abs);
        } else if abs.is_file() {
            roots.push(abs.parent().map(Path::to_path_buf).unwrap_or_default());
            files.push(abs);
        }
    }
    files.retain(|f| !excludes.iter().any(|r| r.is_match(&f.to_string_lossy())));
    files.sort();
    files.dedup();
    // least common ancestor of the search roots
    let mut root = roots.first().cloned().unwrap_or_else(|| job.workspace.clone());
    for r in &roots {
        while !r.starts_with(&root) {
            if !root.pop() {
                break;
            }
        }
    }
    (files, root)
}

fn resolve_path(job: &Job, p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        job.home.join(rest)
    } else if p == "~" {
        job.home.clone()
    } else if Path::new(p).is_absolute() {
        PathBuf::from(p)
    } else {
        job.workspace.join(p.strip_prefix("./").unwrap_or(p))
    }
}

async fn upload_artifact(job: &mut Job, with: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let name = Some(input(with, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "artifact".into());
    let spec = input(with, "path");
    if spec.trim().is_empty() {
        return Err("Input required and not supplied: path".into());
    }
    let (files, root) = collect_files(job, &spec, truthy_input(with, "include-hidden-files", false));
    if files.is_empty() {
        let msg = format!("No files were found with the provided path: {}. No artifacts will be uploaded.", spec.trim());
        return match input(with, "if-no-files-found").as_str() {
            "error" => Err(msg),
            "ignore" => {
                job.log.line(&msg);
                Ok(Map::new())
            }
            _ => {
                job.log.line(&format!("##[warning]{msg}"));
                Ok(Map::new())
            }
        };
    }
    let mut z = super::zip::ZipWriter::new();
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        let data = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        z.add(&rel, &data);
    }
    let bytes = z.finish();
    job.log.line(&format!("With the provided path, there will be {} file(s) uploaded", files.len()));
    let retention = input(with, "retention-days");
    let q = format!("/artifacts?name={}{}", urlencode(&name), if retention.is_empty() { String::new() } else { format!("&retention_days={retention}") });
    let (mut st, mut v) = job.api.post_bytes(&q, bytes.clone()).await.map_err(|e| e.to_string())?;
    if st == reqwest::StatusCode::CONFLICT && truthy_input(with, "overwrite", false) {
        delete_artifact_named(job, &name).await?;
        (st, v) = job.api.post_bytes(&q, bytes).await.map_err(|e| e.to_string())?;
    }
    if !st.is_success() {
        let m = v["message"].as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
        return Err(format!("Failed to CreateArtifact: {m} ({st})"));
    }
    let id = v["id"].as_i64().unwrap_or(0);
    job.log.line(&format!("Artifact {name} has been successfully uploaded! Final size is {} bytes. Artifact ID is {id}", v["size"]));
    let gh = &job.contexts["github"];
    let mut out = Map::new();
    out.insert("artifact-id".into(), json!(id.to_string()));
    out.insert("artifact-url".into(), json!(format!("{}/{}/actions/runs/{}/artifacts/{id}", expr::to_string(&gh["server_url"]), expr::to_string(&gh["repository"]), expr::to_string(&gh["run_id"]))));
    out.insert("artifact-digest".into(), json!(v["digest"].as_str().unwrap_or("").trim_start_matches("sha256:")));
    Ok(out)
}

fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

async fn list_artifacts(job: &Job) -> Result<Vec<Value>, String> {
    let r = job.api.get("/artifacts").await.map_err(|e| e.to_string())?;
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    Ok(v["artifacts"].as_array().cloned().unwrap_or_default())
}

async fn delete_artifact_named(job: &Job, name: &str) -> Result<(), String> {
    for a in list_artifacts(job).await? {
        if a["name"].as_str() == Some(name) {
            let url = format!("{}/api/v3/repos/{}/actions/artifacts/{}", job.api.base, expr::to_string(&job.contexts["github"]["repository"]), a["id"]);
            let r = job.api.http.delete(url).bearer_auth(&job.api.token).send().await.map_err(|e| e.to_string())?;
            if !r.status().is_success() {
                return Err(format!("could not delete the existing artifact '{name}': HTTP {}", r.status()));
            }
        }
    }
    Ok(())
}

async fn download_artifact(job: &mut Job, with: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    if !input(with, "run-id").is_empty() && input(with, "run-id") != expr::to_string(&job.contexts["github"]["run_id"]) {
        return Err("Downloading artifacts of other workflow runs is not supported by forge runner".into());
    }
    let name = input(with, "name");
    let pattern = input(with, "pattern");
    let merge = truthy_input(with, "merge-multiple", false);
    let path = input(with, "path");
    let dest = resolve_path(job, if path.is_empty() { "." } else { &path });
    let all = list_artifacts(job).await?;
    let chosen: Vec<Value> = all
        .into_iter()
        .filter(|a| {
            let n = a["name"].as_str().unwrap_or("");
            if !name.is_empty() {
                n == name
            } else if !pattern.is_empty() {
                super::glob::glob_match(&pattern, n)
            } else {
                true
            }
        })
        .collect();
    if chosen.is_empty() {
        if !name.is_empty() {
            return Err(format!("Unable to download artifact(s): Artifact not found for name: {name}"));
        }
        job.log.line("No artifacts found");
    }
    let single = !name.is_empty();
    for a in &chosen {
        let n = a["name"].as_str().unwrap_or("artifact");
        let r = job.api.get(&format!("/artifacts/{}", a["id"])).await.map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("Unable to download artifact {n}: HTTP {}", r.status()));
        }
        let bytes = r.bytes().await.map_err(|e| e.to_string())?;
        let files = super::zip::read_all(&bytes)?;
        let target = if single || merge { dest.clone() } else { dest.join(n) };
        for (f, data) in files {
            let p = target.join(&f);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&p, data).map_err(|e| e.to_string())?;
        }
        job.log.line(&format!("Artifact {n} was downloaded to {}", target.display()));
    }
    let mut out = Map::new();
    out.insert("download-path".into(), json!(dest.to_string_lossy()));
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Caches

fn cache_paths(job: &Job, spec: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in spec.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('!')) {
        let abs = resolve_path(job, line);
        let s = abs.to_string_lossy().to_string();
        if has_glob(&s) {
            let (files, _) = collect_files(job, line, true);
            out.extend(files);
        } else {
            out.push(abs);
        }
    }
    out
}

fn cache_version(spec: &str) -> String {
    use sha2::{Digest, Sha256};
    let lines: Vec<&str> = spec.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    hex::encode(Sha256::digest(format!("{}|gzip|forge-1", lines.join("\n")).as_bytes()))
}

async fn cache_restore(job: &mut Job, with: &Map<String, Value>, register_post: bool, step_name: &str) -> Result<Map<String, Value>, String> {
    let key = input(with, "key");
    let spec = input(with, "path");
    if key.is_empty() || spec.trim().is_empty() {
        return Err("Input required and not supplied: key and path".into());
    }
    let restore_keys: Vec<String> = input(with, "restore-keys").lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let mut keys = vec![key.clone()];
    keys.extend(restore_keys);
    let version = cache_version(&spec);
    let q = format!("/caches?keys={}&version={version}", urlencode(&keys.join(",")));
    let r = job.api.get(&q).await.map_err(|e| e.to_string())?;
    let mut out = Map::new();
    out.insert("cache-primary-key".into(), json!(key));
    let mut exact = false;
    if r.status() == reqwest::StatusCode::OK {
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        let matched = v["key"].as_str().unwrap_or("").to_string();
        exact = matched == key;
        if !truthy_input(with, "lookup-only", false) {
            let d = job.api.get(&format!("/caches/{}", v["id"])).await.map_err(|e| e.to_string())?;
            let bytes = d.bytes().await.map_err(|e| e.to_string())?;
            let tgz = job.temp.join(format!("cache-{}.tgz", crate::auth::random_string(6)));
            std::fs::write(&tgz, &bytes).map_err(|e| e.to_string())?;
            let (ok, o) = run_cmd(tokio::process::Command::new("tar").args(["-xzf", &tgz.to_string_lossy(), "-C", "/"])).await.map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&tgz);
            if !ok {
                job.log.line(&format!("##[warning]Failed to restore cache: {}", o.trim()));
            } else {
                job.log.line(&format!("Cache restored from key: {matched} ({} bytes)", bytes.len()));
            }
        }
        out.insert("cache-matched-key".into(), json!(matched));
    } else {
        if truthy_input(with, "fail-on-cache-miss", false) {
            return Err(format!("Failed to restore cache entry. Exiting as fail-on-cache-miss is set. Input key: {key}"));
        }
        job.log.line(&format!("Cache not found for input keys: {}", keys.join(", ")));
        out.insert("cache-matched-key".into(), json!(""));
    }
    out.insert("cache-hit".into(), json!(if exact { "true" } else { "false" }));
    if register_post && !exact {
        job.post.push(PostStep { name: format!("Post {step_name}"), kind: "cache-save".into(), data: json!({ "key": key, "path": spec }), when: "success" });
    }
    Ok(out)
}

pub async fn cache_save(job: &mut Job, with: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let key = input(with, "key");
    let spec = input(with, "path");
    if key.is_empty() || spec.trim().is_empty() {
        return Err("Input required and not supplied: key and path".into());
    }
    let paths: Vec<PathBuf> = cache_paths(job, &spec).into_iter().filter(|p| p.exists()).collect();
    if paths.is_empty() {
        job.log.line("##[warning]Path Validation Error: Path(s) specified in the action for caching do(es) not exist, hence no cache is being saved.");
        return Ok(Map::new());
    }
    let tgz = job.temp.join(format!("cache-save-{}.tgz", crate::auth::random_string(6)));
    let mut args: Vec<String> = vec!["-czf".into(), tgz.to_string_lossy().into(), "-C".into(), "/".into()];
    for p in &paths {
        args.push(p.to_string_lossy().trim_start_matches('/').to_string());
    }
    let (ok, o) = run_cmd(tokio::process::Command::new("tar").args(&args)).await.map_err(|e| e.to_string())?;
    if !ok {
        return Err(format!("tar failed: {}", o.trim()));
    }
    let bytes = std::fs::read(&tgz).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tgz);
    let size = bytes.len();
    let (st, v) = job.api.post_bytes(&format!("/caches?key={}&version={}", urlencode(&key), cache_version(&spec)), bytes).await.map_err(|e| e.to_string())?;
    if st == reqwest::StatusCode::CONFLICT {
        job.log.line(&format!("##[warning]Failed to save: {}", v["message"].as_str().unwrap_or("cache already exists")));
    } else if !st.is_success() {
        job.log.line(&format!("##[warning]Failed to save cache: HTTP {st}"));
    } else {
        job.log.line(&format!("Cache saved with key: {key} ({size} bytes)"));
    }
    Ok(Map::new())
}

/// Runs the post steps registered by actions, newest first.
pub async fn run_post_steps(job: &mut Job) {
    let posts: Vec<PostStep> = std::mem::take(&mut job.post).into_iter().rev().collect();
    for p in posts {
        let run = match p.when {
            "always" => true,
            _ => job.current_status() == Status::Success,
        };
        let n = job.begin_step(&p.name);
        if !run {
            job.end_step(n, "skipped");
            continue;
        }
        let res = match p.kind.as_str() {
            "cache-save" => {
                let mut with = Map::new();
                with.insert("key".into(), p.data["key"].clone());
                with.insert("path".into(), p.data["path"].clone());
                cache_save(job, &with).await
            }
            _ => Ok(Map::new()),
        };
        match res {
            Ok(_) => job.end_step(n, "success"),
            Err(e) => {
                job.err(&e);
                job.status = Status::Failure;
                job.end_step(n, "failure");
            }
        }
    }
}
