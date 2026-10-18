//! Reactions on issues and issue comments. The service functions are generic over the subject
//! (`issue`, `issue_comment`, `review_comment`, `commit_comment`, `release`) so other areas can
//! expose their own reaction endpoints with them.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use super::issues::{get_issue, invalid, load_ctx, page_window};
use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::issue::{self as ji, ReactionRow, REACTIONS};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/issues/{number}/reactions", get(list_issue).post(create_issue))
        .route("/repos/{owner}/{repo}/issues/{number}/reactions/{reaction_id}", delete(delete_issue))
        .route("/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions", get(list_comment).post(create_comment))
        .route("/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions/{reaction_id}", delete(delete_comment))
}

// ---------------------------------------------------------------------------------------------
// Service layer.

/// Adds `actor`'s reaction. Returns the reaction and whether it was newly created (GitHub
/// answers 201 for new, 200 for an existing identical reaction).
pub async fn create_reaction(state: &AppState, subject_type: &str, subject_id: i64, actor: &User, content: &str) -> ApiResult<(ReactionRow, bool)> {
    if !REACTIONS.contains(&content) {
        return Err(invalid("Reaction", "content", content));
    }
    let inserted: Option<ReactionRow> = sqlx::query_as(
        "INSERT INTO reactions (subject_type, subject_id, user_id, content) VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(subject_type)
    .bind(subject_id)
    .bind(actor.id)
    .bind(content)
    .fetch_optional(&state.db)
    .await?;
    if let Some(r) = inserted {
        return Ok((r, true));
    }
    let existing: ReactionRow = sqlx::query_as(
        "SELECT * FROM reactions WHERE subject_type = $1 AND subject_id = $2 AND user_id = $3 AND content = $4",
    )
    .bind(subject_type)
    .bind(subject_id)
    .bind(actor.id)
    .bind(content)
    .fetch_one(&state.db)
    .await?;
    Ok((existing, false))
}

/// Deletes a reaction of the subject. Only the reaction's author may (GitHub: a reaction is
/// the user's own; repository admins cannot remove other users' reactions).
pub async fn delete_reaction(state: &AppState, _ctx: &RepoCtx, actor: &User, subject_type: &str, subject_id: i64, reaction_id: i64) -> ApiResult<()> {
    let r: ReactionRow = sqlx::query_as("SELECT * FROM reactions WHERE id = $1 AND subject_type = $2 AND subject_id = $3")
        .bind(reaction_id)
        .bind(subject_type)
        .bind(subject_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if r.user_id != actor.id {
        return Err(ApiError::forbidden("You can only delete your own reactions."));
    }
    sqlx::query("DELETE FROM reactions WHERE id = $1").bind(r.id).execute(&state.db).await?;
    Ok(())
}

/// One page (`limit+1` rows for windowing) of reactions, optionally filtered by content.
pub async fn list_reactions(state: &AppState, subject_type: &str, subject_id: i64, content: Option<&str>, p: &PageParams) -> ApiResult<Vec<ReactionRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM reactions WHERE subject_type = $1 AND subject_id = $2 AND ($3::text IS NULL OR content = $3)
          ORDER BY id LIMIT $4 OFFSET $5",
    )
    .bind(subject_type)
    .bind(subject_id)
    .bind(content)
    .bind(p.limit() + 1)
    .bind(p.offset())
    .fetch_all(&state.db)
    .await?)
}

pub async fn reactions_json(s: &AppState, rows: &[ReactionRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.user_id).collect();
    let users = ji::users_by_ids(&s.db, &ids).await?;
    Ok(rows.iter().map(|r| ji::reaction(&s.api_url(), s.base_url(), r, users.get(&r.user_id))).collect())
}

/// Renders a create result with 201 (new) or 200 (existing).
pub async fn created_response(s: &AppState, row: ReactionRow, new: bool, actor: &User) -> ApiResult<Response> {
    let v = ji::reaction(&s.api_url(), s.base_url(), &row, Some(actor));
    let status = if new { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(v)).into_response())
}

