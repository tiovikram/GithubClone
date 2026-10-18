//! The runner protocol: plain HTTP + JSON under `/api/forge/runner`, used by `forge runner`.
//! Runners never touch Postgres or object storage directly.
//!
//! - `POST /register` (registration token) -> runner id + credential
//! - `POST /remove` (remove token, runner credential) -> deletes the runner
//! - `POST /acquire` (runner credential, long poll) -> a job message with a fresh job token
//! - `/jobs/{id}/...` (job token): `logs?step=N` (append), `heartbeat` (steps; answers whether
//!   the job was cancelled), `complete`, `artifacts`, `caches`
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::engine::{self, JobRow};
use super::token::{self, RunnerRow, RunnerScope};
use super::{logs, storage};
use crate::api::actions::RunRow;
use crate::db::{self, Perm, RepoCtx};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let p = "/api/forge/runner";
    Router::new()
        .route(&format!("{p}/register"), post(register))
        .route(&format!("{p}/remove"), post(remove))
        .route(&format!("{p}/acquire"), post(acquire))
        .route(&format!("{p}/jobs/{{id}}/logs"), post(append_log))
        .route(&format!("{p}/jobs/{{id}}/heartbeat"), post(heartbeat))
        .route(&format!("{p}/jobs/{{id}}/complete"), post(complete))
        .route(&format!("{p}/jobs/{{id}}/artifacts"), get(list_artifacts).post(upload_artifact))
        .route(&format!("{p}/jobs/{{id}}/artifacts/{{aid}}"), get(download_artifact))
        .route(&format!("{p}/jobs/{{id}}/caches"), get(find_cache).post(save_cache))
        .route(&format!("{p}/jobs/{{id}}/caches/{{cid}}"), get(download_cache))
        .layer(DefaultBodyLimit::disable())
}

fn bearer(h: &HeaderMap) -> Option<String> {
    let v = h.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, cred) = v.split_once(' ')?;
    matches!(scheme.to_ascii_lowercase().as_str(), "bearer" | "token").then(|| cred.trim().to_string())
}

async fn runner_auth(s: &AppState, h: &HeaderMap) -> ApiResult<RunnerRow> {
    let cred = bearer(h).ok_or_else(ApiError::unauthorized)?;
    token::runner_for_credential(&s.db, &cred).await?.ok_or_else(ApiError::bad_credentials)
}

/// Job endpoints: the runner credential of the runner that claimed the job.
async fn job_auth(s: &AppState, h: &HeaderMap, job_id: i64) -> ApiResult<JobRow> {
    let runner = runner_auth(s, h).await?;
    let job: JobRow = sqlx::query_as("SELECT * FROM actions_jobs WHERE id = $1").bind(job_id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
    if job.runner_id != Some(runner.id) {
        return Err(ApiError::forbidden("This job is not assigned to this runner"));
    }
    Ok(job)
}

// ---------------------------------------------------------------------------------------------
// Registration

#[derive(Deserialize)]
struct RegisterBody {
    token: String,
    name: String,
    #[serde(default)]
    labels: Vec<String>,
    os: Option<String>,
    arch: Option<String>,
    version: Option<String>,
    #[serde(default)]
    ephemeral: bool,
}

/// Normalised custom labels (system labels self-hosted/OS/arch are implicit).
pub fn custom_labels(labels: &[String], os: &str, arch: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in labels {
        let l = l.trim();
        if l.is_empty() || l.eq_ignore_ascii_case("self-hosted") || l.eq_ignore_ascii_case(os) || l.eq_ignore_ascii_case(arch) {
            continue;
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(l)) {
            out.push(l.to_string());
        }
    }
    out
}

/// Creates (or replaces, by name within the scope) a runner and returns its credential.
pub async fn create_runner(
    s: &AppState,
    scope: &RunnerScope,
    name: &str,
    labels: &[String],
    os: &str,
    arch: &str,
    version: Option<&str>,
    ephemeral: bool,
    replace: bool,
) -> ApiResult<(RunnerRow, String)> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(ApiError::validation("Runner", "name", "invalid"));
    }
    let (repo_id, org_id) = scope.ids();
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM actions_runners WHERE COALESCE(repo_id, 0) = COALESCE($1, 0) AND COALESCE(org_id, 0) = COALESCE($2, 0) AND lower(name) = lower($3)",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(name)
    .fetch_optional(&s.db)
    .await?;
    if let Some(id) = existing {
        if !replace {
            return Err(ApiError::conflict(format!("A runner with the name '{name}' already exists")));
        }
        sqlx::query("DELETE FROM actions_runners WHERE id = $1").bind(id).execute(&s.db).await?;
    }
    let cred = token::new_runner_credential();
    let row: RunnerRow = sqlx::query_as(
        "INSERT INTO actions_runners (repo_id, org_id, name, os, arch, labels, credential_hash, ephemeral, version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING *",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(name)
    .bind(os)
    .bind(arch)
    .bind(custom_labels(labels, os, arch))
    .bind(crate::auth::sha256_hex(&cred))
    .bind(ephemeral)
    .bind(version)
    .fetch_one(&s.db)
    .await?;
    Ok((row, cred))
}

