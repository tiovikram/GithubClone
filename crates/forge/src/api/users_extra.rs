//! User endpoints: email addresses (`/user/emails`, `/user/public_emails`), social accounts,
//! `GET /user/{account_id}` and hovercards (`/users/{username}/hovercard`).
//!
//! The primary email is `users.email` (or the `@users.noreply` address when unset); extra
//! addresses live in `user_emails`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
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
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/emails", get(list_emails).post(add_emails).delete(delete_emails))
        .route("/user/public_emails", get(public_emails))
        .route("/user/social_accounts", get(my_social).post(add_social).delete(delete_social))
        .route("/users/{username}/social_accounts", get(user_social))
        .route("/user/{account_id}", get(get_by_id))
        .route("/users/{username}/hovercard", get(hovercard))
}

// ---------------------------------------------------------------------------------------------
// Emails

pub fn primary_email(u: &User) -> String {
    u.email.clone().filter(|e| !e.is_empty()).unwrap_or_else(|| format!("{}@users.noreply.forge", u.login))
}

/// All addresses of a user: `(email, primary, verified)`, primary first.
pub async fn emails_of(s: &AppState, u: &User) -> ApiResult<Vec<(String, bool, bool)>> {
    let primary = primary_email(u);
    let mut out = vec![(primary.clone(), true, true)];
    let rows: Vec<(String, bool)> = sqlx::query_as("SELECT email, verified FROM user_emails WHERE user_id = $1 ORDER BY id").bind(u.id).fetch_all(&s.db).await?;
    for (e, v) in rows {
        if !e.eq_ignore_ascii_case(&primary) {
            out.push((e, false, v));
        }
    }
    Ok(out)
}

fn email_json(e: &str, primary: bool, verified: bool) -> Value {
    json!({"email": e, "primary": primary, "verified": verified, "visibility": if primary { json!("public") } else { Value::Null }})
}

/// `{emails: [..]}`, `[..]` or `".."`.
fn email_list(body: &Value) -> ApiResult<Vec<String>> {
    let arr: Vec<Value> = match body {
        Value::Object(o) => match o.get("emails") {
            Some(Value::Array(a)) => a.clone(),
            _ => return Err(ApiError::validation("User", "emails", "missing_field")),
        },
        Value::Array(a) => a.clone(),
        Value::String(s) => vec![Value::String(s.clone())],
        _ => return Err(ApiError::validation("User", "emails", "invalid")),
    };
    if arr.is_empty() {
        return Err(ApiError::validation("User", "emails", "missing_field"));
    }
    arr.into_iter().map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or_else(|| ApiError::validation("User", "emails", "invalid"))).collect()
}

fn valid_email(e: &str) -> bool {
    let Some((local, domain)) = e.rsplit_once('@') else { return false };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && e.len() <= 254 && !e.chars().any(|c| c.is_whitespace() || c == ',')
}

/// Adds email addresses to the user's account; returns the added addresses.
pub async fn add_emails_svc(s: &AppState, u: &User, emails: Vec<String>) -> ApiResult<Vec<Value>> {
    for e in &emails {
        if !valid_email(e) {
            let mut err = ApiError::validation("User", "email", "invalid");
            err.errors = Some(json!([{"resource": "User", "field": "email", "code": "invalid", "value": e}]));
            return Err(err);
        }
    }
    let mut tx = s.db.begin().await?;
    let primary = primary_email(u);
    for e in &emails {
        let taken: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE lower(email) = lower($1) AND id <> $2 UNION ALL SELECT user_id FROM user_emails WHERE lower(email) = lower($1) LIMIT 1",
        )
        .bind(e)
        .bind(u.id)
        .fetch_optional(&mut *tx)
        .await?;
        if taken.is_some() || e.eq_ignore_ascii_case(&primary) {
            let mut err = ApiError::validation("User", "email", "already_exists");
            err.errors = Some(json!([{"resource": "User", "field": "email", "code": "custom", "message": "email is already in use"}]));
            return Err(err);
        }
        sqlx::query("INSERT INTO user_emails (user_id, email) VALUES ($1, $2)").bind(u.id).bind(e).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(emails.iter().map(|e| email_json(e, false, true)).collect())
}

