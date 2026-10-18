//! OAuth token management:
//!
//! * `/applications/{client_id}/token*`, `/applications/{client_id}/grant`: an application
//!   (HTTP Basic `client_id:client_secret`) checks, resets, deletes or scopes tokens issued to it;
//! * the legacy OAuth Authorizations API (`/authorizations*`, `/applications/grants*`), HTTP
//!   Basic only: `login:password` or a classic personal token as the password. App-issued tokens
//!   (`ghs_`, `ghu_`) are refused, so they can't mint broader credentials.
use axum::extract::{FromRequestParts, OriginalUri, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::{no_content, PageParams, Paged};
use crate::apps::oauth::{self, AuthzRow, GrantRow, NewAuthz};
use crate::apps::{self, AppRow};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/applications/{client_id}/token", post(check_token).patch(reset_token).delete(delete_token))
        .route("/applications/{client_id}/token/scoped", post(scope_token))
        .route("/applications/{client_id}/grant", delete(delete_app_grant))
        .route("/applications/grants", get(list_grants))
        .route("/applications/grants/{grant_id}", get(get_grant).delete(delete_grant))
        .route("/authorizations", get(list_authorizations).post(create_authorization))
        .route("/authorizations/clients/{client_id}", put(get_or_create_for_app))
        .route("/authorizations/clients/{client_id}/{fingerprint}", put(get_or_create_for_fingerprint))
        .route("/authorizations/{authorization_id}", get(get_authorization).patch(update_authorization).delete(delete_authorization))
}

fn basic(parts: &Parts) -> Option<(String, String)> {
    let h = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, cred) = h.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD.decode(cred.trim()).ok()?;
    let s = String::from_utf8(decoded).ok()?;
    let (a, b) = s.split_once(':')?;
    Some((a.to_string(), b.to_string()))
}

// ---------------------------------------------------------------------------------------------
// Client credentials

/// The application authenticated by HTTP Basic `client_id:client_secret`.
struct ClientAuth(AppRow);

impl FromRequestParts<AppState> for ClientAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let path_client = parts.uri.path().trim_start_matches("/api/v3").split('/').nth(2).unwrap_or("").to_string();
        let (cid, secret) = basic(parts).ok_or_else(ApiError::not_found)?;
        if cid != path_client {
            return Err(ApiError::not_found());
        }
        let app = apps::app_by_client_id(&state.db, &cid).await?;
        if !apps::secret_matches(&app, &secret) {
            return Err(ApiError::not_found());
        }
        Ok(ClientAuth(app))
    }
}

#[derive(Deserialize, Default)]
struct TokenBody {
    access_token: Option<String>,
}

async fn app_token(s: &AppState, app: &AppRow, token: Option<&str>) -> ApiResult<AuthzRow> {
    let t = token.filter(|t| !t.is_empty()).ok_or_else(|| ApiError::validation("Token", "access_token", "missing_field"))?;
    oauth::authz_by_token(&s.db, t).await?.filter(|a| a.app_id == Some(app.id)).ok_or_else(ApiError::not_found)
}

async fn check_token(State(s): State<AppState>, ClientAuth(app): ClientAuth, Json(b): Json<TokenBody>) -> ApiResult<Json<Value>> {
    let a = app_token(&s, &app, b.access_token.as_deref()).await?;
    Ok(Json(oauth::authorization_json(&s, &a, b.access_token.as_deref()).await?))
}

async fn reset_token(State(s): State<AppState>, ClientAuth(app): ClientAuth, Json(b): Json<TokenBody>) -> ApiResult<Json<Value>> {
    let a = app_token(&s, &app, b.access_token.as_deref()).await?;
    let (a, token) = oauth::reset(&s, &a).await?;
    Ok(Json(oauth::authorization_json(&s, &a, Some(&token)).await?))
}

async fn delete_token(State(s): State<AppState>, ClientAuth(app): ClientAuth, Json(b): Json<TokenBody>) -> ApiResult<Response> {
    let a = app_token(&s, &app, b.access_token.as_deref()).await?;
    oauth::revoke(&s, &a).await?;
    Ok(no_content())
}