fn norm_os(os: Option<&str>) -> String {
    match os.unwrap_or(std::env::consts::OS).to_ascii_lowercase().as_str() {
        "macos" | "darwin" => "macOS".into(),
        "windows" => "Windows".into(),
        _ => "Linux".into(),
    }
}

fn norm_arch(arch: Option<&str>) -> String {
    match arch.unwrap_or(std::env::consts::ARCH).to_ascii_lowercase().as_str() {
        "aarch64" | "arm64" => "ARM64".into(),
        "arm" => "ARM".into(),
        _ => "X64".into(),
    }
}

async fn register(State(s): State<AppState>, Json(b): Json<RegisterBody>) -> ApiResult<Response> {
    let scope = token::runner_token_scope(&s.db, "registration", &b.token).await?.ok_or_else(ApiError::bad_credentials)?;
    let os = norm_os(b.os.as_deref());
    let arch = norm_arch(b.arch.as_deref());
    let (row, cred) = create_runner(&s, &scope, &b.name, &b.labels, &os, &arch, b.version.as_deref(), b.ephemeral, true).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": row.id, "name": row.name, "credential": cred, "labels": all_labels(&row) }))).into_response())
}

pub fn all_labels(r: &RunnerRow) -> Vec<String> {
    let mut v = vec!["self-hosted".to_string(), r.os.clone(), r.arch.clone()];
    v.extend(r.labels.iter().cloned());
    v
}

#[derive(Deserialize)]
struct RemoveBody {
    token: String,
}

async fn remove(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<RemoveBody>) -> ApiResult<Response> {
    let runner = runner_auth(&s, &headers).await?;
    let scope = token::runner_token_scope(&s.db, "remove", &b.token).await?.ok_or_else(ApiError::bad_credentials)?;
    if scope != RunnerScope::of(runner.repo_id, runner.org_id) {
        return Err(ApiError::forbidden("The remove token does not belong to this runner's scope"));
    }
    sqlx::query("DELETE FROM actions_runners WHERE id = $1").bind(runner.id).execute(&s.db).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------------------------
// Job acquisition

#[derive(Deserialize, Default)]
struct AcquireBody {
    /// seconds to wait for a job (long poll), at most 50
    wait: Option<u64>,
}

/// Claims the oldest queued job this runner can take (scope + labels) with
/// `FOR UPDATE SKIP LOCKED`, so any number of runners and pods can poll concurrently.
pub async fn claim(s: &AppState, r: &RunnerRow) -> ApiResult<Option<JobRow>> {
    let labels: Vec<String> = all_labels(r).into_iter().map(|l| l.to_lowercase()).collect();
    let mut tx = s.db.begin().await?;
    // Candidates by scope and labels, locked; then Actions policy (self-hosted runners allowed,
    // runner group) decides which one this runner takes. Candidates are paged by id, so jobs
    // the policy rejects (e.g. other orgs' jobs for a group-restricted enterprise runner)
    // cannot starve the ones it may take. Rejected candidates unlock on commit.
    let mut after = 0i64;
    loop {
        let cands: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT j.id, j.repo_id, u.login || '/' || rp.name || '/' || r.workflow_path || '@' || COALESCE(r.git_ref, '')
               FROM actions_jobs j JOIN repositories rp ON rp.id = j.repo_id JOIN users u ON u.id = rp.owner_id
               JOIN actions_runs r ON r.id = j.run_id
              WHERE j.status = 'queued' AND j.id > $4
                AND ($1::bigint IS NULL OR j.repo_id = $1)
                AND ($2::bigint IS NULL OR rp.owner_id = $2)
                AND NOT EXISTS (SELECT 1 FROM unnest(j.labels) l
                                 WHERE NOT (lower(l) = ANY($3) OR lower(l) ~ '^(ubuntu|macos|windows)-' OR lower(l) IN ('ubuntu', 'github-hosted')))
              ORDER BY j.id LIMIT 20 FOR UPDATE OF j SKIP LOCKED",
        )
        .bind(r.repo_id)
        .bind(r.org_id)
        .bind(&labels)
        .bind(after)
        .fetch_all(&mut *tx)
        .await?;
        let Some(last) = cands.last().map(|c| c.0) else { break };
        after = last;
        for (id, repo_id, wref) in cands {
            if !super::settings::runner_may_take(s, r, repo_id, Some(&wref)).await {
                continue;
            }
            let job: JobRow = sqlx::query_as(
                "UPDATE actions_jobs j SET status = 'in_progress', runner_id = $2, runner_name = $3, started_at = now(), heartbeat_at = now(),
                        timeout_at = now() + make_interval(mins => COALESCE(NULLIF(j.definition->>'timeout-minutes', '')::float8::int, 360))
                  WHERE j.id = $1 RETURNING j.*",
            )
            .bind(id)
            .bind(r.id)
            .bind(&r.name)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(Some(job));
        }
    }
    tx.commit().await?;
    Ok(None)
}

