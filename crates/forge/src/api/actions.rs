//! GitHub Actions REST: workflows, runs, attempts, jobs, logs, artifacts and caches of a
//! repository. Execution lives in `crate::runner` (triggers, engine, `forge runner`).
//! Secrets and variables live in `actions_secrets.rs`; self-hosted runners in
//! `runner_runners.rs`; OIDC, approvals and cache usage in `runner_misc.rs`.
use axum::body::Body;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::plat::total_envelope;
use super::{no_content, PageParams};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::runner::engine::{self, JobRow, Rerun};
use crate::runner::storage::{self, ArtifactRow, CacheRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/actions";
    Router::new()
        .route(&format!("{r}/workflows"), get(list_workflows))
        .route(&format!("{r}/workflows/{{workflow_id}}"), get(get_workflow))
        .route(&format!("{r}/workflows/{{workflow_id}}/enable"), put(enable_workflow))
        .route(&format!("{r}/workflows/{{workflow_id}}/disable"), put(disable_workflow))
        .route(&format!("{r}/workflows/{{workflow_id}}/dispatches"), post(dispatch_workflow))
        .route(&format!("{r}/workflows/{{workflow_id}}/runs"), get(workflow_runs))
        .route(&format!("{r}/runs"), get(repo_runs))
        .route(&format!("{r}/runs/{{run_id}}"), get(get_run).delete(delete_run))
        .route(&format!("{r}/runs/{{run_id}}/cancel"), post(cancel_run_h))
        .route(&format!("{r}/runs/{{run_id}}/force-cancel"), post(force_cancel_h))
        .route(&format!("{r}/runs/{{run_id}}/rerun"), post(rerun_run_h))
        .route(&format!("{r}/runs/{{run_id}}/rerun-failed-jobs"), post(rerun_failed_h))
        .route(&format!("{r}/runs/{{run_id}}/jobs"), get(run_jobs))
        .route(&format!("{r}/runs/{{run_id}}/attempts/{{attempt}}"), get(get_attempt))
        .route(&format!("{r}/runs/{{run_id}}/attempts/{{attempt}}/jobs"), get(attempt_jobs))
        .route(&format!("{r}/runs/{{run_id}}/attempts/{{attempt}}/logs"), get(attempt_logs))
        .route(&format!("{r}/runs/{{run_id}}/artifacts"), get(run_artifacts))
        .route(&format!("{r}/runs/{{run_id}}/logs"), get(run_logs).delete(delete_run_logs))
        .route(&format!("{r}/jobs/{{job_id}}"), get(get_job))
        .route(&format!("{r}/jobs/{{job_id}}/logs"), get(job_logs))
        .route(&format!("{r}/jobs/{{job_id}}/rerun"), post(rerun_job_h))
        .route(&format!("{r}/artifacts"), get(artifacts))
        .route(&format!("{r}/artifacts/{{artifact_id}}"), get(get_artifact).delete(delete_artifact_h))
        .route(&format!("{r}/artifacts/{{artifact_id}}/{{archive_format}}"), get(download_artifact))
        .route(&format!("{r}/caches"), get(caches).delete(delete_caches_by_key))
        .route(&format!("{r}/caches/{{cache_id}}"), axum::routing::delete(delete_cache))
    // `{r}/permissions*` lives in actions_settings::policy; runners in runner_runners.
}

/// Forge-only endpoints for the web UI (mounted under `/api/forge`): per-step job logs, so the
/// run page can show each step's log (live while the job runs) without parsing the whole log.
pub fn forge_routes() -> Router<AppState> {
    Router::new().route("/repos/{owner}/{repo}/actions/jobs/{job_id}/steps", get(job_step_logs))
}

/// `{"job_id", "status", "steps": [{"number", "log"}]}`: the log of each step so far (finished
/// jobs from the compacted log's step ranges, running jobs from the live chunks).
async fn job_step_logs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let (j, _) = job_by_id(&s, &ctx, &id).await?;
    let per = if j.started_at.is_some() { crate::runner::logs::step_logs(&s, &j).await.map_err(ApiError::internal)? } else { Default::default() };
    let steps: Vec<Value> = per.into_iter().map(|(n, b)| json!({ "number": n, "log": String::from_utf8_lossy(&b) })).collect();
    Ok(Json(json!({ "job_id": j.id, "status": j.status, "steps": steps })))
}

/// Signed downloads (followed without credentials after the API's 302, like GitHub's
/// blob-storage redirects). Mounted outside `/api/v3`.
pub fn download_routes() -> Router<AppState> {
    Router::new()
        .route("/_actions/runs/{run_id}/attempts/{attempt}/logs", get(signed_run_logs))
        .route("/_actions/jobs/{job_id}/logs", get(signed_job_logs))
        .route("/_actions/artifacts/{artifact_id}/zip", get(signed_artifact))
}

// ---------------------------------------------------------------------------------------------
// Workflows

/// A workflow file discovered in the repository.
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub path: String,
    /// Declares a `workflow_dispatch` trigger.
    pub dispatchable: bool,
    /// `active` or `disabled_manually`.
    pub state: String,
}

