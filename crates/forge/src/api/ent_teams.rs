//! Enterprise teams, their members and organization assignments. Members of an enterprise team
//! assigned to an organization are members of that organization: forge adds an `org_members`
//! row (recorded in `ent_org_grants`) when needed and removes exactly those rows when the
//! assignment or membership ends. Site administrators only.
use std::collections::HashSet;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::db::User;
use crate::enterprise::{self as ent, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let t = "/enterprises/{enterprise}/teams";
    Router::new()
        .route(t, get(list).post(create))
        .route(&format!("{t}/{{team}}"), get(get_team).patch(update).delete(delete_team))
        .route(&format!("{t}/{{team}}/memberships"), get(list_members))
        .route(&format!("{t}/{{team}}/memberships/add"), post(bulk_add_members))
        .route(&format!("{t}/{{team}}/memberships/remove"), post(bulk_remove_members))
        .route(&format!("{t}/{{team}}/memberships/{{username}}"), axum::routing::put(add_member).delete(remove_member))
        .route(&format!("{t}/{{team}}/organizations"), get(list_orgs))
        .route(&format!("{t}/{{team}}/organizations/add"), post(bulk_add_orgs))
        .route(&format!("{t}/{{team}}/organizations/remove"), post(bulk_remove_orgs))
        .route(&format!("{t}/{{team}}/organizations/{{org}}"), get(get_org).put(add_org).delete(remove_org))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EntTeam {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub sync_to_organizations: String,
    pub organization_selection_type: String,
    pub group_id: Option<String>,
    pub notification_setting: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

async fn team_json(s: &AppState, t: &EntTeam) -> ApiResult<Value> {
    let slug = ent::slug();
    let url = format!("{}/enterprises/{slug}/teams/{}", s.api_url(), t.slug);
    let group_name: Option<String> = match &t.group_id {
        Some(g) => sqlx::query_scalar("SELECT display_name FROM ent_scim_groups WHERE id = $1").bind(g).fetch_optional(&s.db).await?,
        None => None,
    };
    Ok(json!({
        "id": t.id,
        "name": t.name,
        "description": t.description.clone().unwrap_or_default(),
        "slug": t.slug,
        "url": url,
        "sync_to_organizations": t.sync_to_organizations,
        "organization_selection_type": t.organization_selection_type,
        "group_id": t.group_id,
        "group_name": group_name,
        "html_url": format!("{}/enterprises/{slug}/teams/{}", s.base_url(), t.slug),
        "members_url": format!("{url}/memberships{{/member}}"),
        "created_at": crate::json::ts(t.created_at),
        "updated_at": crate::json::ts(t.updated_at),
        "notification_setting": t.notification_setting,
    }))
}

/// Enterprise team slugs are prefixed `ent:` on GitHub; forge accepts both forms.
async fn load(s: &AppState, e: &str, team: &str) -> ApiResult<EntTeam> {
    ent::check_enterprise(e)?;
    let slug = team.strip_prefix("ent:").unwrap_or(team);
    sqlx::query_as("SELECT * FROM ent_teams WHERE lower(slug) = lower($1) OR lower(slug) = lower($2)")
        .bind(team)
        .bind(slug)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn list(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Path(e): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    ent::check_enterprise(&e)?;
    let rows: Vec<EntTeam> = sqlx::query_as("SELECT * FROM ent_teams ORDER BY lower(name) LIMIT $1 OFFSET $2").bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_teams").fetch_one(&s.db).await?;
    let mut items = Vec::new();
    for t in &rows {
        items.push(team_json(&s, t).await?);
    }
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
struct TeamBody {
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    description: Option<Option<String>>,
    sync_to_organizations: Option<String>,
    organization_selection_type: Option<String>,
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    group_id: Option<Option<String>>,
    notification_setting: Option<String>,
}

fn check_body(b: &TeamBody) -> ApiResult<()> {
    let bad = |f: &str| Err(ApiError::validation("EnterpriseTeam", f, "invalid"));
    if b.sync_to_organizations.as_deref().is_some_and(|v| !matches!(v, "all" | "disabled")) {
        return bad("sync_to_organizations");
    }
    if b.organization_selection_type.as_deref().is_some_and(|v| !matches!(v, "disabled" | "selected" | "all")) {
        return bad("organization_selection_type");
    }
    if b.notification_setting.as_deref().is_some_and(|v| !matches!(v, "notifications_enabled" | "notifications_disabled")) {
        return bad("notification_setting");
    }
    Ok(())
}

async fn check_group(s: &AppState, g: &Option<Option<String>>) -> ApiResult<()> {
    if let Some(Some(id)) = g {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ent_scim_groups WHERE id = $1)").bind(id).fetch_one(&s.db).await?;
        if !ok {
            return Err(ApiError::validation("EnterpriseTeam", "group_id", "invalid"));
        }
    }
    Ok(())
}

async fn create(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<TeamBody>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    check_body(&b)?;
    check_group(&s, &b.group_id).await?;
    let name = b.name.clone().filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("EnterpriseTeam", "name", "missing_field"))?;
    let slug = super::orgs_teams::slugify(&name);
    if slug.is_empty() {
        return Err(ApiError::validation("EnterpriseTeam", "name", "invalid"));
    }
    let sel = b.organization_selection_type.clone().or_else(|| (b.sync_to_organizations.as_deref() == Some("all")).then(|| "all".to_string()));
    let t: Option<EntTeam> = sqlx::query_as(
        "INSERT INTO ent_teams (name, slug, description, sync_to_organizations, organization_selection_type, group_id, notification_setting)
         VALUES ($1, $2, $3, COALESCE($4, 'disabled'), COALESCE($5, 'disabled'), $6, COALESCE($7, 'notifications_enabled'))
         ON CONFLICT (slug) DO NOTHING RETURNING *",
    )
    .bind(name.trim())
    .bind(&slug)
    .bind(b.description.clone().flatten())
    .bind(&b.sync_to_organizations)
    .bind(&sel)
    .bind(b.group_id.clone().flatten())
    .bind(&b.notification_setting)
    .fetch_optional(&s.db)
    .await?;
    let t = t.ok_or_else(|| ApiError::validation("EnterpriseTeam", "name", "already_exists"))?;
    sync_group_members(&s, &t).await?;
    reconcile(&s, &t).await?;
    ent::audit(&s, "business.create_team", Some(&u), json!({"team": t.slug, "team_id": t.id})).await;
    Ok(created(team_json(&s, &t).await?))
}

async fn get_team(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, team)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let t = load(&s, &e, &team).await?;
    Ok(Json(team_json(&s, &t).await?))
}

async fn update(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, team)): Path<(String, String)>, Json(b): Json<TeamBody>) -> ApiResult<Json<Value>> {
    let t = load(&s, &e, &team).await?;
    check_body(&b)?;
    check_group(&s, &b.group_id).await?;
    let name = b.name.clone().filter(|n| !n.trim().is_empty());
    let slug = name.as_deref().map(super::orgs_teams::slugify);
    let t: EntTeam = sqlx::query_as(
        "UPDATE ent_teams SET name = COALESCE($2, name), slug = COALESCE($3, slug),
            description = CASE WHEN $4 THEN $5 ELSE description END,
            sync_to_organizations = COALESCE($6, sync_to_organizations),
            organization_selection_type = COALESCE($7, organization_selection_type),
            group_id = CASE WHEN $8 THEN $9 ELSE group_id END,
            notification_setting = COALESCE($10, notification_setting), updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(t.id)
    .bind(name.as_deref().map(str::trim))
    .bind(&slug)
    .bind(b.description.is_some())
    .bind(b.description.clone().flatten())
    .bind(&b.sync_to_organizations)
    .bind(&b.organization_selection_type)
    .bind(b.group_id.is_some())
    .bind(b.group_id.clone().flatten())
    .bind(&b.notification_setting)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("EnterpriseTeam", "name", "already_exists"),
        e => e.into(),
    })?;
    sync_group_members(&s, &t).await?;
    reconcile(&s, &t).await?;
    ent::audit(&s, "business.update_team", Some(&u), json!({"team": t.slug, "team_id": t.id})).await;
    Ok(Json(team_json(&s, &t).await?))
}

