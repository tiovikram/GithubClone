//! GitHub Apps REST endpoints: the authenticated app (JWT), its webhook, installations and
//! installation access tokens, installation-token endpoints and the user's installations.
//! Service layer and JSON shapes: `crate::apps`.
use axum::extract::{FromRequestParts, OriginalUri, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::apps::{self, jwt, AppRow, InstallationRow};
use crate::auth::AuthUser;
use crate::db::{self, Perm, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;
use crate::webhooks::{DeliveryRow, HookRow};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/app", get(get_app))
        .route("/app-manifests/{code}/conversions", post(convert_manifest))
        .route("/app/hook/config", get(get_hook_config).patch(update_hook_config))
        .route("/app/hook/deliveries", get(list_deliveries))
        .route("/app/hook/deliveries/{delivery_id}", get(get_delivery))
        .route("/app/hook/deliveries/{delivery_id}/attempts", post(redeliver))
        .route("/app/installation-requests", get(installation_requests))
        .route("/app/installations", get(list_installations))
        .route("/app/installations/{installation_id}", get(get_installation).delete(delete_installation))
        .route("/app/installations/{installation_id}/access_tokens", post(create_access_token))
        .route("/app/installations/{installation_id}/suspended", put(suspend).delete(unsuspend))
        .route("/apps/{app_slug}", get(get_by_slug))
        .route("/enterprises/{enterprise}/installation", get(enterprise_installation))
        .route("/installation/repositories", get(installation_repositories))
        .route("/installation/token", axum::routing::delete(revoke_installation_token))
        .route("/orgs/{org}/installation", get(org_installation))
        .route("/repos/{owner}/{repo}/installation", get(repo_installation))
        .route("/users/{username}/installation", get(user_installation))
        .route("/user/installations", get(user_installations))
        .route("/user/installations/{installation_id}/repositories", get(user_installation_repos))
        .route(
            "/user/installations/{installation_id}/repositories/{repository_id}",
            put(user_add_repo).delete(user_remove_repo),
        )
}

// ---------------------------------------------------------------------------------------------
// App (JWT) authentication

/// The GitHub App authenticated by a JWT (`Authorization: Bearer <jwt>`).
pub struct AppJwt(pub AppRow);

fn jwt_error(msg: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, msg)
}

impl FromRequestParts<AppState> for AppJwt {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let bad = || jwt_error(jwt::JwtError::Malformed.message());
        let h = parts.headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).ok_or_else(bad)?;
        let (scheme, cred) = h.trim().split_once(' ').ok_or_else(bad)?;
        if !matches!(scheme.to_ascii_lowercase().as_str(), "bearer" | "token") || !jwt::looks_like_jwt(cred.trim()) {
            return Err(bad());
        }
        let now = chrono::Utc::now().timestamp();
        let unverified = jwt::parse(cred.trim(), now).map_err(|e| jwt_error(e.message()))?;
        let app: Option<AppRow> = match unverified.issuer.parse::<i64>() {
            Ok(id) => apps::app_by_id(&state.db, id).await.ok(),
            Err(_) => apps::app_by_client_id(&state.db, &unverified.issuer).await.ok(),
        };
        let app = app.filter(|a| a.kind == "app").ok_or_else(|| jwt_error("Integration not found"))?;
        let keys: Vec<Vec<u8>> = sqlx::query_scalar("SELECT public_key FROM app_keys WHERE app_id = $1").bind(app.id).fetch_all(&state.db).await?;
        if !unverified.verify(keys.iter().map(Vec::as_slice)) {
            return Err(bad());
        }
        Ok(AppJwt(app))
    }
}

async fn app_installation(s: &AppState, app: &AppRow, id: i64) -> ApiResult<InstallationRow> {
    let inst = apps::installation_by_id(&s.db, id).await?;
    if inst.app_id != app.id {
        return Err(ApiError::not_found());
    }
    Ok(inst)
}

// ---------------------------------------------------------------------------------------------
// The app

async fn get_app(State(s): State<AppState>, AppJwt(app): AppJwt) -> ApiResult<Json<Value>> {
    Ok(Json(apps::integration_json(&s, &app).await?))
}

