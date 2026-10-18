//! Pre-receive hooks (GHES): site admins register hook scripts (a file in a "script
//! repository") that run in an environment during every push, before the push is committed.
//!
//! Flow for a push (`hook_guard`, called by HTTP and SSH receive-pack next to branch protection):
//! 1. the hooks whose effective enforcement (global -> org -> repo) is `enabled` or `testing`
//!    are resolved; their scripts are read from the script repositories' default branches and
//!    written to a private temp dir; custom environments are extracted from object storage;
//! 2. `git receive-pack` runs with `core.hooksPath` pointing at a generated `pre-receive` that
//!    execs `forge pre-receive-hooks <spec>` (this binary); it first runs the branch-protection
//!    hook (if any), then each script as a sandboxed subprocess (see [`run_hooks_main`]);
//! 3. a non-zero exit of an `enabled` hook rejects the whole push (`pre-receive hook declined`)
//!    with the hook's output shown to the client; `testing` hooks only report.
//!
//! Trust model: see docs/security.md ("Pre-receive hooks").
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::StreamExt;
use object_store::path::Path as ObjPath;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const DOWNLOAD_QUEUE: &str = "pre_receive_env_download";
/// Wall-clock limit per hook script (GHES uses 5 seconds).
pub const HOOK_TIMEOUT_SECS: u64 = 5;
const MAX_OUTPUT: usize = 64 * 1024;
const MAX_ENV_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EnvRow {
    pub id: i64,
    pub name: String,
    pub image_url: String,
    pub default_environment: bool,
    pub download_state: String,
    pub downloaded_at: Option<DateTime<Utc>>,
    pub download_message: Option<String>,
    pub object_key: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HookRow {
    pub id: i64,
    pub name: String,
    pub script: String,
    pub script_repo_id: i64,
    pub environment_id: i64,
    pub enforcement: String,
    pub allow_downstream_configuration: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn valid_enforcement(e: &str) -> bool {
    matches!(e, "enabled" | "disabled" | "testing")
}

pub fn download_json(api: &str, e: &EnvRow) -> Value {
    json!({
        "url": format!("{api}/admin/pre-receive-environments/{}/downloads/latest", e.id),
        "state": e.download_state,
        "downloaded_at": e.downloaded_at.map(crate::json::ts),
        "message": e.download_message,
    })
}

pub async fn env_json(s: &AppState, e: &EnvRow) -> ApiResult<Value> {
    let api = s.api_url();
    let hooks: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_pre_receive_hooks WHERE environment_id = $1").bind(e.id).fetch_one(&s.db).await?;
    Ok(json!({
        "id": e.id,
        "name": e.name,
        "image_url": e.image_url,
        "url": format!("{api}/admin/pre-receive-environments/{}", e.id),
        "html_url": format!("{}/admin/pre-receive-environments/{}", s.base_url(), e.id),
        "default_environment": e.default_environment,
        "created_at": crate::json::ts(e.created_at),
        "hooks_count": hooks,
        "download": download_json(&api, e),
    }))
}

pub async fn env_by_id(s: &AppState, id: i64) -> ApiResult<EnvRow> {
    sqlx::query_as("SELECT * FROM ent_pre_receive_environments WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

pub async fn hook_by_id(s: &AppState, id: i64) -> ApiResult<HookRow> {
    sqlx::query_as("SELECT * FROM ent_pre_receive_hooks WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

/// `pre-receive-hook` (global view).
pub async fn hook_json(s: &AppState, h: &HookRow) -> ApiResult<Value> {
    let api = s.api_url();
    let repo = crate::db::repo_by_id(&s.db, h.script_repo_id).await?;
    let owner = crate::db::user_by_id(&s.db, repo.owner_id).await?;
    let full = format!("{}/{}", owner.login, repo.name);
    let env = env_by_id(s, h.environment_id).await?;
    Ok(json!({
        "id": h.id,
        "name": h.name,
        "enforcement": h.enforcement,
        "script": h.script,
        "script_repository": {
            "id": repo.id,
            "full_name": full,
            "url": format!("{api}/repos/{full}"),
            "html_url": format!("{}/{full}", s.base_url()),
        },
        "environment": env_json(s, &env).await?,
        "allow_downstream_configuration": h.allow_downstream_configuration,
    }))
}

/// Enforcement of a hook at the organization level and whether repositories may override it,
/// plus where that configuration lives (for `configuration_url`).
#[derive(Debug, Clone)]
pub struct Effective {
    pub enforcement: String,
    pub allow_downstream: bool,
    /// "global" | "org" | "repo"
    pub level: &'static str,
}

/// Effective configuration of `h` for organization `org_id` (None for user-owned repositories).
pub async fn effective_org(s: &AppState, h: &HookRow, org_id: Option<i64>) -> ApiResult<Effective> {
    let global = Effective { enforcement: h.enforcement.clone(), allow_downstream: h.allow_downstream_configuration, level: "global" };
    let Some(org_id) = org_id else { return Ok(global) };
    if !h.allow_downstream_configuration {
        return Ok(global);
    }
    let row: Option<(String, bool)> =
        sqlx::query_as("SELECT enforcement, allow_downstream_configuration FROM ent_pre_receive_org_settings WHERE hook_id = $1 AND org_id = $2")
            .bind(h.id)
            .bind(org_id)
            .fetch_optional(&s.db)
            .await?;
    Ok(match row {
        Some((e, a)) => Effective { enforcement: e, allow_downstream: a, level: "org" },
        None => global,
    })
}

/// Effective configuration of `h` for a repository.
pub async fn effective_repo(s: &AppState, h: &HookRow, ctx: &RepoCtx) -> ApiResult<Effective> {
    let org_id = (ctx.owner.kind == "Organization").then_some(ctx.owner.id);
    let up = effective_org(s, h, org_id).await?;
    if !up.allow_downstream {
        return Ok(up);
    }
    let row: Option<String> = sqlx::query_scalar("SELECT enforcement FROM ent_pre_receive_repo_settings WHERE hook_id = $1 AND repo_id = $2")
        .bind(h.id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?;
    Ok(match row {
        Some(e) => Effective { enforcement: e, allow_downstream: false, level: "repo" },
        None => up,
    })
}

// ---------------------------------------------------------------------------------------------
// Environments: download (job) and local extraction

fn http_client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(10))
            .dns_resolver(crate::security::PublicOnlyResolver)
            .user_agent("forge-pre-receive-environment")
            .build()
            .expect("http client")
    })
}

/// Marks the environment as downloading and queues the download job.
pub async fn start_download(s: &AppState, env: &EnvRow) -> ApiResult<EnvRow> {
    if env.default_environment {
        return Err(ApiError::unprocessable("The default environment cannot be downloaded."));
    }
    if env.download_state == "in_progress" {
        return Err(ApiError::unprocessable("A download of this environment is already in progress."));
    }
    let mut tx = s.db.begin().await?;
    let e: EnvRow = sqlx::query_as(
        "UPDATE ent_pre_receive_environments SET download_state = 'in_progress', download_message = NULL WHERE id = $1 RETURNING *",
    )
    .bind(env.id)
    .fetch_one(&mut *tx)
    .await?;
    crate::jobs::enqueue(&mut *tx, DOWNLOAD_QUEUE, &json!({ "environment_id": env.id })).await?;
    tx.commit().await?;
    Ok(e)
}

async fn set_download_result(s: &AppState, id: i64, ok: bool, msg: Option<String>, key: Option<String>) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE ent_pre_receive_environments SET download_state = $2, download_message = $3,
            downloaded_at = CASE WHEN $2 = 'success' THEN now() ELSE downloaded_at END,
            object_key = COALESCE($4, object_key)
          WHERE id = $1",
    )
    .bind(id)
    .bind(if ok { "success" } else { "failed" })
    .bind(msg)
    .bind(key)
    .execute(&s.db)
    .await?;
    Ok(())
}

/// Job: downloads `image_url` (a .tar.gz) into object storage. Failures are recorded on the
/// environment (state `failed`, message) and not retried.
pub async fn run_download_job(s: &AppState, job: &crate::jobs::Job) -> anyhow::Result<()> {
    let id = job.payload.get("environment_id").and_then(Value::as_i64).ok_or_else(|| anyhow::anyhow!("bad job"))?;
    let Some(env) = sqlx::query_as::<_, EnvRow>("SELECT * FROM ent_pre_receive_environments WHERE id = $1").bind(id).fetch_optional(&s.db).await?
    else {
        return Ok(());
    };
    match download(s, &env).await {
        Ok(key) => set_download_result(s, id, true, None, Some(key)).await,
        Err(e) => set_download_result(s, id, false, Some(format!("{e:#}")), None).await,
    }
}

async fn download(s: &AppState, env: &EnvRow) -> anyhow::Result<String> {
    let url = url::Url::parse(&env.image_url).map_err(|_| anyhow::anyhow!("image_url is not a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("image_url must be an http(s) URL");
    }
    if let Some(h) = url.host() {
        if !crate::security::webhook_allow_private() && crate::security::blocked_literal_host(&h) {
            anyhow::bail!("image_url is not a public address");
        }
    }
    let res = http_client().get(url).send().await.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    if !res.status().is_success() {
        anyhow::bail!("download failed: HTTP {}", res.status().as_u16());
    }
    let mut stream = res.bytes_stream();
    let mut data: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
        data.extend_from_slice(&chunk);
        if data.len() as u64 > MAX_ENV_BYTES {
            anyhow::bail!("environment is larger than {} MiB", MAX_ENV_BYTES / 1024 / 1024);
        }
    }
    if data.len() < 2 || data[0] != 0x1f || data[1] != 0x8b {
        anyhow::bail!("image_url must point to a gzip-compressed tarball (.tar.gz)");
    }
    let digest = hex::encode(sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(&data)));
    let key = format!("pre-receive-environments/{}/{digest}.tar.gz", env.id);
    let mut w = object_store::buffered::BufWriter::new(s.store.clone(), ObjPath::from(key.clone()));
    w.write_all(&data).await?;
    w.shutdown().await?;
    Ok(key)
}