/// Lists `.github/workflows/*.yml|yaml` on the default branch, with their enabled state.
pub async fn workflows(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<Workflow>> {
    let refname = format!("refs/heads/{}", ctx.repo.default_branch);
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    if !refs.contains_key(&refname) {
        return Ok(vec![]);
    }
    let files = crate::runner::trigger::workflow_files(s, ctx.repo.id, &refname).await;
    let states: Vec<(i64, String)> = sqlx::query_as("SELECT workflow_id, state FROM actions_workflow_states WHERE repo_id = $1")
        .bind(ctx.repo.id)
        .fetch_all(&s.db)
        .await?;
    let mut list = Vec::new();
    for (path, text) in files {
        let name = crate::runner::workflow::display_name(&text, &path);
        let id = engine::workflow_id(ctx.repo.id, &path);
        let state = states.iter().find(|(w, _)| *w == id).map(|(_, st)| st.clone()).unwrap_or_else(|| "active".into());
        let dispatchable = match crate::runner::workflow::parse(&text) {
            Ok(w) => w.on.contains_key("workflow_dispatch"),
            Err(_) => text.contains("workflow_dispatch"),
        };
        list.push(Workflow { id, name, path, dispatchable, state });
    }
    Ok(list)
}

fn workflow_json(s: &AppState, ctx: &RepoCtx, w: &Workflow) -> Value {
    let full = ctx.full_name();
    let base = s.base_url();
    json!({
        "id": w.id,
        "node_id": crate::ids::encode("Workflow", w.id),
        "name": w.name,
        "path": w.path,
        "state": w.state,
        "created_at": crate::json::ts(ctx.repo.created_at),
        "updated_at": crate::json::ts(ctx.repo.pushed_at.unwrap_or(ctx.repo.updated_at)),
        "url": format!("{}/repos/{full}/actions/workflows/{}", s.api_url(), w.id),
        "html_url": format!("{base}/{full}/blob/{}/{}", ctx.repo.default_branch, w.path),
        "badge_url": format!("{base}/{full}/workflows/{}/badge.svg", w.name),
    })
}

async fn repo(s: &AppState, u: Option<&User>, owner: &str, name: &str) -> ApiResult<RepoCtx> {
    db::load_repo(&s.db, u, owner, name).await
}

/// Finds a workflow by numeric id or file name (`ci.yml`).
pub async fn find_workflow(s: &AppState, ctx: &RepoCtx, id: &str) -> ApiResult<Workflow> {
    workflows(s, ctx)
        .await?
        .into_iter()
        .find(|w| w.id.to_string() == id || w.path.rsplit('/').next() == Some(id) || w.path == id)
        .ok_or_else(ApiError::not_found)
}

/// Enables or disables a workflow (`state` is `active` or `disabled_manually`).
pub async fn set_workflow_state(s: &AppState, ctx: &RepoCtx, workflow: &Workflow, state: &str) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    sqlx::query(
        "INSERT INTO actions_workflow_states (repo_id, workflow_id, state) VALUES ($1, $2, $3)
         ON CONFLICT (repo_id, workflow_id) DO UPDATE SET state = EXCLUDED.state, updated_at = now()",
    )
    .bind(ctx.repo.id)
    .bind(workflow.id)
    .bind(state)
    .execute(&s.db)
    .await?;
    Ok(())
}

async fn list_workflows(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let wfs = workflows(&s, &ctx).await?;
    let n = wfs.len() as i64;
    Ok(Json(total_envelope("workflows", wfs.iter().map(|w| workflow_json(&s, &ctx, w)).collect(), n)))
}

async fn get_workflow(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let w = find_workflow(&s, &ctx, &id).await?;
    Ok(Json(workflow_json(&s, &ctx, &w)))
}

async fn enable_workflow(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let w = find_workflow(&s, &ctx, &id).await?;
    set_workflow_state(&s, &ctx, &w, "active").await?;
    Ok(no_content())
}

async fn disable_workflow(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let w = find_workflow(&s, &ctx, &id).await?;
    set_workflow_state(&s, &ctx, &w, "disabled_manually").await?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct DispatchBody {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    inputs: Option<Value>,
    return_run_details: Option<bool>,
}

/// Resolves a branch or tag name (or a fully qualified ref) to (branch name if any, full ref,
/// commit sha).
async fn resolve_ref(s: &AppState, ctx: &RepoCtx, r: &str) -> ApiResult<Option<(Option<String>, String, String)>> {
    let candidates: Vec<(Option<String>, String)> = if r.starts_with("refs/") {
        vec![(r.strip_prefix("refs/heads/").map(str::to_string), r.to_string())]
    } else {
        vec![(Some(r.to_string()), format!("refs/heads/{r}")), (None, format!("refs/tags/{r}"))]
    };
    for (branch, full) in candidates {
        let spec = format!("{full}^{{commit}}");
        if let Some(out) = super::plat::git_read(s, ctx.repo.id, &["rev-parse", "--verify", "-q", &spec]).await? {
            let sha = String::from_utf8_lossy(&out).trim().to_string();
            if !sha.is_empty() {
                let branch = branch.or_else(|| full.strip_prefix("refs/tags/").map(str::to_string));
                return Ok(Some((branch, full, sha)));
            }
        }
    }
    Ok(None)
}

/// `simple-commit` JSON for a commit.
pub async fn simple_commit(s: &AppState, ctx: &RepoCtx, sha: &str) -> ApiResult<Option<Value>> {
    let Some(out) = super::plat::git_read(s, ctx.repo.id, &["log", "-1", "--format=%H%x00%T%x00%an%x00%ae%x00%cn%x00%ce%x00%cI%x00%B", sha]).await? else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&out);
    let f: Vec<&str> = text.splitn(8, '\0').collect();
    if f.len() < 8 {
        return Ok(None);
    }
    let when = DateTime::parse_from_rfc3339(f[6]).map(|t| crate::json::ts(t.with_timezone(&Utc))).unwrap_or_default();
    Ok(Some(json!({
        "id": f[0], "tree_id": f[1], "message": f[7].trim_end(), "timestamp": when,
        "author": { "name": f[2], "email": f[3] },
        "committer": { "name": f[4], "email": f[5] },
    })))
}

