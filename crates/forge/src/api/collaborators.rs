//! Repository collaborators. forge adds collaborators directly (no invitation round-trip), which
//! matches GHES 3.22's documented `204` for `PUT /collaborators/{username}`; the invitations
//! list is therefore always empty.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::issues::{invalid, load_ctx};
use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/collaborators", get(list))
        .route("/repos/{owner}/{repo}/collaborators/{username}", get(check).put(add_handler).delete(remove_handler))
        .route("/repos/{owner}/{repo}/collaborators/{username}/permission", get(permission))
}

/// GitHub's `role_name` for a permission level.
pub fn role_name(p: Perm) -> &'static str {
    match p {
        Perm::Admin => "admin",
        Perm::Maintain => "maintain",
        Perm::Write => "write",
        Perm::Triage => "triage",
        Perm::Read => "read",
        Perm::None => "none",
    }
}

/// The legacy `permission` string of the permission-level endpoint (admin/write/read/none).
fn legacy_permission(p: Perm) -> &'static str {
    match p {
        Perm::Admin => "admin",
        Perm::Maintain | Perm::Write => "write",
        Perm::Triage | Perm::Read => "read",
        Perm::None => "none",
    }
}

/// `collaborator` schema.
pub fn collaborator_json(s: &AppState, u: &User, p: Perm) -> Value {
    let mut v = json::user::simple(&s.api_url(), s.base_url(), u);
    v["permissions"] = json!({
        "pull": p >= Perm::Read,
        "triage": p >= Perm::Triage,
        "push": p >= Perm::Write,
        "maintain": p >= Perm::Maintain,
        "admin": p >= Perm::Admin,
    });
    v["role_name"] = json!(role_name(p));
    v
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    Owner,
    OrgMember,
    Direct,
}

/// Everyone with explicit access to the repository, with their effective permission.
async fn all_collaborators(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<(User, Perm, Source)>> {
    let mut out: Vec<(User, Perm, Source)> = Vec::new();
    if ctx.owner.kind != "Organization" {
        out.push((ctx.owner.clone(), Perm::Admin, Source::Owner));
    } else {
        let members: Vec<(i64, String)> = sqlx::query_as("SELECT user_id, role FROM org_members WHERE org_id = $1")
            .bind(ctx.owner.id)
            .fetch_all(&s.db)
            .await?;
        let ids: Vec<i64> = members.iter().map(|m| m.0).collect();
        let users = json::issue::users_by_ids(&s.db, &ids).await?;
        // effective permission: owner, base permission, teams, org roles (crate::orgs::perm)
        let perms = crate::orgs::perm::org_repo_permissions(&s.db, ctx.owner.id, ctx.repo.id, &ids).await?;
        for (id, _role) in members {
            let mut p = perms.get(&id).copied().unwrap_or(Perm::None);
            if !ctx.repo.private {
                p = p.max(Perm::Read);
            }
            if p == Perm::None {
                continue;
            }
            if let Some(u) = users.get(&id) {
                out.push((u.clone(), p, Source::OrgMember));
            }
        }
    }
    let collabs: Vec<(i64, String)> = sqlx::query_as("SELECT user_id, permission FROM repo_collaborators WHERE repo_id = $1")
        .bind(ctx.repo.id)
        .fetch_all(&s.db)
        .await?;
    let ids: Vec<i64> = collabs.iter().map(|c| c.0).collect();
    let users = json::issue::users_by_ids(&s.db, &ids).await?;
    for (id, perm) in collabs {
        let p = match Perm::from_str(&perm) {
            Perm::None if ctx.owner.kind == "Organization" => {
                crate::orgs::perm::resolve_grant(&perm, &crate::orgs::perm::custom_roles(&s.db, ctx.owner.id).await?)
            }
            p => p,
        };
        if let Some(existing) = out.iter_mut().find(|e| e.0.id == id) {
            existing.1 = existing.1.max(p);
            continue;
        }
        if let Some(u) = users.get(&id) {
            out.push((u.clone(), p, Source::Direct));
        }
    }
    out.sort_by_key(|e| e.0.id);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Service layer.

fn parse_permission(p: Option<&str>) -> ApiResult<&'static str> {
    Ok(match p.unwrap_or("push") {
        "pull" | "read" => "pull",
        "triage" => "triage",
        "push" | "write" => "push",
        "maintain" => "maintain",
        "admin" => "admin",
        other => return Err(invalid("Repository", "permission", other)),
    })
}

/// Grants `user` access to the repository. Returns true when newly added.
pub async fn add_collaborator(state: &AppState, ctx: &RepoCtx, user: &User, permission: Option<&str>) -> ApiResult<bool> {
    ctx.require(Perm::Admin)?;
    let perm = parse_permission(permission)?;
    if user.id == ctx.owner.id {
        return Err(ApiError::unprocessable("Repository owner cannot be a collaborator"));
    }
    if user.kind == "Organization" {
        return Err(invalid("Repository", "username", user.login.as_str()));
    }
    let existed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM repo_collaborators WHERE repo_id = $1 AND user_id = $2)")
        .bind(ctx.repo.id)
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    sqlx::query(
        "INSERT INTO repo_collaborators (repo_id, user_id, permission) VALUES ($1, $2, $3)
         ON CONFLICT (repo_id, user_id) DO UPDATE SET permission = $3",
    )
    .bind(ctx.repo.id)
    .bind(user.id)
    .bind(perm)
    .execute(&state.db)
    .await?;
    Ok(!existed)
}

