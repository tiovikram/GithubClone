//! Forge-only endpoints under /api/forge (not part of the GitHub API): signup, web sessions,
//! personal access tokens.
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::Router;
use crate::extract::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, AuthUser, SESSION_COOKIE};
use crate::db::User;
use crate::error::{ApiError, ApiResult};
use crate::json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/signup", post(signup))
        .route("/session", post(login).delete(logout).get(whoami))
        .route("/tokens", post(create_token).get(list_tokens))
        .route("/tokens/{id}", delete(delete_token))
        .route("/healthz", axum::routing::get(|| async { "ok" }))
}

pub fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && !login.ends_with('-')
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !matches!(
            login.to_ascii_lowercase().as_str(),
            "api" | "login" | "logout" | "signup" | "new" | "settings" | "assets" | "avatars" | "organizations" | "orgs" | "users" | "explore" | "notifications" | "search" | "static"
        )
}

pub async fn create_user(state: &AppState, login: &str, email: Option<&str>, password: &str, admin: bool) -> ApiResult<User> {
    if !valid_login(login) {
        return Err(ApiError::validation("User", "login", "invalid"));
    }
    if password.len() < 8 {
        return Err(ApiError::validation("User", "password", "invalid"));
    }
    let pw = password.to_string();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&pw)).await.map_err(ApiError::internal)??;
    // The first user to sign up becomes site admin.
    let u: Option<User> = sqlx::query_as(
        "INSERT INTO users (login, email, password_hash, site_admin)
         VALUES ($1, $2, $3, $4 OR NOT EXISTS (SELECT 1 FROM users WHERE kind = 'User'))
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(login)
    .bind(email)
    .bind(hash)
    .bind(admin)
    .fetch_optional(&state.db)
    .await?;
    u.ok_or_else(|| ApiError::validation("User", "login", "already_exists"))
}

pub async fn create_token_for(state: &AppState, user_id: i64, name: &str) -> ApiResult<(i64, String)> {
    let token = auth::new_token();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tokens (user_id, name, token_hash, token_last_eight) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(auth::sha256_hex(&token))
    .bind(&token[token.len() - 8..])
    .fetch_one(&state.db)
    .await?;
    Ok((id, token))
}

async fn session_response(state: &AppState, u: &User, status: StatusCode) -> ApiResult<Response> {
    let sid = auth::random_string(48);
    sqlx::query("INSERT INTO sessions (id_hash, user_id, expires_at) VALUES ($1, $2, now() + interval '30 days')")
        .bind(auth::sha256_hex(&sid))
        .bind(u.id)
        .execute(&state.db)
        .await?;
    let secure = if state.base_url().starts_with("https://") { "; Secure" } else { "" };
    let cookie = format!("{SESSION_COOKIE}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{secure}");
    let body = json::user::private(&state.api_url(), state.base_url(), u, 0, 0);
    let mut res = (status, Json(body)).into_response();
    res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    Ok(res)
}

#[derive(Deserialize)]
struct Signup {
    login: String,
    email: Option<String>,
    password: String,
}

async fn signup(State(s): State<AppState>, Json(req): Json<Signup>) -> ApiResult<Response> {
    let u = create_user(&s, &req.login, req.email.as_deref(), &req.password, false).await?;
    session_response(&s, &u, StatusCode::CREATED).await
}

#[derive(Deserialize)]
struct Login {
    login: String,
    password: String,
}

async fn login(State(s): State<AppState>, Json(req): Json<Login>) -> ApiResult<Response> {
    let u = auth::user_for_password(&s, &req.login, &req.password)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Incorrect username or password."))?;
    session_response(&s, &u, StatusCode::OK).await
}

async fn whoami(State(s): State<AppState>, AuthUser(u): AuthUser) -> Json<Value> {
    Json(json::user::private(&s.api_url(), s.base_url(), &u, 0, 0))
}

async fn logout(State(s): State<AppState>, req: axum::extract::Request) -> ApiResult<Response> {
    let (parts, _) = req.into_parts();
    if let Some(sid) = auth::cookie_value(&parts, SESSION_COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE id_hash = $1").bind(auth::sha256_hex(&sid)).execute(&s.db).await?;
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("{SESSION_COOKIE}=; Path=/; HttpOnly; Max-Age=0")).unwrap(),
    );
    Ok(res)
}

#[derive(Deserialize)]
struct NewToken {
    name: Option<String>,
}

async fn create_token(State(s): State<AppState>, AuthUser(u): AuthUser, Json(req): Json<NewToken>) -> ApiResult<Response> {
    let name = req.name.unwrap_or_else(|| "token".into());
    let (id, token) = create_token_for(&s, u.id, &name).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "name": name, "token": token, "scopes": crate::api::SCOPES }))).into_response())
}

async fn list_tokens(State(s): State<AppState>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let rows: Vec<(i64, String, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, name, token_last_eight, created_at, last_used_at FROM tokens WHERE user_id = $1 ORDER BY id DESC",
    )
    .bind(u.id)
    .fetch_all(&s.db)
    .await?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|(id, name, last8, created, used)| {
                json!({ "id": id, "name": name, "token_last_eight": last8, "created_at": json::ts(created), "last_used_at": json::ts_opt(used) })
            })
            .collect(),
    )))
}

async fn delete_token(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let n = sqlx::query("DELETE FROM tokens WHERE id = $1 AND user_id = $2").bind(id).bind(u.id).execute(&s.db).await?;
    if n.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
