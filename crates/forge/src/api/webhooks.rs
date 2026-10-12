//! Repository and organization webhooks: CRUD, config, ping/test, deliveries + redelivery.
//! Delivery itself lives in `crate::webhooks` (Postgres job queue).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;
use crate::webhooks::{DeliveryRow, HookRow};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/hooks", get(list_repo).post(create_repo_hook))
        .route("/repos/{owner}/{repo}/hooks/{hook_id}", get(get_repo_hook).patch(update_repo_hook).delete(delete_repo_hook))
        .route("/repos/{owner}/{repo}/hooks/{hook_id}/config", get(get_repo_config).patch(update_repo_config))
        .route("/repos/{owner}/{repo}/hooks/{hook_id}/pings", post(ping_repo_hook))
        .route("/repos/{owner}/{repo}/hooks/{hook_id}/tests", post(test_repo_hook))
        .route("/repos/{owner}/{repo}/hooks/{hook_id}/deliveries", get(list_repo_deliveries))
        .route("/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}", get(get_repo_delivery))
        .route("/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}/attempts", post(redeliver_repo))
        .route("/orgs/{org}/hooks", get(list_org).post(create_org_hook))
        .route("/orgs/{org}/hooks/{hook_id}", get(get_org_hook).patch(update_org_hook).delete(delete_org_hook))
        .route("/orgs/{org}/hooks/{hook_id}/config", get(get_org_config).patch(update_org_config))
        .route("/orgs/{org}/hooks/{hook_id}/pings", post(ping_org_hook))
        .route("/orgs/{org}/hooks/{hook_id}/deliveries", get(list_org_deliveries))
        .route("/orgs/{org}/hooks/{hook_id}/deliveries/{delivery_id}", get(get_org_delivery))
        .route("/orgs/{org}/hooks/{hook_id}/deliveries/{delivery_id}/attempts", post(redeliver_org))
}

/// Where a hook is installed.
#[derive(Clone)]
pub enum Scope {
    Repo(RepoCtx),
    Org(User),
}

impl Scope {
    fn base(&self, api: &str) -> String {
        match self {
            Scope::Repo(c) => format!("{api}/repos/{}/hooks", c.full_name()),
            Scope::Org(o) => format!("{api}/orgs/{}/hooks", o.login),
        }
    }
}

/// `webhook-config` with the secret masked like GitHub does.
pub fn config_json(cfg: &Value) -> Value {
    let mut out = Map::new();
    out.insert("url".into(), cfg.get("url").cloned().unwrap_or(Value::Null));
    out.insert("content_type".into(), cfg.get("content_type").cloned().unwrap_or(json!("form")));
    let insecure = match cfg.get("insecure_ssl") {
        Some(Value::String(s)) => json!(s),
        Some(Value::Number(n)) => json!(n.to_string()),
        Some(Value::Bool(b)) => json!(if *b { "1" } else { "0" }),
        _ => json!("0"),
    };
    out.insert("insecure_ssl".into(), insecure);
    if cfg.get("secret").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
        out.insert("secret".into(), json!("********"));
    }
    if out.get("url").is_some_and(Value::is_null) {
        out.remove("url");
    }
    Value::Object(out)
}

pub fn hook_json(api: &str, scope: &Scope, h: &HookRow) -> Value {
    let url = format!("{}/{}", scope.base(api), h.id);
    let mut v = json!({
        "type": if matches!(scope, Scope::Org(_)) { "Organization" } else { "Repository" },
        "id": h.id,
        "name": h.name,
        "active": h.active,
        "events": h.events,
        "config": config_json(&h.config),
        "updated_at": crate::json::ts(h.updated_at),
        "created_at": crate::json::ts(h.created_at),
        "url": url,
        "ping_url": format!("{url}/pings"),
        "deliveries_url": format!("{url}/deliveries"),
    });
    if matches!(scope, Scope::Repo(_)) {
        v["test_url"] = json!(format!("{url}/tests"));
        v["last_response"] = h.last_response.clone();
    }
    v
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
        "installation_id": null,
        "repository_id": d.repo_id,
        "throttled_at": null,
    })
}

