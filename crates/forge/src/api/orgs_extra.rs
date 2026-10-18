//! Organizations: /organizations, org memberships (incl. the authenticated user's), member
//! removal and public membership. Teams live in `orgs_teams`, settings/roles/etc. in the
//! other `orgs_*` modules.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::orgs::{audit_quiet, org_by_login, require_owner};
use crate::state::AppState;

pub use crate::orgs::org_role;
pub use super::orgs_teams::{team_full_json, team_json, TeamInput, TeamRow};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/organizations", get(list_orgs))
        .route("/user/memberships/orgs", get(my_memberships))
        .route("/user/memberships/orgs/{org}", get(my_membership).patch(update_my_membership))
        .route("/orgs/{org}/memberships/{username}", get(get_membership).put(set_membership).delete(remove_membership))
        .route("/orgs/{org}/members/{username}", axum::routing::delete(remove_member))
        .route("/orgs/{org}/public_members", get(public_members))
        .route("/orgs/{org}/public_members/{username}", get(check_public).put(publicize).delete(conceal))
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

/// Membership JSON with `permissions.can_create_repository` from the org's member privileges.
async fn membership_json_for(s: &AppState, org: &User, user: &User, role: &str) -> ApiResult<Value> {
    let mut v = membership_json(s, org, user, role);
    let can = role == "admin" || crate::orgs::settings(&s.db, org.id).await?.members_can_create_repositories;
    v["permissions"] = json!({ "can_create_repository": can });
    Ok(v)
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
    let mut items = Vec::new();
    for (id, role) in &rows {
        if let Some(o) = orgs.get(id) {
            items.push(membership_json_for(&s, o, &u, role).await?);
        }
    }
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn my_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    let role = org_role(&s.db, o.id, u.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(membership_json_for(&s, &o, &u, &role).await?))
}

#[derive(Deserialize)]
struct StateBody {
    state: Option<String>,
}

/// Memberships are always active in forge (GHES adds members directly), so the only valid
/// transition is to `active`.
async fn update_my_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<StateBody>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    let role = org_role(&s.db, o.id, u.id).await?.ok_or_else(ApiError::not_found)?;
    match b.state.as_deref() {
        Some("active") => {}
        None => return Err(ApiError::validation("Membership", "state", "missing_field")),
        Some(_) => return Err(ApiError::validation("Membership", "state", "invalid")),
    }
    Ok(Json(membership_json_for(&s, &o, &u, &role).await?))
}

async fn get_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    if org_role(&s.db, o.id, u.id).await?.is_none() && !u.site_admin {
        return Err(ApiError::not_found());
    }
    let target = db::user_by_login(&s.db, &login).await?;
    let role = org_role(&s.db, o.id, target.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(membership_json_for(&s, &o, &target, &role).await?))
}

#[derive(Deserialize)]
struct RoleBody {
    role: Option<String>,
}

async fn other_owners(s: &AppState, org: &User, user_id: i64) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM org_members WHERE org_id = $1 AND role = 'admin' AND user_id <> $2")
        .bind(org.id)
        .bind(user_id)
        .fetch_one(&s.db)
        .await?)
}

/// Adds or updates an org membership (no invitation flow: membership is immediate, as on GHES).
/// The last owner cannot be demoted.
pub async fn set_org_membership(s: &AppState, org: &User, actor: &User, user: &User, role: &str) -> ApiResult<()> {
    if role != "admin" && role != "member" {
        return Err(ApiError::validation("Membership", "role", "invalid"));
    }
    let prev = org_role(&s.db, org.id, user.id).await?;
    if prev.as_deref() == Some("admin") && role == "member" && other_owners(s, org, user.id).await? == 0 {
        return Err(ApiError::forbidden("Cannot demote the last owner of an organization."));
    }
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1,$2,$3) ON CONFLICT (org_id, user_id) DO UPDATE SET role = $3")
        .bind(org.id)
        .bind(user.id)
        .bind(role)
        .execute(&s.db)
        .await?;
    if prev.as_deref() == Some(role) {
        return Ok(());
    }
    let action = match prev {
        None => "org.add_member",
        Some(_) => "org.update_member",
    };
    audit_quiet(&s.db, org, Some(actor), action, json!({ "user": user.login, "permission": role })).await;
    if action == "org.add_member" {
        let api = s.api_url();
        let p = json!({ "action": "member_added", "membership": membership_json(s, org, user, role),
            "sender": crate::json::user::simple(&api, s.base_url(), actor) });
        let _ = crate::webhooks::enqueue_org_event(s, org, "organization", p).await;
    }
    Ok(())
}