async fn delete_team(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, team)): Path<(String, String)>) -> ApiResult<Response> {
    let t = load(&s, &e, &team).await?;
    // Revoke memberships granted through this team before the grants cascade away.
    sqlx::query("DELETE FROM ent_team_orgs WHERE team_id = $1").bind(t.id).execute(&s.db).await?;
    sqlx::query("UPDATE ent_teams SET organization_selection_type = 'disabled' WHERE id = $1").bind(t.id).execute(&s.db).await?;
    let t2 = EntTeam { organization_selection_type: "disabled".into(), ..t.clone() };
    reconcile(&s, &t2).await?;
    sqlx::query("DELETE FROM ent_teams WHERE id = $1").bind(t.id).execute(&s.db).await?;
    ent::audit(&s, "business.delete_team", Some(&u), json!({"team": t.slug, "team_id": t.id})).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Membership → organization membership

/// Organizations the team is assigned to (`all` selection: every organization).
async fn assigned_orgs(s: &AppState, t: &EntTeam) -> ApiResult<Vec<User>> {
    Ok(match t.organization_selection_type.as_str() {
        "all" => sqlx::query_as("SELECT * FROM users WHERE kind = 'Organization' ORDER BY id").fetch_all(&s.db).await?,
        "disabled" if t.sync_to_organizations == "all" => sqlx::query_as("SELECT * FROM users WHERE kind = 'Organization' ORDER BY id").fetch_all(&s.db).await?,
        _ => sqlx::query_as("SELECT u.* FROM ent_team_orgs x JOIN users u ON u.id = x.org_id WHERE x.team_id = $1 ORDER BY u.id").bind(t.id).fetch_all(&s.db).await?,
    })
}

/// Brings org memberships in line with the team's members × assigned organizations: grants
/// missing memberships (recording them) and revokes grants that no longer apply (only rows
/// forge added, and only when no other enterprise team still grants them).
pub async fn reconcile(s: &AppState, t: &EntTeam) -> ApiResult<()> {
    let orgs = assigned_orgs(s, t).await?;
    let members: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM ent_team_members WHERE team_id = $1").bind(t.id).fetch_all(&s.db).await?;
    let want: HashSet<(i64, i64)> = orgs.iter().flat_map(|o| members.iter().map(move |m| (o.id, *m))).collect();
    let have: Vec<(i64, i64)> = sqlx::query_as("SELECT org_id, user_id FROM ent_org_grants WHERE team_id = $1").bind(t.id).fetch_all(&s.db).await?;
    let have_set: HashSet<(i64, i64)> = have.iter().copied().collect();
    for (org_id, user_id) in &want {
        if have_set.contains(&(*org_id, *user_id)) {
            continue;
        }
        let added = sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1, $2, 'member') ON CONFLICT DO NOTHING")
            .bind(org_id)
            .bind(user_id)
            .execute(&s.db)
            .await?
            .rows_affected();
        let granted_elsewhere: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ent_org_grants WHERE org_id = $1 AND user_id = $2)")
            .bind(org_id)
            .bind(user_id)
            .fetch_one(&s.db)
            .await?;
        // Record the grant if forge created the membership (or another team's grant owns it).
        if added > 0 || granted_elsewhere {
            sqlx::query("INSERT INTO ent_org_grants (team_id, org_id, user_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
                .bind(t.id)
                .bind(org_id)
                .bind(user_id)
                .execute(&s.db)
                .await?;
        }
        if added > 0 {
            if let (Ok(org), Ok(u)) = (crate::db::user_by_id(&s.db, *org_id).await, crate::db::user_by_id(&s.db, *user_id).await) {
                crate::orgs::audit_quiet(&s.db, &org, None, "org.add_member", json!({"user": u.login, "via": format!("enterprise team {}", t.slug)})).await;
            }
        }
    }
    for (org_id, user_id) in have.iter().filter(|g| !want.contains(g)) {
        sqlx::query("DELETE FROM ent_org_grants WHERE team_id = $1 AND org_id = $2 AND user_id = $3").bind(t.id).bind(org_id).bind(user_id).execute(&s.db).await?;
        let still: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ent_org_grants WHERE org_id = $1 AND user_id = $2)")
            .bind(org_id)
            .bind(user_id)
            .fetch_one(&s.db)
            .await?;
        if !still {
            let n = sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2 AND role = 'member'")
                .bind(org_id)
                .bind(user_id)
                .execute(&s.db)
                .await?
                .rows_affected();
            if n > 0 {
                if let (Ok(org), Ok(u)) = (crate::db::user_by_id(&s.db, *org_id).await, crate::db::user_by_id(&s.db, *user_id).await) {
                    crate::orgs::audit_quiet(&s.db, &org, None, "org.remove_member", json!({"user": u.login, "via": format!("enterprise team {}", t.slug)})).await;
                }
            }
        }
    }
    Ok(())
}

