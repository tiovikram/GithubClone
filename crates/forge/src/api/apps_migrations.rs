//! Migrations API (user and organization exports). Service layer: `crate::migration`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;

use super::{created, no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::migration::{self, MigrationRow, StartMigration};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let o = "/orgs/{org}/migrations";
    let u = "/user/migrations";
    Router::new()
        .route(o, get(list_org).post(start_org))
        .route(&format!("{o}/{{migration_id}}"), get(get_org))
        .route(&format!("{o}/{{migration_id}}/archive"), get(archive_org).delete(delete_archive_org))
        .route(&format!("{o}/{{migration_id}}/repos/{{repo_name}}/lock"), delete(unlock_org))
        .route(&format!("{o}/{{migration_id}}/repositories"), get(repos_org))
        .route(u, get(list_user).post(start_user))
        .route(&format!("{u}/{{migration_id}}"), get(get_user))
        .route(&format!("{u}/{{migration_id}}/archive"), get(archive_user).delete(delete_archive_user))
        .route(&format!("{u}/{{migration_id}}/repos/{{repo_name}}/lock"), delete(unlock_user))
        .route(&format!("{u}/{{migration_id}}/repositories"), get(repos_user))
}

/// Forge-only download route (nested under /api/forge): the signed redirect target.
pub fn forge_routes() -> Router<AppState> {
    Router::new().route("/migrations/{migration_id}/archive", get(download))
}

#[derive(Deserialize, Default)]
struct ExcludeQuery {
    exclude: Option<String>,
}

fn with_repos(q: &ExcludeQuery) -> bool {
    !q.exclude.as_deref().is_some_and(|e| e.split(',').any(|x| x.trim() == "repositories"))
}

async fn org_owner(s: &AppState, u: &User, org: &str) -> ApiResult<User> {
    let o = crate::orgs::org_by_login(s, org).await?;
    crate::orgs::require_owner(s, &o, u).await?;
    Ok(o)
}

fn user_owner(u: &User) -> ApiResult<User> {
    if u.kind != "User" {
        return Err(ApiError::forbidden(crate::apps::scope::NOT_ACCESSIBLE));
    }
    Ok(u.clone())
}

async fn list(s: &AppState, uri: &OriginalUri, owner: &User, viewer: &User, p: PageParams, q: &ExcludeQuery) -> ApiResult<Paged> {
    let rows: Vec<MigrationRow> = sqlx::query_as("SELECT * FROM migrations WHERE owner_id = $1 ORDER BY id DESC LIMIT $2 OFFSET $3")
        .bind(owner.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM migrations WHERE owner_id = $1").bind(owner.id).fetch_one(&s.db).await?;
    let mut items = Vec::with_capacity(rows.len());
    for m in &rows {
        items.push(migration::migration_json(s, m, owner, viewer, with_repos(q)).await?);
    }
    Ok(Paged::new(uri, s, p, Some(total), items))
}

async fn start(s: &AppState, actor: &User, owner: &User, b: StartMigration) -> ApiResult<Response> {
    let m = migration::start(s, actor, owner, b).await?;
    Ok(created(migration::migration_json(s, &m, owner, actor, true).await?))
}

async fn archive(s: &AppState, m: &MigrationRow) -> ApiResult<Response> {
    if m.state != "exported" || m.archive_key.is_none() {
        return Err(ApiError::not_found());
    }
    let loc = migration::signed_archive_url(s, m).await?;
    let mut r = StatusCode::FOUND.into_response();
    r.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&loc).map_err(ApiError::internal)?);
    Ok(r)
}

async fn repos(s: &AppState, uri: &OriginalUri, m: &MigrationRow, viewer: &User, p: PageParams) -> ApiResult<Paged> {
    let ids = migration::repo_ids(&s.db, m).await?;
    let total = ids.len() as i64;
    let page: Vec<i64> = ids.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    let api = s.api_url();
    let mut items = Vec::new();
    for ctx in crate::apps::repos_by_ids(&s.db, &page, Perm::None).await? {
        let perm = db::permission(&s.db, Some(viewer), &ctx.repo, &ctx.owner).await?;
        items.push(crate::json::repo::repo(&api, s.base_url(), &RepoCtx { perm, ..ctx }));
    }
    Ok(Paged::new(uri, s, p, Some(total), items))
}

