//! Code security configurations (organization and enterprise), the global "GitHub
//! recommended" configuration, attachments to repositories, and defaults for new repositories.
//!
//! Attaching a configuration applies its settings to each repository: every `enabled` /
//! `disabled` product is written to `repo_security_products` (and Dependabot alerts to
//! `repo_settings.vulnerability_alerts`) through [`super::set_enabled`], which also starts the
//! scans; `not_set` leaves the repository's own setting alone. Detaching keeps the settings.
//! While an `enforced` configuration is attached, `security_and_analysis` changes on the
//! repository are refused.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::CursorParams;
use crate::auth::AuthUser;
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let o = "/orgs/{org}/code-security/configurations";
    let e = "/enterprises/{enterprise}/code-security/configurations";
    Router::new()
        .route(o, get(org_list).post(org_create))
        .route(&format!("{o}/defaults"), get(org_defaults))
        .route(&format!("{o}/detach"), delete(org_detach))
        .route(&format!("{o}/{{configuration_id}}"), get(org_get).patch(org_update).delete(org_delete))
        .route(&format!("{o}/{{configuration_id}}/attach"), post(org_attach))
        .route(&format!("{o}/{{configuration_id}}/defaults"), put(org_set_default))
        .route(&format!("{o}/{{configuration_id}}/repositories"), get(org_repos))
        .route(e, get(ent_list).post(ent_create))
        .route(&format!("{e}/defaults"), get(ent_defaults))
        .route(&format!("{e}/{{configuration_id}}"), get(ent_get).patch(ent_update).delete(ent_delete))
        .route(&format!("{e}/{{configuration_id}}/attach"), post(ent_attach))
        .route(&format!("{e}/{{configuration_id}}/defaults"), put(ent_set_default))
        .route(&format!("{e}/{{configuration_id}}/repositories"), get(ent_repos))
        .route("/repos/{owner}/{repo}/code-security-configuration", get(repo_config))
}

/// Tri-state settings and their defaults for a new configuration (GitHub's defaults).
const SETTINGS: &[(&str, &str)] = &[
    ("advanced_security", "disabled"),
    ("code_security", "not_set"),
    ("secret_protection", "not_set"),
    ("dependency_graph", "enabled"),
    ("dependency_graph_autosubmit_action", "disabled"),
    ("dependabot_alerts", "disabled"),
    ("dependabot_security_updates", "disabled"),
    ("dependabot_delegated_alert_dismissal", "disabled"),
    ("code_scanning_default_setup", "disabled"),
    ("code_scanning_delegated_alert_dismissal", "disabled"),
    ("secret_scanning", "disabled"),
    ("secret_scanning_push_protection", "disabled"),
    ("secret_scanning_delegated_bypass", "disabled"),
    ("secret_scanning_validity_checks", "disabled"),
    ("secret_scanning_non_provider_patterns", "disabled"),
    ("secret_scanning_delegated_alert_dismissal", "disabled"),
    ("private_vulnerability_reporting", "disabled"),
];

