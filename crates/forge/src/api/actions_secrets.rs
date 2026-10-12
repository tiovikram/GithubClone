//! GitHub Actions secrets and variables for repositories, repository environments and
//! organizations.
//!
//! Secrets arrive already encrypted by the client (libsodium sealed box against the owner's
//! public key from `.../secrets/public-key`); forge stores the ciphertext and never returns it.
//! forge has no runner that would consume secrets, so it never needs the private half: the
//! public key is 32 random bytes generated once per owner, which is all a client needs.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::plat::total_envelope;
use super::{no_content, PageParams};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let repo = "/repos/{owner}/{repo}/actions";
    let env = "/repos/{owner}/{repo}/environments/{env}";
    let env_by_id = "/repositories/{id}/environments/{env}";
    let org = "/orgs/{org}/actions";
    Router::new()
        .route(&format!("{repo}/secrets"), get(repo_list_secrets))
        .route(&format!("{repo}/secrets/public-key"), get(repo_public_key))
        .route(&format!("{repo}/secrets/{{name}}"), get(repo_get_secret).put(repo_put_secret).delete(repo_delete_secret))
        .route(&format!("{repo}/variables"), get(repo_list_variables).post(repo_create_variable))
        .route(&format!("{repo}/variables/{{name}}"), get(repo_get_variable).patch(repo_update_variable).delete(repo_delete_variable))
        .route(&format!("{env}/secrets"), get(env_list_secrets))
        .route(&format!("{env}/secrets/public-key"), get(env_public_key))
        .route(&format!("{env}/secrets/{{name}}"), get(env_get_secret).put(env_put_secret).delete(env_delete_secret))
        .route(&format!("{env}/variables"), get(env_list_variables).post(env_create_variable))
        .route(&format!("{env}/variables/{{name}}"), get(env_get_variable).patch(env_update_variable).delete(env_delete_variable))
        .route(&format!("{env_by_id}/variables"), get(envid_list_variables).post(envid_create_variable))
        .route(&format!("{env_by_id}/variables/{{name}}"), get(envid_get_variable).patch(envid_update_variable).delete(envid_delete_variable))
        .route(&format!("{org}/secrets"), get(org_list_secrets))
        .route(&format!("{org}/secrets/public-key"), get(org_public_key))
        .route(&format!("{org}/secrets/{{name}}"), get(org_get_secret).put(org_put_secret).delete(org_delete_secret))
        .route(&format!("{org}/secrets/{{name}}/repositories"), get(org_secret_repos))
        .route(&format!("{org}/variables"), get(org_list_variables).post(org_create_variable))
        .route(&format!("{org}/variables/{{name}}"), get(org_get_variable).patch(org_update_variable).delete(org_delete_variable))
        .route(&format!("{org}/variables/{{name}}/repositories"), get(org_variable_repos))
}

// ---------------------------------------------------------------------------------------------
// Owners

/// Who a secret or variable belongs to.
#[derive(Clone)]
pub enum Scope {
    Repo { ctx: Box<RepoCtx>, env: String },
    Org(User),
}

impl Scope {
    fn repo_id(&self) -> Option<i64> {
        match self {
            Scope::Repo { ctx, .. } => Some(ctx.repo.id),
            Scope::Org(_) => None,
        }
    }
    fn org_id(&self) -> Option<i64> {
        match self {
            Scope::Org(o) => Some(o.id),
            Scope::Repo { .. } => None,
        }
    }
    fn env(&self) -> &str {
        match self {
            Scope::Repo { env, .. } => env,
            Scope::Org(_) => "",
        }
    }
    /// (owner_kind, owner_id) for the public key: environments share their repository's key.
    fn key_owner(&self) -> (&'static str, i64) {
        match self {
            Scope::Repo { ctx, .. } => ("repo", ctx.repo.id),
            Scope::Org(o) => ("org", o.id),
        }
    }
    fn is_org(&self) -> bool {
        matches!(self, Scope::Org(_))
    }
    /// API URL of the collection (`.../secrets` or `.../variables` is appended by callers).
    fn api_base(&self, s: &AppState) -> String {
        match self {
            Scope::Repo { ctx, env } if env.is_empty() => format!("{}/repos/{}/actions", s.api_url(), ctx.full_name()),
            Scope::Repo { ctx, env } => format!("{}/repos/{}/environments/{}", s.api_url(), ctx.full_name(), crate::json::release::seg(env)),
            Scope::Org(o) => format!("{}/orgs/{}/actions", s.api_url(), o.login),
        }
    }
}

