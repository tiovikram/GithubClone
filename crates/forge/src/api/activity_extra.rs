//! Activity extras: `GET /feeds` plus the Atom feeds it points to (`{base}/feeds/...`),
//! `GET /users/{username}/events/orgs/{org}`, and codes of conduct (`/codes_of_conduct`).
//!
//! Private feeds (`*.private.atom`) are authenticated by a per-user token in the URL
//! (HMAC of the user id with a server secret), like GitHub's feed tokens.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

use super::{PageParams, Paged};
use crate::activity::EventRow;
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/feeds", get(feeds))
        .route("/users/{username}/events/orgs/{org}", get(org_events_for_user))
        .route("/codes_of_conduct", get(list_coc))
        .route("/codes_of_conduct/{key}", get(get_coc))
}

/// Atom feeds (merged at the top level in `lib.rs`).
pub fn web_routes() -> Router<AppState> {
    Router::new().route("/feeds/{*rest}", get(atom))
}

// ---------------------------------------------------------------------------------------------
// Feeds

async fn feed_token(s: &AppState, user_id: i64) -> ApiResult<String> {
    let secret = crate::signed_url::server_secret(&s.db, "feeds").await?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(ApiError::internal)?;
    mac.update(format!("feed:{user_id}").as_bytes());
    Ok(hex::encode(&mac.finalize().into_bytes()[..16]))
}

fn link(href: String) -> Value {
    json!({"href": href, "type": "application/atom+xml"})
}

