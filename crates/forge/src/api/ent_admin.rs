//! GHES enterprise administration, instance level: global webhooks, public keys, LDAP mapping,
//! organization rename, personal access tokens, users (create/rename/delete, impersonation,
//! promote/demote, suspend/unsuspend), instance statistics, license and announcement banner.
//! Site administrators only (403 otherwise). Service logic lives in `crate::enterprise`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::db::{self, User};
use crate::enterprise::{self as ent, SiteAdmin, TokenRow};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;
use crate::webhooks::HookRow;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/hooks", get(list_hooks).post(create_hook))
        .route("/admin/hooks/{hook_id}", get(get_hook).patch(update_hook).delete(delete_hook))
        .route("/admin/hooks/{hook_id}/pings", post(ping_hook))
        .route("/admin/keys", get(list_keys))
        .route("/admin/keys/{key_ids}", delete(delete_keys))
        .route("/admin/ldap/teams/{team_id}/mapping", patch(ldap_team_mapping))
        .route("/admin/ldap/teams/{team_id}/sync", post(ldap_team_sync))
        .route("/admin/ldap/users/{username}/mapping", patch(ldap_user_mapping))
        .route("/admin/ldap/users/{username}/sync", post(ldap_user_sync))
        .route("/admin/organizations/{org}", patch(rename_org))
        .route("/admin/tokens", get(list_tokens))
        .route("/admin/tokens/{token_id}", delete(delete_token))
        .route("/admin/users", post(create_user))
        .route("/admin/users/{username}", patch(rename_user).delete(delete_user))
        .route("/admin/users/{username}/authorizations", post(create_impersonation).delete(delete_impersonation))
        .route("/users/{username}/site_admin", put(promote).delete(demote))
        .route("/users/{username}/suspended", put(suspend).delete(unsuspend))
        .route("/enterprise/stats/{kind}", get(stats))
        .route("/enterprise/settings/license", get(license))
        .route("/enterprise/announcement", get(get_announcement).patch(set_announcement).delete(remove_announcement))
}

async fn account(s: &AppState, login: &str) -> ApiResult<User> {
    let u = db::user_by_login(&s.db, login).await?;
    if u.kind != "User" {
        return Err(ApiError::not_found());
    }
    Ok(u)
}

// ---------------------------------------------------------------------------------------------
// Global webhooks

fn global_hook_json(api: &str, h: &HookRow) -> Value {
    let url = format!("{api}/admin/hooks/{}", h.id);
    json!({
        "type": "Global",
        "id": h.id,
        "name": h.name,
        "active": h.active,
        "events": h.events,
        "config": super::webhooks::config_json(&h.config),
        "updated_at": crate::json::ts(h.updated_at),
        "created_at": crate::json::ts(h.created_at),
        "url": url,
        "ping_url": format!("{url}/pings"),
    })
}

