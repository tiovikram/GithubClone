//! OAuth tokens bound to applications.
//!
//! Every such token is a `tokens` row (so the normal auth layer accepts it) plus an
//! `oauth_authorizations` row, grouped per user and application in `oauth_grants`:
//!
//! * device flow (`/login/device/code` + `/login/oauth/access_token`) for a registered client id:
//!   GitHub Apps get a user-to-server token `ghu_` (8 h) with a refresh token `ghr_` (6 months,
//!   `grant_type=refresh_token` rotates both); OAuth apps get a classic `gho_` token with the
//!   requested scopes;
//! * the legacy OAuth Authorizations API (`/authorizations`), classic `gho_` tokens with scopes,
//!   personal (client id `00000000000000000000`) or for an application;
//! * `/applications/{client_id}/token*` checks, resets, deletes and scopes them.
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use super::{AppRow, PERSONAL_CLIENT_ID};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const USER_TOKEN_SECS: i64 = 8 * 3600;
pub const REFRESH_TOKEN_SECS: i64 = 15_897_600;
pub const LEGACY_DOCS_URL: &str = "https://docs.github.com/enterprise-server@3.22/rest/oauth-authorizations/oauth-authorizations";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuthzRow {
    pub id: i64,
    pub grant_id: i64,
    pub user_id: i64,
    pub token_id: i64,
    pub app_id: Option<i64>,
    pub client_id: String,
    pub note: Option<String>,
    pub note_url: Option<String>,
    pub fingerprint: Option<String>,
    pub scopes: Vec<String>,
    pub hashed_token: String,
    pub token_last_eight: String,
    pub refresh_hash: Option<String>,
    pub refresh_expires_at: Option<DateTime<Utc>>,
    pub target_id: Option<i64>,
    pub repository_ids: Option<Vec<i64>>,
    pub permissions: Option<Value>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GrantRow {
    pub id: i64,
    pub user_id: i64,
    pub app_id: Option<i64>,
    pub client_id: String,
    pub app_name: String,
    pub app_url: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Tokens handed to a client by the token endpoint.
pub struct IssuedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub refresh_token_expires_in: Option<i64>,
    pub scope: String,
}

/// Valid OAuth scope names (classic tokens).
pub const SCOPES: &[&str] = &[
    "repo", "repo:status", "repo_deployment", "public_repo", "repo:invite", "security_events", "admin:repo_hook",
    "write:repo_hook", "read:repo_hook", "admin:org", "write:org", "read:org", "admin:public_key", "write:public_key",
    "read:public_key", "admin:org_hook", "gist", "notifications", "user", "read:user", "user:email", "user:follow",
    "project", "read:project", "delete_repo", "write:packages", "read:packages", "delete:packages", "admin:gpg_key",
    "write:gpg_key", "read:gpg_key", "codespace", "workflow", "admin:enterprise", "manage_runners:enterprise",
    "manage_billing:enterprise", "read:enterprise", "audit_log", "read:audit_log", "site_admin", "devtools",
    "admin:ssh_signing_key", "write:ssh_signing_key", "read:ssh_signing_key", "copilot",
];

pub fn validate_scopes(scopes: &[String]) -> ApiResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for s in scopes {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if !SCOPES.contains(&s) {
            let mut e = ApiError::validation("OauthAccess", "scopes", "invalid");
            e.errors = Some(json!([{"resource": "OauthAccess", "field": "scopes", "code": "invalid", "message": format!("{s} is not a valid scope")}]));
            return Err(e);
        }
        if !out.iter().any(|x| x == s) {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

fn new_user_token() -> String {
    format!("ghu_{}", crate::auth::random_string(36))
}

fn new_refresh_token() -> String {
    format!("ghr_{}", crate::auth::random_string(76))
}

async fn ensure_grant(tx: &mut sqlx::PgConnection, user_id: i64, app: Option<&AppRow>, name: &str, url: &str, scopes: &[String]) -> ApiResult<i64> {
    let client_id = app.map(|a| a.client_id.as_str()).unwrap_or(PERSONAL_CLIENT_ID);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO oauth_grants (user_id, app_id, client_id, app_name, app_url, scopes) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (user_id, client_id) DO UPDATE SET scopes = COALESCE((SELECT array_agg(DISTINCT s) FROM unnest(oauth_grants.scopes || EXCLUDED.scopes) s), '{}'),
             updated_at = now()
         RETURNING id",
    )
    .bind(user_id)
    .bind(app.map(|a| a.id))
    .bind(client_id)
    .bind(name)
    .bind(url)
    .bind(scopes)
    .fetch_one(&mut *tx)
    .await?;
    Ok(id)
}

/// Options for a new application-bound token.
#[derive(Default)]
pub struct NewAuthz<'a> {
    pub app: Option<&'a AppRow>,
    pub scopes: Vec<String>,
    pub note: Option<String>,
    pub note_url: Option<String>,
    pub fingerprint: Option<String>,
    /// GitHub App user-to-server token (`ghu_` + refresh token, 8 h).
    pub user_to_server: bool,
    pub target_id: Option<i64>,
    pub repository_ids: Option<Vec<i64>>,
    pub permissions: Option<Value>,
}

/// Mints a token for `user_id` and records its authorization. Returns (row, token, refresh).
pub async fn mint(s: &AppState, user_id: i64, n: NewAuthz<'_>) -> ApiResult<(AuthzRow, String, Option<String>)> {
    let token = if n.user_to_server { new_user_token() } else { crate::auth::new_token() };
    let refresh = n.user_to_server.then(new_refresh_token);
    let (name, url) = match n.app {
        Some(a) => (a.name.clone(), a.external_url.clone()),
        None => (n.note.clone().unwrap_or_default(), LEGACY_DOCS_URL.to_string()),
    };
    let hash = crate::auth::sha256_hex(&token);
    let last8 = token[token.len() - 8..].to_string();
    let mut tx = s.db.begin().await?;
    let token_id: i64 = sqlx::query_scalar(
        "INSERT INTO tokens (user_id, name, token_hash, token_last_eight, scopes, expires_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() + make_interval(secs => $7) END) RETURNING id",
    )
    .bind(user_id)
    .bind(if n.user_to_server { format!("{name} (user-to-server)") } else if n.app.is_some() { format!("{name} (OAuth)") } else { name.clone() })
    .bind(&hash)
    .bind(&last8)
    .bind(&n.scopes)
    .bind(n.user_to_server)
    .bind(USER_TOKEN_SECS as f64)
    .fetch_one(&mut *tx)
    .await?;
    let grant = ensure_grant(&mut tx, user_id, n.app, if n.app.is_some() { &name } else { "Personal access tokens" }, &url, &n.scopes).await?;
    let row: AuthzRow = sqlx::query_as(
        "INSERT INTO oauth_authorizations (grant_id, user_id, token_id, app_id, client_id, note, note_url, fingerprint, scopes,
             hashed_token, token_last_eight, refresh_hash, refresh_expires_at, target_id, repository_ids, permissions, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
             CASE WHEN $12 IS NOT NULL THEN now() + make_interval(secs => $13) END, $14, $15, $16,
             CASE WHEN $17 THEN now() + make_interval(secs => $18) END)
         RETURNING *",
    )
    .bind(grant)
    .bind(user_id)
    .bind(token_id)
    .bind(n.app.map(|a| a.id))
    .bind(n.app.map(|a| a.client_id.as_str()).unwrap_or(PERSONAL_CLIENT_ID))
    .bind(&n.note)
    .bind(&n.note_url)
    .bind(&n.fingerprint)
    .bind(&n.scopes)
    .bind(&hash)
    .bind(&last8)
    .bind(refresh.as_deref().map(crate::auth::sha256_hex))
    .bind(REFRESH_TOKEN_SECS as f64)
    .bind(n.target_id)
    .bind(&n.repository_ids)
    .bind(&n.permissions)
    .bind(n.user_to_server)
    .bind(USER_TOKEN_SECS as f64)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((row, token, refresh))
}