async fn repo_scope(s: &AppState, u: &User, owner: &str, repo: &str, env: Option<String>) -> ApiResult<Scope> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    ctx.require(Perm::Admin)?;
    Ok(Scope::Repo { ctx: Box::new(ctx), env: env.unwrap_or_default() })
}

async fn repo_id_scope(s: &AppState, u: &User, repo_id: i64, env: String) -> ApiResult<Scope> {
    let ctx = db::load_repo_by_id(&s.db, Some(u), repo_id).await?;
    ctx.require(Perm::Admin)?;
    Ok(Scope::Repo { ctx: Box::new(ctx), env })
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
    match role.as_deref() {
        Some("admin") => Ok(Scope::Org(o)),
        _ if u.site_admin => Ok(Scope::Org(o)),
        Some(_) => Err(ApiError::forbidden("You must be an org admin to manage Actions secrets and variables.")),
        None => Err(ApiError::not_found()),
    }
}

// ---------------------------------------------------------------------------------------------
// Public key

/// The owner's (key_id, base64 key), created on first use.
pub async fn public_key(s: &AppState, scope: &Scope) -> ApiResult<(String, String)> {
    let (kind, id) = scope.key_owner();
    let key: [u8; 32] = rand::random();
    let key_id = (rand::random::<u64>() >> 4).to_string();
    sqlx::query("INSERT INTO actions_public_keys (owner_kind, owner_id, key_id, key) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
        .bind(kind)
        .bind(id)
        .bind(&key_id)
        .bind(base64::engine::general_purpose::STANDARD.encode(key))
        .execute(&s.db)
        .await?;
    Ok(sqlx::query_as("SELECT key_id, key FROM actions_public_keys WHERE owner_kind = $1 AND owner_id = $2")
        .bind(kind)
        .bind(id)
        .fetch_one(&s.db)
        .await?)
}

async fn public_key_json(s: &AppState, scope: &Scope) -> ApiResult<Json<Value>> {
    let (key_id, key) = public_key(s, scope).await?;
    Ok(Json(json!({ "key_id": key_id, "key": key })))
}

// ---------------------------------------------------------------------------------------------
// Secrets

#[derive(sqlx::FromRow)]
pub struct SecretRow {
    pub name: String,
    pub visibility: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn secret_json(s: &AppState, scope: &Scope, r: &SecretRow) -> Value {
    let mut v = json!({ "name": r.name, "created_at": crate::json::ts(r.created_at), "updated_at": crate::json::ts(r.updated_at) });
    if scope.is_org() {
        v["visibility"] = json!(r.visibility.as_deref().unwrap_or("private"));
        if r.visibility.as_deref() == Some("selected") {
            v["selected_repositories_url"] = json!(format!("{}/secrets/{}/repositories", scope.api_base(s), r.name));
        }
    }
    v
}

/// GitHub's naming rules for secrets and variables.
fn valid_name(resource: &str, name: &str) -> ApiResult<()> {
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && !name.to_ascii_uppercase().starts_with("GITHUB_");
    if ok {
        Ok(())
    } else {
        let mut e = ApiError::validation(resource, "name", "invalid");
        e.errors = Some(json!([{ "resource": resource, "field": "name", "code": "invalid",
            "message": "Names can only contain alphanumeric characters or underscores, must not start with a number or GITHUB_" }]));
        Err(e)
    }
}

fn valid_visibility(scope: &Scope, v: Option<&str>) -> ApiResult<Option<String>> {
    if !scope.is_org() {
        return Ok(None);
    }
    match v.unwrap_or("private") {
        x @ ("all" | "private" | "selected") => Ok(Some(x.to_string())),
        _ => Err(ApiError::validation("Secret", "visibility", "invalid")),
    }
}

const SECRET_COLS: &str = "name, visibility, created_at, updated_at";
const OWNER_FILTER: &str = "COALESCE(repo_id, 0) = COALESCE($1, 0) AND COALESCE(org_id, 0) = COALESCE($2, 0) AND environment = $3";

pub async fn list_secrets(s: &AppState, scope: &Scope, p: PageParams) -> ApiResult<(Vec<SecretRow>, i64)> {
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM actions_secrets WHERE {OWNER_FILTER}")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .fetch_one(&s.db)
        .await?;
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SECRET_COLS} FROM actions_secrets WHERE {OWNER_FILTER} ORDER BY name LIMIT $4 OFFSET $5")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    Ok((rows, total))
}

pub async fn get_secret(s: &AppState, scope: &Scope, name: &str) -> ApiResult<SecretRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SECRET_COLS} FROM actions_secrets WHERE {OWNER_FILTER} AND upper(name) = upper($4)")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .bind(name)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
pub struct PutSecret {
    pub encrypted_value: Option<String>,
    pub key_id: Option<String>,
    pub visibility: Option<String>,
}

