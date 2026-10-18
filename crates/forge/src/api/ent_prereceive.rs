//! Pre-receive environments and hooks (GHES `enterprise-admin`): global configuration by site
//! admins, enforcement overrides by organization owners and repository admins. Enforcement at
//! push time lives in `crate::enterprise::prereceive`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::enterprise::prereceive::{self as pr, EnvRow, HookRow};
use crate::enterprise::{self as ent, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/pre-receive-environments", get(list_envs).post(create_env))
        .route("/admin/pre-receive-environments/{id}", get(get_env).patch(update_env).delete(delete_env))
        .route("/admin/pre-receive-environments/{id}/downloads", post(start_download))
        .route("/admin/pre-receive-environments/{id}/downloads/latest", get(download_status))
        .route("/admin/pre-receive-hooks", get(list_hooks).post(create_hook))
        .route("/admin/pre-receive-hooks/{id}", get(get_hook).patch(update_hook).delete(delete_hook))
        .route("/orgs/{org}/pre-receive-hooks", get(list_org_hooks))
        .route("/orgs/{org}/pre-receive-hooks/{id}", get(get_org_hook).patch(update_org_hook).delete(remove_org_hook))
        .route("/repos/{owner}/{repo}/pre-receive-hooks", get(list_repo_hooks))
        .route("/repos/{owner}/{repo}/pre-receive-hooks/{id}", get(get_repo_hook).patch(update_repo_hook).delete(remove_repo_hook))
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    direction: Option<String>,
    sort: Option<String>,
}

impl ListQuery {
    fn page(&self) -> PageParams {
        PageParams { page: self.page, per_page: self.per_page }
    }
    /// ORDER BY clause from `sort` (created|updated|name) and `direction`.
    fn order(&self, has_updated: bool) -> String {
        let col = match self.sort.as_deref() {
            Some("name") => "lower(name)",
            Some("updated") if has_updated => "updated_at",
            _ => "created_at",
        };
        let default_desc = self.sort.as_deref() != Some("name");
        let dir = match self.direction.as_deref() {
            Some("asc") => "ASC",
            Some("desc") => "DESC",
            _ if default_desc => "DESC",
            _ => "ASC",
        };
        format!("{col} {dir}, id {dir}")
    }
}

// ---------------------------------------------------------------------------------------------
// Environments

#[derive(Deserialize)]
struct EnvBody {
    name: Option<String>,
    image_url: Option<String>,
}

fn check_image_url(u: &str) -> ApiResult<()> {
    match url::Url::parse(u) {
        Ok(p) if matches!(p.scheme(), "http" | "https") => Ok(()),
        _ => Err(ApiError::validation("PreReceiveEnvironment", "image_url", "invalid")),
    }
}

fn unique_name(e: sqlx::Error, resource: &str) -> ApiError {
    match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation(resource, "name", "already_exists"),
        e => e.into(),
    }
}

async fn list_envs(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let p = q.page();
    let rows: Vec<EnvRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM ent_pre_receive_environments ORDER BY {} LIMIT $1 OFFSET $2", q.order(false))))
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_pre_receive_environments").fetch_one(&s.db).await?;
    let mut items = Vec::new();
    for e in &rows {
        items.push(pr::env_json(&s, e).await?);
    }
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create_env(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<EnvBody>) -> ApiResult<Response> {
    let name = b.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("PreReceiveEnvironment", "name", "missing_field"))?;
    let image = b.image_url.ok_or_else(|| ApiError::validation("PreReceiveEnvironment", "image_url", "missing_field"))?;
    check_image_url(&image)?;
    let e: EnvRow = sqlx::query_as("INSERT INTO ent_pre_receive_environments (name, image_url) VALUES ($1, $2) RETURNING *")
        .bind(name.trim())
        .bind(&image)
        .fetch_one(&s.db)
        .await
        .map_err(|e| unique_name(e, "PreReceiveEnvironment"))?;
    // GHES downloads a new environment right away.
    let e = pr::start_download(&s, &e).await?;
    ent::audit(&s, "pre_receive_environment.create", Some(&u), json!({"name": e.name, "environment_id": e.id})).await;
    Ok(created(pr::env_json(&s, &e).await?))
}