async fn acquire(State(s): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> ApiResult<Response> {
    let body: Option<AcquireBody> = serde_json::from_slice(&body).ok();
    let runner = runner_auth(&s, &headers).await?;
    let wait = Duration::from_secs(body.and_then(|b| b.wait).unwrap_or(25).min(50));
    let deadline = Instant::now() + wait;
    loop {
        sqlx::query("UPDATE actions_runners SET last_seen_at = now() WHERE id = $1").bind(runner.id).execute(&s.db).await?;
        // a runner holding a job... may still take another (a runner process can run several slots)
        if let Some(job) = claim(&s, &runner).await? {
            match job_message(&s, &job, &runner).await {
                Ok(msg) => return Ok(Json(msg).into_response()),
                Err(e) => {
                    tracing::warn!(job = job.id, error = %e.message, "could not build job message");
                    let _ = logs::append(&s, job.id, 1, format!("{} ##[error]Could not start the job: {}\n", logs::stamp(), e.message).as_bytes()).await;
                    engine::finish_job(&s, job.id, "failure", None, None).await?;
                    continue;
                }
            }
        }
        if Instant::now() >= deadline {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// GITHUB_TOKEN permission: write unless the workflow/job restricts `contents` to read (or
/// the run comes from a fork).
/// GITHUB_TOKEN permissions (app permission names): the job's or workflow's `permissions:`,
/// else the repository default (permissive: everything write; restricted: contents and
/// packages read). Fork pull requests get read-only tokens unless the fork policy allows.
pub fn token_permissions(run: &RunRow, job_def: &Value, default: Perm, fork_write: bool) -> Map<String, Value> {
    let spec = job_def.get("permissions").or_else(|| run.definition.as_ref().and_then(|d| d.get("permissions")));
    let all = |lvl: &str| -> Map<String, Value> { token::TOKEN_PERMISSIONS.iter().map(|(_, p)| (p.to_string(), json!(lvl))).collect() };
    let mut m = match spec {
        None if default >= Perm::Write => all("write"),
        None => [("contents", "read"), ("packages", "read")].iter().map(|(k, v)| (k.to_string(), json!(v))).collect(),
        Some(Value::String(s)) if s == "write-all" => all("write"),
        Some(Value::String(s)) if s == "read-all" => all("read"),
        Some(Value::Object(o)) => {
            let mut m = Map::new();
            for (k, v) in o {
                if let Some((_, app)) = token::TOKEN_PERMISSIONS.iter().find(|(gh, _)| gh == k) {
                    if let Some(l @ ("read" | "write")) = v.as_str() {
                        m.insert(app.to_string(), json!(l));
                    }
                }
            }
            m
        }
        Some(_) => Map::new(),
    };
    let fork = run.head_repo_id.is_some_and(|h| h != run.repo_id);
    if fork && !fork_write {
        for v in m.values_mut() {
            *v = json!("read");
        }
    }
    m
}

/// Secrets for a job: GITHUB_TOKEN plus the Actions-settings secrets; fork pull requests get
/// none.
async fn secrets_context(s: &AppState, ctx: &RepoCtx, run: &RunRow, environment: Option<&str>, job_token: &str, fork_secrets: bool) -> Value {
    let mut m = Map::new();
    let fork = run.head_repo_id.is_some_and(|h| h != run.repo_id);
    if !fork || fork_secrets {
        for (k, v) in super::settings::secrets_for_job(s, ctx, environment).await {
            m.insert(k, Value::String(v));
        }
    }
    m.insert("GITHUB_TOKEN".into(), Value::String(job_token.to_string()));
    Value::Object(m)
}

/// The message handed to the runner: everything needed to run the job without database
/// access.
pub async fn job_message(s: &AppState, job: &JobRow, runner: &RunnerRow) -> ApiResult<Value> {
    let run: RunRow = sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(job.run_id).fetch_one(&s.db).await?;
    let repo = db::repo_by_id(&s.db, run.repo_id).await?;
    let owner = db::user_by_id(&s.db, repo.owner_id).await?;
    let ctx = RepoCtx { repo, owner, perm: Perm::Admin };
    let actor = match run.actor_id {
        Some(i) => db::user_by_id(&s.db, i).await.ok(),
        None => None,
    };
    let trig = match run.triggering_actor_id {
        Some(i) => db::user_by_id(&s.db, i).await.ok(),
        None => None,
    };
    let def = &job.definition;
    let timeout = def.get("timeout-minutes").and_then(Value::as_f64).unwrap_or(360.0);
    let expires = chrono::Utc::now() + chrono::Duration::minutes(timeout as i64 + 30);
    let fork_policy = super::settings::fork_pr_policy(s, ctx.repo.id).await;
    let fork_write = fork_policy.as_ref().is_some_and(|p| p.send_write_tokens_to_workflows);
    let fork_secrets = fork_policy.as_ref().is_some_and(|p| p.send_secrets_and_variables);
    let perms = token_permissions(&run, def, super::settings::default_token_perm(s, ctx.repo.id).await, fork_write);
    let job_token = token::mint_job_token(&s.db, job.id, &ctx, &perms, expires).await?;
    let mut github = engine::github_context(s, &ctx, &run, actor.as_ref(), trig.as_ref(), &job.job_key);
    github["token"] = json!(job_token);
    let siblings: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2 ORDER BY id")
        .bind(run.id)
        .bind(run.run_attempt)
        .fetch_all(&s.db)
        .await?;
    let needs = engine::needs_context(&job.needs, &siblings);
    let same: Vec<&JobRow> = siblings.iter().filter(|j| j.job_key == job.job_key).collect();
    let index = same.iter().position(|j| j.id == job.id).unwrap_or(0);
    let strategy = json!({
        "fail-fast": def.pointer("/strategy/fail-fast").cloned().unwrap_or(json!(true)),
        "job-index": index,
        "job-total": same.len(),
        "max-parallel": def.pointer("/strategy/max-parallel").cloned().unwrap_or(json!(same.len())),
    });
    let environment = match def.get("environment") {
        Some(Value::String(e)) => Some(e.clone()),
        Some(Value::Object(o)) => o.get("name").and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    let fork = run.head_repo_id.is_some_and(|h| h != run.repo_id);
    let vars = if fork && !fork_secrets { json!({}) } else { engine::vars_context(s, &ctx, environment.as_deref()).await? };
    let secrets = secrets_context(s, &ctx, &run, environment.as_deref(), &job_token, fork_secrets).await;
    // OIDC: ACTIONS_ID_TOKEN_REQUEST_URL / _TOKEN for jobs with `id-token: write`.
    let id_token = super::oidc::request_token(s, &ctx, &run, job, &github, environment.as_deref(), fork_write, expires).await?;
    // `uses:` the Actions policy forbids (the runner fails those steps)
    let mut blocked: Vec<String> = Vec::new();
    for st in def.get("steps").and_then(Value::as_array).cloned().unwrap_or_default() {
        if let Some(u) = st.get("uses").and_then(Value::as_str) {
            if !u.contains("${{") && !super::settings::action_allowed(s, ctx.repo.id, u).await && !blocked.iter().any(|b| b == u) {
                blocked.push(u.to_string());
            }
        }
    }
    let wdef = run.definition.clone().unwrap_or(Value::Null);
    let _ = sqlx::query("UPDATE actions_jobs SET steps = '[]' WHERE id = $1").bind(job.id).execute(&s.db).await;
    // mark the run in progress and announce the job
    engine::post_status(s, &ctx, &run, job).await;
    let _ = engine::advance(s, run.id).await;
    Ok(json!({
        "job_id": job.id,
        "run_id": run.id,
        "run_attempt": run.run_attempt,
        "job_key": job.job_key,
        "name": job.name,
        "token": job_token,
        "repository": { "owner": ctx.owner.login, "name": ctx.repo.name, "full_name": ctx.full_name(), "private": ctx.repo.private,
                        "default_branch": ctx.repo.default_branch },
        "sha": run.github_sha.clone().unwrap_or_else(|| run.head_sha.clone()),
        "ref": run.git_ref,
        "workflow": { "name": run.name, "path": run.workflow_path, "env": wdef.get("env").cloned().unwrap_or(json!({})),
                      "defaults": wdef.get("defaults").cloned().unwrap_or(json!({})) },
        "job": def,
        "labels": job.labels,
        "timeout_minutes": timeout,
        "contexts": {
            "github": github,
            "needs": needs,
            "matrix": job.matrix.clone().unwrap_or(json!({})),
            "strategy": strategy,
            "inputs": engine::inputs_context(&run),
            "vars": vars,
            "secrets": secrets,
        },
        "runner": { "id": runner.id, "name": runner.name, "os": runner.os, "arch": runner.arch },
        "environment": environment,
        "blocked_uses": blocked,
        "id_token": id_token,
    }))
}

// ---------------------------------------------------------------------------------------------
// Job updates

#[derive(Deserialize)]
struct StepQuery {
    step: Option<i32>,
}

async fn append_log(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Query(q): Query<StepQuery>, body: axum::body::Bytes) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    if body.len() > 8 << 20 {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "log chunk too large"));
    }
    logs::append(&s, job.id, q.step.unwrap_or(0), &body).await?;
    Ok(Json(json!({ "cancel": job.cancel_requested || job.status == "completed" })).into_response())
}

#[derive(Deserialize, Default)]
struct HeartbeatBody {
    steps: Option<Value>,
}

async fn heartbeat(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, body: axum::body::Bytes) -> ApiResult<Response> {
    let body: Option<HeartbeatBody> = serde_json::from_slice(&body).ok();
    let job = job_auth(&s, &headers, id).await?;
    let steps = body.and_then(|b| b.steps);
    sqlx::query("UPDATE actions_jobs SET heartbeat_at = now(), steps = COALESCE($2, steps) WHERE id = $1 AND status = 'in_progress'")
        .bind(job.id)
        .bind(&steps)
        .execute(&s.db)
        .await?;
    if let Some(rid) = job.runner_id {
        sqlx::query("UPDATE actions_runners SET last_seen_at = now() WHERE id = $1").bind(rid).execute(&s.db).await?;
    }
    Ok(Json(json!({ "cancel": job.cancel_requested || job.status == "completed" })).into_response())
}

#[derive(Deserialize)]
struct CompleteBody {
    conclusion: String,
    outputs: Option<Value>,
    steps: Option<Value>,
    annotations: Option<Vec<crate::api::checks::AnnotationInput>>,
    /// The job summary (markdown written to GITHUB_STEP_SUMMARY), already masked by the runner.
    summary: Option<String>,
}

async fn complete(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Json(b): Json<CompleteBody>) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    let concl = match b.conclusion.as_str() {
        "success" | "failure" | "cancelled" | "skipped" | "timed_out" | "neutral" => b.conclusion.clone(),
        _ => "failure".into(),
    };
    let concl = if job.cancel_requested && concl == "failure" { "cancelled".to_string() } else { concl };
    let outputs = b.outputs.map(|o| match o {
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, Value::String(super::expr::to_string(&v)))).collect()),
        _ => json!({}),
    });
    engine::finish_job(&s, job.id, &concl, outputs, b.steps).await?;
    // Annotations on the job's check run (what `gh run view` lists under ANNOTATIONS), and the
    // job summary (GITHUB_STEP_SUMMARY markdown) as the check run's output text.
    let list = b.annotations.unwrap_or_default();
    let summary: Option<String> = b.summary.filter(|t| !t.trim().is_empty()).map(|t| t.chars().take(65_000).collect());
    if !list.is_empty() || summary.is_some() {
        let repo = db::repo_by_id(&s.db, job.repo_id).await?;
        let owner = db::user_by_id(&s.db, repo.owner_id).await?;
        let ctx = RepoCtx { repo, owner, perm: Perm::Admin };
        let bot = token::bot_user(&s.db).await?;
        let input = crate::api::checks::CheckRunInput {
            output: Some(crate::api::checks::OutputInput {
                title: Some(format!("{} {concl}", job.name)),
                summary: Some(format!("Job `{}` finished with conclusion `{concl}`.", job.name)),
                text: summary,
                annotations: (!list.is_empty()).then(|| list.into_iter().take(50).collect()),
                ..Default::default()
            }),
            ..Default::default()
        };
        if let Err(e) = crate::api::checks::update_check_run(&s, &ctx, &bot, job.id, input).await {
            tracing::warn!(job = job.id, error = %e.message, "could not record annotations");
        }
    }
    // Ephemeral (and JIT) runners are removed after their job.
    if let Some(rid) = job.runner_id {
        sqlx::query("DELETE FROM actions_runners WHERE id = $1 AND ephemeral").bind(rid).execute(&s.db).await?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------------------------
// Artifacts and caches for jobs

#[derive(Deserialize)]
struct ArtifactQuery {
    name: String,
    retention_days: Option<i64>,
}

async fn job_run(s: &AppState, job: &JobRow) -> ApiResult<RunRow> {
    Ok(sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(job.run_id).fetch_one(&s.db).await?)
}

async fn upload_artifact(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Query(q): Query<ArtifactQuery>, body: Body) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    let run = job_run(&s, &job).await?;
    let a = storage::create_artifact(&s, &run, job.id, &q.name, q.retention_days, body).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": a.id, "name": a.name, "size": a.size_in_bytes, "digest": a.digest })))
        .into_response())
}

