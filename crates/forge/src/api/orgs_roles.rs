//! Custom repository roles, organization roles (predefined `all_repo_*`/`security_manager` and
//! custom ones) with their user/team assignments, security manager teams, and the
//! fine-grained permission catalogues.
//!
//! Effects: a custom repository role granted to a team or collaborator counts as its base role;
//! an organization role's `base_role` applies to every repository of the org; org-level
//! fine-grained permissions gate the org endpoints that check `orgs::require_capability`
//! (audit log, roles, ruleset history, security products, ...). See `orgs::perm`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::{get, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::orgs_teams::{load_team, team_simple, teams_json, TeamRow};
use super::{created, no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::orgs::{audit_quiet, org_by_login, org_role, require_capability};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/orgs/{org}/organization-roles";
    Router::new()
        .route("/organizations/{organization_id}/custom_roles", get(list_custom_roles_by_id))
        .route("/orgs/{org}/custom-repository-roles", get(list_repo_roles).post(create_repo_role))
        .route("/orgs/{org}/custom-repository-roles/{role_id}", get(get_repo_role).patch(update_repo_role).delete(delete_repo_role))
        .route("/orgs/{org}/organization-fine-grained-permissions", get(org_permissions))
        .route("/orgs/{org}/repository-fine-grained-permissions", get(repo_permissions))
        .route(r, get(list_org_roles).post(create_org_role))
        .route(&format!("{r}/{{role_id}}"), get(get_org_role).patch(update_org_role).delete(delete_org_role))
        .route(&format!("{r}/{{role_id}}/teams"), get(role_teams))
        .route(&format!("{r}/{{role_id}}/users"), get(role_users))
        .route(&format!("{r}/teams/{{team_slug}}"), axum::routing::delete(revoke_all_team))
        .route(&format!("{r}/teams/{{team_slug}}/{{role_id}}"), put(assign_team).delete(revoke_team))
        .route(&format!("{r}/users/{{username}}"), axum::routing::delete(revoke_all_user))
        .route(&format!("{r}/users/{{username}}/{{role_id}}"), put(assign_user).delete(revoke_user))
        .route("/orgs/{org}/security-managers", get(security_managers))
        .route("/orgs/{org}/security-managers/teams/{team_slug}", put(add_security_manager).delete(remove_security_manager))
}

/// Id of the predefined `security_manager` organization role.
pub const SECURITY_MANAGER_ROLE: i64 = 138;

/// Fine-grained permissions that custom repository roles can add on top of their base role.
pub const REPO_PERMISSIONS: &[(&str, &str)] = &[
    ("add_assignee", "Assign or remove a user"),
    ("remove_assignee", "Remove an assigned user"),
    ("add_label", "Add or remove a label"),
    ("remove_label", "Remove a label"),
    ("close_issue", "Close an issue"),
    ("reopen_issue", "Reopen a closed issue"),
    ("delete_issue", "Delete an issue"),
    ("mark_as_duplicate", "Mark an issue as a duplicate"),
    ("set_milestone", "Set the milestone on an issue or pull request"),
    ("close_pull_request", "Close a pull request"),
    ("reopen_pull_request", "Reopen a closed pull request"),
    ("request_pr_review", "Request a pull request review"),
    ("manage_topics", "Manage topics"),
    ("manage_settings_wiki", "Manage wiki settings"),
    ("manage_settings_projects", "Manage project settings"),
    ("manage_settings_merge_types", "Manage pull request merging settings"),
    ("manage_settings_pages", "Manage GitHub Pages settings"),
    ("manage_webhooks", "Manage webhooks"),
    ("manage_deploy_keys", "Manage deploy keys"),
    ("edit_repo_metadata", "Edit repository metadata"),
    ("set_interaction_limits", "Set interaction limits"),
    ("set_social_preview", "Set the social preview"),
    ("push_protected_branch", "Push to protected branches"),
    ("edit_repo_protections", "Manage branch protections"),
    ("edit_repo_custom_properties_values", "Edit custom property values"),
    ("bypass_branch_protection", "Bypass branch protections"),
    ("view_secret_scanning_alerts", "View secret scanning results"),
    ("resolve_secret_scanning_alerts", "Dismiss or reopen secret scanning results"),
    ("read_code_scanning", "Read code scanning results"),
    ("write_code_scanning", "Write code scanning results"),
    ("delete_alerts_code_scanning", "Delete code scanning results"),
    ("view_dependabot_alerts", "View Dependabot alerts"),
    ("resolve_dependabot_alerts", "Dismiss or reopen Dependabot alerts"),
    ("create_discussion_category", "Create a discussion category"),
    ("edit_discussion_category", "Edit a discussion category"),
    ("delete_discussion", "Delete a discussion"),
    ("toggle_discussion_answer", "Mark or unmark a discussion answer"),
    ("convert_issues_to_discussions", "Convert issues to discussions"),
    ("create_tag", "Create tags"),
    ("delete_tag", "Delete tags"),
];