pub async fn delete_emails_svc(s: &AppState, u: &User, emails: Vec<String>) -> ApiResult<()> {
    let primary = primary_email(u);
    if emails.iter().any(|e| e.eq_ignore_ascii_case(&primary)) {
        let mut err = ApiError::unprocessable("Validation Failed");
        err.errors = Some(json!([{"resource": "User", "field": "email", "code": "custom", "message": "Cannot delete your primary email address"}]));
        return Err(err);
    }
    let lower: Vec<String> = emails.iter().map(|e| e.to_lowercase()).collect();
    let n = sqlx::query("DELETE FROM user_emails WHERE user_id = $1 AND lower(email) = ANY($2)").bind(u.id).bind(&lower).execute(&s.db).await?.rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn list_emails(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let all = emails_of(&s, &u).await?;
    let total = all.len() as i64;
    let items = all.iter().skip(p.offset() as usize).take(p.limit() as usize).map(|(e, pr, v)| email_json(e, *pr, *v)).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn add_emails(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<Value>) -> ApiResult<Response> {
    let added = add_emails_svc(&s, &u, email_list(&b)?).await?;
    Ok((StatusCode::CREATED, axum::Json(Value::Array(added))).into_response())
}

async fn delete_emails(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<Value>) -> ApiResult<Response> {
    delete_emails_svc(&s, &u, email_list(&b)?).await?;
    Ok(no_content())
}

async fn public_emails(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let items = if p.page() == 1 { vec![email_json(&primary_email(&u), true, true)] } else { vec![] };
    Ok(Paged::new(&uri, &s, p, Some(1), items))
}

// ---------------------------------------------------------------------------------------------
// Social accounts

/// Provider name GitHub reports for a profile URL.
pub fn provider_of(url: &str) -> &'static str {
    let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_lowercase())).unwrap_or_default();
    match host.as_str() {
        "twitter.com" | "x.com" => "twitter",
        "facebook.com" => "facebook",
        "linkedin.com" => "linkedin",
        "instagram.com" => "instagram",
        "youtube.com" => "youtube",
        "reddit.com" => "reddit",
        "twitch.tv" => "twitch",
        "npmjs.com" => "npm",
        "bsky.app" => "bluesky",
        _ if url.contains("/@") => "mastodon",
        _ => "generic",
    }
}

const MAX_SOCIAL: i64 = 4;

#[derive(Deserialize)]
struct SocialBody {
    account_urls: Option<Vec<String>>,
}

pub async fn social_accounts(s: &AppState, user_id: i64) -> ApiResult<Vec<Value>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT provider, url FROM social_accounts WHERE user_id = $1 ORDER BY id").bind(user_id).fetch_all(&s.db).await?;
    Ok(rows.into_iter().map(|(p, u)| json!({"provider": p, "url": u})).collect())
}

