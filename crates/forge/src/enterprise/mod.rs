//! Enterprise administration (GHES-like single enterprise). forge models exactly one
//! enterprise, whose slug is `FORGE_ENTERPRISE_SLUG` (default `forge`); site admins
//! (`users.site_admin`) are its owners.
//!
//! Service layer used by the REST handlers in `api/ent_*.rs` (and callable by other areas):
//! - settings: [`get_setting`] / [`set_setting`] (key/value JSON in `ent_settings`);
//! - audit log: [`audit`] records an instance-level event in `ent_audit_log`; organization
//!   events go to the orgs area's `org_audit_log` (`crate::orgs::audit`). The enterprise
//!   audit-log endpoint reads both;
//! - global webhooks: [`emit_global`] delivers an event to every active global hook through the
//!   standard webhook queue;
//! - users: [`admin_create_user`], [`rename_user`], [`delete_user`], [`set_suspended`],
//!   [`set_site_admin`], [`create_impersonation_token`];
//! - organizations: [`rename_org`], [`org_created`];
//! - maintenance mode: [`maintenance_guard`] middleware (503 for everyone but site admins and the
//!   IP exception list while enabled).
pub mod prereceive;

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::auth::{AuthResult, AuthUser};
use crate::db::User;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// The enterprise slug (`FORGE_ENTERPRISE_SLUG`, default `forge`).
pub fn slug() -> String {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| {
        std::env::var("FORGE_ENTERPRISE_SLUG").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "forge".into())
    })
    .clone()
}

/// 404 unless `name` is this install's enterprise.
pub fn check_enterprise(name: &str) -> ApiResult<()> {
    if name.eq_ignore_ascii_case(&slug()) {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

pub fn forbidden() -> ApiError {
    ApiError::forbidden("You must be a site administrator to access this resource.")
}

/// 403 unless `u` is a site admin (enterprise owner).
pub fn require_site_admin(u: &User) -> ApiResult<()> {
    if u.site_admin { Ok(()) } else { Err(forbidden()) }
}

/// Extractor: an authenticated site administrator (401 / 403 otherwise).
pub struct SiteAdmin(pub User);

impl FromRequestParts<AppState> for SiteAdmin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let AuthUser(u) = AuthUser::from_request_parts(parts, state).await?;
        require_site_admin(&u)?;
        Ok(SiteAdmin(u))
    }
}

/// `enterprise` object for webhook payloads.
pub fn enterprise_json(state: &AppState) -> Value {
    let slug = slug();
    json!({
        "id": 1,
        "slug": slug,
        "name": slug,
        "node_id": crate::ids::encode("Enterprise", 1),
        "avatar_url": format!("{}/avatars/{slug}", state.base_url()),
        "description": null,
        "website_url": null,
        "html_url": format!("{}/enterprises/{slug}", state.base_url()),
        "created_at": null,
        "updated_at": null,
    })
}

// ---------------------------------------------------------------------------------------------
// Settings

pub async fn get_setting(db: &sqlx::PgPool, key: &str) -> ApiResult<Option<Value>> {
    Ok(sqlx::query_scalar("SELECT value FROM ent_settings WHERE key = $1").bind(key).fetch_optional(db).await?)
}

