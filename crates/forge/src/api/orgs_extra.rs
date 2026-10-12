//! Organizations extras: /organizations, memberships, teams (minimal), org events.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/organizations", get(list_orgs))
        .route("/user/memberships/orgs", get(my_memberships))
        .route("/user/memberships/orgs/{org}", get(my_membership).patch(update_my_membership))
        .route("/orgs/{org}/memberships/{username}", get(get_membership).put(set_membership).delete(remove_membership))
        .route("/orgs/{org}/members/{username}", axum::routing::delete(remove_member))
        .route("/orgs/{org}/public_members", get(public_members))
        .route("/orgs/{org}/teams", get(list_teams).post(create_team))
        .route("/orgs/{org}/teams/{slug}", get(get_team).patch(update_team).delete(delete_team))
        .route("/orgs/{org}/teams/{slug}/members", get(team_members))
        .route("/orgs/{org}/teams/{slug}/memberships/{username}", get(get_team_membership).put(put_team_membership).delete(delete_team_membership))
        .route("/user/teams", get(my_teams))
}

async fn org_by_login(s: &AppState, org: &str) -> ApiResult<User> {
    let o = db::user_by_login(&s.db, org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    Ok(o)
}

/// Role of `user_id` in `org_id` ("admin" | "member"), if a member.
pub async fn org_role(db: &sqlx::PgPool, org_id: i64, user_id: i64) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
        .bind(org_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?)
}

async fn require_org_admin(s: &AppState, org: &User, u: &User) -> ApiResult<()> {
    if u.site_admin || org_role(&s.db, org.id, u.id).await?.as_deref() == Some("admin") {
        Ok(())
    } else {
        Err(ApiError::forbidden("You must be an admin to perform this action."))
    }
}

#[derive(Deserialize)]
struct SinceQuery {
    since: Option<i64>,
    per_page: Option<i64>,
}

async fn list_orgs(State(s): State<AppState>, uri: OriginalUri, Query(q): Query<SinceQuery>) -> ApiResult<Response> {
    let per = q.per_page.unwrap_or(30).clamp(1, 100);
    let orgs: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE kind = 'Organization' AND id > $1 ORDER BY id LIMIT $2")
        .bind(q.since.unwrap_or(0))
        .bind(per)
        .fetch_all(&s.db)
        .await?;
    let items: Vec<Value> = orgs.iter().map(|o| crate::json::user::org_simple(&s.api_url(), s.base_url(), o)).collect();
    let mut res = axum::Json(Value::Array(items)).into_response();
    if orgs.len() as i64 == per {
        let next = format!("<{}{}?per_page={per}&since={}>; rel=\"next\"", s.base_url(), uri.0.path(), orgs.last().unwrap().id);
        if let Ok(v) = next.parse() {
            res.headers_mut().insert(axum::http::header::LINK, v);
        }
    }
    Ok(res)
}

pub fn membership_json(s: &AppState, org: &User, user: &User, role: &str) -> Value {
    let api = s.api_url();
    json!({
        "url": format!("{api}/orgs/{}/memberships/{}", org.login, user.login),
        "state": "active",
        "role": role,
        "organization_url": format!("{api}/orgs/{}", org.login),
        "organization": crate::json::user::org_simple(&api, s.base_url(), org),
        "user": crate::json::user::simple(&api, s.base_url(), user),
        "permissions": { "can_create_repository": true },
    })
}

#[derive(Deserialize)]
struct MembershipsQuery {
    state: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn my_memberships(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<MembershipsQuery>) -> ApiResult<Paged> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    if q.state.as_deref() == Some("pending") {
        return Ok(Paged::new(&uri, &s, p, Some(0), vec![]));
    }
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT org_id, role FROM org_members WHERE user_id = $1 ORDER BY org_id LIMIT $2 OFFSET $3")
        .bind(u.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let orgs = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let items = rows
        .iter()
        .filter_map(|(id, role)| orgs.get(id).map(|o| membership_json(&s, o, &u, role)))
        .collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn my_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    let role = org_role(&s.db, o.id, u.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(membership_json(&s, &o, &u, &role)))
}

#[derive(Deserialize)]
struct StateBody {
    #[allow(dead_code)]
    state: Option<String>,
}

async fn update_my_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(_b): Json<StateBody>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    let role = org_role(&s.db, o.id, u.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(membership_json(&s, &o, &u, &role)))
}

