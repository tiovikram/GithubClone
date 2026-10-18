//! Organizations: settings, access checks, audit log and shared helpers used by the
//! `api::orgs_*` modules (and by `db::permission`, through `perm`).
pub mod perm;
pub mod sealed_box;

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Loads an organization by login (404 for users and unknown logins).
pub async fn org_by_login(s: &AppState, org: &str) -> ApiResult<User> {
    let o = db::user_by_login(&s.db, org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    Ok(o)
}

/// Role of `user_id` in `org_id` ("admin" = owner, "member"), if a member.
pub async fn org_role(db: &PgPool, org_id: i64, user_id: i64) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
        .bind(org_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?)
}

pub async fn is_owner(db: &PgPool, org: &User, u: &User) -> ApiResult<bool> {
    Ok(u.site_admin || org_role(db, org.id, u.id).await?.as_deref() == Some("admin"))
}

pub async fn is_member(db: &PgPool, org: &User, u: &User) -> ApiResult<bool> {
    Ok(u.site_admin || org_role(db, org.id, u.id).await?.is_some())
}

/// Org owners only: 404 for non-members (the org's settings are not theirs to see), 403 for
/// members.
pub async fn require_owner(s: &AppState, org: &User, u: &User) -> ApiResult<()> {
    if is_owner(&s.db, org, u).await? {
        return Ok(());
    }
    if org_role(&s.db, org.id, u.id).await?.is_some() {
        Err(ApiError::forbidden("You must be an organization owner to perform this action."))
    } else {
        Err(ApiError::not_found())
    }
}

/// Members only (404 otherwise).
pub async fn require_member(s: &AppState, org: &User, u: &User) -> ApiResult<()> {
    if is_member(&s.db, org, u).await? { Ok(()) } else { Err(ApiError::not_found()) }
}

/// Fine-grained organization permissions a user holds through organization roles (assigned
/// directly or through a team, including the teams' ancestors).
pub async fn org_permissions(db: &PgPool, org_id: i64, user_id: i64) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE mt(team_id) AS (
             SELECT tm.team_id FROM team_members tm JOIN teams t ON t.id = tm.team_id WHERE t.org_id = $1 AND tm.user_id = $2
             UNION SELECT t.parent_id FROM mt JOIN teams t ON t.id = mt.team_id WHERE t.parent_id IS NOT NULL)
         SELECT DISTINCT unnest(r.permissions) FROM org_roles r
          WHERE (r.org_id IS NULL OR r.org_id = $1)
            AND (r.id IN (SELECT role_id FROM org_role_users WHERE org_id = $1 AND user_id = $2)
              OR r.id IN (SELECT role_id FROM org_role_teams WHERE org_id = $1 AND team_id IN (SELECT team_id FROM mt)))",
    )
    .bind(org_id)
    .bind(user_id)
    .fetch_all(db)
    .await?)
}

/// Owner, or a member holding one of `perms` through an organization role. Non-members get 404,
/// members without the permission 403.
pub async fn require_capability(s: &AppState, org: &User, u: &User, perms: &[&str]) -> ApiResult<()> {
    if is_owner(&s.db, org, u).await? {
        return Ok(());
    }
    if org_role(&s.db, org.id, u.id).await?.is_none() {
        return Err(ApiError::not_found());
    }
    let have = org_permissions(&s.db, org.id, u.id).await?;
    if perms.iter().any(|p| have.iter().any(|h| h == p)) {
        Ok(())
    } else {
        Err(ApiError::forbidden("You must be an organization owner or hold the required organization role."))
    }
}

// ---------------------------------------------------------------------------------------------
// Settings

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrgSettings {
    pub org_id: i64,
    pub billing_email: Option<String>,
    pub default_repository_permission: String,
    pub has_organization_projects: bool,
    pub has_repository_projects: bool,
    pub members_can_create_repositories: bool,
    pub members_can_create_public_repositories: bool,
    pub members_can_create_private_repositories: bool,
    pub members_can_create_internal_repositories: bool,
    pub members_can_create_pages: bool,
    pub members_can_fork_private_repositories: bool,
    pub members_can_create_teams: bool,
    pub web_commit_signoff_required: bool,
    pub deploy_keys_enabled_for_repositories: bool,
    pub security_defaults: Value,
    pub announcement: Option<String>,
    pub announcement_expires_at: Option<DateTime<Utc>>,
    pub announcement_user_dismissible: bool,
    pub immutable_releases: String,
    pub updated_at: DateTime<Utc>,
}

