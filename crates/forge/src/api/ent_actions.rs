//! Enterprise-level GitHub Actions administration (`/enterprises/{enterprise}/actions/...`):
//! the enterprise Actions policy, runner groups and enterprise (instance-wide) self-hosted
//! runners. Site administrators only.
//!
//! The policy is stored in `ent_settings` (see [`actions_policy`] for other areas to enforce).
//! Runners live in the runner area's `actions_runners` table (rows with neither repo nor org);
//! when that table is not present (runner area not deployed) the enterprise has no runners and
//! registration tokens cannot be issued (503).
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::ent_admin::table_exists;
use super::{created, no_content, PageParams};
use crate::db::User;
use crate::enterprise::{self as ent, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let p = "/enterprises/{enterprise}/actions";
    Router::new()
        .route(&format!("{p}/permissions"), get(get_permissions).put(set_permissions))
        .route(&format!("{p}/permissions/artifact-and-log-retention"), get(get_retention).put(set_retention))
        .route(&format!("{p}/permissions/fork-pr-contributor-approval"), get(get_fork_approval).put(set_fork_approval))
        .route(&format!("{p}/permissions/fork-pr-workflows-private-repos"), get(get_fork_private).put(set_fork_private))
        .route(&format!("{p}/permissions/organizations"), get(list_enabled_orgs).put(set_enabled_orgs))
        .route(&format!("{p}/permissions/organizations/{{org_id}}"), put(enable_org).delete(disable_org))
        .route(&format!("{p}/permissions/selected-actions"), get(get_selected_actions).put(set_selected_actions))
        .route(&format!("{p}/permissions/self-hosted-runners"), get(get_self_hosted).put(set_self_hosted))
        .route(&format!("{p}/runner-groups"), get(list_groups).post(create_group))
        .route(&format!("{p}/runner-groups/{{group_id}}"), get(get_group).patch(update_group).delete(delete_group))
        .route(&format!("{p}/runner-groups/{{group_id}}/organizations"), get(list_group_orgs).put(set_group_orgs))
        .route(&format!("{p}/runner-groups/{{group_id}}/organizations/{{org_id}}"), put(add_group_org).delete(remove_group_org))
        .route(&format!("{p}/runner-groups/{{group_id}}/runners"), get(list_group_runners).put(set_group_runners))
        .route(&format!("{p}/runner-groups/{{group_id}}/runners/{{runner_id}}"), put(add_group_runner).delete(remove_group_runner))
        .route(&format!("{p}/runners"), get(list_runners))
        .route(&format!("{p}/runners/downloads"), get(downloads))
        .route(&format!("{p}/runners/registration-token"), post(registration_token))
        .route(&format!("{p}/runners/remove-token"), post(remove_token))
        .route(&format!("{p}/runners/{{runner_id}}"), get(get_runner).delete(delete_runner))
        .route(&format!("{p}/runners/{{runner_id}}/labels"), get(list_labels).post(add_labels).put(set_labels).delete(remove_custom_labels))
        .route(&format!("{p}/runners/{{runner_id}}/labels/{{name}}"), delete(remove_label))
}

fn base(s: &AppState) -> String {
    format!("{}/enterprises/{}/actions", s.api_url(), ent::slug())
}

// ---------------------------------------------------------------------------------------------
// Policy (stored settings)

/// The enterprise Actions policy: `enabled_organizations` (all|none|selected),
/// `allowed_actions` (all|local_only|selected), `sha_pinning_required`. Other areas can call
/// this (and [`org_actions_enabled`]) to enforce it.
pub async fn actions_policy(s: &AppState) -> ApiResult<Value> {
    ent::setting_with_defaults(&s.db, "actions_permissions", json!({"enabled_organizations": "all", "allowed_actions": "all", "sha_pinning_required": false})).await
}