/// Object-valued settings.
const OBJECT_SETTINGS: &[&str] = &["code_scanning_options", "code_scanning_default_setup_options", "secret_scanning_delegated_bypass_options", "dependency_graph_autosubmit_action_options"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Org,
    Enterprise,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConfigRow {
    pub id: i64,
    pub target_type: String,
    pub owner_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub settings: Value,
    pub enforcement: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn config_url(s: &AppState, level: Level, scope: &str, id: i64) -> (String, String) {
    match level {
        Level::Org => (
            format!("{}/orgs/{scope}/code-security/configurations/{id}", s.api_url()),
            format!("{}/organizations/{scope}/settings/security_analysis/configurations/{id}/edit", s.base_url()),
        ),
        Level::Enterprise => (
            format!("{}/enterprises/{scope}/code-security/configurations/{id}", s.api_url()),
            format!("{}/enterprises/{scope}/settings/security_analysis/configurations/{id}/edit", s.base_url()),
        ),
    }
}

pub fn config_json(s: &AppState, level: Level, scope: &str, c: &ConfigRow) -> Value {
    let (url, html) = config_url(s, level, scope, c.id);
    let mut v = json!({
        "id": c.id,
        "name": c.name,
        "target_type": c.target_type,
        "description": c.description,
        "enforcement": c.enforcement,
        "url": url,
        "html_url": html,
        "created_at": crate::json::ts(c.created_at),
        "updated_at": crate::json::ts(c.updated_at),
    });
    for (k, d) in SETTINGS {
        v[*k] = c.settings.get(*k).cloned().unwrap_or_else(|| json!(d));
    }
    v["code_scanning_options"] = c.settings.get("code_scanning_options").cloned().unwrap_or(Value::Null);
    v["code_scanning_default_setup_options"] = c.settings.get("code_scanning_default_setup_options").cloned().unwrap_or(Value::Null);
    v["secret_scanning_delegated_bypass_options"] = c.settings.get("secret_scanning_delegated_bypass_options").cloned().unwrap_or_else(|| json!({ "reviewers": [] }));
    if let Some(x) = c.settings.get("dependency_graph_autosubmit_action_options") {
        v["dependency_graph_autosubmit_action_options"] = x.clone();
    }
    v
}

/// Validates a create/update body into a settings map (only keys present).
fn parse_settings(b: &Map<String, Value>, cfg_id: Option<i64>) -> ApiResult<Map<String, Value>> {
    let mut out = Map::new();
    for (k, _) in SETTINGS {
        if let Some(v) = b.get(*k) {
            let ok: &[&str] = if *k == "advanced_security" { &["enabled", "disabled", "code_security", "secret_protection"] } else { &["enabled", "disabled", "not_set"] };
            match v.as_str() {
                Some(x) if ok.contains(&x) => {
                    out.insert(k.to_string(), json!(x));
                }
                _ => return Err(ApiError::validation("CodeSecurityConfiguration", k, "invalid")),
            }
        }
    }
    for k in OBJECT_SETTINGS {
        if let Some(v) = b.get(*k) {
            if !(v.is_object() || v.is_null()) {
                return Err(ApiError::validation("CodeSecurityConfiguration", k, "invalid"));
            }
            let mut v = v.clone();
            if *k == "code_scanning_default_setup_options" {
                if let Some(rt) = v.get("runner_type").and_then(Value::as_str) {
                    if !["standard", "labeled", "not_set"].contains(&rt) {
                        return Err(ApiError::validation("CodeSecurityConfiguration", "runner_type", "invalid"));
                    }
                }
                if v.is_object() && v.get("runner_label").is_none() {
                    v["runner_label"] = Value::Null;
                }
            }
            if *k == "secret_scanning_delegated_bypass_options" {
                let mut reviewers = Vec::new();
                for r in v.get("reviewers").and_then(Value::as_array).into_iter().flatten() {
                    let id = r.get("reviewer_id").and_then(Value::as_i64).ok_or_else(|| ApiError::validation("CodeSecurityConfiguration", "reviewer_id", "missing_field"))?;
                    let t = r.get("reviewer_type").and_then(Value::as_str).filter(|t| ["TEAM", "ROLE"].contains(t)).ok_or_else(|| ApiError::validation("CodeSecurityConfiguration", "reviewer_type", "invalid"))?;
                    let mut x = json!({ "reviewer_id": id, "reviewer_type": t, "mode": r.get("mode").and_then(Value::as_str).unwrap_or("ALWAYS") });
                    if let Some(c) = cfg_id {
                        x["security_configuration_id"] = json!(c);
                    }
                    reviewers.push(x);
                }
                v = json!({ "reviewers": reviewers });
            }
            out.insert(k.to_string(), v);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Scope helpers

async fn org_scope(s: &AppState, u: &User, org: &str) -> ApiResult<User> {
    let o = crate::orgs::org_by_login(s, org).await?;
    super::require_security_manager(s, &o, u).await?;
    Ok(o)
}

/// A configuration visible at a level: org configs of that org plus the global ones; enterprise
/// configs at the enterprise level.
async fn load(s: &AppState, level: Level, owner_id: i64, id: i64) -> ApiResult<ConfigRow> {
    let row: Option<ConfigRow> = sqlx::query_as("SELECT id, target_type, owner_id, name, description, settings, enforcement, created_at, updated_at FROM code_security_configurations WHERE id = $1")
        .bind(id)
        .fetch_optional(&s.db)
        .await?;
    let row = row.ok_or_else(ApiError::not_found)?;
    let visible = match level {
        Level::Org => (row.target_type == "organization" && row.owner_id == owner_id) || row.target_type == "global" || row.target_type == "enterprise",
        Level::Enterprise => row.target_type == "enterprise" || row.target_type == "global",
    };
    if !visible {
        return Err(ApiError::not_found());
    }
    Ok(row)
}

fn owned(level: Level, owner_id: i64, c: &ConfigRow) -> bool {
    match level {
        Level::Org => c.target_type == "organization" && c.owner_id == owner_id,
        Level::Enterprise => c.target_type == "enterprise",
    }
}

#[derive(Deserialize, Default)]
struct ListQ {
    target_type: Option<String>,
    #[serde(flatten)]
    cursor: CursorParams,
}

async fn list(s: &AppState, uri: &OriginalUri, level: Level, owner_id: i64, scope: &str, q: ListQ) -> ApiResult<Response> {
    super::check_enum("CodeSecurityConfiguration", "target_type", &q.target_type, &["global", "all"])?;
    let only_global = q.target_type.as_deref() == Some("global");
    let rows: Vec<ConfigRow> = sqlx::query_as(
        "SELECT id, target_type, owner_id, name, description, settings, enforcement, created_at, updated_at FROM code_security_configurations
          WHERE (target_type = 'global' OR ($1 = 'org' AND NOT $3 AND ((target_type = 'organization' AND owner_id = $2) OR target_type = 'enterprise'))
                 OR ($1 = 'enterprise' AND NOT $3 AND target_type = 'enterprise'))
          ORDER BY id",
    )
    .bind(if level == Level::Org { "org" } else { "enterprise" })
    .bind(owner_id)
    .bind(only_global)
    .fetch_all(&s.db)
    .await?;
    let total = rows.len() as i64;
    let items = rows.iter().skip(q.cursor.offset() as usize).take(q.cursor.per_page() as usize).map(|c| config_json(s, level, scope, c)).collect();
    Ok(super::list_response(s, uri, &q.cursor, total, items))
}

async fn create(s: &AppState, level: Level, owner_id: i64, scope: &str, b: Map<String, Value>) -> ApiResult<Response> {
    let name = b.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("CodeSecurityConfiguration", "name", "missing_field"))?;
    if name.len() > 255 {
        return Err(ApiError::validation("CodeSecurityConfiguration", "name", "invalid"));
    }
    let description = b.get("description").and_then(Value::as_str).map(str::to_string);
    if description.as_ref().is_some_and(|d| d.len() > 255) {
        return Err(ApiError::validation("CodeSecurityConfiguration", "description", "invalid"));
    }
    let enforcement = match b.get("enforcement").and_then(Value::as_str) {
        None => "enforced",
        Some(e @ ("enforced" | "unenforced")) => e,
        Some(_) => return Err(ApiError::validation("CodeSecurityConfiguration", "enforcement", "invalid")),
    };
    let mut settings = Map::new();
    for (k, d) in SETTINGS {
        settings.insert(k.to_string(), json!(d));
    }
    settings.extend(parse_settings(&b, None)?);
    let target = if level == Level::Org { "organization" } else { "enterprise" };
    let row: ConfigRow = sqlx::query_as(
        "INSERT INTO code_security_configurations (target_type, owner_id, name, description, settings, enforcement) VALUES ($1,$2,$3,$4,$5,$6)
         RETURNING id, target_type, owner_id, name, description, settings, enforcement, created_at, updated_at",
    )
    .bind(target)
    .bind(owner_id)
    .bind(name)
    .bind(&description)
    .bind(Value::Object(settings))
    .bind(enforcement)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("CodeSecurityConfiguration", "name", "already_exists"),
        e => e.into(),
    })?;
    let row = stamp_reviewers(s, row).await?;
    Ok((StatusCode::CREATED, Json(config_json(s, level, scope, &row))).into_response())
}

/// Reviewer entries carry their configuration id.
async fn stamp_reviewers(s: &AppState, mut row: ConfigRow) -> ApiResult<ConfigRow> {
    if let Some(rs) = row.settings.pointer_mut("/secret_scanning_delegated_bypass_options/reviewers").and_then(Value::as_array_mut) {
        for r in rs.iter_mut() {
            r["security_configuration_id"] = json!(row.id);
        }
        sqlx::query("UPDATE code_security_configurations SET settings = $2 WHERE id = $1").bind(row.id).bind(&row.settings).execute(&s.db).await?;
    }
    Ok(row)
}

async fn update(s: &AppState, level: Level, owner_id: i64, scope: &str, id: i64, b: Map<String, Value>) -> ApiResult<Response> {
    let cur = load(s, level, owner_id, id).await?;
    if !owned(level, owner_id, &cur) {
        return Err(ApiError::forbidden("This configuration cannot be changed here."));
    }
    let mut settings = cur.settings.as_object().cloned().unwrap_or_default();
    let changes = parse_settings(&b, Some(id))?;
    let name = match b.get("name") {
        Some(Value::String(n)) if !n.trim().is_empty() => n.trim().to_string(),
        Some(_) => return Err(ApiError::validation("CodeSecurityConfiguration", "name", "invalid")),
        None => cur.name.clone(),
    };
    let description = match b.get("description") {
        Some(Value::String(d)) => Some(d.clone()),
        Some(Value::Null) => None,
        Some(_) => return Err(ApiError::validation("CodeSecurityConfiguration", "description", "invalid")),
        None => cur.description.clone(),
    };
    let enforcement = match b.get("enforcement").and_then(Value::as_str) {
        None => cur.enforcement.clone(),
        Some(e @ ("enforced" | "unenforced")) => e.to_string(),
        Some(_) => return Err(ApiError::validation("CodeSecurityConfiguration", "enforcement", "invalid")),
    };
    settings.extend(changes);
    let row: ConfigRow = sqlx::query_as(
        "UPDATE code_security_configurations SET name = $2, description = $3, settings = $4, enforcement = $5, updated_at = now() WHERE id = $1
         RETURNING id, target_type, owner_id, name, description, settings, enforcement, created_at, updated_at",
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(Value::Object(settings))
    .bind(&enforcement)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("CodeSecurityConfiguration", "name", "already_exists"),
        e => e.into(),
    })?;
    // Attached repositories follow the new settings.
    let repos: Vec<i64> = sqlx::query_scalar("SELECT repo_id FROM code_security_configuration_repos WHERE configuration_id = $1").bind(id).fetch_all(&s.db).await?;
    for r in repos {
        apply_to_repo(s, &row, r).await?;
    }
    Ok(Json(config_json(s, level, scope, &row)).into_response())
}

async fn remove(s: &AppState, level: Level, owner_id: i64, id: i64) -> ApiResult<Response> {
    let cur = load(s, level, owner_id, id).await?;
    if !owned(level, owner_id, &cur) {
        return Err(ApiError::forbidden("This configuration cannot be deleted here."));
    }
    sqlx::query("DELETE FROM code_security_configurations WHERE id = $1").bind(id).execute(&s.db).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Applies a configuration's settings to one repository.
pub async fn apply_to_repo(s: &AppState, c: &ConfigRow, repo_id: i64) -> ApiResult<()> {
    let get = |k: &str| c.settings.get(k).and_then(Value::as_str).unwrap_or("not_set").to_string();
    let adv = get("advanced_security");
    let mut pairs: Vec<(&str, String)> = vec![("advanced_security", match adv.as_str() {
        "enabled" | "code_security" | "secret_protection" => "enabled".into(),
        _ => "disabled".into(),
    })];
    for k in [
        "dependency_graph",
        "dependabot_alerts",
        "dependabot_security_updates",
        "dependabot_delegated_alert_dismissal",
        "code_scanning_default_setup",
        "code_scanning_delegated_alert_dismissal",
        "secret_scanning",
        "secret_scanning_push_protection",
        "secret_scanning_validity_checks",
        "secret_scanning_non_provider_patterns",
        "secret_scanning_delegated_bypass",
        "secret_scanning_delegated_alert_dismissal",
        "private_vulnerability_reporting",
    ] {
        pairs.push((k, get(k)));
    }
    for (product, v) in pairs {
        let on = match v.as_str() {
            "enabled" => true,
            "disabled" => false,
            _ => continue,
        };
        let options = match product {
            "secret_scanning_delegated_bypass" => Some(c.settings.get("secret_scanning_delegated_bypass_options").cloned().unwrap_or_else(|| json!({ "reviewers": [] }))),
            _ => None,
        };
        super::set_enabled(s, repo_id, product, on, options).await?;
        if product == "code_scanning_default_setup" {
            sqlx::query(
                "INSERT INTO code_scanning_default_setup (repo_id, state, updated_at) VALUES ($1, $2, now())
                 ON CONFLICT (repo_id) DO UPDATE SET state = $2, updated_at = now()",
            )
            .bind(repo_id)
            .bind(if on { "configured" } else { "not-configured" })
            .execute(&s.db)
            .await?;
        }
    }
    Ok(())
}

async fn attach_repos(s: &AppState, c: &ConfigRow, repo_ids: &[i64]) -> ApiResult<()> {
    let status = if c.enforcement == "enforced" { "enforced" } else { "attached" };
    for &r in repo_ids {
        sqlx::query(
            "INSERT INTO code_security_configuration_repos (repo_id, configuration_id, status) VALUES ($1,$2,$3)
             ON CONFLICT (repo_id) DO UPDATE SET configuration_id = $2, status = $3, attached_at = now()",
        )
        .bind(r)
        .bind(c.id)
        .bind(status)
        .execute(&s.db)
        .await?;
        apply_to_repo(s, c, r).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct AttachBody {
    scope: Option<String>,
    selected_repository_ids: Option<Vec<i64>>,
}

async fn attach(s: &AppState, level: Level, owner_id: i64, id: i64, b: AttachBody) -> ApiResult<Response> {
    let c = load(s, level, owner_id, id).await?;
    let scope = b.scope.ok_or_else(|| ApiError::validation("CodeSecurityConfiguration", "scope", "missing_field"))?;
    let allowed: &[&str] = if level == Level::Org { &["all", "all_without_configurations", "public", "private_or_internal", "selected"] } else { &["all", "all_without_configurations"] };
    if !allowed.contains(&scope.as_str()) {
        return Err(ApiError::validation("CodeSecurityConfiguration", "scope", "invalid"));
    }
    let base = if level == Level::Org { "SELECT id FROM repositories WHERE owner_id = $1" } else { "SELECT r.id FROM repositories r JOIN users o ON o.id = r.owner_id WHERE o.kind = 'Organization' AND $1 = 0" };
    let mut ids: Vec<i64> = match scope.as_str() {
        "selected" => {
            let sel = b.selected_repository_ids.ok_or_else(|| ApiError::validation("CodeSecurityConfiguration", "selected_repository_ids", "missing_field"))?;
            let found: Vec<i64> = sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1 AND id = ANY($2)").bind(owner_id).bind(&sel).fetch_all(&s.db).await?;
            if found.len() != sel.iter().collect::<std::collections::HashSet<_>>().len() {
                return Err(ApiError::validation("CodeSecurityConfiguration", "selected_repository_ids", "invalid"));
            }
            found
        }
        "public" => sqlx::query_scalar(sqlx::AssertSqlSafe(format!("{base} AND visibility = 'public'"))).bind(owner_id).fetch_all(&s.db).await?,
        "private_or_internal" => sqlx::query_scalar(sqlx::AssertSqlSafe(format!("{base} AND visibility <> 'public'"))).bind(owner_id).fetch_all(&s.db).await?,
        "all_without_configurations" => sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT id FROM ({base}) x WHERE NOT EXISTS(SELECT 1 FROM code_security_configuration_repos a WHERE a.repo_id = x.id)"
        )))
        .bind(owner_id)
        .fetch_all(&s.db)
        .await?,
        _ => sqlx::query_scalar(sqlx::AssertSqlSafe(base.to_string())).bind(owner_id).fetch_all(&s.db).await?,
    };
    ids.sort_unstable();
    attach_repos(s, &c, &ids).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response())
}