// ---- organization ----

async fn list_org(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>, Query(q): Query<ExcludeQuery>) -> ApiResult<Paged> {
    let o = org_owner(&s, &u, &org).await?;
    list(&s, &uri, &o, &u, p, &q).await
}

async fn start_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<StartMigration>) -> ApiResult<Response> {
    let o = org_owner(&s, &u, &org).await?;
    start(&s, &u, &o, b).await
}

async fn get_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Query(q): Query<ExcludeQuery>) -> ApiResult<Json<Value>> {
    let o = org_owner(&s, &u, &org).await?;
    let m = migration::by_id(&s.db, &o, id).await?;
    Ok(Json(migration::migration_json(&s, &m, &o, &u, with_repos(&q)).await?))
}

async fn archive_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_owner(&s, &u, &org).await?;
    archive(&s, &migration::by_id(&s.db, &o, id).await?).await
}

async fn delete_archive_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_owner(&s, &u, &org).await?;
    migration::delete_archive(&s, &migration::by_id(&s.db, &o, id).await?).await?;
    Ok(no_content())
}

async fn unlock_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id, repo)): Path<(String, i64, String)>) -> ApiResult<Response> {
    let o = org_owner(&s, &u, &org).await?;
    migration::unlock(&s, &migration::by_id(&s.db, &o, id).await?, &repo).await?;
    Ok(no_content())
}

async fn repos_org(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = org_owner(&s, &u, &org).await?;
    let m = migration::by_id(&s.db, &o, id).await?;
    repos(&s, &uri, &m, &u, p).await
}

// ---- authenticated user ----

async fn list_user(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>, Query(q): Query<ExcludeQuery>) -> ApiResult<Paged> {
    let o = user_owner(&u)?;
    list(&s, &uri, &o, &u, p, &q).await
}

async fn start_user(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<StartMigration>) -> ApiResult<Response> {
    let o = user_owner(&u)?;
    start(&s, &u, &o, b).await
}

async fn get_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>, Query(q): Query<ExcludeQuery>) -> ApiResult<Json<Value>> {
    let o = user_owner(&u)?;
    let m = migration::by_id(&s.db, &o, id).await?;
    Ok(Json(migration::migration_json(&s, &m, &o, &u, with_repos(&q)).await?))
}

async fn archive_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let o = user_owner(&u)?;
    archive(&s, &migration::by_id(&s.db, &o, id).await?).await
}

async fn delete_archive_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<i64>) -> ApiResult<Response> {
    let o = user_owner(&u)?;
    migration::delete_archive(&s, &migration::by_id(&s.db, &o, id).await?).await?;
    Ok(no_content())
}

async fn unlock_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, repo)): Path<(i64, String)>) -> ApiResult<Response> {
    let o = user_owner(&u)?;
    migration::unlock(&s, &migration::by_id(&s.db, &o, id).await?, &repo).await?;
    Ok(no_content())
}

async fn repos_user(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(id): Path<i64>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = user_owner(&u)?;
    let m = migration::by_id(&s.db, &o, id).await?;
    repos(&s, &uri, &m, &u, p).await
}

// ---- signed download ----

#[derive(Deserialize, Default)]
struct TokenQuery {
    token: Option<String>,
}

async fn download(State(s): State<AppState>, Path(id): Path<i64>, Query(q): Query<TokenQuery>) -> Response {
    let ok = match q.token.as_deref() {
        Some(t) => migration::verify_archive_token(&s, id, t).await,
        None => false,
    };
    if !ok {
        return ApiError::not_found().into_response();
    }
    match migration::stream_archive(&s, id).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}