/// Whether the enterprise policy lets organization `org_id` use Actions.
pub async fn org_actions_enabled(s: &AppState, org_id: i64) -> ApiResult<bool> {
    let p = actions_policy(s).await?;
    Ok(match p.get("enabled_organizations").and_then(Value::as_str) {
        Some("none") => false,
        Some("selected") => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ent_actions_orgs WHERE org_id = $1)").bind(org_id).fetch_one(&s.db).await?,
        _ => true,
    })
}

fn bad(field: &str) -> ApiError {
    ApiError::validation("ActionsPolicy", field, "invalid")
}

async fn get_permissions(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    let mut v = actions_policy(&s).await?;
    let b = base(&s);
    if v["enabled_organizations"] == "selected" {
        v["selected_organizations_url"] = json!(format!("{b}/permissions/organizations"));
    }
    if v["allowed_actions"] == "selected" {
        v["selected_actions_url"] = json!(format!("{b}/permissions/selected-actions"));
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct PermissionsBody {
    enabled_organizations: Option<String>,
    allowed_actions: Option<String>,
    sha_pinning_required: Option<bool>,
}

async fn set_permissions(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<PermissionsBody>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let mut v = actions_policy(&s).await?;
    let eo = b.enabled_organizations.ok_or_else(|| ApiError::validation("ActionsPolicy", "enabled_organizations", "missing_field"))?;
    if !matches!(eo.as_str(), "all" | "none" | "selected") {
        return Err(bad("enabled_organizations"));
    }
    v["enabled_organizations"] = json!(eo);
    if let Some(a) = b.allowed_actions {
        if !matches!(a.as_str(), "all" | "local_only" | "selected") {
            return Err(bad("allowed_actions"));
        }
        v["allowed_actions"] = json!(a);
    }
    if let Some(p) = b.sha_pinning_required {
        v["sha_pinning_required"] = json!(p);
    }
    ent::set_setting(&s.db, "actions_permissions", &v).await?;
    ent::audit(&s, "business.actions_policy_update", Some(&u), v.clone()).await;
    Ok(no_content())
}

async fn get_retention(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(ent::setting_with_defaults(&s.db, "actions_retention", json!({"days": 90, "maximum_allowed_days": 400})).await?))
}

#[derive(Deserialize)]
struct DaysBody {
    days: Option<i64>,
}

async fn set_retention(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<DaysBody>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let days = b.days.ok_or_else(|| ApiError::validation("ActionsPolicy", "days", "missing_field"))?;
    if !(1..=400).contains(&days) {
        return Err(bad("days"));
    }
    ent::set_setting(&s.db, "actions_retention", &json!({"days": days, "maximum_allowed_days": 400})).await?;
    ent::audit(&s, "business.update_actions_retention", Some(&u), json!({"days": days})).await;
    Ok(no_content())
}

async fn get_fork_approval(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(ent::setting_with_defaults(&s.db, "actions_fork_pr_approval", json!({"approval_policy": "first_time_contributors"})).await?))
}

#[derive(Deserialize)]
struct ApprovalBody {
    approval_policy: Option<String>,
}

async fn set_fork_approval(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<ApprovalBody>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let p = b.approval_policy.ok_or_else(|| ApiError::validation("ActionsPolicy", "approval_policy", "missing_field"))?;
    if !matches!(p.as_str(), "first_time_contributors_new_to_github" | "first_time_contributors" | "all_external_contributors") {
        return Err(bad("approval_policy"));
    }
    ent::set_setting(&s.db, "actions_fork_pr_approval", &json!({"approval_policy": p})).await?;
    ent::audit(&s, "business.actions_fork_pr_approval_policy_update", Some(&u), json!({"approval_policy": p})).await;
    Ok(no_content())
}

const FORK_PRIVATE_DEFAULTS: fn() -> Value = || {
    json!({
        "run_workflows_from_fork_pull_requests": false,
        "send_write_tokens_to_workflows": false,
        "send_secrets_and_variables": false,
        "require_approval_for_fork_pr_workflows": true,
    })
};

async fn get_fork_private(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(ent::setting_with_defaults(&s.db, "actions_fork_pr_private", FORK_PRIVATE_DEFAULTS()).await?))
}