async fn list_artifacts(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    let rows: Vec<storage::ArtifactRow> = sqlx::query_as("SELECT * FROM actions_artifacts WHERE run_id = $1 AND NOT expired ORDER BY id")
        .bind(job.run_id)
        .fetch_all(&s.db)
        .await?;
    let items: Vec<Value> = rows.iter().map(|a| json!({ "id": a.id, "name": a.name, "size": a.size_in_bytes })).collect();
    Ok(Json(json!({ "artifacts": items })).into_response())
}

async fn download_artifact(State(s): State<AppState>, headers: HeaderMap, Path((id, aid)): Path<(i64, i64)>) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    let a: storage::ArtifactRow = sqlx::query_as("SELECT * FROM actions_artifacts WHERE id = $1 AND repo_id = $2 AND NOT expired")
        .bind(aid)
        .bind(job.repo_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let got = storage::read_object(&s, &a.object_key).await?;
    Ok(([(header::CONTENT_TYPE, "application/zip")], Body::from_stream(got.into_stream())).into_response())
}

#[derive(Deserialize)]
struct CacheQuery {
    keys: Option<String>,
    key: Option<String>,
    version: String,
}

/// Refs whose caches a job may restore: its own ref, then the default branch (and the base
/// branch of a pull request).
fn cache_refs(run: &RunRow, default_branch: &str) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(r) = &run.git_ref {
        v.push(r.clone());
    }
    if let Some(base) = run.pull_requests.pointer("/0/base/ref").and_then(Value::as_str) {
        v.push(format!("refs/heads/{base}"));
    }
    let d = format!("refs/heads/{default_branch}");
    if !v.contains(&d) {
        v.push(d);
    }
    v
}