async fn load_hook(s: &AppState, id: i64) -> ApiResult<HookRow> {
    sqlx::query_as("SELECT * FROM webhooks WHERE id = $1 AND is_global").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
struct HookBody {
    name: Option<String>,
    config: Option<Value>,
    events: Option<Vec<String>>,
    active: Option<bool>,
}

fn check_config(cfg: &Value) -> ApiResult<()> {
    let url = cfg.get("url").and_then(Value::as_str).unwrap_or("");
    if url::Url::parse(url).ok().filter(|u| matches!(u.scheme(), "http" | "https")).is_none() {
        return Err(ApiError::validation("Hook", "url", if url.is_empty() { "missing_field" } else { "invalid" }));
    }
    if let Some(ct) = cfg.get("content_type").and_then(Value::as_str) {
        if !matches!(ct, "json" | "form") {
            return Err(ApiError::validation("Hook", "content_type", "invalid"));
        }
    }
    Ok(())
}

async fn list_hooks(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let rows: Vec<HookRow> = sqlx::query_as("SELECT * FROM webhooks WHERE is_global ORDER BY id LIMIT $1 OFFSET $2")
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM webhooks WHERE is_global").fetch_one(&s.db).await?;
    let api = s.api_url();
    Ok(Paged::new(&uri, &s, p, Some(total), rows.iter().map(|h| global_hook_json(&api, h)).collect()))
}

async fn create_hook(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<HookBody>) -> ApiResult<Response> {
    let name = b.name.unwrap_or_else(|| "web".into());
    if name != "web" {
        return Err(ApiError::validation("Hook", "name", "invalid"));
    }
    let cfg = b.config.ok_or_else(|| ApiError::validation("Hook", "config", "missing_field"))?;
    check_config(&cfg)?;
    let events = b.events.filter(|e| !e.is_empty()).unwrap_or_else(|| vec!["user".into(), "organization".into()]);
    let h: HookRow = sqlx::query_as(
        "INSERT INTO webhooks (repo_id, org_id, config, events, active, name, is_global) VALUES (NULL, NULL, $1, $2, $3, $4, TRUE) RETURNING *",
    )
    .bind(&cfg)
    .bind(&events)
    .bind(b.active.unwrap_or(true))
    .bind(&name)
    .fetch_one(&s.db)
    .await?;
    ent::audit(&s, "hook.create", Some(&u), json!({"hook_id": h.id, "global": true})).await;
    Ok(created(global_hook_json(&s.api_url(), &h)))
}

async fn get_hook(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let h = load_hook(&s, id).await?;
    Ok(Json(global_hook_json(&s.api_url(), &h)))
}

async fn update_hook(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>, Json(b): Json<HookBody>) -> ApiResult<Json<Value>> {
    let h = load_hook(&s, id).await?;
    let cfg = match b.config {
        Some(mut c) => {
            // A masked or omitted secret keeps the stored one.
            if c.get("secret").is_none() || c.get("secret").and_then(Value::as_str) == Some("********") {
                if let Some(old) = h.config.get("secret") {
                    c["secret"] = old.clone();
                }
            }
            check_config(&c)?;
            c
        }
        None => h.config.clone(),
    };
    let events = b.events.filter(|e| !e.is_empty()).unwrap_or(h.events.clone());
    let h: HookRow = sqlx::query_as(
        "UPDATE webhooks SET config = $2, events = $3, active = COALESCE($4, active), updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(h.id)
    .bind(&cfg)
    .bind(&events)
    .bind(b.active)
    .fetch_one(&s.db)
    .await?;
    ent::audit(&s, "hook.config_changed", Some(&u), json!({"hook_id": h.id, "global": true})).await;
    let mut v = global_hook_json(&s.api_url(), &h);
    if let Some(c) = v.get_mut("config").and_then(Value::as_object_mut) {
        c.remove("secret");
    }
    Ok(Json(v))
}

async fn delete_hook(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Response> {
    let h = load_hook(&s, id).await?;
    sqlx::query("DELETE FROM webhooks WHERE id = $1").bind(h.id).execute(&s.db).await?;
    ent::audit(&s, "hook.destroy", Some(&u), json!({"hook_id": h.id, "global": true})).await;
    Ok(no_content())
}

async fn ping_hook(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Response> {
    let h = load_hook(&s, id).await?;
    let payload = json!({
        "zen": "Design for failure.",
        "hook_id": h.id,
        "hook": global_hook_json(&s.api_url(), &h),
        "sender": crate::json::user::simple(&s.api_url(), s.base_url(), &u),
        "enterprise": ent::enterprise_json(&s),
    });
    crate::webhooks::create_delivery(&s, &h, "ping", None, payload, false).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Public keys (user SSH keys and deploy keys)

#[derive(Deserialize)]
struct KeysQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    direction: Option<String>,
    sort: Option<String>,
    since: Option<String>,
}

#[derive(sqlx::FromRow)]
struct KeyRow {
    id: i64,
    key: String,
    title: String,
    user_id: Option<i64>,
    repo_id: Option<i64>,
    read_only: bool,
    added_by: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used: Option<chrono::DateTime<chrono::Utc>>,
    url_path: String,
}

async fn list_keys(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Query(q): Query<KeysQuery>) -> ApiResult<Paged> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let col = match q.sort.as_deref() {
        Some("accessed") => "last_used",
        _ => "created_at",
    };
    let dir = if q.direction.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let since = q
        .since
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok().map(|d| d.with_timezone(&chrono::Utc)).or_else(|| {
            chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)).map(|d| d.and_utc())
        }));
    let base = "SELECT k.id, k.key, k.title, k.user_id, NULL::bigint AS repo_id, FALSE AS read_only, NULL::text AS added_by,
                       k.created_at, k.last_used, '/user/keys/' || k.id AS url_path
                  FROM ssh_keys k
                UNION ALL
                SELECT d.id, d.key, d.title, NULL::bigint, d.repo_id, d.read_only, a.login, d.created_at, d.last_used,
                       '/repos/' || o.login || '/' || r.name || '/keys/' || d.id
                  FROM deploy_keys d JOIN repositories r ON r.id = d.repo_id JOIN users o ON o.id = r.owner_id
                  LEFT JOIN users a ON a.id = d.added_by_id";
    let sql = format!(
        "SELECT * FROM ({base}) x WHERE ($1::timestamptz IS NULL OR created_at >= $1) ORDER BY {col} {dir} NULLS LAST, id {dir} LIMIT $2 OFFSET $3"
    );
    let rows: Vec<KeyRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(since).bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM ({base}) x WHERE ($1::timestamptz IS NULL OR created_at >= $1)")))
        .bind(since)
        .fetch_one(&s.db)
        .await?;
    let api = s.api_url();
    let items = rows
        .iter()
        .map(|k| {
            json!({
                "id": k.id,
                "key": k.key,
                "user_id": k.user_id,
                "repository_id": k.repo_id,
                "url": format!("{api}{}", k.url_path),
                "title": k.title,
                "read_only": k.read_only,
                "verified": true,
                "created_at": crate::json::ts(k.created_at),
                "added_by": k.added_by,
                "last_used": k.last_used.map(crate::json::ts),
                "enabled": true,
            })
        })
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn delete_keys(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(ids): Path<String>) -> ApiResult<Response> {
    let ids: Vec<i64> = ids.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if ids.is_empty() {
        return Err(ApiError::not_found());
    }
    let a = sqlx::query("DELETE FROM ssh_keys WHERE id = ANY($1)").bind(&ids).execute(&s.db).await?.rows_affected();
    let b = sqlx::query("DELETE FROM deploy_keys WHERE id = ANY($1)").bind(&ids).execute(&s.db).await?.rows_affected();
    if a + b == 0 {
        return Err(ApiError::not_found());
    }
    ent::audit(&s, "public_key.delete", Some(&u), json!({"key_ids": ids})).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// LDAP mapping (stored only: forge does not contact an LDAP server; sync requests are recorded)

#[derive(Deserialize)]
struct LdapBody {
    ldap_dn: Option<String>,
}

fn ldap_dn(b: LdapBody) -> ApiResult<String> {
    b.ldap_dn.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).ok_or_else(|| ApiError::validation("LdapMapping", "ldap_dn", "missing_field"))
}

async fn store_mapping(s: &AppState, kind: &str, id: i64, dn: &str) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO ent_ldap_mappings (kind, subject_id, ldap_dn) VALUES ($1, $2, $3)
         ON CONFLICT (kind, subject_id) DO UPDATE SET ldap_dn = $3, updated_at = now()",
    )
    .bind(kind)
    .bind(id)
    .bind(dn)
    .execute(&s.db)
    .await?;
    Ok(())
}