/// Reconciles every enterprise team (after SCIM group changes, new organizations, ...).
pub async fn reconcile_all(s: &AppState) -> ApiResult<()> {
    let teams: Vec<EntTeam> = sqlx::query_as("SELECT * FROM ent_teams").fetch_all(&s.db).await?;
    for t in &teams {
        sync_group_members(s, t).await?;
        reconcile(s, t).await?;
    }
    Ok(())
}

/// A team connected to a SCIM group (`group_id`) mirrors the group's members.
async fn sync_group_members(s: &AppState, t: &EntTeam) -> ApiResult<()> {
    let Some(g) = &t.group_id else { return Ok(()) };
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM ent_team_members WHERE team_id = $1").bind(t.id).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO ent_team_members (team_id, user_id)
         SELECT $1, m.user_id FROM ent_scim_groups g JOIN external_group_members m ON m.group_id = g.external_group_id WHERE g.id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(t.id)
    .bind(g)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

fn user_json(s: &AppState, u: &User) -> Value {
    crate::json::user::simple(&s.api_url(), s.base_url(), u)
}

async fn list_members(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Path((e, team)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let t = load(&s, &e, &team).await?;
    let rows: Vec<User> = sqlx::query_as("SELECT u.* FROM ent_team_members m JOIN users u ON u.id = m.user_id WHERE m.team_id = $1 ORDER BY u.id LIMIT $2 OFFSET $3")
        .bind(t.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_team_members WHERE team_id = $1").bind(t.id).fetch_one(&s.db).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), rows.iter().map(|u| user_json(&s, u)).collect()))
}

async fn users_by_login(s: &AppState, logins: &[String]) -> ApiResult<Vec<User>> {
    let mut out = Vec::new();
    for l in logins {
        let u = crate::db::user_by_login(&s.db, l).await.map_err(|_| ApiError::validation("EnterpriseTeam", "usernames", "invalid"))?;
        if u.kind != "User" {
            return Err(ApiError::validation("EnterpriseTeam", "usernames", "invalid"));
        }
        out.push(u);
    }
    Ok(out)
}

fn managed_by_group(t: &EntTeam) -> ApiResult<()> {
    if t.group_id.is_some() {
        return Err(ApiError::unprocessable("This team's membership is managed by its identity provider group."));
    }
    Ok(())
}

async fn change_members(s: &AppState, actor: &User, t: &EntTeam, users: &[User], add: bool) -> ApiResult<()> {
    managed_by_group(t)?;
    for u in users {
        let q = if add {
            "INSERT INTO ent_team_members (team_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM ent_team_members WHERE team_id = $1 AND user_id = $2"
        };
        sqlx::query(sqlx::AssertSqlSafe(q)).bind(t.id).bind(u.id).execute(&s.db).await?;
        ent::audit(s, if add { "business.add_team_member" } else { "business.remove_team_member" }, Some(actor), json!({"team": t.slug, "user": u.login, "user_id": u.id}))
            .await;
    }
    reconcile(s, t).await
}

#[derive(Deserialize)]
struct UsernamesBody {
    usernames: Option<Vec<String>>,
}

async fn bulk_add_members(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team)): Path<(String, String)>, Json(b): Json<UsernamesBody>) -> ApiResult<Json<Value>> {
    let t = load(&s, &e, &team).await?;
    let users = users_by_login(&s, &b.usernames.ok_or_else(|| ApiError::validation("EnterpriseTeam", "usernames", "missing_field"))?).await?;
    change_members(&s, &a, &t, &users, true).await?;
    Ok(Json(json!(users.iter().map(|u| user_json(&s, u)).collect::<Vec<_>>())))
}

async fn bulk_remove_members(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team)): Path<(String, String)>, Json(b): Json<UsernamesBody>) -> ApiResult<Json<Value>> {
    let t = load(&s, &e, &team).await?;
    let users = users_by_login(&s, &b.usernames.ok_or_else(|| ApiError::validation("EnterpriseTeam", "usernames", "missing_field"))?).await?;
    change_members(&s, &a, &t, &users, false).await?;
    Ok(Json(json!(users.iter().map(|u| user_json(&s, u)).collect::<Vec<_>>())))
}