fn issued(app: &AppRow, token: String, refresh: Option<String>, scopes: &[String]) -> IssuedTokens {
    let u2s = app.kind == "app";
    IssuedTokens {
        access_token: token,
        expires_in: u2s.then_some(USER_TOKEN_SECS),
        refresh_token_expires_in: refresh.as_ref().map(|_| REFRESH_TOKEN_SECS),
        refresh_token: refresh,
        scope: scopes.join(","),
    }
}

/// Device flow: tokens for an approved request of a registered application (None for unknown
/// client ids, which get a plain personal token as before).
pub async fn device_grant(s: &AppState, client_id: &str, user_id: i64, scopes: &str) -> ApiResult<Option<IssuedTokens>> {
    let Ok(app) = super::app_by_client_id(&s.db, client_id).await else { return Ok(None) };
    let u2s = app.kind == "app";
    let scopes: Vec<String> = if u2s { vec![] } else { validate_scopes(&scopes.split([' ', ',']).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default() };
    let (_, token, refresh) = mint(s, user_id, NewAuthz { app: Some(&app), scopes: scopes.clone(), user_to_server: u2s, ..Default::default() }).await?;
    Ok(Some(issued(&app, token, refresh, &scopes)))
}

/// `grant_type=refresh_token`: rotates a GitHub App user-to-server token. Err = OAuth error code.
pub async fn refresh(s: &AppState, client_id: &str, client_secret: Option<&str>, refresh_token: &str) -> ApiResult<Result<IssuedTokens, &'static str>> {
    let Ok(app) = super::app_by_client_id(&s.db, client_id).await else { return Ok(Err("incorrect_client_credentials")) };
    if !client_secret.is_some_and(|sec| super::secret_matches(&app, sec)) {
        return Ok(Err("incorrect_client_credentials"));
    }
    let row: Option<AuthzRow> = sqlx::query_as(
        "SELECT * FROM oauth_authorizations WHERE refresh_hash = $1 AND app_id = $2 AND refresh_expires_at > now()",
    )
    .bind(crate::auth::sha256_hex(refresh_token))
    .bind(app.id)
    .fetch_optional(&s.db)
    .await?;
    let Some(old) = row else { return Ok(Err("bad_refresh_token")) };
    // The old token and refresh token stop working.
    sqlx::query("DELETE FROM tokens WHERE id = $1").bind(old.token_id).execute(&s.db).await?;
    let (_, token, refresh) = mint(
        s,
        old.user_id,
        NewAuthz {
            app: Some(&app),
            scopes: old.scopes.clone(),
            user_to_server: true,
            target_id: old.target_id,
            repository_ids: old.repository_ids.clone(),
            permissions: old.permissions.clone(),
            ..Default::default()
        },
    )
    .await?;
    Ok(Ok(issued(&app, token, refresh, &old.scopes)))
}