pub async fn set_setting(db: &sqlx::PgPool, key: &str, value: &Value) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO ent_settings (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete_setting(db: &sqlx::PgPool, key: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM ent_settings WHERE key = $1").bind(key).execute(db).await?;
    Ok(())
}

/// A settings object: the stored value merged over `defaults` (top-level keys).
pub async fn setting_with_defaults(db: &sqlx::PgPool, key: &str, defaults: Value) -> ApiResult<Value> {
    let mut v = defaults;
    if let (Some(Value::Object(stored)), Some(o)) = (get_setting(db, key).await?, v.as_object_mut()) {
        for (k, x) in stored {
            o.insert(k, x);
        }
    }
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// Audit log

/// Records an audit event. `data` may carry `user`, `org`, `repo` (logins / full names) and
/// their ids (`user_id`, `org_id`, `repo_id`), which are indexed as columns; everything is kept
/// in `data`. Never fails the caller.
pub async fn audit(state: &AppState, action: &str, actor: Option<&User>, data: Value) {
    let s = |k: &str| data.get(k).and_then(Value::as_str).map(str::to_string);
    let i = |k: &str| data.get(k).and_then(Value::as_i64);
    let res = sqlx::query(
        "INSERT INTO ent_audit_log (action, actor_id, actor, user_id, user_login, org_id, org, repo_id, repo, actor_ip, data)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(action)
    .bind(actor.map(|u| u.id))
    .bind(actor.map(|u| u.login.clone()))
    .bind(i("user_id"))
    .bind(s("user"))
    .bind(i("org_id"))
    .bind(s("org"))
    .bind(i("repo_id"))
    .bind(s("repo"))
    .bind(s("actor_ip"))
    .bind(&data)
    .execute(&state.db)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, action, "could not record audit event");
    }
}

// ---------------------------------------------------------------------------------------------
// Global webhooks

/// Emits `event` (with `payload`, which should carry `action` and `sender`) to every active
/// global webhook subscribed to it. `enterprise` is added. Errors are logged, never returned.
pub async fn emit_global(state: &AppState, event: &str, mut payload: Value) {
    if let Some(o) = payload.as_object_mut() {
        o.entry("enterprise").or_insert_with(|| enterprise_json(state));
    }
    let hooks: Result<Vec<crate::webhooks::HookRow>, _> = sqlx::query_as(
        "SELECT * FROM webhooks WHERE is_global AND active AND (events && ARRAY[$1, '*']::text[])",
    )
    .bind(event)
    .fetch_all(&state.db)
    .await;
    match hooks {
        Ok(hooks) => {
            for h in hooks {
                let mut p = payload.clone();
                p["hook_id"] = json!(h.id);
                if let Err(e) = crate::webhooks::create_delivery(state, &h, event, None, p, false).await {
                    tracing::warn!(error = %format!("{e:#}"), "global webhook delivery not queued");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "global webhooks lookup failed"),
    }
}

fn user_json(state: &AppState, u: &User) -> Value {
    crate::json::user::simple(&state.api_url(), state.base_url(), u)
}

/// Global `user` event (`created`, `deleted`, `suspended`, `unsuspended`, `renamed`, ...).
pub async fn user_event(state: &AppState, action: &str, user: &User, actor: Option<&User>, extra: Value) {
    let mut p = json!({ "action": action, "user": user_json(state, user) });
    if let Some(a) = actor {
        p["sender"] = user_json(state, a);
    }
    if let Value::Object(x) = extra {
        for (k, v) in x {
            p[k] = v;
        }
    }
    emit_global(state, "user", p).await;
}

/// Global `organization` event (`created`, `deleted`, `renamed`, ...).
pub async fn org_event(state: &AppState, action: &str, org: &User, actor: Option<&User>, extra: Value) {
    let mut p = json!({
        "action": action,
        "organization": crate::json::user::org_simple(&state.api_url(), state.base_url(), org),
    });
    if let Some(a) = actor {
        p["sender"] = user_json(state, a);
    }
    if let Value::Object(x) = extra {
        for (k, v) in x {
            p[k] = v;
        }
    }
    emit_global(state, "organization", p).await;
}

/// Hook for code that creates organizations (POST /admin/organizations, ...): audit + global
/// `organization.created` event.
pub async fn org_created(state: &AppState, org: &User, actor: Option<&User>) {
    crate::orgs::audit_quiet(&state.db, org, actor, "org.create", json!({})).await;
    org_event(state, "created", org, actor, json!({})).await;
}

/// Hook for code that creates users (signup, admin, SCIM): audit + global `user.created`.
pub async fn user_created(state: &AppState, user: &User, actor: Option<&User>) {
    audit(state, "user.create", actor, json!({"user": user.login, "user_id": user.id})).await;
    user_event(state, "created", user, actor, json!({})).await;
}

// ---------------------------------------------------------------------------------------------
// Users

pub async fn suspended_at(db: &sqlx::PgPool, user_id: i64) -> ApiResult<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar("SELECT suspended_at FROM users WHERE id = $1").bind(user_id).fetch_one(db).await?)
}

/// Creates a user without a password (they sign in through an impersonation token, SCIM/SSO
/// or after an admin sets credentials). `suspended` creates the account suspended.
pub async fn admin_create_user(state: &AppState, actor: Option<&User>, login: &str, email: Option<&str>, suspended: bool) -> ApiResult<User> {
    if !crate::forge_api::valid_login(login) {
        return Err(ApiError::validation("User", "login", "invalid"));
    }
    let u: Option<User> = sqlx::query_as(
        "INSERT INTO users (login, email, suspended_at) VALUES ($1, $2, CASE WHEN $3 THEN now() END)
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(login)
    .bind(email.filter(|e| !e.is_empty()))
    .bind(suspended)
    .fetch_optional(&state.db)
    .await?;
    let u = u.ok_or_else(|| ApiError::validation("User", "login", "already_exists"))?;
    user_created(state, &u, actor).await;
    if suspended {
        audit(state, "user.suspend", actor, json!({"user": u.login, "user_id": u.id})).await;
    }
    Ok(u)
}

/// Renames a user or organization (`kind` is checked by the caller). URLs derive from the
/// login, so repositories follow automatically.
async fn rename_account(state: &AppState, account: &User, new_login: &str) -> ApiResult<User> {
    if !crate::forge_api::valid_login(new_login) {
        return Err(ApiError::validation(&account.kind, "login", "invalid"));
    }
    let renamed: Option<User> = sqlx::query_as(
        "UPDATE users SET login = $2, updated_at = now() WHERE id = $1
           AND NOT EXISTS (SELECT 1 FROM users WHERE lower(login) = lower($2) AND id <> $1) RETURNING *",
    )
    .bind(account.id)
    .bind(new_login)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation(&account.kind, "login", "already_exists"),
        e => e.into(),
    })?;
    renamed.ok_or_else(|| ApiError::validation(&account.kind, "login", "already_exists"))
}