fn delivery_full(api: &str, scope: &Scope, d: &DeliveryRow) -> Value {
    let mut v = delivery_item(d);
    v["url"] = json!(format!("{}/{}/deliveries/{}", scope.base(api), d.hook_id, d.id));
    v["request"] = json!({ "headers": d.request_headers, "payload": d.payload });
    v["response"] = json!({ "headers": d.response_headers, "payload": d.response_body });
    v
}

// ---------------------------------------------------------------------------------------------
// Service layer

#[derive(Deserialize, Default)]
pub struct CreateHook {
    pub name: Option<String>,
    pub config: Option<Value>,
    pub events: Option<Vec<String>>,
    pub active: Option<bool>,
}

#[derive(Deserialize, Default)]
pub struct UpdateHook {
    pub config: Option<Value>,
    pub events: Option<Vec<String>>,
    pub add_events: Option<Vec<String>>,
    pub remove_events: Option<Vec<String>>,
    pub active: Option<bool>,
}

fn validate_config(cfg: &Value) -> ApiResult<()> {
    let url = cfg.get("url").and_then(Value::as_str).unwrap_or("");
    if url.is_empty() {
        let mut e = ApiError::validation("Hook", "url", "custom");
        e.errors = Some(json!([{"resource": "Hook", "code": "custom", "field": "url", "message": "Config must contain URL."}]));
        return Err(e);
    }
    match url::Url::parse(url) {
        Ok(u) if u.scheme() == "http" || u.scheme() == "https" => Ok(()),
        _ => {
            let mut e = ApiError::validation("Hook", "url", "custom");
            e.errors = Some(json!([{"resource": "Hook", "code": "custom", "field": "url", "message": "url is not supported because it isn't reachable over the public Internet"}]));
            Err(e)
        }
    }
}