/// The org's settings row (created with GitHub's defaults on first use).
pub async fn settings(db: &PgPool, org_id: i64) -> ApiResult<OrgSettings> {
    if let Some(s) = sqlx::query_as("SELECT * FROM org_settings WHERE org_id = $1").bind(org_id).fetch_optional(db).await? {
        return Ok(s);
    }
    sqlx::query("INSERT INTO org_settings (org_id) VALUES ($1) ON CONFLICT DO NOTHING").bind(org_id).execute(db).await?;
    Ok(sqlx::query_as("SELECT * FROM org_settings WHERE org_id = $1").bind(org_id).fetch_one(db).await?)
}

/// Whether `actor` may create a repository with the given visibility in `org` (owners always;
/// members per `members_can_create_*`).
pub async fn may_create_repo(db: &PgPool, org: &User, actor: &User, private: bool) -> ApiResult<bool> {
    if is_owner(db, org, actor).await? {
        return Ok(true);
    }
    if org_role(db, org.id, actor.id).await?.is_none() {
        return Ok(false);
    }
    let st = settings(db, org.id).await?;
    Ok(st.members_can_create_repositories
        && if private { st.members_can_create_private_repositories } else { st.members_can_create_public_repositories })
}

/// GitHub gives the member who creates (or forks into) an org repository admin on it. Owners
/// already have admin; for everyone else this is a direct collaborator grant.
pub async fn grant_creator_admin(db: &PgPool, owner: &User, repo_id: i64, creator: &User) -> ApiResult<()> {
    if owner.kind != "Organization" || owner.id == creator.id || is_owner(db, owner, creator).await? {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO repo_collaborators (repo_id, user_id, permission) VALUES ($1, $2, 'admin')
         ON CONFLICT (repo_id, user_id) DO UPDATE SET permission = 'admin'",
    )
    .bind(repo_id)
    .bind(creator.id)
    .execute(db)
    .await?;
    Ok(())
}

/// Whether releases of `repo_id` (owned by `org_id`) are immutable by org policy.
pub async fn immutable_releases_enforced(db: &PgPool, org_id: i64, repo_id: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT immutable_releases = 'all'
                            OR (immutable_releases = 'selected'
                                AND EXISTS(SELECT 1 FROM org_immutable_release_repos r WHERE r.org_id = $1 AND r.repo_id = $2))
                          FROM org_settings WHERE org_id = $1), false)",
    )
    .bind(org_id)
    .bind(repo_id)
    .fetch_one(db)
    .await?)
}

fn members_allowed_type(st: &OrgSettings) -> &'static str {
    if !st.members_can_create_repositories {
        "none"
    } else if st.members_can_create_public_repositories {
        "all"
    } else {
        "private"
    }
}