/// Fine-grained organization permissions for custom organization roles.
pub const ORG_PERMISSIONS: &[(&str, &str)] = &[
    ("read_audit_logs", "Read the organization's audit log"),
    ("read_organization_custom_org_role", "View organization roles"),
    ("write_organization_custom_org_role", "Manage custom organization roles"),
    ("read_organization_custom_repo_role", "View custom repository roles"),
    ("write_organization_custom_repo_role", "Manage custom repository roles"),
    ("manage_organization_webhooks", "Manage organization webhooks"),
    ("manage_organization_ref_rules", "Manage organization rulesets"),
    ("read_organization_custom_properties", "Read custom property definitions"),
    ("write_organization_custom_properties", "Manage custom property definitions"),
    ("edit_org_custom_properties_values", "Edit custom property values of repositories"),
    ("read_organization_actions_usage_metrics", "View Actions usage metrics"),
    ("write_organization_actions_secrets", "Manage organization Actions secrets"),
    ("write_organization_actions_variables", "Manage organization Actions variables"),
    ("write_organization_actions_settings", "Manage organization Actions settings"),
    ("read_organization_runners_and_runner_groups", "View runners and runner groups"),
    ("write_organization_runners_and_runner_groups", "Manage runners and runner groups"),
    ("read_organization_private_registries", "View private registry configurations"),
    ("write_organization_private_registries", "Manage private registry configurations"),
    ("write_organization_security_products", "Enable or disable security products"),
    ("manage_organization_announcement_banners", "Manage the announcement banner"),
    ("read_code_scanning", "Read code scanning results on all repositories"),
    ("write_code_scanning", "Write code scanning results on all repositories"),
    ("read_secret_scanning", "Read secret scanning results on all repositories"),
    ("write_secret_scanning", "Write secret scanning results on all repositories"),
    ("read_dependabot_alerts", "Read Dependabot alerts on all repositories"),
    ("write_dependabot_alerts", "Write Dependabot alerts on all repositories"),
    ("read_security_events", "Read security events"),
    ("write_security_events", "Write security events"),
];

fn simple_org(s: &AppState, o: &User) -> Value {
    crate::json::user::simple(&s.api_url(), s.base_url(), o)
}

// ---------------------------------------------------------------------------------------------
// Custom repository roles

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepoRoleRow {
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub base_role: String,
    pub permissions: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn repo_role_json(s: &AppState, o: &User, r: &RepoRoleRow) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "description": r.description,
        "base_role": r.base_role,
        "permissions": r.permissions,
        "organization": simple_org(s, o),
        "created_at": crate::json::ts(r.created_at),
        "updated_at": crate::json::ts(r.updated_at),
    })
}

const READ_REPO_ROLES: &[&str] = &["read_organization_custom_repo_role", "write_organization_custom_repo_role"];
const WRITE_REPO_ROLES: &[&str] = &["write_organization_custom_repo_role"];

async fn repo_roles_body(s: &AppState, o: &User) -> ApiResult<Value> {
    let rows: Vec<RepoRoleRow> = sqlx::query_as("SELECT * FROM custom_repo_roles WHERE org_id = $1 ORDER BY id").bind(o.id).fetch_all(&s.db).await?;
    Ok(json!({ "total_count": rows.len(), "custom_roles": rows.iter().map(|r| repo_role_json(s, o, r)).collect::<Vec<_>>() }))
}

