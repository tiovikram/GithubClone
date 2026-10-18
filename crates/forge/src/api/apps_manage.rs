//! Forge-only endpoints behind the web UI's GitHub App / OAuth app pages (nested under
//! `/api/forge`; on GitHub these are web UI flows with no REST equivalent):
//!
//! * `GET|PATCH /apps/{slug}`: an app as its managers see it (callback/setup URLs, webhook,
//!   keys, visibility) and edits to it (`name`, `description`, `url`, `callback_urls`,
//!   `setup_url`, `redirect_url`, `hook_attributes {url, active}`, `webhook_secret`,
//!   `default_permissions`, `default_events`, `public`, `request_oauth_on_install`,
//!   `setup_on_update`). Permission changes apply to installations once their owner accepts
//!   them (`POST /installations/{id}/permissions`), like GitHub.
//! * `GET /apps/{slug}/keys`, `DELETE /apps/{slug}/keys/{id}` (the last key can't be deleted).
//! * `POST /apps/{slug}/client-secret`: a new client secret, shown once; the old one stops
//!   working (forge keeps one secret per app).
//! * `GET|PATCH|DELETE /installations/{id}` (`repository_selection`, `repository_ids`),
//!   `PUT|DELETE /installations/{id}/suspended`, `POST /installations/{id}/permissions`:
//!   installation settings for the account's owner (the user, or an owner of the org).
//! * `GET /authorizations` (`kind=app|oauth`), `DELETE /authorizations/{grant_id}`: the
//!   applications the caller authorized (Settings → Applications → Authorized ... Apps).
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::no_content;
use crate::apps::{self, AppRow, InstallationRow};
use crate::auth::AuthUser;
use crate::db::{self, Perm, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

fn require_human(u: &User) -> ApiResult<()> {
    if u.kind != "User" {
        return Err(ApiError::forbidden(apps::scope::NOT_ACCESSIBLE));
    }
    Ok(())
}

/// An app (either kind) the caller manages; 404 otherwise.
pub async fn managed_app(s: &AppState, u: &User, slug: &str) -> ApiResult<AppRow> {
    require_human(u)?;
    let app: AppRow = sqlx::query_as("SELECT * FROM apps WHERE lower(slug) = lower($1)")
        .bind(slug)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !apps::can_manage(&s.db, &app, u).await? {
        return Err(ApiError::not_found());
    }
    Ok(app)
}

async fn keys_json(s: &AppState, app_id: i64) -> ApiResult<Value> {
    let rows: Vec<(i64, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as("SELECT id, fingerprint, created_at FROM app_keys WHERE app_id = $1 ORDER BY id")
        .bind(app_id)
        .fetch_all(&s.db)
        .await?;
    Ok(Value::Array(rows.into_iter().map(|(id, fp, at)| json!({ "id": id, "fingerprint": fp, "created_at": crate::json::ts(at) })).collect()))
}

async fn managed_json(s: &AppState, app: &AppRow) -> ApiResult<Value> {
    let owner = db::user_by_id(&s.db, app.owner_id).await?;
    let api = s.api_url();
    let mut v = if app.kind == "app" {
        apps::integration_json(s, app).await?
    } else {
        json!({
            "id": app.id,
            "slug": app.slug,
            "name": app.name,
            "description": app.description,
            "external_url": app.external_url,
            "client_id": app.client_id,
            "owner": crate::json::user::simple(&api, s.base_url(), &owner),
            "created_at": crate::json::ts(app.created_at),
            "updated_at": crate::json::ts(app.updated_at),
        })
    };
    v["kind"] = json!(app.kind);
    v["public"] = json!(app.public);
    v["callback_urls"] = json!(app.callback_urls);
    v["setup_url"] = json!(app.setup_url);
    v["redirect_url"] = json!(app.redirect_url);
    v["request_oauth_on_install"] = json!(app.request_oauth_on_install);
    v["setup_on_update"] = json!(app.setup_on_update);
    if app.kind == "app" {
        let hook = apps::hooks::app_hook(s, app.id).await?;
        v["webhook"] = match hook {
            Some(h) => json!({
                "url": h.config.get("url").and_then(Value::as_str).unwrap_or(""),
                "active": h.active,
                "has_secret": h.config.get("secret_sealed").and_then(Value::as_str).is_some_and(|x| !x.is_empty()),
            }),
            None => json!({ "url": "", "active": false, "has_secret": false }),
        };
        v["keys"] = keys_json(s, app.id).await?;
        let bot = match app.bot_user_id {
            Some(b) => db::user_by_id(&s.db, b).await.ok().map(|u| u.login),
            None => None,
        };
        v["bot_login"] = json!(bot);
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_grants WHERE app_id = $1").bind(app.id).fetch_one(&s.db).await?;
    v["users_count"] = json!(n);
    Ok(v)
}

pub async fn get_app(State(s): State<AppState>, AuthUser(u): AuthUser, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let app = managed_app(&s, &u, &slug).await?;
    Ok(Json(managed_json(&s, &app).await?))
}

#[derive(Deserialize, Default)]
pub struct UpdateApp {
    name: Option<String>,
    description: Option<String>,
    url: Option<String>,
    callback_urls: Option<Vec<String>>,
    setup_url: Option<String>,
    redirect_url: Option<String>,
    hook_attributes: Option<apps::HookAttributes>,
    webhook_secret: Option<String>,
    default_permissions: Option<Map<String, Value>>,
    default_events: Option<Vec<String>>,
    public: Option<bool>,
    request_oauth_on_install: Option<bool>,
    setup_on_update: Option<bool>,
}

fn http_url(u: &str) -> bool {
    url::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
}

fn invalid(resource: &str, field: &str, msg: &str) -> ApiError {
    let mut e = ApiError::validation(resource, field, "invalid");
    e.errors = Some(json!([{ "resource": resource, "field": field, "code": "invalid", "message": msg }]));
    e
}

pub async fn update_app(State(s): State<AppState>, AuthUser(u): AuthUser, Path(slug): Path<String>, Json(b): Json<UpdateApp>) -> ApiResult<Json<Value>> {
    let app = managed_app(&s, &u, &slug).await?;
    let resource = if app.kind == "app" { "Integration" } else { "OauthApplication" };
    let name = match b.name.as_deref().map(str::trim) {
        Some("") => return Err(ApiError::validation(resource, "name", "missing_field")),
        Some(n) if app.kind == "app" && n.chars().count() > 34 => return Err(invalid(resource, "name", "Name is too long (maximum is 34 characters)")),
        Some(n) => Some(n.to_string()),
        None => None,
    };
    if let Some(url) = &b.url {
        if !http_url(url) {
            return Err(invalid(resource, "url", "Homepage URL must be an http(s) URL"));
        }
    }
    let opt_url = |v: &Option<String>, field: &str| -> ApiResult<Option<Option<String>>> {
        match v.as_deref().map(str::trim) {
            None => Ok(None),
            Some("") => Ok(Some(None)),
            Some(x) if http_url(x) => Ok(Some(Some(x.to_string()))),
            Some(_) => Err(invalid(resource, field, &format!("{field} must be an http(s) URL"))),
        }
    };
    let setup_url = opt_url(&b.setup_url, "setup_url")?;
    let redirect_url = opt_url(&b.redirect_url, "redirect_url")?;
    let callbacks = match &b.callback_urls {
        Some(list) => {
            let list: Vec<String> = list.iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
            if list.iter().any(|c| !http_url(c)) {
                return Err(invalid(resource, "callback_urls", "Callback URLs must be http(s) URLs"));
            }
            Some(list)
        }
        None => None,
    };
    let permissions = match &b.default_permissions {
        Some(p) if app.kind == "app" => Some(Value::Object(apps::validate_permissions(Some(p), resource)?)),
        _ => None,
    };
    let events = b.default_events.as_ref().filter(|_| app.kind == "app").map(|e| e.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>());
    if b.public == Some(false) && app.public {
        let elsewhere: i64 = sqlx::query_scalar("SELECT count(*) FROM app_installations WHERE app_id = $1 AND target_id IS DISTINCT FROM $2")
            .bind(app.id)
            .bind(app.owner_id)
            .fetch_one(&s.db)
            .await?;
        if elsewhere > 0 {
            return Err(ApiError::unprocessable("This app is installed on other accounts: uninstall it there before making it private."));
        }
    }
    let updated: AppRow = sqlx::query_as(
        "UPDATE apps SET name = COALESCE($2, name), description = CASE WHEN $3::bool THEN $4 ELSE description END,
             external_url = COALESCE($5, external_url), callback_urls = COALESCE($6, callback_urls),
             setup_url = CASE WHEN $7::bool THEN $8 ELSE setup_url END, redirect_url = CASE WHEN $9::bool THEN $10 ELSE redirect_url END,
             permissions = COALESCE($11, permissions), events = COALESCE($12, events), public = COALESCE($13, public),
             request_oauth_on_install = COALESCE($14, request_oauth_on_install), setup_on_update = COALESCE($15, setup_on_update),
             updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(app.id)
    .bind(&name)
    .bind(b.description.is_some())
    .bind(b.description.as_deref().map(str::trim).filter(|d| !d.is_empty()))
    .bind(&b.url)
    .bind(&callbacks)
    .bind(setup_url.is_some())
    .bind(setup_url.flatten())
    .bind(redirect_url.is_some())
    .bind(redirect_url.flatten())
    .bind(&permissions)
    .bind(&events)
    .bind(b.public)
    .bind(b.request_oauth_on_install)
    .bind(b.setup_on_update)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            let mut e = ApiError::validation(resource, "name", "already_exists");
            e.errors = Some(json!([{"resource": resource, "field": "name", "code": "already_exists", "message": "Name has already been taken"}]));
            e
        }
        e => e.into(),
    })?;
    if name.is_some() {
        if let Some(bot) = updated.bot_user_id {
            sqlx::query("UPDATE users SET name = $2 WHERE id = $1").bind(bot).bind(&updated.name).execute(&s.db).await?;
        }
    }
    if app.kind == "app" && (b.hook_attributes.is_some() || b.webhook_secret.is_some() || events.is_some()) {
        if let Some(h) = apps::hooks::app_hook(&s, app.id).await? {
            let mut cfg = h.config.as_object().cloned().unwrap_or_default();
            let mut active = h.active;
            if let Some(ha) = &b.hook_attributes {
                match ha.url.as_deref().map(str::trim) {
                    Some("") => {
                        cfg.remove("url");
                        active = false;
                    }
                    Some(x) if http_url(x) => {
                        cfg.insert("url".into(), json!(x));
                        active = ha.active.unwrap_or(true);
                    }
                    Some(_) => return Err(invalid(resource, "hook_attributes.url", "Webhook URL must be an http(s) URL")),
                    None => {
                        if let Some(a) = ha.active {
                            active = a && cfg.get("url").and_then(Value::as_str).is_some_and(|u| !u.is_empty());
                        }
                    }
                }
            }
            if let Some(sec) = &b.webhook_secret {
                cfg.remove("secret");
                if sec.is_empty() {
                    cfg.remove("secret_sealed");
                } else {
                    cfg.insert("secret_sealed".into(), json!(apps::hooks::seal_secret(&s, app.id, sec).await?));
                }
            }
            sqlx::query("UPDATE webhooks SET config = $2, active = $3, events = COALESCE($4, events), updated_at = now() WHERE id = $1")
                .bind(h.id)
                .bind(Value::Object(cfg))
                .bind(active)
                .bind(&events)
                .execute(&s.db)
                .await?;
        }
    }
    Ok(Json(managed_json(&s, &updated).await?))
}

pub async fn list_keys(State(s): State<AppState>, AuthUser(u): AuthUser, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let app = managed_app(&s, &u, &slug).await?;
    Ok(Json(keys_json(&s, app.id).await?))
}

pub async fn delete_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path((slug, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let app = managed_app(&s, &u, &slug).await?;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM app_keys WHERE app_id = $1").bind(app.id).fetch_one(&s.db).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM app_keys WHERE app_id = $1 AND id = $2)").bind(app.id).bind(id).fetch_one(&s.db).await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    if n <= 1 {
        return Err(ApiError::unprocessable("An app must have at least one private key: generate a new key before deleting this one."));
    }
    sqlx::query("DELETE FROM app_keys WHERE app_id = $1 AND id = $2").bind(app.id).bind(id).execute(&s.db).await?;
    Ok(no_content())
}

pub async fn new_client_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path(slug): Path<String>) -> ApiResult<Response> {
    let app = managed_app(&s, &u, &slug).await?;
    let mut b = [0u8; 20];
    rand::fill(&mut b[..]);
    let secret = hex::encode(b);
    sqlx::query("UPDATE apps SET client_secret_hash = $2, updated_at = now() WHERE id = $1")
        .bind(app.id)
        .bind(crate::auth::sha256_hex(&secret))
        .execute(&s.db)
        .await?;
    Ok(super::created(json!({ "client_secret": secret, "client_id": app.client_id })))
}

