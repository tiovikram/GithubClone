//! Checks API: check runs, their annotations, and check suites.
//!
//! forge has no GitHub Apps. Every suite and run belongs to one built-in, synthetic app
//! ("forge", [`FORGE_APP_ID`], rendered with GitHub's `integration` schema), and any user with
//! write access to a repository may create and update check runs (GitHub requires an app
//! installation token). A check suite exists per (repository, head_sha, app): it is created on
//! demand by `POST /check-suites` or implicitly by the first check run on that commit. A suite's
//! status/conclusion are derived from its runs (the stored values only apply to a suite without
//! runs). Annotations sent in `output.annotations` are appended on create and on every update,
//! like GitHub. `PATCH /check-suites/preferences` (`auto_trigger_checks`) is stored per repo and
//! makes pushes create the forge app's suite for new branch heads. Commit statuses live in `statuses.rs`; the GraphQL `statusCheckRollup` merges
//! both via [`latest_runs_for_sha`].
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::gitapi::{self, GitResult};
use super::{PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::read;
use crate::state::AppState;

/// Id of the built-in app that owns every check suite and run.
pub const FORGE_APP_ID: i64 = 1;

pub const RUN_STATUSES: &[&str] = &["queued", "in_progress", "completed", "waiting", "requested", "pending"];
pub const CONCLUSIONS: &[&str] = &["action_required", "cancelled", "failure", "neutral", "success", "skipped", "stale", "timed_out"];
const LEVELS: &[&str] = &["notice", "warning", "failure"];
const MAX_ANNOTATIONS: usize = 50;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}";
    Router::new()
        .route(&format!("{r}/check-runs"), post(create_run_h))
        .route(&format!("{r}/check-runs/{{id}}"), get(get_run_h).patch(update_run_h))
        .route(&format!("{r}/check-runs/{{id}}/annotations"), get(annotations_h))
        .route(&format!("{r}/check-runs/{{id}}/rerequest"), post(rerequest_run_h))
        .route(&format!("{r}/check-suites"), post(create_suite_h))
        .route(&format!("{r}/check-suites/preferences"), axum::routing::patch(suite_preferences_h))
        .route(&format!("{r}/check-suites/{{id}}"), get(get_suite_h))
        .route(&format!("{r}/check-suites/{{id}}/check-runs"), get(suite_runs_h))
        .route(&format!("{r}/check-suites/{{id}}/rerequest"), post(rerequest_suite_h))
}