#[derive(Deserialize)]
struct DefaultBody {
    default_for_new_repos: Option<String>,
}

async fn set_default(s: &AppState, level: Level, owner_id: i64, scope: &str, id: i64, b: DefaultBody) -> ApiResult<Json<Value>> {
    let c = load(s, level, owner_id, id).await?;
    let d = b.default_for_new_repos.ok_or_else(|| ApiError::validation("CodeSecurityConfiguration", "default_for_new_repos", "missing_field"))?;
    if !["all", "none", "private_and_internal", "public"].contains(&d.as_str()) {
        return Err(ApiError::validation("CodeSecurityConfiguration", "default_for_new_repos", "invalid"));
    }
    let kind = if level == Level::Org { "org" } else { "enterprise" };
    let mut tx = s.db.begin().await?;
    // One default per repository visibility: a broader default replaces narrower ones.
    let clash: &[&str] = match d.as_str() {
        "all" => &["all", "public", "private_and_internal"],
        "public" => &["all", "public"],
        "private_and_internal" => &["all", "private_and_internal"],
        _ => &[],
    };
    sqlx::query("DELETE FROM code_security_defaults WHERE scope_kind = $1 AND scope_id = $2 AND (configuration_id = $3 OR default_for_new_repos = ANY($4))")
        .bind(kind)
        .bind(owner_id)
        .bind(id)
        .bind(clash.iter().map(|x| x.to_string()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;
    if d != "none" {
        sqlx::query("INSERT INTO code_security_defaults (scope_kind, scope_id, configuration_id, default_for_new_repos) VALUES ($1,$2,$3,$4)")
            .bind(kind)
            .bind(owner_id)
            .bind(id)
            .bind(&d)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "default_for_new_repos": d, "configuration": config_json(s, level, scope, &c) })))
}

