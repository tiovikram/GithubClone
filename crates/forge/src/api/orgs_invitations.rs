//! Organization invitations and blocks, shaped like api.github.com (these endpoints are not in
//! GHES 3.22, whose `PUT /orgs/{org}/memberships/{username}` adds members directly; forge keeps
//! that and offers invitations on top, for the web UI's "Invite member" flow):
//!
//! - `GET/POST /orgs/{org}/invitations`, `DELETE /orgs/{org}/invitations/{id}`,
//!   `GET /orgs/{org}/invitations/{id}/teams`, `GET /orgs/{org}/failed_invitations`,
//!   `GET /orgs/{org}/teams/{team_slug}/invitations`;
//! - the invitee sees the invitation as a `pending` membership (`GET /user/memberships/orgs`)
//!   and accepts it with `PATCH /user/memberships/orgs/{org}` `{"state":"active"}` (in
//!   `orgs_extra`), or declines it with the forge-only `DELETE /api/forge/user/memberships/orgs/{org}`;
//! - an invitation fails when it expires (7 days) or the invitee is blocked;
//! - `GET /orgs/{org}/blocks`, `GET/PUT/DELETE /orgs/{org}/blocks/{username}`: blocked users
//!   cannot be invited or added, and cannot open issues or pull requests or comment in the
//!   organization's repositories.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::{delete, get};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::orgs::{audit_quiet, org_by_login, org_role, require_member, require_owner};
use crate::state::AppState;

/// Pending invitations older than this are reported as failed ("expired").
pub const INVITATION_TTL_DAYS: i64 = 7;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/invitations", get(list).post(create))
        .route("/orgs/{org}/invitations/{id}", delete(cancel))
        .route("/orgs/{org}/invitations/{id}/teams", get(invitation_teams))
        .route("/orgs/{org}/failed_invitations", get(list_failed))
        .route("/orgs/{org}/teams/{team_slug}/invitations", get(team_invitations))
        .route("/orgs/{org}/blocks", get(list_blocks))
        .route("/orgs/{org}/blocks/{username}", get(check_block).put(block).delete(unblock))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InvitationRow {
    pub id: i64,
    pub org_id: i64,
    pub invitee_id: Option<i64>,
    pub email: Option<String>,
    pub role: String,
    pub team_ids: Vec<i64>,
    pub inviter_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub failed_at: Option<DateTime<Utc>>,
    pub failed_reason: Option<String>,
}

/// Marks expired pending invitations of `org_id` as failed.
async fn expire(db: &sqlx::PgPool, org_id: i64) -> ApiResult<()> {
    sqlx::query(
        "UPDATE org_invitations SET failed_at = created_at + make_interval(days => $2), failed_reason = 'Invitation expired'
          WHERE org_id = $1 AND failed_at IS NULL AND created_at < now() - make_interval(days => $2)",
    )
    .bind(org_id)
    .bind(INVITATION_TTL_DAYS as i32)
    .execute(db)
    .await?;
    Ok(())
}

async fn invitations_json(s: &AppState, org: &User, rows: &[InvitationRow]) -> ApiResult<Vec<Value>> {
    let mut ids: Vec<i64> = rows.iter().filter_map(|r| r.invitee_id).collect();
    ids.extend(rows.iter().filter_map(|r| r.inviter_id));
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let api = s.api_url();
    Ok(rows
        .iter()
        .map(|r| {
            let invitee = r.invitee_id.and_then(|i| users.get(&i));
            json!({
                "id": r.id,
                "node_id": crate::ids::encode("OrganizationInvitation", r.id),
                "login": invitee.map(|u| u.login.clone()),
                "email": r.email.clone().or_else(|| invitee.and_then(|u| u.email.clone())),
                "role": r.role,
                "created_at": crate::json::ts(r.created_at),
                "failed_at": crate::json::ts_opt(r.failed_at),
                "failed_reason": r.failed_reason,
                "inviter": r.inviter_id.and_then(|i| users.get(&i)).map(|u| crate::json::user::simple(&api, s.base_url(), u)),
                "team_count": r.team_ids.len(),
                "invitation_teams_url": format!("{api}/organizations/{}/invitations/{}/teams", org.id, r.id),
                "invitation_source": "member",
            })
        })
        .collect())
}