pub async fn add_social_svc(s: &AppState, u: &User, urls: Vec<String>) -> ApiResult<Vec<Value>> {
    let mut clean = Vec::new();
    for raw in urls {
        let url = raw.trim().to_string();
        let ok = url::Url::parse(&url).is_ok_and(|x| matches!(x.scheme(), "http" | "https") && x.host_str().is_some());
        if !ok {
            let mut e = ApiError::validation("SocialAccount", "account_urls", "invalid");
            e.errors = Some(json!([{"resource": "SocialAccount", "field": "account_urls", "code": "invalid", "value": url}]));
            return Err(e);
        }
        if !clean.contains(&url) {
            clean.push(url);
        }
    }
    let mut tx = s.db.begin().await?;
    let mut added = Vec::new();
    for url in &clean {
        let p = provider_of(url);
        let n = sqlx::query("INSERT INTO social_accounts (user_id, provider, url) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(u.id)
            .bind(p)
            .bind(url)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n > 0 {
            added.push(json!({"provider": p, "url": url}));
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM social_accounts WHERE user_id = $1").bind(u.id).fetch_one(&mut *tx).await?;
    if count > MAX_SOCIAL {
        let mut e = ApiError::unprocessable("Validation Failed");
        e.errors = Some(json!([{"resource": "SocialAccount", "field": "account_urls", "code": "custom", "message": format!("You can have at most {MAX_SOCIAL} social accounts")}]));
        return Err(e);
    }
    tx.commit().await?;
    Ok(added)
}

async fn my_social(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let all = social_accounts(&s, u.id).await?;
    let total = all.len() as i64;
    Ok(Paged::new(&uri, &s, p, Some(total), all.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect()))
}

async fn user_social(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    let all = social_accounts(&s, u.id).await?;
    let total = all.len() as i64;
    Ok(Paged::new(&uri, &s, p, Some(total), all.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect()))
}

async fn add_social(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<SocialBody>) -> ApiResult<Response> {
    let urls = b.account_urls.filter(|v| !v.is_empty()).ok_or_else(|| ApiError::validation("SocialAccount", "account_urls", "missing_field"))?;
    let added = add_social_svc(&s, &u, urls).await?;
    Ok((StatusCode::CREATED, axum::Json(Value::Array(added))).into_response())
}

async fn delete_social(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<SocialBody>) -> ApiResult<Response> {
    let urls = b.account_urls.filter(|v| !v.is_empty()).ok_or_else(|| ApiError::validation("SocialAccount", "account_urls", "missing_field"))?;
    let urls: Vec<String> = urls.into_iter().map(|x| x.trim().to_string()).collect();
    let n = sqlx::query("DELETE FROM social_accounts WHERE user_id = $1 AND url = ANY($2)").bind(u.id).bind(&urls).execute(&s.db).await?.rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// GET /user/{account_id}

async fn get_by_id(State(s): State<AppState>, MaybeUser(v): MaybeUser, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    let u = db::user_by_id(&s.db, id).await?;
    let (public, private) = super::users::repo_counts(&s, u.id).await?;
    let (followers, following): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM follows WHERE followee_id = $1), (SELECT count(*) FROM follows WHERE follower_id = $1)",
    )
    .bind(u.id)
    .fetch_one(&s.db)
    .await?;
    let api = s.api_url();
    let mut out = if v.as_ref().is_some_and(|v| v.id == u.id) {
        crate::json::user::private(&api, s.base_url(), &u, public, private)
    } else {
        crate::json::user::public(&api, s.base_url(), &u, public, followers, following)
    };
    out["followers"] = json!(followers);
    out["following"] = json!(following);
    let gists: i64 = sqlx::query_scalar("SELECT count(*) FROM gists WHERE owner_id = $1 AND public").bind(u.id).fetch_one(&s.db).await?;
    out["public_gists"] = json!(gists);
    Ok(Json(out))
}

// ---------------------------------------------------------------------------------------------
// Hovercard

#[derive(Deserialize)]
struct HoverQuery {
    subject_type: Option<String>,
    subject_id: Option<String>,
}

fn ctx_item(message: String, octicon: &str) -> Value {
    json!({"message": message, "octicon": octicon})
}

async fn hovercard(State(s): State<AppState>, AuthUser(viewer): AuthUser, Path(login): Path<String>, Query(q): Query<HoverQuery>) -> ApiResult<Json<Value>> {
    let u = db::user_by_login(&s.db, &login).await?;
    let mut contexts = Vec::new();
    match (q.subject_type.as_deref(), q.subject_id.as_deref()) {
        (Some(_), None) | (None, Some(_)) => return Err(ApiError::validation("Hovercard", "subject_id", "missing_field")),
        (Some(t), Some(id)) => {
            let id: i64 = id.parse().map_err(|_| ApiError::validation("Hovercard", "subject_id", "invalid"))?;
            match t {
                "organization" => {
                    let org = db::user_by_id(&s.db, id).await?;
                    if super::orgs_extra::org_role(&s.db, org.id, u.id).await?.is_some() {
                        contexts.push(ctx_item(format!("Member of @{}", org.login), "organization"));
                    }
                }
                "repository" => {
                    let ctx = db::load_repo_by_id(&s.db, Some(&viewer), id).await?;
                    if ctx.owner.id == u.id {
                        contexts.push(ctx_item("Owns this repository".into(), "repo"));
                    } else {
                        let p = db::permission(&s.db, Some(&u), &ctx.repo, &ctx.owner).await?;
                        if p >= db::Perm::Write {
                            contexts.push(ctx_item(format!("Has {} access to this repository", p.as_str()), "repo"));
                        }
                    }
                }
                "issue" | "pull_request" => {
                    let row: Option<(i64, Option<i64>, bool)> = sqlx::query_as("SELECT repo_id, user_id, is_pull FROM issues WHERE id = $1").bind(id).fetch_optional(&s.db).await?;
                    let (repo_id, author, is_pull) = row.ok_or_else(ApiError::not_found)?;
                    db::load_repo_by_id(&s.db, Some(&viewer), repo_id).await?;
                    let noun = if is_pull { "pull request" } else { "issue" };
                    if author == Some(u.id) {
                        contexts.push(ctx_item(format!("Opened this {noun}"), if is_pull { "git-pull-request" } else { "issue-opened" }));
                    }
                    let commented: Option<i64> = sqlx::query_scalar("SELECT 1::bigint FROM issue_comments WHERE issue_id = $1 AND user_id = $2 LIMIT 1")
                        .bind(id)
                        .bind(u.id)
                        .fetch_optional(&s.db)
                        .await?;
                    if commented.is_some() {
                        contexts.push(ctx_item(format!("Commented on this {noun}"), "comment"));
                    }
                }
                _ => return Err(ApiError::validation("Hovercard", "subject_type", "invalid")),
            }
        }
        (None, None) => {}
    }
    if contexts.is_empty() {
        let orgs: Vec<String> = sqlx::query_scalar("SELECT o.login FROM org_members m JOIN users o ON o.id = m.org_id WHERE m.user_id = $1 ORDER BY o.login LIMIT 3")
            .bind(u.id)
            .fetch_all(&s.db)
            .await?;
        if !orgs.is_empty() {
            contexts.push(ctx_item(format!("Member of {}", orgs.iter().map(|o| format!("@{o}")).collect::<Vec<_>>().join(", ")), "organization"));
        }
        if let Some(l) = u.location.clone().filter(|l| !l.is_empty()) {
            contexts.push(ctx_item(l, "location"));
        }
    }
    Ok(Json(json!({"contexts": contexts})))
}
