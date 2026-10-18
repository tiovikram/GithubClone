//! Social features the web UI needs beyond the GHES REST surface.
//!
//! - `/api/v3/user/blocks[/{username}]`: blocking users, shaped like github.com's API. A blocked
//!   user cannot follow the blocker, open issues or comment in the blocker's repositories, and
//!   existing follows between the two are removed.
//! - `/api/forge/...` (forge-only, used by the web UI):
//!   - `GET|PUT /repos/{o}/{r}/subscription`: watch state incl. "custom" (subject kinds).
//!   - `GET|PUT|DELETE /repos/{o}/{r}/issues/{n}/subscription`: subscribe to one issue or PR
//!     without needing a notification thread first (the GitHub UI's Subscribe button).
//!   - `PUT /user/password`, `PATCH /user/login`, `DELETE /user`: account settings.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::{self, AuthUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/blocks", get(list_blocks))
        .route("/user/blocks/{username}", get(check_block).put(block).delete(unblock))
}

pub fn forge_routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/subscription", get(get_watch).put(put_watch))
        .route("/repos/{owner}/{repo}/issues/{number}/subscription", get(get_issue_sub).put(put_issue_sub).delete(delete_issue_sub))
        .route("/user/password", put(change_password))
        .route("/user/login", patch(rename_me))
        .route("/user", axum::routing::delete(delete_me))
}

// ---------------------------------------------------------------------------------------------
// Blocks

/// Whether `blocker` has blocked `user`.
pub async fn is_blocked(db: &sqlx::PgPool, blocker: i64, user: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_blocks WHERE user_id = $1 AND blocked_id = $2)")
        .bind(blocker)
        .bind(user)
        .fetch_one(db)
        .await?)
}

/// Fails with 403 when the owner of a repository has blocked `actor` (issues, comments).
pub async fn ensure_not_blocked_by_owner(s: &AppState, owner_id: i64, actor: &User) -> ApiResult<()> {
    if owner_id != actor.id && is_blocked(&s.db, owner_id, actor.id).await? {
        return Err(ApiError::forbidden("You can't perform that action at this time."));
    }
    Ok(())
}

