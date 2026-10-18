//! Repository invitations: `/repos/{o}/{r}/invitations[/{id}]` (admin) and
//! `/user/repository_invitations[/{id}]` (invitee accepts / declines).
//!
//! GHES adds collaborators directly (`PUT /collaborators/{u}` answers 204), which is forge's
//! default. With the site setting `repository_invitations.enabled` (manage API
//! `/manage/v1/config/settings`; default `FORGE_REPO_INVITATIONS=1`), adding a user who is not yet a collaborator creates
//! an invitation instead (github.com behaviour, 201 with the invitation); accepting it makes the
//! user a collaborator. Invitations expire after 7 days.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/invitations", get(list_repo))
        .route("/repos/{owner}/{repo}/invitations/{id}", patch(update).delete(delete))
        .route("/user/repository_invitations", get(list_mine))
        .route("/user/repository_invitations/{id}", patch(accept).delete(decline))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InvitationRow {
    pub id: i64,
    pub repo_id: i64,
    pub invitee_id: i64,
    pub inviter_id: Option<i64>,
    pub permission: String,
    pub created_at: DateTime<Utc>,
}

/// Runtime site setting (`/manage/v1/config/settings` `repository_invitations.enabled`,
/// default `FORGE_REPO_INVITATIONS`).
pub async fn invitations_enabled(s: &AppState) -> bool {
    crate::enterprise::repo_invitations_enabled(&s.db).await
}

fn expired(i: &InvitationRow) -> bool {
    i.created_at < Utc::now() - chrono::Duration::days(7)
}

/// Stored permission (`pull`/`push`/...) -> invitation `permissions` value.
fn public_perm(p: &str) -> &'static str {
    match p {
        "pull" | "read" => "read",
        "triage" => "triage",
        "maintain" => "maintain",
        "admin" => "admin",
        _ => "write",
    }
}

fn stored_perm(p: &str) -> Option<&'static str> {
    Some(match p {
        "read" | "pull" => "pull",
        "triage" => "triage",
        "write" | "push" => "push",
        "maintain" => "maintain",
        "admin" => "admin",
        _ => return None,
    })
}

pub async fn invitations_json(s: &AppState, viewer: &User, rows: &[InvitationRow]) -> ApiResult<Vec<Value>> {
    let mut ids: Vec<i64> = rows.iter().map(|r| r.invitee_id).collect();
    ids.extend(rows.iter().filter_map(|r| r.inviter_id));
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let api = s.api_url();
    let mut out = Vec::new();
    for r in rows {
        let repo = db::repo_by_id(&s.db, r.repo_id).await?;
        let owner = db::user_by_id(&s.db, repo.owner_id).await?;
        let perm = db::permission(&s.db, Some(viewer), &repo, &owner).await?;
        let ctx = RepoCtx { repo, owner, perm: perm.max(Perm::Read) };
        let user = |id: Option<i64>| id.and_then(|i| users.get(&i)).map(|u| crate::json::user::simple(&api, s.base_url(), u)).unwrap_or(Value::Null);
        out.push(json!({
            "id": r.id,
            "node_id": crate::ids::encode("RepositoryInvitation", r.id),
            "repository": crate::json::repo::repo(&api, s.base_url(), &ctx),
            "invitee": user(Some(r.invitee_id)),
            "inviter": user(r.inviter_id),
            "permissions": public_perm(&r.permission),
            "created_at": crate::json::ts(r.created_at),
            "expired": expired(r),
            "url": format!("{api}/user/repository_invitations/{}", r.id),
            "html_url": format!("{}/{}/invitations", s.base_url(), ctx.full_name()),
        }));
    }
    Ok(out)
}