/// Creates or updates a secret; returns true when it was created.
pub async fn put_secret(s: &AppState, scope: &Scope, name: &str, input: PutSecret) -> ApiResult<bool> {
    valid_name("Secret", name)?;
    let value = input.encrypted_value.filter(|v| !v.is_empty()).ok_or_else(|| ApiError::validation("Secret", "encrypted_value", "missing_field"))?;
    if base64::engine::general_purpose::STANDARD.decode(&value).is_err() {
        return Err(ApiError::validation("Secret", "encrypted_value", "invalid"));
    }
    let key_id = input.key_id.filter(|k| !k.is_empty()).ok_or_else(|| ApiError::validation("Secret", "key_id", "missing_field"))?;
    let (current, _) = public_key(s, scope).await?;
    if key_id != current {
        return Err(ApiError::validation("Secret", "key_id", "invalid"));
    }
    let visibility = valid_visibility(scope, input.visibility.as_deref())?;
    let name = name.to_ascii_uppercase();
    let updated = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE actions_secrets SET encrypted_value = $5, key_id = $6, visibility = COALESCE($7, visibility), updated_at = now()
          WHERE {OWNER_FILTER} AND name = $4"
    )))
    .bind(scope.repo_id())
    .bind(scope.org_id())
    .bind(scope.env())
    .bind(&name)
    .bind(&value)
    .bind(&key_id)
    .bind(&visibility)
    .execute(&s.db)
    .await?
    .rows_affected();
    if updated > 0 {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO actions_secrets (repo_id, org_id, environment, name, encrypted_value, key_id, visibility)
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
    )
    .bind(scope.repo_id())
    .bind(scope.org_id())
    .bind(scope.env())
    .bind(&name)
    .bind(&value)
    .bind(&key_id)
    .bind(&visibility)
    .execute(&s.db)
    .await?;
    Ok(true)
}

pub async fn delete_secret(s: &AppState, scope: &Scope, name: &str) -> ApiResult<()> {
    let n = sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM actions_secrets WHERE {OWNER_FILTER} AND upper(name) = upper($4)")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .bind(name)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn list_secrets_resp(s: &AppState, scope: &Scope, p: PageParams, uri: &OriginalUri) -> ApiResult<Response> {
    let (rows, total) = list_secrets(s, scope, p).await?;
    let items = rows.iter().map(|r| secret_json(s, scope, r)).collect();
    Ok(envelope(s, uri, p, "secrets", items, total))
}

fn envelope(s: &AppState, uri: &OriginalUri, p: PageParams, key: &str, items: Vec<Value>, total: i64) -> Response {
    let links = super::Paged::new(uri, s, p, Some(total), vec![]).into_response();
    let mut res = (StatusCode::OK, Json(total_envelope(key, items, total))).into_response();
    if let Some(link) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, link.clone());
    }
    res
}

async fn put_secret_resp(s: &AppState, scope: &Scope, name: &str, body: PutSecret) -> ApiResult<Response> {
    if put_secret(s, scope, name, body).await? {
        Ok((StatusCode::CREATED, Json(json!({}))).into_response())
    } else {
        Ok(no_content())
    }
}

