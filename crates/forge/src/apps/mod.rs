//! GitHub Apps and OAuth apps: registration (manifest flow and forge endpoints), keys, installations,
//! installation access tokens and the JSON shapes shared by the `api::apps_*` handlers.
//!
//! * A GitHub App has a bot user (`<slug>[bot]`, kind `Bot`) that acts for its installations,
//!   one or more RSA keys (public half stored, see [`jwt`]), an app webhook (`webhooks.app_id`)
//!   and installations on user/org accounts (or the enterprise) with `all` or `selected`
//!   repositories and a copy of the app's permissions/events.
//! * Installation access tokens (`ghs_`, 1 h) authenticate as the bot user and are confined to
//!   their installation's repositories and permissions by [`scope`].
//! * OAuth apps (kind `oauth`) only have a client id/secret; their device-flow tokens and the
//!   legacy OAuth Authorizations API live in [`oauth`].
pub mod hooks;
pub mod jwt;
pub mod oauth;
pub mod scope;

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Client id of personal (application-less) OAuth authorizations, like GitHub.
pub const PERSONAL_CLIENT_ID: &str = "00000000000000000000";

/// Every permission name GitHub Apps can request (`app-permissions` schema).
pub const PERMISSION_KEYS: &[&str] = &[
    "actions", "administration", "artifact_metadata", "attestations", "checks", "code_quality", "codespaces", "contents",
    "dependabot_secrets", "deployments", "discussions", "environments", "issues", "merge_queues", "metadata", "packages",
    "pages", "pull_requests", "repository_custom_properties", "repository_hooks", "repository_projects",
    "secret_scanning_alerts", "secrets", "security_events", "single_file", "statuses", "vulnerability_alerts", "workflows",
    "custom_properties_for_organizations", "members", "organization_administration", "organization_custom_roles",
    "organization_custom_org_roles", "organization_custom_properties", "organization_copilot_seat_management",
    "organization_copilot_agent_settings", "organization_external_properties_for_repos",
    "organization_announcement_banners", "organization_events", "organization_hooks",
    "organization_personal_access_tokens", "organization_personal_access_token_requests", "organization_plan",
    "organization_projects", "organization_packages", "organization_secrets", "organization_self_hosted_runners",
    "organization_user_blocking", "email_addresses", "followers", "git_ssh_keys", "gpg_keys", "interaction_limits",
    "profile", "starring", "enterprise_administration", "enterprise_custom_properties",
    "enterprise_custom_properties_for_organizations", "enterprise_organization_installations",
    "enterprise_organization_installation_repositories",
];