async fn team_by_id(s: &AppState, id: i64) -> ApiResult<(User, super::orgs_teams::TeamRow)> {
    let t: super::orgs_teams::TeamRow =
        sqlx::query_as("SELECT * FROM teams WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
    let org = db::user_by_id(&s.db, t.org_id).await?;
    Ok((org, t))
}

async fn ldap_team_mapping(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>, Json(b): Json<LdapBody>) -> ApiResult<Json<Value>> {
    let (org, t) = team_by_id(&s, id).await?;
    let dn = ldap_dn(b)?;
    store_mapping(&s, "team", t.id, &dn).await?;
    crate::orgs::audit_quiet(&s.db, &org, Some(&u), "team.ldap_mapping", json!({"team": format!("{}/{}", org.login, t.slug), "ldap_dn": dn})).await;
    let mut v = super::orgs_teams::team_simple(&s, &org, &t);
    v["ldap_dn"] = json!(dn);
    v["parent"] = Value::Null;
    v["enterprise_id"] = json!(1);
    Ok(Json(v))
}

async fn record_sync(s: &AppState, kind: &str, id: i64, actor: &User) -> ApiResult<Response> {
    let dn: Option<String> = sqlx::query_scalar("SELECT ldap_dn FROM ent_ldap_mappings WHERE kind = $1 AND subject_id = $2")
        .bind(kind)
        .bind(id)
        .fetch_optional(&s.db)
        .await?;
    sqlx::query("INSERT INTO ent_ldap_sync_requests (kind, subject_id, ldap_dn, requested_by) VALUES ($1, $2, $3, $4)")
        .bind(kind)
        .bind(id)
        .bind(&dn)
        .bind(actor.id)
        .execute(&s.db)
        .await?;
    Ok((StatusCode::CREATED, axum::Json(json!({ "status": "queued" }))).into_response())
}

async fn ldap_team_sync(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Response> {
    let (_, t) = team_by_id(&s, id).await?;
    record_sync(&s, "team", t.id, &u).await
}

async fn ldap_user_mapping(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>, Json(b): Json<LdapBody>) -> ApiResult<Json<Value>> {
    let user = account(&s, &login).await?;
    let dn = ldap_dn(b)?;
    store_mapping(&s, "user", user.id, &dn).await?;
    ent::audit(&s, "user.ldap_mapping", Some(&a), json!({"user": user.login, "user_id": user.id, "ldap_dn": dn})).await;
    let (public, private): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE NOT private), count(*) FILTER (WHERE private) FROM repositories WHERE owner_id = $1",
    )
    .bind(user.id)
    .fetch_one(&s.db)
    .await?;
    let mut v = crate::json::user::private(&s.api_url(), s.base_url(), &user, public, private);
    v["ldap_dn"] = json!(dn);
    v["suspended_at"] = ent::suspended_at(&s.db, user.id).await?.map(crate::json::ts).map(Value::from).unwrap_or(Value::Null);
    Ok(Json(v))
}