#[derive(Deserialize, Default)]
struct ListQuery {
    role: Option<String>,
    invitation_source: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_rows(s: &AppState, org: &User, failed: bool, team: Option<i64>, role: Option<&str>, p: &PageParams) -> ApiResult<(i64, Vec<InvitationRow>)> {
    expire(&s.db, org.id).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM org_invitations WHERE org_id = $1 AND (failed_at IS NOT NULL) = $2 AND ($3::bigint IS NULL OR $3 = ANY(team_ids)) AND ($4::text IS NULL OR role = $4)")
        .bind(org.id)
        .bind(failed)
        .bind(team)
        .bind(role)
        .fetch_one(&s.db)
        .await?;
    let rows = sqlx::query_as("SELECT * FROM org_invitations WHERE org_id = $1 AND (failed_at IS NOT NULL) = $2 AND ($3::bigint IS NULL OR $3 = ANY(team_ids)) AND ($4::text IS NULL OR role = $4) ORDER BY id DESC LIMIT $5 OFFSET $6")
        .bind(org.id)
        .bind(failed)
        .bind(team)
        .bind(role)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    Ok((total, rows))
}

const ROLE_FILTERS: &[&str] = &["all", "admin", "direct_member", "billing_manager", "hiring_manager"];

async fn list(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    if q.role.as_deref().is_some_and(|r| !ROLE_FILTERS.contains(&r)) {
        return Err(ApiError::validation("OrganizationInvitation", "role", "invalid"));
    }
    if q.invitation_source.as_deref().is_some_and(|r| !["all", "member", "scim"].contains(&r)) {
        return Err(ApiError::validation("OrganizationInvitation", "invitation_source", "invalid"));
    }
    let p = PageParams { page: q.page, per_page: q.per_page };
    let role = q.role.as_deref().filter(|r| *r != "all");
    let (total, rows) = list_rows(&s, &o, false, None, role, &p).await?;
    let items = if q.invitation_source.as_deref() == Some("scim") { vec![] } else { invitations_json(&s, &o, &rows).await? };
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn list_failed(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let (total, rows) = list_rows(&s, &o, true, None, None, &p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), invitations_json(&s, &o, &rows).await?))
}

async fn team_invitations(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, slug)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_member(&s, &o, &u).await?;
    let t = super::orgs_teams::load_team(&s, &o, &slug).await?;
    if !crate::orgs::is_owner(&s.db, &o, &u).await? && super::orgs_teams::direct_role(&s, t.id, u.id).await?.as_deref() != Some("maintainer") {
        return Err(ApiError::forbidden("You must be an organization owner or team maintainer."));
    }
    let (total, rows) = list_rows(&s, &o, false, Some(t.id), None, &p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), invitations_json(&s, &o, &rows).await?))
}

#[derive(Deserialize, Default)]
struct CreateBody {
    invitee_id: Option<i64>,
    email: Option<String>,
    role: Option<String>,
    team_ids: Option<Vec<i64>>,
}

fn invalid(field: &str) -> ApiError {
    ApiError::validation("OrganizationInvitation", field, "invalid")
}

/// Whether `user_id` is blocked by `org_id`.
pub async fn is_blocked(db: &sqlx::PgPool, org_id: i64, user_id: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_blocks WHERE org_id = $1 AND user_id = $2)")
        .bind(org_id)
        .bind(user_id)
        .fetch_one(db)
        .await?)
}

