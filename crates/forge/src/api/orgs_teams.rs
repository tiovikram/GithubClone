//! Teams: CRUD (by slug and the legacy `/teams/{team_id}` routes), nested teams, team
//! membership with maintainer/member roles, team repository access, child teams, linked
//! external IdP groups, and `/user/teams`.
//!
//! Visibility follows GitHub: closed teams are visible to every org member, secret teams only to
//! their members and org owners; non-members get 404. Teams are managed by org owners and team
//! maintainers. Child teams inherit their ancestors' repository access (see `orgs::perm`).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::orgs::{self, audit_quiet, org_by_login, org_role};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let t = "/orgs/{org}/teams/{slug}";
    let l = "/teams/{team_id}";
    Router::new()
        .route("/orgs/{org}/teams", get(list_teams).post(create_team))
        .route(t, get(get_team).patch(update_team).delete(delete_team))
        .route(&format!("{t}/members"), get(team_members))
        .route(&format!("{t}/memberships/{{username}}"), get(get_membership).put(put_membership).delete(delete_membership))
        .route(&format!("{t}/repos"), get(team_repos))
        .route(&format!("{t}/repos/{{owner}}/{{repo}}"), get(check_repo).put(put_repo).delete(delete_repo))
        .route(&format!("{t}/teams"), get(child_teams))
        .route(&format!("{t}/external-groups"), get(team_groups).patch(link_group).delete(unlink_group))
        .route("/orgs/{org}/external-groups", get(list_groups))
        .route("/orgs/{org}/external-group/{group_id}", get(get_group))
        .route(l, get(get_team_l).patch(update_team_l).delete(delete_team_l))
        .route(&format!("{l}/members"), get(team_members_l))
        .route(&format!("{l}/members/{{username}}"), get(get_member_l).put(put_member_l).delete(delete_member_l))
        .route(&format!("{l}/memberships/{{username}}"), get(get_membership_l).put(put_membership_l).delete(delete_membership_l))
        .route(&format!("{l}/repos"), get(team_repos_l))
        .route(&format!("{l}/repos/{{owner}}/{{repo}}"), get(check_repo_l).put(put_repo_l).delete(delete_repo_l))
        .route(&format!("{l}/teams"), get(child_teams_l))
        .route("/user/teams", get(my_teams))
}

// ---------------------------------------------------------------------------------------------
// Rows and JSON

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TeamRow {
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub privacy: String,
    pub permission: String,
    pub parent_id: Option<i64>,
    pub notification_setting: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// `team-simple`.
pub fn team_simple(s: &AppState, org: &User, t: &TeamRow) -> Value {
    let api = s.api_url();
    let url = format!("{api}/organizations/{}/team/{}", org.id, t.id);
    json!({
        "id": t.id,
        "node_id": crate::ids::encode("Team", t.id),
        "url": url,
        "members_url": format!("{url}/members{{/member}}"),
        "name": t.name,
        "description": t.description,
        "permission": t.permission,
        "privacy": t.privacy,
        "notification_setting": t.notification_setting,
        "html_url": format!("{}/orgs/{}/teams/{}", s.base_url(), org.login, t.slug),
        "repositories_url": format!("{url}/repos"),
        "slug": t.slug,
        "type": "organization",
        "organization_id": org.id,
    })
}

fn perms_json(p: Perm) -> Value {
    json!({
        "pull": p >= Perm::Read,
        "triage": p >= Perm::Triage,
        "push": p >= Perm::Write,
        "maintain": p >= Perm::Maintain,
        "admin": p >= Perm::Admin,
    })
}

/// `team`: simple plus `permissions` and `parent`.
pub fn team_json(s: &AppState, org: &User, t: &TeamRow, parent: Option<&TeamRow>) -> Value {
    let mut v = team_simple(s, org, t);
    v["permissions"] = perms_json(Perm::from_str(&t.permission).max(Perm::Read));
    v["parent"] = parent.map(|pt| team_simple(s, org, pt)).unwrap_or(Value::Null);
    v
}

pub async fn teams_json(s: &AppState, org: &User, rows: &[TeamRow]) -> ApiResult<Vec<Value>> {
    let pids: Vec<i64> = rows.iter().filter_map(|t| t.parent_id).collect();
    // Only same-org parents: never expose another organization's (possibly secret) team.
    let parents: Vec<TeamRow> =
        sqlx::query_as("SELECT * FROM teams WHERE id = ANY($1) AND org_id = $2").bind(&pids).bind(org.id).fetch_all(&s.db).await?;
    Ok(rows
        .iter()
        .map(|t| team_json(s, org, t, t.parent_id.and_then(|p| parents.iter().find(|x| x.id == p))))
        .collect())
}

/// `team-full`: team plus counts and the organization.
pub async fn team_full_json(s: &AppState, org: &User, t: &TeamRow) -> ApiResult<Value> {
    let mut v = teams_json(s, org, std::slice::from_ref(t)).await?.remove(0);
    let (members, repos): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM team_members WHERE team_id = $1), (SELECT count(*) FROM team_repos WHERE team_id = $1)",
    )
    .bind(t.id)
    .fetch_one(&s.db)
    .await?;
    v["created_at"] = json!(crate::json::ts(t.created_at));
    v["updated_at"] = json!(crate::json::ts(t.updated_at));
    v["members_count"] = json!(members);
    v["repos_count"] = json!(repos);
    v["organization"] = super::users::org_json(s, org).await?;
    Ok(v)
}