async fn list_repo_roles(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_REPO_ROLES).await?;
    Ok(Json(repo_roles_body(&s, &o).await?))
}

async fn list_custom_roles_by_id(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let o = db::user_by_id(&s.db, id).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    require_capability(&s, &o, &u, READ_REPO_ROLES).await?;
    Ok(Json(repo_roles_body(&s, &o).await?))
}

#[derive(Deserialize, Default)]
pub struct RoleInput {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    pub base_role: Option<Option<String>>,
    pub permissions: Option<Vec<String>>,
}

fn check_permissions(resource: &str, perms: &[String], catalogue: &[(&str, &str)]) -> ApiResult<()> {
    for p in perms {
        if !catalogue.iter().any(|(n, _)| n == p) {
            let mut e = ApiError::validation(resource, "permissions", "invalid");
            e.errors = Some(json!([{ "resource": resource, "field": "permissions", "code": "invalid", "message": format!("{p} is not a valid permission") }]));
            return Err(e);
        }
    }
    Ok(())
}

const BUILTIN_ROLE_NAMES: &[&str] = &["read", "pull", "triage", "write", "push", "maintain", "admin", "none"];

fn check_role_name(resource: &str, name: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::validation(resource, "name", "missing_field"));
    }
    if BUILTIN_ROLE_NAMES.contains(&name.to_lowercase().as_str()) {
        return Err(ApiError::validation(resource, "name", "invalid"));
    }
    Ok(())
}

fn unique_violation(resource: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation(resource, "name", "already_exists"),
        e => e.into(),
    }
}

pub async fn create_repo_role_svc(s: &AppState, o: &User, actor: &User, req: RoleInput) -> ApiResult<RepoRoleRow> {
    require_capability(s, o, actor, WRITE_REPO_ROLES).await?;
    let name = req.name.unwrap_or_default();
    check_role_name("CustomRepositoryRole", &name)?;
    let base = req.base_role.flatten().ok_or_else(|| ApiError::validation("CustomRepositoryRole", "base_role", "missing_field"))?;
    if !["read", "triage", "write", "maintain"].contains(&base.as_str()) {
        return Err(ApiError::validation("CustomRepositoryRole", "base_role", "invalid"));
    }
    let perms = req.permissions.ok_or_else(|| ApiError::validation("CustomRepositoryRole", "permissions", "missing_field"))?;
    check_permissions("CustomRepositoryRole", &perms, REPO_PERMISSIONS)?;
    let r: RepoRoleRow = sqlx::query_as(
        "INSERT INTO custom_repo_roles (org_id, name, description, base_role, permissions) VALUES ($1,$2,$3,$4,$5) RETURNING *",
    )
    .bind(o.id)
    .bind(name.trim())
    .bind(req.description.flatten())
    .bind(&base)
    .bind(&perms)
    .fetch_one(&s.db)
    .await
    .map_err(unique_violation("CustomRepositoryRole"))?;
    audit_quiet(&s.db, o, Some(actor), "role.create", json!({ "name": r.name, "base_role": r.base_role })).await;
    Ok(r)
}

async fn create_repo_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(req): Json<RoleInput>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let r = create_repo_role_svc(&s, &o, &u, req).await?;
    Ok(created(repo_role_json(&s, &o, &r)))
}