/// 403 when `owner` is an organization that blocked `actor` (issues, pull requests, comments).
pub async fn require_not_blocked(db: &sqlx::PgPool, owner: &User, actor: &User) -> ApiResult<()> {
    if owner.kind == "Organization" && is_blocked(db, owner.id, actor.id).await? {
        return Err(ApiError::forbidden("You have been blocked from interacting with this organization's repositories."));
    }
    Ok(())
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<CreateBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let role = b.role.unwrap_or_else(|| "direct_member".into());
    if !["admin", "direct_member", "billing_manager", "reinstate"].contains(&role.as_str()) {
        return Err(invalid("role"));
    }
    let role = if role == "reinstate" { "direct_member".to_string() } else { role };
    let email = b.email.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
    let invitee = match (b.invitee_id, &email) {
        (Some(id), _) => Some(db::user_by_id(&s.db, id).await.map_err(|_| invalid("invitee_id"))?),
        (None, Some(e)) => {
            if !e.contains('@') {
                return Err(invalid("email"));
            }
            sqlx::query_as::<_, User>("SELECT * FROM users WHERE kind = 'User' AND lower(email) = lower($1) LIMIT 1")
                .bind(e)
                .fetch_optional(&s.db)
                .await?
        }
        (None, None) => return Err(ApiError::validation("OrganizationInvitation", "invitee_id", "missing_field")),
    };
    if let Some(t) = &invitee {
        if t.kind != "User" {
            return Err(invalid("invitee_id"));
        }
        if org_role(&s.db, o.id, t.id).await?.is_some() {
            return Err(ApiError::unprocessable("Invitee is already a part of this organization"));
        }
        if is_blocked(&s.db, o.id, t.id).await? {
            return Err(ApiError::unprocessable("Invitee is blocked by this organization"));
        }
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_invitations WHERE org_id = $1 AND invitee_id = $2 AND failed_at IS NULL)")
            .bind(o.id)
            .bind(t.id)
            .fetch_one(&s.db)
            .await?;
        if pending {
            return Err(ApiError::unprocessable("Invitee has already been invited to this organization"));
        }
    }
    let team_ids = b.team_ids.unwrap_or_default();
    if !team_ids.is_empty() {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM teams WHERE org_id = $1 AND id = ANY($2)").bind(o.id).bind(&team_ids).fetch_one(&s.db).await?;
        if n != team_ids.len() as i64 {
            return Err(invalid("team_ids"));
        }
    }
    let row: InvitationRow = sqlx::query_as(
        "INSERT INTO org_invitations (org_id, invitee_id, email, role, team_ids, inviter_id) VALUES ($1,$2,$3,$4,$5,$6) RETURNING *",
    )
    .bind(o.id)
    .bind(invitee.as_ref().map(|t| t.id))
    .bind(if invitee.is_some() { None } else { email.clone() })
    .bind(&role)
    .bind(&team_ids)
    .bind(u.id)
    .fetch_one(&s.db)
    .await?;
    let who = invitee.as_ref().map(|t| t.login.clone()).or(email).unwrap_or_default();
    audit_quiet(&s.db, &o, Some(&u), "org.invite_member", json!({ "user": who, "role": role })).await;
    let v = invitations_json(&s, &o, std::slice::from_ref(&row)).await?.remove(0);
    Ok(created(v))
}

async fn load(s: &AppState, org: &User, id: i64) -> ApiResult<InvitationRow> {
    sqlx::query_as("SELECT * FROM org_invitations WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn cancel(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let inv = load(&s, &o, id).await?;
    sqlx::query("DELETE FROM org_invitations WHERE id = $1").bind(inv.id).execute(&s.db).await?;
    audit_quiet(&s.db, &o, Some(&u), "org.cancel_invitation", json!({ "invitation_id": inv.id })).await;
    Ok(no_content())
}

async fn invitation_teams(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let inv = load(&s, &o, id).await?;
    let rows: Vec<super::orgs_teams::TeamRow> = sqlx::query_as("SELECT * FROM teams WHERE org_id = $1 AND id = ANY($2) ORDER BY id LIMIT $3 OFFSET $4")
        .bind(o.id)
        .bind(&inv.team_ids)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = super::orgs_teams::teams_json(&s, &o, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(inv.team_ids.len() as i64), items))
}

// ---------------------------------------------------------------------------------------------
// The invitee's side (used by `orgs_extra` for /user/memberships/orgs)

/// Pending (not failed, not expired) invitations addressed to `u` (by id or by email).
pub async fn pending_for(s: &AppState, u: &User, org_id: Option<i64>) -> ApiResult<Vec<InvitationRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM org_invitations
          WHERE failed_at IS NULL AND created_at >= now() - make_interval(days => $3)
            AND (invitee_id = $1 OR (invitee_id IS NULL AND $2::text IS NOT NULL AND lower(email) = lower($2)))
            AND ($4::bigint IS NULL OR org_id = $4)
          ORDER BY id",
    )
    .bind(u.id)
    .bind(u.email.as_deref())
    .bind(INVITATION_TTL_DAYS as i32)
    .bind(org_id)
    .fetch_all(&s.db)
    .await?)
}