fn team_membership_json(s: &AppState, org: &User, t: &TeamRow, user: &User, role: &str) -> Value {
    json!({
        "url": format!("{}/organizations/{}/team/{}/memberships/{}", s.api_url(), org.id, t.id, user.login),
        "role": role,
        "state": "active",
    })
}

// ---------------------------------------------------------------------------------------------
// Lookup and access

pub async fn load_team(s: &AppState, org: &User, slug: &str) -> ApiResult<TeamRow> {
    sqlx::query_as("SELECT * FROM teams WHERE org_id = $1 AND slug = $2")
        .bind(org.id)
        .bind(slug.to_lowercase())
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn team_by_id(s: &AppState, id: i64) -> ApiResult<(User, TeamRow)> {
    let t: TeamRow = sqlx::query_as("SELECT * FROM teams WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
    let org = db::user_by_id(&s.db, t.org_id).await?;
    Ok((org, t))
}

/// Direct role of `user_id` in the team (`maintainer`/`member`).
pub async fn direct_role(s: &AppState, team_id: i64, user_id: i64) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2")
        .bind(team_id)
        .bind(user_id)
        .fetch_optional(&s.db)
        .await?)
}

/// Ids of the team and all its descendants.
async fn subtree(s: &AppState, team_id: i64) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE d(id) AS (SELECT $1::bigint UNION SELECT t.id FROM teams t JOIN d ON t.parent_id = d.id) SELECT id FROM d",
    )
    .bind(team_id)
    .fetch_all(&s.db)
    .await?)
}

/// Whether `u` can see `t`: owners, team members (also of child teams) and, for closed teams,
/// every org member.
async fn can_see(s: &AppState, org: &User, t: &TeamRow, u: &User) -> ApiResult<bool> {
    if orgs::is_owner(&s.db, org, u).await? {
        return Ok(true);
    }
    let Some(_) = org_role(&s.db, org.id, u.id).await? else { return Ok(false) };
    if t.privacy == "closed" {
        return Ok(true);
    }
    let ids = subtree(s, t.id).await?;
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM team_members WHERE team_id = ANY($1) AND user_id = $2)")
        .bind(&ids)
        .bind(u.id)
        .fetch_one(&s.db)
        .await?)
}

async fn require_see(s: &AppState, org: &User, t: &TeamRow, u: &User) -> ApiResult<()> {
    if can_see(s, org, t, u).await? { Ok(()) } else { Err(ApiError::not_found()) }
}

/// Org owners and team maintainers manage a team (403 for other viewers, 404 if invisible).
async fn require_manage(s: &AppState, org: &User, t: &TeamRow, u: &User) -> ApiResult<()> {
    if orgs::is_owner(&s.db, org, u).await? || direct_role(s, t.id, u.id).await?.as_deref() == Some("maintainer") {
        return Ok(());
    }
    require_see(s, org, t, u).await?;
    Err(ApiError::forbidden("You must be an organization owner or team maintainer to perform this action."))
}

async fn resolve_slug(s: &AppState, u: &User, org: &str, slug: &str) -> ApiResult<(User, TeamRow)> {
    let o = org_by_login(s, org).await?;
    let t = load_team(s, &o, slug).await?;
    require_see(s, &o, &t, u).await?;
    Ok((o, t))
}

async fn resolve_id(s: &AppState, u: &User, id: i64) -> ApiResult<(User, TeamRow)> {
    let (o, t) = team_by_id(s, id).await?;
    require_see(s, &o, &t, u).await?;
    Ok((o, t))
}