fn normalize_config(cfg: Value) -> Value {
    let mut m = match cfg {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    if let Some(Value::Number(n)) = m.get("insecure_ssl").cloned() {
        m.insert("insecure_ssl".into(), json!(n.to_string()));
    }
    m.entry("content_type").or_insert(json!("form"));
    m.entry("insecure_ssl").or_insert(json!("0"));
    Value::Object(m)
}

pub async fn create_hook(s: &AppState, scope: &Scope, actor: &User, req: CreateHook) -> ApiResult<HookRow> {
    if let Some(n) = &req.name {
        if n != "web" {
            return Err(ApiError::validation("Hook", "name", "invalid"));
        }
    }
    let cfg = normalize_config(req.config.unwrap_or(Value::Null));
    validate_config(&cfg)?;
    let events = req.events.filter(|e| !e.is_empty()).unwrap_or_else(|| vec!["push".into()]);
    let (repo_id, org_id) = match scope {
        Scope::Repo(c) => (Some(c.repo.id), None),
        Scope::Org(o) => (None, Some(o.id)),
    };
    let h: HookRow = sqlx::query_as(
        "INSERT INTO webhooks (repo_id, org_id, config, events, active) VALUES ($1,$2,$3,$4,$5) RETURNING *",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(&cfg)
    .bind(&events)
    .bind(req.active.unwrap_or(true))
    .fetch_one(&s.db)
    .await?;
    // GitHub pings a new hook right away.
    ping(s, scope, actor, &h).await?;
    Ok(h)
}

pub async fn update_hook(s: &AppState, h: &HookRow, req: UpdateHook) -> ApiResult<HookRow> {
    let cfg = match req.config {
        Some(new) => {
            let mut merged = h.config.as_object().cloned().unwrap_or_default();
            // A replaced config keeps the old secret unless a new one is given.
            let old_secret = merged.get("secret").cloned();
            merged.clear();
            if let Value::Object(n) = new {
                merged.extend(n);
            }
            if !merged.contains_key("secret") {
                if let Some(sec) = old_secret {
                    merged.insert("secret".into(), sec);
                }
            }
            let c = normalize_config(Value::Object(merged));
            validate_config(&c)?;
            c
        }
        None => h.config.clone(),
    };
    let mut events = req.events.unwrap_or_else(|| h.events.clone());
    for e in req.add_events.unwrap_or_default() {
        if !events.contains(&e) {
            events.push(e);
        }
    }
    let remove = req.remove_events.unwrap_or_default();
    events.retain(|e| !remove.contains(e));
    Ok(sqlx::query_as(
        "UPDATE webhooks SET config = $2, events = $3, active = COALESCE($4, active), updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(h.id)
    .bind(cfg)
    .bind(&events)
    .bind(req.active)
    .fetch_one(&s.db)
    .await?)
}

pub async fn update_config(s: &AppState, h: &HookRow, patch: Value) -> ApiResult<HookRow> {
    let mut merged = h.config.as_object().cloned().unwrap_or_default();
    if let Value::Object(p) = patch {
        merged.extend(p);
    }
    let cfg = normalize_config(Value::Object(merged));
    validate_config(&cfg)?;
    Ok(sqlx::query_as("UPDATE webhooks SET config = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(h.id)
        .bind(cfg)
        .fetch_one(&s.db)
        .await?)
}

const ZEN: [&str; 6] = [
    "Keep it logically awesome.",
    "Design for failure.",
    "Speak like a human.",
    "Favor focus over features.",
    "Approachable is better than simple.",
    "Mind your words, they are important.",
];

pub async fn ping(s: &AppState, scope: &Scope, actor: &User, h: &HookRow) -> ApiResult<i64> {
    let api = s.api_url();
    let mut payload = json!({
        "zen": ZEN[(h.id as usize) % ZEN.len()],
        "hook_id": h.id,
        "hook": hook_json(&api, scope, h),
        "sender": crate::json::user::simple(&api, s.base_url(), actor),
    });
    let repo_id = match scope {
        Scope::Repo(c) => {
            payload["repository"] = crate::webhooks::repo_payload(s, c);
            if c.owner.kind == "Organization" {
                payload["organization"] = crate::json::user::org_simple(&api, s.base_url(), &c.owner);
            }
            Some(c.repo.id)
        }
        Scope::Org(o) => {
            payload["organization"] = crate::json::user::org_simple(&api, s.base_url(), o);
            None
        }
    };
    Ok(crate::webhooks::create_delivery(s, h, "ping", repo_id, payload, false).await?)
}

// ---------------------------------------------------------------------------------------------
// Handlers

async fn repo_scope(s: &AppState, u: &User, owner: &str, repo: &str) -> ApiResult<Scope> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    ctx.require(Perm::Admin)?;
    Ok(Scope::Repo(ctx))
}

async fn org_scope(s: &AppState, u: &User, org: &str) -> ApiResult<Scope> {
    let o = db::user_by_login(&s.db, org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
        .bind(o.id)
        .bind(u.id)
        .fetch_optional(&s.db)
        .await?;
    if role.as_deref() != Some("admin") && !u.site_admin {
        return Err(ApiError::not_found());
    }
    Ok(Scope::Org(o))
}

async fn load_hook(s: &AppState, scope: &Scope, id: i64) -> ApiResult<HookRow> {
    let (repo_id, org_id) = match scope {
        Scope::Repo(c) => (Some(c.repo.id), None),
        Scope::Org(o) => (None, Some(o.id)),
    };
    sqlx::query_as::<_, HookRow>(
        "SELECT * FROM webhooks WHERE id = $1 AND (repo_id = $2 OR org_id = $3)",
    )
    .bind(id)
    .bind(repo_id)
    .bind(org_id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(ApiError::not_found)
}

async fn list_hooks(s: &AppState, uri: &OriginalUri, scope: &Scope, p: PageParams) -> ApiResult<Paged> {
    let (repo_id, org_id) = match scope {
        Scope::Repo(c) => (Some(c.repo.id), None),
        Scope::Org(o) => (None, Some(o.id)),
    };
    let rows: Vec<HookRow> = sqlx::query_as(
        "SELECT * FROM webhooks WHERE (repo_id = $1 OR org_id = $2) ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM webhooks WHERE (repo_id = $1 OR org_id = $2)")
        .bind(repo_id)
        .bind(org_id)
        .fetch_one(&s.db)
        .await?;
    let api = s.api_url();
    Ok(Paged::new(uri, s, p, Some(total), rows.iter().map(|h| hook_json(&api, scope, h)).collect()))
}

#[derive(Deserialize)]
struct DeliveryQuery {
    per_page: Option<i64>,
    cursor: Option<String>,
}

async fn list_deliveries(s: &AppState, scope: &Scope, hook_id: i64, q: DeliveryQuery) -> ApiResult<Response> {
    let h = load_hook(s, scope, hook_id).await?;
    let per = q.per_page.unwrap_or(30).clamp(1, 100);
    let before: i64 = q.cursor.as_deref().and_then(|c| c.parse().ok()).unwrap_or(i64::MAX);
    let rows: Vec<DeliveryRow> = sqlx::query_as(
        "SELECT * FROM webhook_deliveries WHERE hook_id = $1 AND id < $2 ORDER BY id DESC LIMIT $3",
    )
    .bind(h.id)
    .bind(before)
    .bind(per)
    .fetch_all(&s.db)
    .await?;
    let mut res = axum::Json(Value::Array(rows.iter().map(delivery_item).collect())).into_response();
    if rows.len() as i64 == per {
        let last = rows.last().unwrap().id;
        let link = format!("<{}/{}/deliveries?per_page={per}&cursor={last}>; rel=\"next\"", scope.base(&s.api_url()), h.id);
        if let Ok(v) = link.parse() {
            res.headers_mut().insert(axum::http::header::LINK, v);
        }
    }
    Ok(res)
}

async fn get_delivery(s: &AppState, scope: &Scope, hook_id: i64, delivery_id: i64) -> ApiResult<Json<Value>> {
    let h = load_hook(s, scope, hook_id).await?;
    let d: DeliveryRow = sqlx::query_as("SELECT * FROM webhook_deliveries WHERE id = $1 AND hook_id = $2")
        .bind(delivery_id)
        .bind(h.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(delivery_full(&s.api_url(), scope, &d)))
}

async fn redeliver(s: &AppState, scope: &Scope, hook_id: i64, delivery_id: i64) -> ApiResult<Response> {
    let h = load_hook(s, scope, hook_id).await?;
    let d: DeliveryRow = sqlx::query_as("SELECT * FROM webhook_deliveries WHERE id = $1 AND hook_id = $2")
        .bind(delivery_id)
        .bind(h.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    crate::webhooks::create_delivery(s, &h, &d.event, d.repo_id, d.payload.clone(), true).await?;
    Ok((StatusCode::ACCEPTED, axum::Json(json!({}))).into_response())
}

async fn list_repo(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    list_hooks(&s, &uri, &scope, p).await
}

async fn create_repo_hook(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<CreateHook>,
) -> ApiResult<Response> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = create_hook(&s, &scope, &u, req).await?;
    Ok(created(hook_json(&s.api_url(), &scope, &h)))
}

async fn get_repo_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = load_hook(&s, &scope, id).await?;
    Ok(Json(hook_json(&s.api_url(), &scope, &h)))
}

async fn update_repo_hook(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(req): Json<UpdateHook>,
) -> ApiResult<Json<Value>> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = load_hook(&s, &scope, id).await?;
    let h = update_hook(&s, &h, req).await?;
    Ok(Json(hook_json(&s.api_url(), &scope, &h)))
}

async fn delete_repo_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = load_hook(&s, &scope, id).await?;
    sqlx::query("DELETE FROM webhooks WHERE id = $1").bind(h.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn get_repo_config(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = load_hook(&s, &scope, id).await?;
    Ok(Json(config_json(&h.config)))
}

async fn update_repo_config(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Value>> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = load_hook(&s, &scope, id).await?;
    let h = update_config(&s, &h, patch).await?;
    Ok(Json(config_json(&h.config)))
}

async fn ping_repo_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = load_hook(&s, &scope, id).await?;
    ping(&s, &scope, &u, &h).await?;
    Ok(no_content())
}

/// Triggers a push event for the head of the default branch (GitHub's "test push").
async fn test_repo_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    let h = load_hook(&s, &scope, id).await?;
    let Scope::Repo(ctx) = &scope else { return Err(ApiError::not_found()) };
    if !h.events.iter().any(|e| e == "push" || e == "*") {
        return Ok(no_content());
    }
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    let refname = format!("refs/heads/{}", ctx.repo.default_branch);
    let Some(sha) = refs.get(&refname) else { return Ok(no_content()) };
    let api = s.api_url();
    let full = ctx.full_name();
    let payload = json!({
        "ref": refname, "before": sha, "after": sha, "created": false, "deleted": false, "forced": false,
        "base_ref": null, "compare": format!("{}/{full}/commit/{sha}", s.base_url()),
        "commits": [], "head_commit": null,
        "pusher": {"name": u.login, "email": u.email},
        "sender": crate::json::user::simple(&api, s.base_url(), &u),
        "repository": crate::webhooks::repo_payload(&s, ctx),
    });
    crate::webhooks::create_delivery(&s, &h, "push", Some(ctx.repo.id), payload, false).await?;
    Ok(no_content())
}

async fn list_repo_deliveries(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Query(q): Query<DeliveryQuery>,
) -> ApiResult<Response> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    list_deliveries(&s, &scope, id, q).await
}

async fn get_repo_delivery(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id, did)): Path<(String, String, i64, i64)>,
) -> ApiResult<Json<Value>> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    get_delivery(&s, &scope, id, did).await
}