/// `workflow_dispatch`: creates a run of the workflow at `git_ref` (queued for a runner).
pub async fn dispatch(s: &AppState, ctx: &RepoCtx, actor: &User, workflow: &Workflow, git_ref: &str, inputs: Option<Value>) -> ApiResult<RunRow> {
    ctx.require(Perm::Write)?;
    if workflow.state != "active" {
        return Err(ApiError::unprocessable("Cannot trigger a 'workflow_dispatch' on a disabled workflow"));
    }
    let (branch, full, sha) = resolve_ref(s, ctx, git_ref).await?.ok_or_else(|| ApiError::unprocessable(format!("No ref found for: {git_ref}")))?;
    crate::runner::trigger::dispatch(s, ctx, actor, &workflow.path, branch, &full, &sha, inputs).await
}

async fn dispatch_workflow(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, name, id)): Path<(String, String, String)>,
    Json(body): Json<DispatchBody>,
) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let w = find_workflow(&s, &ctx, &id).await?;
    if !super::actions_settings::actions_allowed(&s, ctx.repo.id).await {
        return Err(ApiError::unprocessable("Actions has been disabled for this repository."));
    }
    let git_ref = body.git_ref.filter(|r| !r.is_empty()).ok_or_else(|| ApiError::validation("Workflow", "ref", "missing_field"))?;
    let run = dispatch(&s, &ctx, &u, &w, &git_ref, body.inputs).await?;
    if body.return_run_details == Some(true) {
        let url = format!("{}/repos/{}/actions/runs/{}", s.api_url(), ctx.full_name(), run.id);
        return Ok(Json(json!({
            "workflow_run_id": run.id,
            "run_url": url,
            "html_url": format!("{}/{}/actions/runs/{}", s.base_url(), ctx.full_name(), run.id),
        }))
        .into_response());
    }
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Runs

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct RunRow {
    pub id: i64,
    pub repo_id: i64,
    pub workflow_id: i64,
    pub workflow_path: String,
    pub name: String,
    pub run_number: i64,
    pub run_attempt: i32,
    pub event: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub head_branch: Option<String>,
    pub head_sha: String,
    pub head_commit: Option<Value>,
    pub inputs: Option<Value>,
    pub actor_id: Option<i64>,
    pub triggering_actor_id: Option<i64>,
    pub run_started_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub git_ref: Option<String>,
    pub github_sha: Option<String>,
    pub display_title: Option<String>,
    pub event_payload: Option<Value>,
    pub definition: Option<Value>,
    pub pull_requests: Value,
    pub head_repo_id: Option<i64>,
    pub completed_at: Option<DateTime<Utc>>,
    pub check_suite_id: Option<i64>,
}

async fn users_by_id(s: &AppState, ids: &[i64]) -> ApiResult<Vec<User>> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(ids).fetch_all(&s.db).await?)
}