async fn team_event(s: &AppState, org: &User, t: &TeamRow, actor: &User, action: &str, extra: Value) {
    let mut p = json!({ "action": action, "team": team_json(s, org, t, None), "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor) });
    if let (Some(o), Value::Object(e)) = (p.as_object_mut(), extra) {
        o.extend(e);
    }
    if let Err(e) = crate::webhooks::enqueue_org_event(s, org, "team", p).await {
        tracing::warn!(error = %e, "team webhook");
    }
}

// ---------------------------------------------------------------------------------------------
// Service layer

#[derive(Deserialize, Default)]
pub struct TeamInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub privacy: Option<String>,
    pub notification_setting: Option<String>,
    pub permission: Option<String>,
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    pub parent_team_id: Option<Option<i64>>,
    pub parent_team_slug: Option<String>,
    pub maintainers: Option<Vec<String>>,
    pub repo_names: Option<Vec<String>>,
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn check_enum(field: &str, v: Option<&str>, allowed: &[&str]) -> ApiResult<()> {
    match v {
        Some(x) if !allowed.contains(&x) => Err(ApiError::validation("Team", field, "invalid")),
        _ => Ok(()),
    }
}

/// Resolves a parent team (same org only) and checks nesting rules: no cycles, no secret
/// parents or children.
async fn resolve_parent(s: &AppState, org: &User, team: Option<&TeamRow>, req: &TeamInput) -> ApiResult<Option<Option<TeamRow>>> {
    let parent = match (&req.parent_team_id, &req.parent_team_slug) {
        (Some(None), _) => return Ok(Some(None)),
        (Some(Some(id)), _) => sqlx::query_as::<_, TeamRow>("SELECT * FROM teams WHERE id = $1 AND org_id = $2")
            .bind(id)
            .bind(org.id)
            .fetch_optional(&s.db)
            .await?
            .ok_or_else(|| ApiError::validation("Team", "parent_team_id", "invalid"))?,
        (None, Some(slug)) => load_team(s, org, slug).await.map_err(|_| ApiError::validation("Team", "parent_team_slug", "invalid"))?,
        (None, None) => return Ok(None),
    };
    if parent.privacy == "secret" {
        return Err(ApiError::unprocessable("A parent team cannot be secret."));
    }
    if let Some(t) = team {
        if subtree(s, t.id).await?.contains(&parent.id) {
            return Err(ApiError::unprocessable("A team cannot be its own ancestor."));
        }
    }
    Ok(Some(Some(parent)))
}

pub async fn create_team_svc(s: &AppState, org: &User, actor: &User, req: TeamInput) -> ApiResult<TeamRow> {
    let owner = orgs::is_owner(&s.db, org, actor).await?;
    if !owner {
        if org_role(&s.db, org.id, actor.id).await?.is_none() {
            return Err(ApiError::not_found());
        }
        if !orgs::settings(&s.db, org.id).await?.members_can_create_teams {
            return Err(ApiError::forbidden("Members of this organization cannot create teams."));
        }
    }
    let name = req.name.clone().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let name = name.ok_or_else(|| ApiError::validation("Team", "name", "missing_field"))?;
    let slug = slugify(&name);
    if slug.is_empty() {
        return Err(ApiError::validation("Team", "name", "invalid"));
    }
    check_enum("privacy", req.privacy.as_deref(), &["secret", "closed"])?;
    check_enum("notification_setting", req.notification_setting.as_deref(), &["notifications_enabled", "notifications_disabled"])?;
    check_enum("permission", req.permission.as_deref(), &["pull", "push"])?;
    let parent = resolve_parent(s, org, None, &req).await?.flatten();
    let privacy = match (req.privacy.as_deref(), &parent) {
        (Some("secret"), Some(_)) => return Err(ApiError::unprocessable("A team with a parent cannot be secret.")),
        (Some(p), _) => p.to_string(),
        (None, Some(_)) => "closed".into(),
        (None, None) => "secret".into(),
    };
    let t: Option<TeamRow> = sqlx::query_as(
        "INSERT INTO teams (org_id, name, slug, description, privacy, permission, parent_id, notification_setting)
         SELECT $1,$2,$3,$4,$5,$6,$7,$8 WHERE NOT EXISTS(SELECT 1 FROM teams WHERE org_id = $1 AND lower(name) = lower($2))
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(org.id)
    .bind(&name)
    .bind(&slug)
    .bind(&req.description)
    .bind(&privacy)
    .bind(req.permission.as_deref().unwrap_or("pull"))
    .bind(parent.as_ref().map(|p| p.id))
    .bind(req.notification_setting.as_deref().unwrap_or("notifications_enabled"))
    .fetch_optional(&s.db)
    .await?;
    let t = t.ok_or_else(|| ApiError::validation("Team", "name", "already_exists"))?;
    let mut maint = req.maintainers.unwrap_or_default();
    if maint.is_empty() || !owner {
        maint.push(actor.login.clone());
    }
    for m in maint {
        if let Ok(mu) = db::user_by_login(&s.db, &m).await {
            if org_role(&s.db, org.id, mu.id).await?.is_none() && mu.id != actor.id {
                continue;
            }
            sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1,$2,'maintainer') ON CONFLICT DO NOTHING")
                .bind(t.id)
                .bind(mu.id)
                .execute(&s.db)
                .await?;
        }
    }
    for full in req.repo_names.unwrap_or_default() {
        let name = full.rsplit('/').next().unwrap_or(&full).to_string();
        if let Ok(ctx) = db::load_repo(&s.db, Some(actor), &org.login, &name).await {
            sqlx::query("INSERT INTO team_repos (team_id, repo_id, permission) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(t.id)
                .bind(ctx.repo.id)
                .bind(&t.permission)
                .execute(&s.db)
                .await?;
        }
    }
    audit_quiet(&s.db, org, Some(actor), "team.create", json!({ "team": format!("{}/{}", org.login, t.slug) })).await;
    team_event(s, org, &t, actor, "created", json!({})).await;
    Ok(t)
}

pub async fn update_team_svc(s: &AppState, org: &User, t: &TeamRow, actor: &User, req: TeamInput) -> ApiResult<TeamRow> {
    require_manage(s, org, t, actor).await?;
    check_enum("privacy", req.privacy.as_deref(), &["secret", "closed"])?;
    check_enum("notification_setting", req.notification_setting.as_deref(), &["notifications_enabled", "notifications_disabled"])?;
    check_enum("permission", req.permission.as_deref(), &["pull", "push", "admin"])?;
    if req.name.as_deref().is_some_and(|n| slugify(n).is_empty()) {
        return Err(ApiError::validation("Team", "name", "invalid"));
    }
    let parent = resolve_parent(s, org, Some(t), &req).await?;
    let new_parent = match &parent {
        Some(p) => p.as_ref().map(|p| p.id),
        None => t.parent_id,
    };
    let privacy = req.privacy.clone().unwrap_or_else(|| t.privacy.clone());
    if privacy == "secret" && new_parent.is_some() {
        return Err(ApiError::unprocessable("A team with a parent cannot be secret."));
    }
    let has_children: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM teams WHERE parent_id = $1)").bind(t.id).fetch_one(&s.db).await?;
    if privacy == "secret" && has_children {
        return Err(ApiError::unprocessable("A team with child teams cannot be secret."));
    }
    let new_slug = req.name.as_deref().map(slugify);
    let row: TeamRow = sqlx::query_as(
        "UPDATE teams SET name = COALESCE($2, name), slug = COALESCE($3, slug), description = COALESCE($4, description),
            privacy = $5, permission = COALESCE($6, permission), parent_id = $7,
            notification_setting = COALESCE($8, notification_setting), updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(t.id)
    .bind(req.name.as_deref().map(str::trim))
    .bind(new_slug)
    .bind(&req.description)
    .bind(&privacy)
    .bind(&req.permission)
    .bind(new_parent)
    .bind(&req.notification_setting)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Team", "name", "already_exists"),
        e => e.into(),
    })?;
    audit_quiet(&s.db, org, Some(actor), "team.update", json!({ "team": format!("{}/{}", org.login, row.slug) })).await;
    team_event(s, org, &row, actor, "edited", json!({})).await;
    Ok(row)
}

/// Deletes a team and all of its child teams.
pub async fn delete_team_svc(s: &AppState, org: &User, t: &TeamRow, actor: &User) -> ApiResult<()> {
    require_manage(s, org, t, actor).await?;
    let ids = subtree(s, t.id).await?;
    sqlx::query("DELETE FROM teams WHERE id = ANY($1)").bind(&ids).execute(&s.db).await?;
    audit_quiet(&s.db, org, Some(actor), "team.destroy", json!({ "team": format!("{}/{}", org.login, t.slug) })).await;
    team_event(s, org, t, actor, "deleted", json!({})).await;
    Ok(())
}

/// Adds or updates a team membership (org members only unless `add_to_org`).
pub async fn set_team_membership(s: &AppState, org: &User, t: &TeamRow, actor: &User, target: &User, role: &str, add_to_org: bool) -> ApiResult<()> {
    require_manage(s, org, t, actor).await?;
    if role != "member" && role != "maintainer" {
        return Err(ApiError::validation("TeamMember", "role", "invalid"));
    }
    if target.kind != "User" {
        return Err(ApiError::validation("TeamMember", "user", "invalid"));
    }
    if org_role(&s.db, org.id, target.id).await?.is_none() {
        if !add_to_org {
            return Err(ApiError::unprocessable("User is not a member of the organization."));
        }
        // GHES adds the user to the organization directly (no invitation round-trip).
        super::orgs_extra::set_org_membership(s, org, actor, target, "member").await?;
    }
    let existed = direct_role(s, t.id, target.id).await?.is_some();
    sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1,$2,$3) ON CONFLICT (team_id, user_id) DO UPDATE SET role = $3")
        .bind(t.id)
        .bind(target.id)
        .bind(role)
        .execute(&s.db)
        .await?;
    if !existed {
        audit_quiet(&s.db, org, Some(actor), "team.add_member", json!({ "team": format!("{}/{}", org.login, t.slug), "user": target.login })).await;
        let api = s.api_url();
        let p = json!({ "action": "added", "scope": "team", "member": crate::json::user::simple(&api, s.base_url(), target),
            "team": team_json(s, org, t, None), "sender": crate::json::user::simple(&api, s.base_url(), actor) });
        let _ = crate::webhooks::enqueue_org_event(s, org, "membership", p).await;
    }
    Ok(())
}