async fn defaults(s: &AppState, level: Level, owner_id: i64, scope: &str) -> ApiResult<Json<Value>> {
    let kind = if level == Level::Org { "org" } else { "enterprise" };
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT default_for_new_repos, configuration_id FROM code_security_defaults WHERE scope_kind = $1 AND scope_id = $2 ORDER BY configuration_id")
        .bind(kind)
        .bind(owner_id)
        .fetch_all(&s.db)
        .await?;
    let mut out = Vec::new();
    for (d, id) in rows {
        if let Ok(c) = load(s, level, owner_id, id).await {
            out.push(json!({ "default_for_new_repos": d, "configuration": config_json(s, level, scope, &c) }));
        }
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize, Default)]
struct ReposQ {
    status: Option<String>,
    #[serde(flatten)]
    cursor: CursorParams,
}

async fn repos(s: &AppState, uri: &OriginalUri, level: Level, owner_id: i64, id: i64, q: ReposQ) -> ApiResult<Response> {
    load(s, level, owner_id, id).await?;
    let statuses = super::csv(&q.status).filter(|v| !v.iter().any(|x| x == "all"));
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT a.repo_id, a.status FROM code_security_configuration_repos a JOIN repositories r ON r.id = a.repo_id
          WHERE a.configuration_id = $1 AND ($2 = 0 OR r.owner_id = $2) AND ($3::text[] IS NULL OR a.status = ANY($3)) ORDER BY a.repo_id",
    )
    .bind(id)
    .bind(if level == Level::Org { owner_id } else { 0 })
    .bind(&statuses)
    .fetch_all(&s.db)
    .await?;
    let total = rows.len() as i64;
    let page: Vec<(i64, String)> = rows.into_iter().skip(q.cursor.offset() as usize).take(q.cursor.per_page() as usize).collect();
    let ctxs = super::repos_by_id(&s.db, page.iter().map(|x| x.0)).await?;
    let items = page.iter().filter_map(|(r, st)| ctxs.get(r).map(|c| json!({ "status": st, "repository": super::repo_json(s, c) }))).collect();
    Ok(super::list_response(s, uri, &q.cursor, total, items))
}

