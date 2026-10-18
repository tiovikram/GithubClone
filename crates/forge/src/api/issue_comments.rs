//! Issue comments (also the conversation comments of pull requests).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use super::issues::{get_issue, load_ctx, page_window, parse_since};
use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::issue::{self as ji, CommentRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/issues/{number}/comments", get(list_for_issue).post(create_handler))
        .route("/repos/{owner}/{repo}/issues/comments", get(list_for_repo))
        .route(
            "/repos/{owner}/{repo}/issues/comments/{comment_id}",
            get(get_handler).patch(update_handler).delete(delete_handler),
        )
}

// ---------------------------------------------------------------------------------------------
// Service layer.

/// A comment of this repository by id (404 otherwise).
pub async fn comment_by_id(state: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<CommentRow> {
    sqlx::query_as("SELECT * FROM issue_comments WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

fn locked_error() -> ApiError {
    ApiError::forbidden("Unable to create comment because issue is locked.")
}

/// Comments on issue/PR `number`. Locked conversations only accept comments from collaborators
/// with write access.
pub async fn create_comment(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, body: &str) -> ApiResult<CommentRow> {
    crate::api::orgs_invitations::require_not_blocked(&state.db, &ctx.owner, actor).await?;
    if body.trim().is_empty() {
        return Err(ApiError::validation("IssueComment", "body", "missing_field"));
    }
    super::social_forge::ensure_not_blocked_by_owner(state, ctx.owner.id, actor).await?;
    let issue = get_issue(state, ctx, number).await?;
    if issue.locked && ctx.perm < Perm::Write {
        return Err(locked_error());
    }
    let mut tx = state.db.begin().await?;
    let row: CommentRow =
        sqlx::query_as("INSERT INTO issue_comments (issue_id, repo_id, user_id, body) VALUES ($1, $2, $3, $4) RETURNING *")
            .bind(issue.id)
            .bind(ctx.repo.id)
            .bind(actor.id)
            .bind(body)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(issue.id).execute(&mut *tx).await?;
    tx.commit().await?;
    super::issues::record_cross_references(state, ctx, actor, &issue, body).await;
    super::events_emit::comment_event(state, ctx, actor, "created", &row, serde_json::json!({})).await;
    Ok(row)
}

fn can_edit(ctx: &RepoCtx, actor: &User, c: &CommentRow) -> ApiResult<()> {
    if c.user_id == Some(actor.id) || ctx.perm >= Perm::Write {
        Ok(())
    } else {
        Err(ApiError::forbidden("Must have write access or be the author to modify this comment."))
    }
}

pub async fn update_comment(state: &AppState, ctx: &RepoCtx, actor: &User, comment_id: i64, body: &str) -> ApiResult<CommentRow> {
    if body.trim().is_empty() {
        return Err(ApiError::validation("IssueComment", "body", "missing_field"));
    }
    let c = comment_by_id(state, ctx, comment_id).await?;
    can_edit(ctx, actor, &c)?;
    let row: CommentRow = sqlx::query_as("UPDATE issue_comments SET body = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(c.id)
        .bind(body)
        .fetch_one(&state.db)
        .await?;
    if c.body != row.body {
        super::events_emit::comment_event(state, ctx, actor, "edited", &row, serde_json::json!({"changes": {"body": {"from": c.body}}})).await;
    }
    Ok(row)
}

pub async fn delete_comment(state: &AppState, ctx: &RepoCtx, actor: &User, comment_id: i64) -> ApiResult<()> {
    let c = comment_by_id(state, ctx, comment_id).await?;
    can_edit(ctx, actor, &c)?;
    // Reactions go with it (trigger, migration 0201).
    sqlx::query("DELETE FROM issue_comments WHERE id = $1").bind(c.id).execute(&state.db).await?;
    super::events_emit::comment_event(state, ctx, actor, "deleted", &c, serde_json::json!({})).await;
    Ok(())
}

pub async fn comments_json(s: &AppState, ctx: &RepoCtx, rows: Vec<CommentRow>) -> ApiResult<Vec<Value>> {
    let full = ji::load_comments(&s.db, ctx, rows).await?;
    Ok(full.iter().map(|c| ji::comment(&s.api_url(), s.base_url(), ctx, c)).collect())
}

pub async fn comment_json(s: &AppState, ctx: &RepoCtx, row: CommentRow) -> ApiResult<Value> {
    comments_json(s, ctx, vec![row]).await?.pop().ok_or_else(ApiError::not_found)
}

// ---------------------------------------------------------------------------------------------
// Handlers.

#[derive(Deserialize, Default)]
struct ListQuery {
    since: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_for_issue(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let issue = get_issue(&s, &ctx, number).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let since = parse_since(&q.since)?;
    let rows: Vec<CommentRow> = sqlx::query_as(
        "SELECT * FROM issue_comments WHERE issue_id = $1 AND ($2::timestamptz IS NULL OR updated_at >= $2)
          ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(issue.id)
    .bind(since)
    .bind(p.limit() + 1)
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let (rows, total) = page_window(rows, &p);
    Ok(Paged::new(&uri, &s, p, total, comments_json(&s, &ctx, rows).await?))
}

async fn list_for_repo(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let since = parse_since(&q.since)?;
    let col = match q.sort.as_deref() {
        Some("updated") => "updated_at",
        None | Some("created") => "created_at",
        Some(other) => return Err(super::issues::invalid("IssueComment", "sort", other)),
    };
    // GitHub ignores `direction` without `sort`.
    let dir = if q.sort.is_some() && q.direction.as_deref() == Some("desc") { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT * FROM issue_comments WHERE repo_id = $1 AND ($2::timestamptz IS NULL OR updated_at >= $2)
          ORDER BY {col} {dir}, id {dir} LIMIT $3 OFFSET $4"
    );
    let rows: Vec<CommentRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ctx.repo.id)
        .bind(since)
        .bind(p.limit() + 1)
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let (rows, total) = page_window(rows, &p);
    Ok(Paged::new(&uri, &s, p, total, comments_json(&s, &ctx, rows).await?))
}

#[derive(Deserialize)]
struct Body {
    body: Option<String>,
}

async fn create_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<Body>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let row = create_comment(&s, &ctx, &u, number, b.body.as_deref().unwrap_or("")).await?;
    Ok(created(comment_json(&s, &ctx, row).await?))
}

async fn get_handler(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let row = comment_by_id(&s, &ctx, id).await?;
    Ok(Json(comment_json(&s, &ctx, row).await?))
}

async fn update_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(b): Json<Body>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let row = update_comment(&s, &ctx, &u, id, b.body.as_deref().unwrap_or("")).await?;
    Ok(Json(comment_json(&s, &ctx, row).await?))
}

async fn delete_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    delete_comment(&s, &ctx, &u, id).await?;
    Ok(no_content())
}
