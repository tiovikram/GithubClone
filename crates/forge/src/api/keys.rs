//! SSH keys: /user/keys, /users/{u}/keys, deploy keys (/repos/{o}/{r}/keys), and the plain-text
//! `/{user}.keys` (served outside /api/v3 via `dot_keys_middleware`).
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm};
use crate::error::ApiResult;
use crate::extract::Json;
use crate::json;
use crate::ssh::keys::{self, NewDeployKey, NewKey};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/keys", get(list_mine).post(create_mine))
        .route("/user/keys/{key_id}", get(get_mine).delete(delete_mine))
        .route("/users/{username}/keys", get(list_for_user))
        .route("/repos/{owner}/{repo}/keys", get(list_deploy).post(create_deploy))
        .route("/repos/{owner}/{repo}/keys/{key_id}", get(get_deploy).delete(delete_deploy))
}

#[derive(Deserialize)]
struct CreateKey {
    title: Option<String>,
    key: Option<String>,
}

async fn list_mine(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (rows, total) = keys::list_user_keys(&s.db, u.id, p.limit(), p.offset()).await?;
    let items = rows.iter().map(|k| json::key::key(&s.api_url(), k)).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create_mine(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<CreateKey>) -> ApiResult<Response> {
    let row = keys::add_user_key(&s.db, &u, NewKey { title: b.title, key: b.key }).await?;
    Ok(created(json::key::key(&s.api_url(), &row)))
}

async fn get_mine(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let row = keys::get_user_key(&s.db, u.id, id).await?;
    Ok(Json(json::key::key(&s.api_url(), &row)))
}

async fn delete_mine(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    keys::delete_user_key(&s.db, u.id, id).await?;
    Ok(no_content())
}

async fn list_for_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    Path(login): Path<String>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    let (rows, total) = keys::list_user_keys(&s.db, u.id, p.limit(), p.offset()).await?;
    let items = rows.iter().map(json::key::key_simple).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

// ----- deploy keys (repo admin only) -----

async fn admin_ctx(s: &AppState, viewer: &MaybeUser, owner: &str, repo: &str) -> ApiResult<db::RepoCtx> {
    let ctx = db::load_repo(&s.db, viewer.as_ref(), owner, repo).await?;
    ctx.require(Perm::Admin)?;
    Ok(ctx)
}

async fn added_by_logins(s: &AppState, rows: &[keys::DeployKeyRow]) -> ApiResult<HashMap<i64, String>> {
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.added_by_id).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let pairs: Vec<(i64, String)> = sqlx::query_as("SELECT id, login FROM users WHERE id = ANY($1)")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    Ok(pairs.into_iter().collect())
}

fn render_deploy(s: &AppState, ctx: &db::RepoCtx, rows: &[keys::DeployKeyRow], logins: &HashMap<i64, String>) -> Vec<Value> {
    rows.iter()
        .map(|k| {
            let by = k.added_by_id.and_then(|id| logins.get(&id)).map(String::as_str);
            json::key::deploy_key(&s.api_url(), &ctx.full_name(), k, by)
        })
        .collect()
}

async fn list_deploy(
    State(s): State<AppState>,
    uri: OriginalUri,
    viewer: MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = admin_ctx(&s, &viewer, &owner, &repo).await?;
    let (rows, total) = keys::list_deploy_keys(&s.db, ctx.repo.id, p.limit(), p.offset()).await?;
    let logins = added_by_logins(&s, &rows).await?;
    let items = render_deploy(&s, &ctx, &rows, &logins);
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
struct CreateDeployKey {
    title: Option<String>,
    key: Option<String>,
    read_only: Option<bool>,
}

async fn create_deploy(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<CreateDeployKey>,
) -> ApiResult<Response> {
    let viewer = MaybeUser(Some(u.clone()));
    let ctx = admin_ctx(&s, &viewer, &owner, &repo).await?;
    let row = keys::add_deploy_key(&s.db, ctx.repo.id, &u, NewDeployKey { title: b.title, key: b.key, read_only: b.read_only }).await?;
    Ok(created(json::key::deploy_key(&s.api_url(), &ctx.full_name(), &row, Some(&u.login))))
}

async fn get_deploy(
    State(s): State<AppState>,
    viewer: MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = admin_ctx(&s, &viewer, &owner, &repo).await?;
    let row = keys::get_deploy_key(&s.db, ctx.repo.id, id).await?;
    let logins = added_by_logins(&s, std::slice::from_ref(&row)).await?;
    Ok(Json(render_deploy(&s, &ctx, std::slice::from_ref(&row), &logins).remove(0)))
}

async fn delete_deploy(
    State(s): State<AppState>,
    viewer: MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &viewer, &owner, &repo).await?;
    keys::delete_deploy_key(&s.db, ctx.repo.id, id).await?;
    Ok(no_content())
}

// ----- /{user}.keys -----

/// Serves `GET /{login}.keys` (one OpenSSH public key per line, like github.com) ahead of the
/// router, so it cannot collide with other single-segment routes or the SPA fallback.
pub async fn dot_keys_middleware(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let login = path
        .strip_prefix('/')
        .and_then(|p| p.strip_suffix(".keys"))
        .filter(|l| !l.is_empty() && !l.contains('/'));
    match (login, req.method() == axum::http::Method::GET || req.method() == axum::http::Method::HEAD) {
        (Some(login), true) => {
            let login = login.to_string();
            dot_keys(&s, &login).await.unwrap_or_else(|e| e.into_response())
        }
        _ => next.run(req).await,
    }
}

async fn dot_keys(s: &AppState, login: &str) -> ApiResult<Response> {
    let u = db::user_by_login(&s.db, login).await?;
    let keys: Vec<String> = sqlx::query_scalar("SELECT key FROM ssh_keys WHERE user_id = $1 ORDER BY id")
        .bind(u.id)
        .fetch_all(&s.db)
        .await?;
    let body: String = keys.iter().map(|k| format!("{k}\n")).collect();
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response())
}
