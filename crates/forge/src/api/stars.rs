//! Starring and watching (repository subscriptions). `stargazers_count` is kept in sync by a
//! trigger on `stars` (migration 0200).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::issues::{load_ctx, page_window, repo_ctxs};
use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/starred", get(list_starred_mine))
        .route("/user/starred/{owner}/{repo}", get(check_starred).put(star_handler).delete(unstar_handler))
        .route("/users/{username}/starred", get(list_starred_user))
        .route("/repos/{owner}/{repo}/stargazers", get(stargazers))
        .route(
            "/repos/{owner}/{repo}/subscription",
            get(get_subscription).put(set_subscription_handler).delete(delete_subscription_handler),
        )
        .route("/repos/{owner}/{repo}/subscribers", get(subscribers))
        .route("/user/subscriptions", get(list_watched_mine))
        .route("/users/{username}/subscriptions", get(list_watched_user))
}

fn wants_star_json(h: &HeaderMap) -> bool {
    h.get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("star+json"))
}

// ---------------------------------------------------------------------------------------------
// Service layer.

pub async fn star(state: &AppState, ctx: &RepoCtx, actor: &User) -> ApiResult<()> {
    sqlx::query("INSERT INTO stars (user_id, repo_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(actor.id)
        .bind(ctx.repo.id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn unstar(state: &AppState, ctx: &RepoCtx, actor: &User) -> ApiResult<()> {
    sqlx::query("DELETE FROM stars WHERE user_id = $1 AND repo_id = $2")
        .bind(actor.id)
        .bind(ctx.repo.id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn is_starred(state: &AppState, repo_id: i64, user_id: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM stars WHERE user_id = $1 AND repo_id = $2)")
        .bind(user_id)
        .bind(repo_id)
        .fetch_one(&state.db)
        .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WatchRow {
    pub user_id: i64,
    pub repo_id: i64,
    pub created_at: DateTime<Utc>,
    pub subscribed: bool,
    pub ignored: bool,
}

/// Sets the actor's subscription. `ignored` mutes notifications; both false removes the row.
pub async fn set_subscription(state: &AppState, ctx: &RepoCtx, actor: &User, subscribed: bool, ignored: bool) -> ApiResult<Option<WatchRow>> {
    if !subscribed && !ignored {
        delete_subscription(state, ctx, actor).await?;
        return Ok(None);
    }
    Ok(Some(
        sqlx::query_as(
            "INSERT INTO watches (user_id, repo_id, subscribed, ignored) VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, repo_id) DO UPDATE SET subscribed = $3, ignored = $4, custom_events = NULL
             RETURNING user_id, repo_id, created_at, subscribed, ignored",
        )
        .bind(actor.id)
        .bind(ctx.repo.id)
        .bind(subscribed)
        .bind(ignored)
        .fetch_one(&state.db)
        .await?,
    ))
}

pub async fn delete_subscription(state: &AppState, ctx: &RepoCtx, actor: &User) -> ApiResult<()> {
    sqlx::query("DELETE FROM watches WHERE user_id = $1 AND repo_id = $2")
        .bind(actor.id)
        .bind(ctx.repo.id)
        .execute(&state.db)
        .await?;
    Ok(())
}

fn subscription_json(s: &AppState, ctx: &RepoCtx, w: &WatchRow) -> Value {
    let repo_url = format!("{}/repos/{}", s.api_url(), ctx.full_name());
    json!({
        "subscribed": w.subscribed,
        "ignored": w.ignored,
        "reason": null,
        "created_at": json::ts(w.created_at),
        "url": format!("{repo_url}/subscription"),
        "repository_url": repo_url,
    })
}

// ---------------------------------------------------------------------------------------------
// Handlers.

async fn star_handler(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    star(&s, &ctx, &u).await?;
    Ok(no_content())
}

async fn unstar_handler(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    unstar(&s, &ctx, &u).await?;
    Ok(no_content())
}

async fn check_starred(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    if is_starred(&s, ctx.repo.id, u.id).await? { Ok(no_content()) } else { Err(ApiError::not_found()) }
}

#[derive(Deserialize, Default)]
struct StarredQuery {
    sort: Option<String>,
    direction: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// Repositories starred (or watched) by `user_id` and visible to `viewer`.
async fn user_repos(
    s: &AppState,
    uri: &OriginalUri,
    viewer: Option<&User>,
    user_id: i64,
    table: &str,
    q: &StarredQuery,
    star_json: bool,
) -> ApiResult<Paged> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let dir = if q.direction.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let col = if q.sort.as_deref() == Some("updated") { "r.updated_at" } else { "t.created_at" };
    let extra = if table == "watches" { " AND t.subscribed" } else { "" };
    let sql = format!(
        "SELECT r.id, t.created_at FROM {table} t JOIN repositories r ON r.id = t.repo_id
          WHERE t.user_id = $1{extra} AND forge_repo_readable(r.id, $2)
          ORDER BY {col} {dir}, r.id {dir} LIMIT $3 OFFSET $4"
    );
    let rows: Vec<(i64, DateTime<Utc>)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .bind(viewer.map(|v| v.id).unwrap_or(-1))
        .bind(p.limit() + 1)
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let (rows, total) = page_window(rows, &p);
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let ctxs = repo_ctxs(s, viewer, &ids).await?;
    let items = rows
        .iter()
        .filter_map(|(id, at)| {
            let ctx = ctxs.get(id).filter(|c| c.perm >= Perm::Read)?;
            let repo = json::repo::repo(&s.api_url(), s.base_url(), ctx);
            Some(if star_json { json!({ "starred_at": json::ts(*at), "repo": repo }) } else { repo })
        })
        .collect();
    Ok(Paged::new(uri, s, p, total, items))
}

async fn list_starred_mine(
    State(s): State<AppState>,
    uri: OriginalUri,
    headers: HeaderMap,
    AuthUser(u): AuthUser,
    Query(q): Query<StarredQuery>,
) -> ApiResult<Paged> {
    user_repos(&s, &uri, Some(&u), u.id, "stars", &q, wants_star_json(&headers)).await
}

async fn list_starred_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    headers: HeaderMap,
    MaybeUser(v): MaybeUser,
    Path(login): Path<String>,
    Query(q): Query<StarredQuery>,
) -> ApiResult<Paged> {
    let target = db::user_by_login(&s.db, &login).await?;
    user_repos(&s, &uri, v.as_ref(), target.id, "stars", &q, wants_star_json(&headers)).await
}

async fn list_watched_mine(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<StarredQuery>) -> ApiResult<Paged> {
    user_repos(&s, &uri, Some(&u), u.id, "watches", &q, false).await
}

async fn list_watched_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(v): MaybeUser,
    Path(login): Path<String>,
    Query(q): Query<StarredQuery>,
) -> ApiResult<Paged> {
    let target = db::user_by_login(&s.db, &login).await?;
    user_repos(&s, &uri, v.as_ref(), target.id, "watches", &q, false).await
}

async fn stargazers(
    State(s): State<AppState>,
    uri: OriginalUri,
    headers: HeaderMap,
    MaybeUser(v): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, v.as_ref(), &owner, &repo).await?;
    let rows: Vec<(i64, DateTime<Utc>)> =
        sqlx::query_as("SELECT user_id, created_at FROM stars WHERE repo_id = $1 ORDER BY created_at, user_id LIMIT $2 OFFSET $3")
            .bind(ctx.repo.id)
            .bind(p.limit() + 1)
            .bind(p.offset())
            .fetch_all(&s.db)
            .await?;
    let (rows, total) = page_window(rows, &p);
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let users = json::issue::users_by_ids(&s.db, &ids).await?;
    let star_json = wants_star_json(&headers);
    let items = rows
        .iter()
        .filter_map(|(id, at)| {
            let u = json::user::simple(&s.api_url(), s.base_url(), users.get(id)?);
            Some(if star_json { json!({ "starred_at": json::ts(*at), "user": u }) } else { u })
        })
        .collect();
    Ok(Paged::new(&uri, &s, p, total, items))
}

async fn subscribers(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(v): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, v.as_ref(), &owner, &repo).await?;
    let users: Vec<User> = sqlx::query_as(
        "SELECT u.* FROM watches w JOIN users u ON u.id = w.user_id WHERE w.repo_id = $1 AND w.subscribed
          ORDER BY w.created_at, u.id LIMIT $2 OFFSET $3",
    )
    .bind(ctx.repo.id)
    .bind(p.limit() + 1)
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let (users, total) = page_window(users, &p);
    let items = users.iter().map(|u| json::user::simple(&s.api_url(), s.base_url(), u)).collect();
    Ok(Paged::new(&uri, &s, p, total, items))
}

async fn get_subscription(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let w: WatchRow = sqlx::query_as("SELECT user_id, repo_id, created_at, subscribed, ignored FROM watches WHERE user_id = $1 AND repo_id = $2")
        .bind(u.id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(subscription_json(&s, &ctx, &w)))
}

#[derive(Deserialize, Default)]
struct SubscriptionBody {
    subscribed: Option<bool>,
    ignored: Option<bool>,
}

async fn set_subscription_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<SubscriptionBody>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let ignored = b.ignored.unwrap_or(false);
    let subscribed = b.subscribed.unwrap_or(!ignored);
    let w = set_subscription(&s, &ctx, &u, subscribed, ignored).await?;
    let w = w.unwrap_or(WatchRow { user_id: u.id, repo_id: ctx.repo.id, created_at: Utc::now(), subscribed: false, ignored: false });
    Ok(Json(subscription_json(&s, &ctx, &w)))
}

async fn delete_subscription_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    delete_subscription(&s, &ctx, &u).await?;
    Ok(no_content())
}