// ---------------------------------------------------------------------------------------------
// Installations (account side)

/// The installation, its app, and whether `u` can manage it (the account itself, an owner of the
/// org, a site admin). Members of an org may look; others get 404.
async fn installation(s: &AppState, u: &User, id: i64) -> ApiResult<(InstallationRow, AppRow, User, bool)> {
    require_human(u)?;
    let inst = apps::installation_by_id(&s.db, id).await?;
    let Some(t) = inst.target_id else { return Err(ApiError::not_found()) };
    let account = db::user_by_id(&s.db, t).await?;
    let role = crate::orgs::org_role(&s.db, t, u.id).await?;
    if t != u.id && role.is_none() && !u.site_admin {
        return Err(ApiError::not_found());
    }
    let manage = t == u.id || role.as_deref() == Some("admin") || u.site_admin;
    let app = apps::app_by_id(&s.db, inst.app_id).await?;
    Ok((inst, app, account, manage))
}

fn require_manage(manage: bool) -> ApiResult<()> {
    if manage { Ok(()) } else { Err(ApiError::forbidden("You must be an owner of the account to change this installation.")) }
}

async fn installation_details(s: &AppState, u: &User, inst: &InstallationRow, app: &AppRow, manage: bool) -> ApiResult<Value> {
    let mut v = apps::installation_json_with(s, inst, app).await?;
    v["app"] = apps::integration_json(s, app).await?;
    let ids = apps::installation_repo_ids(&s.db, inst).await?;
    let api = s.api_url();
    let mut repos = Vec::new();
    for ctx in apps::repos_by_ids(&s.db, &ids, Perm::None).await? {
        let perm = db::permission(&s.db, Some(u), &ctx.repo, &ctx.owner).await?;
        if perm >= Perm::Read {
            repos.push(crate::json::repo::repo(&api, s.base_url(), &db::RepoCtx { perm, ..ctx }));
        }
    }
    v["repositories"] = Value::Array(repos);
    v["viewer_can_manage"] = json!(manage);
    let pending = app.permissions != inst.permissions || {
        let mut a = app.events.clone();
        let mut b = inst.events.clone();
        a.sort();
        b.sort();
        a != b
    };
    v["pending_update"] = if pending { json!({ "permissions": app.permissions, "events": app.events }) } else { Value::Null };
    Ok(v)
}