pub async fn remove_team_membership(s: &AppState, org: &User, t: &TeamRow, actor: &User, target: &User) -> ApiResult<()> {
    require_manage(s, org, t, actor).await?;
    let n = sqlx::query("DELETE FROM team_members WHERE team_id = $1 AND user_id = $2").bind(t.id).bind(target.id).execute(&s.db).await?;
    if n.rows_affected() > 0 {
        audit_quiet(&s.db, org, Some(actor), "team.remove_member", json!({ "team": format!("{}/{}", org.login, t.slug), "user": target.login })).await;
        let api = s.api_url();
        let p = json!({ "action": "removed", "scope": "team", "member": crate::json::user::simple(&api, s.base_url(), target),
            "team": team_json(s, org, t, None), "sender": crate::json::user::simple(&api, s.base_url(), actor) });
        let _ = crate::webhooks::enqueue_org_event(s, org, "membership", p).await;
    }
    Ok(())
}

/// Valid team/collaborator grant: a built-in level or one of the org's custom repository roles.
pub async fn normalize_grant(s: &AppState, org_id: i64, p: &str) -> ApiResult<String> {
    Ok(match p {
        "pull" | "read" => "pull".into(),
        "triage" => "triage".into(),
        "push" | "write" => "push".into(),
        "maintain" => "maintain".into(),
        "admin" => "admin".into(),
        other => {
            let name: Option<String> = sqlx::query_scalar("SELECT name FROM custom_repo_roles WHERE org_id = $1 AND lower(name) = lower($2)")
                .bind(org_id)
                .bind(other)
                .fetch_optional(&s.db)
                .await?;
            name.ok_or_else(|| ApiError::validation("Team", "permission", "invalid"))?
        }
    })
}

/// The repository (owned by the team's org) and the actor's admin right on it.
async fn team_target_repo(s: &AppState, org: &User, actor: &User, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(actor), owner, repo).await?;
    if ctx.owner.id != org.id {
        return Err(ApiError::unprocessable("The repository must be owned by the team's organization."));
    }
    Ok(ctx)
}