#[derive(sqlx::FromRow, Clone)]
struct AttemptRow {
    attempt: i32,
    status: String,
    conclusion: Option<String>,
    triggering_actor_id: Option<i64>,
    run_started_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn run_json(s: &AppState, ctx: &RepoCtx, r: &RunRow, attempt: Option<&AttemptRow>, users: &[User]) -> Value {
    let api = s.api_url();
    let base = s.base_url();
    let full = ctx.full_name();
    let url = format!("{api}/repos/{full}/actions/runs/{}", r.id);
    let attempt_n = attempt.map(|a| a.attempt).unwrap_or(r.run_attempt);
    let user = |id: Option<i64>| -> Value {
        id.and_then(|i| users.iter().find(|u| u.id == i)).map(|u| crate::json::user::simple(&api, base, u)).unwrap_or(Value::Null)
    };
    let repo = crate::json::repo::repo(&api, base, ctx);
    let mut v = json!({
        "id": r.id,
        "name": r.name,
        "node_id": crate::ids::encode("WorkflowRun", r.id),
        "check_suite_id": r.check_suite_id.unwrap_or(r.id),
        "check_suite_node_id": crate::ids::encode("CheckSuite", r.check_suite_id.unwrap_or(r.id)),
        "head_branch": r.head_branch,
        "head_sha": r.head_sha,
        "path": r.workflow_path,
        "display_title": r.display_title.clone().unwrap_or_else(|| r.name.clone()),
        "run_number": r.run_number,
        "run_attempt": attempt_n,
        "event": r.event,
        "status": attempt.map(|a| a.status.clone()).unwrap_or_else(|| r.status.clone()),
        "conclusion": attempt.map(|a| a.conclusion.clone()).unwrap_or_else(|| r.conclusion.clone()),
        "workflow_id": r.workflow_id,
        "url": url,
        "html_url": format!("{base}/{full}/actions/runs/{}", r.id),
        "pull_requests": r.pull_requests,
        "created_at": crate::json::ts(r.created_at),
        "updated_at": crate::json::ts(attempt.map(|a| a.updated_at).unwrap_or(r.updated_at)),
        "run_started_at": crate::json::ts(attempt.map(|a| a.run_started_at).unwrap_or(r.run_started_at)),
        "jobs_url": format!("{url}/jobs"),
        "logs_url": format!("{url}/logs"),
        "check_suite_url": format!("{api}/repos/{full}/check-suites/{}", r.check_suite_id.unwrap_or(r.id)),
        "artifacts_url": format!("{url}/artifacts"),
        "cancel_url": format!("{url}/cancel"),
        "rerun_url": format!("{url}/rerun"),
        "previous_attempt_url": if attempt_n > 1 { Value::String(format!("{url}/attempts/{}", attempt_n - 1)) } else { Value::Null },
        "workflow_url": format!("{api}/repos/{full}/actions/workflows/{}", r.workflow_id),
        "head_commit": r.head_commit.clone().unwrap_or(Value::Null),
        "repository": repo.clone(),
        "head_repository": repo,
        "head_repository_id": r.head_repo_id.unwrap_or(ctx.repo.id),
        "referenced_workflows": [],
    });
    let actor = user(r.actor_id);
    if !actor.is_null() {
        v["actor"] = actor;
    }
    let trig = user(attempt.and_then(|a| a.triggering_actor_id).or(r.triggering_actor_id));
    if !trig.is_null() {
        v["triggering_actor"] = trig;
    }
    v
}

async fn runs_json(s: &AppState, ctx: &RepoCtx, rows: &[RunRow]) -> ApiResult<Vec<Value>> {
    let mut ids: Vec<i64> = rows.iter().flat_map(|r| [r.actor_id, r.triggering_actor_id]).flatten().collect();
    ids.sort_unstable();
    ids.dedup();
    let users = users_by_id(s, &ids).await?;
    Ok(rows.iter().map(|r| run_json(s, ctx, r, None, &users)).collect())
}

pub async fn run_by_id(s: &AppState, ctx: &RepoCtx, id: &str) -> ApiResult<RunRow> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    sqlx::query_as("SELECT * FROM actions_runs WHERE repo_id = $1 AND id = $2")
        .bind(ctx.repo.id)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize, Default)]
struct RunsQuery {
    actor: Option<String>,
    branch: Option<String>,
    event: Option<String>,
    status: Option<String>,
    head_sha: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

const STATUSES: &[&str] = &["requested", "in_progress", "queued", "waiting", "pending", "completed"];

/// Sets the Link header of a `total_count` envelope response.
fn with_links(uri: &OriginalUri, s: &AppState, p: PageParams, total: i64, body: Value) -> Response {
    let links = super::Paged::new(uri, s, p, Some(total), vec![]).into_response();
    let mut res = (StatusCode::OK, Json(body)).into_response();
    if let Some(link) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, link.clone());
    }
    res
}

async fn list_runs(s: &AppState, ctx: &RepoCtx, workflow_id: Option<i64>, q: RunsQuery, uri: &OriginalUri) -> ApiResult<Response> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let actor_id: Option<i64> = match q.actor.as_deref().filter(|a| !a.is_empty()) {
        Some(a) => match db::user_by_login(&s.db, a).await {
            Ok(u) => Some(u.id),
            Err(_) => Some(-1),
        },
        None => None,
    };
    let status = q.status.filter(|x| !x.is_empty());
    let (st, concl) = match status.as_deref() {
        Some(x) if STATUSES.contains(&x) => (Some(x.to_string()), None),
        Some(x) => (None, Some(x.to_string())),
        None => (None, None),
    };
    let filter = "repo_id = $1 AND ($2::bigint IS NULL OR workflow_id = $2) AND ($3::bigint IS NULL OR actor_id = $3)
         AND ($4::text IS NULL OR head_branch = $4) AND ($5::text IS NULL OR event = $5) AND ($6::text IS NULL OR status = $6)
         AND ($7::text IS NULL OR conclusion = $7) AND ($8::text IS NULL OR head_sha = $8)";
    let branch = q.branch.filter(|x| !x.is_empty());
    let event = q.event.filter(|x| !x.is_empty());
    let head_sha = q.head_sha.filter(|x| !x.is_empty());
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM actions_runs WHERE {filter}")))
        .bind(ctx.repo.id)
        .bind(workflow_id)
        .bind(actor_id)
        .bind(&branch)
        .bind(&event)
        .bind(&st)
        .bind(&concl)
        .bind(&head_sha)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<RunRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM actions_runs WHERE {filter} ORDER BY id DESC LIMIT $9 OFFSET $10")))
        .bind(ctx.repo.id)
        .bind(workflow_id)
        .bind(actor_id)
        .bind(&branch)
        .bind(&event)
        .bind(&st)
        .bind(&concl)
        .bind(&head_sha)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = runs_json(s, ctx, &rows).await?;
    Ok(with_links(uri, s, p, total, total_envelope("workflow_runs", items, total)))
}