async fn list_blocks(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let users: Vec<User> = sqlx::query_as(
        "SELECT u.* FROM user_blocks b JOIN users u ON u.id = b.blocked_id WHERE b.user_id = $1 ORDER BY b.created_at, u.id LIMIT $2 OFFSET $3",
    )
    .bind(u.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM user_blocks WHERE user_id = $1").bind(u.id).fetch_one(&s.db).await?;
    let items = users.iter().map(|x| crate::json::user::simple(&s.api_url(), s.base_url(), x)).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn check_block(State(s): State<AppState>, AuthUser(u): AuthUser, Path(login): Path<String>) -> ApiResult<Response> {
    let t = db::user_by_login(&s.db, &login).await?;
    if is_blocked(&s.db, u.id, t.id).await? {
        Ok(no_content())
    } else {
        Err(ApiError::not_found())
    }
}

async fn block(State(s): State<AppState>, AuthUser(u): AuthUser, Path(login): Path<String>) -> ApiResult<Response> {
    let t = db::user_by_login(&s.db, &login).await?;
    if t.id == u.id || t.kind != "User" {
        return Err(ApiError::unprocessable("You can't block that account."));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO user_blocks (user_id, blocked_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(u.id).bind(t.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM follows WHERE (follower_id = $1 AND followee_id = $2) OR (follower_id = $2 AND followee_id = $1)")
        .bind(u.id)
        .bind(t.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(no_content())
}

async fn unblock(State(s): State<AppState>, AuthUser(u): AuthUser, Path(login): Path<String>) -> ApiResult<Response> {
    let t = db::user_by_login(&s.db, &login).await?;
    sqlx::query("DELETE FROM user_blocks WHERE user_id = $1 AND blocked_id = $2").bind(u.id).bind(t.id).execute(&s.db).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Repository watching (with "custom")

const KINDS: [&str; 3] = ["issues", "pulls", "releases"];

async fn watch_json(s: &AppState, user_id: i64, repo_id: i64) -> ApiResult<Value> {
    let row: Option<(bool, bool, Option<Vec<String>>)> =
        sqlx::query_as("SELECT subscribed, ignored, custom_events FROM watches WHERE user_id = $1 AND repo_id = $2")
            .bind(user_id)
            .bind(repo_id)
            .fetch_optional(&s.db)
            .await?;
    let count = super::repos::subscribers_count(s, repo_id).await?;
    let (state, events) = match row {
        None => ("not_watching", vec![]),
        Some((_, true, _)) => ("ignoring", vec![]),
        Some((true, false, Some(ev))) => ("custom", ev),
        Some((true, false, None)) => ("watching", vec![]),
        Some((false, false, _)) => ("not_watching", vec![]),
    };
    Ok(json!({"state": state, "events": events, "subscribers_count": count}))
}

async fn get_watch(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    Ok(Json(watch_json(&s, u.id, ctx.repo.id).await?))
}

#[derive(Deserialize)]
struct WatchBody {
    state: String,
    #[serde(default)]
    events: Vec<String>,
}

async fn put_watch(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<WatchBody>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    match b.state.as_str() {
        "not_watching" => super::stars::delete_subscription(&s, &ctx, &u).await?,
        "watching" | "ignoring" | "custom" => {
            let ignoring = b.state == "ignoring";
            let custom: Option<Vec<String>> = if b.state == "custom" {
                if b.events.iter().any(|e| !KINDS.contains(&e.as_str())) {
                    return Err(ApiError::validation("Subscription", "events", "invalid"));
                }
                if b.events.is_empty() {
                    // Custom with nothing selected = participating and @mentions only.
                    super::stars::delete_subscription(&s, &ctx, &u).await?;
                    return Ok(Json(watch_json(&s, u.id, ctx.repo.id).await?));
                }
                Some(b.events.clone())
            } else {
                None
            };
            sqlx::query(
                "INSERT INTO watches (user_id, repo_id, subscribed, ignored, custom_events) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (user_id, repo_id) DO UPDATE SET subscribed = $3, ignored = $4, custom_events = $5",
            )
            .bind(u.id)
            .bind(ctx.repo.id)
            .bind(!ignoring)
            .bind(ignoring)
            .bind(custom)
            .execute(&s.db)
            .await?;
        }
        _ => return Err(ApiError::validation("Subscription", "state", "invalid")),
    }
    Ok(Json(watch_json(&s, u.id, ctx.repo.id).await?))
}

// ---------------------------------------------------------------------------------------------
// Issue / pull request subscriptions

async fn issue_sub_json(s: &AppState, u: &User, ctx: &db::RepoCtx, issue_id: i64) -> ApiResult<Value> {
    let key = format!("Issue:{issue_id}");
    let explicit: Option<(bool, bool)> = sqlx::query_as("SELECT subscribed, ignored FROM notification_subscriptions WHERE user_id = $1 AND subject_key = $2")
        .bind(u.id)
        .bind(&key)
        .fetch_optional(&s.db)
        .await?;
    let (subscribed, reason) = match explicit {
        Some((_, true)) => (false, "ignored"),
        Some((true, false)) => (true, "manual"),
        Some((false, false)) => (false, "unsubscribed"),
        None => {
            let (author, commented, assigned): (bool, bool, bool) = sqlx::query_as(
                "SELECT EXISTS(SELECT 1 FROM issues WHERE id = $1 AND user_id = $2),
                        EXISTS(SELECT 1 FROM issue_comments WHERE issue_id = $1 AND user_id = $2),
                        EXISTS(SELECT 1 FROM issue_assignees WHERE issue_id = $1 AND user_id = $2)",
            )
            .bind(issue_id)
            .bind(u.id)
            .fetch_one(&s.db)
            .await?;
            let watching: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM watches WHERE user_id = $1 AND repo_id = $2 AND subscribed AND NOT ignored AND custom_events IS NULL)")
                    .bind(u.id)
                    .bind(ctx.repo.id)
                    .fetch_one(&s.db)
                    .await?;
            if author {
                (true, "author")
            } else if assigned {
                (true, "assign")
            } else if commented {
                (true, "comment")
            } else if watching {
                (true, "subscribed")
            } else {
                (false, "not_subscribed")
            }
        }
    };
    Ok(json!({"subscribed": subscribed, "reason": reason}))
}

async fn issue_for(s: &AppState, u: &User, o: &str, r: &str, n: i64) -> ApiResult<(db::RepoCtx, i64)> {
    let ctx = db::load_repo(&s.db, Some(u), o, r).await?;
    let issue = super::issues::get_issue(s, &ctx, n).await?;
    Ok((ctx, issue.id))
}

async fn get_issue_sub(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let (ctx, id) = issue_for(&s, &u, &o, &r, n).await?;
    Ok(Json(issue_sub_json(&s, &u, &ctx, id).await?))
}

async fn set_issue_sub(s: &AppState, u: &User, ctx: &db::RepoCtx, issue_id: i64, subscribed: bool) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO notification_subscriptions (user_id, subject_key, repo_id, subscribed, ignored, reason) VALUES ($1,$2,$3,$4,FALSE,'manual')
         ON CONFLICT (user_id, subject_key) DO UPDATE SET subscribed = $4, ignored = FALSE",
    )
    .bind(u.id)
    .bind(format!("Issue:{issue_id}"))
    .bind(ctx.repo.id)
    .bind(subscribed)
    .execute(&s.db)
    .await?;
    Ok(())
}

async fn put_issue_sub(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let (ctx, id) = issue_for(&s, &u, &o, &r, n).await?;
    set_issue_sub(&s, &u, &ctx, id, true).await?;
    Ok(Json(issue_sub_json(&s, &u, &ctx, id).await?))
}

async fn delete_issue_sub(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let (ctx, id) = issue_for(&s, &u, &o, &r, n).await?;
    set_issue_sub(&s, &u, &ctx, id, false).await?;
    Ok(Json(issue_sub_json(&s, &u, &ctx, id).await?))
}

// ---------------------------------------------------------------------------------------------
// Account

#[derive(Deserialize)]
struct PasswordBody {
    current_password: String,
    password: String,
}

async fn check_password(u: &User, pw: &str) -> ApiResult<()> {
    let hash = u.password_hash.clone().unwrap_or_default();
    let pw = pw.to_string();
    let ok = tokio::task::spawn_blocking(move || !hash.is_empty() && auth::verify_password(&pw, &hash)).await.map_err(ApiError::internal)?;
    if !ok {
        return Err(ApiError::validation("User", "current_password", "invalid"));
    }
    Ok(())
}

async fn change_password(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<PasswordBody>) -> ApiResult<Response> {
    check_password(&u, &b.current_password).await?;
    if b.password.len() < 8 {
        return Err(ApiError::validation("User", "password", "invalid"));
    }
    let pw = b.password.clone();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&pw)).await.map_err(ApiError::internal)?.map_err(ApiError::internal)?;
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1").bind(u.id).bind(hash).execute(&s.db).await?;
    crate::enterprise::audit(&s, "user.change_password", Some(&u), json!({"user": u.login, "user_id": u.id})).await;
    Ok(no_content())
}

#[derive(Deserialize)]
struct LoginBody {
    login: String,
}

async fn rename_me(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<LoginBody>) -> ApiResult<Json<Value>> {
    let new = b.login.trim();
    let reserved = !crate::forge_api::valid_login(new);
    if reserved {
        return Err(ApiError::validation("User", "login", "invalid"));
    }
    let renamed = crate::enterprise::rename_user(&s, &u, &u, new).await?;
    Ok(Json(super::users::me_json(&s, &renamed).await?))
}

#[derive(Deserialize)]
struct DeleteBody {
    /// Must equal the account's login (GitHub's "type your username to confirm").
    login: String,
    password: String,
}

async fn delete_me(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<DeleteBody>) -> ApiResult<Response> {
    if !b.login.eq_ignore_ascii_case(&u.login) {
        return Err(ApiError::validation("User", "login", "invalid"));
    }
    check_password(&u, &b.password).await?;
    if u.site_admin {
        return Err(ApiError::forbidden("Site administrators can't delete their account. Ask another site administrator to demote you first."));
    }
    // Sole owner of an organization: GitHub refuses until ownership is transferred.
    let sole: Option<String> = sqlx::query_scalar(
        "SELECT o.login FROM org_members m JOIN users o ON o.id = m.org_id WHERE m.user_id = $1 AND m.role = 'admin'
           AND NOT EXISTS (SELECT 1 FROM org_members m2 WHERE m2.org_id = m.org_id AND m2.role = 'admin' AND m2.user_id <> $1) LIMIT 1",
    )
    .bind(u.id)
    .fetch_optional(&s.db)
    .await?;
    if let Some(org) = sole {
        return Err(ApiError::unprocessable(format!("You are the only owner of the {org} organization. Transfer ownership or delete it first.")));
    }
    sqlx::query("DELETE FROM users WHERE id = $1 AND kind = 'User'").bind(u.id).execute(&s.db).await?;
    crate::enterprise::audit(&s, "user.delete", Some(&u), json!({"user": u.login, "user_id": u.id})).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
