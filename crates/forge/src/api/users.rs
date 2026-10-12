//! Users and organizations (core).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use crate::extract::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user", get(get_me).patch(patch_me))
        .route("/user/emails", get(my_emails))
        .route("/user/orgs", get(my_orgs))
        .route("/users", get(list_users))
        .route("/users/{username}", get(get_user))
        .route("/users/{username}/orgs", get(user_orgs))
        .route("/orgs/{org}", get(get_org))
        .route("/orgs/{org}/members", get(org_members))
        .route("/orgs/{org}/members/{username}", get(org_member_check))
}

pub async fn repo_counts(state: &AppState, user_id: i64) -> ApiResult<(i64, i64)> {
    let (public, private): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE NOT private), count(*) FILTER (WHERE private)
           FROM repositories WHERE owner_id = $1",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    Ok((public, private))
}

async fn get_me(State(s): State<AppState>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let (public, private) = repo_counts(&s, u.id).await?;
    Ok(Json(json::user::private(&s.api_url(), s.base_url(), &u, public, private)))
}

#[derive(Deserialize)]
struct PatchMe {
    name: Option<String>,
    email: Option<String>,
    blog: Option<String>,
    twitter_username: Option<String>,
    company: Option<String>,
    location: Option<String>,
    bio: Option<String>,
}

async fn patch_me(State(s): State<AppState>, AuthUser(u): AuthUser, Json(p): Json<PatchMe>) -> ApiResult<Json<Value>> {
    let u: User = sqlx::query_as(
        "UPDATE users SET name = COALESCE($2, name), email = COALESCE($3, email), blog = COALESCE($4, blog),
            twitter_username = COALESCE($5, twitter_username), company = COALESCE($6, company),
            location = COALESCE($7, location), bio = COALESCE($8, bio), updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(u.id)
    .bind(p.name)
    .bind(p.email)
    .bind(p.blog)
    .bind(p.twitter_username)
    .bind(p.company)
    .bind(p.location)
    .bind(p.bio)
    .fetch_one(&s.db)
    .await?;
    let (public, private) = repo_counts(&s, u.id).await?;
    Ok(Json(json::user::private(&s.api_url(), s.base_url(), &u, public, private)))
}

async fn my_emails(AuthUser(u): AuthUser) -> Json<Value> {
    let email = u.email.unwrap_or_else(|| format!("{}@users.noreply.forge", u.login));
    Json(json!([{ "email": email, "primary": true, "verified": true, "visibility": "public" }]))
}

async fn orgs_of(s: &AppState, user_id: i64, p: PageParams) -> ApiResult<Vec<Value>> {
    let orgs: Vec<User> = sqlx::query_as(
        "SELECT u.* FROM org_members m JOIN users u ON u.id = m.org_id WHERE m.user_id = $1
          ORDER BY u.id LIMIT $2 OFFSET $3",
    )
    .bind(user_id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    Ok(orgs.iter().map(|o| json::user::org_simple(&s.api_url(), s.base_url(), o)).collect())
}

async fn my_orgs(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let items = orgs_of(&s, u.id, p).await?;
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn user_orgs(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    let items = orgs_of(&s, u.id, p).await?;
    Ok(Paged::new(&uri, &s, p, None, items))
}

#[derive(Deserialize)]
struct ListUsers {
    since: Option<i64>,
    per_page: Option<i64>,
}

async fn list_users(State(s): State<AppState>, uri: OriginalUri, Query(q): Query<ListUsers>) -> ApiResult<Response> {
    let per = q.per_page.unwrap_or(30).clamp(1, 100);
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id > $1 ORDER BY id LIMIT $2")
        .bind(q.since.unwrap_or(0))
        .bind(per)
        .fetch_all(&s.db)
        .await?;
    let items: Vec<Value> = users.iter().map(|u| json::user::simple(&s.api_url(), s.base_url(), u)).collect();
    let mut res = axum::response::IntoResponse::into_response(Json(Value::Array(items)));
    if users.len() as i64 == per {
        let next = format!(
            "<{}{}?per_page={per}&since={}>; rel=\"next\"",
            s.base_url(),
            uri.0.path(),
            users.last().unwrap().id
        );
        res.headers_mut().insert(axum::http::header::LINK, next.parse().unwrap());
    }
    Ok(res)
}

async fn get_user(State(s): State<AppState>, Path(login): Path<String>) -> ApiResult<Json<Value>> {
    let u = db::user_by_login(&s.db, &login).await?;
    if u.kind == "Organization" {
        return org_json(&s, &u).await.map(Json);
    }
    let (public, _) = repo_counts(&s, u.id).await?;
    Ok(Json(json::user::public(&s.api_url(), s.base_url(), &u, public, 0, 0)))
}

async fn org_json(s: &AppState, o: &User) -> ApiResult<Value> {
    let (public, private) = repo_counts(s, o.id).await?;
    let mut v = json::user::org_simple(&s.api_url(), s.base_url(), o);
    let m = v.as_object_mut().unwrap();
    for (k, val) in [
        ("name", json!(o.name.clone().unwrap_or_else(|| o.login.clone()))),
        ("company", json::opt_str(&o.company)),
        ("location", json::opt_str(&o.location)),
        ("email", json::opt_str(&o.email)),
        ("twitter_username", json::opt_str(&o.twitter_username)),
        ("is_verified", json!(false)),
        ("has_organization_projects", json!(true)),
        ("has_repository_projects", json!(true)),
        ("public_repos", json!(public)),
        ("public_gists", json!(0)),
        ("followers", json!(0)),
        ("following", json!(0)),
        ("html_url", json!(format!("{}/{}", s.base_url(), o.login))),
        ("created_at", json!(json::ts(o.created_at))),
        ("updated_at", json!(json::ts(o.updated_at))),
        ("type", json!("Organization")),
        ("total_private_repos", json!(private)),
        ("owned_private_repos", json!(private)),
        ("archived_at", Value::Null),
    ] {
        m.insert(k.into(), val);
    }
    if !o.blog.is_empty() {
        m.insert("blog".into(), json!(o.blog));
    }
    Ok(v)
}

async fn get_org(State(s): State<AppState>, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = db::user_by_login(&s.db, &org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    org_json(&s, &o).await.map(Json)
}

async fn org_members(State(s): State<AppState>, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = db::user_by_login(&s.db, &org).await?;
    let users: Vec<User> = sqlx::query_as(
        "SELECT u.* FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1
          ORDER BY u.id LIMIT $2 OFFSET $3",
    )
    .bind(o.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items = users.iter().map(|u| json::user::simple(&s.api_url(), s.base_url(), u)).collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn org_member_check(State(s): State<AppState>, Path((org, login)): Path<(String, String)>, _u: MaybeUser) -> ApiResult<Response> {
    let o = db::user_by_login(&s.db, &org).await?;
    let u = db::user_by_login(&s.db, &login).await?;
    let is: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)")
        .bind(o.id)
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if is {
        Ok(super::no_content())
    } else {
        Err(ApiError::not_found())
    }
}