async fn feeds(State(s): State<AppState>, MaybeUser(u): MaybeUser) -> ApiResult<Json<Value>> {
    let f = format!("{}/feeds", s.base_url());
    let mut v = json!({
        "timeline_url": format!("{f}/timeline.atom"),
        "user_url": format!("{f}/users/{{user}}.atom"),
        "security_advisories_url": format!("{f}/security-advisories.atom"),
        "repository_discussions_url": format!("{f}/repos/{{user}}/{{repo}}/discussions.atom"),
        "repository_discussions_category_url": format!("{f}/repos/{{user}}/{{repo}}/discussions/categories/{{category}}.atom"),
        "_links": {
            "timeline": link(format!("{f}/timeline.atom")),
            "user": link(format!("{f}/users/{{user}}.atom")),
            "security_advisories": link(format!("{f}/security-advisories.atom")),
            "repository_discussions": link(format!("{f}/repos/{{user}}/{{repo}}/discussions.atom")),
            "repository_discussions_category": link(format!("{f}/repos/{{user}}/{{repo}}/discussions/categories/{{category}}.atom")),
        },
    });
    if let Some(u) = u {
        let token = feed_token(&s, u.id).await?;
        let public = format!("{f}/users/{}.atom", u.login);
        let private = format!("{f}/users/{}.private.atom?token={token}", u.login);
        let actor = format!("{f}/users/{}.private.actor.atom?token={token}", u.login);
        let orgs: Vec<String> = sqlx::query_scalar("SELECT o.login FROM org_members m JOIN users o ON o.id = m.org_id WHERE m.user_id = $1 ORDER BY o.login")
            .bind(u.id)
            .fetch_all(&s.db)
            .await?;
        let org_urls: Vec<String> = orgs.iter().map(|o| format!("{f}/orgs/{o}.private.atom?token={token}")).collect();
        v["current_user_public_url"] = json!(public);
        v["current_user_url"] = json!(private);
        v["current_user_actor_url"] = json!(actor);
        v["current_user_organization_url"] = json!(format!("{f}/orgs/{{org}}.private.atom?token={token}"));
        v["current_user_organization_urls"] = json!(org_urls);
        v["_links"]["current_user_public"] = link(public);
        v["_links"]["current_user"] = link(private);
        v["_links"]["current_user_actor"] = link(actor);
        v["_links"]["current_user_organization"] = link(format!("{f}/orgs/{{org}}.private.atom?token={token}"));
        v["_links"]["current_user_organizations"] = json!(org_urls.into_iter().map(link).collect::<Vec<_>>());
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct TokenQ {
    token: Option<String>,
}

const PUBLIC_NOW: &str = "(public AND (repo_id IS NULL OR EXISTS(SELECT 1 FROM repositories vr WHERE vr.id = activity_events.repo_id AND NOT vr.private)))";
/// Events in repositories user $1 can read.
const READABLE: &str = "(repo_id IS NULL OR forge_repo_readable(activity_events.repo_id, $1))";

fn esc(x: &str) -> String {
    x.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

async fn atom_feed(s: &AppState, title: &str, self_url: &str, where_sql: &str, id: i64) -> ApiResult<Response> {
    let sql = format!("SELECT * FROM activity_events WHERE {where_sql} ORDER BY id DESC LIMIT 30");
    let rows: Vec<EventRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).fetch_all(&s.db).await?;
    let items = crate::activity::render(s, &rows).await?;
    let host = crate::json::repo::host_of(s.base_url());
    let updated = items.first().and_then(|e| e["created_at"].as_str()).map(str::to_string).unwrap_or_else(|| crate::json::ts(chrono::Utc::now()));
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\" xml:lang=\"en-US\">\n  <id>tag:{host},2008:{}</id>\n  <link type=\"application/atom+xml\" rel=\"self\" href=\"{}\"/>\n  <title>{}</title>\n  <updated>{updated}</updated>\n",
        esc(self_url), esc(self_url), esc(title)
    );
    for e in &items {
        let actor = e["actor"]["login"].as_str().unwrap_or("ghost");
        let repo = e["repo"]["name"].as_str().unwrap_or("");
        let kind = e["type"].as_str().unwrap_or("Event");
        let verb = kind.trim_end_matches("Event");
        let href = if repo.is_empty() { format!("{}/{actor}", s.base_url()) } else { format!("{}/{repo}", s.base_url()) };
        out.push_str(&format!(
            "  <entry>\n    <id>tag:{host},2008:{kind}/{}</id>\n    <published>{}</published>\n    <updated>{}</updated>\n    <link type=\"text/html\" rel=\"alternate\" href=\"{}\"/>\n    <title type=\"html\">{} {} {}</title>\n    <author><name>{}</name><uri>{}/{}</uri></author>\n  </entry>\n",
            e["id"].as_str().unwrap_or(""),
            e["created_at"].as_str().unwrap_or(""),
            e["created_at"].as_str().unwrap_or(""),
            esc(&href),
            esc(actor),
            esc(&verb.to_lowercase()),
            esc(repo),
            esc(actor),
            s.base_url(),
            esc(actor),
        ));
    }
    out.push_str("</feed>\n");
    Ok(([(header::CONTENT_TYPE, "application/atom+xml; charset=utf-8")], out).into_response())
}

async fn atom(State(s): State<AppState>, Path(rest): Path<String>, Query(q): Query<TokenQ>) -> ApiResult<Response> {
    let base = format!("{}/feeds/{rest}", s.base_url());
    let check = |u: &User, tok: &Option<String>| {
        let s = s.clone();
        let (uid, tok) = (u.id, tok.clone());
        async move { Ok::<bool, ApiError>(tok.as_deref().is_some_and(|t| !t.is_empty()) && feed_token(&s, uid).await? == tok.unwrap_or_default()) }
    };
    if rest == "timeline.atom" {
        return atom_feed(&s, "Public timeline", &base, &format!("{PUBLIC_NOW} AND $1 = $1"), 0).await;
    }
    if rest == "security-advisories.atom" {
        return atom_feed(&s, "Security advisories", &base, "FALSE AND $1 = $1", 0).await;
    }
    if let Some(name) = rest.strip_prefix("users/") {
        if let Some(login) = name.strip_suffix(".private.actor.atom") {
            let u = db::user_by_login(&s.db, login).await?;
            if !check(&u, &q.token).await? {
                return Err(ApiError::not_found());
            }
            return atom_feed(&s, &format!("{}'s activity", u.login), &base, &format!("actor_id = $1 AND {READABLE}"), u.id).await;
        }
        if let Some(login) = name.strip_suffix(".private.atom") {
            let u = db::user_by_login(&s.db, login).await?;
            if !check(&u, &q.token).await? {
                return Err(ApiError::not_found());
            }
            let w = format!(
                "{READABLE} AND (actor_id IN (SELECT followee_id FROM follows WHERE follower_id = $1)
                   OR repo_id IN (SELECT repo_id FROM watches WHERE user_id = $1 AND subscribed) OR actor_id = $1)"
            );
            return atom_feed(&s, &format!("Private feed for {}", u.login), &base, &w, u.id).await;
        }
        if let Some(login) = name.strip_suffix(".atom") {
            let u = db::user_by_login(&s.db, login).await?;
            return atom_feed(&s, &format!("{}'s activity", u.login), &base, &format!("actor_id = $1 AND {PUBLIC_NOW}"), u.id).await;
        }
    }
    if let Some(name) = rest.strip_prefix("orgs/") {
        if let Some(login) = name.strip_suffix(".private.atom") {
            let o = db::user_by_login(&s.db, login).await?;
            // The token identifies the member; try every member (orgs are small).
            let members: Vec<User> = sqlx::query_as("SELECT u.* FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1").bind(o.id).fetch_all(&s.db).await?;
            for m in &members {
                if check(m, &q.token).await? {
                    return atom_feed(&s, &format!("{} organization feed", o.login), &base, "org_id = $1", o.id).await;
                }
            }
            return Err(ApiError::not_found());
        }
        if let Some(login) = name.strip_suffix(".atom") {
            let o = db::user_by_login(&s.db, login).await?;
            return atom_feed(&s, &format!("{} organization feed", o.login), &base, &format!("org_id = $1 AND {PUBLIC_NOW}"), o.id).await;
        }
    }
    Ok((StatusCode::NOT_FOUND, "Not Found").into_response())
}