async fn ldap_user_sync(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    record_sync(&s, "user", user.id, &a).await
}

// ---------------------------------------------------------------------------------------------
// Organizations and users

#[derive(Deserialize)]
struct LoginBody {
    login: Option<String>,
}

fn queued(msg: &str, url: String) -> Response {
    (StatusCode::ACCEPTED, axum::Json(json!({ "message": msg, "url": url }))).into_response()
}

async fn rename_org(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(org): Path<String>, Json(b): Json<LoginBody>) -> ApiResult<Response> {
    let o = crate::orgs::org_by_login(&s, &org).await?;
    let login = b.login.ok_or_else(|| ApiError::validation("Organization", "login", "missing_field"))?;
    let o = ent::rename_org(&s, &u, &o, &login).await?;
    Ok(queued("Job queued to rename organization. It may take a few minutes to complete.", format!("{}/orgs/{}", s.api_url(), o.login)))
}

#[derive(Deserialize)]
struct CreateUserBody {
    login: Option<String>,
    email: Option<String>,
    suspended: Option<bool>,
}

async fn create_user(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Json(b): Json<CreateUserBody>) -> ApiResult<Response> {
    let login = b.login.ok_or_else(|| ApiError::validation("User", "login", "missing_field"))?;
    let u = ent::admin_create_user(&s, Some(&a), &login, b.email.as_deref(), b.suspended.unwrap_or(false)).await?;
    Ok(created(crate::json::user::simple(&s.api_url(), s.base_url(), &u)))
}

async fn rename_user(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>, Json(b): Json<LoginBody>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    let new = b.login.ok_or_else(|| ApiError::validation("User", "login", "missing_field"))?;
    let u = ent::rename_user(&s, &a, &user, &new).await?;
    Ok(queued("Job queued to rename user. It may take a few minutes to complete.", format!("{}/users/{}", s.api_url(), u.login)))
}

async fn delete_user(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    ent::delete_user(&s, &a, &user).await?;
    Ok(no_content())
}

/// `authorization` JSON for a token row (the token value only right after creation).
pub fn authorization_json(s: &AppState, t: &TokenRow, user: &User, token: Option<&str>) -> Value {
    let api = s.api_url();
    json!({
        "id": t.id,
        "url": format!("{api}/authorizations/{}", t.id),
        "scopes": t.scopes,
        "token": token.unwrap_or(""),
        "token_last_eight": t.token_last_eight,
        "hashed_token": t.token_hash,
        "app": { "client_id": "forge", "name": t.name, "url": s.base_url() },
        "note": t.name,
        "note_url": null,
        "updated_at": crate::json::ts(t.last_used_at.unwrap_or(t.created_at).max(t.created_at)),
        "created_at": crate::json::ts(t.created_at),
        "fingerprint": null,
        "user": crate::json::user::simple(&api, s.base_url(), user),
        "installation": null,
        "expires_at": t.expires_at.map(crate::json::ts),
    })
}