// ---------------------------------------------------------------------------------------------
// Variables

#[derive(sqlx::FromRow)]
pub struct VariableRow {
    pub name: String,
    pub value: String,
    pub visibility: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn variable_json(s: &AppState, scope: &Scope, r: &VariableRow) -> Value {
    let mut v = json!({
        "name": r.name, "value": r.value,
        "created_at": crate::json::ts(r.created_at), "updated_at": crate::json::ts(r.updated_at),
    });
    if scope.is_org() {
        v["visibility"] = json!(r.visibility.as_deref().unwrap_or("private"));
        if r.visibility.as_deref() == Some("selected") {
            v["selected_repositories_url"] = json!(format!("{}/variables/{}/repositories", scope.api_base(s), r.name));
        }
    }
    v
}

const VAR_COLS: &str = "name, value, visibility, created_at, updated_at";

pub async fn list_variables(s: &AppState, scope: &Scope, p: PageParams) -> ApiResult<(Vec<VariableRow>, i64)> {
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM actions_variables WHERE {OWNER_FILTER}")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .fetch_one(&s.db)
        .await?;
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {VAR_COLS} FROM actions_variables WHERE {OWNER_FILTER} ORDER BY name LIMIT $4 OFFSET $5")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    Ok((rows, total))
}

pub async fn get_variable(s: &AppState, scope: &Scope, name: &str) -> ApiResult<VariableRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {VAR_COLS} FROM actions_variables WHERE {OWNER_FILTER} AND upper(name) = upper($4)")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .bind(name)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
pub struct VariableInput {
    pub name: Option<String>,
    pub value: Option<String>,
    pub visibility: Option<String>,
}

/// Creates a variable; 409 when one with that name exists (gh then PATCHes).
pub async fn create_variable(s: &AppState, scope: &Scope, input: VariableInput) -> ApiResult<VariableRow> {
    let name = input.name.filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("Variable", "name", "missing_field"))?;
    valid_name("Variable", &name)?;
    let value = input.value.ok_or_else(|| ApiError::validation("Variable", "value", "missing_field"))?;
    let visibility = valid_visibility(scope, input.visibility.as_deref())?;
    let row: Option<VariableRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO actions_variables (repo_id, org_id, environment, name, value, visibility)
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING RETURNING {VAR_COLS}"
    )))
    .bind(scope.repo_id())
    .bind(scope.org_id())
    .bind(scope.env())
    .bind(name.to_ascii_uppercase())
    .bind(&value)
    .bind(&visibility)
    .fetch_optional(&s.db)
    .await?;
    row.ok_or_else(|| ApiError::conflict("Already exists - Variable already exists"))
}

pub async fn update_variable(s: &AppState, scope: &Scope, name: &str, input: VariableInput) -> ApiResult<()> {
    let cur = get_variable(s, scope, name).await?;
    let new_name = match input.name.filter(|n| !n.is_empty()) {
        Some(n) => {
            valid_name("Variable", &n)?;
            n.to_ascii_uppercase()
        }
        None => cur.name.clone(),
    };
    let visibility = match input.visibility.as_deref() {
        Some(v) => valid_visibility(scope, Some(v))?,
        None => cur.visibility.clone(),
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE actions_variables SET name = $5, value = $6, visibility = $7, updated_at = now() WHERE {OWNER_FILTER} AND name = $4"
    )))
    .bind(scope.repo_id())
    .bind(scope.org_id())
    .bind(scope.env())
    .bind(&cur.name)
    .bind(&new_name)
    .bind(input.value.unwrap_or(cur.value))
    .bind(&visibility)
    .execute(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::conflict("Already exists - Variable already exists"),
        e => e.into(),
    })?;
    Ok(())
}