async fn repo_runs(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, name)): Path<(String, String)>,
    Query(q): Query<RunsQuery>,
) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    list_runs(&s, &ctx, None, q, &uri).await
}

async fn workflow_runs(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, name, id)): Path<(String, String, String)>,
    Query(q): Query<RunsQuery>,
) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let w = find_workflow(&s, &ctx, &id).await?;
    list_runs(&s, &ctx, Some(w.id), q, &uri).await
}

type RunPath = Path<(String, String, String)>;

async fn get_run(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    let mut v = runs_json(&s, &ctx, std::slice::from_ref(&r)).await?;
    Ok(Json(v.remove(0)))
}

async fn load_attempt(s: &AppState, r: &RunRow, attempt: i32) -> ApiResult<AttemptRow> {
    if attempt < 1 || attempt > r.run_attempt {
        return Err(ApiError::not_found());
    }
    let row: Option<AttemptRow> = sqlx::query_as("SELECT attempt, status, conclusion, triggering_actor_id, run_started_at, updated_at FROM actions_run_attempts WHERE run_id = $1 AND attempt = $2")
        .bind(r.id)
        .bind(attempt)
        .fetch_optional(&s.db)
        .await?;
    Ok(row.unwrap_or(AttemptRow {
        attempt,
        status: if attempt < r.run_attempt { "completed".into() } else { r.status.clone() },
        conclusion: if attempt < r.run_attempt { Some("cancelled".into()) } else { r.conclusion.clone() },
        triggering_actor_id: r.triggering_actor_id,
        run_started_at: r.run_started_at,
        updated_at: r.updated_at,
    }))
}

async fn get_attempt(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id, attempt)): Path<(String, String, String, i32)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    let a = load_attempt(&s, &r, attempt).await?;
    let ids: Vec<i64> = [r.actor_id, a.triggering_actor_id].into_iter().flatten().collect();
    let users = users_by_id(&s, &ids).await?;
    Ok(Json(run_json(&s, &ctx, &r, Some(&a), &users)))
}

/// Cancels a run (202); 409 when it is already completed.
pub async fn cancel_run(s: &AppState, ctx: &RepoCtx, run: &RunRow) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    engine::cancel(s, run, false).await
}

/// Re-runs every job of a completed run as its next attempt.
pub async fn rerun(s: &AppState, ctx: &RepoCtx, actor: &User, run: &RunRow) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    engine::rerun(s, run, actor, Rerun::All).await.map(|_| ())
}

/// Deletes a run with its jobs, logs and artifacts.
pub async fn delete_run_svc(s: &AppState, ctx: &RepoCtx, run: &RunRow) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    if run.status != "completed" {
        engine::cancel(s, run, true).await.ok();
    }
    let jobs: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE run_id = $1").bind(run.id).fetch_all(&s.db).await?;
    crate::runner::logs::delete_logs(s, &jobs).await?;
    let arts: Vec<ArtifactRow> = sqlx::query_as("SELECT * FROM actions_artifacts WHERE run_id = $1").bind(run.id).fetch_all(&s.db).await?;
    for a in &arts {
        storage::delete_artifact(s, a).await?;
    }
    sqlx::query("DELETE FROM actions_runs WHERE id = $1").bind(run.id).execute(&s.db).await?;
    Ok(())
}

async fn cancel_run_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    cancel_run(&s, &ctx, &r).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response())
}

async fn force_cancel_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    ctx.require(Perm::Write)?;
    engine::cancel(&s, &r, true).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response())
}

async fn rerun_run_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    rerun(&s, &ctx, &u, &r).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

async fn rerun_failed_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    ctx.require(Perm::Write)?;
    engine::rerun(&s, &r, &u, Rerun::Failed).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