const TOKEN_COLS: &str = "id, user_id, name, token_hash, token_last_eight, scopes, created_at, last_used_at, expires_at, impersonation";

async fn list_tokens(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let rows: Vec<TokenRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TOKEN_COLS} FROM tokens ORDER BY id LIMIT $1 OFFSET $2")))
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM tokens").fetch_one(&s.db).await?;
    let ids: Vec<i64> = rows.iter().map(|t| t.user_id).collect();
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&ids).fetch_all(&s.db).await?;
    let items = rows
        .iter()
        .filter_map(|t| users.iter().find(|u| u.id == t.user_id).map(|u| authorization_json(&s, t, u, None)))
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn delete_token(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Response> {
    let row: Option<(i64,)> = sqlx::query_as("DELETE FROM tokens WHERE id = $1 RETURNING user_id").bind(id).fetch_optional(&s.db).await?;
    let (uid,) = row.ok_or_else(ApiError::not_found)?;
    ent::audit(&s, "oauth_access.destroy", Some(&a), json!({"user_id": uid, "token_id": id})).await;
    Ok(no_content())
}

#[derive(Deserialize)]
struct ScopesBody {
    scopes: Option<Vec<String>>,
}

async fn create_impersonation(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>, Json(b): Json<ScopesBody>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    let scopes = b.scopes.ok_or_else(|| ApiError::validation("OauthAccess", "scopes", "missing_field"))?;
    let (row, token) = ent::create_impersonation_token(&s, &a, &user, &scopes).await?;
    Ok(created(authorization_json(&s, &row, &user, Some(&token))))
}

async fn delete_impersonation(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    sqlx::query("DELETE FROM tokens WHERE user_id = $1 AND impersonation").bind(user.id).execute(&s.db).await?;
    ent::audit(&s, "oauth_access.destroy", Some(&a), json!({"user": user.login, "user_id": user.id, "impersonation": true})).await;
    Ok(no_content())
}

async fn promote(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    ent::set_site_admin(&s, &a, &user, true).await?;
    Ok(no_content())
}

async fn demote(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    ent::set_site_admin(&s, &a, &user, false).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
struct ReasonBody {
    reason: Option<String>,
}

async fn suspend(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>, Json(b): Json<ReasonBody>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    ent::set_suspended(&s, Some(&a), &user, true, b.reason.as_deref()).await?;
    Ok(no_content())
}

async fn unsuspend(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>, Json(b): Json<ReasonBody>) -> ApiResult<Response> {
    let user = account(&s, &login).await?;
    ent::set_suspended(&s, Some(&a), &user, false, b.reason.as_deref()).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Statistics (real counts)

async fn counts(s: &AppState, sql: &str, names: &[&str]) -> ApiResult<Value> {
    let row = sqlx::query(sqlx::AssertSqlSafe(sql.to_string())).fetch_one(&s.db).await?;
    let mut o = serde_json::Map::new();
    for (i, n) in names.iter().enumerate() {
        let v: i64 = sqlx::Row::try_get(&row, i)?;
        o.insert((*n).into(), json!(v));
    }
    Ok(Value::Object(o))
}

pub async fn stat(s: &AppState, kind: &str) -> ApiResult<Value> {
    match kind {
        "repos" => counts(
            s,
            "SELECT count(*), count(*) FILTER (WHERE r.fork_parent_id IS NULL), count(*) FILTER (WHERE r.fork_parent_id IS NOT NULL),
                    count(*) FILTER (WHERE o.kind = 'Organization'),
                    (SELECT count(*) FROM activity_events WHERE type = 'PushEvent'),
                    count(*) FILTER (WHERE r.has_wiki AND FALSE)
               FROM repositories r JOIN users o ON o.id = r.owner_id",
            &["total_repos", "root_repos", "fork_repos", "org_repos", "total_pushes", "total_wikis"],
        )
        .await,
        "hooks" => counts(
            s,
            "SELECT count(*), count(*) FILTER (WHERE active), count(*) FILTER (WHERE NOT active) FROM webhooks",
            &["total_hooks", "active_hooks", "inactive_hooks"],
        )
        .await,
        "pages" => Ok(json!({ "total_pages": pages_count(s).await? })),
        "orgs" => counts(
            s,
            "SELECT (SELECT count(*) FROM users WHERE kind = 'Organization'), 0::bigint,
                    (SELECT count(*) FROM teams), (SELECT count(*) FROM team_members)",
            &["total_orgs", "disabled_orgs", "total_teams", "total_team_members"],
        )
        .await,
        "users" => counts(
            s,
            "SELECT count(*), count(*) FILTER (WHERE site_admin), count(*) FILTER (WHERE suspended_at IS NOT NULL) FROM users WHERE kind = 'User'",
            &["total_users", "admin_users", "suspended_users"],
        )
        .await,
        "pulls" => counts(
            s,
            "SELECT count(*), count(*) FILTER (WHERE p.merged),
                    count(*) FILTER (WHERE NOT p.merged AND i.state = 'open' AND p.merge_base_sha IS NOT NULL),
                    count(*) FILTER (WHERE NOT p.merged AND i.state = 'open' AND p.merge_base_sha IS NULL)
               FROM pull_requests p JOIN issues i ON i.id = p.issue_id",
            &["total_pulls", "merged_pulls", "mergeable_pulls", "unmergeable_pulls"],
        )
        .await,
        "issues" => counts(
            s,
            "SELECT count(*), count(*) FILTER (WHERE state = 'open'), count(*) FILTER (WHERE state = 'closed') FROM issues WHERE NOT is_pull",
            &["total_issues", "open_issues", "closed_issues"],
        )
        .await,
        "milestones" => counts(
            s,
            "SELECT count(*), count(*) FILTER (WHERE state = 'open'), count(*) FILTER (WHERE state = 'closed') FROM milestones",
            &["total_milestones", "open_milestones", "closed_milestones"],
        )
        .await,
        "gists" => counts(
            s,
            "SELECT count(*), count(*) FILTER (WHERE NOT public), count(*) FILTER (WHERE public) FROM gists",
            &["total_gists", "private_gists", "public_gists"],
        )
        .await,
        "comments" => {
            let gist: i64 = if table_exists(s, "gist_comments").await? {
                sqlx::query_scalar("SELECT count(*) FROM gist_comments").fetch_one(&s.db).await?
            } else {
                0
            };
            let mut v = counts(
                s,
                "SELECT (SELECT count(*) FROM commit_comments),
                        (SELECT count(*) FROM issue_comments c JOIN issues i ON i.id = c.issue_id WHERE NOT i.is_pull),
                        (SELECT count(*) FROM review_comments)",
                &["total_commit_comments", "total_issue_comments", "total_pull_request_comments"],
            )
            .await?;
            v["total_gist_comments"] = json!(gist);
            Ok(v)
        }
        "security-products" => security_products(s).await,
        "all" => {
            let mut o = serde_json::Map::new();
            for k in ["repos", "hooks", "pages", "orgs", "users", "pulls", "issues", "milestones", "gists", "comments"] {
                o.insert(k.into(), Box::pin(stat(s, k)).await?);
            }
            Ok(Value::Object(o))
        }
        _ => Err(ApiError::not_found()),
    }
}

pub async fn table_exists(s: &AppState, name: &str) -> ApiResult<bool> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL").bind(format!("public.{name}")).fetch_one(&s.db).await?)
}

async fn pages_count(s: &AppState) -> ApiResult<i64> {
    if table_exists(s, "pages_sites").await? {
        Ok(sqlx::query_scalar("SELECT count(*) FROM pages_sites").fetch_one(&s.db).await?)
    } else {
        Ok(0)
    }
}

/// Distinct committers (pushers) to active repositories in the last 90 days (GHAS counting).
pub async fn active_committers(s: &AppState, owner_id: Option<i64>) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(DISTINCT e.actor_id) FROM activity_events e JOIN repositories r ON r.id = e.repo_id
          WHERE e.type = 'PushEvent' AND e.created_at > now() - interval '90 days' AND NOT r.archived
            AND ($1::bigint IS NULL OR r.owner_id = $1)",
    )
    .bind(owner_id)
    .fetch_one(&s.db)
    .await?)
}