/// Local directory holding the extracted environment (extracted once per pod and version).
async fn ensure_env_dir(s: &AppState, env: &EnvRow) -> anyhow::Result<Option<PathBuf>> {
    if env.default_environment {
        return Ok(None);
    }
    let Some(key) = env.object_key.as_deref() else {
        anyhow::bail!("pre-receive environment '{}' has not been downloaded", env.name);
    };
    let tag = crate::auth::sha256_hex(key);
    let root = s.cfg.cache_dir.join(".forge-pre-receive-envs");
    let dir = root.join(format!("{}-{}", env.id, &tag[..16]));
    if dir.join(".forge-extracted").exists() {
        return Ok(Some(dir));
    }
    tokio::fs::create_dir_all(&root).await?;
    let tmp = root.join(format!(".x-{}", crate::auth::random_string(10)));
    tokio::fs::create_dir_all(&tmp).await?;
    let tarball = root.join(format!(".t-{}.tar.gz", crate::auth::random_string(10)));
    let mut f = tokio::fs::File::create(&tarball).await?;
    let mut stream = s.store.get(&ObjPath::from(key.to_string())).await?.into_stream();
    while let Some(c) = stream.next().await {
        f.write_all(&c?).await?;
    }
    f.flush().await?;
    drop(f);
    let out = tokio::process::Command::new("tar").arg("-xzf").arg(&tarball).arg("-C").arg(&tmp).output().await?;
    let _ = tokio::fs::remove_file(&tarball).await;
    if !out.status.success() {
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        anyhow::bail!("could not extract environment '{}': {}", env.name, String::from_utf8_lossy(&out.stderr).trim());
    }
    tokio::fs::write(tmp.join(".forge-extracted"), b"").await?;
    if tokio::fs::rename(&tmp, &dir).await.is_err() {
        // Another push extracted it concurrently.
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
    Ok(Some(dir))
}

// ---------------------------------------------------------------------------------------------
// Push integration

static EXEC_BIN: OnceLock<PathBuf> = OnceLock::new();

/// Overrides the binary used to run hooks (tests: `env!("CARGO_BIN_EXE_forge")`). Defaults to
/// `FORGE_HOOK_EXEC_BIN` or the current executable.
pub fn set_exec_bin(p: impl Into<PathBuf>) {
    let _ = EXEC_BIN.set(p.into());
}

fn exec_bin() -> anyhow::Result<PathBuf> {
    if let Some(p) = EXEC_BIN.get() {
        return Ok(p.clone());
    }
    if let Ok(p) = std::env::var("FORGE_HOOK_EXEC_BIN") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    Ok(std::env::current_exe()?)
}

const WRAPPER: &str = "#!/bin/sh\n# forge pre-receive hooks (generated by crates/forge/src/enterprise/prereceive.rs)\nexec \"$FORGE_HOOK_EXEC\" pre-receive-hooks \"$FORGE_ENT_HOOKS\"\n";

/// Per-push hook configuration; its temp dir (scripts, spec) is removed on drop.
pub struct HookGuard {
    hooks_dir: PathBuf,
    work: PathBuf,
    spec: PathBuf,
    exec: PathBuf,
}

impl HookGuard {
    /// Points `git receive-pack` at the enterprise hook runner. Call after
    /// `PushGuard::apply` (branch protection): the runner also runs the protection hook.
    pub fn apply(&self, cmd: &mut tokio::process::Command) {
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.hooksPath")
            .env("GIT_CONFIG_VALUE_0", &self.hooks_dir)
            .env("FORGE_ENT_HOOKS", &self.spec)
            .env("FORGE_HOOK_EXEC", &self.exec);
    }
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

/// Resolves the pre-receive hooks that apply to a push to `ctx` by `pusher`; None when no hook
/// is enabled or in testing for the repository.
pub async fn hook_guard(s: &AppState, ctx: &RepoCtx, pusher: &User, via: &str, client_ip: Option<std::net::IpAddr>) -> anyhow::Result<Option<HookGuard>> {
    let hooks: Vec<HookRow> = sqlx::query_as("SELECT * FROM ent_pre_receive_hooks ORDER BY id").fetch_all(&s.db).await?;
    if hooks.is_empty() {
        return Ok(None);
    }
    let mut active = Vec::new();
    for h in hooks {
        let eff = effective_repo(s, &h, ctx).await.map_err(|e| anyhow::anyhow!(e.message))?;
        if eff.enforcement != "disabled" {
            active.push((h, eff.enforcement));
        }
    }
    if active.is_empty() {
        return Ok(None);
    }
    let root = s.cfg.cache_dir.join(".forge-ent-hooks");
    tokio::fs::create_dir_all(&root).await?;
    let wrapper = root.join("pre-receive");
    if tokio::fs::read_to_string(&wrapper).await.ok().as_deref() != Some(WRAPPER) {
        let tmp = root.join(format!(".pre-receive.{}", crate::auth::random_string(8)));
        tokio::fs::write(&tmp, WRAPPER).await?;
        set_mode(&tmp, 0o755).await?;
        tokio::fs::rename(&tmp, &wrapper).await?;
    }
    let work = root.join(format!("push-{}", crate::auth::random_string(16)));
    tokio::fs::create_dir_all(&work).await?;
    set_mode(&work, 0o700).await?;
    let guard_work = work.clone();
    let mut specs = Vec::new();
    for (i, (h, enforcement)) in active.iter().enumerate() {
        let script_path = work.join(format!("hook-{i}"));
        let content = read_script(s, h).await;
        let env = env_by_id(s, h.environment_id).await.map_err(|e| anyhow::anyhow!(e.message))?;
        let (env_dir, error) = match (&content, ensure_env_dir(s, &env).await) {
            (Err(e), _) => (None, Some(e.clone())),
            (Ok(_), Err(e)) => (None, Some(format!("{e:#}"))),
            (Ok(_), Ok(d)) => (d, None),
        };
        if let Ok(bytes) = &content {
            tokio::fs::write(&script_path, bytes).await?;
            set_mode(&script_path, 0o700).await?;
        }
        specs.push(json!({
            "name": h.name,
            "enforcement": enforcement,
            "script": script_path,
            "env_dir": env_dir,
            "error": error,
        }));
    }
    let public = !ctx.repo.private;
    let vars = json!({
        "GITHUB_USER_LOGIN": pusher.login,
        "GITHUB_USER_IP": client_ip.map(|i| i.to_string()).unwrap_or_default(),
        "GITHUB_REPO_NAME": ctx.full_name(),
        "GITHUB_REPO_PUBLIC": if public { "true" } else { "false" },
        "GITHUB_VIA": via,
    });
    let spec = work.join("spec.json");
    let doc = json!({
        "hooks": specs,
        "vars": vars,
        "timeout_secs": HOOK_TIMEOUT_SECS,
        "protect_hook": s.cfg.cache_dir.join(".forge-hooks").join("pre-receive"),
        "home": work,
    });
    tokio::fs::write(&spec, serde_json::to_vec(&doc)?).await?;
    Ok(Some(HookGuard { hooks_dir: root, work: guard_work, spec, exec: exec_bin()? }))
}

async fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).await?;
    }
    let _ = (p, mode);
    Ok(())
}

