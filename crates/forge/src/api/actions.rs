//! GitHub Actions workflows and runs. forge has no runner: workflow files under
//! `.github/workflows` are listed and can be enabled/disabled, a `workflow_dispatch` records a
//! run that stays `queued` (nothing executes it) until it is cancelled, and runs can be viewed,
//! cancelled, re-run (re-queued) and deleted. Jobs, logs and artifacts are validly empty.
//! Secrets and variables live in `actions_secrets.rs`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::plat::total_envelope;
use super::{no_content, PageParams};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
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
        .route(&format!("{r}/runs/{{run_id}}/force-cancel"), post(cancel_run_h))
        .route(&format!("{r}/runs/{{run_id}}/rerun"), post(rerun_run_h))
        .route(&format!("{r}/runs/{{run_id}}/rerun-failed-jobs"), post(rerun_run_h))
        .route(&format!("{r}/runs/{{run_id}}/jobs"), get(run_jobs))
        .route(&format!("{r}/runs/{{run_id}}/attempts/{{attempt}}"), get(get_attempt))
        .route(&format!("{r}/runs/{{run_id}}/attempts/{{attempt}}/jobs"), get(attempt_jobs))
        .route(&format!("{r}/runs/{{run_id}}/artifacts"), get(run_artifacts))
        .route(&format!("{r}/runs/{{run_id}}/logs"), get(run_logs).delete(run_logs))
        .route(&format!("{r}/jobs/{{job_id}}"), get(no_job))
        .route(&format!("{r}/jobs/{{job_id}}/logs"), get(no_job))
        .route(&format!("{r}/jobs/{{job_id}}/rerun"), post(no_job))
        .route(&format!("{r}/artifacts"), get(artifacts))
        .route(&format!("{r}/caches"), get(caches).delete(delete_caches_by_key))
        .route(&format!("{r}/caches/{{cache_id}}"), axum::routing::delete(delete_cache))
        .route(&format!("{r}/runners"), get(runners))
    // `{r}/permissions*` lives in actions_settings::policy.
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

fn workflow_id(repo_id: i64, path: &str) -> i64 {
    let h = Sha256::digest(format!("{repo_id}:{path}").as_bytes());
    // 47 bits: positive and safe for JavaScript clients.
    (u64::from_be_bytes(h[..8].try_into().unwrap()) >> 17) as i64
}

/// Lists `.github/workflows/*.yml|yaml` on the default branch, with their enabled state.
pub async fn workflows(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<Workflow>> {
    let refname = format!("refs/heads/{}", ctx.repo.default_branch);
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    if !refs.contains_key(&refname) {
        return Ok(vec![]);
    }
    let tree = format!("{refname}:.github/workflows");
    let Some(out) = super::plat::git_read(s, ctx.repo.id, &["ls-tree", &tree]).await? else { return Ok(vec![]) };
    let states: Vec<(i64, String)> = sqlx::query_as("SELECT workflow_id, state FROM actions_workflow_states WHERE repo_id = $1")
        .bind(ctx.repo.id)
        .fetch_all(&s.db)
        .await?;
    let mut list = Vec::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let Some((meta, name)) = line.split_once('\t') else { continue };
        let f: Vec<&str> = meta.split_whitespace().collect();
        if f.get(1) != Some(&"blob") || !(name.ends_with(".yml") || name.ends_with(".yaml")) {
            continue;
        }
        let path = format!(".github/workflows/{name}");
        let content = super::plat::git_read(s, ctx.repo.id, &["cat-file", "blob", f[2]]).await?.unwrap_or_default();
        let text = String::from_utf8_lossy(&content);
        let wf_name = text
            .lines()
            .find_map(|l| l.strip_prefix("name:").map(|v| v.trim().trim_matches(['"', '\'']).to_string()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| path.clone());
        let id = workflow_id(ctx.repo.id, &path);
        let state = states.iter().find(|(w, _)| *w == id).map(|(_, st)| st.clone()).unwrap_or_else(|| "active".into());
        list.push(Workflow { id, name: wf_name, path, dispatchable: text.contains("workflow_dispatch"), state });
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
}

/// Resolves a branch or tag name (or a fully qualified ref) to (branch name if any, commit sha).
async fn resolve_ref(s: &AppState, ctx: &RepoCtx, r: &str) -> ApiResult<Option<(Option<String>, String)>> {
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
                return Ok(Some((branch, sha)));
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

/// Records a `workflow_dispatch` run for `git_ref`. The run stays queued: forge has no runner.
pub async fn dispatch(s: &AppState, ctx: &RepoCtx, actor: &User, workflow: &Workflow, git_ref: &str, inputs: Option<Value>) -> ApiResult<RunRow> {
    ctx.require(Perm::Write)?;
    if workflow.state != "active" {
        return Err(ApiError::unprocessable("Cannot trigger a 'workflow_dispatch' on a disabled workflow"));
    }
    if !workflow.dispatchable {
        return Err(ApiError::unprocessable("Workflow does not have 'workflow_dispatch' trigger"));
    }
    let (branch, sha) = resolve_ref(s, ctx, git_ref).await?.ok_or_else(|| ApiError::unprocessable(format!("No ref found for: {git_ref}")))?;
    let head_commit = simple_commit(s, ctx, &sha).await?;
    for _ in 0..3 {
        let row: Option<RunRow> = sqlx::query_as(
            "INSERT INTO actions_runs (repo_id, workflow_id, workflow_path, name, run_number, event, status, head_branch, head_sha,
                 head_commit, inputs, actor_id, triggering_actor_id)
             SELECT $1, $2, $3, $4, COALESCE(MAX(run_number), 0) + 1, 'workflow_dispatch', 'queued', $5, $6, $7, $8, $9, $9
               FROM actions_runs WHERE repo_id = $1 AND workflow_id = $2
             ON CONFLICT (repo_id, workflow_id, run_number) DO NOTHING RETURNING *",
        )
        .bind(ctx.repo.id)
        .bind(workflow.id)
        .bind(&workflow.path)
        .bind(&workflow.name)
        .bind(&branch)
        .bind(&sha)
        .bind(&head_commit)
        .bind(&inputs)
        .bind(actor.id)
        .fetch_optional(&s.db)
        .await?;
        if let Some(r) = row {
            return Ok(r);
        }
    }
    Err(ApiError::conflict("Could not allocate a run number, try again"))
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
    dispatch(&s, &ctx, &u, &w, &git_ref, body.inputs).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Runs

#[derive(sqlx::FromRow, Clone)]
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
}

async fn users_by_id(s: &AppState, ids: &[i64]) -> ApiResult<Vec<User>> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(ids).fetch_all(&s.db).await?)
}