/// The configuration attached to a repository, if any.
pub async fn attached(s: &AppState, repo_id: i64) -> ApiResult<Option<(ConfigRow, String)>> {
    let row: Option<(i64, String)> = sqlx::query_as("SELECT configuration_id, status FROM code_security_configuration_repos WHERE repo_id = $1")
        .bind(repo_id)
        .fetch_optional(&s.db)
        .await?;
    let Some((id, status)) = row else { return Ok(None) };
    let c: ConfigRow = sqlx::query_as("SELECT id, target_type, owner_id, name, description, settings, enforcement, created_at, updated_at FROM code_security_configurations WHERE id = $1")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    Ok(Some((c, status)))
}

/// A new repository in an organization gets the matching default configuration (org defaults
/// first, then enterprise defaults).
pub async fn on_repo_created(s: &AppState, ctx: &RepoCtx) {
    if ctx.owner.kind != "Organization" {
        return;
    }
    let vis = if ctx.repo.visibility == "public" { "public" } else { "private_and_internal" };
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT configuration_id FROM code_security_defaults
          WHERE ((scope_kind = 'org' AND scope_id = $1) OR (scope_kind = 'enterprise' AND scope_id = 0)) AND default_for_new_repos IN ('all', $2)
          ORDER BY (scope_kind = 'org') DESC LIMIT 1",
    )
    .bind(ctx.owner.id)
    .bind(vis)
    .fetch_optional(&s.db)
    .await
    .ok()
    .flatten();
    let Some(id) = id else { return };
    let Ok(Some(c)) = sqlx::query_as::<_, ConfigRow>("SELECT id, target_type, owner_id, name, description, settings, enforcement, created_at, updated_at FROM code_security_configurations WHERE id = $1")
        .bind(id)
        .fetch_optional(&s.db)
        .await
    else {
        return;
    };
    if let Err(e) = attach_repos(s, &c, &[ctx.repo.id]).await {
        tracing::warn!(error = %e.message, "applying the default code security configuration failed");
    }
}