pub async fn get_installation(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let (inst, app, _, manage) = installation(&s, &u, id).await?;
    Ok(Json(installation_details(&s, &u, &inst, &app, manage).await?))
}

#[derive(Deserialize, Default)]
pub struct UpdateInstallation {
    repository_selection: Option<String>,
    repository_ids: Option<Vec<i64>>,
}

pub async fn update_installation(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(b): Json<UpdateInstallation>) -> ApiResult<Json<Value>> {
    let (inst, app, account, manage) = installation(&s, &u, id).await?;
    require_manage(manage)?;
    let selection = b.repository_selection.unwrap_or_else(|| inst.repository_selection.clone());
    if !matches!(selection.as_str(), "all" | "selected") {
        return Err(ApiError::validation("Installation", "repository_selection", "invalid"));
    }
    let mut ids = match b.repository_ids {
        Some(ids) => ids,
        None if selection == "selected" => apps::installation_repo_ids(&s.db, &inst).await?,
        None => vec![],
    };
    ids.sort_unstable();
    ids.dedup();
    if selection == "selected" {
        if ids.is_empty() {
            return Err(ApiError::unprocessable("Select at least one repository, or uninstall the app."));
        }
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE owner_id = $1 AND id = ANY($2)")
            .bind(account.id)
            .bind(&ids)
            .fetch_one(&s.db)
            .await?;
        if n as usize != ids.len() {
            return Err(ApiError::validation("Installation", "repositories", "invalid"));
        }
    }
    let updated = apps::set_repositories(&s, &u, &app, &inst, &selection, &ids).await?;
    Ok(Json(installation_details(&s, &u, &updated, &app, manage).await?))
}