async fn set_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>, Json(b): Json<RoleBody>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    if target.kind != "User" {
        return Err(ApiError::validation("Membership", "user", "invalid"));
    }
    let role = b.role.unwrap_or_else(|| "member".into());
    set_org_membership(&s, &o, &u, &target, &role).await?;
    Ok(Json(membership_json_for(&s, &o, &target, &role).await?))
}

/// Removes `target` from the org: membership, team memberships and org-role assignments.
pub async fn remove_org_member(s: &AppState, org: &User, actor: &User, target: &User) -> ApiResult<()> {
    if target.id != actor.id {
        require_owner(s, org, actor).await?;
    }
    let role = org_role(&s.db, org.id, target.id).await?;
    if role.as_deref() == Some("admin") && other_owners(s, org, target.id).await? == 0 {
        return Err(ApiError::forbidden("Cannot remove the last owner of an organization."));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2").bind(org.id).bind(target.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM team_members WHERE user_id = $2 AND team_id IN (SELECT id FROM teams WHERE org_id = $1)")
        .bind(org.id)
        .bind(target.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM org_role_users WHERE org_id = $1 AND user_id = $2").bind(org.id).bind(target.id).execute(&mut *tx).await?;
    tx.commit().await?;
    if role.is_some() {
        audit_quiet(&s.db, org, Some(actor), "org.remove_member", json!({ "user": target.login })).await;
        let api = s.api_url();
        let p = json!({ "action": "member_removed", "membership": membership_json(s, org, target, role.as_deref().unwrap_or("member")),
            "sender": crate::json::user::simple(&api, s.base_url(), actor) });
        let _ = crate::webhooks::enqueue_org_event(s, org, "organization", p).await;
    }
    Ok(())
}

async fn remove_membership(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    remove_org_member(&s, &o, &u, &target).await?;
    Ok(no_content())
}

async fn remove_member(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    remove_org_member(&s, &o, &u, &target).await?;
    Ok(no_content())
}

/// Members who publicized their membership.
async fn public_members(State(s): State<AppState>, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    let users: Vec<User> = sqlx::query_as(
        "SELECT u.* FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 AND m.public ORDER BY u.id LIMIT $2 OFFSET $3",
    )
    .bind(o.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items = users.iter().map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn check_public(State(s): State<AppState>, _u: MaybeUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    let public: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2 AND public)")
        .bind(o.id)
        .bind(target.id)
        .fetch_one(&s.db)
        .await?;
    if public { Ok(no_content()) } else { Err(ApiError::not_found()) }
}

/// Only the member themselves can publicize or conceal their membership.
async fn set_public(s: &AppState, u: &User, org: &str, login: &str, public: bool) -> ApiResult<Response> {
    let o = org_by_login(s, org).await?;
    if !login.eq_ignore_ascii_case(&u.login) {
        return Err(ApiError::forbidden("You can only publicize or conceal your own membership."));
    }
    let n = sqlx::query("UPDATE org_members SET public = $3 WHERE org_id = $1 AND user_id = $2")
        .bind(o.id)
        .bind(u.id)
        .bind(public)
        .execute(&s.db)
        .await?;
    if n.rows_affected() == 0 {
        return Err(ApiError::forbidden("You must be a member of the organization."));
    }
    audit_quiet(&s.db, &o, Some(u), if public { "org.publicize_member" } else { "org.conceal_member" }, json!({ "user": u.login })).await;
    Ok(no_content())
}

async fn publicize(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    set_public(&s, &u, &org, &login, true).await
}

async fn conceal(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    set_public(&s, &u, &org, &login, false).await
}