/// Visible to everyone when public; private apps only to their managers, their own bot or site admins.
async fn get_by_slug(State(s): State<AppState>, crate::auth::MaybeUser(u): crate::auth::MaybeUser, Path(slug): Path<String>) -> ApiResult<Json<Value>> {
    let app = apps::app_by_slug(&s.db, &slug).await?;
    if !app.public {
        let ok = match &u {
            Some(u) => app.bot_user_id == Some(u.id) || apps::can_manage(&s.db, &app, u).await? || crate::orgs::org_role(&s.db, app.owner_id, u.id).await?.is_some(),
            None => false,
        };
        if !ok {
            return Err(ApiError::not_found());
        }
    }
    Ok(Json(apps::integration_json(&s, &app).await?))
}

/// Manifest flow, step 2: exchanges a one-time code (issued by `POST /api/forge/app-manifests`)
/// for the new app and its credentials. No authentication, like GitHub.
async fn convert_manifest(State(s): State<AppState>, Path(code): Path<String>) -> ApiResult<Response> {
    let row: Option<(i64, i64, Value)> = sqlx::query_as(
        "DELETE FROM app_manifest_codes WHERE code_hash = $1 AND expires_at > now() RETURNING user_id, owner_id, manifest",
    )
    .bind(crate::auth::sha256_hex(&code))
    .fetch_optional(&s.db)
    .await?;
    let (user_id, owner_id, manifest) = row.ok_or_else(ApiError::not_found)?;
    let actor = db::user_by_id(&s.db, user_id).await?;
    let owner = db::user_by_id(&s.db, owner_id).await?;
    let m: apps::Manifest = serde_json::from_value(manifest).map_err(|_| ApiError::unprocessable("Invalid manifest"))?;
    let (app, secrets) = apps::create_app(&s, &actor, &owner, m).await?;
    Ok(created(app_with_secrets(&s, &app, &secrets).await?))
}