async fn load_repo_role(s: &AppState, o: &User, id: i64) -> ApiResult<RepoRoleRow> {
    sqlx::query_as("SELECT * FROM custom_repo_roles WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(o.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_repo_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_REPO_ROLES).await?;
    Ok(Json(repo_role_json(&s, &o, &load_repo_role(&s, &o, id).await?)))
}

/// Replaces a role name in every team and collaborator grant of the org's repositories.
async fn rename_grants(tx: &mut sqlx::PgConnection, org_id: i64, from: &str, to: &str) -> ApiResult<()> {
    sqlx::query("UPDATE team_repos SET permission = $3 WHERE lower(permission) = lower($2) AND team_id IN (SELECT id FROM teams WHERE org_id = $1)")
        .bind(org_id)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE repo_collaborators SET permission = $3 WHERE lower(permission) = lower($2) AND repo_id IN (SELECT id FROM repositories WHERE owner_id = $1)")
        .bind(org_id)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

async fn update_repo_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Json(req): Json<RoleInput>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_REPO_ROLES).await?;
    let cur = load_repo_role(&s, &o, id).await?;
    if let Some(n) = &req.name {
        check_role_name("CustomRepositoryRole", n)?;
    }
    let base = match req.base_role {
        Some(Some(b)) if ["read", "triage", "write", "maintain"].contains(&b.as_str()) => b,
        Some(_) => return Err(ApiError::validation("CustomRepositoryRole", "base_role", "invalid")),
        None => cur.base_role.clone(),
    };
    if let Some(p) = &req.permissions {
        check_permissions("CustomRepositoryRole", p, REPO_PERMISSIONS)?;
    }
    let mut tx = s.db.begin().await?;
    let r: RepoRoleRow = sqlx::query_as(
        "UPDATE custom_repo_roles SET name = COALESCE($2, name), description = CASE WHEN $3 THEN $4 ELSE description END,
            base_role = $5, permissions = COALESCE($6, permissions), updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(req.name.as_deref().map(str::trim))
    .bind(req.description.is_some())
    .bind(req.description.clone().flatten())
    .bind(&base)
    .bind(&req.permissions)
    .fetch_one(&mut *tx)
    .await
    .map_err(unique_violation("CustomRepositoryRole"))?;
    if r.name != cur.name {
        rename_grants(&mut tx, o.id, &cur.name, &r.name).await?;
    }
    tx.commit().await?;
    audit_quiet(&s.db, &o, Some(&u), "role.update", json!({ "name": r.name })).await;
    Ok(Json(repo_role_json(&s, &o, &r)))
}

/// Deletes a custom repository role; teams and collaborators that held it keep its base role.
async fn delete_repo_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_REPO_ROLES).await?;
    let Some(r) = sqlx::query_as::<_, RepoRoleRow>("SELECT * FROM custom_repo_roles WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(o.id)
        .fetch_optional(&s.db)
        .await?
    else {
        return Ok(no_content());
    };
    let legacy = match r.base_role.as_str() {
        "read" => "pull",
        "write" => "push",
        other => other,
    };
    let mut tx = s.db.begin().await?;
    rename_grants(&mut tx, o.id, &r.name, legacy).await?;
    sqlx::query("DELETE FROM custom_repo_roles WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    audit_quiet(&s.db, &o, Some(&u), "role.destroy", json!({ "name": r.name })).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Fine-grained permission catalogues

async fn org_permissions(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_ORG_ROLES).await?;
    Ok(Json(json!(ORG_PERMISSIONS.iter().map(|(n, d)| json!({ "name": n, "description": d })).collect::<Vec<_>>())))
}

async fn repo_permissions(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_REPO_ROLES).await?;
    Ok(Json(json!(REPO_PERMISSIONS.iter().map(|(n, d)| json!({ "name": n, "description": d })).collect::<Vec<_>>())))
}

// ---------------------------------------------------------------------------------------------
// Organization roles

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrgRoleRow {
    pub id: i64,
    pub org_id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub base_role: Option<String>,
    pub permissions: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const READ_ORG_ROLES: &[&str] = &["read_organization_custom_org_role", "write_organization_custom_org_role"];
const WRITE_ORG_ROLES: &[&str] = &["write_organization_custom_org_role"];

fn org_role_json(s: &AppState, o: &User, r: &OrgRoleRow) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "description": r.description,
        "base_role": r.base_role,
        "source": if r.org_id.is_some() { "Organization" } else { "Predefined" },
        "permissions": r.permissions,
        "organization": simple_org(s, o),
        "created_at": crate::json::ts(r.created_at),
        "updated_at": crate::json::ts(r.updated_at),
    })
}

