//! OAuth 2.0 device authorization grant (RFC 8628) as GitHub serves it, so `gh auth login --web`
//! and `gh auth refresh` work against forge:
//!
//! * `POST /login/device/code` (`client_id`, `scope`) -> `device_code`, `user_code`,
//!   `verification_uri`, `expires_in`, `interval`.
//! * `POST /login/oauth/access_token` (`client_id`, `device_code`, `grant_type` =
//!   `urn:ietf:params:oauth:grant-type:device_code`) -> `authorization_pending` until the user
//!   approves, then `access_token` (a new personal access token) once.
//! * The person approves at `verification_uri` (`/login/device`, a web UI page) which calls
//!   `POST /api/forge/device/approve` / `deny` with the user code; `GET /api/forge/device/{code}`
//!   describes a pending request for that page.
//!
//! Like GitHub, responses are `application/x-www-form-urlencoded` unless the client asks for
//! JSON (`Accept: application/json`); token-endpoint errors are 200s with an `error` field.
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

/// Seconds a device code stays valid (GitHub: 15 minutes).
const EXPIRES_IN: i64 = 900;
/// Minimum polling interval in seconds (GitHub: 5).
const INTERVAL: i64 = 5;
const GRANT_DEVICE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Top-level routes (outside /api): the endpoints OAuth clients call.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login/device/code", post(device_code))
        .route("/login/oauth/access_token", post(access_token))
}

/// Forge-only routes, nested under /api/forge: approval by the signed-in user.
pub fn forge_routes() -> Router<AppState> {
    Router::new()
        .route("/device/approve", post(approve))
        .route("/device/deny", post(deny))
        .route("/device/{user_code}", get(describe))
}

fn params(headers: &HeaderMap, body: &[u8]) -> Vec<(String, String)> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/json"));
    if is_json {
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        return v
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect())
            .unwrap_or_default();
    }
    url::form_urlencoded::parse(body).into_owned().collect()
}

fn param<'a>(p: &'a [(String, String)], key: &str) -> Option<&'a str> {
    p.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty())
}

/// Form-encoded (default) or JSON response, like github.com's OAuth endpoints.
fn reply(headers: &HeaderMap, status: StatusCode, fields: &[(&str, String)]) -> Response {
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("application/json"));
    if wants_json {
        let obj: serde_json::Map<String, Value> = fields
            .iter()
            .map(|(k, v)| {
                let val = if matches!(*k, "expires_in" | "interval") { v.parse::<i64>().map(Value::from).unwrap_or(json!(v)) } else { json!(v) };
                (k.to_string(), val)
            })
            .collect();
        return (status, axum::Json(Value::Object(obj))).into_response();
    }
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields.iter().map(|(k, v)| (*k, v.as_str()))).finish();
    (status, [(header::CONTENT_TYPE, "application/x-www-form-urlencoded; charset=utf-8")], body).into_response()
}

fn oauth_error(headers: &HeaderMap, error: &str, description: &str) -> Response {
    reply(
        headers,
        StatusCode::OK,
        &[
            ("error", error.to_string()),
            ("error_description", description.to_string()),
            ("error_uri", "https://docs.github.com/developers/apps/authorizing-oauth-apps#error-codes-for-the-device-flow".to_string()),
        ],
    )
}

/// `XXXX-XXXX` from an alphabet without vowels or look-alikes.
fn new_user_code() -> String {
    use rand::RngExt;
    const ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ23456789";
    let mut rng = rand::rng();
    let mut s: String = (0..8).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect();
    s.insert(4, '-');
    s
}

fn normalize_user_code(c: &str) -> String {
    let s: String = c.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if s.len() == 8 { format!("{}-{}", &s[..4], &s[4..]) } else { s }
}

/// Starts a device authorization: returns the codes for `client_id`/`scope`.
pub async fn start(state: &AppState, client_id: &str, scope: &str) -> ApiResult<(String, String)> {
    sqlx::query("DELETE FROM oauth_device_codes WHERE expires_at < now() - interval '1 day'").execute(&state.db).await?;
    let device_code = auth::random_string(40);
    for _ in 0..5 {
        let user_code = new_user_code();
        let res = sqlx::query(
            "INSERT INTO oauth_device_codes (device_code_hash, user_code, client_id, scopes, expires_at)
             VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5)) ON CONFLICT (user_code) DO NOTHING",
        )
        .bind(auth::sha256_hex(&device_code))
        .bind(&user_code)
        .bind(client_id)
        .bind(scope)
        .bind(EXPIRES_IN as f64)
        .execute(&state.db)
        .await?;
        if res.rows_affected() == 1 {
            return Ok((device_code, user_code));
        }
    }
    Err(ApiError::internal("could not allocate a user code"))
}