async fn add_member(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team, login)): Path<(String, String, String)>) -> ApiResult<Response> {
    let t = load(&s, &e, &team).await?;
    let users = users_by_login(&s, &[login]).await.map_err(|_| ApiError::not_found())?;
    change_members(&s, &a, &t, &users, true).await?;
    Ok((StatusCode::CREATED, axum::Json(user_json(&s, &users[0]))).into_response())
}

async fn remove_member(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team, login)): Path<(String, String, String)>) -> ApiResult<Response> {
    let t = load(&s, &e, &team).await?;
    let users = users_by_login(&s, &[login]).await.map_err(|_| ApiError::not_found())?;
    change_members(&s, &a, &t, &users, false).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Organization assignments

fn org_json(s: &AppState, o: &User) -> Value {
    crate::json::user::org_simple(&s.api_url(), s.base_url(), o)
}

async fn list_orgs(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Path((e, team)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let t = load(&s, &e, &team).await?;
    let all = assigned_orgs(&s, &t).await?;
    let total = all.len() as i64;
    let items = all.iter().skip(p.offset() as usize).take(p.limit() as usize).map(|o| org_json(&s, o)).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn orgs_by_login(s: &AppState, logins: &[String]) -> ApiResult<Vec<User>> {
    let mut out = Vec::new();
    for l in logins {
        out.push(crate::orgs::org_by_login(s, l).await.map_err(|_| ApiError::validation("EnterpriseTeam", "organization_slugs", "invalid"))?);
    }
    Ok(out)
}

async fn change_orgs(s: &AppState, actor: &User, t: &EntTeam, orgs: &[User], add: bool) -> ApiResult<EntTeam> {
    // Explicit assignments switch the team to "selected" organizations.
    let mut t = t.clone();
    if t.organization_selection_type != "selected" {
        if add || t.organization_selection_type == "all" {
            // From "all": start from the full list, so removing one keeps the others assigned.
            if t.organization_selection_type == "all" {
                sqlx::query("INSERT INTO ent_team_orgs (team_id, org_id) SELECT $1, id FROM users WHERE kind = 'Organization' ON CONFLICT DO NOTHING")
                    .bind(t.id)
                    .execute(&s.db)
                    .await?;
            }
            t = sqlx::query_as("UPDATE ent_teams SET organization_selection_type = 'selected', updated_at = now() WHERE id = $1 RETURNING *")
                .bind(t.id)
                .fetch_one(&s.db)
                .await?;
        }
    }
    for o in orgs {
        let q = if add {
            "INSERT INTO ent_team_orgs (team_id, org_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM ent_team_orgs WHERE team_id = $1 AND org_id = $2"
        };
        sqlx::query(sqlx::AssertSqlSafe(q)).bind(t.id).bind(o.id).execute(&s.db).await?;
        crate::orgs::audit_quiet(&s.db, o, Some(actor), if add { "org.add_enterprise_team" } else { "org.remove_enterprise_team" }, json!({"team": format!("ent:{}", t.slug)}))
            .await;
    }
    reconcile(s, &t).await?;
    Ok(t)
}

#[derive(Deserialize)]
struct OrgSlugsBody {
    organization_slugs: Option<Vec<String>>,
}

async fn bulk_add_orgs(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team)): Path<(String, String)>, Json(b): Json<OrgSlugsBody>) -> ApiResult<Json<Value>> {
    let t = load(&s, &e, &team).await?;
    let orgs = orgs_by_login(&s, &b.organization_slugs.ok_or_else(|| ApiError::validation("EnterpriseTeam", "organization_slugs", "missing_field"))?).await?;
    change_orgs(&s, &a, &t, &orgs, true).await?;
    Ok(Json(json!(orgs.iter().map(|o| org_json(&s, o)).collect::<Vec<_>>())))
}

async fn bulk_remove_orgs(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team)): Path<(String, String)>, Json(b): Json<OrgSlugsBody>) -> ApiResult<Response> {
    let t = load(&s, &e, &team).await?;
    let orgs = orgs_by_login(&s, &b.organization_slugs.ok_or_else(|| ApiError::validation("EnterpriseTeam", "organization_slugs", "missing_field"))?).await?;
    change_orgs(&s, &a, &t, &orgs, false).await?;
    Ok(no_content())
}

async fn get_org(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, team, org)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let t = load(&s, &e, &team).await?;
    let o = crate::orgs::org_by_login(&s, &org).await?;
    if !assigned_orgs(&s, &t).await?.iter().any(|x| x.id == o.id) {
        return Err(ApiError::not_found());
    }
    Ok(Json(org_json(&s, &o)))
}

async fn add_org(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team, org)): Path<(String, String, String)>) -> ApiResult<Response> {
    let t = load(&s, &e, &team).await?;
    let o = crate::orgs::org_by_login(&s, &org).await?;
    change_orgs(&s, &a, &t, std::slice::from_ref(&o), true).await?;
    Ok(created(org_json(&s, &o)))
}

async fn remove_org(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, team, org)): Path<(String, String, String)>) -> ApiResult<Response> {
    let t = load(&s, &e, &team).await?;
    let o = crate::orgs::org_by_login(&s, &org).await?;
    change_orgs(&s, &a, &t, std::slice::from_ref(&o), false).await?;
    Ok(no_content())
}