/// The hook script from the script repository's default branch.
async fn read_script(s: &AppState, h: &HookRow) -> Result<Vec<u8>, String> {
    let repo = crate::db::repo_by_id(&s.db, h.script_repo_id).await.map_err(|_| "script repository not found".to_string())?;
    let path = s.git.sync(repo.id).await.map_err(|e| format!("script repository unavailable: {e}"))?;
    let spec = format!("refs/heads/{}:{}", repo.default_branch, h.script.trim_start_matches('/'));
    match s.git.git.try_run(&path, &["cat-file", "blob", &spec]).await {
        Ok((true, out, _)) => Ok(out),
        _ => Err(format!("script '{}' not found in {} ({})", h.script, repo.name, repo.default_branch)),
    }
}

// ---------------------------------------------------------------------------------------------
// The hook runner (`forge pre-receive-hooks <spec>`), executed by git as the pre-receive hook.

fn absolute(cwd: &Path, v: &str) -> String {
    if v.is_empty() || Path::new(v).is_absolute() {
        v.to_string()
    } else {
        cwd.join(v).display().to_string()
    }
}

fn absolute_list(cwd: &Path, v: &str) -> String {
    v.split(':').map(|p| absolute(cwd, p)).collect::<Vec<_>>().join(":")
}

/// Runs a child to completion with a wall-clock limit, killing its whole process group on
/// timeout. Returns (exit success, combined output, timed out).
fn run_limited(mut cmd: std::process::Command, input: &[u8], timeout: Duration) -> std::io::Result<(bool, Vec<u8>, bool)> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let read_capped = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out.len() < MAX_OUTPUT {
                            out.extend_from_slice(&buf[..n.min(MAX_OUTPUT - out.len())]);
                        }
                    }
                }
            }
            out
        })
    };
    let out_t = read_capped(Box::new(child.stdout.take().unwrap()));
    let err_t = read_capped(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).status();
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = writer.join();
    let mut out = out_t.join().unwrap_or_default();
    out.extend(err_t.join().unwrap_or_default());
    Ok((status.is_some_and(|s| s.success()), out, timed_out))
}

