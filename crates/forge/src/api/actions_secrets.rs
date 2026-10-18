//! GitHub Actions secrets and variables for repositories, repository environments and
//! organizations.
//!
//! Secrets arrive already encrypted by the client (libsodium sealed box against the owner's
//! public key from `.../secrets/public-key`); forge stores the ciphertext and never returns it
//! through the API. forge holds each owner's private key (sealed with its master key, see
//! `actions_settings::crypto`), checks that a new value opens, and decrypts secrets only to
//! inject them into jobs (`actions_settings::secrets_for_job`).
//!
//! Org secrets and variables have a visibility: `all` repositories of the org, `private`
//! (private and internal repositories) or `selected` (`actions_secret_repos` /
//! `actions_variable_repos`).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::plat::total_envelope;
use super::{no_content, PageParams};
use super::actions_settings::crypto;
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
        .route(&format!("{org}/secrets/{{name}}/repositories"), get(org_secret_repos).put(org_secret_set_repos))
        .route(&format!("{org}/secrets/{{name}}/repositories/{{repository_id}}"), put(org_secret_add_repo).delete(org_secret_remove_repo))
        .route(&format!("{org}/variables"), get(org_list_variables).post(org_create_variable))
        .route(&format!("{org}/variables/{{name}}"), get(org_get_variable).patch(org_update_variable).delete(org_delete_variable))
        .route(&format!("{org}/variables/{{name}}/repositories"), get(org_variable_repos).put(org_variable_set_repos))
        .route(&format!("{org}/variables/{{name}}/repositories/{{repository_id}}"), put(org_variable_add_repo).delete(org_variable_remove_repo))
        .route(&format!("{repo}/organization-secrets"), get(repo_org_secrets_h))
        .route(&format!("{repo}/organization-variables"), get(repo_org_variables_h))
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

/// The owner's (key_id, base64 key), created on first use (an X25519 key pair whose private
/// half forge keeps sealed with its master key, see `actions_settings::crypto`).
pub async fn public_key(s: &AppState, scope: &Scope) -> ApiResult<(String, String)> {
    let (kind, id) = scope.key_owner();
    super::actions_settings::crypto::public_key(s, kind, id).await
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
    pub selected_repository_ids: Option<Vec<i64>>,
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
    // The value must be a sealed box for this key, or it could never be injected into a job.
    let (kind, owner) = scope.key_owner();
    match crypto::private_key(s, kind, owner).await? {
        Some((_, sk)) if crypto::open_sealed(&sk, &value).is_some() => {}
        _ => {
            let mut e = ApiError::validation("Secret", "encrypted_value", "invalid");
            e.errors = Some(json!([{ "resource": "Secret", "field": "encrypted_value", "code": "invalid",
                "message": "encrypted_value must be a libsodium sealed box for the key from the public-key endpoint" }]));
            return Err(e);
        }
    }
    let visibility = valid_visibility(scope, input.visibility.as_deref())?;
    let selected = match (&input.selected_repository_ids, scope) {
        (Some(ids), Scope::Org(o)) => Some(org_repo_ids(s, o, ids).await?),
        _ => None,
    };
    let name = name.to_ascii_uppercase();
    let (id, created): (i64, bool) = sqlx::query_as(
        "INSERT INTO actions_secrets (repo_id, org_id, environment, name, encrypted_value, key_id, visibility)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (COALESCE(repo_id, 0), COALESCE(org_id, 0), environment, name) DO UPDATE
            SET encrypted_value = EXCLUDED.encrypted_value, key_id = EXCLUDED.key_id,
                visibility = COALESCE($8, actions_secrets.visibility), updated_at = now()
         RETURNING id, (xmax = 0)",
    )
    .bind(scope.repo_id())
    .bind(scope.org_id())
    .bind(scope.env())
    .bind(&name)
    .bind(&value)
    .bind(&key_id)
    .bind(&visibility)
    .bind(input.visibility.as_ref().and(visibility.as_ref()))
    .fetch_one(&s.db)
    .await?;
    if let Some(ids) = selected {
        replace_selected(s, Selected::Secret, id, &ids).await?;
    }
    Ok(created)
}

// ---------------------------------------------------------------------------------------------
// Selected repositories of org secrets and variables