fn run_json(s: &AppState, ctx: &RepoCtx, r: &RunRow, attempt: Option<i32>, users: &[User]) -> Value {
    let api = s.api_url();
    let base = s.base_url();
    let full = ctx.full_name();
    let url = format!("{api}/repos/{full}/actions/runs/{}", r.id);
    let attempt = attempt.unwrap_or(r.run_attempt);
    let user = |id: Option<i64>| -> Value {
        id.and_then(|i| users.iter().find(|u| u.id == i)).map(|u| crate::json::user::simple(&api, base, u)).unwrap_or(Value::Null)
    };
    let repo = crate::json::repo::repo(&api, base, ctx);
    let mut v = json!({
        "id": r.id,
        "name": r.name,
        "node_id": crate::ids::encode("WorkflowRun", r.id),
        "head_branch": r.head_branch,
        "head_sha": r.head_sha,
        "path": r.workflow_path,
        "display_title": r.name,
        "run_number": r.run_number,
        "run_attempt": attempt,
        "event": r.event,
        "status": r.status,
        "conclusion": r.conclusion,
        "workflow_id": r.workflow_id,
        "url": url,
        "html_url": format!("{base}/{full}/actions/runs/{}", r.id),
        "pull_requests": [],
        "created_at": crate::json::ts(r.created_at),
        "updated_at": crate::json::ts(r.updated_at),
        "run_started_at": crate::json::ts(r.run_started_at),
        "jobs_url": format!("{url}/jobs"),
        "logs_url": format!("{url}/logs"),
        "check_suite_url": format!("{api}/repos/{full}/check-suites/{}", r.id),
        "artifacts_url": format!("{url}/artifacts"),
        "cancel_url": format!("{url}/cancel"),
        "rerun_url": format!("{url}/rerun"),
        "previous_attempt_url": if attempt > 1 { Value::String(format!("{url}/attempts/{}", attempt - 1)) } else { Value::Null },
        "workflow_url": format!("{api}/repos/{full}/actions/workflows/{}", r.workflow_id),
        "head_commit": r.head_commit.clone().unwrap_or(Value::Null),
        "repository": repo.clone(),
        "head_repository": repo,
        "head_repository_id": ctx.repo.id,
        "referenced_workflows": [],
    });
    let actor = user(r.actor_id);
    if !actor.is_null() {
        v["actor"] = actor;
    }
    let trig = user(r.triggering_actor_id);
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
    // The body is the `total_count` envelope; the Link header comes from the generic Paged helper
    // (with a known total it doesn't need the items).
    let links = super::Paged::new(uri, s, p, Some(total), vec![]).into_response();
    let mut res = (StatusCode::OK, Json(total_envelope("workflow_runs", items, total))).into_response();
    if let Some(link) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, link.clone());
    }
    Ok(res)
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