pub async fn remove_collaborator(state: &AppState, ctx: &RepoCtx, actor: &User, user: &User) -> ApiResult<()> {
    // Collaborators may remove themselves.
    if actor.id != user.id {
        ctx.require(Perm::Admin)?;
    }
    sqlx::query("DELETE FROM repo_collaborators WHERE repo_id = $1 AND user_id = $2")
        .bind(ctx.repo.id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    sqlx::query(
        "DELETE FROM issue_assignees a USING issues i WHERE a.issue_id = i.id AND i.repo_id = $1 AND a.user_id = $2
           AND NOT EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = $3 AND m.user_id = $2)",
    )
    .bind(ctx.repo.id)
    .bind(user.id)
    .bind(ctx.owner.id)
    .execute(&state.db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Handlers.

#[derive(Deserialize, Default)]
struct ListQuery {
    affiliation: Option<String>,
    permission: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    ctx.require(Perm::Write)?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let min = q.permission.as_deref().map(Perm::from_str);
    let all = all_collaborators(&s, &ctx).await?;
    let filtered: Vec<&(User, Perm, Source)> = all
        .iter()
        .filter(|(_, _, src)| match q.affiliation.as_deref() {
            Some("outside") => *src == Source::Direct && ctx.owner.kind == "Organization",
            Some("direct") => *src != Source::OrgMember,
            _ => true,
        })
        .filter(|(_, perm, _)| min.is_none_or(|m| *perm >= m))
        .collect();
    let total = filtered.len() as i64;
    let items = filtered
        .into_iter()
        .skip(p.offset() as usize)
        .take(p.per_page() as usize)
        .map(|(u, perm, _)| collaborator_json(&s, u, *perm))
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn check(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, username)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let all = all_collaborators(&s, &ctx).await?;
    if all.iter().any(|(c, _, _)| c.login.eq_ignore_ascii_case(&username)) {
        Ok(no_content())
    } else {
        Err(ApiError::not_found())
    }
}

#[derive(Deserialize, Default)]
struct AddBody {
    permission: Option<String>,
}

async fn add_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, username)): Path<(String, String, String)>,
    Json(b): Json<AddBody>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let target = db::user_by_login(&s.db, &username).await?;
    // github.com behaviour (opt-in): invite users who have no access yet (repos area).
    if ctx.perm >= Perm::Admin && target.kind == "User" && super::invitations::invitations_enabled(&s).await {
        let cur = db::permission(&s.db, Some(&target), &ctx.repo, &ctx.owner).await?;
        if cur < Perm::Triage && target.id != u.id {
            let inv = super::invitations::create_invitation(&s, &ctx, &u, &target, b.permission.as_deref().unwrap_or("push")).await?;
            let v = super::invitations::invitations_json(&s, &u, &[inv]).await?.remove(0);
            return Ok((axum::http::StatusCode::CREATED, axum::Json(v)).into_response());
        }
    }
    add_collaborator(&s, &ctx, &target, b.permission.as_deref()).await?;
    Ok(no_content())
}

async fn remove_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, username)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let target = db::user_by_login(&s.db, &username).await?;
    remove_collaborator(&s, &ctx, &u, &target).await?;
    Ok(no_content())
}

async fn permission(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, username)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let target = db::user_by_login(&s.db, &username).await?;
    let p = db::permission(&s.db, Some(&target), &ctx.repo, &ctx.owner).await?;
    Ok(Json(json!({
        "permission": legacy_permission(p),
        "role_name": role_name(p),
        "user": collaborator_json(&s, &target, p),
    })))
}

#[allow(dead_code)]
async fn invitations(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    Ok(Paged::new(&uri, &s, p, Some(0), vec![]))
}