// ---------------------------------------------------------------------------------------------
// Rows

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SuiteRow {
    pub id: i64,
    pub repo_id: i64,
    pub app_id: i64,
    pub head_sha: String,
    pub head_branch: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub creator_id: Option<i64>,
    pub rerequested_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CheckRunRow {
    pub id: i64,
    pub repo_id: i64,
    pub suite_id: i64,
    pub head_sha: String,
    pub name: String,
    pub external_id: Option<String>,
    pub details_url: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub output_title: Option<String>,
    pub output_summary: Option<String>,
    pub output_text: Option<String>,
    pub output_images: Value,
    pub actions: Value,
    pub creator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AnnotationRow {
    pub id: i64,
    pub check_run_id: i64,
    pub path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub start_column: Option<i32>,
    pub end_column: Option<i32>,
    pub annotation_level: String,
    pub title: Option<String>,
    pub message: String,
    pub raw_details: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Inputs

#[derive(Deserialize, Debug, Clone, Default)]
pub struct AnnotationInput {
    pub path: Option<String>,
    pub start_line: Option<i32>,
    pub end_line: Option<i32>,
    pub start_column: Option<i32>,
    pub end_column: Option<i32>,
    pub annotation_level: Option<String>,
    pub message: Option<String>,
    pub title: Option<String>,
    pub raw_details: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct OutputInput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub text: Option<String>,
    pub annotations: Option<Vec<AnnotationInput>>,
    pub images: Option<Value>,
}

/// Body of `POST /check-runs` and `PATCH /check-runs/{id}` (`head_sha` only on create).
#[derive(Deserialize, Debug, Clone, Default)]
pub struct CheckRunInput {
    pub name: Option<String>,
    pub head_sha: Option<String>,
    pub details_url: Option<String>,
    pub external_id: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub output: Option<OutputInput>,
    pub actions: Option<Value>,
}

fn invalid(field: &str, value: &str) -> ApiError {
    let mut e = ApiError::validation("CheckRun", field, "invalid");
    e.errors = Some(json!([{"resource": "CheckRun", "field": field, "code": "invalid", "value": value}]));
    e
}

fn validate_annotations(list: &[AnnotationInput]) -> ApiResult<()> {
    if list.len() > MAX_ANNOTATIONS {
        return Err(ApiError::unprocessable(format!("Only {MAX_ANNOTATIONS} annotations are allowed per request")));
    }
    for a in list {
        for (f, missing) in [
            ("path", a.path.as_deref().is_none_or(str::is_empty)),
            ("start_line", a.start_line.is_none()),
            ("end_line", a.end_line.is_none()),
            ("annotation_level", a.annotation_level.is_none()),
            ("message", a.message.as_deref().is_none_or(str::is_empty)),
        ] {
            if missing {
                return Err(ApiError::validation("CheckRunAnnotation", f, "missing_field"));
            }
        }
        let level = a.annotation_level.as_deref().unwrap_or_default();
        if !LEVELS.contains(&level) {
            return Err(invalid("annotation_level", level));
        }
        if a.start_line.unwrap_or(0) < 1 || a.end_line.unwrap_or(0) < a.start_line.unwrap_or(0) {
            return Err(invalid("end_line", &a.end_line.unwrap_or(0).to_string()));
        }
    }
    Ok(())
}

/// Validated status/conclusion pair: a conclusion forces `completed`; `completed` needs one.
fn status_pair(status: Option<&str>, conclusion: Option<&str>, current: Option<(&str, Option<&str>)>) -> ApiResult<(String, Option<String>)> {
    if let Some(s) = status {
        if !RUN_STATUSES.contains(&s) {
            return Err(invalid("status", s));
        }
    }
    if let Some(c) = conclusion {
        if !CONCLUSIONS.contains(&c) {
            return Err(invalid("conclusion", c));
        }
        return Ok(("completed".into(), Some(c.into())));
    }
    match (status, current) {
        (Some("completed"), Some((_, Some(c)))) => Ok(("completed".into(), Some(c.into()))),
        (Some("completed"), _) => Err(ApiError::validation("CheckRun", "conclusion", "missing_field")),
        (Some(s), _) => Ok((s.into(), None)),
        (None, Some((s, c))) => Ok((s.into(), c.map(str::to_string))),
        (None, None) => Ok(("queued".into(), None)),
    }
}

// ---------------------------------------------------------------------------------------------
// Service layer

/// Resolves a commit sha (or ref) of the repository; 422 like GitHub when there is none.
pub async fn resolve_sha(s: &AppState, ctx: &RepoCtx, rev: &str) -> ApiResult<String> {
    if gitapi::is_empty(s, ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let path = gitapi::sync(s, ctx).await?;
    read::resolve_commit(&s.git.git, &path, rev)
        .await
        .api()?
        .ok_or_else(|| ApiError::unprocessable(format!("No commit found for SHA: {rev}")))
}

/// A branch whose tip is `sha` (the default branch preferred), for `head_branch`.
async fn branch_at(s: &AppState, ctx: &RepoCtx, sha: &str) -> ApiResult<Option<String>> {
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    let default = format!("refs/heads/{}", ctx.repo.default_branch);
    if refs.get(&default).map(String::as_str) == Some(sha) {
        return Ok(Some(ctx.repo.default_branch.clone()));
    }
    Ok(refs
        .iter()
        .find(|(name, target)| name.starts_with("refs/heads/") && target.as_str() == sha)
        .map(|(name, _)| name["refs/heads/".len()..].to_string()))
}

/// The forge app's suite for `sha` (a resolved commit), created if missing.
/// Returns the suite and whether it was created now.
pub async fn ensure_suite(s: &AppState, ctx: &RepoCtx, actor: &User, sha: &str) -> ApiResult<(SuiteRow, bool)> {
    ensure_suite_by(s, ctx, Some(actor.id), sha).await
}

async fn ensure_suite_by(s: &AppState, ctx: &RepoCtx, creator: Option<i64>, sha: &str) -> ApiResult<(SuiteRow, bool)> {
    let branch = branch_at(s, ctx, sha).await?;
    let inserted: Option<SuiteRow> = sqlx::query_as(
        "INSERT INTO check_suites (repo_id, app_id, head_sha, head_branch, creator_id) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (repo_id, head_sha, app_id) DO NOTHING RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(FORGE_APP_ID)
    .bind(sha)
    .bind(&branch)
    .bind(creator)
    .fetch_optional(&s.db)
    .await?;
    if let Some(row) = inserted {
        return Ok((row, true));
    }
    let row: SuiteRow = sqlx::query_as("SELECT * FROM check_suites WHERE repo_id = $1 AND head_sha = $2 AND app_id = $3")
        .bind(ctx.repo.id)
        .bind(sha)
        .bind(FORGE_APP_ID)
        .fetch_one(&s.db)
        .await?;
    Ok((row, false))
}

async fn insert_annotations(s: &AppState, run_id: i64, list: &[AnnotationInput]) -> ApiResult<()> {
    for a in list {
        // Columns only apply to single-line annotations (GitHub drops them otherwise).
        let single = a.start_line == a.end_line;
        sqlx::query(
            "INSERT INTO check_annotations (check_run_id, path, start_line, end_line, start_column, end_column,
                 annotation_level, title, message, raw_details)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(run_id)
        .bind(a.path.as_deref().unwrap_or_default())
        .bind(a.start_line.unwrap_or(1))
        .bind(a.end_line.unwrap_or(1))
        .bind(a.start_column.filter(|_| single))
        .bind(a.end_column.filter(|_| single))
        .bind(a.annotation_level.as_deref().unwrap_or("notice"))
        .bind(a.title.as_deref().filter(|t| !t.is_empty()))
        .bind(a.message.as_deref().unwrap_or_default())
        .bind(a.raw_details.as_deref().filter(|t| !t.is_empty()))
        .execute(&s.db)
        .await?;
    }
    Ok(())
}

fn check_output(o: &OutputInput, create: bool) -> ApiResult<()> {
    if create && o.title.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::validation("CheckRun", "output.title", "missing_field"));
    }
    if o.summary.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::validation("CheckRun", "output.summary", "missing_field"));
    }
    validate_annotations(o.annotations.as_deref().unwrap_or_default())
}

async fn emit(s: &AppState, ctx: &RepoCtx, actor: &User, event: &str, action: &str, key: &str, object: Value) {
    let payload = json!({
        "action": action,
        key: object,
        "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor),
    });
    if let Err(e) = crate::webhooks::enqueue_event_ctx(s, ctx, event, payload).await {
        tracing::error!(error = %format!("{e:#}"), "{event} webhook failed");
    }
}

/// Creates a check run on `input.head_sha` (auto-creating the commit's check suite).
pub async fn create_check_run(s: &AppState, ctx: &RepoCtx, actor: &User, input: CheckRunInput) -> ApiResult<CheckRunRow> {
    let name = input.name.clone().filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("CheckRun", "name", "missing_field"))?;
    let head = input.head_sha.clone().filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("CheckRun", "head_sha", "missing_field"))?;
    let (status, conclusion) = status_pair(input.status.as_deref(), input.conclusion.as_deref(), None)?;
    if let Some(o) = &input.output {
        check_output(o, true)?;
    }
    let sha = resolve_sha(s, ctx, &head).await?;
    let (suite, suite_new) = ensure_suite(s, ctx, actor, &sha).await?;
    let now = Utc::now();
    let started = input.started_at.unwrap_or(now);
    let completed = if status == "completed" { Some(input.completed_at.unwrap_or(now)) } else { None };
    let out = input.output.clone().unwrap_or_default();
    let row: CheckRunRow = sqlx::query_as(
        "INSERT INTO check_runs (repo_id, suite_id, head_sha, name, external_id, details_url, status, conclusion,
             started_at, completed_at, output_title, output_summary, output_text, output_images, actions, creator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(suite.id)
    .bind(&sha)
    .bind(name.trim())
    .bind(input.external_id.as_deref())
    .bind(input.details_url.as_deref().filter(|u| !u.is_empty()))
    .bind(&status)
    .bind(&conclusion)
    .bind(started)
    .bind(completed)
    .bind(out.title.as_deref())
    .bind(out.summary.as_deref())
    .bind(out.text.as_deref())
    .bind(out.images.clone().filter(Value::is_array).unwrap_or_else(|| json!([])))
    .bind(input.actions.clone().filter(Value::is_array).unwrap_or_else(|| json!([])))
    .bind(actor.id)
    .fetch_one(&s.db)
    .await?;
    insert_annotations(s, row.id, out.annotations.as_deref().unwrap_or_default()).await?;
    sqlx::query("UPDATE check_suites SET updated_at = now() WHERE id = $1").bind(suite.id).execute(&s.db).await?;
    if let Ok(v) = render_runs(s, ctx, std::slice::from_ref(&row)).await {
        if suite_new {
            if let Ok(sv) = render_suites(s, ctx, std::slice::from_ref(&suite)).await {
                emit(s, ctx, actor, "check_suite", "requested", "check_suite", sv.into_iter().next().unwrap_or(Value::Null)).await;
            }
        }
        let v = v.into_iter().next().unwrap_or(Value::Null);
        emit(s, ctx, actor, "check_run", "created", "check_run", v.clone()).await;
        if row.status == "completed" {
            emit(s, ctx, actor, "check_run", "completed", "check_run", v).await;
        }
    }
    Ok(row)
}