async fn set_fork_private(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<Value>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let mut v = ent::setting_with_defaults(&s.db, "actions_fork_pr_private", FORK_PRIVATE_DEFAULTS()).await?;
    if !b.get("run_workflows_from_fork_pull_requests").is_some_and(Value::is_boolean) {
        return Err(ApiError::validation("ActionsPolicy", "run_workflows_from_fork_pull_requests", "missing_field"));
    }
    for k in ["run_workflows_from_fork_pull_requests", "send_write_tokens_to_workflows", "send_secrets_and_variables", "require_approval_for_fork_pr_workflows"] {
        match b.get(k) {
            None => {}
            Some(Value::Bool(x)) => v[k] = json!(x),
            Some(_) => return Err(bad(k)),
        }
    }
    ent::set_setting(&s.db, "actions_fork_pr_private", &v).await?;
    ent::audit(&s, "business.actions_fork_pr_workflows_policy_update", Some(&u), v.clone()).await;
    Ok(no_content())
}

async fn get_selected_actions(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(
        ent::setting_with_defaults(&s.db, "actions_selected_actions", json!({"github_owned_allowed": true, "verified_allowed": false, "patterns_allowed": []}))
            .await?,
    ))
}

async fn set_selected_actions(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<Value>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let mut v =
        ent::setting_with_defaults(&s.db, "actions_selected_actions", json!({"github_owned_allowed": true, "verified_allowed": false, "patterns_allowed": []})).await?;
    for k in ["github_owned_allowed", "verified_allowed"] {
        match b.get(k) {
            None => {}
            Some(Value::Bool(x)) => v[k] = json!(x),
            Some(_) => return Err(bad(k)),
        }
    }
    match b.get("patterns_allowed") {
        None => {}
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => v["patterns_allowed"] = json!(a),
        Some(_) => return Err(bad("patterns_allowed")),
    }
    ent::set_setting(&s.db, "actions_selected_actions", &v).await?;
    ent::audit(&s, "business.update_actions_settings", Some(&u), v.clone()).await;
    Ok(no_content())
}

async fn get_self_hosted(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(ent::setting_with_defaults(&s.db, "actions_self_hosted", json!({"disable_self_hosted_runners_for_all_orgs": false})).await?))
}

async fn set_self_hosted(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<Value>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let Some(Value::Bool(x)) = b.get("disable_self_hosted_runners_for_all_orgs") else {
        return Err(ApiError::validation("ActionsPolicy", "disable_self_hosted_runners_for_all_orgs", "missing_field"));
    };
    ent::set_setting(&s.db, "actions_self_hosted", &json!({"disable_self_hosted_runners_for_all_orgs": x})).await?;
    ent::audit(&s, "business.self_hosted_runners_policy_update", Some(&u), json!({"disabled": x})).await;
    Ok(no_content())
}

// Organizations selected for Actions -------------------------------------------------------

async fn org_by_id(s: &AppState, id: i64) -> ApiResult<User> {
    let o = crate::db::user_by_id(&s.db, id).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    Ok(o)
}

async fn orgs_page(s: &AppState, table: &str, filter: &str, id: Option<i64>, p: PageParams) -> ApiResult<Value> {
    let sql = format!("SELECT u.* FROM {table} t JOIN users u ON u.id = t.org_id WHERE {filter} ORDER BY u.id LIMIT $2 OFFSET $3");
    let rows: Vec<User> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table} t WHERE {filter}"))).bind(id).fetch_one(&s.db).await?;
    let api = s.api_url();
    Ok(json!({
        "total_count": total,
        "organizations": rows.iter().map(|o| crate::json::user::org_simple(&api, s.base_url(), o)).collect::<Vec<_>>(),
    }))
}

async fn list_enabled_orgs(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(orgs_page(&s, "ent_actions_orgs", "($1::bigint IS NULL OR TRUE)", None, p).await?))
}