#[derive(Clone, Copy)]
pub enum Selected {
    Secret,
    Variable,
}

impl Selected {
    fn table(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Selected::Secret => ("actions_secrets", "actions_secret_repos", "secret_id"),
            Selected::Variable => ("actions_variables", "actions_variable_repos", "variable_id"),
        }
    }
}

/// Validates that every id is a repository owned by `org` (422 otherwise); returns them deduplicated.
pub async fn org_repo_ids(s: &AppState, org: &User, ids: &[i64]) -> ApiResult<Vec<i64>> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let found: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE owner_id = $1 AND id = ANY($2)")
        .bind(org.id)
        .bind(&ids)
        .fetch_one(&s.db)
        .await?;
    if found != ids.len() as i64 {
        return Err(ApiError::validation("Repository", "selected_repository_ids", "invalid"));
    }
    Ok(ids)
}

/// Replaces the selected repositories of an org secret/variable.
pub async fn replace_selected(s: &AppState, kind: Selected, id: i64, repo_ids: &[i64]) -> ApiResult<()> {
    let (_, table, col) = kind.table();
    let mut tx = s.db.begin().await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE {col} = $1"))).bind(id).execute(&mut *tx).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO {table} ({col}, repo_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")))
        .bind(id)
        .bind(repo_ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// (id, visibility) of an org secret/variable by name.
async fn org_item(s: &AppState, kind: Selected, org: &User, name: &str) -> ApiResult<(i64, String)> {
    let (items, _, _) = kind.table();
    let row: Option<(i64, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT id, visibility FROM {items} WHERE org_id = $1 AND upper(name) = upper($2)")))
            .bind(org.id)
            .bind(name)
            .fetch_optional(&s.db)
            .await?;
    let (id, vis) = row.ok_or_else(ApiError::not_found)?;
    Ok((id, vis.unwrap_or_else(|| "private".into())))
}

fn not_selected() -> ApiError {
    ApiError::conflict("The visibility must be set to 'selected' to manage the selected repositories.")
}

/// Repositories an org secret/variable is shared with (visibility `selected`), paginated.
pub async fn list_selected(s: &AppState, kind: Selected, org: &User, name: &str, p: PageParams) -> ApiResult<(Vec<RepoCtx>, i64)> {
    let (id, vis) = org_item(s, kind, org, name).await?;
    if vis != "selected" {
        return match kind {
            Selected::Variable => Err(not_selected()),
            Selected::Secret => Ok((vec![], 0)),
        };
    }
    let (_, table, col) = kind.table();
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {table} t JOIN repositories r ON r.id = t.repo_id WHERE t.{col} = $1"
    )))
    .bind(id)
    .fetch_one(&s.db)
    .await?;
    let repos: Vec<crate::db::Repo> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.* FROM {table} t JOIN repositories r ON r.id = t.repo_id WHERE t.{col} = $1 ORDER BY r.id LIMIT $2 OFFSET $3"
    )))
    .bind(id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    Ok((repos.into_iter().map(|repo| RepoCtx { repo, owner: org.clone(), perm: Perm::Admin }).collect(), total))
}

pub async fn set_selected(s: &AppState, kind: Selected, org: &User, name: &str, repo_ids: &[i64]) -> ApiResult<()> {
    let (id, vis) = org_item(s, kind, org, name).await?;
    if vis != "selected" {
        return Err(not_selected());
    }
    let ids = org_repo_ids(s, org, repo_ids).await?;
    replace_selected(s, kind, id, &ids).await
}

pub async fn add_selected(s: &AppState, kind: Selected, org: &User, name: &str, repo_id: i64, add: bool) -> ApiResult<()> {
    let (id, vis) = org_item(s, kind, org, name).await?;
    if vis != "selected" {
        return Err(not_selected());
    }
    let (_, table, col) = kind.table();
    if add {
        let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM repositories WHERE id = $1 AND owner_id = $2)")
            .bind(repo_id)
            .bind(org.id)
            .fetch_one(&s.db)
            .await?;
        if !owned {
            return Err(ApiError::not_found());
        }
        sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO {table} ({col}, repo_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")))
            .bind(id)
            .bind(repo_id)
            .execute(&s.db)
            .await?;
    } else {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE {col} = $1 AND repo_id = $2")))
            .bind(id)
            .bind(repo_id)
            .execute(&s.db)
            .await?;
    }
    Ok(())
}

