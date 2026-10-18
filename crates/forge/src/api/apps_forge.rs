//! Forge-only app management (nested under `/api/forge`, not part of the GitHub API). On GitHub
//! these are web UI flows:
//!
//! * `POST /api/forge/apps` registers a GitHub App directly (body: a GitHub App manifest plus an
//!   optional `owner` org login). Returns the app with `client_secret`, `webhook_secret` and the
//!   private key `pem`, shown once.
//! * `POST /api/forge/app-manifests` `{manifest, owner?}` starts the manifest flow: returns a
//!   one-time `code` (1 h) that the app exchanges at `POST /api/v3/app-manifests/{code}/conversions`.
//! * `POST /api/forge/oauth-apps` `{name, url, callback_url?, owner?}` registers an OAuth app.
//! * `GET /api/forge/apps` lists the caller's apps; `DELETE /api/forge/apps/{slug}` deletes one;
//!   `POST /api/forge/apps/{slug}/keys` adds a private key (PEM shown once).
//! * `POST /api/forge/apps/{slug}/installations` `{account?, enterprise?, repository_selection,
//!   repository_ids?, repositories?}` installs the app on the caller's account, an org they own,
//!   or (site admins) the enterprise.
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content};
use crate::apps::{self, InstallTarget, Manifest};
use crate::auth::AuthUser;
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn forge_routes() -> Router<AppState> {
    Router::new()
        .route("/apps", get(list_apps).post(create_app))
        .route("/apps/{slug}", get(super::apps_manage::get_app).patch(super::apps_manage::update_app).delete(delete_app))
        .route("/apps/{slug}/keys", get(super::apps_manage::list_keys).post(add_key))
        .route("/apps/{slug}/keys/{key_id}", delete(super::apps_manage::delete_key))
        .route("/apps/{slug}/client-secret", post(super::apps_manage::new_client_secret))
        .route(
            "/installations/{installation_id}",
            get(super::apps_manage::get_installation).patch(super::apps_manage::update_installation).delete(super::apps_manage::delete_installation),
        )
        .route(
            "/installations/{installation_id}/suspended",
            axum::routing::put(super::apps_manage::suspend_installation).delete(super::apps_manage::unsuspend_installation),
        )
        .route("/installations/{installation_id}/permissions", post(super::apps_manage::accept_permissions))
        .route("/authorizations", get(super::apps_manage::list_authorizations))
        .route("/authorizations/{grant_id}", delete(super::apps_manage::revoke_authorization))
        .route("/apps/{slug}/installations", post(install))
        .route("/app-manifests", post(create_manifest_code))
        .route("/oauth-apps", post(create_oauth_app))
        .merge(super::apps_migrations::forge_routes())
        .merge(super::packages_forge::forge_routes())
}

async fn owner_for(s: &AppState, u: &User, owner: Option<&str>) -> ApiResult<User> {
    match owner.filter(|o| !o.is_empty() && !o.eq_ignore_ascii_case(&u.login)) {
        Some(o) => crate::orgs::org_by_login(s, o).await,
        None => Ok(u.clone()),
    }
}

fn require_human(u: &User) -> ApiResult<()> {
    if u.kind != "User" {
        return Err(ApiError::forbidden(apps::scope::NOT_ACCESSIBLE));
    }
    Ok(())
}

#[derive(Deserialize, Default)]
struct CreateApp {
    owner: Option<String>,
    #[serde(flatten)]
    manifest: Manifest,
}

async fn create_app(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<CreateApp>) -> ApiResult<Response> {
    require_human(&u)?;
    let owner = owner_for(&s, &u, b.owner.as_deref()).await?;
    let (app, secrets) = apps::create_app(&s, &u, &owner, b.manifest).await?;
    Ok(created(super::apps_core::app_with_secrets(&s, &app, &secrets).await?))
}

async fn list_apps(State(s): State<AppState>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let rows: Vec<apps::AppRow> = sqlx::query_as(
        "SELECT * FROM apps WHERE owner_id = $1 OR owner_id IN (SELECT org_id FROM org_members WHERE user_id = $1 AND role = 'admin') ORDER BY id",
    )
    .bind(u.id)
    .fetch_all(&s.db)
    .await?;
    let mut out = Vec::new();
    for a in &rows {
        let mut v = if a.kind == "app" {
            apps::integration_json(&s, a).await?
        } else {
            let owner = db::user_by_id(&s.db, a.owner_id).await?;
            json!({
                "id": a.id,
                "slug": a.slug,
                "name": a.name,
                "url": a.external_url,
                "client_id": a.client_id,
                "description": a.description,
                "owner": crate::json::user::simple(&s.api_url(), s.base_url(), &owner),
            })
        };
        v["kind"] = json!(a.kind);
        out.push(v);
    }
    Ok(Json(Value::Array(out)))
}

async fn managed_app(s: &AppState, u: &User, slug: &str) -> ApiResult<apps::AppRow> {
    let app: apps::AppRow = sqlx::query_as("SELECT * FROM apps WHERE lower(slug) = lower($1)")
        .bind(slug)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !apps::can_manage(&s.db, &app, u).await? {
        return Err(ApiError::not_found());
    }
    Ok(app)
}