/// `organization-full`. Owners also see the private settings (billing email, plan, base
/// permission, member privileges), like on GitHub.
pub async fn org_full_json(s: &AppState, o: &User, viewer: Option<&User>) -> ApiResult<Value> {
    let mut v = crate::api::users::org_json(s, o).await?;
    let st = settings(&s.db, o.id).await?;
    let m = v.as_object_mut().expect("object");
    m.insert("has_organization_projects".into(), json!(st.has_organization_projects));
    m.insert("has_repository_projects".into(), json!(st.has_repository_projects));
    let owner = match viewer {
        Some(u) => is_owner(&s.db, o, u).await?,
        None => false,
    };
    if !owner {
        return Ok(v);
    }
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM org_members WHERE org_id = $1").bind(o.id).fetch_one(&s.db).await?;
    let collaborators: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT c.user_id) FROM repo_collaborators c JOIN repositories r ON r.id = c.repo_id
          WHERE r.owner_id = $1 AND NOT EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = $1 AND m.user_id = c.user_id)",
    )
    .bind(o.id)
    .fetch_one(&s.db)
    .await?;
    let disk: i64 = sqlx::query_scalar("SELECT COALESCE(sum(size_kb), 0)::bigint FROM repositories WHERE owner_id = $1")
        .bind(o.id)
        .fetch_one(&s.db)
        .await?;
    let sec = |k: &str| st.security_defaults.get(k).and_then(Value::as_bool).unwrap_or(false);
    let fields: Vec<(&str, Value)> = vec![
        ("private_gists", json!(0)),
        ("disk_usage", json!(disk)),
        ("collaborators", json!(collaborators)),
        ("billing_email", json!(st.billing_email.clone().or_else(|| o.email.clone()))),
        ("plan", json!({ "name": "enterprise", "space": 976562499, "private_repos": 999999, "filled_seats": members, "seats": 0 })),
        ("default_repository_permission", json!(st.default_repository_permission)),
        ("default_repository_branch", json!("main")),
        ("members_can_create_repositories", json!(st.members_can_create_repositories)),
        ("two_factor_requirement_enabled", json!(false)),
        ("members_allowed_repository_creation_type", json!(members_allowed_type(&st))),
        ("members_can_create_public_repositories", json!(st.members_can_create_public_repositories)),
        ("members_can_create_private_repositories", json!(st.members_can_create_private_repositories)),
        ("members_can_create_internal_repositories", json!(st.members_can_create_internal_repositories)),
        ("members_can_create_pages", json!(st.members_can_create_pages)),
        ("members_can_create_public_pages", json!(st.members_can_create_pages)),
        ("members_can_create_private_pages", json!(st.members_can_create_pages)),
        ("members_can_delete_repositories", json!(false)),
        ("members_can_change_repo_visibility", json!(false)),
        ("members_can_invite_outside_collaborators", json!(false)),
        ("members_can_delete_issues", json!(false)),
        ("display_commenter_full_name_setting_enabled", json!(false)),
        ("readers_can_create_discussions", json!(true)),
        ("members_can_create_teams", json!(st.members_can_create_teams)),
        ("members_can_view_dependency_insights", json!(true)),
        ("members_can_fork_private_repositories", json!(st.members_can_fork_private_repositories)),
        ("web_commit_signoff_required", json!(st.web_commit_signoff_required)),
        ("deploy_keys_enabled_for_repositories", json!(st.deploy_keys_enabled_for_repositories)),
        ("advanced_security_enabled_for_new_repositories", json!(sec("advanced_security_enabled_for_new_repositories"))),
        ("dependabot_alerts_enabled_for_new_repositories", json!(sec("dependabot_alerts_enabled_for_new_repositories"))),
        ("dependabot_security_updates_enabled_for_new_repositories", json!(sec("dependabot_security_updates_enabled_for_new_repositories"))),
        ("dependency_graph_enabled_for_new_repositories", json!(sec("dependency_graph_enabled_for_new_repositories"))),
        ("secret_scanning_enabled_for_new_repositories", json!(sec("secret_scanning_enabled_for_new_repositories"))),
        ("secret_scanning_push_protection_enabled_for_new_repositories", json!(sec("secret_scanning_push_protection_enabled_for_new_repositories"))),
        ("secret_scanning_push_protection_custom_link_enabled", json!(sec("secret_scanning_push_protection_custom_link_enabled"))),
        ("secret_scanning_push_protection_custom_link", st.security_defaults.get("secret_scanning_push_protection_custom_link").cloned().unwrap_or(Value::Null)),
    ];
    for (k, val) in fields {
        m.insert(k.into(), val);
    }
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// Audit log

/// Records an audit-log event for `org`. `data` may carry `user`, `repo` and `team` (stored in
/// their own columns for search); everything else is kept as event data.
pub async fn audit(db: &PgPool, org: &User, actor: Option<&User>, action: &str, data: Value) -> ApiResult<()> {
    let mut data = match data {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let take = |m: &mut Map<String, Value>, k: &str| m.remove(k).and_then(|v| v.as_str().map(str::to_string));
    let user = take(&mut data, "user");
    let repo = take(&mut data, "repo");
    let team = take(&mut data, "team");
    sqlx::query(
        "INSERT INTO org_audit_log (org_id, action, actor_id, actor, user_login, repo, team, data) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(org.id)
    .bind(action)
    .bind(actor.map(|a| a.id))
    .bind(actor.map(|a| a.login.clone()))
    .bind(user)
    .bind(repo)
    .bind(team)
    .bind(Value::Object(data))
    .execute(db)
    .await?;
    Ok(())
}

/// serde helper: distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
/// Use with `#[serde(default, deserialize_with = "crate::orgs::double_option")]`.
pub fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    use serde::Deserialize;
    Option::<T>::deserialize(d).map(Some)
}

/// Best-effort audit: an audit failure never fails the mutation that was already applied.
pub async fn audit_quiet(db: &PgPool, org: &User, actor: Option<&User>, action: &str, data: Value) {
    if let Err(e) = audit(db, org, actor, action, data).await {
        tracing::warn!(error = %e.message, action, "audit log write failed");
    }
}