async fn security_products(s: &AppState) -> ApiResult<Value> {
    let (total, nonarchived): (i64, i64) =
        sqlx::query_as("SELECT count(*), count(*) FILTER (WHERE NOT archived) FROM repositories").fetch_one(&s.db).await?;
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT product, count(*) FROM repo_security_products WHERE enabled GROUP BY product").fetch_all(&s.db).await?;
    let n = |p: &str| rows.iter().find(|(x, _)| x == p).map(|(_, c)| *c).unwrap_or(0);
    let committers = active_committers(s, None).await?;
    Ok(json!({
        "total_repos": total,
        "nonarchived_repos": nonarchived,
        "secret_scanning_enabled_repos": n("secret_scanning"),
        "secret_scanning_push_protection_enabled_repos": n("secret_scanning_push_protection"),
        "code_scanning_enabled_repos": n("code_scanning_default_setup"),
        "code_scanning_pr_reviews_enabled_repos": 0,
        "code_scanning_default_setup_enabled_repos": n("code_scanning_default_setup"),
        "code_scanning_default_setup_eligible_repos": nonarchived,
        "dependabot_alerts_enabled_repos": n("dependabot_alerts"),
        "dependabot_security_updates_enabled_repos": n("dependabot_security_updates"),
        "dependabot_version_updates_enabled_repos": 0,
        "advanced_security_enabled_repos": n("advanced_security"),
        "active_committers": committers,
        "purchased_committers": 0,
        "maximum_committers": committers,
        "secret_protection_licenses": 0,
        "secret_protection_active_committers": committers,
        "code_security_licenses": 0,
        "code_security_active_committers": committers,
    }))
}