pub async fn run_by_id(s: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<CheckRunRow> {
    sqlx::query_as("SELECT * FROM check_runs WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn suite_by_id(s: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<SuiteRow> {
    sqlx::query_as("SELECT * FROM check_suites WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Updates a check run. Absent fields are kept; `output.annotations` are appended.
pub async fn update_check_run(s: &AppState, ctx: &RepoCtx, actor: &User, id: i64, input: CheckRunInput) -> ApiResult<CheckRunRow> {
    let cur = run_by_id(s, ctx, id).await?;
    let (status, conclusion) = status_pair(input.status.as_deref(), input.conclusion.as_deref(), Some((&cur.status, cur.conclusion.as_deref())))?;
    if let Some(o) = &input.output {
        check_output(o, false)?;
    }
    if let Some(n) = &input.name {
        if n.trim().is_empty() {
            return Err(ApiError::validation("CheckRun", "name", "missing_field"));
        }
    }
    let completed = if status == "completed" { Some(input.completed_at.or(cur.completed_at).unwrap_or_else(Utc::now)) } else { None };
    let out = input.output.clone();
    let row: CheckRunRow = sqlx::query_as(
        "UPDATE check_runs SET name = $2, external_id = $3, details_url = $4, status = $5, conclusion = $6,
             started_at = $7, completed_at = $8, output_title = $9, output_summary = $10, output_text = $11,
             output_images = $12, actions = $13, updated_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(cur.id)
    .bind(input.name.as_deref().map(str::trim).unwrap_or(&cur.name))
    .bind(input.external_id.clone().or(cur.external_id.clone()))
    .bind(input.details_url.clone().filter(|u| !u.is_empty()).or(cur.details_url.clone()))
    .bind(&status)
    .bind(&conclusion)
    .bind(input.started_at.or(cur.started_at))
    .bind(completed)
    .bind(out.as_ref().and_then(|o| o.title.clone()).or(cur.output_title.clone()))
    .bind(out.as_ref().and_then(|o| o.summary.clone()).or(cur.output_summary.clone()))
    .bind(out.as_ref().and_then(|o| o.text.clone()).or(cur.output_text.clone()))
    .bind(out.as_ref().and_then(|o| o.images.clone()).filter(Value::is_array).unwrap_or(cur.output_images.clone()))
    .bind(input.actions.clone().filter(Value::is_array).unwrap_or(cur.actions.clone()))
    .fetch_one(&s.db)
    .await?;
    if let Some(o) = &out {
        insert_annotations(s, row.id, o.annotations.as_deref().unwrap_or_default()).await?;
    }
    sqlx::query("UPDATE check_suites SET updated_at = now() WHERE id = $1").bind(row.suite_id).execute(&s.db).await?;
    if cur.status != "completed" && row.status == "completed" {
        if let Ok(v) = render_runs(s, ctx, std::slice::from_ref(&row)).await {
            emit(s, ctx, actor, "check_run", "completed", "check_run", v.into_iter().next().unwrap_or(Value::Null)).await;
        }
    }
    Ok(row)
}

/// Re-requests a suite: GitHub re-queues it for its app. forge's app has nothing to re-run, so
/// this records the request (and emits the `check_suite` `rerequested` webhook).
pub async fn rerequest_suite(s: &AppState, ctx: &RepoCtx, actor: &User, id: i64) -> ApiResult<SuiteRow> {
    let suite = suite_by_id(s, ctx, id).await?;
    let row: SuiteRow = sqlx::query_as("UPDATE check_suites SET rerequested_at = now(), updated_at = now() WHERE id = $1 RETURNING *")
        .bind(suite.id)
        .fetch_one(&s.db)
        .await?;
    if let Ok(v) = render_suites(s, ctx, std::slice::from_ref(&row)).await {
        emit(s, ctx, actor, "check_suite", "rerequested", "check_suite", v.into_iter().next().unwrap_or(Value::Null)).await;
    }
    Ok(row)
}

/// The latest check run per name on `sha` (what GitHub's rollups and `filter=latest` show).
pub async fn latest_runs_for_sha(db: &sqlx::PgPool, repo_id: i64, sha: &str) -> sqlx::Result<Vec<CheckRunRow>> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT ON (name) * FROM check_runs WHERE repo_id = $1 AND head_sha = $2 ORDER BY name, id DESC",
    )
    .bind(repo_id)
    .bind(sha)
    .fetch_all(db)
    .await?)
}

/// (status, conclusion) of a suite from its runs (stored values when it has none).
pub fn suite_state(suite: &SuiteRow, runs: &[&CheckRunRow]) -> (String, Option<String>) {
    if runs.is_empty() {
        return (suite.status.clone(), suite.conclusion.clone());
    }
    if runs.iter().all(|r| r.status == "completed") {
        let has = |c: &str| runs.iter().any(|r| r.conclusion.as_deref() == Some(c));
        let order = ["action_required", "failure", "timed_out", "cancelled", "stale", "success", "neutral", "skipped"];
        let c = order.iter().find(|c| has(c)).map(|c| c.to_string()).unwrap_or_else(|| "success".into());
        return ("completed".into(), Some(c));
    }
    if runs.iter().any(|r| r.status == "in_progress") {
        ("in_progress".into(), None)
    } else {
        ("queued".into(), None)
    }
}

// ---------------------------------------------------------------------------------------------
// JSON

/// The synthetic "forge" app (`integration` schema).
pub fn app_json(s: &AppState) -> Value {
    let api = s.api_url();
    let base = s.base_url();
    let created = crate::json::ts(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
    let owner_url = format!("{api}/users/forge");
    json!({
        "id": FORGE_APP_ID,
        "slug": "forge",
        "node_id": crate::ids::encode("App", FORGE_APP_ID),
        "client_id": "Iv1.forge",
        "owner": {
            "login": "forge", "id": 0, "node_id": crate::ids::encode("Organization", 0),
            "avatar_url": format!("{base}/avatars/forge"), "gravatar_id": "",
            "url": owner_url, "html_url": format!("{base}/forge"),
            "followers_url": format!("{owner_url}/followers"),
            "following_url": format!("{owner_url}/following{{/other_user}}"),
            "gists_url": format!("{owner_url}/gists{{/gist_id}}"),
            "starred_url": format!("{owner_url}/starred{{/owner}}{{/repo}}"),
            "subscriptions_url": format!("{owner_url}/subscriptions"),
            "organizations_url": format!("{owner_url}/orgs"),
            "repos_url": format!("{owner_url}/repos"),
            "events_url": format!("{owner_url}/events{{/privacy}}"),
            "received_events_url": format!("{owner_url}/received_events"),
            "type": "Organization", "user_view_type": "public", "site_admin": false,
        },
        "name": "forge",
        "description": "Built-in checks app: check runs created through the Checks API by users with write access.",
        "external_url": base,
        "html_url": format!("{base}/apps/forge"),
        "created_at": created,
        "updated_at": created,
        "permissions": { "checks": "write", "metadata": "read", "statuses": "write", "contents": "read", "pull_requests": "read" },
        "events": ["check_run", "check_suite"],
    })
}

/// `pull-request-minimal` objects of the open PRs of `ctx` whose head is one of `shas`.
async fn pulls_for_shas(s: &AppState, ctx: &RepoCtx, shas: &[String]) -> ApiResult<HashMap<String, Vec<Value>>> {
    let rows: Vec<(i64, i64, String, String, String, String, Option<i64>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT i.id, i.number, p.head_ref, p.head_sha, p.base_ref, p.base_sha, p.head_repo_id, hr.name, hu.login
           FROM pull_requests p JOIN issues i ON i.id = p.issue_id
           LEFT JOIN repositories hr ON hr.id = p.head_repo_id LEFT JOIN users hu ON hu.id = hr.owner_id
          WHERE p.repo_id = $1 AND p.head_sha = ANY($2) AND i.state = 'open' ORDER BY i.number",
    )
    .bind(ctx.repo.id)
    .bind(shas)
    .fetch_all(&s.db)
    .await?;
    let api = s.api_url();
    let full = ctx.full_name();
    let base_repo = json!({ "id": ctx.repo.id, "url": format!("{api}/repos/{full}"), "name": ctx.repo.name });
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    for (id, number, head_ref, head_sha, base_ref, base_sha, head_repo_id, hname, hlogin) in rows {
        let head_repo = match (head_repo_id, hname, hlogin) {
            (Some(rid), Some(n), Some(l)) => json!({ "id": rid, "url": format!("{api}/repos/{l}/{n}"), "name": n }),
            _ => base_repo.clone(),
        };
        out.entry(head_sha.clone()).or_default().push(json!({
            "id": id,
            "number": number,
            "url": format!("{api}/repos/{full}/pulls/{number}"),
            "head": { "ref": head_ref, "sha": head_sha, "repo": head_repo },
            "base": { "ref": base_ref, "sha": base_sha, "repo": base_repo },
        }));
    }
    Ok(out)
}

/// Renders check runs (`check-run` schema), batching annotation counts and PR lookups.
pub async fn render_runs(s: &AppState, ctx: &RepoCtx, rows: &[CheckRunRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let counts: Vec<(i64, i64)> = sqlx::query_as("SELECT check_run_id, count(*) FROM check_annotations WHERE check_run_id = ANY($1) GROUP BY check_run_id")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let counts: HashMap<i64, i64> = counts.into_iter().collect();
    let mut shas: Vec<String> = rows.iter().map(|r| r.head_sha.clone()).collect();
    shas.dedup();
    let pulls = pulls_for_shas(s, ctx, &shas).await?;
    let api = s.api_url();
    let base = s.base_url();
    let full = ctx.full_name();
    let app = app_json(s);
    Ok(rows
        .iter()
        .map(|r| {
            let url = format!("{api}/repos/{full}/check-runs/{}", r.id);
            let html = format!("{base}/{full}/runs/{}", r.id);
            json!({
                "id": r.id,
                "head_sha": r.head_sha,
                "node_id": crate::ids::encode("CheckRun", r.id),
                "external_id": r.external_id.clone().unwrap_or_default(),
                "url": url,
                "html_url": html,
                "details_url": r.details_url.clone().unwrap_or_else(|| html.clone()),
                "status": r.status,
                "conclusion": r.conclusion,
                "started_at": crate::json::ts_opt(r.started_at),
                "completed_at": crate::json::ts_opt(r.completed_at),
                "output": {
                    "title": r.output_title,
                    "summary": r.output_summary,
                    "text": r.output_text,
                    "annotations_count": counts.get(&r.id).copied().unwrap_or(0),
                    "annotations_url": format!("{url}/annotations"),
                },
                "name": r.name,
                "check_suite": { "id": r.suite_id },
                "app": app,
                "pull_requests": pulls.get(&r.head_sha).cloned().unwrap_or_default(),
            })
        })
        .collect())
}

fn fallback_commit(sha: &str) -> Value {
    json!({ "id": sha, "tree_id": "", "message": "", "timestamp": crate::json::ts(Utc::now()), "author": null, "committer": null })
}

/// Renders check suites (`check-suite` schema).
pub async fn render_suites(s: &AppState, ctx: &RepoCtx, suites: &[SuiteRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = suites.iter().map(|x| x.id).collect();
    let runs: Vec<CheckRunRow> = sqlx::query_as("SELECT DISTINCT ON (suite_id, name) * FROM check_runs WHERE suite_id = ANY($1) ORDER BY suite_id, name, id DESC")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let shas: Vec<String> = suites.iter().map(|x| x.head_sha.clone()).collect();
    let pulls = pulls_for_shas(s, ctx, &shas).await?;
    let api = s.api_url();
    let full = ctx.full_name();
    let repo = crate::json::repo::repo(&api, s.base_url(), ctx);
    let app = app_json(s);
    let mut out = Vec::with_capacity(suites.len());
    for x in suites {
        let mine: Vec<&CheckRunRow> = runs.iter().filter(|r| r.suite_id == x.id).collect();
        let (status, conclusion) = suite_state(x, &mine);
        let head_commit = super::actions::simple_commit(s, ctx, &x.head_sha).await?.unwrap_or_else(|| fallback_commit(&x.head_sha));
        let url = format!("{api}/repos/{full}/check-suites/{}", x.id);
        out.push(json!({
            "id": x.id,
            "node_id": crate::ids::encode("CheckSuite", x.id),
            "head_branch": x.head_branch,
            "head_sha": x.head_sha,
            "status": status,
            "conclusion": conclusion,
            "url": url,
            "before": null,
            "after": x.head_sha,
            "pull_requests": pulls.get(&x.head_sha).cloned().unwrap_or_default(),
            "app": app,
            "repository": repo,
            "created_at": crate::json::ts(x.created_at),
            "updated_at": crate::json::ts(x.updated_at),
            "head_commit": head_commit,
            "latest_check_runs_count": mine.len(),
            "check_runs_url": format!("{url}/check-runs"),
            "rerequestable": true,
            "runs_rerequestable": true,
        }));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Listing

#[derive(Deserialize, Default)]
pub struct RunsQuery {
    pub check_name: Option<String>,
    pub status: Option<String>,
    pub filter: Option<String>,
    pub app_id: Option<i64>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// `{total_count, <key>: [...]}` with GitHub's Link header.
fn envelope(s: &AppState, uri: &OriginalUri, p: PageParams, key: &str, items: Vec<Value>, total: i64) -> Response {
    let links = Paged::new(uri, s, p, Some(total), vec![]).into_response();
    let mut res = (StatusCode::OK, Json(json!({ "total_count": total, key: items }))).into_response();
    if let Some(link) = links.headers().get(header::LINK) {
        res.headers_mut().insert(header::LINK, link.clone());
    }
    res
}

/// Check runs on a commit (`sha`) or in a suite, filtered like GitHub.
async fn list_runs(s: &AppState, ctx: &RepoCtx, uri: &OriginalUri, sha: Option<&str>, suite: Option<i64>, q: RunsQuery) -> ApiResult<Response> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    if let Some(st) = q.status.as_deref().filter(|x| !x.is_empty()) {
        if !RUN_STATUSES.contains(&st) {
            return Err(invalid("status", st));
        }
    }
    let latest = !matches!(q.filter.as_deref(), Some("all"));
    if q.app_id.is_some_and(|a| a != FORGE_APP_ID) {
        return Ok(envelope(s, uri, p, "check_runs", vec![], 0));
    }
    let cte = "WITH r AS (
        SELECT DISTINCT ON (CASE WHEN $4 THEN name ELSE id::text END) * FROM check_runs
         WHERE repo_id = $1 AND ($2::text IS NULL OR head_sha = $2) AND ($3::bigint IS NULL OR suite_id = $3)
         ORDER BY CASE WHEN $4 THEN name ELSE id::text END, id DESC)
        SELECT %s FROM r WHERE ($5::text IS NULL OR name = $5) AND ($6::text IS NULL OR status = $6)";
    let name = q.check_name.filter(|x| !x.is_empty());
    let status = q.status.filter(|x| !x.is_empty());
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(cte.replace("%s", "count(*)")))
        .bind(ctx.repo.id)
        .bind(sha)
        .bind(suite)
        .bind(latest)
        .bind(&name)
        .bind(&status)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<CheckRunRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{} ORDER BY id DESC LIMIT $7 OFFSET $8", cte.replace("%s", "*"))))
        .bind(ctx.repo.id)
        .bind(sha)
        .bind(suite)
        .bind(latest)
        .bind(&name)
        .bind(&status)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = render_runs(s, ctx, &rows).await?;
    Ok(envelope(s, uri, p, "check_runs", items, total))
}

#[derive(Deserialize, Default)]
pub struct SuitesQuery {
    pub app_id: Option<i64>,
    pub check_name: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// `GET /commits/{ref}/check-runs` (mounted by `statuses::commit_ref_routes`).
pub async fn list_for_ref(s: &AppState, viewer: Option<&User>, uri: &OriginalUri, owner: &str, repo: &str, r: &str, q: RunsQuery) -> ApiResult<Response> {
    let ctx = gitapi::load(s, viewer, owner, repo).await?;
    let sha = resolve_sha(s, &ctx, r).await?;
    list_runs(s, &ctx, uri, Some(&sha), None, q).await
}

/// `GET /commits/{ref}/check-suites` (mounted by `statuses::commit_ref_routes`).
pub async fn suites_for_ref(s: &AppState, viewer: Option<&User>, uri: &OriginalUri, owner: &str, repo: &str, r: &str, q: SuitesQuery) -> ApiResult<Response> {
    let ctx = gitapi::load(s, viewer, owner, repo).await?;
    let sha = resolve_sha(s, &ctx, r).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let name = q.check_name.filter(|x| !x.is_empty());
    let filter = "FROM check_suites cs WHERE cs.repo_id = $1 AND cs.head_sha = $2 AND ($3::bigint IS NULL OR cs.app_id = $3)
                    AND ($4::text IS NULL OR EXISTS (SELECT 1 FROM check_runs r WHERE r.suite_id = cs.id AND r.name = $4))";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {filter}")))
        .bind(ctx.repo.id)
        .bind(&sha)
        .bind(q.app_id)
        .bind(&name)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<SuiteRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT cs.* {filter} ORDER BY cs.id LIMIT $5 OFFSET $6")))
        .bind(ctx.repo.id)
        .bind(&sha)
        .bind(q.app_id)
        .bind(&name)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = render_suites(s, &ctx, &rows).await?;
    Ok(envelope(s, uri, p, "check_suites", items, total))
}

// ---------------------------------------------------------------------------------------------
// Handlers

type IdPath = Path<(String, String, i64)>;

async fn one_run(s: &AppState, ctx: &RepoCtx, row: &CheckRunRow) -> ApiResult<Value> {
    Ok(render_runs(s, ctx, std::slice::from_ref(row)).await?.pop().unwrap_or(Value::Null))
}

async fn one_suite(s: &AppState, ctx: &RepoCtx, row: &SuiteRow) -> ApiResult<Value> {
    Ok(render_suites(s, ctx, std::slice::from_ref(row)).await?.pop().unwrap_or(Value::Null))
}

async fn create_run_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>, Json(b): Json<CheckRunInput>) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let row = create_check_run(&s, &ctx, &u, b).await?;
    Ok(super::created(one_run(&s, &ctx, &row).await?))
}

async fn get_run_h(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): IdPath) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let row = run_by_id(&s, &ctx, id).await?;
    Ok(Json(one_run(&s, &ctx, &row).await?))
}