/// A role usable in `o`: predefined or the org's own.
pub async fn load_org_role(s: &AppState, o: &User, id: i64) -> ApiResult<OrgRoleRow> {
    sqlx::query_as("SELECT * FROM org_roles WHERE id = $1 AND (org_id IS NULL OR org_id = $2)")
        .bind(id)
        .bind(o.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn list_org_roles(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_ORG_ROLES).await?;
    let rows: Vec<OrgRoleRow> = sqlx::query_as("SELECT * FROM org_roles WHERE org_id IS NULL OR org_id = $1 ORDER BY org_id NULLS FIRST, id")
        .bind(o.id)
        .fetch_all(&s.db)
        .await?;
    Ok(Json(json!({ "total_count": rows.len(), "roles": rows.iter().map(|r| org_role_json(&s, &o, r)).collect::<Vec<_>>() })))
}

fn check_org_base(b: Option<&str>) -> ApiResult<Option<String>> {
    match b {
        None | Some("none") => Ok(None),
        Some(x) if ["read", "triage", "write", "maintain", "admin"].contains(&x) => Ok(Some(x.to_string())),
        Some(_) => Err(ApiError::validation("OrganizationRole", "base_role", "invalid")),
    }
}

async fn create_org_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(req): Json<RoleInput>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let name = req.name.unwrap_or_default();
    check_role_name("OrganizationRole", &name)?;
    let perms = req.permissions.ok_or_else(|| ApiError::validation("OrganizationRole", "permissions", "missing_field"))?;
    check_permissions("OrganizationRole", &perms, ORG_PERMISSIONS)?;
    let base = check_org_base(req.base_role.flatten().as_deref())?;
    let predefined: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_roles WHERE org_id IS NULL AND lower(name) = lower($1))")
        .bind(name.trim())
        .fetch_one(&s.db)
        .await?;
    if predefined {
        return Err(ApiError::validation("OrganizationRole", "name", "already_exists"));
    }
    let r: OrgRoleRow = sqlx::query_as(
        "INSERT INTO org_roles (org_id, name, description, base_role, permissions) VALUES ($1,$2,$3,$4,$5) RETURNING *",
    )
    .bind(o.id)
    .bind(name.trim())
    .bind(req.description.flatten())
    .bind(&base)
    .bind(&perms)
    .fetch_one(&s.db)
    .await
    .map_err(unique_violation("OrganizationRole"))?;
    audit_quiet(&s.db, &o, Some(&u), "organization_role.create", json!({ "name": r.name })).await;
    Ok(created(org_role_json(&s, &o, &r)))
}

async fn get_org_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_ORG_ROLES).await?;
    Ok(Json(org_role_json(&s, &o, &load_org_role(&s, &o, id).await?)))
}

async fn update_org_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Json(req): Json<RoleInput>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let cur = load_org_role(&s, &o, id).await?;
    if cur.org_id.is_none() {
        return Err(ApiError::unprocessable("Predefined organization roles cannot be changed."));
    }
    if let Some(n) = &req.name {
        check_role_name("OrganizationRole", n)?;
    }
    if let Some(p) = &req.permissions {
        check_permissions("OrganizationRole", p, ORG_PERMISSIONS)?;
    }
    let base = match &req.base_role {
        Some(b) => check_org_base(b.as_deref())?,
        None => cur.base_role.clone(),
    };
    let r: OrgRoleRow = sqlx::query_as(
        "UPDATE org_roles SET name = COALESCE($2, name), description = CASE WHEN $3 THEN $4 ELSE description END,
            base_role = $5, permissions = COALESCE($6, permissions), updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(req.name.as_deref().map(str::trim))
    .bind(req.description.is_some())
    .bind(req.description.clone().flatten())
    .bind(&base)
    .bind(&req.permissions)
    .fetch_one(&s.db)
    .await
    .map_err(unique_violation("OrganizationRole"))?;
    audit_quiet(&s.db, &o, Some(&u), "organization_role.update", json!({ "name": r.name })).await;
    Ok(Json(org_role_json(&s, &o, &r)))
}