pub async fn rename_user(state: &AppState, actor: &User, user: &User, new_login: &str) -> ApiResult<User> {
    let u = rename_account(state, user, new_login).await?;
    audit(state, "user.rename", Some(actor), json!({"user": u.login, "user_id": u.id, "old_login": user.login})).await;
    user_event(state, "renamed", &u, Some(actor), json!({"changes": {"login": {"from": user.login}}})).await;
    Ok(u)
}

pub async fn rename_org(state: &AppState, actor: &User, org: &User, new_login: &str) -> ApiResult<User> {
    let o = rename_account(state, org, new_login).await?;
    crate::orgs::audit_quiet(&state.db, &o, Some(actor), "org.rename", json!({"old_login": org.login})).await;
    org_event(state, "renamed", &o, Some(actor), json!({"changes": {"login": {"from": org.login}}})).await;
    Ok(o)
}

/// Deletes a user and everything they own (repositories cascade; their packs are queued for
/// `forge gc`). Site admins must be demoted first; nobody can delete themselves here.
pub async fn delete_user(state: &AppState, actor: &User, user: &User) -> ApiResult<()> {
    if user.id == actor.id {
        return Err(ApiError::forbidden("You cannot delete your own account."));
    }
    if user.site_admin {
        return Err(ApiError::forbidden("Site administrators must be demoted before they can be deleted."));
    }
    sqlx::query("DELETE FROM users WHERE id = $1 AND kind = 'User'").bind(user.id).execute(&state.db).await?;
    audit(state, "user.delete", Some(actor), json!({"user": user.login, "user_id": user.id})).await;
    user_event(state, "deleted", user, Some(actor), json!({})).await;
    Ok(())
}

/// Suspends or unsuspends a user. Suspended users cannot authenticate (tokens, sessions,
/// password, SSH keys, git) — `crate::auth` and `crate::ssh::keys` filter on `suspended_at`.
/// Their web sessions are also revoked.
pub async fn set_suspended(state: &AppState, actor: Option<&User>, user: &User, suspend: bool, reason: Option<&str>) -> ApiResult<()> {
    if suspend {
        if user.site_admin {
            return Err(ApiError::forbidden("Site administrators cannot be suspended. Demote the user first."));
        }
        if actor.is_some_and(|a| a.id == user.id) {
            return Err(ApiError::forbidden("You cannot suspend yourself."));
        }
        sqlx::query("UPDATE users SET suspended_at = COALESCE(suspended_at, now()), suspension_reason = $2, updated_at = now() WHERE id = $1")
            .bind(user.id)
            .bind(reason)
            .execute(&state.db)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(user.id).execute(&state.db).await?;
    } else {
        sqlx::query("UPDATE users SET suspended_at = NULL, suspension_reason = NULL, updated_at = now() WHERE id = $1")
            .bind(user.id)
            .execute(&state.db)
            .await?;
    }
    let action = if suspend { "suspended" } else { "unsuspended" };
    audit(state, if suspend { "user.suspend" } else { "user.unsuspend" }, actor, json!({"user": user.login, "user_id": user.id, "reason": reason}))
        .await;
    user_event(state, action, user, actor, json!({})).await;
    Ok(())
}