pub async fn delete_variable(s: &AppState, scope: &Scope, name: &str) -> ApiResult<()> {
    let n = sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM actions_variables WHERE {OWNER_FILTER} AND upper(name) = upper($4)")))
        .bind(scope.repo_id())
        .bind(scope.org_id())
        .bind(scope.env())
        .bind(name)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn list_variables_resp(s: &AppState, scope: &Scope, p: PageParams, uri: &OriginalUri) -> ApiResult<Response> {
    let p = PageParams { page: p.page, per_page: Some(p.per_page.unwrap_or(10).clamp(1, 30)) };
    let (rows, total) = list_variables(s, scope, p).await?;
    let items = rows.iter().map(|r| variable_json(s, scope, r)).collect();
    Ok(envelope(s, uri, p, "variables", items, total))
}

async fn create_variable_resp(s: &AppState, scope: &Scope, body: VariableInput) -> ApiResult<Response> {
    create_variable(s, scope, body).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

// ---------------------------------------------------------------------------------------------
// Handlers

type RepoP = Path<(String, String)>;
type RepoNameP = Path<(String, String, String)>;
type EnvP = Path<(String, String, String)>;
type EnvNameP = Path<(String, String, String, String)>;

async fn repo_list_secrets(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): RepoP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = repo_scope(&s, &u, &o, &r, None).await?;
    list_secrets_resp(&s, &sc, p, &uri).await
}
async fn repo_public_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP) -> ApiResult<Json<Value>> {
    public_key_json(&s, &repo_scope(&s, &u, &o, &r, None).await?).await
}
async fn repo_get_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RepoNameP) -> ApiResult<Json<Value>> {
    let sc = repo_scope(&s, &u, &o, &r, None).await?;
    Ok(Json(secret_json(&s, &sc, &get_secret(&s, &sc, &n).await?)))
}
async fn repo_put_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RepoNameP, Json(b): Json<PutSecret>) -> ApiResult<Response> {
    put_secret_resp(&s, &repo_scope(&s, &u, &o, &r, None).await?, &n, b).await
}
async fn repo_delete_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RepoNameP) -> ApiResult<Response> {
    delete_secret(&s, &repo_scope(&s, &u, &o, &r, None).await?, &n).await?;
    Ok(no_content())
}
async fn repo_list_variables(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): RepoP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = repo_scope(&s, &u, &o, &r, None).await?;
    list_variables_resp(&s, &sc, p, &uri).await
}
async fn repo_create_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    create_variable_resp(&s, &repo_scope(&s, &u, &o, &r, None).await?, b).await
}
async fn repo_get_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RepoNameP) -> ApiResult<Json<Value>> {
    let sc = repo_scope(&s, &u, &o, &r, None).await?;
    Ok(Json(variable_json(&s, &sc, &get_variable(&s, &sc, &n).await?)))
}
async fn repo_update_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RepoNameP, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    update_variable(&s, &repo_scope(&s, &u, &o, &r, None).await?, &n, b).await?;
    Ok(no_content())
}
async fn repo_delete_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RepoNameP) -> ApiResult<Response> {
    delete_variable(&s, &repo_scope(&s, &u, &o, &r, None).await?, &n).await?;
    Ok(no_content())
}

async fn env_list_secrets(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r, e)): EnvP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = repo_scope(&s, &u, &o, &r, Some(e)).await?;
    list_secrets_resp(&s, &sc, p, &uri).await
}
async fn env_public_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e)): EnvP) -> ApiResult<Json<Value>> {
    public_key_json(&s, &repo_scope(&s, &u, &o, &r, Some(e)).await?).await
}
async fn env_get_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e, n)): EnvNameP) -> ApiResult<Json<Value>> {
    let sc = repo_scope(&s, &u, &o, &r, Some(e)).await?;
    Ok(Json(secret_json(&s, &sc, &get_secret(&s, &sc, &n).await?)))
}
async fn env_put_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e, n)): EnvNameP, Json(b): Json<PutSecret>) -> ApiResult<Response> {
    put_secret_resp(&s, &repo_scope(&s, &u, &o, &r, Some(e)).await?, &n, b).await
}
async fn env_delete_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e, n)): EnvNameP) -> ApiResult<Response> {
    delete_secret(&s, &repo_scope(&s, &u, &o, &r, Some(e)).await?, &n).await?;
    Ok(no_content())
}
async fn env_list_variables(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r, e)): EnvP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = repo_scope(&s, &u, &o, &r, Some(e)).await?;
    list_variables_resp(&s, &sc, p, &uri).await
}
async fn env_create_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e)): EnvP, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    create_variable_resp(&s, &repo_scope(&s, &u, &o, &r, Some(e)).await?, b).await
}
async fn env_get_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e, n)): EnvNameP) -> ApiResult<Json<Value>> {
    let sc = repo_scope(&s, &u, &o, &r, Some(e)).await?;
    Ok(Json(variable_json(&s, &sc, &get_variable(&s, &sc, &n).await?)))
}
async fn env_update_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e, n)): EnvNameP, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    update_variable(&s, &repo_scope(&s, &u, &o, &r, Some(e)).await?, &n, b).await?;
    Ok(no_content())
}
async fn env_delete_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, e, n)): EnvNameP) -> ApiResult<Response> {
    delete_variable(&s, &repo_scope(&s, &u, &o, &r, Some(e)).await?, &n).await?;
    Ok(no_content())
}