async fn delete_app_grant(State(s): State<AppState>, ClientAuth(app): ClientAuth, Json(b): Json<TokenBody>) -> ApiResult<Response> {
    let a = app_token(&s, &app, b.access_token.as_deref()).await?;
    oauth::revoke_grant(&s, a.user_id, a.grant_id).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
struct ScopeBody {
    access_token: Option<String>,
    target: Option<String>,
    target_id: Option<i64>,
    repositories: Option<Vec<String>>,
    repository_ids: Option<Vec<i64>>,
    permissions: Option<Map<String, Value>>,
}

async fn scope_token(State(s): State<AppState>, ClientAuth(app): ClientAuth, Json(b): Json<ScopeBody>) -> ApiResult<Json<Value>> {
    let a = app_token(&s, &app, b.access_token.as_deref()).await?;
    let target = match (&b.target, b.target_id) {
        (Some(login), _) => Some(db::user_by_login(&s.db, login).await.map_err(|_| ApiError::validation("Authorization", "target", "invalid"))?),
        (None, Some(id)) => Some(db::user_by_id(&s.db, id).await.map_err(|_| ApiError::validation("Authorization", "target_id", "invalid"))?),
        (None, None) => None,
    };
    let mut ids = b.repository_ids.clone().unwrap_or_default();
    if let Some(names) = b.repositories.as_ref().filter(|n| !n.is_empty()) {
        let t = target.as_ref().ok_or_else(|| ApiError::validation("Authorization", "target", "missing_field"))?;
        match apps::resolve_repo_names(&s.db, t.id, names).await? {
            Ok(found) => ids.extend(found),
            Err(_) => return Err(ApiError::unprocessable("There is at least one repository that does not exist or is not accessible to the parent installation.")),
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let repo_ids = if b.repositories.is_some() || b.repository_ids.is_some() { Some(ids) } else { None };
    let (row, token) = oauth::scope_token(&s, &app, &a, target.as_ref(), repo_ids, b.permissions.as_ref()).await?;
    Ok(Json(oauth::authorization_json(&s, &row, Some(&token)).await?))
}

// ---------------------------------------------------------------------------------------------
// Legacy OAuth Authorizations API (Basic auth only)

/// A user authenticated with HTTP Basic (`login:password`, or a classic token as password).
struct BasicUser(User);

impl FromRequestParts<AppState> for BasicUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let only_basic = || ApiError::new(StatusCode::UNAUTHORIZED, "This API can only be accessed with username and password Basic Auth");
        let (login, secret) = basic(parts).ok_or_else(only_basic)?;
        if secret.is_empty() {
            return Err(only_basic());
        }
        if ["ghs_", "ghu_", "ghr_"].iter().any(|p| secret.starts_with(p)) {
            return Err(ApiError::forbidden("App tokens cannot be used with the OAuth Authorizations API."));
        }
        if let Some(u) = crate::auth::user_for_token(state, &secret).await? {
            if u.kind != "User" {
                return Err(only_basic());
            }
            return Ok(BasicUser(u));
        }
        let u = crate::auth::user_for_password(state, &login, &secret, crate::security::client_ip(parts)).await?;
        u.filter(|u| u.kind == "User").map(BasicUser).ok_or_else(ApiError::bad_credentials)
    }
}

#[derive(Deserialize, Default)]
struct ClientQuery {
    client_id: Option<String>,
}

async fn list_grants(State(s): State<AppState>, uri: OriginalUri, BasicUser(u): BasicUser, Query(p): Query<PageParams>, Query(q): Query<ClientQuery>) -> ApiResult<Paged> {
    let rows: Vec<GrantRow> = sqlx::query_as(
        "SELECT * FROM oauth_grants WHERE user_id = $1 AND ($2::text IS NULL OR client_id = $2) ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(u.id)
    .bind(&q.client_id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_grants WHERE user_id = $1 AND ($2::text IS NULL OR client_id = $2)")
        .bind(u.id)
        .bind(&q.client_id)
        .fetch_one(&s.db)
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for g in &rows {
        items.push(oauth::grant_json(&s, g).await?);
    }
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn own_grant(s: &AppState, u: &User, id: i64) -> ApiResult<GrantRow> {
    let g = oauth::grant_by_id(&s.db, id).await?;
    if g.user_id != u.id {
        return Err(ApiError::not_found());
    }
    Ok(g)
}

async fn get_grant(State(s): State<AppState>, BasicUser(u): BasicUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let g = own_grant(&s, &u, id).await?;
    Ok(Json(oauth::grant_json(&s, &g).await?))
}

async fn delete_grant(State(s): State<AppState>, BasicUser(u): BasicUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let g = own_grant(&s, &u, id).await?;
    oauth::revoke_grant(&s, u.id, g.id).await?;
    Ok(no_content())
}

async fn list_authorizations(State(s): State<AppState>, uri: OriginalUri, BasicUser(u): BasicUser, Query(p): Query<PageParams>, Query(q): Query<ClientQuery>) -> ApiResult<Paged> {
    let rows: Vec<AuthzRow> = sqlx::query_as(
        "SELECT * FROM oauth_authorizations WHERE user_id = $1 AND ($2::text IS NULL OR client_id = $2) ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(u.id)
    .bind(&q.client_id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_authorizations WHERE user_id = $1 AND ($2::text IS NULL OR client_id = $2)")
        .bind(u.id)
        .bind(&q.client_id)
        .fetch_one(&s.db)
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    for a in &rows {
        items.push(oauth::authorization_json(&s, a, None).await?);
    }
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize, Default)]
struct CreateAuthz {
    scopes: Option<Vec<String>>,
    note: Option<String>,
    note_url: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    fingerprint: Option<String>,
}

async fn client_app(s: &AppState, client_id: &str, secret: Option<&str>) -> ApiResult<AppRow> {
    let invalid = || ApiError::validation("OauthAccess", "client_id", "invalid");
    let app = apps::app_by_client_id(&s.db, client_id).await.map_err(|_| invalid())?;
    if !secret.is_some_and(|sec| apps::secret_matches(&app, sec)) {
        return Err(ApiError::validation("OauthAccess", "client_secret", "invalid"));
    }
    Ok(app)
}

async fn duplicate_check(s: &AppState, u: &User, client_id: &str, note: Option<&str>, fingerprint: Option<&str>) -> ApiResult<()> {
    let dup: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM oauth_authorizations WHERE user_id = $1 AND client_id = $2
             AND fingerprint IS NOT DISTINCT FROM $4 AND ($3::text IS NOT NULL AND note = $3))",
    )
    .bind(u.id)
    .bind(client_id)
    .bind(note)
    .bind(fingerprint)
    .fetch_one(&s.db)
    .await?;
    if dup {
        let mut e = ApiError::validation("OauthAccess", "description", "already_exists");
        e.errors = Some(serde_json::json!([{"resource": "OauthAccess", "code": "already_exists", "field": "description"}]));
        return Err(e);
    }
    Ok(())
}

async fn create_authorization(State(s): State<AppState>, BasicUser(u): BasicUser, Json(b): Json<CreateAuthz>) -> ApiResult<Response> {
    let scopes = oauth::validate_scopes(&b.scopes.clone().unwrap_or_default())?;
    let app = match b.client_id.as_deref().filter(|c| !c.is_empty()) {
        Some(cid) => Some(client_app(&s, cid, b.client_secret.as_deref()).await?),
        None => None,
    };
    let note = b.note.clone().filter(|n| !n.is_empty());
    if app.is_none() && note.is_none() {
        return Err(ApiError::validation("OauthAccess", "note", "missing_field"));
    }
    let cid = app.as_ref().map(|a| a.client_id.clone()).unwrap_or_else(|| apps::PERSONAL_CLIENT_ID.into());
    duplicate_check(&s, &u, &cid, note.as_deref(), b.fingerprint.as_deref()).await?;
    let (row, token, _) = oauth::mint(
        &s,
        u.id,
        NewAuthz { app: app.as_ref(), scopes, note, note_url: b.note_url.clone(), fingerprint: b.fingerprint.clone(), ..Default::default() },
    )
    .await?;
    let v = oauth::authorization_json(&s, &row, Some(&token)).await?;
    Ok(super::created(v))
}

#[derive(Deserialize, Default)]
struct ForApp {
    client_secret: Option<String>,
    scopes: Option<Vec<String>>,
    note: Option<String>,
    note_url: Option<String>,
    fingerprint: Option<String>,
}

async fn get_or_create(s: &AppState, u: &User, client_id: &str, fingerprint: Option<String>, b: ForApp) -> ApiResult<Response> {
    let app = client_app(s, client_id, b.client_secret.as_deref()).await?;
    let scopes = oauth::validate_scopes(&b.scopes.clone().unwrap_or_default())?;
    let existing: Option<AuthzRow> = sqlx::query_as(
        "SELECT * FROM oauth_authorizations WHERE user_id = $1 AND app_id = $2 AND fingerprint IS NOT DISTINCT FROM $3 ORDER BY id LIMIT 1",
    )
    .bind(u.id)
    .bind(app.id)
    .bind(&fingerprint)
    .fetch_optional(&s.db)
    .await?;
    if let Some(a) = existing {
        return Ok((StatusCode::OK, axum::Json(oauth::authorization_json(s, &a, None).await?)).into_response());
    }
    let (row, token, _) = oauth::mint(
        s,
        u.id,
        NewAuthz { app: Some(&app), scopes, note: b.note.filter(|n| !n.is_empty()), note_url: b.note_url, fingerprint, ..Default::default() },
    )
    .await?;
    Ok(super::created(oauth::authorization_json(s, &row, Some(&token)).await?))
}

async fn get_or_create_for_app(State(s): State<AppState>, BasicUser(u): BasicUser, Path(client_id): Path<String>, Json(b): Json<ForApp>) -> ApiResult<Response> {
    let fp = b.fingerprint.clone().filter(|f| !f.is_empty());
    get_or_create(&s, &u, &client_id, fp, b).await
}

async fn get_or_create_for_fingerprint(
    State(s): State<AppState>,
    BasicUser(u): BasicUser,
    Path((client_id, fingerprint)): Path<(String, String)>,
    Json(b): Json<ForApp>,
) -> ApiResult<Response> {
    get_or_create(&s, &u, &client_id, Some(fingerprint), b).await
}

async fn own_authz(s: &AppState, u: &User, id: i64) -> ApiResult<AuthzRow> {
    sqlx::query_as("SELECT * FROM oauth_authorizations WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(u.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_authorization(State(s): State<AppState>, BasicUser(u): BasicUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let a = own_authz(&s, &u, id).await?;
    Ok(Json(oauth::authorization_json(&s, &a, None).await?))
}

#[derive(Deserialize, Default)]
struct UpdateAuthz {
    scopes: Option<Vec<String>>,
    add_scopes: Option<Vec<String>>,
    remove_scopes: Option<Vec<String>>,
    note: Option<String>,
    note_url: Option<String>,
    fingerprint: Option<String>,
}

async fn update_authorization(State(s): State<AppState>, BasicUser(u): BasicUser, Path(id): Path<i64>, Json(b): Json<UpdateAuthz>) -> ApiResult<Json<Value>> {
    let a = own_authz(&s, &u, id).await?;
    let n = [b.scopes.is_some(), b.add_scopes.is_some(), b.remove_scopes.is_some()].iter().filter(|x| **x).count();
    if n > 1 {
        return Err(ApiError::unprocessable("You can only send one of these scope keys at a time: scopes, add_scopes, remove_scopes."));
    }
    let scopes = if let Some(sc) = &b.scopes {
        Some(oauth::validate_scopes(sc)?)
    } else if let Some(add) = &b.add_scopes {
        let mut cur = a.scopes.clone();
        cur.extend(oauth::validate_scopes(add)?);
        Some(oauth::validate_scopes(&cur)?)
    } else if let Some(rm) = &b.remove_scopes {
        Some(a.scopes.iter().filter(|x| !rm.contains(x)).cloned().collect())
    } else {
        None
    };
    let a = oauth::update(&s, &a, scopes, b.note, b.note_url, b.fingerprint).await?;
    Ok(Json(oauth::authorization_json(&s, &a, None).await?))
}

async fn delete_authorization(State(s): State<AppState>, BasicUser(u): BasicUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let a = own_authz(&s, &u, id).await?;
    oauth::revoke(&s, &a).await?;
    Ok(no_content())
}