pub async fn set_team_repo(s: &AppState, org: &User, t: &TeamRow, actor: &User, ctx: &RepoCtx, permission: Option<&str>) -> ApiResult<()> {
    if ctx.perm < Perm::Admin {
        return Err(ApiError::forbidden("You must have admin access to the repository to change team access."));
    }
    let grant = normalize_grant(s, org.id, permission.unwrap_or(&t.permission)).await?;
    let existed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM team_repos WHERE team_id = $1 AND repo_id = $2)")
        .bind(t.id)
        .bind(ctx.repo.id)
        .fetch_one(&s.db)
        .await?;
    sqlx::query("INSERT INTO team_repos (team_id, repo_id, permission) VALUES ($1,$2,$3) ON CONFLICT (team_id, repo_id) DO UPDATE SET permission = $3")
        .bind(t.id)
        .bind(ctx.repo.id)
        .bind(&grant)
        .execute(&s.db)
        .await?;
    let team = format!("{}/{}", org.login, t.slug);
    audit_quiet(&s.db, org, Some(actor), if existed { "team.update_repository_permission" } else { "team.add_repository" },
        json!({ "team": team, "repo": ctx.full_name(), "permission": grant })).await;
    if !existed {
        team_event(s, org, t, actor, "added_to_repository", json!({ "repository": crate::webhooks::repo_payload(s, ctx) })).await;
    }
    Ok(())
}

pub async fn remove_team_repo(s: &AppState, org: &User, t: &TeamRow, actor: &User, ctx: &RepoCtx) -> ApiResult<()> {
    if ctx.perm < Perm::Admin {
        return Err(ApiError::forbidden("You must have admin access to the repository to change team access."));
    }
    let n = sqlx::query("DELETE FROM team_repos WHERE team_id = $1 AND repo_id = $2").bind(t.id).bind(ctx.repo.id).execute(&s.db).await?;
    if n.rows_affected() > 0 {
        audit_quiet(&s.db, org, Some(actor), "team.remove_repository", json!({ "team": format!("{}/{}", org.login, t.slug), "repo": ctx.full_name() })).await;
        team_event(s, org, t, actor, "removed_from_repository", json!({ "repository": crate::webhooks::repo_payload(s, ctx) })).await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Shared handler bodies (slug and legacy routes)

async fn list_teams(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    if !orgs::is_member(&s.db, &o, &u).await? {
        return Err(ApiError::forbidden("You must be a member of the organization to list teams."));
    }
    let owner = orgs::is_owner(&s.db, &o, &u).await?;
    // secret teams only for owners and members of the team (or of one of its descendants)
    let filter = "org_id = $1 AND ($2 OR privacy = 'closed' OR id IN (
        WITH RECURSIVE a(id) AS (SELECT team_id FROM team_members WHERE user_id = $3
                                 UNION SELECT t.parent_id FROM teams t JOIN a ON t.id = a.id WHERE t.parent_id IS NOT NULL)
        SELECT id FROM a))";
    let rows: Vec<TeamRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM teams WHERE {filter} ORDER BY lower(name) LIMIT $4 OFFSET $5")))
        .bind(o.id)
        .bind(owner)
        .bind(u.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM teams WHERE {filter}")))
        .bind(o.id)
        .bind(owner)
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    let items = teams_json(&s, &o, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(req): Json<TeamInput>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let t = create_team_svc(&s, &o, &u, req).await?;
    Ok(created(team_full_json(&s, &o, &t).await?))
}

async fn get_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    Ok(Json(team_full_json(&s, &o, &t).await?))
}
async fn get_team_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    Ok(Json(team_full_json(&s, &o, &t).await?))
}

async fn update_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>, Json(req): Json<TeamInput>) -> ApiResult<Json<Value>> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    let t = update_team_svc(&s, &o, &t, &u, req).await?;
    Ok(Json(team_full_json(&s, &o, &t).await?))
}
async fn update_team_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>, Json(req): Json<TeamInput>) -> ApiResult<Json<Value>> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    let t = update_team_svc(&s, &o, &t, &u, req).await?;
    Ok(Json(team_full_json(&s, &o, &t).await?))
}

async fn delete_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Response> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    delete_team_svc(&s, &o, &t, &u).await?;
    Ok(no_content())
}
async fn delete_team_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    delete_team_svc(&s, &o, &t, &u).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
struct MembersQuery {
    role: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// Members of the team and of its child teams (inherited members have role `member`).
async fn members_body(s: &AppState, uri: &OriginalUri, t: &TeamRow, q: MembersQuery) -> ApiResult<Paged> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let role = q.role.as_deref().unwrap_or("all");
    if !["all", "member", "maintainer"].contains(&role) {
        return Err(ApiError::validation("TeamMember", "role", "invalid"));
    }
    let ids = subtree(s, t.id).await?;
    let rows: Vec<(i64, String, bool)> = sqlx::query_as(
        "SELECT user_id, role, inherited FROM (
            SELECT DISTINCT ON (m.user_id) m.user_id,
                   CASE WHEN m.team_id = $1 THEN m.role ELSE 'member' END AS role, m.team_id <> $1 AS inherited
              FROM team_members m WHERE m.team_id = ANY($2) ORDER BY m.user_id, (m.team_id <> $1)) x
          WHERE $3 = 'all' OR role = $3 ORDER BY user_id LIMIT $4 OFFSET $5",
    )
    .bind(t.id)
    .bind(&ids)
    .bind(role)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let uids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &uids).await?;
    let items = rows
        .iter()
        .filter_map(|(id, _, _)| users.get(id).map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)))
        .collect();
    Ok(Paged::new(uri, s, p, None, items))
}