/// SQL condition: the org item (alias `i`) is visible to repository `r`.
pub const VISIBLE_TO_REPO: &str = "(COALESCE(i.visibility, 'private') = 'all'
    OR (COALESCE(i.visibility, 'private') = 'private' AND r.visibility <> 'public')
    OR (i.visibility = 'selected' AND EXISTS (SELECT 1 FROM {sel} x WHERE x.{col} = i.id AND x.repo_id = r.id)))";

/// SQL condition for `kind` with the selected-table placeholders filled in.
pub fn visible_to_repo(kind: Selected) -> String {
    let (_, sel, col) = kind.table();
    VISIBLE_TO_REPO.replace("{sel}", sel).replace("{col}", col)
}

/// Org secrets shared with a repository (names only), paginated.
pub async fn repo_org_secrets(s: &AppState, ctx: &RepoCtx, p: PageParams) -> ApiResult<(Vec<SecretRow>, i64)> {
    let cond = visible_to_repo(Selected::Secret);
    let from = format!("FROM actions_secrets i JOIN repositories r ON r.id = $1 WHERE i.org_id = r.owner_id AND {cond}");
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}"))).bind(ctx.repo.id).fetch_one(&s.db).await?;
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT i.name, i.visibility, i.created_at, i.updated_at {from} ORDER BY i.name LIMIT $2 OFFSET $3"
    )))
    .bind(ctx.repo.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    Ok((rows, total))
}

/// Org variables shared with a repository, paginated.
pub async fn repo_org_variables(s: &AppState, ctx: &RepoCtx, p: PageParams) -> ApiResult<(Vec<VariableRow>, i64)> {
    let cond = visible_to_repo(Selected::Variable);
    let from = format!("FROM actions_variables i JOIN repositories r ON r.id = $1 WHERE i.org_id = r.owner_id AND {cond}");
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}"))).bind(ctx.repo.id).fetch_one(&s.db).await?;
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT i.name, i.value, i.visibility, i.created_at, i.updated_at {from} ORDER BY i.name LIMIT $2 OFFSET $3"
    )))
    .bind(ctx.repo.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    Ok((rows, total))
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
    pub selected_repository_ids: Option<Vec<i64>>,
}

async fn selected_ids(s: &AppState, scope: &Scope, ids: &Option<Vec<i64>>) -> ApiResult<Option<Vec<i64>>> {
    match (ids, scope) {
        (Some(ids), Scope::Org(o)) => Ok(Some(org_repo_ids(s, o, ids).await?)),
        _ => Ok(None),
    }
}

async fn set_variable_selected(s: &AppState, scope: &Scope, name: &str, ids: Option<Vec<i64>>) -> ApiResult<()> {
    let (Some(ids), Scope::Org(o)) = (ids, scope) else { return Ok(()) };
    let (id, _) = org_item(s, Selected::Variable, o, name).await?;
    replace_selected(s, Selected::Variable, id, &ids).await
}

/// Creates a variable; 409 when one with that name exists (gh then PATCHes).
pub async fn create_variable(s: &AppState, scope: &Scope, input: VariableInput) -> ApiResult<VariableRow> {
    let name = input.name.filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("Variable", "name", "missing_field"))?;
    valid_name("Variable", &name)?;
    let value = input.value.ok_or_else(|| ApiError::validation("Variable", "value", "missing_field"))?;
    let visibility = valid_visibility(scope, input.visibility.as_deref())?;
    let selected = selected_ids(s, scope, &input.selected_repository_ids).await?;
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
    let row = row.ok_or_else(|| ApiError::conflict("Already exists - Variable already exists"))?;
    set_variable_selected(s, scope, &row.name, selected).await?;
    Ok(row)
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
    let selected = selected_ids(s, scope, &input.selected_repository_ids).await?;
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
    set_variable_selected(s, scope, &new_name, selected).await
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
fn org_of(sc: &Scope) -> &User {
    match sc {
        Scope::Org(o) => o,
        Scope::Repo { .. } => unreachable!("org scope"),
    }
}