async fn get_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    if org_role(&s.db, o.id, u.id).await?.is_none() && !u.site_admin {
        return Err(ApiError::not_found());
    }
    let target = db::user_by_login(&s.db, &login).await?;
    let role = org_role(&s.db, o.id, target.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(membership_json(&s, &o, &target, &role)))
}

#[derive(Deserialize)]
struct RoleBody {
    role: Option<String>,
}

/// Adds or updates an org membership (no invitation flow: membership is immediate).
pub async fn set_org_membership(s: &AppState, org: &User, user: &User, role: &str) -> ApiResult<()> {
    if role != "admin" && role != "member" {
        return Err(ApiError::validation("Membership", "role", "invalid"));
    }
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1,$2,$3) ON CONFLICT (org_id, user_id) DO UPDATE SET role = $3")
        .bind(org.id)
        .bind(user.id)
        .bind(role)
        .execute(&s.db)
        .await?;
    Ok(())
}

async fn set_membership(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, login)): Path<(String, String)>,
    Json(b): Json<RoleBody>,
) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_org_admin(&s, &o, &u).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    if target.kind != "User" {
        return Err(ApiError::validation("Membership", "user", "invalid"));
    }
    let role = b.role.unwrap_or_else(|| "member".into());
    set_org_membership(&s, &o, &target, &role).await?;
    Ok(Json(membership_json(&s, &o, &target, &role)))
}

async fn remove_member_inner(s: &AppState, u: &User, org: &str, login: &str) -> ApiResult<Response> {
    let o = org_by_login(s, org).await?;
    let target = db::user_by_login(&s.db, login).await?;
    if target.id != u.id {
        require_org_admin(s, &o, u).await?;
    }
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM org_members WHERE org_id = $1 AND role = 'admin' AND user_id <> $2")
        .bind(o.id)
        .bind(target.id)
        .fetch_one(&s.db)
        .await?;
    if admins == 0 && org_role(&s.db, o.id, target.id).await?.as_deref() == Some("admin") {
        return Err(ApiError::forbidden("Cannot remove the last owner of an organization."));
    }
    sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2").bind(o.id).bind(target.id).execute(&s.db).await?;
    sqlx::query("DELETE FROM team_members WHERE user_id = $2 AND team_id IN (SELECT id FROM teams WHERE org_id = $1)")
        .bind(o.id)
        .bind(target.id)
        .execute(&s.db)
        .await?;
    Ok(no_content())
}

async fn remove_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    remove_member_inner(&s, &u, &org, &login).await
}

async fn remove_member(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    remove_member_inner(&s, &u, &org, &login).await
}

async fn public_members(State(s): State<AppState>, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    let users: Vec<User> = sqlx::query_as(
        "SELECT u.* FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 ORDER BY u.id LIMIT $2 OFFSET $3",
    )
    .bind(o.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items = users.iter().map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

// ---------------------------------------------------------------------------------------------
// Teams

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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn team_simple(s: &AppState, org: &User, t: &TeamRow) -> Value {
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
        "notification_setting": "notifications_enabled",
        "html_url": format!("{}/orgs/{}/teams/{}", s.base_url(), org.login, t.slug),
        "repositories_url": format!("{url}/repos"),
        "slug": t.slug,
        "type": "organization",
        "organization_id": org.id,
    })
}