async fn team_members(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>, Query(q): Query<MembersQuery>) -> ApiResult<Paged> {
    let (_, t) = resolve_slug(&s, &u, &org, &slug).await?;
    members_body(&s, &uri, &t, q).await
}
async fn team_members_l(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(id): Path<i64>, Query(q): Query<MembersQuery>) -> ApiResult<Paged> {
    let (_, t) = resolve_id(&s, &u, id).await?;
    members_body(&s, &uri, &t, q).await
}

/// Membership of a user: the direct role, or `member` when inherited from a child team.
async fn membership_body(s: &AppState, org: &User, t: &TeamRow, login: &str) -> ApiResult<Json<Value>> {
    let target = db::user_by_login(&s.db, login).await?;
    let role = match direct_role(s, t.id, target.id).await? {
        Some(r) => r,
        None => {
            let ids = subtree(s, t.id).await?;
            let inherited: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM team_members WHERE team_id = ANY($1) AND user_id = $2)")
                .bind(&ids)
                .bind(target.id)
                .fetch_one(&s.db)
                .await?;
            if !inherited {
                return Err(ApiError::not_found());
            }
            "member".into()
        }
    };
    Ok(Json(team_membership_json(s, org, t, &target, &role)))
}

async fn get_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug, login)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    membership_body(&s, &o, &t, &login).await
}
async fn get_membership_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, login)): Path<(i64, String)>) -> ApiResult<Json<Value>> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    membership_body(&s, &o, &t, &login).await
}

#[derive(Deserialize, Default)]
struct RoleBody {
    role: Option<String>,
}

async fn put_membership_body(s: &AppState, o: &User, t: &TeamRow, u: &User, login: &str, b: RoleBody) -> ApiResult<Json<Value>> {
    let target = db::user_by_login(&s.db, login).await?;
    let role = b.role.unwrap_or_else(|| "member".into());
    set_team_membership(s, o, t, u, &target, &role, true).await?;
    Ok(Json(team_membership_json(s, o, t, &target, &role)))
}

async fn put_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug, login)): Path<(String, String, String)>, Json(b): Json<RoleBody>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    let t = load_team(&s, &o, &slug).await?;
    put_membership_body(&s, &o, &t, &u, &login, b).await
}
async fn put_membership_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, login)): Path<(i64, String)>, Json(b): Json<RoleBody>) -> ApiResult<Json<Value>> {
    let (o, t) = team_by_id(&s, id).await?;
    put_membership_body(&s, &o, &t, &u, &login, b).await
}

async fn delete_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug, login)): Path<(String, String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let t = load_team(&s, &o, &slug).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    remove_team_membership(&s, &o, &t, &u, &target).await?;
    Ok(no_content())
}
async fn delete_membership_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, login)): Path<(i64, String)>) -> ApiResult<Response> {
    let (o, t) = team_by_id(&s, id).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    remove_team_membership(&s, &o, &t, &u, &target).await?;
    Ok(no_content())
}

// Legacy member endpoints (204 / 404).
async fn get_member_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, login)): Path<(i64, String)>) -> ApiResult<Response> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    membership_body(&s, &o, &t, &login).await?;
    Ok(no_content())
}
async fn put_member_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, login)): Path<(i64, String)>) -> ApiResult<Response> {
    let (o, t) = team_by_id(&s, id).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    let role = direct_role(&s, t.id, target.id).await?.unwrap_or_else(|| "member".into());
    set_team_membership(&s, &o, &t, &u, &target, &role, false).await?;
    Ok(no_content())
}
async fn delete_member_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, login)): Path<(i64, String)>) -> ApiResult<Response> {
    let (o, t) = team_by_id(&s, id).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    if direct_role(&s, t.id, target.id).await?.is_none() {
        require_manage(&s, &o, &t, &u).await?;
        return Err(ApiError::not_found());
    }
    remove_team_membership(&s, &o, &t, &u, &target).await?;
    Ok(no_content())
}

/// Repositories the team can access (directly or through an ancestor), with the team's
/// permission, limited to repositories the viewer can see.
async fn repos_body(s: &AppState, uri: &OriginalUri, org: &User, t: &TeamRow, viewer: &User, p: PageParams) -> ApiResult<Paged> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "WITH RECURSIVE anc(id) AS (SELECT $1::bigint UNION SELECT t.parent_id FROM teams t JOIN anc ON t.id = anc.id WHERE t.parent_id IS NOT NULL)
         SELECT tr.repo_id, tr.permission FROM team_repos tr JOIN repositories r ON r.id = tr.repo_id
          WHERE tr.team_id IN (SELECT id FROM anc) ORDER BY lower(r.name), tr.team_id = $1 DESC",
    )
    .bind(t.id)
    .fetch_all(&s.db)
    .await?;
    let custom = crate::orgs::perm::custom_roles(&s.db, org.id).await?;
    let mut best: Vec<(i64, Perm, String)> = Vec::new();
    for (rid, g) in rows {
        let p = crate::orgs::perm::resolve_grant(&g, &custom);
        match best.iter_mut().find(|b| b.0 == rid) {
            Some(b) if p > b.1 => {
                b.1 = p;
                b.2 = g;
            }
            Some(_) => {}
            None => best.push((rid, p, g)),
        }
    }
    let ids: Vec<i64> = best.iter().map(|b| b.0).collect();
    let viewer_perms = crate::orgs::perm::org_repo_permissions_for_repos(&s.db, org.id, &ids, viewer).await?;
    let mut items = Vec::new();
    let visible: Vec<&(i64, Perm, String)> = best.iter().filter(|b| viewer_perms.get(&b.0).is_some_and(|p| *p >= Perm::Read)).collect();
    let total = visible.len() as i64;
    for (rid, perm, grant) in visible.into_iter().skip(p.offset() as usize).take(p.limit() as usize) {
        let repo = db::repo_by_id(&s.db, *rid).await?;
        let ctx = RepoCtx { repo, owner: org.clone(), perm: *perm };
        let mut v = crate::json::repo::repo(&s.api_url(), s.base_url(), &ctx);
        v["role_name"] = json!(role_name(grant));
        items.push(v);
    }
    Ok(Paged::new(uri, s, p, Some(total), items))
}