async fn delete_run(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    delete_run_svc(&s, &ctx, &r).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Jobs

pub fn job_json(s: &AppState, ctx: &RepoCtx, run: &RunRow, j: &JobRow) -> Value {
    let api = s.api_url();
    let full = ctx.full_name();
    let status = match j.status.as_str() {
        "pending" => "pending",
        other => other,
    };
    let steps: Vec<Value> = j
        .steps
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|st| {
            json!({
                "name": st["name"].as_str().unwrap_or(""),
                "status": st["status"].as_str().unwrap_or("queued"),
                "conclusion": st.get("conclusion").cloned().unwrap_or(Value::Null),
                "number": st["number"].as_i64().unwrap_or(0),
                "started_at": st.get("started_at").cloned().unwrap_or(Value::Null),
                "completed_at": st.get("completed_at").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    json!({
        "id": j.id,
        "run_id": j.run_id,
        "run_url": format!("{api}/repos/{full}/actions/runs/{}", j.run_id),
        "run_attempt": j.run_attempt,
        "node_id": crate::ids::encode("CheckRun", j.id),
        "head_sha": run.head_sha,
        "url": format!("{api}/repos/{full}/actions/jobs/{}", j.id),
        "html_url": format!("{}/{full}/actions/runs/{}/job/{}", s.base_url(), j.run_id, j.id),
        "status": status,
        "conclusion": j.conclusion,
        "created_at": crate::json::ts(j.created_at),
        "started_at": crate::json::ts(j.started_at.unwrap_or(j.created_at)),
        "completed_at": crate::json::ts_opt(j.completed_at),
        "name": j.name,
        "steps": steps,
        "check_run_url": format!("{api}/repos/{full}/check-runs/{}", j.id),
        "labels": j.labels,
        "runner_id": j.runner_id,
        "runner_name": j.runner_name,
        "runner_group_id": j.runner_id.map(|_| 1),
        "runner_group_name": j.runner_id.map(|_| "Default"),
        "workflow_name": run.name,
        "head_branch": run.head_branch,
    })
}

#[derive(Deserialize, Default)]
struct JobsQuery {
    filter: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_jobs(s: &AppState, ctx: &RepoCtx, run: &RunRow, attempt: Option<i32>, q: JobsQuery, uri: &OriginalUri) -> ApiResult<Response> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let all = attempt.is_none() && q.filter.as_deref() == Some("all");
    let attempt = attempt.unwrap_or(run.run_attempt);
    let filter = "run_id = $1 AND ($2 OR run_attempt = $3)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM actions_jobs WHERE {filter}")))
        .bind(run.id)
        .bind(all)
        .bind(attempt)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<JobRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM actions_jobs WHERE {filter} ORDER BY run_attempt, {JOB_ORDER} LIMIT $4 OFFSET $5")))
        .bind(run.id)
        .bind(all)
        .bind(attempt)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items: Vec<Value> = rows.iter().map(|j| job_json(s, ctx, run, j)).collect();
    Ok(with_links(uri, s, p, total, total_envelope("jobs", items, total)))
}

async fn run_jobs(State(s): State<AppState>, MaybeUser(u): MaybeUser, uri: OriginalUri, Path((owner, name, id)): RunPath, Query(q): Query<JobsQuery>) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    list_jobs(&s, &ctx, &r, None, q, &uri).await
}

async fn attempt_jobs(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, name, id, attempt)): Path<(String, String, String, i32)>,
    Query(q): Query<JobsQuery>,
) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    if attempt < 1 || attempt > r.run_attempt {
        return Err(ApiError::not_found());
    }
    list_jobs(&s, &ctx, &r, Some(attempt), q, &uri).await
}

async fn job_by_id(s: &AppState, ctx: &RepoCtx, id: &str) -> ApiResult<(JobRow, RunRow)> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    let j: JobRow = sqlx::query_as("SELECT * FROM actions_jobs WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let r: RunRow = sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(j.run_id).fetch_one(&s.db).await?;
    Ok((j, r))
}

async fn get_job(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let (j, r) = job_by_id(&s, &ctx, &id).await?;
    Ok(Json(job_json(&s, &ctx, &r, &j)))
}

async fn rerun_job_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let (j, r) = job_by_id(&s, &ctx, &id).await?;
    ctx.require(Perm::Write)?;
    if j.run_attempt != r.run_attempt {
        return Err(ApiError::forbidden("Only jobs of the latest attempt can be re-run"));
    }
    engine::rerun(&s, &r, &u, Rerun::Job(j.id)).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

// ---------------------------------------------------------------------------------------------
// Logs

fn redirect(url: String) -> Response {
    let mut res = StatusCode::FOUND.into_response();
    if let Ok(v) = HeaderValue::from_str(&url) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res
}

async fn signed(s: &AppState, repo_id: i64, what: &str) -> ApiResult<String> {
    crate::signed_url::archive_token(&s.db, repo_id, what).await.map_err(ApiError::internal)
}

async fn attempt_jobs_rows(s: &AppState, run_id: i64, attempt: i32) -> ApiResult<Vec<JobRow>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2 ORDER BY {JOB_ORDER}")))
        .bind(run_id)
        .bind(attempt)
        .fetch_all(&s.db)
        .await?)
}

/// Jobs in workflow-file order: matrix instances (planned later) sort with their job.
const JOB_ORDER: &str = "(SELECT min(o.id) FROM actions_jobs o WHERE o.run_id = actions_jobs.run_id AND o.run_attempt = actions_jobs.run_attempt AND o.job_key = actions_jobs.job_key), id";

async fn logs_redirect(s: &AppState, ctx: &RepoCtx, r: &RunRow, attempt: i32) -> ApiResult<Response> {
    let jobs = attempt_jobs_rows(s, r.id, attempt).await?;
    if !jobs.iter().any(|j| j.started_at.is_some() && j.conclusion.as_deref() != Some("skipped")) {
        return Err(ApiError::not_found());
    }
    let what = format!("actions-run-logs:{}:{attempt}", r.id);
    let tok = signed(s, ctx.repo.id, &what).await?;
    Ok(redirect(format!("{}/_actions/runs/{}/attempts/{attempt}/logs?token={tok}", s.base_url(), r.id)))
}