async fn device_code(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let p = params(&headers, &body);
    let Some(client_id) = param(&p, "client_id") else {
        return Ok(reply(&headers, StatusCode::UNAUTHORIZED, &[("error", "unauthorized_client".into()), ("error_description", "client_id is required".into())]));
    };
    let (device_code, user_code) = start(&s, client_id, param(&p, "scope").unwrap_or("")).await?;
    Ok(reply(
        &headers,
        StatusCode::OK,
        &[
            ("device_code", device_code),
            ("user_code", user_code),
            ("verification_uri", format!("{}/login/device", s.base_url())),
            ("expires_in", EXPIRES_IN.to_string()),
            ("interval", INTERVAL.to_string()),
        ],
    ))
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: i64,
    client_id: String,
    scopes: String,
    user_id: Option<i64>,
    status: String,
    expired: bool,
}

/// Outcome of polling the token endpoint with a device code.
pub enum Poll {
    Pending,
    Expired,
    Denied,
    Invalid,
    Token { token: String, scopes: String },
}

/// Exchanges an approved device code for a new personal access token (once).
pub async fn poll(state: &AppState, client_id: &str, device_code: &str) -> ApiResult<Poll> {
    let row: Option<DeviceRow> = sqlx::query_as(
        "SELECT id, client_id, scopes, user_id, status, expires_at < now() AS expired FROM oauth_device_codes WHERE device_code_hash = $1",
    )
    .bind(auth::sha256_hex(device_code))
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row.filter(|r| r.client_id == client_id) else { return Ok(Poll::Invalid) };
    if row.expired {
        return Ok(Poll::Expired);
    }
    match (row.status.as_str(), row.user_id) {
        ("denied", _) => Ok(Poll::Denied),
        ("approved", Some(uid)) => {
            // Consume the code first so a token is minted at most once.
            let gone = sqlx::query("DELETE FROM oauth_device_codes WHERE id = $1").bind(row.id).execute(&state.db).await?.rows_affected();
            if gone == 0 {
                return Ok(Poll::Invalid);
            }
            let (_, token) = crate::forge_api::create_token_for(state, uid, &format!("{} (device flow)", row.client_id)).await?;
            Ok(Poll::Token { token, scopes: row.scopes })
        }
        _ => Ok(Poll::Pending),
    }
}

async fn access_token(State(s): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let p = params(&headers, &body);
    let client_id = param(&p, "client_id").unwrap_or("");
    if param(&p, "grant_type") != Some(GRANT_DEVICE) {
        return Ok(oauth_error(&headers, "unsupported_grant_type", "Only the device authorization grant is supported."));
    }
    let Some(code) = param(&p, "device_code") else {
        return Ok(oauth_error(&headers, "incorrect_device_code", "The device_code provided is not valid."));
    };
    Ok(match poll(&s, client_id, code).await? {
        Poll::Pending => oauth_error(&headers, "authorization_pending", "The authorization request is still pending."),
        Poll::Expired => oauth_error(&headers, "expired_token", "The device_code has expired."),
        Poll::Denied => oauth_error(&headers, "access_denied", "The authorization request was denied."),
        Poll::Invalid => oauth_error(&headers, "incorrect_device_code", "The device_code provided is not valid."),
        Poll::Token { token, scopes } => reply(
            &headers,
            StatusCode::OK,
            &[("access_token", token), ("token_type", "bearer".into()), ("scope", scopes.split_whitespace().collect::<Vec<_>>().join(","))],
        ),
    })
}

#[derive(serde::Deserialize)]
struct Decision {
    user_code: String,
}

/// Approves (or denies) a pending, unexpired request for `user_id`. Returns false if no such request.
pub async fn decide(state: &AppState, user_id: i64, user_code: &str, approve: bool) -> ApiResult<bool> {
    let res = sqlx::query(
        "UPDATE oauth_device_codes SET status = $3, user_id = $2
          WHERE user_code = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(normalize_user_code(user_code))
    .bind(user_id)
    .bind(if approve { "approved" } else { "denied" })
    .execute(&state.db)
    .await?;
    Ok(res.rows_affected() == 1)
}

async fn approve(State(s): State<AppState>, AuthUser(u): AuthUser, Json(d): Json<Decision>) -> ApiResult<Response> {
    if !decide(&s, u.id, &d.user_code, true).await? {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn deny(State(s): State<AppState>, AuthUser(u): AuthUser, Json(d): Json<Decision>) -> ApiResult<Response> {
    if !decide(&s, u.id, &d.user_code, false).await? {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn describe(State(s): State<AppState>, AuthUser(_u): AuthUser, Path(code): Path<String>) -> ApiResult<axum::Json<Value>> {
    let row: Option<(String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT client_id, scopes, expires_at FROM oauth_device_codes WHERE user_code = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(normalize_user_code(&code))
    .fetch_optional(&s.db)
    .await?;
    let (client_id, scopes, expires_at) = row.ok_or_else(ApiError::not_found)?;
    Ok(axum::Json(json!({
        "user_code": normalize_user_code(&code),
        "client_id": client_id,
        "scopes": scopes.split_whitespace().collect::<Vec<_>>(),
        "expires_at": crate::json::ts(expires_at),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_codes_are_normalized() {
        assert_eq!(normalize_user_code("abcd-2345"), "ABCD-2345");
        assert_eq!(normalize_user_code("abcd2345"), "ABCD-2345");
        let c = new_user_code();
        assert_eq!(c.len(), 9);
        assert_eq!(normalize_user_code(&c), c);
    }
}