// ---------------------------------------------------------------------------------------------
// Lookups and JSON

pub async fn authz_by_token(db: &sqlx::PgPool, token: &str) -> ApiResult<Option<AuthzRow>> {
    Ok(sqlx::query_as(
        "SELECT oa.* FROM oauth_authorizations oa JOIN tokens t ON t.id = oa.token_id
          WHERE oa.hashed_token = $1 AND (t.expires_at IS NULL OR t.expires_at > now())",
    )
    .bind(crate::auth::sha256_hex(token))
    .fetch_optional(db)
    .await?)
}

pub async fn grant_by_id(db: &sqlx::PgPool, id: i64) -> ApiResult<GrantRow> {
    sqlx::query_as("SELECT * FROM oauth_grants WHERE id = $1").bind(id).fetch_optional(db).await?.ok_or_else(ApiError::not_found)
}

fn app_object(grant: &GrantRow) -> Value {
    json!({ "client_id": grant.client_id, "name": grant.app_name, "url": grant.app_url })
}

/// `authorization` schema. `token` is the full token right after creation/reset/check, else "".
pub async fn authorization_json(s: &AppState, a: &AuthzRow, token: Option<&str>) -> ApiResult<Value> {
    let grant = grant_by_id(&s.db, a.grant_id).await?;
    let user = db::user_by_id(&s.db, a.user_id).await?;
    let api = s.api_url();
    let installation = match (&a.permissions, a.app_id) {
        (Some(p), Some(_)) => {
            let account = match a.target_id {
                Some(t) => db::user_by_id(&s.db, t).await.ok(),
                None => None,
            };
            json!({
                "permissions": p,
                "repository_selection": if a.repository_ids.is_some() { "selected" } else { "all" },
                "single_file_name": null,
                "has_multiple_single_files": false,
                "single_file_paths": [],
                "repositories_url": format!("{api}/user/repos"),
                "account": crate::json::user::simple(&api, s.base_url(), account.as_ref().unwrap_or(&user)),
            })
        }
        _ => Value::Null,
    };
    Ok(json!({
        "id": a.id,
        "url": format!("{api}/authorizations/{}", a.id),
        "scopes": a.scopes,
        "token": token.unwrap_or(""),
        "token_last_eight": a.token_last_eight,
        "hashed_token": a.hashed_token,
        "app": app_object(&grant),
        "note": a.note,
        "note_url": a.note_url,
        "updated_at": crate::json::ts(a.updated_at),
        "created_at": crate::json::ts(a.created_at),
        "fingerprint": a.fingerprint,
        "user": crate::json::user::simple(&api, s.base_url(), &user),
        "installation": installation,
        "expires_at": crate::json::ts_opt(a.expires_at),
    }))
}

pub async fn grant_json(s: &AppState, g: &GrantRow) -> ApiResult<Value> {
    let user = db::user_by_id(&s.db, g.user_id).await?;
    let api = s.api_url();
    Ok(json!({
        "id": g.id,
        "url": format!("{api}/applications/grants/{}", g.id),
        "app": app_object(g),
        "created_at": crate::json::ts(g.created_at),
        "updated_at": crate::json::ts(g.updated_at),
        "scopes": g.scopes,
        "user": crate::json::user::simple(&api, s.base_url(), &user),
    }))
}