/// read < write < admin; anything else is 0.
pub fn level_rank(v: Option<&str>) -> u8 {
    match v {
        Some("read") => 1,
        Some("write") => 2,
        Some("admin") => 3,
        _ => 0,
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AppRow {
    pub id: i64,
    pub kind: String,
    pub slug: String,
    pub name: String,
    pub owner_id: i64,
    pub bot_user_id: Option<i64>,
    pub description: Option<String>,
    pub external_url: String,
    pub client_id: String,
    pub client_secret_hash: String,
    pub permissions: Value,
    pub events: Vec<String>,
    pub public: bool,
    pub callback_urls: Vec<String>,
    pub redirect_url: Option<String>,
    pub setup_url: Option<String>,
    pub request_oauth_on_install: bool,
    pub setup_on_update: bool,
    pub created_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstallationRow {
    pub id: i64,
    pub app_id: i64,
    pub target_id: Option<i64>,
    pub target_type: String,
    pub repository_selection: String,
    pub permissions: Value,
    pub events: Vec<String>,
    pub installer_id: Option<i64>,
    pub suspended_at: Option<DateTime<Utc>>,
    pub suspended_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HookAttributes {
    pub url: Option<String>,
    pub active: Option<bool>,
}

/// A GitHub App manifest (also the body of the forge registration endpoint).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Manifest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub hook_attributes: Option<HookAttributes>,
    pub redirect_url: Option<String>,
    pub callback_urls: Option<Vec<String>>,
    pub setup_url: Option<String>,
    pub public: Option<bool>,
    pub default_permissions: Option<Map<String, Value>>,
    pub default_events: Option<Vec<String>>,
    pub request_oauth_on_install: Option<bool>,
    pub setup_on_update: Option<bool>,
}

/// Credentials returned once when an app is created.
pub struct AppSecrets {
    pub client_secret: String,
    pub webhook_secret: Option<String>,
    pub pem: Option<String>,
}

/// GitHub's slug rules: lowercase, runs of other characters become one dash.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n.div_ceil(2)];
    rand::fill(&mut b[..]);
    let mut s = hex::encode(b);
    s.truncate(n);
    s
}

/// Validated permissions map (`metadata: read` is always granted to GitHub Apps).
pub fn validate_permissions(p: Option<&Map<String, Value>>, resource: &str) -> ApiResult<Map<String, Value>> {
    let mut out = Map::new();
    for (k, v) in p.into_iter().flatten() {
        let lv = v.as_str().unwrap_or("");
        if !PERMISSION_KEYS.contains(&k.as_str()) || level_rank(Some(lv)) == 0 {
            let mut e = ApiError::validation(resource, "permissions", "invalid");
            e.errors = Some(json!([{"resource": resource, "field": "permissions", "code": "invalid", "message": format!("Invalid permission: {k}={lv}")}]));
            return Err(e);
        }
        out.insert(k.clone(), json!(lv));
    }
    out.entry("metadata").or_insert(json!("read"));
    Ok(out)
}

fn http_url(u: &str) -> bool {
    url::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
}

// ---------------------------------------------------------------------------------------------
// Lookups

pub async fn app_by_id(db: &PgPool, id: i64) -> ApiResult<AppRow> {
    sqlx::query_as("SELECT * FROM apps WHERE id = $1").bind(id).fetch_optional(db).await?.ok_or_else(ApiError::not_found)
}

pub async fn app_by_slug(db: &PgPool, slug: &str) -> ApiResult<AppRow> {
    sqlx::query_as("SELECT * FROM apps WHERE lower(slug) = lower($1) AND kind = 'app'")
        .bind(slug)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn app_by_client_id(db: &PgPool, client_id: &str) -> ApiResult<AppRow> {
    sqlx::query_as("SELECT * FROM apps WHERE client_id = $1").bind(client_id).fetch_optional(db).await?.ok_or_else(ApiError::not_found)
}

pub async fn installation_by_id(db: &PgPool, id: i64) -> ApiResult<InstallationRow> {
    sqlx::query_as("SELECT * FROM app_installations WHERE id = $1").bind(id).fetch_optional(db).await?.ok_or_else(ApiError::not_found)
}

/// The installation of `app` on account `target_id` (None = the enterprise).
pub async fn installation_for(db: &PgPool, app_id: i64, target_id: Option<i64>) -> ApiResult<Option<InstallationRow>> {
    Ok(sqlx::query_as("SELECT * FROM app_installations WHERE app_id = $1 AND target_id IS NOT DISTINCT FROM $2")
        .bind(app_id)
        .bind(target_id)
        .fetch_optional(db)
        .await?)
}

/// Whether `secret` is the app's client secret (constant-time on the hashes).
pub fn secret_matches(app: &AppRow, secret: &str) -> bool {
    let h = crate::auth::sha256_hex(secret);
    h.len() == app.client_secret_hash.len() && h.bytes().zip(app.client_secret_hash.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// Whether `u` may manage `app` (owner, owner org's owners, site admins).
pub async fn can_manage(db: &PgPool, app: &AppRow, u: &User) -> ApiResult<bool> {
    if u.site_admin || u.id == app.owner_id {
        return Ok(true);
    }
    Ok(crate::orgs::org_role(db, app.owner_id, u.id).await?.as_deref() == Some("admin"))
}

// ---------------------------------------------------------------------------------------------
// Registration

/// Registers a GitHub App owned by `owner` (the actor or an org the actor owns).
pub async fn create_app(s: &AppState, actor: &User, owner: &User, m: Manifest) -> ApiResult<(AppRow, AppSecrets)> {
    if owner.kind == "Organization" && !crate::orgs::is_owner(&s.db, owner, actor).await? {
        return Err(ApiError::forbidden("You must be an organization owner to register apps for it."));
    } else if owner.kind != "Organization" && owner.id != actor.id && !actor.site_admin {
        return Err(ApiError::forbidden("You can only register apps for yourself or an organization you own."));
    }
    let name = m.name.clone().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("Integration", "name", "missing_field"))?;
    if name.chars().count() > 34 {
        let mut e = ApiError::validation("Integration", "name", "invalid");
        e.errors = Some(json!([{"resource": "Integration", "field": "name", "code": "invalid", "message": "Name is too long (maximum is 34 characters)"}]));
        return Err(e);
    }
    let slug = slugify(&name);
    if slug.is_empty() {
        return Err(ApiError::validation("Integration", "name", "invalid"));
    }
    let url = m.url.clone().filter(|u| !u.trim().is_empty()).ok_or_else(|| ApiError::validation("Integration", "url", "missing_field"))?;
    if !http_url(&url) {
        return Err(ApiError::validation("Integration", "url", "invalid"));
    }
    let hook_url = m.hook_attributes.as_ref().and_then(|h| h.url.clone()).filter(|u| !u.is_empty());
    if hook_url.as_deref().is_some_and(|u| !http_url(u)) {
        return Err(ApiError::validation("Integration", "hook_attributes.url", "invalid"));
    }
    for u in m.callback_urls.iter().flatten().chain(m.redirect_url.iter()).chain(m.setup_url.iter()) {
        if !http_url(u) {
            return Err(ApiError::validation("Integration", "callback_urls", "invalid"));
        }
    }
    let permissions = validate_permissions(m.default_permissions.as_ref(), "Integration")?;
    let events: Vec<String> = m.default_events.clone().unwrap_or_default().into_iter().filter(|e| !e.is_empty()).collect();
    let key = tokio::task::spawn_blocking(jwt::generate_key).await.map_err(ApiError::internal)??;
    let client_secret = random_hex(40);
    let webhook_secret = random_hex(40);
    let taken = || {
        let mut e = ApiError::validation("Integration", "name", "already_exists");
        e.errors = Some(json!([{"resource": "Integration", "field": "name", "code": "already_exists", "message": "Name has already been taken"}]));
        e
    };
    let mut tx = s.db.begin().await?;
    let bot: Option<i64> = sqlx::query_scalar("INSERT INTO users (login, kind, name) VALUES ($1, 'Bot', $2) ON CONFLICT DO NOTHING RETURNING id")
        .bind(format!("{slug}[bot]"))
        .bind(&name)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(bot) = bot else { return Err(taken()) };
    let app: Option<AppRow> = sqlx::query_as(
        "INSERT INTO apps (kind, slug, name, owner_id, bot_user_id, description, external_url, client_id, client_secret_hash,
             permissions, events, public, callback_urls, redirect_url, setup_url, request_oauth_on_install, setup_on_update, created_by)
         VALUES ('app', $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(&slug)
    .bind(&name)
    .bind(owner.id)
    .bind(bot)
    .bind(m.description.as_deref().filter(|d| !d.is_empty()))
    .bind(&url)
    .bind(format!("Iv1.{}", random_hex(16)))
    .bind(crate::auth::sha256_hex(&client_secret))
    .bind(Value::Object(permissions))
    .bind(&events)
    .bind(m.public.unwrap_or(false))
    .bind(m.callback_urls.clone().unwrap_or_default())
    .bind(m.redirect_url.as_deref())
    .bind(m.setup_url.as_deref())
    .bind(m.request_oauth_on_install.unwrap_or(false))
    .bind(m.setup_on_update.unwrap_or(false))
    .bind(actor.id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(app) = app else { return Err(taken()) };
    sqlx::query("INSERT INTO app_keys (app_id, public_key, fingerprint) VALUES ($1, $2, $3)")
        .bind(app.id)
        .bind(&key.public_key)
        .bind(&key.fingerprint)
        .execute(&mut *tx)
        .await?;
    let mut cfg = json!({ "content_type": "json", "insecure_ssl": "0" });
    if let Some(u) = &hook_url {
        cfg["url"] = json!(u);
    }
    let active = hook_url.is_some() && m.hook_attributes.as_ref().and_then(|h| h.active).unwrap_or(true);
    sqlx::query("INSERT INTO webhooks (repo_id, org_id, app_id, config, events, active) VALUES (NULL, NULL, $1, $2, $3, $4)")
        .bind(app.id)
        .bind(cfg)
        .bind(&events)
        .bind(active)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let sealed = hooks::seal_secret(s, app.id, &webhook_secret).await?;
    sqlx::query("UPDATE webhooks SET config = config || jsonb_build_object('secret_sealed', $2::text) WHERE app_id = $1")
        .bind(app.id)
        .bind(sealed)
        .execute(&s.db)
        .await?;
    Ok((app, AppSecrets { client_secret, webhook_secret: Some(webhook_secret), pem: Some(key.pem) }))
}

/// Registers an OAuth app (client id and secret only).
pub async fn create_oauth_app(s: &AppState, actor: &User, owner: &User, name: &str, url: &str, callback: Option<&str>) -> ApiResult<(AppRow, AppSecrets)> {
    if owner.kind == "Organization" && !crate::orgs::is_owner(&s.db, owner, actor).await? {
        return Err(ApiError::forbidden("You must be an organization owner to register apps for it."));
    } else if owner.kind != "Organization" && owner.id != actor.id && !actor.site_admin {
        return Err(ApiError::forbidden("You can only register apps for yourself or an organization you own."));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::validation("OauthApplication", "name", "missing_field"));
    }
    if !http_url(url) || callback.is_some_and(|c| !http_url(c)) {
        return Err(ApiError::validation("OauthApplication", "url", "invalid"));
    }
    let client_secret = random_hex(40);
    let app: AppRow = sqlx::query_as(
        "INSERT INTO apps (kind, slug, name, owner_id, external_url, client_id, client_secret_hash, callback_urls, created_by)
         VALUES ('oauth', $1, $2, $3, $4, $5, $6, $7, $8) RETURNING *",
    )
    .bind(format!("oauth-{}", random_hex(12)))
    .bind(name)
    .bind(owner.id)
    .bind(url)
    .bind(random_hex(20))
    .bind(crate::auth::sha256_hex(&client_secret))
    .bind(callback.map(|c| vec![c.to_string()]).unwrap_or_default())
    .bind(actor.id)
    .fetch_one(&s.db)
    .await?;
    Ok((app, AppSecrets { client_secret, webhook_secret: None, pem: None }))
}

/// Adds a new private key to a GitHub App; returns (key id, PEM).
pub async fn add_key(s: &AppState, app: &AppRow) -> ApiResult<(i64, String)> {
    let key = tokio::task::spawn_blocking(jwt::generate_key).await.map_err(ApiError::internal)??;
    let id: i64 = sqlx::query_scalar("INSERT INTO app_keys (app_id, public_key, fingerprint) VALUES ($1, $2, $3) RETURNING id")
        .bind(app.id)
        .bind(&key.public_key)
        .bind(&key.fingerprint)
        .fetch_one(&s.db)
        .await?;
    Ok((id, key.pem))
}

/// Deletes an app with its bot user, installations, tokens and webhook.
pub async fn delete_app(s: &AppState, app: &AppRow) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM tokens WHERE id IN (SELECT token_id FROM oauth_authorizations WHERE app_id = $1)").bind(app.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM apps WHERE id = $1").bind(app.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM actions_public_keys WHERE owner_kind = 'app' AND owner_id = $1").bind(app.id).execute(&mut *tx).await?;
    if let Some(b) = app.bot_user_id {
        sqlx::query("DELETE FROM users WHERE id = $1 AND kind = 'Bot'").bind(b).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// JSON

/// `enterprise` object for the single forge enterprise.
pub fn enterprise_json(s: &AppState) -> Value {
    let slug = enterprise_slug();
    let base = s.base_url();
    json!({
        "id": 1,
        "node_id": crate::ids::encode("Enterprise", 1),
        "name": slug,
        "slug": slug,
        "description": null,
        "website_url": null,
        "html_url": format!("{base}/enterprises/{slug}"),
        "created_at": null,
        "updated_at": null,
        "avatar_url": format!("{base}/avatars/{slug}"),
    })
}

/// The enterprise slug (`FORGE_ENTERPRISE_SLUG`, default `forge`; see `crate::enterprise`).
pub fn enterprise_slug() -> String {
    crate::enterprise::slug()
}

/// `integration` schema.
pub fn integration_value(s: &AppState, app: &AppRow, owner: &User, installations: i64) -> Value {
    let api = s.api_url();
    let base = s.base_url();
    json!({
        "id": app.id,
        "slug": app.slug,
        "node_id": crate::ids::encode("Integration", app.id),
        "client_id": app.client_id,
        "owner": crate::json::user::simple(&api, base, owner),
        "name": app.name,
        "description": app.description,
        "external_url": app.external_url,
        "html_url": format!("{base}/apps/{}", app.slug),
        "created_at": crate::json::ts(app.created_at),
        "updated_at": crate::json::ts(app.updated_at),
        "permissions": app.permissions,
        "events": app.events,
        "installations_count": installations,
    })
}

pub async fn integration_json(s: &AppState, app: &AppRow) -> ApiResult<Value> {
    let owner = db::user_by_id(&s.db, app.owner_id).await?;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_installations WHERE app_id = $1").bind(app.id).fetch_one(&s.db).await?;
    Ok(integration_value(s, app, &owner, n))
}

/// `integration` JSON for an app id (check runs/suites); None for unknown ids.
pub async fn integration_json_by_id(s: &AppState, id: i64) -> Option<Value> {
    let app = app_by_id(&s.db, id).await.ok()?;
    integration_json(s, &app).await.ok()
}

/// `installation` schema.
pub fn installation_value(s: &AppState, inst: &InstallationRow, app: &AppRow, account: Option<&User>, suspender: Option<&User>) -> Value {
    let api = s.api_url();
    let base = s.base_url();
    let (account_json, html) = match (inst.target_type.as_str(), account) {
        ("Enterprise", _) | (_, None) => (enterprise_json(s), format!("{base}/enterprises/{}/settings/installations/{}", enterprise_slug(), inst.id)),
        ("Organization", Some(a)) => (crate::json::user::simple(&api, base, a), format!("{base}/organizations/{}/settings/installations/{}", a.login, inst.id)),
        (_, Some(a)) => (crate::json::user::simple(&api, base, a), format!("{base}/settings/installations/{}", inst.id)),
    };
    json!({
        "id": inst.id,
        "account": account_json,
        "repository_selection": inst.repository_selection,
        "access_tokens_url": format!("{api}/app/installations/{}/access_tokens", inst.id),
        "repositories_url": format!("{api}/installation/repositories"),
        "html_url": html,
        "app_id": app.id,
        "client_id": app.client_id,
        "target_id": inst.target_id.unwrap_or(1),
        "target_type": inst.target_type,
        "permissions": inst.permissions,
        "events": inst.events,
        "created_at": crate::json::ts(inst.created_at),
        "updated_at": crate::json::ts(inst.updated_at),
        "single_file_name": null,
        "has_multiple_single_files": false,
        "single_file_paths": [],
        "app_slug": app.slug,
        "suspended_by": suspender.map(|u| crate::json::user::simple(&api, base, u)),
        "suspended_at": crate::json::ts_opt(inst.suspended_at),
        "contact_email": null,
    })
}

pub async fn installation_json(s: &AppState, inst: &InstallationRow) -> ApiResult<Value> {
    let app = app_by_id(&s.db, inst.app_id).await?;
    installation_json_with(s, inst, &app).await
}

pub async fn installation_json_with(s: &AppState, inst: &InstallationRow, app: &AppRow) -> ApiResult<Value> {
    let account = match inst.target_id {
        Some(id) => db::user_by_id(&s.db, id).await.ok(),
        None => None,
    };
    let suspender = match inst.suspended_by {
        Some(id) => db::user_by_id(&s.db, id).await.ok(),
        None => None,
    };
    Ok(installation_value(s, inst, app, account.as_ref(), suspender.as_ref()))
}

/// Renders many installations (batch-loads apps and accounts).
pub async fn installations_json(s: &AppState, rows: &[InstallationRow]) -> ApiResult<Vec<Value>> {
    let app_ids: Vec<i64> = rows.iter().map(|r| r.app_id).collect();
    let apps: Vec<AppRow> = sqlx::query_as("SELECT * FROM apps WHERE id = ANY($1)").bind(&app_ids).fetch_all(&s.db).await?;
    let user_ids: Vec<i64> = rows.iter().flat_map(|r| [r.target_id, r.suspended_by]).flatten().collect();
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&user_ids).fetch_all(&s.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(app) = apps.iter().find(|a| a.id == r.app_id) else { continue };
        let account = r.target_id.and_then(|id| users.iter().find(|u| u.id == id));
        let susp = r.suspended_by.and_then(|id| users.iter().find(|u| u.id == id));
        out.push(installation_value(s, r, app, account, susp));
    }
    Ok(out)
}

/// `{id, node_id, name, full_name, private}` as in installation webhook payloads.
pub fn repo_mini(ctx: &RepoCtx) -> Value {
    json!({
        "id": ctx.repo.id,
        "node_id": crate::ids::encode("Repository", ctx.repo.id),
        "name": ctx.repo.name,
        "full_name": ctx.full_name(),
        "private": ctx.repo.private,
    })
}

// ---------------------------------------------------------------------------------------------
// Installations

/// Repository ids an installation can access, in id order.
pub async fn installation_repo_ids(db: &PgPool, inst: &InstallationRow) -> ApiResult<Vec<i64>> {
    let Some(target) = inst.target_id else { return Ok(vec![]) };
    Ok(if inst.repository_selection == "all" {
        sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1 ORDER BY id").bind(target).fetch_all(db).await?
    } else {
        sqlx::query_scalar(
            "SELECT r.repo_id FROM app_installation_repos r JOIN repositories p ON p.id = r.repo_id
              WHERE r.installation_id = $1 AND p.owner_id = $2 ORDER BY r.repo_id",
        )
        .bind(inst.id)
        .bind(target)
        .fetch_all(db)
        .await?
    })
}

/// Loads repositories by id as `RepoCtx` with `perm` (batch, keeps `ids` order).
pub async fn repos_by_ids(db: &PgPool, ids: &[i64], perm: Perm) -> ApiResult<Vec<RepoCtx>> {
    let repos: Vec<db::Repo> = sqlx::query_as("SELECT * FROM repositories WHERE id = ANY($1)").bind(ids).fetch_all(db).await?;
    let owner_ids: Vec<i64> = repos.iter().map(|r| r.owner_id).collect();
    let owners: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&owner_ids).fetch_all(db).await?;
    Ok(ids
        .iter()
        .filter_map(|id| repos.iter().find(|r| r.id == *id))
        .filter_map(|r| owners.iter().find(|o| o.id == r.owner_id).map(|o| RepoCtx { repo: r.clone(), owner: o.clone(), perm }))
        .collect())
}

/// Resolves repository names (`name` or `owner/name`) of the account `target_id`.
pub async fn resolve_repo_names(db: &PgPool, target_id: i64, names: &[String]) -> ApiResult<Result<Vec<i64>, String>> {
    let mut ids = Vec::new();
    for n in names {
        let name = n.rsplit('/').next().unwrap_or(n);
        let id: Option<i64> = sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1 AND lower(name) = lower($2)")
            .bind(target_id)
            .bind(name)
            .fetch_optional(db)
            .await?;
        match id {
            Some(id) => ids.push(id),
            None => return Ok(Err(n.clone())),
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(Ok(ids))
}

/// Where an app is being installed.
pub enum InstallTarget {
    Account(User),
    Enterprise,
}

/// Installs `app` on `target`. `repo_ids` applies to `selected`.
/// Returns the installation and whether it was created (false = an existing one was updated).
pub async fn install(
    s: &AppState,
    actor: &User,
    app: &AppRow,
    target: &InstallTarget,
    selection: &str,
    repo_ids: &[i64],
    update_existing: bool,
) -> ApiResult<(InstallationRow, bool)> {
    if app.kind != "app" {
        return Err(ApiError::not_found());
    }
    let (target_id, target_type) = match target {
        InstallTarget::Enterprise => {
            if !actor.site_admin {
                return Err(ApiError::forbidden("You must be an enterprise owner to install apps on the enterprise."));
            }
            (None, "Enterprise")
        }
        InstallTarget::Account(a) if a.kind == "Organization" => {
            crate::orgs::require_owner(s, a, actor).await?;
            (Some(a.id), "Organization")
        }
        InstallTarget::Account(a) => {
            if a.id != actor.id && !actor.site_admin {
                return Err(ApiError::forbidden("You can only install apps on your own account."));
            }
            (Some(a.id), "User")
        }
    };
    if !app.public && target_id != Some(app.owner_id) && !actor.site_admin {
        return Err(ApiError::forbidden("This app is private: it can only be installed on the account that owns it."));
    }
    if !matches!(selection, "all" | "selected") {
        return Err(ApiError::validation("Installation", "repository_selection", "invalid"));
    }
    if let Some(t) = target_id {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE owner_id = $1 AND id = ANY($2)")
            .bind(t)
            .bind(repo_ids)
            .fetch_one(&s.db)
            .await?;
        if n as usize != repo_ids.len() {
            return Err(ApiError::validation("Installation", "repositories", "invalid"));
        }
    }
    if let Some(existing) = installation_for(&s.db, app.id, target_id).await? {
        if !update_existing {
            let mut e = ApiError::validation("Installation", "account", "already_exists");
            e.errors = Some(json!([{"resource": "Installation", "field": "account", "code": "already_exists", "message": "The app is already installed on this account."}]));
            return Err(e);
        }
        let inst = set_repositories(s, actor, app, &existing, selection, repo_ids).await?;
        return Ok((inst, false));
    }
    let mut tx = s.db.begin().await?;
    let inst: InstallationRow = sqlx::query_as(
        "INSERT INTO app_installations (app_id, target_id, target_type, repository_selection, permissions, events, installer_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *",
    )
    .bind(app.id)
    .bind(target_id)
    .bind(target_type)
    .bind(selection)
    .bind(&app.permissions)
    .bind(&app.events)
    .bind(actor.id)
    .fetch_one(&mut *tx)
    .await?;
    if selection == "selected" {
        sqlx::query("INSERT INTO app_installation_repos (installation_id, repo_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(inst.id)
            .bind(repo_ids)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    let repos = repos_by_ids(&s.db, &installation_repo_ids(&s.db, &inst).await?, Perm::Admin).await?;
    let mut extra = Map::new();
    extra.insert("repositories".into(), Value::Array(repos.iter().map(repo_mini).collect()));
    extra.insert("requester".into(), Value::Null);
    hooks::emit_installation(s, app, &inst, "installation", "created", actor, extra).await;
    Ok((inst, true))
}

/// Changes the repository selection of an installation and emits `installation_repositories`.
pub async fn set_repositories(s: &AppState, actor: &User, app: &AppRow, inst: &InstallationRow, selection: &str, repo_ids: &[i64]) -> ApiResult<InstallationRow> {
    let before: HashSet<i64> = installation_repo_ids(&s.db, inst).await?.into_iter().collect();
    let mut tx = s.db.begin().await?;
    let updated: InstallationRow = sqlx::query_as("UPDATE app_installations SET repository_selection = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(inst.id)
        .bind(selection)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM app_installation_repos WHERE installation_id = $1").bind(inst.id).execute(&mut *tx).await?;
    if selection == "selected" {
        sqlx::query("INSERT INTO app_installation_repos (installation_id, repo_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(inst.id)
            .bind(repo_ids)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    let after: HashSet<i64> = installation_repo_ids(&s.db, &updated).await?.into_iter().collect();
    emit_repo_changes(s, actor, app, &updated, &before, &after).await?;
    Ok(updated)
}

/// Adds (or removes) repositories of a `selected` installation.
pub async fn change_repositories(s: &AppState, actor: &User, app: &AppRow, inst: &InstallationRow, add: &[i64], remove: &[i64]) -> ApiResult<InstallationRow> {
    if inst.repository_selection != "selected" {
        return Err(ApiError::unprocessable("The installation has access to all repositories; switch it to selected repositories first."));
    }
    let before: HashSet<i64> = installation_repo_ids(&s.db, inst).await?.into_iter().collect();
    if let Some(t) = inst.target_id {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE owner_id = $1 AND id = ANY($2)")
            .bind(t)
            .bind(add)
            .fetch_one(&s.db)
            .await?;
        if n as usize != add.len() {
            return Err(ApiError::not_found());
        }
    }
    sqlx::query("INSERT INTO app_installation_repos (installation_id, repo_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(inst.id)
        .bind(add)
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM app_installation_repos WHERE installation_id = $1 AND repo_id = ANY($2)")
        .bind(inst.id)
        .bind(remove)
        .execute(&s.db)
        .await?;
    let updated: InstallationRow = sqlx::query_as("UPDATE app_installations SET updated_at = now() WHERE id = $1 RETURNING *")
        .bind(inst.id)
        .fetch_one(&s.db)
        .await?;
    let after: HashSet<i64> = installation_repo_ids(&s.db, &updated).await?.into_iter().collect();
    emit_repo_changes(s, actor, app, &updated, &before, &after).await?;
    Ok(updated)
}

async fn emit_repo_changes(s: &AppState, actor: &User, app: &AppRow, inst: &InstallationRow, before: &HashSet<i64>, after: &HashSet<i64>) -> ApiResult<()> {
    let mut added: Vec<i64> = after.difference(before).copied().collect();
    let mut removed: Vec<i64> = before.difference(after).copied().collect();
    if added.is_empty() && removed.is_empty() {
        return Ok(());
    }
    added.sort_unstable();
    removed.sort_unstable();
    let added = repos_by_ids(&s.db, &added, Perm::Admin).await?;
    let removed = repos_by_ids(&s.db, &removed, Perm::Admin).await?;
    let mut extra = Map::new();
    extra.insert("repository_selection".into(), json!(inst.repository_selection));
    extra.insert("repositories_added".into(), Value::Array(added.iter().map(repo_mini).collect()));
    extra.insert("repositories_removed".into(), Value::Array(removed.iter().map(repo_mini).collect()));
    extra.insert("requester".into(), Value::Null);
    let action = if added.is_empty() { "removed" } else { "added" };
    hooks::emit_installation(s, app, inst, "installation_repositories", action, actor, extra).await;
    Ok(())
}

/// Uninstalls (deletes the installation and its tokens).
pub async fn uninstall(s: &AppState, actor: &User, app: &AppRow, inst: &InstallationRow) -> ApiResult<()> {
    // Render before deleting so the payload carries the installation.
    let payload_inst = inst.clone();
    sqlx::query("DELETE FROM app_installations WHERE id = $1").bind(inst.id).execute(&s.db).await?;
    hooks::emit_installation(s, app, &payload_inst, "installation", "deleted", actor, Map::new()).await;
    Ok(())
}

pub async fn set_suspended(s: &AppState, actor: &User, app: &AppRow, inst: &InstallationRow, suspend: bool) -> ApiResult<InstallationRow> {
    let row: InstallationRow = if suspend {
        sqlx::query_as("UPDATE app_installations SET suspended_at = COALESCE(suspended_at, now()), suspended_by = COALESCE(suspended_by, $2), updated_at = now() WHERE id = $1 RETURNING *")
            .bind(inst.id)
            .bind(actor.id)
            .fetch_one(&s.db)
            .await?
    } else {
        sqlx::query_as("UPDATE app_installations SET suspended_at = NULL, suspended_by = NULL, updated_at = now() WHERE id = $1 RETURNING *")
            .bind(inst.id)
            .fetch_one(&s.db)
            .await?
    };
    if suspend != inst.suspended_at.is_some() {
        hooks::emit_installation(s, app, &row, "installation", if suspend { "suspend" } else { "unsuspend" }, actor, Map::new()).await;
    }
    Ok(row)
}

// ---------------------------------------------------------------------------------------------
// Installation access tokens

#[derive(Debug, Default, Deserialize)]
pub struct TokenRequest {
    pub repositories: Option<Vec<String>>,
    pub repository_ids: Option<Vec<i64>>,
    pub permissions: Option<Map<String, Value>>,
}

/// Lifetime of installation access tokens.
pub const INSTALLATION_TOKEN_SECS: i64 = 3600;

pub fn new_installation_token() -> String {
    format!("ghs_{}", crate::auth::random_string(36))
}

/// Checks that `requested` is a subset of `granted`; returns the effective permissions.
pub fn narrow_permissions(granted: &Value, requested: Option<&Map<String, Value>>) -> ApiResult<Value> {
    let Some(req) = requested.filter(|r| !r.is_empty()) else { return Ok(granted.clone()) };
    let mut out = Map::new();
    for (k, v) in req {
        let want = level_rank(v.as_str());
        let have = level_rank(granted.get(k).and_then(Value::as_str));
        if want == 0 || !PERMISSION_KEYS.contains(&k.as_str()) {
            return Err(ApiError::unprocessable(format!("The permissions requested are not valid: {k}")));
        }
        if want > have {
            return Err(ApiError::unprocessable("The permissions requested are not granted to this installation."));
        }
        out.insert(k.clone(), v.clone());
    }
    Ok(Value::Object(out))
}

/// Creates an installation access token (`installation-token` JSON).
pub async fn create_installation_token(s: &AppState, app: &AppRow, inst: &InstallationRow, req: TokenRequest) -> ApiResult<Value> {
    if inst.suspended_at.is_some() {
        return Err(ApiError::forbidden("This installation has been suspended"));
    }
    let perms = narrow_permissions(&inst.permissions, req.permissions.as_ref())?;
    let accessible: Vec<i64> = installation_repo_ids(&s.db, inst).await?;
    let names = req.repositories.unwrap_or_default();
    let mut ids = req.repository_ids.unwrap_or_default();
    if names.len() + ids.len() > 500 {
        return Err(ApiError::unprocessable("Too many repositories: at most 500 can be requested."));
    }
    let not_accessible = || ApiError::unprocessable("There is at least one repository that does not exist or is not accessible to the parent installation.");
    if !names.is_empty() {
        let Some(t) = inst.target_id else { return Err(not_accessible()) };
        match resolve_repo_names(&s.db, t, &names).await? {
            Ok(found) => ids.extend(found),
            Err(_) => return Err(not_accessible()),
        }
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.iter().any(|id| !accessible.contains(id)) {
        return Err(not_accessible());
    }
    let restricted = !ids.is_empty();
    let token = new_installation_token();
    let row: (DateTime<Utc>,) = sqlx::query_as(
        "INSERT INTO app_installation_tokens (installation_id, token_hash, repository_ids, permissions, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5)) RETURNING expires_at",
    )
    .bind(inst.id)
    .bind(crate::auth::sha256_hex(&token))
    .bind(if restricted { Some(ids.clone()) } else { None })
    .bind(&perms)
    .bind(INSTALLATION_TOKEN_SECS as f64)
    .fetch_one(&s.db)
    .await?;
    // Opportunistic cleanup of long-expired tokens.
    if rand::random::<u8>() < 8 {
        let _ = sqlx::query("DELETE FROM app_installation_tokens WHERE expires_at < now() - interval '1 day'").execute(&s.db).await;
    }
    let mut v = json!({
        "token": token,
        "expires_at": crate::json::ts(row.0),
        "permissions": perms,
        "repository_selection": if restricted { "selected" } else { inst.repository_selection.as_str() },
    });
    if restricted {
        let repos = repos_by_ids(&s.db, &ids, Perm::Admin).await?;
        let api = s.api_url();
        v["repositories"] = Value::Array(repos.iter().map(|r| crate::json::repo::repo(&api, s.base_url(), r)).collect());
    }
    let _ = app;
    Ok(v)
}

/// GitHub Apps with an active installation covering `repo`.
pub async fn apps_installed_on(db: &PgPool, repo: &db::Repo) -> ApiResult<Vec<AppRow>> {
    Ok(sqlx::query_as(
        "SELECT a.* FROM apps a JOIN app_installations i ON i.app_id = a.id
          WHERE a.kind = 'app' AND i.target_id = $1 AND i.suspended_at IS NULL
            AND (i.repository_selection = 'all' OR EXISTS (SELECT 1 FROM app_installation_repos r WHERE r.installation_id = i.id AND r.repo_id = $2))
          ORDER BY a.id",
    )
    .bind(repo.owner_id)
    .bind(repo.id)
    .fetch_all(db)
    .await?)
}

/// Installations visible to `u`: on their own account and on orgs they belong to.
pub async fn installations_for_user(db: &PgPool, u: &User) -> ApiResult<Vec<InstallationRow>> {
    Ok(sqlx::query_as(
        "SELECT i.* FROM app_installations i
          WHERE (i.target_id = $1 OR i.target_id IN (SELECT org_id FROM org_members WHERE user_id = $1))
            AND i.app_id NOT IN (SELECT id FROM apps WHERE slug = 'github-actions')
          ORDER BY i.id",
    )
    .bind(u.id)
    .fetch_all(db)
    .await?)
}