pub fn team_json(s: &AppState, org: &User, t: &TeamRow, parent: Option<&TeamRow>) -> Value {
    let mut v = team_simple(s, org, t);
    let p = t.permission.as_str();
    v["permissions"] = json!({
        "pull": true,
        "triage": matches!(p, "triage" | "push" | "maintain" | "admin"),
        "push": matches!(p, "push" | "maintain" | "admin"),
        "maintain": matches!(p, "maintain" | "admin"),
        "admin": p == "admin",
    });
    v["parent"] = parent.map(|pt| team_simple(s, org, pt)).unwrap_or(Value::Null);
    v
}

async fn teams_json(s: &AppState, org: &User, rows: &[TeamRow]) -> ApiResult<Vec<Value>> {
    let pids: Vec<i64> = rows.iter().filter_map(|t| t.parent_id).collect();
    // Only same-org parents: `parent_team_id` is client input and must not expose another
    // organization's (possibly secret) team.
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
    let members: i64 = sqlx::query_scalar("SELECT count(*) FROM team_members WHERE team_id = $1").bind(t.id).fetch_one(&s.db).await?;
    v["created_at"] = json!(crate::json::ts(t.created_at));
    v["updated_at"] = json!(crate::json::ts(t.updated_at));
    v["members_count"] = json!(members);
    v["repos_count"] = json!(0);
    v["organization"] = super::users::org_json(s, org).await?;
    Ok(v)
}

async fn load_team(s: &AppState, org: &User, slug: &str) -> ApiResult<TeamRow> {
    sqlx::query_as("SELECT * FROM teams WHERE org_id = $1 AND slug = $2")
        .bind(org.id)
        .bind(slug.to_lowercase())
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn list_teams(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    if org_role(&s.db, o.id, u.id).await?.is_none() && !u.site_admin {
        return Err(ApiError::forbidden("You must be a member of the organization to list teams."));
    }
    let rows: Vec<TeamRow> = sqlx::query_as("SELECT * FROM teams WHERE org_id = $1 ORDER BY name LIMIT $2 OFFSET $3")
        .bind(o.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM teams WHERE org_id = $1").bind(o.id).fetch_one(&s.db).await?;
    let items = teams_json(&s, &o, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
pub struct TeamInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub privacy: Option<String>,
    pub permission: Option<String>,
    pub parent_team_id: Option<i64>,
    pub maintainers: Option<Vec<String>>,
}

fn slugify(name: &str) -> String {
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

pub async fn create_team_svc(s: &AppState, org: &User, actor: &User, req: TeamInput) -> ApiResult<TeamRow> {
    let name = req.name.clone().filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("Team", "name", "missing_field"))?;
    let slug = slugify(&name);
    let t: Option<TeamRow> = sqlx::query_as(
        "INSERT INTO teams (org_id, name, slug, description, privacy, permission, parent_id) VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(org.id)
    .bind(&name)
    .bind(&slug)
    .bind(&req.description)
    .bind(req.privacy.as_deref().unwrap_or("secret"))
    .bind(req.permission.as_deref().unwrap_or("pull"))
    .bind(req.parent_team_id)
    .fetch_optional(&s.db)
    .await?;
    let t = t.ok_or_else(|| ApiError::validation("Team", "name", "already_exists"))?;
    let mut maint = req.maintainers.unwrap_or_default();
    if maint.is_empty() {
        maint.push(actor.login.clone());
    }
    for m in maint {
        if let Ok(mu) = db::user_by_login(&s.db, &m).await {
            sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1,$2,'maintainer') ON CONFLICT DO NOTHING")
                .bind(t.id)
                .bind(mu.id)
                .execute(&s.db)
                .await?;
        }
    }
    Ok(t)
}

async fn create_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(req): Json<TeamInput>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_org_admin(&s, &o, &u).await?;
    let t = create_team_svc(&s, &o, &u, req).await?;
    Ok(created(team_full_json(&s, &o, &t).await?))
}

async fn get_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    if org_role(&s.db, o.id, u.id).await?.is_none() && !u.site_admin {
        return Err(ApiError::not_found());
    }
    let t = load_team(&s, &o, &slug).await?;
    Ok(Json(team_full_json(&s, &o, &t).await?))
}

async fn update_team(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, slug)): Path<(String, String)>,
    Json(req): Json<TeamInput>,
) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_org_admin(&s, &o, &u).await?;
    let t = load_team(&s, &o, &slug).await?;
    let new_slug = req.name.as_deref().map(slugify);
    let t: TeamRow = sqlx::query_as(
        "UPDATE teams SET name = COALESCE($2, name), slug = COALESCE($3, slug), description = COALESCE($4, description),
            privacy = COALESCE($5, privacy), permission = COALESCE($6, permission),
            parent_id = COALESCE($7, parent_id), updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(t.id)
    .bind(&req.name)
    .bind(new_slug)
    .bind(&req.description)
    .bind(&req.privacy)
    .bind(&req.permission)
    .bind(req.parent_team_id)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Team", "name", "already_exists"),
        e => e.into(),
    })?;
    Ok(Json(team_full_json(&s, &o, &t).await?))
}