async fn update_run_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): IdPath, Json(b): Json<CheckRunInput>) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let row = update_check_run(&s, &ctx, &u, id, b).await?;
    Ok(Json(one_run(&s, &ctx, &row).await?))
}

async fn annotations_h(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo, id)): IdPath, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let run = run_by_id(&s, &ctx, id).await?;
    let rows: Vec<AnnotationRow> = sqlx::query_as("SELECT * FROM check_annotations WHERE check_run_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(run.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM check_annotations WHERE check_run_id = $1").bind(run.id).fetch_one(&s.db).await?;
    let base = s.base_url();
    let full = ctx.full_name();
    let items = rows
        .iter()
        .map(|a| {
            json!({
                "path": a.path,
                "blob_href": format!("{base}/{full}/blob/{}/{}", run.head_sha, crate::json::git::enc_path(&a.path)),
                "start_line": a.start_line,
                "end_line": a.end_line,
                "start_column": a.start_column,
                "end_column": a.end_column,
                "annotation_level": a.annotation_level,
                "title": a.title,
                "message": a.message,
                "raw_details": a.raw_details,
            })
        })
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

/// GitHub only lets the owning app re-request a run (403 otherwise). forge's app has nothing
/// to execute, so a re-request by a writer is accepted and only emits the webhook.
async fn rerequest_run_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): IdPath) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let row = run_by_id(&s, &ctx, id).await?;
    let v = one_run(&s, &ctx, &row).await?;
    emit(&s, &ctx, &u, "check_run", "rerequested", "check_run", v).await;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