fn role_name(grant: &str) -> String {
    match grant {
        "pull" => "read".into(),
        "push" => "write".into(),
        g => g.to_string(),
    }
}

async fn team_repos(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    repos_body(&s, &uri, &o, &t, &u, p).await
}
async fn team_repos_l(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(id): Path<i64>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    repos_body(&s, &uri, &o, &t, &u, p).await
}

/// 204 when the team has access; 200 with `team-repository` when the client asks for
/// `application/vnd.github.v3.repository+json`; 404 otherwise.
async fn check_body(s: &AppState, headers: &HeaderMap, org: &User, t: &TeamRow, u: &User, owner: &str, repo: &str) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    if ctx.owner.id != org.id {
        return Err(ApiError::not_found());
    }
    let (perm, grant) = crate::orgs::perm::team_repo_permission(&s.db, org.id, t.id, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    let wants = headers.get(axum::http::header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|a| a.contains("repository+json"));
    if !wants {
        return Ok(no_content());
    }
    let tctx = RepoCtx { perm, ..ctx };
    let mut v = crate::json::repo::full(&s.api_url(), s.base_url(), &tctx, None, None);
    v["role_name"] = json!(role_name(&grant));
    v["temp_clone_token"] = json!("");
    Ok(axum::Json(v).into_response())
}

async fn check_repo(State(s): State<AppState>, headers: HeaderMap, AuthUser(u): AuthUser, Path((org, slug, owner, repo)): Path<(String, String, String, String)>) -> ApiResult<Response> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    check_body(&s, &headers, &o, &t, &u, &owner, &repo).await
}
async fn check_repo_l(State(s): State<AppState>, headers: HeaderMap, AuthUser(u): AuthUser, Path((id, owner, repo)): Path<(i64, String, String)>) -> ApiResult<Response> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    check_body(&s, &headers, &o, &t, &u, &owner, &repo).await
}

#[derive(Deserialize, Default)]
struct PermBody {
    permission: Option<String>,
}

async fn put_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug, owner, repo)): Path<(String, String, String, String)>, Json(b): Json<PermBody>) -> ApiResult<Response> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    let ctx = team_target_repo(&s, &o, &u, &owner, &repo).await?;
    set_team_repo(&s, &o, &t, &u, &ctx, b.permission.as_deref()).await?;
    Ok(no_content())
}
async fn put_repo_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, owner, repo)): Path<(i64, String, String)>, Json(b): Json<PermBody>) -> ApiResult<Response> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    let ctx = team_target_repo(&s, &o, &u, &owner, &repo).await?;
    set_team_repo(&s, &o, &t, &u, &ctx, b.permission.as_deref()).await?;
    Ok(no_content())
}

async fn delete_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug, owner, repo)): Path<(String, String, String, String)>) -> ApiResult<Response> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    let ctx = team_target_repo(&s, &o, &u, &owner, &repo).await?;
    remove_team_repo(&s, &o, &t, &u, &ctx).await?;
    Ok(no_content())
}
async fn delete_repo_l(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, owner, repo)): Path<(i64, String, String)>) -> ApiResult<Response> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    let ctx = team_target_repo(&s, &o, &u, &owner, &repo).await?;
    remove_team_repo(&s, &o, &t, &u, &ctx).await?;
    Ok(no_content())
}