async fn get_env(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let e = pr::env_by_id(&s, id).await?;
    Ok(Json(pr::env_json(&s, &e).await?))
}

async fn update_env(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>, Json(b): Json<EnvBody>) -> ApiResult<Json<Value>> {
    let e = pr::env_by_id(&s, id).await?;
    if e.default_environment {
        return Err(ApiError::unprocessable("Cannot modify or delete the default environment"));
    }
    if let Some(i) = &b.image_url {
        check_image_url(i)?;
    }
    let image_changed = b.image_url.as_ref().is_some_and(|i| *i != e.image_url);
    let e: EnvRow = sqlx::query_as(
        "UPDATE ent_pre_receive_environments SET name = COALESCE($2, name), image_url = COALESCE($3, image_url) WHERE id = $1 RETURNING *",
    )
    .bind(e.id)
    .bind(b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
    .bind(&b.image_url)
    .fetch_one(&s.db)
    .await
    .map_err(|e| unique_name(e, "PreReceiveEnvironment"))?;
    let e = if image_changed && e.download_state != "in_progress" { pr::start_download(&s, &e).await? } else { e };
    ent::audit(&s, "pre_receive_environment.update", Some(&u), json!({"name": e.name, "environment_id": e.id})).await;
    Ok(Json(pr::env_json(&s, &e).await?))
}

async fn delete_env(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Response> {
    let e = pr::env_by_id(&s, id).await?;
    if e.default_environment {
        return Err(ApiError::unprocessable("Cannot modify or delete the default environment"));
    }
    let used: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_pre_receive_hooks WHERE environment_id = $1").bind(e.id).fetch_one(&s.db).await?;
    if used > 0 {
        return Err(ApiError::unprocessable("Cannot delete environment that has hooks"));
    }
    if e.download_state == "in_progress" {
        return Err(ApiError::unprocessable("Cannot delete environment when download is in progress"));
    }
    sqlx::query("DELETE FROM ent_pre_receive_environments WHERE id = $1").bind(e.id).execute(&s.db).await?;
    if let Some(k) = &e.object_key {
        let _ = object_store::ObjectStoreExt::delete(&*s.store, &object_store::path::Path::from(k.clone())).await;
    }
    ent::audit(&s, "pre_receive_environment.destroy", Some(&u), json!({"name": e.name, "environment_id": e.id})).await;
    Ok(no_content())
}

async fn start_download(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Response> {
    let e = pr::env_by_id(&s, id).await?;
    let e = pr::start_download(&s, &e).await?;
    ent::audit(&s, "pre_receive_environment.download", Some(&u), json!({"name": e.name, "environment_id": e.id})).await;
    Ok((StatusCode::ACCEPTED, axum::Json(pr::download_json(&s.api_url(), &e))).into_response())
}

async fn download_status(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let e = pr::env_by_id(&s, id).await?;
    Ok(Json(pr::download_json(&s.api_url(), &e)))
}

// ---------------------------------------------------------------------------------------------
// Hooks (global)

#[derive(Deserialize)]
struct HookBody {
    name: Option<String>,
    script: Option<String>,
    script_repository: Option<Value>,
    environment: Option<Value>,
    enforcement: Option<String>,
    allow_downstream_configuration: Option<bool>,
}

async fn resolve_script_repo(s: &AppState, v: &Value) -> ApiResult<i64> {
    let bad = || ApiError::validation("PreReceiveHook", "script_repository", "invalid");
    if let Some(id) = v.get("id").and_then(Value::as_i64) {
        return Ok(db::repo_by_id(&s.db, id).await.map_err(|_| bad())?.id);
    }
    let full = v.get("full_name").and_then(Value::as_str).ok_or_else(bad)?;
    let (o, r) = full.split_once('/').ok_or_else(bad)?;
    let owner = db::user_by_login(&s.db, o).await.map_err(|_| bad())?;
    sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1 AND lower(name) = lower($2)")
        .bind(owner.id)
        .bind(r.strip_suffix(".git").unwrap_or(r))
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(bad)
}

async fn resolve_env(s: &AppState, v: &Value) -> ApiResult<i64> {
    let bad = || ApiError::validation("PreReceiveHook", "environment", "invalid");
    let id = v.get("id").and_then(Value::as_i64).ok_or_else(bad)?;
    Ok(pr::env_by_id(s, id).await.map_err(|_| bad())?.id)
}

fn check_enforcement(e: &Option<String>) -> ApiResult<()> {
    match e {
        Some(x) if !pr::valid_enforcement(x) => Err(ApiError::validation("PreReceiveHook", "enforcement", "invalid")),
        _ => Ok(()),
    }
}

async fn list_hooks(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let p = q.page();
    let rows: Vec<HookRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM ent_pre_receive_hooks ORDER BY {} LIMIT $1 OFFSET $2", q.order(true))))
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_pre_receive_hooks").fetch_one(&s.db).await?;
    let mut items = Vec::new();
    for h in &rows {
        items.push(pr::hook_json(&s, h).await?);
    }
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create_hook(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<HookBody>) -> ApiResult<Response> {
    let name = b.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("PreReceiveHook", "name", "missing_field"))?;
    let script = b.script.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("PreReceiveHook", "script", "missing_field"))?;
    let repo_id = resolve_script_repo(&s, b.script_repository.as_ref().ok_or_else(|| ApiError::validation("PreReceiveHook", "script_repository", "missing_field"))?).await?;
    let env_id = resolve_env(&s, b.environment.as_ref().ok_or_else(|| ApiError::validation("PreReceiveHook", "environment", "missing_field"))?).await?;
    check_enforcement(&b.enforcement)?;
    let h: HookRow = sqlx::query_as(
        "INSERT INTO ent_pre_receive_hooks (name, script, script_repo_id, environment_id, enforcement, allow_downstream_configuration)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(name.trim())
    .bind(script.trim())
    .bind(repo_id)
    .bind(env_id)
    .bind(b.enforcement.as_deref().unwrap_or("disabled"))
    .bind(b.allow_downstream_configuration.unwrap_or(false))
    .fetch_one(&s.db)
    .await
    .map_err(|e| unique_name(e, "PreReceiveHook"))?;
    ent::audit(&s, "pre_receive_hook.create", Some(&u), json!({"name": h.name, "hook_id": h.id, "enforcement": h.enforcement})).await;
    Ok(created(pr::hook_json(&s, &h).await?))
}

async fn get_hook(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let h = pr::hook_by_id(&s, id).await?;
    Ok(Json(pr::hook_json(&s, &h).await?))
}

async fn update_hook(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>, Json(b): Json<HookBody>) -> ApiResult<Json<Value>> {
    let h = pr::hook_by_id(&s, id).await?;
    check_enforcement(&b.enforcement)?;
    let repo_id = match &b.script_repository {
        Some(v) => resolve_script_repo(&s, v).await?,
        None => h.script_repo_id,
    };
    let env_id = match &b.environment {
        Some(v) => resolve_env(&s, v).await?,
        None => h.environment_id,
    };
    let h: HookRow = sqlx::query_as(
        "UPDATE ent_pre_receive_hooks SET name = COALESCE($2, name), script = COALESCE($3, script), script_repo_id = $4,
            environment_id = $5, enforcement = COALESCE($6, enforcement),
            allow_downstream_configuration = COALESCE($7, allow_downstream_configuration), updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(h.id)
    .bind(b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
    .bind(b.script.as_deref().map(str::trim).filter(|n| !n.is_empty()))
    .bind(repo_id)
    .bind(env_id)
    .bind(&b.enforcement)
    .bind(b.allow_downstream_configuration)
    .fetch_one(&s.db)
    .await
    .map_err(|e| unique_name(e, "PreReceiveHook"))?;
    ent::audit(&s, "pre_receive_hook.update", Some(&u), json!({"name": h.name, "hook_id": h.id, "enforcement": h.enforcement})).await;
    Ok(Json(pr::hook_json(&s, &h).await?))
}

async fn delete_hook(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(id): Path<i64>) -> ApiResult<Response> {
    let h = pr::hook_by_id(&s, id).await?;
    sqlx::query("DELETE FROM ent_pre_receive_hooks WHERE id = $1").bind(h.id).execute(&s.db).await?;
    ent::audit(&s, "pre_receive_hook.destroy", Some(&u), json!({"name": h.name, "hook_id": h.id})).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Organization level (org owners and site admins)

fn config_url(s: &AppState, eff: &pr::Effective, h: &HookRow, org: Option<&User>, repo: Option<&RepoCtx>) -> String {
    let api = s.api_url();
    match (eff.level, org, repo) {
        ("repo", _, Some(r)) => format!("{api}/repos/{}/pre-receive-hooks/{}", r.full_name(), h.id),
        ("org", Some(o), _) => format!("{api}/orgs/{}/pre-receive-hooks/{}", o.login, h.id),
        _ => format!("{api}/admin/pre-receive-hooks/{}", h.id),
    }
}

async fn org_hook_json(s: &AppState, org: &User, h: &HookRow) -> ApiResult<Value> {
    let eff = pr::effective_org(s, h, Some(org.id)).await?;
    Ok(json!({
        "id": h.id,
        "name": h.name,
        "enforcement": eff.enforcement,
        "configuration_url": config_url(s, &eff, h, Some(org), None),
        "allow_downstream_configuration": eff.allow_downstream,
    }))
}

async fn org_admin(s: &AppState, u: &User, org: &str) -> ApiResult<User> {
    let o = crate::orgs::org_by_login(s, org).await?;
    crate::orgs::require_owner(s, &o, u).await?;
    Ok(o)
}

async fn hooks_page(s: &AppState, q: &ListQuery) -> ApiResult<(Vec<HookRow>, i64)> {
    let p = q.page();
    let rows: Vec<HookRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM ent_pre_receive_hooks ORDER BY {} LIMIT $1 OFFSET $2", q.order(true))))
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_pre_receive_hooks").fetch_one(&s.db).await?;
    Ok((rows, total))
}

async fn list_org_hooks(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let o = org_admin(&s, &u, &org).await?;
    let (rows, total) = hooks_page(&s, &q).await?;
    let mut items = Vec::new();
    for h in &rows {
        items.push(org_hook_json(&s, &o, h).await?);
    }
    Ok(Paged::new(&uri, &s, q.page(), Some(total), items))
}

async fn get_org_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let o = org_admin(&s, &u, &org).await?;
    let h = pr::hook_by_id(&s, id).await?;
    Ok(Json(org_hook_json(&s, &o, &h).await?))
}

#[derive(Deserialize)]
struct EnforcementBody {
    enforcement: Option<String>,
    allow_downstream_configuration: Option<bool>,
}

fn not_configurable() -> ApiError {
    ApiError::unprocessable("This pre-receive hook does not allow downstream configuration.")
}

async fn update_org_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Json(b): Json<EnforcementBody>) -> ApiResult<Json<Value>> {
    let o = org_admin(&s, &u, &org).await?;
    let h = pr::hook_by_id(&s, id).await?;
    if !h.allow_downstream_configuration {
        return Err(not_configurable());
    }
    check_enforcement(&b.enforcement)?;
    let cur = pr::effective_org(&s, &h, Some(o.id)).await?;
    sqlx::query(
        "INSERT INTO ent_pre_receive_org_settings (hook_id, org_id, enforcement, allow_downstream_configuration) VALUES ($1, $2, $3, $4)
         ON CONFLICT (hook_id, org_id) DO UPDATE SET enforcement = $3, allow_downstream_configuration = $4",
    )
    .bind(h.id)
    .bind(o.id)
    .bind(b.enforcement.unwrap_or(cur.enforcement))
    .bind(b.allow_downstream_configuration.unwrap_or(cur.allow_downstream))
    .execute(&s.db)
    .await?;
    crate::orgs::audit_quiet(&s.db, &o, Some(&u), "pre_receive_hook.update_enforcement", json!({"hook_id": h.id, "name": h.name})).await;
    Ok(Json(org_hook_json(&s, &o, &h).await?))
}

async fn remove_org_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let o = org_admin(&s, &u, &org).await?;
    let h = pr::hook_by_id(&s, id).await?;
    sqlx::query("DELETE FROM ent_pre_receive_org_settings WHERE hook_id = $1 AND org_id = $2").bind(h.id).bind(o.id).execute(&s.db).await?;
    crate::orgs::audit_quiet(&s.db, &o, Some(&u), "pre_receive_hook.remove_enforcement", json!({"hook_id": h.id, "name": h.name})).await;
    Ok(Json(org_hook_json(&s, &o, &h).await?))
}

// ---------------------------------------------------------------------------------------------
// Repository level (repository admins)

async fn repo_hook_json(s: &AppState, ctx: &RepoCtx, h: &HookRow) -> ApiResult<Value> {
    let eff = pr::effective_repo(s, h, ctx).await?;
    let org = (ctx.owner.kind == "Organization").then_some(&ctx.owner);
    Ok(json!({
        "id": h.id,
        "name": h.name,
        "enforcement": eff.enforcement,
        "configuration_url": config_url(s, &eff, h, org, Some(ctx)),
    }))
}

async fn repo_admin(s: &AppState, u: &User, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    ctx.require(Perm::Admin)?;
    Ok(ctx)
}

async fn list_repo_hooks(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    uri: OriginalUri,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let ctx = repo_admin(&s, &u, &owner, &repo).await?;
    let (rows, total) = hooks_page(&s, &q).await?;
    let mut items = Vec::new();
    for h in &rows {
        items.push(repo_hook_json(&s, &ctx, h).await?);
    }
    Ok(Paged::new(&uri, &s, q.page(), Some(total), items))
}

async fn get_repo_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = repo_admin(&s, &u, &owner, &repo).await?;
    let h = pr::hook_by_id(&s, id).await?;
    Ok(Json(repo_hook_json(&s, &ctx, &h).await?))
}

async fn update_repo_hook(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(b): Json<EnforcementBody>,
) -> ApiResult<Json<Value>> {
    let ctx = repo_admin(&s, &u, &owner, &repo).await?;
    let h = pr::hook_by_id(&s, id).await?;
    let org_id = (ctx.owner.kind == "Organization").then_some(ctx.owner.id);
    if !pr::effective_org(&s, &h, org_id).await?.allow_downstream {
        return Err(not_configurable());
    }
    check_enforcement(&b.enforcement)?;
    if let Some(e) = &b.enforcement {
        sqlx::query(
            "INSERT INTO ent_pre_receive_repo_settings (hook_id, repo_id, enforcement) VALUES ($1, $2, $3)
             ON CONFLICT (hook_id, repo_id) DO UPDATE SET enforcement = $3",
        )
        .bind(h.id)
        .bind(ctx.repo.id)
        .bind(e)
        .execute(&s.db)
        .await?;
    }
    Ok(Json(repo_hook_json(&s, &ctx, &h).await?))
}

async fn remove_repo_hook(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = repo_admin(&s, &u, &owner, &repo).await?;
    let h = pr::hook_by_id(&s, id).await?;
    sqlx::query("DELETE FROM ent_pre_receive_repo_settings WHERE hook_id = $1 AND repo_id = $2").bind(h.id).bind(ctx.repo.id).execute(&s.db).await?;
    Ok(Json(repo_hook_json(&s, &ctx, &h).await?))
}
