//! Request authentication: PATs (token/Bearer/Basic), Basic login:password, session cookies.
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, Method};
use base64::Engine;
use rand::RngExt;
use sha2::{Digest, Sha256};

use crate::db::User;
use crate::error::ApiError;
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "forge_session";
pub const CSRF_HEADER: &str = "x-forge-csrf";

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn random_string(len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..len).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect()
}

pub fn new_token() -> String {
    format!("gho_{}", random_string(36))
}

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    Ok(Argon2::default()
        .hash_password(pw.as_bytes())
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .to_string())
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// How a request was authenticated.
#[derive(Clone, Debug)]
pub enum AuthResult {
    Anonymous,
    User(User),
    /// Credentials were presented but were invalid -> 401 "Bad credentials".
    Invalid,
}

pub async fn user_for_token(state: &AppState, token: &str) -> Result<Option<User>, ApiError> {
    if token.is_empty() {
        return Ok(None);
    }
    if token.starts_with("ghs_") {
        return crate::apps::scope::bot_for_installation_token(&state.db, token).await;
    }
    let hash = sha256_hex(token);
    let user = sqlx::query_as::<_, User>(
        "SELECT u.* FROM tokens t JOIN users u ON u.id = t.user_id
          WHERE t.token_hash = $1 AND (t.expires_at IS NULL OR t.expires_at > now())",
    )
    .bind(&hash)
    .fetch_optional(&state.db)
    .await?;
    if user.is_some() {
        // Throttled bookkeeping; never blocks the request.
        let db = state.db.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "UPDATE tokens SET last_used_at = now() WHERE token_hash = $1
                  AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
            )
            .bind(hash)
            .execute(&db)
            .await;
        });
    }
    Ok(user)
}

/// Error returned while password attempts for a login are throttled (see `crate::security`).
pub fn too_many_attempts() -> ApiError {
    ApiError::new(axum::http::StatusCode::TOO_MANY_REQUESTS, "Too many failed login attempts. Try again later.")
}

/// Checks `login` (or email) and `password`. Failed attempts are counted per login and client
/// address; once throttled, this returns a 429 error without checking the password.
pub async fn user_for_password(state: &AppState, login: &str, password: &str, client: Option<std::net::IpAddr>) -> Result<Option<User>, ApiError> {
    if crate::security::login_throttled(&state.db, login, client).await? {
        return Err(too_many_attempts());
    }
    // An exact login match wins over another account that uses the same string as its email.
    let u = sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE lower(login) = lower($1) OR lower(email) = lower($1)
          ORDER BY (lower(login) = lower($1)) DESC, id LIMIT 1",
    )
    .bind(login)
    .fetch_optional(&state.db)
    .await?;
    // Unknown users still pay for a hash verification so timing does not reveal which logins exist.
    let hash = u.as_ref().and_then(|u| u.password_hash.clone()).unwrap_or_else(|| dummy_hash().to_string());
    let pw = password.to_string();
    let ok = tokio::task::spawn_blocking(move || verify_password(&pw, &hash))
        .await
        .unwrap_or(false);
    let u = u.filter(|u| ok && u.password_hash.is_some());
    crate::security::record_login(&state.db, login, client, u.is_some()).await?;
    Ok(u)
}

fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash_password(&random_string(24)).unwrap_or_default())
}

async fn user_for_session(state: &AppState, sid: &str) -> Result<Option<User>, ApiError> {
    Ok(sqlx::query_as::<_, User>(
        "SELECT u.* FROM sessions s JOIN users u ON u.id = s.user_id
          WHERE s.id_hash = $1 AND s.expires_at > now()",
    )
    .bind(sha256_hex(sid))
    .fetch_optional(&state.db)
    .await?)
}

pub fn cookie_value(parts: &Parts, name: &str) -> Option<String> {
    parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// Authenticate from headers. Results are cached in request extensions.
pub async fn authenticate(parts: &mut Parts, state: &AppState) -> Result<AuthResult, ApiError> {
    if let Some(r) = parts.extensions.get::<AuthResult>() {
        return Ok(r.clone());
    }
    let result = authenticate_uncached(parts, state).await?;
    parts.extensions.insert(result.clone());
    Ok(result)
}

async fn authenticate_uncached(parts: &Parts, state: &AppState) -> Result<AuthResult, ApiError> {
    if let Some(h) = parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let h = h.trim();
        let (scheme, cred) = h.split_once(' ').unwrap_or((h, ""));
        let cred = cred.trim();
        let user = match scheme.to_ascii_lowercase().as_str() {
            "token" | "bearer" => user_for_token(state, cred).await?,
            "basic" => {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(cred)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default();
                let (login, secret) = decoded.split_once(':').unwrap_or((&decoded, ""));
                match user_for_token(state, secret).await? {
                    Some(u) => Some(u),
                    None if !secret.is_empty() => user_for_password(state, login, secret, crate::security::client_ip(parts)).await?,
                    None => None,
                }
            }
            _ => None,
        };
        return Ok(user.map(AuthResult::User).unwrap_or(AuthResult::Invalid));
    }
    if let Some(sid) = cookie_value(parts, SESSION_COOKIE) {
        if let Some(u) = user_for_session(state, &sid).await? {
            let safe = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
            if safe || parts.headers.contains_key(CSRF_HEADER) {
                return Ok(AuthResult::User(u));
            }
            // Cookie without CSRF header on a mutating request: treat as anonymous.
        }
    }
    Ok(AuthResult::Anonymous)
}

/// Optional authentication. Invalid credentials still yield 401 like GitHub.
pub struct MaybeUser(pub Option<User>);

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        match authenticate(parts, state).await? {
            AuthResult::User(u) => Ok(MaybeUser(Some(u))),
            AuthResult::Anonymous => Ok(MaybeUser(None)),
            AuthResult::Invalid => Err(ApiError::bad_credentials()),
        }
    }
}

/// Required authentication (401 otherwise).
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        match authenticate(parts, state).await? {
            AuthResult::User(u) => Ok(AuthUser(u)),
            AuthResult::Anonymous => Err(ApiError::unauthorized()),
            AuthResult::Invalid => Err(ApiError::bad_credentials()),
        }
    }
}

impl MaybeUser {
    pub fn as_ref(&self) -> Option<&User> {
        self.0.as_ref()
    }
}