async fn children_body(s: &AppState, uri: &OriginalUri, org: &User, t: &TeamRow, p: PageParams) -> ApiResult<Paged> {
    let rows: Vec<TeamRow> = sqlx::query_as("SELECT * FROM teams WHERE parent_id = $1 AND org_id = $2 ORDER BY lower(name) LIMIT $3 OFFSET $4")
        .bind(t.id)
        .bind(org.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = teams_json(s, org, &rows).await?;
    Ok(Paged::new(uri, s, p, None, items))
}

async fn child_teams(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    children_body(&s, &uri, &o, &t, p).await
}
async fn child_teams_l(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(id): Path<i64>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (o, t) = resolve_id(&s, &u, id).await?;
    children_body(&s, &uri, &o, &t, p).await
}

async fn my_teams(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let rows: Vec<TeamRow> = sqlx::query_as(
        "SELECT t.* FROM teams t JOIN team_members m ON m.team_id = t.id WHERE m.user_id = $1 ORDER BY t.id LIMIT $2 OFFSET $3",
    )
    .bind(u.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let org_ids: Vec<i64> = rows.iter().map(|t| t.org_id).collect();
    let orgs = crate::json::issue::users_by_ids(&s.db, &org_ids).await?;
    let mut items = Vec::new();
    for t in &rows {
        if let Some(o) = orgs.get(&t.org_id) {
            items.push(team_full_json(&s, o, t).await?);
        }
    }
    Ok(Paged::new(&uri, &s, p, None, items))
}

// ---------------------------------------------------------------------------------------------
// External IdP groups. Groups are provisioned by the identity provider (SCIM) into
// `external_groups`; linking a group to a team adds the group's members to the team.

#[derive(sqlx::FromRow)]
struct GroupRow {
    id: i64,
    name: String,
    updated_at: chrono::DateTime<chrono::Utc>,
}

fn group_brief(g: &GroupRow) -> Value {
    json!({ "group_id": g.id, "group_name": g.name, "updated_at": crate::json::ts(g.updated_at) })
}

async fn group_full(s: &AppState, org: &User, g: &GroupRow) -> ApiResult<Value> {
    let teams: Vec<(i64, String)> = sqlx::query_as(
        "SELECT t.id, t.name FROM team_external_groups x JOIN teams t ON t.id = x.team_id WHERE x.group_id = $1 AND t.org_id = $2 ORDER BY t.id",
    )
    .bind(g.id)
    .bind(org.id)
    .fetch_all(&s.db)
    .await?;
    let members: Vec<User> = sqlx::query_as("SELECT u.* FROM external_group_members m JOIN users u ON u.id = m.user_id WHERE m.group_id = $1 ORDER BY u.id")
        .bind(g.id)
        .fetch_all(&s.db)
        .await?;
    Ok(json!({
        "group_id": g.id,
        "group_name": g.name,
        "updated_at": crate::json::ts(g.updated_at),
        "teams": teams.iter().map(|(id, n)| json!({ "team_id": id, "team_name": n })).collect::<Vec<_>>(),
        "members": members.iter().map(|m| json!({
            "member_id": m.id, "member_login": m.login,
            "member_name": m.name.clone().unwrap_or_else(|| m.login.clone()),
            "member_email": m.email.clone().unwrap_or_default(),
        })).collect::<Vec<_>>(),
    }))
}

async fn load_group(s: &AppState, org: &User, id: i64) -> ApiResult<Option<GroupRow>> {
    Ok(sqlx::query_as("SELECT id, name, updated_at FROM external_groups WHERE id = $1 AND (org_id IS NULL OR org_id = $2)")
        .bind(id)
        .bind(org.id)
        .fetch_optional(&s.db)
        .await?)
}

#[derive(Deserialize, Default)]
struct GroupsQuery {
    display_name: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_groups(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<GroupsQuery>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    orgs::require_member(&s, &o, &u).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let rows: Vec<GroupRow> = sqlx::query_as(
        "SELECT id, name, updated_at FROM external_groups WHERE (org_id IS NULL OR org_id = $1)
            AND ($2::text IS NULL OR name ILIKE '%' || $2 || '%') ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(o.id)
    .bind(&q.display_name)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let _ = uri;
    Ok(axum::Json(json!({ "groups": rows.iter().map(group_brief).collect::<Vec<_>>() })).into_response())
}

async fn get_group(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    orgs::require_member(&s, &o, &u).await?;
    let g = load_group(&s, &o, gid).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(group_full(&s, &o, &g).await?))
}

async fn team_groups(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let (_, t) = resolve_slug(&s, &u, &org, &slug).await?;
    let rows: Vec<GroupRow> = sqlx::query_as(
        "SELECT g.id, g.name, g.updated_at FROM team_external_groups x JOIN external_groups g ON g.id = x.group_id WHERE x.team_id = $1 ORDER BY g.id",
    )
    .bind(t.id)
    .fetch_all(&s.db)
    .await?;
    Ok(Json(json!({ "groups": rows.iter().map(group_brief).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct LinkBody {
    group_id: Option<i64>,
}

async fn link_group(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>, Json(b): Json<LinkBody>) -> ApiResult<Json<Value>> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    require_manage(&s, &o, &t, &u).await?;
    let gid = b.group_id.ok_or_else(|| ApiError::validation("ExternalGroup", "group_id", "missing_field"))?;
    let g = load_group(&s, &o, gid).await?.ok_or_else(|| ApiError::validation("ExternalGroup", "group_id", "invalid"))?;
    // a team is linked to at most one group
    sqlx::query("DELETE FROM team_external_groups WHERE team_id = $1").bind(t.id).execute(&s.db).await?;
    sqlx::query("INSERT INTO team_external_groups (team_id, group_id) VALUES ($1, $2)").bind(t.id).bind(g.id).execute(&s.db).await?;
    // membership is synced from the group: members join the org (if needed) and the team
    let members: Vec<User> = sqlx::query_as("SELECT u.* FROM external_group_members m JOIN users u ON u.id = m.user_id WHERE m.group_id = $1")
        .bind(g.id)
        .fetch_all(&s.db)
        .await?;
    for m in members {
        if direct_role(&s, t.id, m.id).await?.is_none() {
            set_team_membership(&s, &o, &t, &u, &m, "member", true).await?;
        }
    }
    audit_quiet(&s.db, &o, Some(&u), "team.link_external_group", json!({ "team": format!("{}/{}", o.login, t.slug), "group_id": g.id })).await;
    Ok(Json(group_full(&s, &o, &g).await?))
}

async fn unlink_group(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Response> {
    let (o, t) = resolve_slug(&s, &u, &org, &slug).await?;
    require_manage(&s, &o, &t, &u).await?;
    sqlx::query("DELETE FROM team_external_groups WHERE team_id = $1").bind(t.id).execute(&s.db).await?;
    audit_quiet(&s.db, &o, Some(&u), "team.unlink_external_group", json!({ "team": format!("{}/{}", o.login, t.slug) })).await;
    Ok(no_content())
}