async fn redeliver_repo(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id, did)): Path<(String, String, i64, i64)>,
) -> ApiResult<Response> {
    let scope = repo_scope(&s, &u, &owner, &repo).await?;
    redeliver(&s, &scope, id, did).await
}

async fn list_org(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(u): AuthUser,
    Path(org): Path<String>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let scope = org_scope(&s, &u, &org).await?;
    list_hooks(&s, &uri, &scope, p).await
}

async fn create_org_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(req): Json<CreateHook>) -> ApiResult<Response> {
    let scope = org_scope(&s, &u, &org).await?;
    let h = create_hook(&s, &scope, &u, req).await?;
    Ok(created(hook_json(&s.api_url(), &scope, &h)))
}

async fn get_org_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let scope = org_scope(&s, &u, &org).await?;
    let h = load_hook(&s, &scope, id).await?;
    Ok(Json(hook_json(&s.api_url(), &scope, &h)))
}

async fn update_org_hook(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, id)): Path<(String, i64)>,
    Json(req): Json<UpdateHook>,
) -> ApiResult<Json<Value>> {
    let scope = org_scope(&s, &u, &org).await?;
    let h = load_hook(&s, &scope, id).await?;
    let h = update_hook(&s, &h, req).await?;
    Ok(Json(hook_json(&s.api_url(), &scope, &h)))
}

