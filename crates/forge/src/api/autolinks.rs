//! Autolink references: /repos/{owner}/{repo}/autolinks (repo admins only, like GitHub).
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/autolinks", get(list).post(create))
        .route("/repos/{owner}/{repo}/autolinks/{id}", get(get_one).delete(delete))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AutolinkRow {
    pub id: i64,
    pub repo_id: i64,
    pub key_prefix: String,
    pub url_template: String,
    pub is_alphanumeric: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn autolink_json(a: &AutolinkRow) -> Value {
    json!({
        "id": a.id,
        "key_prefix": a.key_prefix,
        "url_template": a.url_template,
        "is_alphanumeric": a.is_alphanumeric,
        "updated_at": crate::json::ts(a.updated_at),
    })
}

pub struct NewAutolink {
    pub key_prefix: Option<String>,
    pub url_template: Option<String>,
    pub is_alphanumeric: Option<bool>,
}

fn custom(field: &str, code: &str, message: &str) -> ApiError {
    let mut e = ApiError::validation("Autolink", field, code);
    e.errors = Some(json!([{ "resource": "Autolink", "code": code, "field": field, "message": message }]));
    e
}

/// Admin check with GitHub's message (gh prints it on 404/403).
fn require_admin(ctx: &RepoCtx) -> ApiResult<()> {
    if ctx.perm >= Perm::Admin {
        return Ok(());
    }
    let mut e = ApiError::not_found();
    if ctx.perm >= Perm::Read {
        e.message = "Must have admin rights to Repository.".into();
    }
    Err(e)
}

pub async fn list_autolinks(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<AutolinkRow>> {
    require_admin(ctx)?;
    Ok(sqlx::query_as("SELECT * FROM autolinks WHERE repo_id = $1 ORDER BY id").bind(ctx.repo.id).fetch_all(&s.db).await?)
}

pub async fn get_autolink(s: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<AutolinkRow> {
    require_admin(ctx)?;
    sqlx::query_as("SELECT * FROM autolinks WHERE repo_id = $1 AND id = $2")
        .bind(ctx.repo.id)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// POST /repos/{o}/{r}/autolinks. `url_template` must contain `<num>`; prefixes are unique per
/// repository (case-insensitive).
pub async fn create_autolink(s: &AppState, ctx: &RepoCtx, _actor: &User, input: NewAutolink) -> ApiResult<AutolinkRow> {
    require_admin(ctx)?;
    let prefix = input.key_prefix.filter(|p| !p.trim().is_empty()).ok_or_else(|| ApiError::validation("Autolink", "key_prefix", "missing_field"))?;
    let template = input.url_template.filter(|t| !t.trim().is_empty()).ok_or_else(|| ApiError::validation("Autolink", "url_template", "missing_field"))?;
    if !template.contains("<num>") {
        return Err(custom("url_template", "invalid", "Url template is missing a <num> token"));
    }
    let row: Option<AutolinkRow> = sqlx::query_as(
        "INSERT INTO autolinks (repo_id, key_prefix, url_template, is_alphanumeric) VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(&prefix)
    .bind(&template)
    .bind(input.is_alphanumeric.unwrap_or(true))
    .fetch_optional(&s.db)
    .await?;
    row.ok_or_else(|| custom("key_prefix", "already_exists", "Key prefix already exists"))
}

pub async fn delete_autolink(s: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<()> {
    require_admin(ctx)?;
    let n = sqlx::query("DELETE FROM autolinks WHERE repo_id = $1 AND id = $2").bind(ctx.repo.id).bind(id).execute(&s.db).await?;
    if n.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn load(s: &AppState, u: Option<&User>, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    db::load_repo(&s.db, u, owner, repo).await
}

async fn list(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = load(&s, u.as_ref(), &owner, &repo).await?;
    let rows = list_autolinks(&s, &ctx).await?;
    Ok(Json(Value::Array(rows.iter().map(autolink_json).collect())))
}

#[derive(Deserialize)]
struct CreateBody {
    key_prefix: Option<String>,
    url_template: Option<String>,
    is_alphanumeric: Option<bool>,
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>, Json(b): Json<CreateBody>) -> ApiResult<Response> {
    let ctx = load(&s, Some(&u), &owner, &repo).await?;
    let row = create_autolink(&s, &ctx, &u, NewAutolink { key_prefix: b.key_prefix, url_template: b.url_template, is_alphanumeric: b.is_alphanumeric }).await?;
    let mut res = created(autolink_json(&row));
    let loc = format!("{}/repos/{}/autolinks/{}", s.api_url(), ctx.full_name(), row.id);
    if let Ok(v) = axum::http::HeaderValue::from_str(&loc) {
        res.headers_mut().insert(axum::http::header::LOCATION, v);
    }
    Ok(res)
}

async fn get_one(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = load(&s, u.as_ref(), &owner, &repo).await?;
    Ok(Json(autolink_json(&get_autolink(&s, &ctx, id).await?)))
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = load(&s, Some(&u), &owner, &repo).await?;
    delete_autolink(&s, &ctx, id).await?;
    Ok(no_content())
}
