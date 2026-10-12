//! SSH signing keys: /user/ssh_signing_keys[/{id}] and /users/{username}/ssh_signing_keys.
//! Keys are parsed/fingerprinted by `crate::ssh::keys::parse_public_key` (same rules as
//! authentication keys); signing keys are a separate namespace, as on GitHub.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::ssh::keys::parse_public_key;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/ssh_signing_keys", get(list_mine).post(create_mine))
        .route("/user/ssh_signing_keys/{id}", get(get_mine).delete(delete_mine))
        .route("/users/{username}/ssh_signing_keys", get(list_for_user))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SigningKeyRow {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub key: String,
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
}

pub fn signing_key_json(k: &SigningKeyRow) -> Value {
    json!({ "key": k.key, "id": k.id, "title": k.title, "created_at": crate::json::ts(k.created_at) })
}

/// Adds a signing key for `user`. 422 `key is already in use` when the key is already a
/// signing key (of anyone).
pub async fn add_signing_key(s: &AppState, user: &User, title: Option<String>, key: Option<String>) -> ApiResult<SigningKeyRow> {
    let text = key.ok_or_else(|| ApiError::validation("PublicKey", "key", "missing_field"))?;
    let parsed = parse_public_key(&text)?;
    let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| parsed.comment.clone());
    let row: Option<SigningKeyRow> = sqlx::query_as(
        "INSERT INTO ssh_signing_keys (user_id, title, key, fingerprint) VALUES ($1, $2, $3, $4)
         ON CONFLICT (fingerprint) DO NOTHING RETURNING *",
    )
    .bind(user.id)
    .bind(&title)
    .bind(&parsed.key)
    .bind(&parsed.fingerprint)
    .fetch_optional(&s.db)
    .await?;
    row.ok_or_else(|| {
        let mut e = ApiError::validation("PublicKey", "key", "custom");
        e.errors = Some(json!([{ "resource": "PublicKey", "code": "custom", "field": "key", "message": "key is already in use" }]));
        e
    })
}

pub async fn list_signing_keys(s: &AppState, user_id: i64, p: &PageParams) -> ApiResult<(Vec<SigningKeyRow>, i64)> {
    let rows = sqlx::query_as("SELECT * FROM ssh_signing_keys WHERE user_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(user_id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total = sqlx::query_scalar("SELECT count(*) FROM ssh_signing_keys WHERE user_id = $1").bind(user_id).fetch_one(&s.db).await?;
    Ok((rows, total))
}

pub async fn get_signing_key(s: &AppState, user_id: i64, id: i64) -> ApiResult<SigningKeyRow> {
    sqlx::query_as("SELECT * FROM ssh_signing_keys WHERE user_id = $1 AND id = $2")
        .bind(user_id)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn delete_signing_key(s: &AppState, user_id: i64, id: i64) -> ApiResult<()> {
    let n = sqlx::query("DELETE FROM ssh_signing_keys WHERE user_id = $1 AND id = $2").bind(user_id).bind(id).execute(&s.db).await?;
    if n.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

#[derive(Deserialize)]
struct CreateBody {
    title: Option<String>,
    key: Option<String>,
}

async fn list_mine(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (rows, total) = list_signing_keys(&s, u.id, &p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), rows.iter().map(signing_key_json).collect()))
}

async fn create_mine(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<CreateBody>) -> ApiResult<Response> {
    let row = add_signing_key(&s, &u, b.title, b.key).await?;
    Ok(created(signing_key_json(&row)))
}

async fn get_mine(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    Ok(Json(signing_key_json(&get_signing_key(&s, u.id, id).await?)))
}

async fn delete_mine(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    delete_signing_key(&s, u.id, id).await?;
    Ok(no_content())
}

async fn list_for_user(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    let (rows, total) = list_signing_keys(&s, u.id, &p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), rows.iter().map(signing_key_json).collect()))
}