async fn delete_org_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let scope = org_scope(&s, &u, &org).await?;
    let h = load_hook(&s, &scope, id).await?;
    sqlx::query("DELETE FROM webhooks WHERE id = $1").bind(h.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn get_org_config(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let scope = org_scope(&s, &u, &org).await?;
    let h = load_hook(&s, &scope, id).await?;
    Ok(Json(config_json(&h.config)))
}

async fn update_org_config(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, id)): Path<(String, i64)>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Value>> {
    let scope = org_scope(&s, &u, &org).await?;
    let h = load_hook(&s, &scope, id).await?;
    let h = update_config(&s, &h, patch).await?;
    Ok(Json(config_json(&h.config)))
}

async fn ping_org_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let scope = org_scope(&s, &u, &org).await?;
    let h = load_hook(&s, &scope, id).await?;
    ping(&s, &scope, &u, &h).await?;
    Ok(no_content())
}

async fn list_org_deliveries(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, id)): Path<(String, i64)>,
    Query(q): Query<DeliveryQuery>,
) -> ApiResult<Response> {
    let scope = org_scope(&s, &u, &org).await?;
    list_deliveries(&s, &scope, id, q).await
}

async fn get_org_delivery(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id, did)): Path<(String, i64, i64)>) -> ApiResult<Json<Value>> {
    let scope = org_scope(&s, &u, &org).await?;
    get_delivery(&s, &scope, id, did).await
}

async fn redeliver_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id, did)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    let scope = org_scope(&s, &u, &org).await?;
    redeliver(&s, &scope, id, did).await
}