#[derive(Deserialize)]
struct SelectedIds {
    selected_repository_ids: Option<Vec<i64>>,
}

async fn selected_list_resp(s: &AppState, u: &User, org: &str, kind: Selected, name: &str, p: PageParams, uri: &OriginalUri) -> ApiResult<Response> {
    let sc = org_scope(s, u, org).await?;
    let (repos, total) = list_selected(s, kind, org_of(&sc), name, p).await?;
    let (api, base) = (s.api_url(), s.base_url().to_string());
    let items = repos.iter().map(|c| crate::json::repo::repo(&api, &base, c)).collect();
    Ok(envelope(s, uri, p, "repositories", items, total))
}
async fn selected_set_resp(s: &AppState, u: &User, org: &str, kind: Selected, name: &str, b: SelectedIds) -> ApiResult<Response> {
    let sc = org_scope(s, u, org).await?;
    let ids = b.selected_repository_ids.ok_or_else(|| ApiError::validation("Repository", "selected_repository_ids", "missing_field"))?;
    set_selected(s, kind, org_of(&sc), name, &ids).await?;
    Ok(no_content())
}
async fn selected_one_resp(s: &AppState, u: &User, org: &str, kind: Selected, name: &str, repo_id: i64, add: bool) -> ApiResult<Response> {
    let sc = org_scope(s, u, org).await?;
    add_selected(s, kind, org_of(&sc), name, repo_id, add).await?;
    Ok(no_content())
}

type OrgNameP = Path<(String, String)>;
type OrgNameRepoP = Path<(String, String, i64)>;

async fn org_secret_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((org, n)): OrgNameP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    selected_list_resp(&s, &u, &org, Selected::Secret, &n, p, &uri).await
}
async fn org_secret_set_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): OrgNameP, Json(b): Json<SelectedIds>) -> ApiResult<Response> {
    selected_set_resp(&s, &u, &org, Selected::Secret, &n, b).await
}
async fn org_secret_add_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, id)): OrgNameRepoP) -> ApiResult<Response> {
    selected_one_resp(&s, &u, &org, Selected::Secret, &n, id, true).await
}
async fn org_secret_remove_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, id)): OrgNameRepoP) -> ApiResult<Response> {
    selected_one_resp(&s, &u, &org, Selected::Secret, &n, id, false).await
}
async fn org_variable_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((org, n)): OrgNameP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    selected_list_resp(&s, &u, &org, Selected::Variable, &n, p, &uri).await
}
async fn org_variable_set_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): OrgNameP, Json(b): Json<SelectedIds>) -> ApiResult<Response> {
    selected_set_resp(&s, &u, &org, Selected::Variable, &n, b).await
}
async fn org_variable_add_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, id)): OrgNameRepoP) -> ApiResult<Response> {
    selected_one_resp(&s, &u, &org, Selected::Variable, &n, id, true).await
}
async fn org_variable_remove_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, id)): OrgNameRepoP) -> ApiResult<Response> {
    selected_one_resp(&s, &u, &org, Selected::Variable, &n, id, false).await
}

/// `GET /repos/{o}/{r}/actions/organization-secrets` and `-variables`: what the org shares with
/// the repository (repository admins, like repository secrets).
async fn repo_org_secrets_h(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): RepoP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Admin)?;
    let (rows, total) = repo_org_secrets(&s, &ctx, p).await?;
    let items = rows
        .iter()
        .map(|r| json!({ "name": r.name, "created_at": crate::json::ts(r.created_at), "updated_at": crate::json::ts(r.updated_at) }))
        .collect();
    Ok(envelope(&s, &uri, p, "secrets", items, total))
}
async fn repo_org_variables_h(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): RepoP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Admin)?;
    let p = PageParams { page: p.page, per_page: Some(p.per_page.unwrap_or(10).clamp(1, 30)) };
    let (rows, total) = repo_org_variables(&s, &ctx, p).await?;
    let items = rows
        .iter()
        .map(|r| json!({ "name": r.name, "value": r.value, "created_at": crate::json::ts(r.created_at), "updated_at": crate::json::ts(r.updated_at) }))
        .collect();
    Ok(envelope(&s, &uri, p, "variables", items, total))
}