/// The membership role an invitation grants ("admin" or "member").
pub fn membership_role(inv: &InvitationRow) -> &'static str {
    if inv.role == "admin" { "admin" } else { "member" }
}

/// Accepts every pending invitation of `u` to `org`: adds the membership and the teams.
pub async fn accept(s: &AppState, org: &User, u: &User) -> ApiResult<Option<String>> {
    let invs = pending_for(s, u, Some(org.id)).await?;
    let Some(inv) = invs.first() else { return Ok(None) };
    let role = invs.iter().map(membership_role).max_by_key(|r| *r == "admin").unwrap_or("member");
    super::orgs_extra::set_org_membership(s, org, u, u, role).await?;
    for inv in &invs {
        for tid in &inv.team_ids {
            sqlx::query("INSERT INTO team_members (team_id, user_id, role) SELECT id, $2, 'member' FROM teams WHERE id = $1 AND org_id = $3 ON CONFLICT DO NOTHING")
                .bind(tid)
                .bind(u.id)
                .bind(org.id)
                .execute(&s.db)
                .await?;
        }
    }
    let ids: Vec<i64> = invs.iter().map(|i| i.id).collect();
    sqlx::query("DELETE FROM org_invitations WHERE id = ANY($1)").bind(&ids).execute(&s.db).await?;
    audit_quiet(&s.db, org, Some(u), "org.accept_invitation", json!({ "user": u.login, "invitation_id": inv.id })).await;
    Ok(Some(role.to_string()))
}

/// Declines (deletes) the pending invitations of `u` to `org`; false when there were none.
pub async fn decline(s: &AppState, org: &User, u: &User) -> ApiResult<bool> {
    let invs = pending_for(s, u, Some(org.id)).await?;
    if invs.is_empty() {
        return Ok(false);
    }
    let ids: Vec<i64> = invs.iter().map(|i| i.id).collect();
    sqlx::query("DELETE FROM org_invitations WHERE id = ANY($1)").bind(&ids).execute(&s.db).await?;
    audit_quiet(&s.db, org, Some(u), "org.decline_invitation", json!({ "user": u.login })).await;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Blocks

async fn list_blocks(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let users: Vec<User> = sqlx::query_as("SELECT u.* FROM org_blocks b JOIN users u ON u.id = b.user_id WHERE b.org_id = $1 ORDER BY b.created_at, u.id LIMIT $2 OFFSET $3")
        .bind(o.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = users.iter().map(|x| crate::json::user::simple(&s.api_url(), s.base_url(), x)).collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn check_block(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let t = db::user_by_login(&s.db, &login).await?;
    if is_blocked(&s.db, o.id, t.id).await? { Ok(no_content()) } else { Err(ApiError::not_found()) }
}

async fn block(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let t = db::user_by_login(&s.db, &login).await?;
    if t.kind != "User" || t.id == u.id {
        return Err(ApiError::unprocessable("You cannot block this user."));
    }
    if org_role(&s.db, o.id, t.id).await?.is_some() {
        return Err(ApiError::unprocessable("You cannot block a member of this organization. Remove them from the organization first."));
    }
    sqlx::query("INSERT INTO org_blocks (org_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(o.id).bind(t.id).execute(&s.db).await?;
    // their pending invitations fail
    sqlx::query("UPDATE org_invitations SET failed_at = now(), failed_reason = 'Invitee was blocked by the organization' WHERE org_id = $1 AND invitee_id = $2 AND failed_at IS NULL")
        .bind(o.id)
        .bind(t.id)
        .execute(&s.db)
        .await?;
    audit_quiet(&s.db, &o, Some(&u), "org.block_user", json!({ "user": t.login })).await;
    Ok(no_content())
}

async fn unblock(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let t = db::user_by_login(&s.db, &login).await?;
    let n = sqlx::query("DELETE FROM org_blocks WHERE org_id = $1 AND user_id = $2").bind(o.id).bind(t.id).execute(&s.db).await?;
    if n.rows_affected() > 0 {
        audit_quiet(&s.db, &o, Some(&u), "org.unblock_user", json!({ "user": t.login })).await;
    }
    Ok(no_content())
}