/// Entry point of `forge pre-receive-hooks <spec>`. Returns the process exit code.
pub fn run_hooks_main(spec_path: &str) -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let spec: Value = match std::fs::read(spec_path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(v) => v,
        None => {
            eprintln!("error: forge pre-receive configuration is missing");
            return 1;
        }
    };
    let stderr = std::io::stderr();
    let mut err = stderr.lock();
    // Branch protection first (it runs with git's environment, like before).
    if std::env::var_os("FORGE_PROTECT_RULES").is_some() {
        if let Some(p) = spec.get("protect_hook").and_then(Value::as_str).filter(|p| Path::new(p).exists()) {
            match run_limited(std::process::Command::new(p), &input, Duration::from_secs(60)) {
                Ok((ok, out, _)) => {
                    let _ = err.write_all(&out);
                    if !ok {
                        return 1;
                    }
                }
                Err(e) => {
                    let _ = writeln!(err, "error: branch protection check failed: {e}");
                    return 1;
                }
            }
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let timeout = Duration::from_secs(spec.get("timeout_secs").and_then(Value::as_u64).unwrap_or(HOOK_TIMEOUT_SECS));
    let home = spec.get("home").and_then(Value::as_str).unwrap_or("/tmp").to_string();
    let git_vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            matches!(k.as_str(), "GIT_DIR" | "GIT_QUARANTINE_PATH" | "GIT_OBJECT_DIRECTORY" | "GIT_ALTERNATE_OBJECT_DIRECTORIES" | "GIT_PUSH_CERT" | "GIT_PUSH_CERT_STATUS")
                || k.starts_with("GIT_PUSH_OPTION")
        })
        .map(|(k, v)| {
            let v = match k.as_str() {
                "GIT_DIR" | "GIT_QUARANTINE_PATH" | "GIT_OBJECT_DIRECTORY" => absolute(&cwd, &v),
                "GIT_ALTERNATE_OBJECT_DIRECTORIES" => absolute_list(&cwd, &v),
                _ => v,
            };
            (k, v)
        })
        .collect();
    let git_dir = git_vars.iter().find(|(k, _)| k == "GIT_DIR").map(|(_, v)| v.clone()).unwrap_or_else(|| cwd.display().to_string());
    let mut rc = 0;
    for h in spec.get("hooks").and_then(Value::as_array).cloned().unwrap_or_default() {
        let name = h.get("name").and_then(Value::as_str).unwrap_or("hook");
        let enforced = h.get("enforcement").and_then(Value::as_str) == Some("enabled");
        let (ok, out, note) = if let Some(e) = h.get("error").and_then(Value::as_str) {
            (false, Vec::new(), Some(e.to_string()))
        } else {
            let script = h.get("script").and_then(Value::as_str).unwrap_or_default();
            let env_dir = h.get("env_dir").and_then(Value::as_str);
            let shebang = std::fs::read(script).map(|b| b.starts_with(b"#!")).unwrap_or(false);
            let mut cmd = if shebang {
                std::process::Command::new(script)
            } else {
                let mut c = std::process::Command::new("/bin/sh");
                c.arg(script);
                c
            };
            let mut path = String::new();
            if let Some(d) = env_dir {
                for sub in ["bin", "usr/bin", "usr/local/bin", "sbin", "usr/sbin"] {
                    path.push_str(&format!("{d}/{sub}:"));
                }
            }
            path.push_str("/usr/local/bin:/usr/bin:/bin");
            cmd.env_clear()
                .env("PATH", path)
                .env("HOME", &home)
                .env("LANG", "C.UTF-8")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .current_dir(env_dir.unwrap_or(&git_dir));
            for (k, v) in &git_vars {
                cmd.env(k, v);
            }
            if let Some(vars) = h.get("vars").or_else(|| spec.get("vars")).and_then(Value::as_object) {
                for (k, v) in vars {
                    if let Some(v) = v.as_str() {
                        cmd.env(k, v);
                    }
                }
            }
            match run_limited(cmd, &input, timeout) {
                Ok((ok, out, timed_out)) => {
                    (ok, out, timed_out.then(|| format!("pre-receive hook '{name}' timed out after {}s", timeout.as_secs())))
                }
                Err(e) => (false, Vec::new(), Some(format!("pre-receive hook '{name}' could not be started: {e}"))),
            }
        };
        let _ = err.write_all(&out);
        if !out.is_empty() && !out.ends_with(b"\n") {
            let _ = err.write_all(b"\n");
        }
        if let Some(n) = &note {
            let _ = writeln!(err, "error: {n}");
        }
        if !ok {
            if enforced {
                let _ = writeln!(err, "error: pre-receive hook '{name}' declined this push");
                rc = 1;
            } else {
                let _ = writeln!(err, "warning: pre-receive hook '{name}' (testing) would have declined this push");
            }
        }
    }
    let _ = err.flush();
    rc
}