async fn find_cache(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Query(q): Query<CacheQuery>) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    let run = job_run(&s, &job).await?;
    let repo = db::repo_by_id(&s.db, job.repo_id).await?;
    let keys: Vec<String> = q.keys.unwrap_or_default().split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
    match storage::find_cache(&s, job.repo_id, &keys, &q.version, &cache_refs(&run, &repo.default_branch)).await? {
        Some(c) => Ok(Json(json!({ "id": c.id, "key": c.key, "size": c.size_in_bytes })).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn download_cache(State(s): State<AppState>, headers: HeaderMap, Path((id, cid)): Path<(i64, i64)>) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    let c: storage::CacheRow = sqlx::query_as("SELECT * FROM actions_caches WHERE id = $1 AND repo_id = $2")
        .bind(cid)
        .bind(job.repo_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let got = storage::read_object(&s, &c.object_key).await?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], Body::from_stream(got.into_stream())).into_response())
}

async fn save_cache(State(s): State<AppState>, headers: HeaderMap, Path(id): Path<i64>, Query(q): Query<CacheQuery>, body: Body) -> ApiResult<Response> {
    let job = job_auth(&s, &headers, id).await?;
    let run = job_run(&s, &job).await?;
    let key = q.key.unwrap_or_default();
    let git_ref = run.git_ref.clone().unwrap_or_else(|| format!("refs/heads/{}", run.head_branch.clone().unwrap_or_default()));
    let c = storage::create_cache(&s, job.repo_id, &git_ref, &key, &q.version, body).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": c.id, "key": c.key, "size": c.size_in_bytes }))).into_response())
}