/// 422 when an enforced configuration is attached (repository settings are locked).
pub async fn check_not_enforced(s: &AppState, repo_id: i64) -> ApiResult<()> {
    if let Some((c, _)) = attached(s, repo_id).await? {
        if c.enforcement == "enforced" {
            return Err(ApiError::unprocessable(format!("Security settings are enforced by the code security configuration \"{}\".", c.name)));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Handlers

type OrgId = Path<(String, i64)>;

async fn org_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let o = org_scope(&s, &u, &org).await?;
    list(&s, &uri, Level::Org, o.id, &o.login, q).await
}
async fn org_create(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<Map<String, Value>>) -> ApiResult<Response> {
    let o = org_scope(&s, &u, &org).await?;
    create(&s, Level::Org, o.id, &o.login, b).await
}
async fn org_defaults(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_scope(&s, &u, &org).await?;
    defaults(&s, Level::Org, o.id, &o.login).await
}
#[derive(Deserialize)]
struct DetachBody {
    selected_repository_ids: Option<Vec<i64>>,
}
async fn org_detach(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<DetachBody>) -> ApiResult<Response> {
    let o = org_scope(&s, &u, &org).await?;
    let ids = b.selected_repository_ids.ok_or_else(|| ApiError::validation("CodeSecurityConfiguration", "selected_repository_ids", "missing_field"))?;
    sqlx::query("DELETE FROM code_security_configuration_repos a USING repositories r WHERE r.id = a.repo_id AND r.owner_id = $1 AND a.repo_id = ANY($2)")
        .bind(o.id)
        .bind(&ids)
        .execute(&s.db)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
async fn org_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): OrgId) -> ApiResult<Json<Value>> {
    let o = org_scope(&s, &u, &org).await?;
    Ok(Json(config_json(&s, Level::Org, &o.login, &load(&s, Level::Org, o.id, id).await?)))
}
async fn org_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): OrgId, Json(b): Json<Map<String, Value>>) -> ApiResult<Response> {
    let o = org_scope(&s, &u, &org).await?;
    update(&s, Level::Org, o.id, &o.login, id, b).await
}
async fn org_delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): OrgId) -> ApiResult<Response> {
    let o = org_scope(&s, &u, &org).await?;
    remove(&s, Level::Org, o.id, id).await
}
async fn org_attach(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): OrgId, Json(b): Json<AttachBody>) -> ApiResult<Response> {
    let o = org_scope(&s, &u, &org).await?;
    attach(&s, Level::Org, o.id, id, b).await
}
async fn org_set_default(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): OrgId, Json(b): Json<DefaultBody>) -> ApiResult<Json<Value>> {
    let o = org_scope(&s, &u, &org).await?;
    set_default(&s, Level::Org, o.id, &o.login, id, b).await
}
async fn org_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((org, id)): OrgId, Query(q): Query<ReposQ>) -> ApiResult<Response> {
    let o = org_scope(&s, &u, &org).await?;
    repos(&s, &uri, Level::Org, o.id, id, q).await
}