// ---------------------------------------------------------------------------------------------
// Organization events for the authenticated user

async fn org_events_for_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(v): AuthUser,
    Path((login, org)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    if !v.login.eq_ignore_ascii_case(&login) {
        return Err(ApiError::not_found());
    }
    let o = db::user_by_login(&s.db, &org).await?;
    if o.kind != "Organization" || (super::orgs_extra::org_role(&s.db, o.id, v.id).await?.is_none() && !v.site_admin) {
        return Err(ApiError::not_found());
    }
    let sql = format!("SELECT * FROM activity_events WHERE org_id = $2 AND {READABLE} ORDER BY id DESC LIMIT $3 OFFSET $4");
    let rows: Vec<EventRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(v.id).bind(o.id).bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let items = crate::activity::render(&s, &rows).await?;
    Ok(Paged::new(&uri, &s, p, None, items))
}

// ---------------------------------------------------------------------------------------------
// Codes of conduct

struct Coc {
    key: &'static str,
    name: &'static str,
    html_url: &'static str,
    body: &'static str,
}

const CONTRIBUTOR_COVENANT: &str = include_str!("coc/contributor_covenant.md");
const CITIZEN: &str = include_str!("coc/citizen_code_of_conduct.md");

const CODES: [Coc; 2] = [
    Coc { key: "citizen_code_of_conduct", name: "Citizen Code of Conduct", html_url: "http://citizencodeofconduct.org/", body: CITIZEN },
    Coc {
        key: "contributor_covenant",
        name: "Contributor Covenant",
        html_url: "https://www.contributor-covenant.org/version/2/1/code_of_conduct/",
        body: CONTRIBUTOR_COVENANT,
    },
];

fn coc_json(s: &AppState, c: &Coc, with_body: bool) -> Value {
    let mut v = json!({"key": c.key, "name": c.name, "url": format!("{}/codes_of_conduct/{}", s.api_url(), c.key), "html_url": c.html_url});
    if with_body {
        v["body"] = json!(c.body);
    }
    v
}

/// The code of conduct with `key`, if known (for repository community metadata).
pub fn code_of_conduct(s: &AppState, key: &str) -> Option<Value> {
    CODES.iter().find(|c| c.key == key).map(|c| coc_json(s, c, false))
}

async fn list_coc(State(s): State<AppState>) -> Json<Value> {
    Json(Value::Array(CODES.iter().map(|c| coc_json(&s, c, false)).collect()))
}

async fn get_coc(State(s): State<AppState>, Path(key): Path<String>) -> ApiResult<Json<Value>> {
    let c = CODES.iter().find(|c| c.key == key).ok_or_else(ApiError::not_found)?;
    Ok(Json(coc_json(&s, c, true)))
}