async fn run_logs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    logs_redirect(&s, &ctx, &r, r.run_attempt).await
}

async fn attempt_logs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id, attempt)): Path<(String, String, String, i32)>) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    if attempt < 1 || attempt > r.run_attempt {
        return Err(ApiError::not_found());
    }
    logs_redirect(&s, &ctx, &r, attempt).await
}

async fn delete_run_logs(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    ctx.require(Perm::Write)?;
    if r.status != "completed" {
        return Err(ApiError::forbidden("Cannot delete logs of a workflow run that is in progress."));
    }
    let jobs: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE run_id = $1").bind(r.id).fetch_all(&s.db).await?;
    crate::runner::logs::delete_logs(&s, &jobs).await?;
    Ok(no_content())
}

async fn job_logs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let (j, _) = job_by_id(&s, &ctx, &id).await?;
    if j.started_at.is_none() {
        return Err(ApiError::not_found());
    }
    let tok = signed(&s, ctx.repo.id, &format!("actions-job-logs:{}", j.id)).await?;
    Ok(redirect(format!("{}/_actions/jobs/{}/logs?token={tok}", s.base_url(), j.id)))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

async fn verify(s: &AppState, repo_id: i64, what: &str, token: Option<&str>) -> ApiResult<()> {
    if crate::signed_url::verify_archive_token(&s.db, repo_id, what, token.unwrap_or("")).await {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

async fn signed_run_logs(State(s): State<AppState>, Path((run_id, attempt)): Path<(i64, i32)>, Query(q): Query<TokenQuery>) -> ApiResult<Response> {
    let r: RunRow = sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(run_id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
    verify(&s, r.repo_id, &format!("actions-run-logs:{run_id}:{attempt}"), q.token.as_deref()).await?;
    let jobs = attempt_jobs_rows(&s, run_id, attempt).await?;
    let zip = crate::runner::logs::run_zip(&s, &jobs).await?;
    let disp = format!("attachment; filename=logs_{run_id}.zip");
    Ok(([(header::CONTENT_TYPE, "application/zip".to_string()), (header::CONTENT_DISPOSITION, disp)], zip).into_response())
}

async fn signed_job_logs(State(s): State<AppState>, Path(job_id): Path<i64>, Query(q): Query<TokenQuery>) -> ApiResult<Response> {
    let j: JobRow = sqlx::query_as("SELECT * FROM actions_jobs WHERE id = $1").bind(job_id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
    verify(&s, j.repo_id, &format!("actions-job-logs:{job_id}"), q.token.as_deref()).await?;
    let log = crate::runner::logs::job_log(&s, &j).await?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], log).into_response())
}

// ---------------------------------------------------------------------------------------------
// Artifacts

pub fn artifact_json(s: &AppState, ctx: &RepoCtx, a: &ArtifactRow) -> Value {
    let url = format!("{}/repos/{}/actions/artifacts/{}", s.api_url(), ctx.full_name(), a.id);
    json!({
        "id": a.id,
        "node_id": crate::ids::encode("Artifact", a.id),
        "name": a.name,
        "size_in_bytes": a.size_in_bytes,
        "url": url,
        "archive_download_url": format!("{url}/zip"),
        "expired": a.expired,
        "created_at": crate::json::ts(a.created_at),
        "expires_at": crate::json::ts_opt(a.expires_at),
        "updated_at": crate::json::ts(a.updated_at),
        "digest": a.digest,
        "workflow_run": a.run_id.map(|rid| json!({
            "id": rid, "repository_id": a.repo_id, "head_repository_id": a.repo_id,
            "head_branch": a.head_branch, "head_sha": a.head_sha,
        })),
    })
}

#[derive(Deserialize, Default)]
struct ArtifactsQuery {
    name: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_artifacts(s: &AppState, ctx: &RepoCtx, run_id: Option<i64>, q: ArtifactsQuery, uri: &OriginalUri) -> ApiResult<Response> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let name = q.name.filter(|n| !n.is_empty());
    let filter = "repo_id = $1 AND ($2::bigint IS NULL OR run_id = $2) AND ($3::text IS NULL OR name = $3)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM actions_artifacts WHERE {filter}")))
        .bind(ctx.repo.id)
        .bind(run_id)
        .bind(&name)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<ArtifactRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM actions_artifacts WHERE {filter} ORDER BY id DESC LIMIT $4 OFFSET $5")))
        .bind(ctx.repo.id)
        .bind(run_id)
        .bind(&name)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = rows.iter().map(|a| artifact_json(s, ctx, a)).collect();
    Ok(with_links(uri, s, p, total, total_envelope("artifacts", items, total)))
}

async fn artifacts(State(s): State<AppState>, MaybeUser(u): MaybeUser, uri: OriginalUri, Path((owner, name)): Path<(String, String)>, Query(q): Query<ArtifactsQuery>) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    list_artifacts(&s, &ctx, None, q, &uri).await
}

async fn run_artifacts(State(s): State<AppState>, MaybeUser(u): MaybeUser, uri: OriginalUri, Path((owner, name, id)): RunPath, Query(q): Query<ArtifactsQuery>) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    list_artifacts(&s, &ctx, Some(r.id), q, &uri).await
}