#[derive(Deserialize)]
struct OrgIdsBody {
    selected_organization_ids: Option<Vec<i64>>,
}

async fn checked_org_ids(s: &AppState, ids: Option<Vec<i64>>) -> ApiResult<Vec<i64>> {
    let ids = ids.ok_or_else(|| ApiError::validation("ActionsPolicy", "selected_organization_ids", "missing_field"))?;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = ANY($1) AND kind = 'Organization'").bind(&ids).fetch_one(&s.db).await?;
    let unique: std::collections::HashSet<i64> = ids.iter().copied().collect();
    if n as usize != unique.len() {
        return Err(bad("selected_organization_ids"));
    }
    Ok(unique.into_iter().collect())
}

async fn set_enabled_orgs(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Json(b): Json<OrgIdsBody>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let ids = checked_org_ids(&s, b.selected_organization_ids).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM ent_actions_orgs").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO ent_actions_orgs (org_id) SELECT unnest($1::bigint[])").bind(&ids).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(no_content())
}

async fn enable_org(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, org_id)): Path<(String, i64)>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let o = org_by_id(&s, org_id).await?;
    sqlx::query("INSERT INTO ent_actions_orgs (org_id) VALUES ($1) ON CONFLICT DO NOTHING").bind(o.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn disable_org(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, org_id)): Path<(String, i64)>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let o = org_by_id(&s, org_id).await?;
    sqlx::query("DELETE FROM ent_actions_orgs WHERE org_id = $1").bind(o.id).execute(&s.db).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Runner groups

#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupRow {
    id: i64,
    name: String,
    visibility: String,
    is_default: bool,
    allows_public_repositories: bool,
    restricted_to_workflows: bool,
    selected_workflows: Vec<String>,
    #[allow(dead_code)]
    created_at: chrono::DateTime<chrono::Utc>,
}

fn group_json(s: &AppState, g: &GroupRow) -> Value {
    let url = format!("{}/runner-groups/{}", base(s), g.id);
    let mut v = json!({
        "id": g.id,
        "name": g.name,
        "visibility": g.visibility,
        "default": g.is_default,
        "runners_url": format!("{url}/runners"),
        "allows_public_repositories": g.allows_public_repositories,
        "workflow_restrictions_read_only": false,
        "restricted_to_workflows": g.restricted_to_workflows,
        "selected_workflows": g.selected_workflows,
    });
    if g.visibility == "selected" {
        v["selected_organizations_url"] = json!(format!("{url}/organizations"));
    }
    v
}