async fn get_attempt(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id, attempt)): Path<(String, String, String, i32)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    if attempt < 1 || attempt > r.run_attempt {
        return Err(ApiError::not_found());
    }
    let mut ids: Vec<i64> = [r.actor_id, r.triggering_actor_id].into_iter().flatten().collect();
    ids.dedup();
    let users = users_by_id(&s, &ids).await?;
    let mut v = run_json(&s, &ctx, &r, Some(attempt), &users);
    if attempt < r.run_attempt {
        // Earlier attempts ended when they were cancelled/re-run.
        v["status"] = json!("completed");
        v["conclusion"] = json!("cancelled");
    }
    Ok(Json(v))
}

/// Cancels a queued run (status `completed`, conclusion `cancelled`); 409 when already completed.
pub async fn cancel_run(s: &AppState, ctx: &RepoCtx, run: &RunRow) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    let n = sqlx::query("UPDATE actions_runs SET status = 'completed', conclusion = 'cancelled', updated_at = now() WHERE id = $1 AND status <> 'completed'")
        .bind(run.id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::conflict("Cannot cancel a workflow run that is completed."));
    }
    Ok(())
}

/// Re-queues a completed run as its next attempt.
pub async fn rerun(s: &AppState, ctx: &RepoCtx, actor: &User, run: &RunRow) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    let n = sqlx::query(
        "UPDATE actions_runs SET status = 'queued', conclusion = NULL, run_attempt = run_attempt + 1, triggering_actor_id = $2,
             run_started_at = now(), updated_at = now() WHERE id = $1 AND status = 'completed'",
    )
    .bind(run.id)
    .bind(actor.id)
    .execute(&s.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::forbidden(format!("This workflow run is not completed and cannot be re-run: {}", run.id)));
    }
    Ok(())
}

pub async fn delete_run_svc(s: &AppState, ctx: &RepoCtx, run: &RunRow) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    sqlx::query("DELETE FROM actions_runs WHERE id = $1").bind(run.id).execute(&s.db).await?;
    Ok(())
}

async fn cancel_run_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    cancel_run(&s, &ctx, &r).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response())
}

async fn rerun_run_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    rerun(&s, &ctx, &u, &r).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

async fn delete_run(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    delete_run_svc(&s, &ctx, &r).await?;
    Ok(no_content())
}

/// No jobs ever start, so every run's job list is empty.
async fn run_jobs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    run_by_id(&s, &ctx, &id).await?;
    Ok(Json(total_envelope("jobs", vec![], 0)))
}

async fn attempt_jobs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id, attempt)): Path<(String, String, String, i32)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let r = run_by_id(&s, &ctx, &id).await?;
    if attempt < 1 || attempt > r.run_attempt {
        return Err(ApiError::not_found());
    }
    Ok(Json(total_envelope("jobs", vec![], 0)))
}

async fn run_artifacts(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    run_by_id(&s, &ctx, &id).await?;
    Ok(Json(total_envelope("artifacts", vec![], 0)))
}

/// Runs never produce logs.
async fn run_logs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): RunPath) -> ApiResult<Response> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    run_by_id(&s, &ctx, &id).await?;
    Err(ApiError::not_found())
}

async fn no_job(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, _)): RunPath) -> ApiResult<Json<Value>> {
    repo(&s, u.as_ref(), &owner, &name).await?;
    Err(ApiError::not_found())
}

// ---------------------------------------------------------------------------------------------
// Artifacts, caches, runners, permissions (always empty / defaults)

async fn artifacts(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    repo(&s, u.as_ref(), &owner, &name).await?;
    Ok(Json(total_envelope("artifacts", vec![], 0)))
}

async fn caches(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    repo(&s, u.as_ref(), &owner, &name).await?;
    Ok(Json(total_envelope("actions_caches", vec![], 0)))
}

/// DELETE /caches?key=… — no caches exist, so any key is not found.
async fn delete_caches_by_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    ctx.require(Perm::Write)?;
    Err(ApiError::not_found())
}

async fn delete_cache(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, name, _)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = repo(&s, Some(&u), &owner, &name).await?;
    ctx.require(Perm::Write)?;
    Err(ApiError::not_found())
}

async fn runners(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    ctx.require(Perm::Admin)?;
    Ok(Json(total_envelope("runners", vec![], 0)))
}