async fn delete_app(State(s): State<AppState>, AuthUser(u): AuthUser, Path(slug): Path<String>) -> ApiResult<Response> {
    require_human(&u)?;
    let app = managed_app(&s, &u, &slug).await?;
    apps::delete_app(&s, &app).await?;
    Ok(no_content())
}

async fn add_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path(slug): Path<String>) -> ApiResult<Response> {
    require_human(&u)?;
    let app = managed_app(&s, &u, &slug).await?;
    if app.kind != "app" {
        return Err(ApiError::not_found());
    }
    let (id, pem) = apps::add_key(&s, &app).await?;
    Ok(created(json!({ "id": id, "pem": pem })))
}

#[derive(Deserialize, Default)]
struct InstallBody {
    account: Option<String>,
    enterprise: Option<bool>,
    repository_selection: Option<String>,
    repository_ids: Option<Vec<i64>>,
    repositories: Option<Vec<String>>,
}

async fn install(State(s): State<AppState>, AuthUser(u): AuthUser, Path(slug): Path<String>, Json(b): Json<InstallBody>) -> ApiResult<Response> {
    require_human(&u)?;
    let app = apps::app_by_slug(&s.db, &slug).await?;
    let target = if b.enterprise.unwrap_or(false) {
        InstallTarget::Enterprise
    } else {
        match b.account.as_deref().filter(|a| !a.is_empty()) {
            Some(a) => InstallTarget::Account(db::user_by_login(&s.db, a).await?),
            None => InstallTarget::Account(u.clone()),
        }
    };
    let selection = b.repository_selection.unwrap_or_else(|| "all".into());
    let mut ids = b.repository_ids.unwrap_or_default();
    if let (Some(names), InstallTarget::Account(a)) = (&b.repositories, &target) {
        match apps::resolve_repo_names(&s.db, a.id, names).await? {
            Ok(found) => ids.extend(found),
            Err(_) => return Err(ApiError::validation("Installation", "repositories", "invalid")),
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let (inst, _) = apps::install(&s, &u, &app, &target, &selection, &ids, false).await?;
    Ok(created(apps::installation_json_with(&s, &inst, &app).await?))
}

#[derive(Deserialize, Default)]
struct ManifestBody {
    manifest: Option<Value>,
    owner: Option<String>,
}

async fn create_manifest_code(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<ManifestBody>) -> ApiResult<Response> {
    require_human(&u)?;
    let owner = owner_for(&s, &u, b.owner.as_deref()).await?;
    if owner.kind == "Organization" {
        crate::orgs::require_owner(&s, &owner, &u).await?;
    }
    let manifest = match b.manifest {
        Some(Value::String(text)) => serde_json::from_str::<Value>(&text).map_err(|_| ApiError::validation("Manifest", "manifest", "invalid"))?,
        Some(v @ Value::Object(_)) => v,
        _ => return Err(ApiError::validation("Manifest", "manifest", "missing_field")),
    };
    let parsed: Manifest = serde_json::from_value(manifest.clone()).map_err(|_| ApiError::validation("Manifest", "manifest", "invalid"))?;
    if parsed.name.as_deref().is_none_or(str::is_empty) || parsed.url.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::validation("Manifest", "manifest", "missing_field"));
    }
    let code = crate::auth::random_string(40);
    let exp: (chrono::DateTime<chrono::Utc>,) = sqlx::query_as(
        "INSERT INTO app_manifest_codes (code_hash, user_id, owner_id, manifest, expires_at) VALUES ($1, $2, $3, $4, now() + interval '1 hour') RETURNING expires_at",
    )
    .bind(crate::auth::sha256_hex(&code))
    .bind(u.id)
    .bind(owner.id)
    .bind(&manifest)
    .fetch_one(&s.db)
    .await?;
    let _ = sqlx::query("DELETE FROM app_manifest_codes WHERE expires_at < now()").execute(&s.db).await;
    let redirect = parsed.redirect_url.as_ref().map(|r| format!("{r}{}code={code}", if r.contains('?') { '&' } else { '?' }));
    Ok(created(json!({
        "code": code,
        "expires_at": crate::json::ts(exp.0),
        "conversion_url": format!("{}/app-manifests/{code}/conversions", s.api_url()),
        "redirect_url": redirect,
    })))
}

#[derive(Deserialize, Default)]
struct OauthBody {
    name: Option<String>,
    url: Option<String>,
    callback_url: Option<String>,
    owner: Option<String>,
}

async fn create_oauth_app(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<OauthBody>) -> ApiResult<Response> {
    require_human(&u)?;
    let owner = owner_for(&s, &u, b.owner.as_deref()).await?;
    let (app, secrets) = apps::create_oauth_app(&s, &u, &owner, b.name.as_deref().unwrap_or(""), b.url.as_deref().unwrap_or(""), b.callback_url.as_deref()).await?;
    Ok(created(json!({
        "id": app.id,
        "slug": app.slug,
        "name": app.name,
        "url": app.external_url,
        "client_id": app.client_id,
        "client_secret": secrets.client_secret,
        "callback_urls": app.callback_urls,
    })))
}