pub async fn delete_installation(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let (inst, app, _, manage) = installation(&s, &u, id).await?;
    require_manage(manage)?;
    apps::uninstall(&s, &u, &app, &inst).await?;
    Ok(no_content())
}

async fn suspend(s: AppState, u: User, id: i64, on: bool) -> ApiResult<Response> {
    let (inst, app, _, manage) = installation(&s, &u, id).await?;
    require_manage(manage)?;
    apps::set_suspended(&s, &u, &app, &inst, on).await?;
    Ok(no_content())
}

pub async fn suspend_installation(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    suspend(s, u, id, true).await
}

pub async fn unsuspend_installation(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    suspend(s, u, id, false).await
}

/// Accepts the app's current permissions and events for this installation.
pub async fn accept_permissions(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let (inst, app, _, manage) = installation(&s, &u, id).await?;
    require_manage(manage)?;
    let updated: InstallationRow = sqlx::query_as("UPDATE app_installations SET permissions = $2, events = $3, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(inst.id)
        .bind(&app.permissions)
        .bind(&app.events)
        .fetch_one(&s.db)
        .await?;
    apps::hooks::emit_installation(&s, &app, &updated, "installation", "new_permissions_accepted", &u, Map::new()).await;
    Ok(Json(installation_details(&s, &u, &updated, &app, manage).await?))
}

// ---------------------------------------------------------------------------------------------
// Authorized applications

#[derive(Deserialize, Default)]
pub struct AuthzQuery {
    kind: Option<String>,
}

pub async fn list_authorizations(State(s): State<AppState>, AuthUser(u): AuthUser, Query(q): Query<AuthzQuery>) -> ApiResult<Json<Value>> {
    require_human(&u)?;
    let kind = q.kind.unwrap_or_else(|| "oauth".into());
    let rows: Vec<(i64, i64, Vec<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT g.id, g.app_id, g.scopes, g.created_at, g.updated_at FROM oauth_grants g JOIN apps a ON a.id = g.app_id
          WHERE g.user_id = $1 AND a.kind = $2 ORDER BY g.updated_at DESC",
    )
    .bind(u.id)
    .bind(if kind == "app" { "app" } else { "oauth" })
    .fetch_all(&s.db)
    .await?;
    let api = s.api_url();
    let mut out = Vec::new();
    for (id, app_id, scopes, created, updated) in rows {
        let Ok(app) = apps::app_by_id(&s.db, app_id).await else { continue };
        let owner = db::user_by_id(&s.db, app.owner_id).await?;
        let last_used: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT max(t.last_used_at) FROM oauth_authorizations oa JOIN tokens t ON t.id = oa.token_id WHERE oa.grant_id = $1",
        )
        .bind(id)
        .fetch_one(&s.db)
        .await?;
        out.push(json!({
            "id": id,
            "scopes": scopes,
            "created_at": crate::json::ts(created),
            "updated_at": crate::json::ts(updated),
            "last_used_at": last_used.map(crate::json::ts),
            "app": {
                "name": app.name,
                "slug": app.slug,
                "kind": app.kind,
                "url": app.external_url,
                "client_id": app.client_id,
                "owner": crate::json::user::simple(&api, s.base_url(), &owner),
            },
        }));
    }
    Ok(Json(Value::Array(out)))
}

pub async fn revoke_authorization(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    require_human(&u)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM oauth_grants WHERE id = $1 AND user_id = $2)").bind(id).bind(u.id).fetch_one(&s.db).await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    apps::oauth::revoke_grant(&s, u.id, id).await?;
    Ok(no_content())
}