#[derive(Deserialize, Default)]
struct CreateSuite {
    head_sha: Option<String>,
}

async fn create_suite_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>, Json(b): Json<CreateSuite>) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let head = b.head_sha.filter(|h| !h.is_empty()).ok_or_else(|| ApiError::validation("CheckSuite", "head_sha", "missing_field"))?;
    let sha = resolve_sha(&s, &ctx, &head).await?;
    let (row, new) = ensure_suite(&s, &ctx, &u, &sha).await?;
    let v = one_suite(&s, &ctx, &row).await?;
    if new {
        emit(&s, &ctx, &u, "check_suite", "requested", "check_suite", v.clone()).await;
        Ok(super::created(v))
    } else {
        Ok(Json(v).into_response())
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct AutoTrigger {
    pub app_id: Option<i64>,
    pub setting: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct SuitePreferencesInput {
    pub auto_trigger_checks: Option<Vec<AutoTrigger>>,
}

/// Stored `auto_trigger_checks` of a repository as (app_id, setting).
pub async fn suite_preferences(s: &AppState, repo_id: i64) -> ApiResult<Vec<(i64, bool)>> {
    Ok(sqlx::query_as("SELECT app_id, setting FROM check_suite_preferences WHERE repo_id = $1 ORDER BY app_id")
        .bind(repo_id)
        .fetch_all(&s.db)
        .await?)
}

/// Sets whether check suites are created automatically on push, per app (admin only). forge's
/// only app is [`FORGE_APP_ID`]; other app ids are rejected like GitHub rejects unknown apps.
pub async fn set_suite_preferences(s: &AppState, ctx: &RepoCtx, input: SuitePreferencesInput) -> ApiResult<Vec<(i64, bool)>> {
    ctx.require(crate::db::Perm::Admin)?;
    for a in input.auto_trigger_checks.unwrap_or_default() {
        let app = a.app_id.ok_or_else(|| ApiError::validation("CheckSuitePreference", "app_id", "missing_field"))?;
        let setting = a.setting.ok_or_else(|| ApiError::validation("CheckSuitePreference", "setting", "missing_field"))?;
        if app != FORGE_APP_ID {
            return Err(ApiError::unprocessable(format!("Invalid app_id: {app}")));
        }
        sqlx::query(
            "INSERT INTO check_suite_preferences (repo_id, app_id, setting) VALUES ($1, $2, $3)
             ON CONFLICT (repo_id, app_id) DO UPDATE SET setting = EXCLUDED.setting, updated_at = now()",
        )
        .bind(ctx.repo.id)
        .bind(app)
        .bind(setting)
        .execute(&s.db)
        .await?;
    }
    suite_preferences(s, ctx.repo.id).await
}

/// Push hook (`hooks::after_refs_changed`): when `auto_trigger_checks` is enabled for the forge
/// app, every new branch head gets its (queued) check suite, like GitHub does for installed
/// apps. Off by default: forge's app runs nothing, so empty suites would only be noise.
pub async fn on_refs_changed(s: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) {
    let enabled = match suite_preferences(s, ctx.repo.id).await {
        Ok(p) => p.iter().any(|(app, on)| *app == FORGE_APP_ID && *on),
        Err(_) => false,
    };
    if !enabled {
        return;
    }
    for (name, _, new) in changed {
        if !name.starts_with("refs/heads/") || new.is_empty() || new.bytes().all(|b| b == b'0') {
            continue;
        }
        match ensure_suite_by(s, ctx, actor.map(|u| u.id), new).await {
            Ok((suite, true)) => {
                if let (Some(a), Ok(v)) = (actor, render_suites(s, ctx, std::slice::from_ref(&suite)).await) {
                    emit(s, ctx, a, "check_suite", "requested", "check_suite", v.into_iter().next().unwrap_or(Value::Null)).await;
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(repo = ctx.repo.id, error = %e.message, "auto-creating check suite failed"),
        }
    }
}

async fn suite_preferences_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>, Json(b): Json<SuitePreferencesInput>) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, Some(&u), &owner, &repo).await?;
    let prefs = set_suite_preferences(&s, &ctx, b).await?;
    let auto: Vec<Value> = prefs.iter().map(|(app, on)| json!({ "app_id": app, "setting": on })).collect();
    Ok(Json(json!({
        "preferences": { "auto_trigger_checks": auto },
        "repository": crate::json::repo::repo(&s.api_url(), s.base_url(), &ctx),
    })))
}

async fn get_suite_h(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): IdPath) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let row = suite_by_id(&s, &ctx, id).await?;
    Ok(Json(one_suite(&s, &ctx, &row).await?))
}

async fn suite_runs_h(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo, id)): IdPath, Query(q): Query<RunsQuery>) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let suite = suite_by_id(&s, &ctx, id).await?;
    list_runs(&s, &ctx, &uri, None, Some(suite.id), q).await
}

async fn rerequest_suite_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): IdPath) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    rerequest_suite(&s, &ctx, &u, id).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}