async fn delete_team(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_org_admin(&s, &o, &u).await?;
    let t = load_team(&s, &o, &slug).await?;
    sqlx::query("DELETE FROM teams WHERE id = $1").bind(t.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn team_members(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(u): AuthUser,
    Path((org, slug)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    if org_role(&s.db, o.id, u.id).await?.is_none() && !u.site_admin {
        return Err(ApiError::not_found());
    }
    let t = load_team(&s, &o, &slug).await?;
    let users: Vec<User> = sqlx::query_as(
        "SELECT u.* FROM team_members m JOIN users u ON u.id = m.user_id WHERE m.team_id = $1 ORDER BY u.id LIMIT $2 OFFSET $3",
    )
    .bind(t.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items = users.iter().map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

fn team_membership_json(s: &AppState, org: &User, t: &TeamRow, user: &User, role: &str) -> Value {
    json!({
        "url": format!("{}/organizations/{}/team/{}/memberships/{}", s.api_url(), org.id, t.id, user.login),
        "role": role,
        "state": "active",
    })
}

async fn get_team_membership(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, slug, login)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    if org_role(&s.db, o.id, u.id).await?.is_none() && !u.site_admin {
        return Err(ApiError::not_found());
    }
    let t = load_team(&s, &o, &slug).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    let role: String = sqlx::query_scalar("SELECT role FROM team_members WHERE team_id = $1 AND user_id = $2")
        .bind(t.id)
        .bind(target.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(team_membership_json(&s, &o, &t, &target, &role)))
}

async fn put_team_membership(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, slug, login)): Path<(String, String, String)>,
    Json(b): Json<RoleBody>,
) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_org_admin(&s, &o, &u).await?;
    let t = load_team(&s, &o, &slug).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    let role = b.role.unwrap_or_else(|| "member".into());
    if role != "member" && role != "maintainer" {
        return Err(ApiError::validation("TeamMember", "role", "invalid"));
    }
    if org_role(&s.db, o.id, target.id).await?.is_none() {
        set_org_membership(&s, &o, &target, "member").await?;
    }
    sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1,$2,$3) ON CONFLICT (team_id, user_id) DO UPDATE SET role = $3")
        .bind(t.id)
        .bind(target.id)
        .bind(&role)
        .execute(&s.db)
        .await?;
    Ok(Json(team_membership_json(&s, &o, &t, &target, &role)))
}

async fn delete_team_membership(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((org, slug, login)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_org_admin(&s, &o, &u).await?;
    let t = load_team(&s, &o, &slug).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    sqlx::query("DELETE FROM team_members WHERE team_id = $1 AND user_id = $2").bind(t.id).bind(target.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn my_teams(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, _m: MaybeUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
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
