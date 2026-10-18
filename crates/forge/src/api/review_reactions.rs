//! Reactions on pull request review comments
//! (`/repos/{o}/{r}/pulls/comments/{comment_id}/reactions`), on top of the generic reaction
//! service in `reactions.rs` (subject type `review_comment`, the same rows the review comment
//! JSON's `reactions` rollup counts).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::{delete, get};
use axum::Router;

use super::reactions::{create_reaction, created_response, delete_reaction, list_response, CreateBody, ListQuery};
use super::{no_content, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::pulls::reviews;
use crate::state::AppState;

pub const SUBJECT: &str = "review_comment";

pub fn routes() -> Router<AppState> {
    // `{id}`: the same parameter name as `reviews.rs`' `/pulls/comments/{id}`.
    Router::new()
        .route("/repos/{owner}/{repo}/pulls/comments/{id}/reactions", get(list).post(create))
        .route("/repos/{owner}/{repo}/pulls/comments/{id}/reactions/{reaction_id}", delete(remove))
}

/// The review comment `id` of the repository, if visible to `viewer` (pending reviews' comments
/// are only visible to their author).
pub async fn visible_comment(s: &AppState, viewer: Option<&User>, owner: &str, repo: &str, id: i64) -> ApiResult<(RepoCtx, i64)> {
    let ctx = db::load_repo(&s.db, viewer, owner, repo).await?;
    let c = reviews::comment_by_id(&s.db, ctx.repo.id, id).await?;
    if !reviews::comment_visible(&s.db, &c, viewer).await? {
        return Err(ApiError::not_found());
    }
    Ok((ctx, c.id))
}

async fn list(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let (_, cid) = visible_comment(&s, u.as_ref(), &owner, &repo, id).await?;
    list_response(&s, &uri, SUBJECT, cid, &q).await
}

async fn create(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(b): Json<CreateBody>,
) -> ApiResult<Response> {
    let (_, cid) = visible_comment(&s, Some(&u), &owner, &repo, id).await?;
    let (row, new) = create_reaction(&s, SUBJECT, cid, &u, b.content.as_deref().unwrap_or("")).await?;
    created_response(&s, row, new, &u).await
}

async fn remove(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id, reaction_id)): Path<(String, String, i64, i64)>,
) -> ApiResult<Response> {
    let (ctx, cid) = visible_comment(&s, Some(&u), &owner, &repo, id).await?;
    delete_reaction(&s, &ctx, &u, SUBJECT, cid, reaction_id).await?;
    Ok(no_content())
}
