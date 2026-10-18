//! Gist comments: `/gists/{gist_id}/comments[/{comment_id}]`.
//! Anyone who can see a gist (secret gists are reachable by id) can read and comment; authors
//! edit their comments; authors and the gist owner delete them.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::gists::{load_gist, GistRow};
use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::User;
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/gists/{gist_id}/comments", get(list).post(create))
        .route("/gists/{gist_id}/comments/{comment_id}", get(get_one).patch(update).delete(delete))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GistCommentRow {
    pub id: i64,
    pub gist_id: i64,
    pub user_id: Option<i64>,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

async fn comments_json(s: &AppState, g: &GistRow, rows: &[GistCommentRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().filter_map(|c| c.user_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let api = s.api_url();
    Ok(rows
        .iter()
        .map(|c| {
            let user = c.user_id.and_then(|i| users.get(&i));
            json!({
                "id": c.id,
                "node_id": crate::ids::encode("GistComment", c.id),
                "url": format!("{api}/gists/{}/comments/{}", g.gist_id, c.id),
                "body": c.body,
                "user": crate::json::issue::user_or_ghost(&api, s.base_url(), user),
                "created_at": crate::json::ts(c.created_at),
                "updated_at": crate::json::ts(c.updated_at),
                "author_association": if c.user_id == Some(g.owner_id) { "OWNER" } else { "NONE" },
            })
        })
        .collect())
}

fn check_body(body: Option<String>) -> ApiResult<String> {
    let b = body.filter(|b| !b.trim().is_empty()).ok_or_else(|| ApiError::validation("GistComment", "body", "missing_field"))?;
    if b.chars().count() > 65535 {
        return Err(ApiError::validation("GistComment", "body", "too_long"));
    }
    Ok(b)
}

pub async fn create_comment(s: &AppState, g: &GistRow, actor: &User, body: Option<String>) -> ApiResult<GistCommentRow> {
    let body = check_body(body)?;
    Ok(sqlx::query_as("INSERT INTO gist_comments (gist_id, user_id, body) VALUES ($1,$2,$3) RETURNING *")
        .bind(g.id)
        .bind(actor.id)
        .bind(body)
        .fetch_one(&s.db)
        .await?)
}

async fn load_comment(s: &AppState, g: &GistRow, id: i64) -> ApiResult<GistCommentRow> {
    sqlx::query_as("SELECT * FROM gist_comments WHERE id = $1 AND gist_id = $2").bind(id).bind(g.id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

async fn list(State(s): State<AppState>, uri: OriginalUri, _u: MaybeUser, Path(id): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let g = load_gist(&s, &id).await?;
    let rows: Vec<GistCommentRow> = sqlx::query_as("SELECT * FROM gist_comments WHERE gist_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(g.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM gist_comments WHERE gist_id = $1").bind(g.id).fetch_one(&s.db).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), comments_json(&s, &g, &rows).await?))
}

#[derive(Deserialize)]
struct Body {
    body: Option<String>,
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>, Json(b): Json<Body>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    let c = create_comment(&s, &g, &u, b.body).await?;
    Ok(created(comments_json(&s, &g, &[c]).await?.remove(0)))
}

async fn get_one(State(s): State<AppState>, _u: MaybeUser, Path((id, cid)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let g = load_gist(&s, &id).await?;
    let c = load_comment(&s, &g, cid).await?;
    Ok(Json(comments_json(&s, &g, &[c]).await?.remove(0)))
}

async fn update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, cid)): Path<(String, i64)>, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    let g = load_gist(&s, &id).await?;
    let c = load_comment(&s, &g, cid).await?;
    if c.user_id != Some(u.id) && !u.site_admin {
        return Err(ApiError::not_found());
    }
    let body = check_body(b.body)?;
    let c: GistCommentRow = sqlx::query_as("UPDATE gist_comments SET body = $2, updated_at = now() WHERE id = $1 RETURNING *").bind(c.id).bind(body).fetch_one(&s.db).await?;
    Ok(Json(comments_json(&s, &g, &[c]).await?.remove(0)))
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, cid)): Path<(String, i64)>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    let c = load_comment(&s, &g, cid).await?;
    if c.user_id != Some(u.id) && g.owner_id != u.id && !u.site_admin {
        return Err(ApiError::not_found());
    }
    sqlx::query("DELETE FROM gist_comments WHERE id = $1").bind(c.id).execute(&s.db).await?;
    Ok(no_content())
}