async fn stats(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(kind): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(stat(&s, &kind).await?))
}

async fn license(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    let used: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'User' AND suspended_at IS NULL").fetch_one(&s.db).await?;
    let expire = chrono::Utc::now() + chrono::Duration::days(36500);
    Ok(Json(json!({
        "seats": "unlimited",
        "seats_used": used,
        "seats_available": "unlimited",
        "kind": "standard",
        "days_until_expiration": 36500,
        "expire_at": expire.format("%Y/%m/%d %H:%M:%S UTC").to_string(),
    })))
}

// ---------------------------------------------------------------------------------------------
// Announcement banner

#[derive(Deserialize)]
pub struct AnnouncementBody {
    pub announcement: Option<String>,
    pub expires_at: Option<String>,
    pub user_dismissible: Option<bool>,
}

/// The current banner (expired banners are not shown).
pub async fn announcement(s: &AppState) -> ApiResult<Value> {
    let v = ent::get_setting(&s.db, "announcement").await?.unwrap_or(Value::Null);
    let expired = v
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
        .is_some_and(|e| e < chrono::Utc::now());
    if v.is_null() || expired {
        return Ok(json!({ "announcement": null, "expires_at": null, "user_dismissible": false }));
    }
    Ok(v)
}

async fn get_announcement(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    Ok(Json(announcement(&s).await?))
}

async fn set_announcement(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<AnnouncementBody>) -> ApiResult<Json<Value>> {
    let text = b.announcement.filter(|t| !t.trim().is_empty());
    let Some(text) = text else {
        ent::delete_setting(&s.db, "announcement").await?;
        return Ok(Json(announcement(&s).await?));
    };
    let expires = match b.expires_at.as_deref().filter(|e| !e.is_empty()) {
        Some(e) => Some(
            chrono::DateTime::parse_from_rfc3339(e)
                .map_err(|_| ApiError::validation("Announcement", "expires_at", "invalid"))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    let v = json!({
        "announcement": text,
        "expires_at": expires.map(crate::json::ts),
        "user_dismissible": b.user_dismissible.unwrap_or(false),
    });
    ent::set_setting(&s.db, "announcement", &v).await?;
    ent::audit(&s, "business.set_announcement", Some(&u), json!({})).await;
    Ok(Json(v))
}

async fn remove_announcement(State(s): State<AppState>, SiteAdmin(u): SiteAdmin) -> ApiResult<Response> {
    ent::delete_setting(&s.db, "announcement").await?;
    ent::audit(&s, "business.remove_announcement", Some(&u), json!({})).await;
    Ok(no_content())
}