async fn delete_org_role(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let n = sqlx::query("DELETE FROM org_roles WHERE id = $1 AND org_id = $2").bind(id).bind(o.id).execute(&s.db).await?;
    if n.rows_affected() > 0 {
        audit_quiet(&s.db, &o, Some(&u), "organization_role.destroy", json!({ "role_id": id })).await;
    }
    Ok(no_content())
}

// Assignments ---------------------------------------------------------------------------------

/// Assigning and revoking organization roles is reserved to org owners (and holders of
/// `write_organization_custom_org_role`).
pub async fn assign_role_to_user(s: &AppState, o: &User, actor: &User, user: &User, role_id: i64) -> ApiResult<()> {
    require_capability(s, o, actor, WRITE_ORG_ROLES).await?;
    let r = load_org_role(s, o, role_id).await?;
    if org_role(&s.db, o.id, user.id).await?.is_none() {
        return Err(ApiError::unprocessable("The user must be a member of the organization."));
    }
    sqlx::query("INSERT INTO org_role_users (org_id, role_id, user_id) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(o.id)
        .bind(r.id)
        .bind(user.id)
        .execute(&s.db)
        .await?;
    audit_quiet(&s.db, o, Some(actor), "org.assign_role", json!({ "user": user.login, "role": r.name })).await;
    Ok(())
}

pub async fn assign_role_to_team(s: &AppState, o: &User, actor: &User, t: &TeamRow, role_id: i64) -> ApiResult<()> {
    require_capability(s, o, actor, WRITE_ORG_ROLES).await?;
    let r = load_org_role(s, o, role_id).await?;
    sqlx::query("INSERT INTO org_role_teams (org_id, role_id, team_id) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(o.id)
        .bind(r.id)
        .bind(t.id)
        .execute(&s.db)
        .await?;
    audit_quiet(&s.db, o, Some(actor), "org.assign_role", json!({ "team": format!("{}/{}", o.login, t.slug), "role": r.name })).await;
    Ok(())
}

async fn assign_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    assign_role_to_user(&s, &o, &u, &target, id).await?;
    Ok(no_content())
}

async fn revoke_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    sqlx::query("DELETE FROM org_role_users WHERE org_id = $1 AND role_id = $2 AND user_id = $3").bind(o.id).bind(id).bind(target.id).execute(&s.db).await?;
    audit_quiet(&s.db, &o, Some(&u), "org.unassign_role", json!({ "user": target.login, "role_id": id })).await;
    Ok(no_content())
}

async fn revoke_all_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    sqlx::query("DELETE FROM org_role_users WHERE org_id = $1 AND user_id = $2").bind(o.id).bind(target.id).execute(&s.db).await?;
    audit_quiet(&s.db, &o, Some(&u), "org.unassign_role", json!({ "user": target.login })).await;
    Ok(no_content())
}

async fn assign_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let t = load_team(&s, &o, &slug).await?;
    assign_role_to_team(&s, &o, &u, &t, id).await?;
    Ok(no_content())
}

async fn revoke_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let t = load_team(&s, &o, &slug).await?;
    sqlx::query("DELETE FROM org_role_teams WHERE org_id = $1 AND role_id = $2 AND team_id = $3").bind(o.id).bind(id).bind(t.id).execute(&s.db).await?;
    audit_quiet(&s.db, &o, Some(&u), "org.unassign_role", json!({ "team": format!("{}/{}", o.login, t.slug), "role_id": id })).await;
    Ok(no_content())
}

async fn revoke_all_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let t = load_team(&s, &o, &slug).await?;
    sqlx::query("DELETE FROM org_role_teams WHERE org_id = $1 AND team_id = $2").bind(o.id).bind(t.id).execute(&s.db).await?;
    audit_quiet(&s.db, &o, Some(&u), "org.unassign_role", json!({ "team": format!("{}/{}", o.login, t.slug) })).await;
    Ok(no_content())
}