async fn artifact_by_id(s: &AppState, ctx: &RepoCtx, id: &str) -> ApiResult<ArtifactRow> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    sqlx::query_as("SELECT * FROM actions_artifacts WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_artifact(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let a = artifact_by_id(&s, &ctx, &id).await?;
    Ok(Json(artifact_json(&s, &ctx, &a)))
}

async fn delete_artifact_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let a = artifact_by_id(&s, &ctx, &id).await?;
    ctx.require(Perm::Write)?;
    storage::delete_artifact(&s, &a).await?;
    Ok(no_content())
}

async fn download_artifact(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id, format)): Path<(String, String, String, String)>) -> ApiResult<Response> {
    // GitHub requires authentication to download artifacts, even of public repositories.
    let u = u.ok_or_else(ApiError::unauthorized)?;
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let a = artifact_by_id(&s, &ctx, &id).await?;
    if format != "zip" {
        return Err(ApiError::not_found());
    }
    if a.expired {
        return Err(ApiError::new(StatusCode::GONE, "Artifact has expired"));
    }
    let tok = signed(&s, ctx.repo.id, &format!("actions-artifact:{}", a.id)).await?;
    Ok(redirect(format!("{}/_actions/artifacts/{}/zip?token={tok}", s.base_url(), a.id)))
}

async fn signed_artifact(State(s): State<AppState>, Path(id): Path<i64>, Query(q): Query<TokenQuery>) -> ApiResult<Response> {
    let a: ArtifactRow = sqlx::query_as("SELECT * FROM actions_artifacts WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
    verify(&s, a.repo_id, &format!("actions-artifact:{id}"), q.token.as_deref()).await?;
    if a.expired {
        return Err(ApiError::new(StatusCode::GONE, "Artifact has expired"));
    }
    let got = storage::read_object(&s, &a.object_key).await?;
    let disp = format!("attachment; filename={}.zip", crate::json::release::seg(&a.name));
    let mut res = Response::new(Body::from_stream(got.into_stream()));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(a.size_in_bytes as u64));
    if let Ok(v) = HeaderValue::from_str(&disp) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// Caches

pub fn cache_json(c: &CacheRow) -> Value {
    json!({
        "id": c.id, "ref": c.git_ref, "key": c.key, "version": c.version,
        "last_accessed_at": crate::json::ts(c.last_accessed_at), "created_at": crate::json::ts(c.created_at),
        "size_in_bytes": c.size_in_bytes,
    })
}

#[derive(Deserialize, Default)]
struct CachesQuery {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    key: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn caches(State(s): State<AppState>, MaybeUser(u): MaybeUser, uri: OriginalUri, Path((owner, name)): Path<(String, String)>, Query(q): Query<CachesQuery>) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    ctx.require(Perm::Read)?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let col = match q.sort.as_deref() {
        Some("created_at") => "created_at",
        Some("size_in_bytes") => "size_in_bytes",
        _ => "last_accessed_at",
    };
    let dir = if q.direction.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let git_ref = q.git_ref.filter(|x| !x.is_empty());
    let key = q.key.filter(|x| !x.is_empty());
    let filter = "repo_id = $1 AND ($2::text IS NULL OR git_ref = $2) AND ($3::text IS NULL OR starts_with(key, $3))";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM actions_caches WHERE {filter}")))
        .bind(ctx.repo.id)
        .bind(&git_ref)
        .bind(&key)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<CacheRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM actions_caches WHERE {filter} ORDER BY {col} {dir}, id {dir} LIMIT $4 OFFSET $5")))
        .bind(ctx.repo.id)
        .bind(&git_ref)
        .bind(&key)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = rows.iter().map(cache_json).collect();
    Ok(with_links(&uri, &s, p, total, total_envelope("actions_caches", items, total)))
}

#[derive(Deserialize, Default)]
struct DeleteCacheQuery {
    key: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
}

/// DELETE /caches?key=…[&ref=…]: deletes the matching entries and returns them.
async fn delete_caches_by_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name)): Path<(String, String)>, Query(q): Query<DeleteCacheQuery>) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    ctx.require(Perm::Write)?;
    let key = q.key.filter(|k| !k.is_empty()).ok_or_else(|| ApiError::validation("ActionsCache", "key", "missing_field"))?;
    let rows: Vec<CacheRow> = sqlx::query_as("SELECT * FROM actions_caches WHERE repo_id = $1 AND key = $2 AND ($3::text IS NULL OR git_ref = $3) ORDER BY id")
        .bind(ctx.repo.id)
        .bind(&key)
        .bind(q.git_ref.filter(|r| !r.is_empty()))
        .fetch_all(&s.db)
        .await?;
    if rows.is_empty() {
        return Err(ApiError::not_found());
    }
    for c in &rows {
        storage::delete_cache(&s, c).await?;
    }
    let n = rows.len() as i64;
    Ok(Json(total_envelope("actions_caches", rows.iter().map(cache_json).collect(), n)).into_response())
}

async fn delete_cache(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    ctx.require(Perm::Write)?;
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    let c: CacheRow = sqlx::query_as("SELECT * FROM actions_caches WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    storage::delete_cache(&s, &c).await?;
    Ok(no_content())
}