/// Invites `invitee` to the repository (admin). Re-inviting refreshes the pending invitation.
pub async fn create_invitation(s: &AppState, ctx: &RepoCtx, inviter: &User, invitee: &User, permission: &str) -> ApiResult<InvitationRow> {
    ctx.require(Perm::Admin)?;
    let perm = stored_perm(permission).ok_or_else(|| ApiError::validation("RepositoryInvitation", "permissions", "invalid"))?;
    let row: InvitationRow = sqlx::query_as(
        "INSERT INTO repo_invitations (repo_id, invitee_id, inviter_id, permission) VALUES ($1,$2,$3,$4)
         ON CONFLICT (repo_id, invitee_id) DO UPDATE SET permission = $4, inviter_id = $3, created_at = now() RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(invitee.id)
    .bind(inviter.id)
    .bind(perm)
    .fetch_one(&s.db)
    .await?;
    let url = format!("{}/user/repository_invitations/{}", s.api_url(), row.id);
    let title = format!("Invitation to join {} from {}", ctx.full_name(), inviter.login);
    super::notifications::notify_user(s, invitee.id, ctx, &format!("RepositoryInvitation:{}", row.id), "RepositoryInvitation", &title, Some(&url), "invitation").await?;
    Ok(row)
}

/// Accepts an invitation: the invitee becomes a collaborator with the invited permission.
pub async fn accept_invitation(s: &AppState, invitee: &User, id: i64) -> ApiResult<()> {
    let inv = mine(s, invitee, id).await?;
    if expired(&inv) {
        return Err(ApiError::not_found());
    }
    let mut tx = s.db.begin().await?;
    sqlx::query(
        "INSERT INTO repo_collaborators (repo_id, user_id, permission) VALUES ($1,$2,$3)
         ON CONFLICT (repo_id, user_id) DO UPDATE SET permission = $3",
    )
    .bind(inv.repo_id)
    .bind(invitee.id)
    .bind(&inv.permission)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM repo_invitations WHERE id = $1").bind(inv.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn mine(s: &AppState, u: &User, id: i64) -> ApiResult<InvitationRow> {
    sqlx::query_as("SELECT * FROM repo_invitations WHERE id = $1 AND invitee_id = $2").bind(id).bind(u.id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

async fn of_repo(s: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<InvitationRow> {
    sqlx::query_as("SELECT * FROM repo_invitations WHERE id = $1 AND repo_id = $2").bind(id).bind(ctx.repo.id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

async fn admin_ctx(s: &AppState, u: &User, o: &str, r: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), o, r).await?;
    ctx.require(Perm::Admin)?;
    Ok(ctx)
}

async fn list_repo(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    let rows: Vec<InvitationRow> = sqlx::query_as("SELECT * FROM repo_invitations WHERE repo_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(ctx.repo.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM repo_invitations WHERE repo_id = $1").bind(ctx.repo.id).fetch_one(&s.db).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), invitations_json(&s, &u, &rows).await?))
}

#[derive(Deserialize)]
struct UpdateBody {
    permissions: Option<String>,
}

async fn update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>, Json(b): Json<UpdateBody>) -> ApiResult<Json<Value>> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    let inv = of_repo(&s, &ctx, id).await?;
    let row = match b.permissions {
        Some(p) => {
            let perm = stored_perm(&p).ok_or_else(|| ApiError::validation("RepositoryInvitation", "permissions", "invalid"))?;
            sqlx::query_as("UPDATE repo_invitations SET permission = $2 WHERE id = $1 RETURNING *").bind(inv.id).bind(perm).fetch_one(&s.db).await?
        }
        None => inv,
    };
    Ok(Json(invitations_json(&s, &u, &[row]).await?.remove(0)))
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    let inv = of_repo(&s, &ctx, id).await?;
    sqlx::query("DELETE FROM repo_invitations WHERE id = $1").bind(inv.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn list_mine(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let rows: Vec<InvitationRow> = sqlx::query_as("SELECT * FROM repo_invitations WHERE invitee_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(u.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM repo_invitations WHERE invitee_id = $1").bind(u.id).fetch_one(&s.db).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), invitations_json(&s, &u, &rows).await?))
}

async fn accept(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    accept_invitation(&s, &u, id).await?;
    Ok(no_content())
}

async fn decline(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let inv = mine(&s, &u, id).await?;
    sqlx::query("DELETE FROM repo_invitations WHERE id = $1").bind(inv.id).execute(&s.db).await?;
    Ok(no_content())
}