async fn role_teams(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_ORG_ROLES).await?;
    let r = load_org_role(&s, &o, id).await?;
    let rows: Vec<TeamRow> = sqlx::query_as(
        "SELECT t.* FROM org_role_teams x JOIN teams t ON t.id = x.team_id WHERE x.org_id = $1 AND x.role_id = $2 ORDER BY t.id LIMIT $3 OFFSET $4",
    )
    .bind(o.id)
    .bind(r.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let mut items = teams_json(&s, &o, &rows).await?;
    for v in &mut items {
        v["assignment"] = json!("direct");
    }
    Ok(Paged::new(&uri, &s, p, None, items))
}

/// Users holding the role directly and/or through a team.
async fn role_users(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_ORG_ROLES).await?;
    let r = load_org_role(&s, &o, id).await?;
    let rows: Vec<(i64, bool, Vec<i64>)> = sqlx::query_as(
        "WITH RECURSIVE rt(team_id) AS (SELECT team_id FROM org_role_teams WHERE org_id = $1 AND role_id = $2
                                         UNION SELECT t.id FROM teams t JOIN rt ON t.parent_id = rt.team_id),
              via AS (SELECT m.user_id, m.team_id FROM team_members m WHERE m.team_id IN (SELECT team_id FROM rt)),
              ids AS (SELECT user_id FROM org_role_users WHERE org_id = $1 AND role_id = $2 UNION SELECT user_id FROM via)
         SELECT ids.user_id,
                EXISTS(SELECT 1 FROM org_role_users d WHERE d.org_id = $1 AND d.role_id = $2 AND d.user_id = ids.user_id),
                ARRAY(SELECT DISTINCT team_id FROM via WHERE via.user_id = ids.user_id)
           FROM ids ORDER BY ids.user_id LIMIT $3 OFFSET $4",
    )
    .bind(o.id)
    .bind(r.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let uids: Vec<i64> = rows.iter().map(|x| x.0).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &uids).await?;
    let tids: Vec<i64> = rows.iter().flat_map(|x| x.2.clone()).collect();
    let teams: Vec<TeamRow> = sqlx::query_as("SELECT * FROM teams WHERE id = ANY($1)").bind(&tids).fetch_all(&s.db).await?;
    let mut items = Vec::new();
    for (uid, direct, via) in &rows {
        let Some(user) = users.get(uid) else { continue };
        let mut v = crate::json::user::simple(&s.api_url(), s.base_url(), user);
        v["assignment"] = json!(match (*direct, via.is_empty()) {
            (true, true) => "direct",
            (false, _) => "indirect",
            (true, false) => "mixed",
        });
        v["inherited_from"] = json!(via.iter().filter_map(|t| teams.iter().find(|x| x.id == *t)).map(|t| team_simple(&s, &o, t)).collect::<Vec<_>>());
        items.push(v);
    }
    Ok(Paged::new(&uri, &s, p, None, items))
}

// ---------------------------------------------------------------------------------------------
// Security managers (teams holding the predefined `security_manager` organization role)

async fn security_managers(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ_ORG_ROLES).await?;
    let rows: Vec<TeamRow> = sqlx::query_as(
        "SELECT t.* FROM org_role_teams x JOIN teams t ON t.id = x.team_id WHERE x.org_id = $1 AND x.role_id = $2 ORDER BY t.id",
    )
    .bind(o.id)
    .bind(SECURITY_MANAGER_ROLE)
    .fetch_all(&s.db)
    .await?;
    Ok(Json(json!(rows.iter().map(|t| team_simple(&s, &o, t)).collect::<Vec<_>>())))
}

async fn add_security_manager(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let t = load_team(&s, &o, &slug).await?;
    assign_role_to_team(&s, &o, &u, &t, SECURITY_MANAGER_ROLE).await?;
    Ok(no_content())
}

async fn remove_security_manager(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_ORG_ROLES).await?;
    let t = load_team(&s, &o, &slug).await?;
    sqlx::query("DELETE FROM org_role_teams WHERE org_id = $1 AND role_id = $2 AND team_id = $3")
        .bind(o.id)
        .bind(SECURITY_MANAGER_ROLE)
        .bind(t.id)
        .execute(&s.db)
        .await?;
    audit_quiet(&s.db, &o, Some(&u), "org.remove_security_manager", json!({ "team": format!("{}/{}", o.login, t.slug) })).await;
    Ok(no_content())
}