/// Paged list response for a subject.
pub async fn list_response(s: &AppState, uri: &OriginalUri, subject_type: &str, subject_id: i64, q: &ListQuery) -> ApiResult<Paged> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    if let Some(c) = q.content.as_deref() {
        if !REACTIONS.contains(&c) {
            return Err(invalid("Reaction", "content", c));
        }
    }
    let rows = list_reactions(s, subject_type, subject_id, q.content.as_deref(), &p).await?;
    let (rows, total) = page_window(rows, &p);
    Ok(Paged::new(uri, s, p, total, reactions_json(s, &rows).await?))
}

// ---------------------------------------------------------------------------------------------
// Handlers.

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub content: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub content: Option<String>,
}

async fn issue_id(s: &AppState, u: Option<&User>, owner: &str, repo: &str, number: i64) -> ApiResult<(RepoCtx, i64, bool)> {
    let ctx = load_ctx(s, u, owner, repo).await?;
    let issue = get_issue(s, &ctx, number).await?;
    Ok((ctx, issue.id, issue.locked))
}

async fn comment_id(s: &AppState, u: Option<&User>, owner: &str, repo: &str, id: i64) -> ApiResult<(RepoCtx, i64)> {
    let ctx = load_ctx(s, u, owner, repo).await?;
    let c = super::issue_comments::comment_by_id(s, &ctx, id).await?;
    Ok((ctx, c.id))
}

/// Locked conversations only accept reactions from users with Write (GitHub). `subject_type`
/// is a reactions subject (`issue`, `issue_comment`, `review_comment`).
pub async fn check_unlocked(s: &AppState, ctx: &RepoCtx, subject_type: &str, subject_id: i64) -> ApiResult<()> {
    if ctx.perm >= Perm::Write {
        return Ok(());
    }
    let sql = match subject_type {
        "issue" => "SELECT locked FROM issues WHERE id = $1",
        "issue_comment" => "SELECT i.locked FROM issue_comments c JOIN issues i ON i.id = c.issue_id WHERE c.id = $1",
        "review_comment" => "SELECT i.locked FROM review_comments c JOIN issues i ON i.id = c.issue_id WHERE c.id = $1",
        _ => return Ok(()),
    };
    let locked: Option<bool> = sqlx::query_scalar(sql).bind(subject_id).fetch_optional(&s.db).await?;
    if locked == Some(true) {
        return Err(ApiError::forbidden("Unable to react because the issue is locked."));
    }
    Ok(())
}

async fn list_issue(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let (_, id, _) = issue_id(&s, u.as_ref(), &owner, &repo, number).await?;
    list_response(&s, &uri, "issue", id, &q).await
}

async fn create_issue(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<CreateBody>,
) -> ApiResult<Response> {
    let (ctx, id, locked) = issue_id(&s, Some(&u), &owner, &repo, number).await?;
    if locked && ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Unable to react because the issue is locked."));
    }
    let (row, new) = create_reaction(&s, "issue", id, &u, b.content.as_deref().unwrap_or("")).await?;
    created_response(&s, row, new, &u).await
}

async fn delete_issue(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number, reaction_id)): Path<(String, String, i64, i64)>,
) -> ApiResult<Response> {
    let (ctx, id, _) = issue_id(&s, Some(&u), &owner, &repo, number).await?;
    delete_reaction(&s, &ctx, &u, "issue", id, reaction_id).await?;
    Ok(no_content())
}

async fn list_comment(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, cid)): Path<(String, String, i64)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let (_, id) = comment_id(&s, u.as_ref(), &owner, &repo, cid).await?;
    list_response(&s, &uri, "issue_comment", id, &q).await
}

async fn create_comment(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, cid)): Path<(String, String, i64)>,
    Json(b): Json<CreateBody>,
) -> ApiResult<Response> {
    let (ctx, id) = comment_id(&s, Some(&u), &owner, &repo, cid).await?;
    check_unlocked(&s, &ctx, "issue_comment", id).await?;
    let (row, new) = create_reaction(&s, "issue_comment", id, &u, b.content.as_deref().unwrap_or("")).await?;
    created_response(&s, row, new, &u).await
}

async fn delete_comment(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, cid, reaction_id)): Path<(String, String, i64, i64)>,
) -> ApiResult<Response> {
    let (ctx, id) = comment_id(&s, Some(&u), &owner, &repo, cid).await?;
    delete_reaction(&s, &ctx, &u, "issue_comment", id, reaction_id).await?;
    Ok(no_content())
}