pub async fn set_site_admin(state: &AppState, actor: &User, user: &User, admin: bool) -> ApiResult<()> {
    if user.kind != "User" {
        return Err(ApiError::not_found());
    }
    if !admin && user.id == actor.id {
        return Err(ApiError::forbidden("You cannot demote yourself."));
    }
    sqlx::query("UPDATE users SET site_admin = $2, updated_at = now() WHERE id = $1").bind(user.id).bind(admin).execute(&state.db).await?;
    audit(state, if admin { "user.promote" } else { "user.demote" }, Some(actor), json!({"user": user.login, "user_id": user.id})).await;
    user_event(state, if admin { "promoted" } else { "demoted" }, user, Some(actor), json!({})).await;
    Ok(())
}

/// Token row as listed by /admin/tokens and returned by impersonation.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TokenRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub token_hash: String,
    pub token_last_eight: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub impersonation: bool,
}

/// Creates an impersonation OAuth token for `user` (it authenticates as the user). Returns the
/// row and the token value (only shown once).
pub async fn create_impersonation_token(state: &AppState, actor: &User, user: &User, scopes: &[String]) -> ApiResult<(TokenRow, String)> {
    if user.kind != "User" {
        return Err(ApiError::not_found());
    }
    let token = crate::auth::new_token();
    let row: TokenRow = sqlx::query_as(
        "INSERT INTO tokens (user_id, name, token_hash, token_last_eight, scopes, impersonation)
         VALUES ($1, 'GitHub Site Administrator impersonation', $2, $3, $4, TRUE)
         RETURNING id, user_id, name, token_hash, token_last_eight, scopes, created_at, last_used_at, expires_at, impersonation",
    )
    .bind(user.id)
    .bind(crate::auth::sha256_hex(&token))
    .bind(&token[token.len() - 8..])
    .bind(scopes)
    .fetch_one(&state.db)
    .await?;
    audit(state, "oauth_access.create", Some(actor), json!({"user": user.login, "user_id": user.id, "impersonation": true, "scopes": scopes}))
        .await;
    Ok((row, token))
}

// ---------------------------------------------------------------------------------------------
// Maintenance mode

#[derive(Clone, Debug, Default)]
pub struct Maintenance {
    pub enabled: bool,
    /// Scheduled start; maintenance is active once this is in the past.
    pub when: Option<DateTime<Utc>>,
    pub ip_exception_list: Vec<String>,
    pub message: String,
}

impl Maintenance {
    pub fn from_value(v: &Value) -> Maintenance {
        Maintenance {
            enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            when: v.get("when").and_then(Value::as_str).and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc)),
            ip_exception_list: v
                .get("ip_exception_list")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            message: v.get("maintenance_mode_message").and_then(Value::as_str).unwrap_or("").to_string(),
        }
    }
    pub fn active(&self) -> bool {
        self.enabled && self.when.is_none_or(|w| w <= Utc::now())
    }
    pub fn status(&self) -> &'static str {
        match (self.enabled, self.when) {
            (false, _) => "off",
            (true, Some(w)) if w > Utc::now() => "scheduled",
            _ => "on",
        }
    }
}

static MAINT: Mutex<Option<(Instant, Maintenance)>> = Mutex::new(None);
const MAINT_TTL: Duration = Duration::from_secs(2);

/// Current maintenance state (cached for two seconds per process; every pod reads Postgres).
pub async fn maintenance(state: &AppState) -> Maintenance {
    if let Some((at, m)) = MAINT.lock().unwrap().as_ref() {
        if at.elapsed() < MAINT_TTL {
            return m.clone();
        }
    }
    let m = match get_setting(&state.db, "maintenance").await {
        Ok(Some(v)) => Maintenance::from_value(&v),
        Ok(None) => Maintenance::default(),
        Err(_) => return MAINT.lock().unwrap().as_ref().map(|(_, m)| m.clone()).unwrap_or_default(),
    };
    *MAINT.lock().unwrap() = Some((Instant::now(), m.clone()));
    m
}

/// Drops the cached maintenance state (after a change on this pod).
pub fn invalidate_maintenance_cache() {
    *MAINT.lock().unwrap() = None;
}

fn ip_in_list(ip: std::net::IpAddr, list: &[String]) -> bool {
    list.iter().any(|entry| {
        let (net, bits) = match entry.split_once('/') {
            Some((n, b)) => (n, b.parse::<u32>().ok()),
            None => (entry.as_str(), None),
        };
        let Ok(net) = net.trim().parse::<std::net::IpAddr>() else { return false };
        match (ip, net) {
            (std::net::IpAddr::V4(a), std::net::IpAddr::V4(b)) => {
                let bits = bits.unwrap_or(32).min(32);
                let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
                (u32::from(a) & mask) == (u32::from(b) & mask)
            }
            (std::net::IpAddr::V6(a), std::net::IpAddr::V6(b)) => {
                let bits = bits.unwrap_or(128).min(128);
                let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
                (u128::from(a) & mask) == (u128::from(b) & mask)
            }
            _ => false,
        }
    })
}