async fn load_group(s: &AppState, e: &str, id: i64) -> ApiResult<GroupRow> {
    ent::check_enterprise(e)?;
    sqlx::query_as("SELECT * FROM ent_runner_groups WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
struct GroupsQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    visible_to_organization: Option<String>,
}

async fn list_groups(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Query(q): Query<GroupsQuery>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let org_id: Option<i64> = match q.visible_to_organization.as_deref() {
        Some(o) => Some(crate::orgs::org_by_login(&s, o).await?.id),
        None => None,
    };
    let filter = "($1::bigint IS NULL OR g.visibility = 'all' OR EXISTS (SELECT 1 FROM ent_runner_group_orgs x WHERE x.group_id = g.id AND x.org_id = $1))";
    let rows: Vec<GroupRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT g.* FROM ent_runner_groups g WHERE {filter} ORDER BY g.id LIMIT $2 OFFSET $3")))
        .bind(org_id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM ent_runner_groups g WHERE {filter}"))).bind(org_id).fetch_one(&s.db).await?;
    Ok(Json(json!({ "total_count": total, "runner_groups": rows.iter().map(|g| group_json(&s, g)).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct GroupBody {
    name: Option<String>,
    visibility: Option<String>,
    selected_organization_ids: Option<Vec<i64>>,
    runners: Option<Vec<i64>>,
    allows_public_repositories: Option<bool>,
    restricted_to_workflows: Option<bool>,
    selected_workflows: Option<Vec<String>>,
}

fn check_visibility(v: &Option<String>) -> ApiResult<()> {
    match v.as_deref() {
        None | Some("all" | "selected") => Ok(()),
        _ => Err(ApiError::validation("RunnerGroup", "visibility", "invalid")),
    }
}

async fn create_group(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<GroupBody>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let name = b.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("RunnerGroup", "name", "missing_field"))?;
    check_visibility(&b.visibility)?;
    let orgs = match b.selected_organization_ids {
        Some(ids) => checked_org_ids(&s, Some(ids)).await?,
        None => vec![],
    };
    let mut tx = s.db.begin().await?;
    let g: GroupRow = sqlx::query_as(
        "INSERT INTO ent_runner_groups (name, visibility, allows_public_repositories, restricted_to_workflows, selected_workflows)
         VALUES ($1, $2, $3, $4, $5) RETURNING *",
    )
    .bind(name.trim())
    .bind(b.visibility.as_deref().unwrap_or("all"))
    .bind(b.allows_public_repositories.unwrap_or(false))
    .bind(b.restricted_to_workflows.unwrap_or(false))
    .bind(b.selected_workflows.unwrap_or_default())
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("RunnerGroup", "name", "already_exists"),
        e => e.into(),
    })?;
    sqlx::query("INSERT INTO ent_runner_group_orgs (group_id, org_id) SELECT $1, unnest($2::bigint[])").bind(g.id).bind(&orgs).execute(&mut *tx).await?;
    tx.commit().await?;
    if let Some(r) = b.runners {
        move_runners(&s, g.id, &r, false).await?;
    }
    ent::audit(&s, "business.runner_group_created", Some(&u), json!({"name": g.name, "runner_group_id": g.id})).await;
    Ok(created(group_json(&s, &g)))
}

async fn get_group(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let g = load_group(&s, &e, id).await?;
    Ok(Json(group_json(&s, &g)))
}

async fn update_group(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<GroupBody>) -> ApiResult<Json<Value>> {
    let g = load_group(&s, &e, id).await?;
    check_visibility(&b.visibility)?;
    if g.is_default && b.name.as_ref().is_some_and(|n| *n != g.name) {
        return Err(ApiError::unprocessable("The default runner group cannot be renamed."));
    }
    let g: GroupRow = sqlx::query_as(
        "UPDATE ent_runner_groups SET name = COALESCE($2, name), visibility = COALESCE($3, visibility),
            allows_public_repositories = COALESCE($4, allows_public_repositories),
            restricted_to_workflows = COALESCE($5, restricted_to_workflows), selected_workflows = COALESCE($6, selected_workflows)
          WHERE id = $1 RETURNING *",
    )
    .bind(g.id)
    .bind(b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
    .bind(&b.visibility)
    .bind(b.allows_public_repositories)
    .bind(b.restricted_to_workflows)
    .bind(&b.selected_workflows)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("RunnerGroup", "name", "already_exists"),
        e => e.into(),
    })?;
    ent::audit(&s, "business.runner_group_updated", Some(&u), json!({"name": g.name, "runner_group_id": g.id})).await;
    Ok(Json(group_json(&s, &g)))
}

async fn delete_group(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let g = load_group(&s, &e, id).await?;
    if g.is_default {
        return Err(ApiError::unprocessable("The default runner group cannot be deleted."));
    }
    // Its runners go back to the Default group.
    if table_exists(&s, "actions_runners").await? {
        sqlx::query("UPDATE actions_runners SET runner_group_id = 1 WHERE runner_group_id = $1 AND repo_id IS NULL AND org_id IS NULL")
            .bind(g.id)
            .execute(&s.db)
            .await?;
    }
    sqlx::query("DELETE FROM ent_runner_groups WHERE id = $1").bind(g.id).execute(&s.db).await?;
    ent::audit(&s, "business.runner_group_removed", Some(&u), json!({"name": g.name, "runner_group_id": g.id})).await;
    Ok(no_content())
}

async fn list_group_orgs(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Json<Value>> {
    let g = load_group(&s, &e, id).await?;
    Ok(Json(orgs_page(&s, "ent_runner_group_orgs", "t.group_id = $1", Some(g.id), p).await?))
}

async fn set_group_orgs(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<OrgIdsBody>) -> ApiResult<Response> {
    let g = load_group(&s, &e, id).await?;
    let ids = checked_org_ids(&s, b.selected_organization_ids).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM ent_runner_group_orgs WHERE group_id = $1").bind(g.id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO ent_runner_group_orgs (group_id, org_id) SELECT $1, unnest($2::bigint[])").bind(g.id).bind(&ids).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(no_content())
}

async fn add_group_org(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id, org_id)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    let g = load_group(&s, &e, id).await?;
    let o = org_by_id(&s, org_id).await?;
    sqlx::query("INSERT INTO ent_runner_group_orgs (group_id, org_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(g.id).bind(o.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn remove_group_org(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id, org_id)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    let g = load_group(&s, &e, id).await?;
    let o = org_by_id(&s, org_id).await?;
    sqlx::query("DELETE FROM ent_runner_group_orgs WHERE group_id = $1 AND org_id = $2").bind(g.id).bind(o.id).execute(&s.db).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Runners (the runner area's actions_runners, enterprise scope)

#[derive(sqlx::FromRow)]
struct RunnerRow {
    id: i64,
    name: String,
    os: String,
    arch: String,
    labels: Vec<String>,
    ephemeral: bool,
    version: Option<String>,
    runner_group_id: i64,
    online: bool,
    busy: bool,
}

const RUNNER_SELECT: &str = "SELECT r.id, r.name, r.os, r.arch, r.labels, r.ephemeral, r.version, r.runner_group_id,
        COALESCE(r.last_seen_at > now() - interval '2 minutes', FALSE) AS online,
        EXISTS (SELECT 1 FROM actions_jobs j WHERE j.runner_id = r.id AND j.status = 'in_progress') AS busy
   FROM actions_runners r WHERE r.repo_id IS NULL AND r.org_id IS NULL";

fn system_labels(r: &RunnerRow) -> Vec<String> {
    vec!["self-hosted".into(), r.os.to_lowercase().replace("macos", "macOS"), r.arch.to_lowercase()]
}

fn is_system(r: &RunnerRow, l: &str) -> bool {
    system_labels(r).iter().any(|x| x.eq_ignore_ascii_case(l))
}

fn labels_json(r: &RunnerRow) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, l) in system_labels(r).into_iter().chain(r.labels.iter().cloned()).enumerate() {
        if seen.insert(l.to_lowercase()) {
            let ty = if is_system(r, &l) { "read-only" } else { "custom" };
            out.push(json!({ "id": i as i64 + 1, "name": l, "type": ty }));
        }
    }
    out
}

fn runner_json(r: &RunnerRow) -> Value {
    json!({
        "id": r.id,
        "runner_group_id": r.runner_group_id,
        "name": r.name,
        "os": r.os,
        "status": if r.online { "online" } else { "offline" },
        "busy": r.busy,
        "labels": labels_json(r),
        "ephemeral": r.ephemeral,
        "version": r.version,
    })
}

async fn runners_available(s: &AppState) -> ApiResult<bool> {
    Ok(table_exists(s, "actions_runners").await? && table_exists(s, "actions_jobs").await?)
}

async fn load_runner(s: &AppState, e: &str, id: i64) -> ApiResult<RunnerRow> {
    ent::check_enterprise(e)?;
    if !runners_available(s).await? {
        return Err(ApiError::not_found());
    }
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{RUNNER_SELECT} AND r.id = $1"))).bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
struct RunnersQuery {
    name: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn runners_page(s: &AppState, group: Option<i64>, name: Option<&str>, p: PageParams) -> ApiResult<Value> {
    if !runners_available(s).await? {
        return Ok(json!({ "total_count": 0, "runners": [] }));
    }
    let filter = "AND ($1::bigint IS NULL OR r.runner_group_id = $1) AND ($2::text IS NULL OR lower(r.name) = lower($2))";
    let rows: Vec<RunnerRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{RUNNER_SELECT} {filter} ORDER BY r.id LIMIT $3 OFFSET $4")))
        .bind(group)
        .bind(name)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM ({RUNNER_SELECT} {filter}) x"))).bind(group).bind(name).fetch_one(&s.db).await?;
    Ok(json!({ "total_count": total, "runners": rows.iter().map(runner_json).collect::<Vec<_>>() }))
}

async fn list_runners(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Query(q): Query<RunnersQuery>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(runners_page(&s, None, q.name.as_deref(), PageParams { page: q.page, per_page: q.per_page }).await?))
}

async fn list_group_runners(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Json<Value>> {
    let g = load_group(&s, &e, id).await?;
    Ok(Json(runners_page(&s, Some(g.id), None, p).await?))
}

/// Moves enterprise runners into group `group_id` (`exclusive`: the group then contains only
/// these; runners taken out return to the Default group).
async fn move_runners(s: &AppState, group_id: i64, ids: &[i64], exclusive: bool) -> ApiResult<()> {
    if !runners_available(s).await? {
        if ids.is_empty() {
            return Ok(());
        }
        return Err(ApiError::validation("RunnerGroup", "runners", "invalid"));
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM actions_runners WHERE id = ANY($1) AND repo_id IS NULL AND org_id IS NULL")
        .bind(ids)
        .fetch_one(&s.db)
        .await?;
    let unique: std::collections::HashSet<&i64> = ids.iter().collect();
    if n as usize != unique.len() {
        return Err(ApiError::validation("RunnerGroup", "runners", "invalid"));
    }
    let mut tx = s.db.begin().await?;
    if exclusive {
        sqlx::query("UPDATE actions_runners SET runner_group_id = 1 WHERE runner_group_id = $1 AND repo_id IS NULL AND org_id IS NULL AND NOT (id = ANY($2))")
            .bind(group_id)
            .bind(ids)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE actions_runners SET runner_group_id = $1 WHERE id = ANY($2)").bind(group_id).bind(ids).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Deserialize)]
struct RunnerIdsBody {
    runners: Option<Vec<i64>>,
}

async fn set_group_runners(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<RunnerIdsBody>) -> ApiResult<Response> {
    let g = load_group(&s, &e, id).await?;
    let ids = b.runners.ok_or_else(|| ApiError::validation("RunnerGroup", "runners", "missing_field"))?;
    move_runners(&s, g.id, &ids, true).await?;
    Ok(no_content())
}

async fn add_group_runner(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id, rid)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    let g = load_group(&s, &e, id).await?;
    let r = load_runner(&s, &e, rid).await?;
    move_runners(&s, g.id, &[r.id], false).await?;
    Ok(no_content())
}

async fn remove_group_runner(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id, rid)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    let g = load_group(&s, &e, id).await?;
    let r = load_runner(&s, &e, rid).await?;
    if r.runner_group_id == g.id && !g.is_default {
        move_runners(&s, 1, &[r.id], false).await?;
    }
    Ok(no_content())
}

async fn get_runner(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    Ok(Json(runner_json(&r)))
}

async fn delete_runner(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let r = load_runner(&s, &e, id).await?;
    if r.busy {
        return Err(ApiError::unprocessable("Bad request - Runner \"{}\" is still running a job\"".replace("{}", &r.name)));
    }
    sqlx::query("DELETE FROM actions_runners WHERE id = $1").bind(r.id).execute(&s.db).await?;
    ent::audit(&s, "business.remove_self_hosted_runner", Some(&u), json!({"runner_id": r.id, "name": r.name})).await;
    Ok(no_content())
}

/// Runner application downloads (actions/runner releases; forge serves no binaries itself).
async fn downloads(SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    const V: &str = "2.321.0";
    let item = |os: &str, arch: &str, ext: &str| {
        let osn = match os {
            "osx" => "osx",
            "win" => "win",
            _ => "linux",
        };
        let file = format!("actions-runner-{osn}-{arch}-{V}.{ext}");
        json!({
            "os": os,
            "architecture": arch,
            "download_url": format!("https://github.com/actions/runner/releases/download/v{V}/{file}"),
            "filename": file,
        })
    };
    Ok(Json(json!([
        item("osx", "x64", "tar.gz"),
        item("osx", "arm64", "tar.gz"),
        item("linux", "x64", "tar.gz"),
        item("linux", "arm", "tar.gz"),
        item("linux", "arm64", "tar.gz"),
        item("win", "x64", "zip"),
        item("win", "arm64", "zip"),
    ])))
}

async fn runner_token(s: &AppState, kind: &str) -> ApiResult<Response> {
    if !table_exists(s, "actions_runner_tokens").await? {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "Self-hosted runners are not available on this instance."));
    }
    let token = crate::auth::random_string(29).to_uppercase();
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    sqlx::query("INSERT INTO actions_runner_tokens (token_hash, kind, repo_id, org_id, expires_at) VALUES ($1, $2, NULL, NULL, $3)")
        .bind(crate::auth::sha256_hex(&token))
        .bind(kind)
        .bind(expires)
        .execute(&s.db)
        .await?;
    Ok((StatusCode::CREATED, axum::Json(json!({ "token": token, "expires_at": crate::json::ts(expires) }))).into_response())
}

async fn registration_token(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    runner_token(&s, "registration").await
}

async fn remove_token(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    runner_token(&s, "remove").await
}

fn labels_response(r: &RunnerRow) -> Json<Value> {
    let l = labels_json(r);
    Json(json!({ "total_count": l.len(), "labels": l }))
}

async fn list_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    Ok(labels_response(&load_runner(&s, &e, id).await?))
}

#[derive(Deserialize)]
struct LabelsBody {
    labels: Option<Vec<String>>,
}

fn checked_labels(b: LabelsBody, allow_empty: bool) -> ApiResult<Vec<String>> {
    let l = b.labels.ok_or_else(|| ApiError::validation("Label", "labels", "missing_field"))?;
    if (!allow_empty && l.is_empty()) || l.len() > 100 || l.iter().any(|x| x.trim().is_empty() || x.len() > 256) {
        return Err(ApiError::validation("Label", "labels", "invalid"));
    }
    Ok(l.into_iter().map(|x| x.trim().to_string()).collect())
}

async fn store_labels(s: &AppState, e: &str, r: &RunnerRow, custom: Vec<String>) -> ApiResult<Json<Value>> {
    let mut out: Vec<String> = Vec::new();
    for l in custom {
        if !is_system(r, &l) && !out.iter().any(|x| x.eq_ignore_ascii_case(&l)) {
            out.push(l);
        }
    }
    sqlx::query("UPDATE actions_runners SET labels = $2 WHERE id = $1").bind(r.id).bind(&out).execute(&s.db).await?;
    Ok(labels_response(&load_runner(s, e, r.id).await?))
}

fn custom_labels(r: &RunnerRow) -> Vec<String> {
    r.labels.iter().filter(|l| !is_system(r, l)).cloned().collect()
}

async fn add_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<LabelsBody>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    let mut l = custom_labels(&r);
    l.extend(checked_labels(b, false)?);
    store_labels(&s, &e, &r, l).await
}

async fn set_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<LabelsBody>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    store_labels(&s, &e, &r, checked_labels(b, true)?).await
}

async fn remove_custom_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    store_labels(&s, &e, &r, vec![]).await
}

async fn remove_label(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id, name)): Path<(String, i64, String)>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    let l = custom_labels(&r);
    if !l.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::not_found());
    }
    store_labels(&s, &e, &r, l.into_iter().filter(|x| !x.eq_ignore_ascii_case(&name)).collect()).await
}
