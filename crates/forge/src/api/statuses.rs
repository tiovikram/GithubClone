//! Commit statuses (create, list, combined) and empty check-runs/check-suites listings.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::gitapi::{self, GitResult};
use super::{created, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::read;
use crate::json::git::{self as gj, StatusRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/statuses/{sha}", get(list_statuses_legacy).post(create_status_h))
        .route("/repos/{owner}/{repo}/commits/{ref}/statuses", get(list_statuses))
        .route("/repos/{owner}/{repo}/commits/{ref}/status", get(combined_status))
        .route("/repos/{owner}/{repo}/commits/{ref}/check-runs", get(check_runs))
        .route("/repos/{owner}/{repo}/commits/{ref}/check-suites", get(check_suites))
}

/// Input for `create_status`.
#[derive(Deserialize, Debug, Clone)]
pub struct CreateStatus {
    pub state: Option<String>,
    pub target_url: Option<String>,
    pub description: Option<String>,
    pub context: Option<String>,
}

/// Records a commit status on `sha` (must be an existing commit).
pub async fn create_status(s: &AppState, ctx: &RepoCtx, actor: &User, sha: &str, input: CreateStatus) -> ApiResult<StatusRow> {
    let state = input.state.ok_or_else(|| ApiError::validation("Status", "state", "missing_field"))?;
    if !matches!(state.as_str(), "error" | "failure" | "pending" | "success") {
        return Err(ApiError::validation("Status", "state", "invalid"));
    }
    let path = gitapi::sync(s, ctx).await?;
    let commit = read::resolve_commit(&s.git.git, &path, sha)
        .await
        .api()?
        .ok_or_else(|| ApiError::unprocessable(format!("No commit found for SHA: {sha}")))?;
    let row: StatusRow = sqlx::query_as(
        "INSERT INTO commit_statuses (repo_id, sha, state, context, description, target_url, creator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, sha, state, context, description, target_url, creator_id, created_at, updated_at",
    )
    .bind(ctx.repo.id)
    .bind(&commit)
    .bind(&state)
    .bind(input.context.filter(|c| !c.is_empty()).unwrap_or_else(|| "default".into()))
    .bind(input.description.filter(|d| !d.is_empty()))
    .bind(input.target_url.filter(|d| !d.is_empty()))
    .bind(actor.id)
    .fetch_one(&s.db)
    .await?;
    Ok(row)
}

async fn create_status_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
    Json(b): Json<CreateStatus>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let row = create_status(&s, &ctx, &u, &sha, b).await?;
    Ok(created(gj::status(&gitapi::urls(&s, &ctx), &row, Some(&u), false)))
}

async fn creators(s: &AppState, rows: &[StatusRow]) -> ApiResult<HashMap<i64, User>> {
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.creator_id).collect();
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&ids).fetch_all(&s.db).await?;
    Ok(users.into_iter().map(|u| (u.id, u)).collect())
}

/// Resolves `ref` to a commit sha for status lookups.
async fn commit_for(s: &AppState, ctx: &RepoCtx, r: &str) -> ApiResult<String> {
    if gitapi::is_empty(s, ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let path = gitapi::sync(s, ctx).await?;
    read::resolve_commit(&s.git.git, &path, r)
        .await
        .api()?
        .ok_or_else(|| ApiError::unprocessable(format!("No commit found for SHA: {r}")))
}

#[derive(Deserialize, Default)]
struct PageQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_for(s: &AppState, ctx: &RepoCtx, uri: &OriginalUri, sha: &str, pages: PageParams) -> ApiResult<Response> {
    let rows: Vec<StatusRow> = sqlx::query_as(
        "SELECT id, sha, state, context, description, target_url, creator_id, created_at, updated_at
           FROM commit_statuses WHERE repo_id = $1 AND sha = $2 ORDER BY id DESC LIMIT $3 OFFSET $4",
    )
    .bind(ctx.repo.id)
    .bind(sha)
    .bind(pages.limit())
    .bind(pages.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM commit_statuses WHERE repo_id = $1 AND sha = $2")
        .bind(ctx.repo.id)
        .bind(sha)
        .fetch_one(&s.db)
        .await?;
    let users = creators(s, &rows).await?;
    let u = gitapi::urls(s, ctx);
    let items = rows.iter().map(|r| gj::status(&u, r, r.creator_id.and_then(|i| users.get(&i)), false)).collect();
    Ok(Paged::new(uri, s, pages, Some(total), items).into_response())
}

async fn list_statuses(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, repo, r)): Path<(String, String, String)>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let sha = commit_for(&s, &ctx, &r).await?;
    list_for(&s, &ctx, &uri, &sha, PageParams { page: q.page, per_page: q.per_page }).await
}

async fn list_statuses_legacy(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, repo, r)): Path<(String, String, String)>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let sha = commit_for(&s, &ctx, &r).await?;
    list_for(&s, &ctx, &uri, &sha, PageParams { page: q.page, per_page: q.per_page }).await
}

/// GitHub's combined state: failure if any latest status is error/failure, pending if none or
/// any pending, else success.
pub fn combined_state(latest: &[StatusRow]) -> &'static str {
    if latest.iter().any(|r| r.state == "error" || r.state == "failure") {
        "failure"
    } else if latest.is_empty() || latest.iter().any(|r| r.state == "pending") {
        "pending"
    } else {
        "success"
    }
}

async fn combined_status(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let sha = commit_for(&s, &ctx, &r).await?;
    let latest: Vec<StatusRow> = sqlx::query_as(
        "SELECT DISTINCT ON (context) id, sha, state, context, description, target_url, creator_id, created_at, updated_at
           FROM commit_statuses WHERE repo_id = $1 AND sha = $2 ORDER BY context, id DESC",
    )
    .bind(ctx.repo.id)
    .bind(&sha)
    .fetch_all(&s.db)
    .await?;
    let users = creators(&s, &latest).await?;
    let urls = gitapi::urls(&s, &ctx);
    let statuses: Vec<Value> = latest.iter().map(|x| gj::status(&urls, x, x.creator_id.and_then(|i| users.get(&i)), true)).collect();
    Ok(Json(json!({
        "state": combined_state(&latest),
        "statuses": statuses,
        "sha": sha,
        "total_count": latest.len(),
        "repository": crate::json::repo::repo(&s.api_url(), s.base_url(), &ctx),
        "commit_url": format!("{}/commits/{sha}", urls.repo_api),
        "url": format!("{}/commits/{sha}/status", urls.repo_api),
    })))
}

async fn check_runs(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    commit_for(&s, &ctx, &r).await?;
    Ok(Json(json!({ "total_count": 0, "check_runs": [] })))
}

async fn check_suites(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    commit_for(&s, &ctx, &r).await?;
    Ok(Json(json!({ "total_count": 0, "check_suites": [] })))
}