/// Paths that stay reachable during maintenance: health checks, the management API (so admins
/// can turn maintenance off) and the session endpoint (so admins can sign in).
fn maintenance_exempt(path: &str) -> bool {
    matches!(path, "/healthz" | "/readyz" | "/api/forge/session" | "/api/forge/healthz") || path.starts_with("/api/v3/manage/")
}

/// Middleware (outermost): records this pod's heartbeat and, while maintenance mode is on,
/// answers 503 to everyone except site admins, addresses on the IP exception list and the
/// exempt paths.
pub async fn maintenance_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    heartbeat(&state);
    let m = maintenance(&state).await;
    if !m.active() || maintenance_exempt(req.uri().path()) {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let allowed_ip = crate::security::client_ip(&parts).is_some_and(|ip| ip_in_list(ip, &m.ip_exception_list));
    let admin = matches!(crate::auth::authenticate(&mut parts, &state).await, Ok(AuthResult::User(ref u)) if u.site_admin);
    if allowed_ip || admin {
        return next.run(Request::from_parts(parts, body)).await;
    }
    let msg = if m.message.is_empty() { "This instance is undergoing maintenance. Please try again later.".to_string() } else { m.message };
    let mut res = (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({ "message": msg, "documentation_url": crate::error::DOCS_URL })))
        .into_response();
    res.headers_mut().insert(axum::http::header::RETRY_AFTER, axum::http::HeaderValue::from_static("60"));
    res
}

// ---------------------------------------------------------------------------------------------
// Nodes (management API): every process records a heartbeat in `ent_nodes`.

/// This process's node identity (uuid, hostname).
pub fn node_identity() -> &'static (String, String, DateTime<Utc>) {
    static N: OnceLock<(String, String, DateTime<Utc>)> = OnceLock::new();
    N.get_or_init(|| {
        let host = std::env::var("HOSTNAME")
            .ok()
            .filter(|h| !h.is_empty())
            .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()).filter(|h| !h.is_empty()))
            .or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|h| !h.is_empty())
            })
            .unwrap_or_else(|| "localhost".into());
        (uuid::Uuid::new_v4().to_string(), host, Utc::now())
    })
}

/// Records this node as alive (at most every 30 s; never blocks the request).
pub fn heartbeat(state: &AppState) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|l| l.elapsed() < Duration::from_secs(30)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let db = state.db.clone();
    tokio::spawn(async move {
        let _ = record_heartbeat(&db).await;
    });
}

pub async fn record_heartbeat(db: &sqlx::PgPool) -> ApiResult<()> {
    let (uuid, host, started) = node_identity();
    sqlx::query(
        "INSERT INTO ent_nodes (uuid, hostname, version, started_at, last_seen_at) VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (uuid) DO UPDATE SET last_seen_at = now()",
    )
    .bind(uuid)
    .bind(host)
    .bind(env!("CARGO_PKG_VERSION"))
    .bind(started)
    .execute(db)
    .await?;
    Ok(())
}

/// Whether self-service signup is allowed (management console `signup_enabled`, default true).
pub async fn signup_enabled(state: &AppState) -> bool {
    match get_setting(&state.db, "manage_settings").await {
        Ok(Some(v)) => v.get("signup_enabled").and_then(Value::as_bool).unwrap_or(true),
        _ => true,
    }
}

/// `FORGE_REPO_INVITATIONS=1|true`: the default of the `repository_invitations.enabled` setting.
pub fn repo_invitations_env_default() -> bool {
    std::env::var("FORGE_REPO_INVITATIONS").is_ok_and(|v| v == "1" || v == "true")
}

/// Whether adding a collaborator creates an invitation (management console setting
/// `repository_invitations.enabled`, default from `FORGE_REPO_INVITATIONS`).
pub async fn repo_invitations_enabled(db: &sqlx::PgPool) -> bool {
    match get_setting(db, "manage_settings").await {
        Ok(Some(v)) => v.pointer("/repository_invitations/enabled").and_then(Value::as_bool).unwrap_or_else(repo_invitations_env_default),
        _ => repo_invitations_env_default(),
    }
}