// ---------------------------------------------------------------------------------------------
// Mutations

/// Deletes an authorization and its token.
pub async fn revoke(s: &AppState, a: &AuthzRow) -> ApiResult<()> {
    sqlx::query("DELETE FROM tokens WHERE id = $1").bind(a.token_id).execute(&s.db).await?;
    Ok(())
}

/// Deletes a grant: every token of that user for that application.
pub async fn revoke_grant(s: &AppState, user_id: i64, grant_id: i64) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM tokens WHERE id IN (SELECT token_id FROM oauth_authorizations WHERE grant_id = $1 AND user_id = $2)")
        .bind(grant_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM oauth_grants WHERE id = $1 AND user_id = $2").bind(grant_id).bind(user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Replaces the token of an authorization (same authorization id, new value). Returns the token.
pub async fn reset(s: &AppState, a: &AuthzRow) -> ApiResult<(AuthzRow, String)> {
    let token = if a.hashed_token.is_empty() || a.refresh_hash.is_some() { new_user_token() } else { crate::auth::new_token() };
    let hash = crate::auth::sha256_hex(&token);
    let last8 = token[token.len() - 8..].to_string();
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE tokens SET token_hash = $2, token_last_eight = $3 WHERE id = $1")
        .bind(a.token_id)
        .bind(&hash)
        .bind(&last8)
        .execute(&mut *tx)
        .await?;
    let row: AuthzRow = sqlx::query_as("UPDATE oauth_authorizations SET hashed_token = $2, token_last_eight = $3, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(a.id)
        .bind(&hash)
        .bind(&last8)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((row, token))
}

/// Updates scopes/note/fingerprint of an authorization (legacy API).
pub async fn update(s: &AppState, a: &AuthzRow, scopes: Option<Vec<String>>, note: Option<String>, note_url: Option<String>, fingerprint: Option<String>) -> ApiResult<AuthzRow> {
    let mut tx = s.db.begin().await?;
    if let Some(sc) = &scopes {
        sqlx::query("UPDATE tokens SET scopes = $2 WHERE id = $1").bind(a.token_id).bind(sc).execute(&mut *tx).await?;
    }
    let row: AuthzRow = sqlx::query_as(
        "UPDATE oauth_authorizations SET scopes = COALESCE($2, scopes), note = COALESCE($3, note), note_url = COALESCE($4, note_url),
             fingerprint = COALESCE($5, fingerprint), updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(a.id)
    .bind(&scopes)
    .bind(&note)
    .bind(&note_url)
    .bind(&fingerprint)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(sc) = &scopes {
        sqlx::query("UPDATE oauth_grants SET scopes = $2, updated_at = now() WHERE id = $1").bind(a.grant_id).bind(sc).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(row)
}

/// Scope-token: a new user-to-server token restricted to an account, repositories and
/// permissions (each a subset of what the original token can do).
pub async fn scope_token(
    s: &AppState,
    app: &AppRow,
    a: &AuthzRow,
    target: Option<&User>,
    repo_ids: Option<Vec<i64>>,
    permissions: Option<&Map<String, Value>>,
) -> ApiResult<(AuthzRow, String)> {
    if app.kind != "app" {
        return Err(ApiError::unprocessable("Only GitHub App user access tokens can be scoped."));
    }
    let granted = a.permissions.clone().unwrap_or_else(|| app.permissions.clone());
    let perms = super::narrow_permissions(&granted, permissions)?;
    let target_id = target.map(|t| t.id).or(a.target_id);
    if let Some(t) = target_id {
        if super::installation_for(&s.db, app.id, Some(t)).await?.is_none() {
            return Err(ApiError::unprocessable("The app is not installed on the target account."));
        }
    }
    if let (Some(ids), Some(t)) = (&repo_ids, target_id) {
        let inst = super::installation_for(&s.db, app.id, Some(t)).await?.ok_or_else(ApiError::not_found)?;
        let accessible = super::installation_repo_ids(&s.db, &inst).await?;
        if ids.iter().any(|i| !accessible.contains(i)) {
            return Err(ApiError::unprocessable("There is at least one repository that does not exist or is not accessible to the parent installation."));
        }
    } else if repo_ids.is_some() {
        return Err(ApiError::validation("Authorization", "target", "missing_field"));
    }
    let (row, token, _) = mint(
        s,
        a.user_id,
        NewAuthz {
            app: Some(app),
            scopes: a.scopes.clone(),
            user_to_server: true,
            target_id,
            repository_ids: repo_ids,
            permissions: Some(perms),
            ..Default::default()
        },
    )
    .await?;
    Ok((row, token))
}
