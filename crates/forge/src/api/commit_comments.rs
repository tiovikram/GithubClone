//! Commit comments (+ their reactions).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::plat::{self, CreateReaction, ReactionQuery};
use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/comments", get(list_for_repo))
        .route("/repos/{owner}/{repo}/comments/{comment_id}", get(get_one).patch(update).delete(delete))
        .route("/repos/{owner}/{repo}/comments/{comment_id}/reactions", get(list_reactions).post(create_reaction))
        .route("/repos/{owner}/{repo}/comments/{comment_id}/reactions/{reaction_id}", axum::routing::delete(delete_reaction))
}

/// Per-ref routes, mounted behind `ref_dispatch` (refs may contain `/`).
pub fn commit_ref_routes() -> Router<AppState> {
    Router::new().route("/repos/{owner}/{repo}/commits/{commit_sha}/comments", get(list_for_commit).post(create))
}

pub const SUBJECT: &str = "commit_comment";
pub const ALL_REACTIONS: [&str; 8] = ["+1", "-1", "laugh", "confused", "heart", "hooray", "rocket", "eyes"];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CommitCommentRow {
    pub id: i64,
    pub repo_id: i64,
    pub commit_id: String,
    pub user_id: Option<i64>,
    pub body: String,
    pub path: Option<String>,
    pub position: Option<i64>,
    pub line: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Renders commit comments (batched users, associations, reactions).
pub async fn comments_json(s: &AppState, ctx: &RepoCtx, rows: &[CommitCommentRow]) -> ApiResult<Vec<Value>> {
    let api = s.api_url();
    let base = s.base_url();
    let full = ctx.full_name();
    let uids: Vec<i64> = rows.iter().filter_map(|r| r.user_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &uids).await?;
    let assoc = crate::json::issue::associations(&s.db, ctx, &uids).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let reactions = plat::reaction_counts(&s.db, SUBJECT, &ids).await?;
    let empty = Default::default();
    Ok(rows
        .iter()
        .map(|c| {
            let url = format!("{api}/repos/{full}/comments/{}", c.id);
            json!({
                "html_url": format!("{base}/{full}/commit/{}#commitcomment-{}", c.commit_id, c.id),
                "url": url,
                "id": c.id,
                "node_id": crate::ids::encode("CommitComment", c.id),
                "body": c.body,
                "path": c.path,
                "position": c.position,
                "line": c.line,
                "commit_id": c.commit_id,
                "user": c.user_id.and_then(|u| users.get(&u)).map(|u| crate::json::user::simple(&api, base, u)).unwrap_or(Value::Null),
                "created_at": crate::json::ts(c.created_at),
                "updated_at": crate::json::ts(c.updated_at),
                "author_association": c.user_id.and_then(|u| assoc.get(&u)).cloned().unwrap_or_else(|| "NONE".into()),
                "reactions": crate::json::issue::reactions_json(&api, &url, reactions.get(&c.id).unwrap_or(&empty)),
            })
        })
        .collect())
}

#[derive(Deserialize)]
pub struct CreateCommitComment {
    pub body: Option<String>,
    pub path: Option<String>,
    pub position: Option<i64>,
    pub line: Option<i64>,
}

pub async fn create_commit_comment(s: &AppState, ctx: &RepoCtx, actor: &User, commit: &str, req: CreateCommitComment) -> ApiResult<CommitCommentRow> {
    ctx.require(Perm::Read)?;
    let body = req.body.filter(|b| !b.is_empty()).ok_or_else(|| ApiError::validation("CommitComment", "body", "missing_field"))?;
    let path = s.git.sync(ctx.repo.id).await?;
    let sha = crate::git::plumbing::rev_parse(&s.git.git, &path, commit)
        .await
        .ok_or_else(|| ApiError::validation("CommitComment", "commit_id", "invalid"))?;
    let row: CommitCommentRow = sqlx::query_as(
        "INSERT INTO commit_comments (repo_id, commit_id, user_id, body, path, position, line)
         VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(&sha)
    .bind(actor.id)
    .bind(&body)
    .bind(req.path.filter(|p| !p.is_empty()))
    .bind(req.position)
    .bind(req.line)
    .fetch_one(&s.db)
    .await?;
    if let Ok(mut v) = comments_json(s, ctx, std::slice::from_ref(&row)).await {
        let payload = json!({
            "action": "created",
            "comment": v.remove(0),
            "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor),
        });
        if let Err(e) = crate::webhooks::enqueue_event_ctx(s, ctx, "commit_comment", payload).await {
            tracing::error!(error = %format!("{e:#}"), "commit_comment webhook failed");
        }
    }
    Ok(row)
}

async fn load_comment(s: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<CommitCommentRow> {
    sqlx::query_as("SELECT * FROM commit_comments WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn list_for_repo(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let rows: Vec<CommitCommentRow> = sqlx::query_as("SELECT * FROM commit_comments WHERE repo_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(ctx.repo.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM commit_comments WHERE repo_id = $1").bind(ctx.repo.id).fetch_one(&s.db).await?;
    let items = comments_json(&s, &ctx, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn list_for_commit(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let path = s.git.sync(ctx.repo.id).await?;
    let full = crate::git::plumbing::rev_parse(&s.git.git, &path, &sha).await.ok_or_else(ApiError::not_found)?;
    let rows: Vec<CommitCommentRow> = sqlx::query_as(
        "SELECT * FROM commit_comments WHERE repo_id = $1 AND commit_id = $2 ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(ctx.repo.id)
    .bind(&full)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM commit_comments WHERE repo_id = $1 AND commit_id = $2")
        .bind(ctx.repo.id)
        .bind(&full)
        .fetch_one(&s.db)
        .await?;
    let items = comments_json(&s, &ctx, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
    Json(req): Json<CreateCommitComment>,
) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let row = create_commit_comment(&s, &ctx, &u, &sha, req).await?;
    Ok(created(comments_json(&s, &ctx, std::slice::from_ref(&row)).await?.remove(0)))
}

async fn get_one(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let row = load_comment(&s, &ctx, id).await?;
    Ok(Json(comments_json(&s, &ctx, std::slice::from_ref(&row)).await?.remove(0)))
}

#[derive(Deserialize)]
struct UpdateBody {
    body: Option<String>,
}

async fn update(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(req): Json<UpdateBody>,
) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let row = load_comment(&s, &ctx, id).await?;
    if row.user_id != Some(u.id) {
        ctx.require(Perm::Write)?;
    }
    let body = req.body.filter(|b| !b.is_empty()).ok_or_else(|| ApiError::validation("CommitComment", "body", "missing_field"))?;
    let row: CommitCommentRow = sqlx::query_as("UPDATE commit_comments SET body = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(id)
        .bind(body)
        .fetch_one(&s.db)
        .await?;
    Ok(Json(comments_json(&s, &ctx, std::slice::from_ref(&row)).await?.remove(0)))
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let row = load_comment(&s, &ctx, id).await?;
    if row.user_id != Some(u.id) {
        ctx.require(Perm::Write)?;
    }
    sqlx::query("DELETE FROM reactions WHERE subject_type = $1 AND subject_id = $2").bind(SUBJECT).bind(id).execute(&s.db).await?;
    sqlx::query("DELETE FROM commit_comments WHERE id = $1").bind(id).execute(&s.db).await?;
    Ok(no_content())
}

async fn list_reactions(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Query(q): Query<ReactionQuery>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    load_comment(&s, &ctx, id).await?;
    let items = plat::list_reactions(&s, SUBJECT, id, &q).await?;
    Ok(Paged::new(&uri, &s, PageParams { page: q.page, per_page: q.per_page }, None, items))
}

async fn create_reaction(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(req): Json<CreateReaction>,
) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    load_comment(&s, &ctx, id).await?;
    plat::create_reaction(&s, SUBJECT, id, &u, &req.content, &ALL_REACTIONS).await
}

async fn delete_reaction(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id, rid)): Path<(String, String, i64, i64)>,
) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    load_comment(&s, &ctx, id).await?;
    plat::delete_reaction(&s, &ctx, SUBJECT, id, rid, &u).await
}