/// `integration` plus `client_secret`, `webhook_secret` and `pem` (shown once).
pub async fn app_with_secrets(s: &AppState, app: &AppRow, secrets: &apps::AppSecrets) -> ApiResult<Value> {
    let mut v = apps::integration_json(s, app).await?;
    v["client_secret"] = json!(secrets.client_secret);
    v["webhook_secret"] = json!(secrets.webhook_secret);
    v["pem"] = json!(secrets.pem);
    if let Some(o) = v.as_object_mut() {
        // the conversion response has no installations_count
        o.remove("installations_count");
    }
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// App webhook

async fn hook_of(s: &AppState, app: &AppRow) -> ApiResult<HookRow> {
    apps::hooks::app_hook(s, app.id).await?.ok_or_else(ApiError::not_found)
}

async fn get_hook_config(State(s): State<AppState>, AppJwt(app): AppJwt) -> ApiResult<Json<Value>> {
    let h = hook_of(&s, &app).await?;
    Ok(Json(apps::hooks::config_json(&h.config)))
}

async fn update_hook_config(State(s): State<AppState>, AppJwt(app): AppJwt, Json(patch): Json<Value>) -> ApiResult<Json<Value>> {
    let h = hook_of(&s, &app).await?;
    let mut cfg = h.config.as_object().cloned().unwrap_or_default();
    let Value::Object(p) = patch else { return Err(ApiError::unprocessable("Invalid request.")) };
    for (k, v) in p {
        match k.as_str() {
            "url" => {
                let u = v.as_str().unwrap_or("");
                if !url::Url::parse(u).is_ok_and(|x| matches!(x.scheme(), "http" | "https")) {
                    return Err(ApiError::validation("Hook", "url", "invalid"));
                }
                cfg.insert(k, json!(u));
            }
            "content_type" => {
                if !matches!(v.as_str(), Some("json" | "form")) {
                    return Err(ApiError::validation("Hook", "content_type", "invalid"));
                }
                cfg.insert(k, v);
            }
            "secret" => {
                cfg.remove("secret");
                match v.as_str().filter(|x| !x.is_empty()) {
                    Some(sec) => {
                        cfg.insert("secret_sealed".into(), json!(apps::hooks::seal_secret(&s, app.id, sec).await?));
                    }
                    None => {
                        cfg.remove("secret_sealed");
                    }
                }
            }
            "insecure_ssl" => {
                let val = match v {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s,
                    _ => return Err(ApiError::validation("Hook", "insecure_ssl", "invalid")),
                };
                cfg.insert(k, json!(val));
            }
            _ => {}
        }
    }
    let active = cfg.get("url").and_then(Value::as_str).is_some_and(|u| !u.is_empty());
    let h: HookRow = sqlx::query_as("UPDATE webhooks SET config = $2, active = $3 OR active, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(h.id)
        .bind(Value::Object(cfg))
        .bind(active)
        .fetch_one(&s.db)
        .await?;
    Ok(Json(apps::hooks::config_json(&h.config)))
}

fn delivery_item(d: &DeliveryRow) -> Value {
    json!({
        "id": d.id,
        "guid": d.guid,
        "delivered_at": crate::json::ts(d.delivered_at.unwrap_or(d.created_at)),
        "redelivery": d.redelivery,
        "duration": d.duration,
        "status": d.status,
        "status_code": d.status_code,
        "event": d.event,
        "action": d.action,
        "installation_id": d.payload.pointer("/installation/id").and_then(Value::as_i64),
        "repository_id": d.repo_id,
        "throttled_at": null,
    })
}

#[derive(Deserialize)]
struct DeliveryQuery {
    per_page: Option<i64>,
    cursor: Option<String>,
    status: Option<String>,
}

async fn list_deliveries(State(s): State<AppState>, AppJwt(app): AppJwt, Query(q): Query<DeliveryQuery>) -> ApiResult<Response> {
    let h = hook_of(&s, &app).await?;
    let per = q.per_page.unwrap_or(30).clamp(1, 100);
    let before: i64 = match q.cursor.as_deref() {
        None | Some("") => i64::MAX,
        Some(c) => c.parse().map_err(|_| ApiError::bad_request("Invalid cursor"))?,
    };
    let status = match q.status.as_deref() {
        None => None,
        Some(v @ ("success" | "failure")) => Some(v == "success"),
        Some(_) => return Err(ApiError::validation("Hook", "status", "invalid")),
    };
    let rows: Vec<DeliveryRow> = sqlx::query_as(
        "SELECT * FROM webhook_deliveries WHERE hook_id = $1 AND id < $2
            AND ($4::boolean IS NULL OR ((status_code BETWEEN 200 AND 299) = $4))
          ORDER BY id DESC LIMIT $3",
    )
    .bind(h.id)
    .bind(before)
    .bind(per)
    .bind(status)
    .fetch_all(&s.db)
    .await?;
    let mut res = axum::Json(Value::Array(rows.iter().map(delivery_item).collect())).into_response();
    if rows.len() as i64 == per {
        let last = rows.last().map(|r| r.id).unwrap_or(0);
        let link = format!("<{}/app/hook/deliveries?per_page={per}&cursor={last}>; rel=\"next\"", s.api_url());
        if let Ok(v) = link.parse() {
            res.headers_mut().insert(header::LINK, v);
        }
    }
    Ok(res)
}

async fn load_delivery(s: &AppState, app: &AppRow, id: i64) -> ApiResult<(HookRow, DeliveryRow)> {
    let h = hook_of(s, app).await?;
    let d: DeliveryRow = sqlx::query_as("SELECT * FROM webhook_deliveries WHERE id = $1 AND hook_id = $2")
        .bind(id)
        .bind(h.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((h, d))
}

async fn get_delivery(State(s): State<AppState>, AppJwt(app): AppJwt, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let (_, d) = load_delivery(&s, &app, id).await?;
    let mut v = delivery_item(&d);
    v["url"] = json!(format!("{}/app/hook/deliveries/{}", s.api_url(), d.id));
    v["request"] = json!({ "headers": d.request_headers, "payload": d.payload });
    v["response"] = json!({ "headers": d.response_headers, "payload": d.response_body });
    Ok(Json(v))
}

async fn redeliver(State(s): State<AppState>, AppJwt(app): AppJwt, Path(id): Path<i64>) -> ApiResult<Response> {
    let (h, d) = load_delivery(&s, &app, id).await?;
    crate::webhooks::create_delivery(&s, &h, &d.event, d.repo_id, d.payload.clone(), true).await?;
    Ok((StatusCode::ACCEPTED, axum::Json(json!({}))).into_response())
}

/// forge has no installation request flow (owners install directly), so the list is empty.
async fn installation_requests(State(s): State<AppState>, uri: OriginalUri, AppJwt(_app): AppJwt, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    Ok(Paged::new(&uri, &s, p, Some(0), vec![]))
}

// ---------------------------------------------------------------------------------------------
// Installations (JWT)

#[derive(Deserialize, Default)]
struct InstallationsQuery {
    since: Option<chrono::DateTime<chrono::Utc>>,
    #[allow(dead_code)]
    outdated: Option<String>,
}

async fn list_installations(
    State(s): State<AppState>,
    uri: OriginalUri,
    AppJwt(app): AppJwt,
    Query(p): Query<PageParams>,
    Query(q): Query<InstallationsQuery>,
) -> ApiResult<Paged> {
    let rows: Vec<InstallationRow> = sqlx::query_as(
        "SELECT * FROM app_installations WHERE app_id = $1 AND ($2::timestamptz IS NULL OR updated_at >= $2) ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(app.id)
    .bind(q.since)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM app_installations WHERE app_id = $1 AND ($2::timestamptz IS NULL OR updated_at >= $2)")
        .bind(app.id)
        .bind(q.since)
        .fetch_one(&s.db)
        .await?;
    Ok(Paged::new(&uri, &s, p, Some(total), apps::installations_json(&s, &rows).await?))
}

async fn get_installation(State(s): State<AppState>, AppJwt(app): AppJwt, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let inst = app_installation(&s, &app, id).await?;
    Ok(Json(apps::installation_json_with(&s, &inst, &app).await?))
}

async fn bot_of(s: &AppState, app: &AppRow) -> ApiResult<User> {
    db::user_by_id(&s.db, app.bot_user_id.ok_or_else(ApiError::not_found)?).await
}

async fn delete_installation(State(s): State<AppState>, AppJwt(app): AppJwt, Path(id): Path<i64>) -> ApiResult<Response> {
    let inst = app_installation(&s, &app, id).await?;
    let bot = bot_of(&s, &app).await?;
    apps::uninstall(&s, &bot, &app, &inst).await?;
    Ok(no_content())
}

async fn create_access_token(
    State(s): State<AppState>,
    AppJwt(app): AppJwt,
    Path(id): Path<i64>,
    Json(req): Json<apps::TokenRequest>,
) -> ApiResult<Response> {
    let inst = app_installation(&s, &app, id).await?;
    let v = apps::create_installation_token(&s, &app, &inst, req).await?;
    Ok((StatusCode::CREATED, axum::Json(v)).into_response())
}

async fn suspend(State(s): State<AppState>, AppJwt(app): AppJwt, Path(id): Path<i64>) -> ApiResult<Response> {
    let inst = app_installation(&s, &app, id).await?;
    let bot = bot_of(&s, &app).await?;
    apps::set_suspended(&s, &bot, &app, &inst, true).await?;
    Ok(no_content())
}

async fn unsuspend(State(s): State<AppState>, AppJwt(app): AppJwt, Path(id): Path<i64>) -> ApiResult<Response> {
    let inst = app_installation(&s, &app, id).await?;
    if inst.suspended_at.is_none() {
        return Err(ApiError::not_found());
    }
    let bot = bot_of(&s, &app).await?;
    apps::set_suspended(&s, &bot, &app, &inst, false).await?;
    Ok(no_content())
}

async fn installed(s: &AppState, app: &AppRow, target: Option<i64>) -> ApiResult<Json<Value>> {
    let inst = apps::installation_for(&s.db, app.id, target).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(apps::installation_json_with(s, &inst, app).await?))
}

async fn enterprise_installation(State(s): State<AppState>, AppJwt(app): AppJwt, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    if !e.eq_ignore_ascii_case(&apps::enterprise_slug()) {
        return Err(ApiError::not_found());
    }
    installed(&s, &app, None).await
}

async fn org_installation(State(s): State<AppState>, AppJwt(app): AppJwt, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = crate::orgs::org_by_login(&s, &org).await?;
    installed(&s, &app, Some(o.id)).await
}

async fn user_installation(State(s): State<AppState>, AppJwt(app): AppJwt, Path(login): Path<String>) -> ApiResult<Json<Value>> {
    let u = db::user_by_login(&s.db, &login).await?;
    installed(&s, &app, Some(u.id)).await
}

/// The app's installation that covers the repository (404 if the app cannot access it).
async fn repo_installation(State(s): State<AppState>, AppJwt(app): AppJwt, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let o = db::user_by_login(&s.db, &owner).await?;
    let r: db::Repo = sqlx::query_as("SELECT * FROM repositories WHERE owner_id = $1 AND lower(name) = lower($2)")
        .bind(o.id)
        .bind(&repo)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let inst = apps::installation_for(&s.db, app.id, Some(o.id)).await?.ok_or_else(ApiError::not_found)?;
    if !apps::installation_repo_ids(&s.db, &inst).await?.contains(&r.id) {
        return Err(ApiError::not_found());
    }
    Ok(Json(apps::installation_json_with(&s, &inst, &app).await?))
}

// ---------------------------------------------------------------------------------------------
// Installation token endpoints

async fn installation_repositories(State(s): State<AppState>, uri: OriginalUri, AuthUser(_u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = apps::scope::current()
        .filter(|sc| sc.bot)
        .ok_or_else(|| ApiError::forbidden("This endpoint requires an installation access token."))?;
    let inst = apps::installation_by_id(&s.db, sc.installation_id.unwrap_or(0)).await?;
    let mut ids = apps::installation_repo_ids(&s.db, &inst).await?;
    if let Some(t) = &sc.token_repos {
        ids.retain(|i| t.contains(i));
    }
    let total = ids.len() as i64;
    let page: Vec<i64> = ids.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    let api = s.api_url();
    let perm = if sc.level("administration") >= 2 { Perm::Admin } else if sc.level("contents") >= 2 { Perm::Write } else { Perm::Read };
    let repos: Vec<Value> = apps::repos_by_ids(&s.db, &page, perm).await?.iter().map(|r| crate::json::repo::repo(&api, s.base_url(), r)).collect();
    let selection = if sc.token_repos.is_some() { "selected" } else { inst.repository_selection.as_str() };
    let body = json!({ "total_count": total, "repository_selection": selection, "repositories": repos });
    let mut res = Paged::new(&uri, &s, p, Some(total), vec![]).into_response();
    // reuse the Link header of Paged, with the object body
    let link = res.headers_mut().remove(header::LINK);
    let mut out = axum::Json(body).into_response();
    if let Some(l) = link {
        out.headers_mut().insert(header::LINK, l);
    }
    Ok(out)
}

async fn revoke_installation_token(State(s): State<AppState>, AuthUser(_u): AuthUser) -> ApiResult<Response> {
    let sc = apps::scope::current()
        .filter(|sc| sc.bot)
        .ok_or_else(|| ApiError::forbidden("This endpoint requires an installation access token."))?;
    sqlx::query("DELETE FROM app_installation_tokens WHERE token_hash = $1").bind(&sc.token_hash).execute(&s.db).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// The user's installations

async fn user_installations(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Response> {
    if u.kind == "Bot" {
        return Err(ApiError::forbidden(apps::scope::NOT_ACCESSIBLE));
    }
    let mut rows = apps::installations_for_user(&s.db, &u).await?;
    // A user-to-server token only sees its own app's installations.
    if let Some(sc) = apps::scope::current() {
        rows.retain(|r| r.app_id == sc.app_id);
    }
    let total = rows.len() as i64;
    let page: Vec<InstallationRow> = rows.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    let body = json!({ "total_count": total, "installations": apps::installations_json(&s, &page).await? });
    let mut link_src = Paged::new(&uri, &s, p, Some(total), vec![]).into_response();
    let mut out = axum::Json(body).into_response();
    if let Some(l) = link_src.headers_mut().remove(header::LINK) {
        out.headers_mut().insert(header::LINK, l);
    }
    Ok(out)
}

/// The installation `id` if `u` can see it (installed on their account or an org they're in).
async fn visible_installation(s: &AppState, u: &User, id: i64) -> ApiResult<InstallationRow> {
    let inst = apps::installation_by_id(&s.db, id).await?;
    let Some(t) = inst.target_id else { return Err(ApiError::not_found()) };
    if t != u.id && crate::orgs::org_role(&s.db, t, u.id).await?.is_none() && !u.site_admin {
        return Err(ApiError::not_found());
    }
    if let Some(sc) = apps::scope::current() {
        if sc.app_id != inst.app_id {
            return Err(ApiError::not_found());
        }
    }
    Ok(inst)
}

async fn user_installation_repos(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(id): Path<i64>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let inst = visible_installation(&s, &u, id).await?;
    let ids = apps::installation_repo_ids(&s.db, &inst).await?;
    let api = s.api_url();
    let mut visible = Vec::new();
    for ctx in apps::repos_by_ids(&s.db, &ids, Perm::None).await? {
        let perm = db::permission(&s.db, Some(&u), &ctx.repo, &ctx.owner).await?;
        if perm >= Perm::Read {
            visible.push(db::RepoCtx { perm, ..ctx });
        }
    }
    let total = visible.len() as i64;
    let repos: Vec<Value> = visible.iter().skip(p.offset() as usize).take(p.limit() as usize).map(|r| crate::json::repo::repo(&api, s.base_url(), r)).collect();
    let body = json!({ "total_count": total, "repository_selection": inst.repository_selection, "repositories": repos });
    let mut link_src = Paged::new(&uri, &s, p, Some(total), vec![]).into_response();
    let mut out = axum::Json(body).into_response();
    if let Some(l) = link_src.headers_mut().remove(header::LINK) {
        out.headers_mut().insert(header::LINK, l);
    }
    Ok(out)
}

/// Adding/removing repositories needs admin on the repository and the right to manage the
/// installation's account (the user themselves or an org owner).
async fn manage_repo(s: &AppState, u: &User, id: i64, repo_id: i64) -> ApiResult<(InstallationRow, AppRow)> {
    let inst = visible_installation(s, u, id).await?;
    let t = inst.target_id.unwrap_or(0);
    let account = db::user_by_id(&s.db, t).await?;
    if account.kind == "Organization" {
        crate::orgs::require_owner(s, &account, u).await.map_err(|e| if e.status == StatusCode::NOT_FOUND { e } else { ApiError::forbidden(e.message) })?;
    }
    let ctx = db::load_repo_by_id(&s.db, Some(u), repo_id).await?;
    if ctx.repo.owner_id != t {
        return Err(ApiError::not_found());
    }
    ctx.require(Perm::Admin)?;
    let app = apps::app_by_id(&s.db, inst.app_id).await?;
    Ok((inst, app))
}

async fn user_add_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, repo_id)): Path<(i64, i64)>) -> ApiResult<Response> {
    let (inst, app) = manage_repo(&s, &u, id, repo_id).await?;
    if inst.repository_selection == "all" {
        return Ok(no_content());
    }
    apps::change_repositories(&s, &u, &app, &inst, &[repo_id], &[]).await?;
    Ok(no_content())
}

async fn user_remove_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, repo_id)): Path<(i64, i64)>) -> ApiResult<Response> {
    let (inst, app) = manage_repo(&s, &u, id, repo_id).await?;
    if inst.repository_selection == "all" {
        return Err(ApiError::unprocessable("The installation has access to all repositories."));
    }
    let ids = apps::installation_repo_ids(&s.db, &inst).await?;
    if !ids.contains(&repo_id) {
        return Err(ApiError::not_found());
    }
    if ids.len() == 1 {
        return Err(ApiError::unprocessable("An installation must keep at least one repository; uninstall the app instead."));
    }
    apps::change_repositories(&s, &u, &app, &inst, &[], &[repo_id]).await?;
    Ok(no_content())
}