async fn ent_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(e): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    list(&s, &uri, Level::Enterprise, 0, &e, q).await
}
async fn ent_create(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>, Json(b): Json<Map<String, Value>>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    create(&s, Level::Enterprise, 0, &e, b).await
}
async fn ent_defaults(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    super::enterprise_admin(&u, &e)?;
    defaults(&s, Level::Enterprise, 0, &e).await
}
async fn ent_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    super::enterprise_admin(&u, &e)?;
    Ok(Json(config_json(&s, Level::Enterprise, &e, &load(&s, Level::Enterprise, 0, id).await?)))
}
async fn ent_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, id)): Path<(String, i64)>, Json(b): Json<Map<String, Value>>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    update(&s, Level::Enterprise, 0, &e, id, b).await
}
async fn ent_delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, id)): Path<(String, i64)>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    remove(&s, Level::Enterprise, 0, id).await
}
async fn ent_attach(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, id)): Path<(String, i64)>, Json(b): Json<AttachBody>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    attach(&s, Level::Enterprise, 0, id, b).await
}
async fn ent_set_default(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, id)): Path<(String, i64)>, Json(b): Json<DefaultBody>) -> ApiResult<Json<Value>> {
    super::enterprise_admin(&u, &e)?;
    set_default(&s, Level::Enterprise, 0, &e, id, b).await
}
async fn ent_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((e, id)): Path<(String, i64)>, Query(q): Query<ReposQ>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    repos(&s, &uri, Level::Enterprise, 0, id, q).await
}

async fn repo_config(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(crate::db::Perm::Admin)?;
    match attached(&s, ctx.repo.id).await? {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some((c, status)) => {
            let (level, scope) = if c.target_type == "enterprise" { (Level::Enterprise, "forge".to_string()) } else { (Level::Org, ctx.owner.login.clone()) };
            Ok(Json(json!({ "status": status, "configuration": config_json(&s, level, &scope, &c) })).into_response())
        }
    }
}