async fn envid_list_variables(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((id, e)): Path<(i64, String)>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = repo_id_scope(&s, &u, id, e).await?;
    list_variables_resp(&s, &sc, p, &uri).await
}
async fn envid_create_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, e)): Path<(i64, String)>, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    create_variable_resp(&s, &repo_id_scope(&s, &u, id, e).await?, b).await
}
async fn envid_get_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, e, n)): Path<(i64, String, String)>) -> ApiResult<Json<Value>> {
    let sc = repo_id_scope(&s, &u, id, e).await?;
    Ok(Json(variable_json(&s, &sc, &get_variable(&s, &sc, &n).await?)))
}
async fn envid_update_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, e, n)): Path<(i64, String, String)>, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    update_variable(&s, &repo_id_scope(&s, &u, id, e).await?, &n, b).await?;
    Ok(no_content())
}
async fn envid_delete_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, e, n)): Path<(i64, String, String)>) -> ApiResult<Response> {
    delete_variable(&s, &repo_id_scope(&s, &u, id, e).await?, &n).await?;
    Ok(no_content())
}

async fn org_list_secrets(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = org_scope(&s, &u, &org).await?;
    list_secrets_resp(&s, &sc, p, &uri).await
}
async fn org_public_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    public_key_json(&s, &org_scope(&s, &u, &org).await?).await
}
async fn org_get_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let sc = org_scope(&s, &u, &org).await?;
    Ok(Json(secret_json(&s, &sc, &get_secret(&s, &sc, &n).await?)))
}
async fn org_put_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>, Json(b): Json<PutSecret>) -> ApiResult<Response> {
    put_secret_resp(&s, &org_scope(&s, &u, &org).await?, &n, b).await
}
async fn org_delete_secret(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>) -> ApiResult<Response> {
    delete_secret(&s, &org_scope(&s, &u, &org).await?, &n).await?;
    Ok(no_content())
}
/// Selected-repository access lists are not tracked: always empty.
async fn org_secret_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let sc = org_scope(&s, &u, &org).await?;
    get_secret(&s, &sc, &n).await?;
    Ok(Json(total_envelope("repositories", vec![], 0)))
}
async fn org_list_variables(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let sc = org_scope(&s, &u, &org).await?;
    list_variables_resp(&s, &sc, p, &uri).await
}
async fn org_create_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    create_variable_resp(&s, &org_scope(&s, &u, &org).await?, b).await
}
async fn org_get_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let sc = org_scope(&s, &u, &org).await?;
    Ok(Json(variable_json(&s, &sc, &get_variable(&s, &sc, &n).await?)))
}
async fn org_update_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>, Json(b): Json<VariableInput>) -> ApiResult<Response> {
    update_variable(&s, &org_scope(&s, &u, &org).await?, &n, b).await?;
    Ok(no_content())
}
async fn org_delete_variable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>) -> ApiResult<Response> {
    delete_variable(&s, &org_scope(&s, &u, &org).await?, &n).await?;
    Ok(no_content())
}
async fn org_variable_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let sc = org_scope(&s, &u, &org).await?;
    get_variable(&s, &sc, &n).await?;
    Ok(Json(total_envelope("repositories", vec![], 0)))
}
