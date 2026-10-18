//! Notifications inbox: `/notifications`, `/repos/{o}/{r}/notifications`,
//! `/notifications/threads/{id}[/subscription]`.
//!
//! Threads are created by [`on_event`], called for every repository webhook event
//! (`webhooks::enqueue_event_ctx`). Recipients follow GitHub: participants of the subject
//! (author, assignees, commenters, requested reviewers, @mentioned users, explicit thread
//! subscribers) plus repository watchers, never the actor, never users who ignore the
//! repository or the thread, and only users who can read the repository. Each recipient has one
//! thread per subject; new activity marks it unread again.
use std::collections::{HashMap, HashSet};

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
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
        .route("/notifications", get(list).put(mark_all))
        .route("/notifications/threads/{thread_id}", get(get_thread).patch(read_thread).delete(done_thread))
        .route("/notifications/threads/{thread_id}/subscription", get(get_sub).put(put_sub).delete(delete_sub))
        .route("/repos/{owner}/{repo}/notifications", get(list_repo).put(mark_repo))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThreadRow {
    pub id: i64,
    pub user_id: i64,
    pub repo_id: i64,
    pub subject_key: String,
    pub subject_type: String,
    pub subject_title: String,
    pub subject_url: Option<String>,
    pub latest_comment_url: Option<String>,
    pub reason: String,
    pub unread: bool,
    pub done: bool,
    pub last_read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------------------------
// Generation

/// Reason precedence (lower wins).
fn rank(reason: &str) -> u8 {
    match reason {
        "review_requested" => 0,
        "assign" => 1,
        "mention" => 2,
        "invitation" => 3,
        "author" => 4,
        "comment" => 5,
        "state_change" => 6,
        "manual" => 7,
        _ => 8,
    }
}

/// `@login` mentions in a Markdown body (lower-cased logins).
pub fn mentions(body: &str) -> Vec<String> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    for (i, c) in b.iter().enumerate() {
        if *c != b'@' || (i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'`' || b[i - 1] == b'/')) {
            continue;
        }
        let end = b[i + 1..].iter().position(|x| !(x.is_ascii_alphanumeric() || *x == b'-')).map(|p| i + 1 + p).unwrap_or(b.len());
        if end > i + 1 {
            let login = body[i + 1..end].trim_end_matches('-').to_lowercase();
            if !login.is_empty() && !out.contains(&login) {
                out.push(login);
            }
        }
    }
    out
}

struct Subject {
    key: String,
    kind: &'static str,
    title: String,
    url: Option<String>,
    latest: Option<String>,
    issue_id: Option<i64>,
}

fn subject_of(event: &str, p: &Value) -> Option<Subject> {
    let s = |v: &Value| v.as_str().map(str::to_string);
    match event {
        "issues" | "issue_comment" => {
            let i = p.get("issue")?;
            let is_pull = i.get("pull_request").is_some_and(|x| !x.is_null());
            let url = s(&i["url"])?;
            let url = if is_pull { url.replace("/issues/", "/pulls/") } else { url };
            let latest = p.pointer("/comment/url").and_then(|v| v.as_str()).map(str::to_string).unwrap_or_else(|| url.clone());
            Some(Subject {
                key: format!("Issue:{}", i["id"].as_i64()?),
                kind: if is_pull { "PullRequest" } else { "Issue" },
                title: s(&i["title"]).unwrap_or_default(),
                url: Some(url),
                latest: Some(latest),
                issue_id: i["id"].as_i64(),
            })
        }
        "pull_request" | "pull_request_review" | "pull_request_review_comment" => {
            let pr = p.get("pull_request")?;
            let url = s(&pr["url"])?;
            let latest = p
                .pointer("/comment/url")
                .or_else(|| p.pointer("/review/_links/html/href"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|_| event == "pull_request_review_comment")
                .unwrap_or_else(|| url.clone());
            Some(Subject {
                key: format!("Issue:{}", pr["id"].as_i64()?),
                kind: "PullRequest",
                title: s(&pr["title"]).unwrap_or_default(),
                url: Some(url),
                latest: Some(latest),
                issue_id: pr["id"].as_i64(),
            })
        }
        "release" => {
            let r = p.get("release")?;
            Some(Subject {
                key: format!("Release:{}", r["id"].as_i64()?),
                kind: "Release",
                title: s(&r["name"]).filter(|n| !n.is_empty()).or_else(|| s(&r["tag_name"])).unwrap_or_default(),
                url: s(&r["url"]),
                latest: s(&r["url"]),
                issue_id: None,
            })
        }
        "commit_comment" => {
            let c = p.get("comment")?;
            let sha = s(&c["commit_id"])?;
            let repo_url = p.pointer("/repository/url").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            Some(Subject {
                key: format!("Commit:{}", sha),
                kind: "Commit",
                title: format!("Comment on commit {}", &sha[..sha.len().min(7)]),
                url: Some(format!("{repo_url}/commits/{sha}")),
                latest: s(&c["url"]),
                issue_id: None,
            })
        }
        _ => None,
    }
}

/// Whether an event (`event` + `action`) produces notifications.
fn notifies(event: &str, action: &str) -> bool {
    match event {
        "issues" => matches!(action, "opened" | "closed" | "reopened" | "assigned"),
        "issue_comment" | "pull_request_review_comment" | "commit_comment" => action == "created",
        "pull_request" => matches!(action, "opened" | "closed" | "reopened" | "assigned" | "review_requested" | "ready_for_review"),
        "pull_request_review" => action == "submitted",
        "release" => action == "published",
        _ => false,
    }
}

/// Creates/refreshes notification threads for a repository event. Never fails the caller.
pub async fn on_event(s: &AppState, ctx: &RepoCtx, event: &str, payload: &Value) {
    let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
    if !notifies(event, action) {
        return;
    }
    if let Err(e) = notify(s, ctx, event, action, payload).await {
        tracing::warn!(error = %e.message, event, "notification fan-out failed");
    }
}

async fn notify(s: &AppState, ctx: &RepoCtx, event: &str, action: &str, p: &Value) -> ApiResult<()> {
    let Some(subj) = subject_of(event, p) else { return Ok(()) };
    let actor = p.pointer("/sender/id").and_then(Value::as_i64);
    let mut reasons: HashMap<i64, &'static str> = HashMap::new();
    fn put(m: &mut HashMap<i64, &'static str>, id: i64, r: &'static str) {
        let e = m.entry(id).or_insert(r);
        if rank(r) < rank(e) {
            *e = r;
        }
    }
    // Watchers.
    // "Custom" watchers only get the subject kinds they picked.
    let kind = match subj.kind {
        "Issue" => "issues",
        "PullRequest" => "pulls",
        "Release" => "releases",
        _ => "",
    };
    let watchers: Vec<i64> =
        sqlx::query_scalar("SELECT user_id FROM watches WHERE repo_id = $1 AND subscribed AND NOT ignored AND (custom_events IS NULL OR $2 = ANY(custom_events))")
            .bind(ctx.repo.id)
            .bind(kind)
            .fetch_all(&s.db)
            .await?;
    for w in watchers {
        put(&mut reasons, w, "subscribed");
    }
    let mut bodies: Vec<String> = Vec::new();
    for ptr in ["/comment/body", "/review/body", "/release/body"] {
        if let Some(b) = p.pointer(ptr).and_then(Value::as_str) {
            bodies.push(b.to_string());
        }
    }
    if matches!(action, "opened") {
        for ptr in ["/issue/body", "/pull_request/body"] {
            if let Some(b) = p.pointer(ptr).and_then(Value::as_str) {
                bodies.push(b.to_string());
            }
        }
    }
    if let Some(issue_id) = subj.issue_id {
        let author: Option<Option<i64>> = sqlx::query_scalar("SELECT user_id FROM issues WHERE id = $1").bind(issue_id).fetch_optional(&s.db).await?;
        if let Some(Some(a)) = author {
            put(&mut reasons, a, "author");
        }
        let commenters: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT user_id FROM issue_comments WHERE issue_id = $1 AND user_id IS NOT NULL").bind(issue_id).fetch_all(&s.db).await?;
        for c in commenters {
            put(&mut reasons, c, "comment");
        }
        let assignees: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM issue_assignees WHERE issue_id = $1").bind(issue_id).fetch_all(&s.db).await?;
        for a in assignees {
            put(&mut reasons, a, if action == "assigned" { "assign" } else { "comment" });
        }
        if action == "review_requested" {
            if let Some(r) = p.pointer("/requested_reviewer/id").and_then(Value::as_i64) {
                put(&mut reasons, r, "review_requested");
            }
        }
        if action == "assigned" {
            if let Some(a) = p.pointer("/assignee/id").and_then(Value::as_i64) {
                put(&mut reasons, a, "assign");
            }
        }
    }
    let logins: Vec<String> = bodies.iter().flat_map(|b| mentions(b)).collect();
    if !logins.is_empty() {
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM users WHERE lower(login) = ANY($1) AND kind = 'User'").bind(&logins).fetch_all(&s.db).await?;
        for i in ids {
            put(&mut reasons, i, "mention");
        }
    }
    // Explicit thread subscriptions.
    let subs: Vec<(i64, bool, bool)> =
        sqlx::query_as("SELECT user_id, subscribed, ignored FROM notification_subscriptions WHERE subject_key = $1 AND repo_id = $2")
            .bind(&subj.key)
            .bind(ctx.repo.id)
            .fetch_all(&s.db)
            .await?;
    let mut muted: HashSet<i64> = HashSet::new();
    for (uid, subscribed, ignored) in subs {
        if ignored {
            muted.insert(uid);
        } else if subscribed {
            put(&mut reasons, uid, "manual");
        } else {
            // Unsubscribed: only direct involvement (mention, assignment, review request) notifies.
            if reasons.get(&uid).is_none_or(|r| rank(r) > rank("mention")) {
                muted.insert(uid);
            }
        }
    }
    // Commenting re-subscribes the actor (GitHub semantics for muted threads).
    if let Some(a) = actor {
        sqlx::query("UPDATE notification_subscriptions SET subscribed = TRUE WHERE user_id = $1 AND subject_key = $2 AND NOT ignored AND NOT subscribed")
            .bind(a)
            .bind(&subj.key)
            .execute(&s.db)
            .await?;
    }
    let ignoring: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM watches WHERE repo_id = $1 AND ignored").bind(ctx.repo.id).fetch_all(&s.db).await?;
    let ids: Vec<i64> = reasons.keys().copied().filter(|id| Some(*id) != actor && !muted.contains(id) && !ignoring.contains(id)).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    for id in ids {
        let Some(u) = users.get(&id) else { continue };
        if u.kind != "User" || db::permission(&s.db, Some(u), &ctx.repo, &ctx.owner).await? < Perm::Read {
            continue;
        }
        let reason = reasons[&id];
        sqlx::query(
            "INSERT INTO notification_threads (user_id, repo_id, subject_key, subject_type, subject_title, subject_url, latest_comment_url, reason)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (user_id, subject_key) DO UPDATE SET unread = TRUE, done = FALSE, updated_at = now(),
                subject_title = EXCLUDED.subject_title, latest_comment_url = EXCLUDED.latest_comment_url,
                reason = CASE WHEN notification_threads.reason IN ('review_requested','assign','mention') THEN notification_threads.reason ELSE EXCLUDED.reason END",
        )
        .bind(id)
        .bind(ctx.repo.id)
        .bind(&subj.key)
        .bind(subj.kind)
        .bind(&subj.title)
        .bind(&subj.url)
        .bind(&subj.latest)
        .bind(reason)
        .execute(&s.db)
        .await?;
    }
    Ok(())
}

/// Notifies one user directly (e.g. a repository invitation).
pub async fn notify_user(s: &AppState, user_id: i64, ctx: &RepoCtx, key: &str, kind: &str, title: &str, url: Option<&str>, reason: &str) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO notification_threads (user_id, repo_id, subject_key, subject_type, subject_title, subject_url, latest_comment_url, reason)
         VALUES ($1,$2,$3,$4,$5,$6,$6,$7)
         ON CONFLICT (user_id, subject_key) DO UPDATE SET unread = TRUE, done = FALSE, updated_at = now(), reason = EXCLUDED.reason",
    )
    .bind(user_id)
    .bind(ctx.repo.id)
    .bind(key)
    .bind(kind)
    .bind(title)
    .bind(url)
    .bind(reason)
    .execute(&s.db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// JSON

fn thread_url(s: &AppState, id: i64) -> String {
    format!("{}/notifications/threads/{id}", s.api_url())
}

pub async fn threads_json(s: &AppState, viewer: &User, rows: &[ThreadRow]) -> ApiResult<Vec<Value>> {
    let mut repos: HashMap<i64, Option<RepoCtx>> = HashMap::new();
    for r in rows {
        if !repos.contains_key(&r.repo_id) {
            repos.insert(r.repo_id, db::load_repo_by_id(&s.db, Some(viewer), r.repo_id).await.ok());
        }
    }
    let api = s.api_url();
    Ok(rows
        .iter()
        .filter_map(|t| {
            let ctx = repos.get(&t.repo_id)?.as_ref()?;
            let url = thread_url(s, t.id);
            Some(json!({
                "id": t.id.to_string(),
                "repository": crate::json::repo::repo(&api, s.base_url(), ctx),
                "subject": {
                    "title": t.subject_title,
                    "url": t.subject_url,
                    "latest_comment_url": t.latest_comment_url,
                    "type": t.subject_type,
                },
                "reason": t.reason,
                "unread": t.unread,
                "updated_at": crate::json::ts(t.updated_at),
                "last_read_at": crate::json::ts_opt(t.last_read_at),
                "url": url,
                "subscription_url": format!("{url}/subscription"),
            }))
        })
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Handlers

#[derive(Deserialize, Default)]
struct ListQuery {
    all: Option<bool>,
    participating: Option<bool>,
    since: Option<String>,
    before: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

fn parse_ts(v: &Option<String>, field: &str) -> ApiResult<Option<DateTime<Utc>>> {
    v.as_deref()
        .map(|x| DateTime::parse_from_rfc3339(x).map(|d| d.with_timezone(&Utc)).map_err(|_| ApiError::validation("Notification", field, "invalid")))
        .transpose()
}

async fn page_of(s: &AppState, uri: &OriginalUri, u: &User, repo_id: Option<i64>, q: ListQuery) -> ApiResult<Paged> {
    let p = PageParams { page: q.page, per_page: q.per_page.map(|x| x.min(50)) };
    let since = parse_ts(&q.since, "since")?;
    let before = parse_ts(&q.before, "before")?;
    let filter = "user_id = $1 AND NOT done AND ($2::bigint IS NULL OR repo_id = $2) AND ($3 OR unread)
        AND (NOT $4 OR reason <> 'subscribed') AND ($5::timestamptz IS NULL OR updated_at >= $5) AND ($6::timestamptz IS NULL OR updated_at < $6)";
    let rows: Vec<ThreadRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM notification_threads WHERE {filter} ORDER BY updated_at DESC, id DESC LIMIT $7 OFFSET $8")))
        .bind(u.id)
        .bind(repo_id)
        .bind(q.all.unwrap_or(false))
        .bind(q.participating.unwrap_or(false))
        .bind(since)
        .bind(before)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = threads_json(s, u, &rows).await?;
    Ok(Paged::new(uri, s, p, None, items))
}

async fn list(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    page_of(&s, &uri, &u, None, q).await
}

async fn list_repo(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    page_of(&s, &uri, &u, Some(ctx.repo.id), q).await
}

#[derive(Deserialize, Default)]
struct MarkBody {
    last_read_at: Option<String>,
    read: Option<bool>,
}

async fn mark(s: &AppState, u: &User, repo_id: Option<i64>, b: MarkBody) -> ApiResult<Response> {
    let at = parse_ts(&b.last_read_at, "last_read_at")?.unwrap_or_else(Utc::now);
    let read = b.read.unwrap_or(true);
    sqlx::query(
        "UPDATE notification_threads SET unread = NOT $3, last_read_at = CASE WHEN $3 THEN $4 ELSE last_read_at END
          WHERE user_id = $1 AND ($2::bigint IS NULL OR repo_id = $2) AND updated_at <= $4",
    )
    .bind(u.id)
    .bind(repo_id)
    .bind(read)
    .bind(at)
    .execute(&s.db)
    .await?;
    Ok(StatusCode::RESET_CONTENT.into_response())
}

async fn mark_all(State(s): State<AppState>, AuthUser(u): AuthUser, Json(b): Json<MarkBody>) -> ApiResult<Response> {
    mark(&s, &u, None, b).await
}

async fn mark_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<MarkBody>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    mark(&s, &u, Some(ctx.repo.id), b).await
}

async fn load_thread(s: &AppState, u: &User, id: &str) -> ApiResult<ThreadRow> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    sqlx::query_as("SELECT * FROM notification_threads WHERE id = $1 AND user_id = $2").bind(id).bind(u.id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

async fn get_thread(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let t = load_thread(&s, &u, &id).await?;
    Ok(Json(threads_json(&s, &u, &[t]).await?.pop().ok_or_else(ApiError::not_found)?))
}

async fn read_thread(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let t = load_thread(&s, &u, &id).await?;
    sqlx::query("UPDATE notification_threads SET unread = FALSE, last_read_at = now() WHERE id = $1").bind(t.id).execute(&s.db).await?;
    Ok(StatusCode::RESET_CONTENT.into_response())
}

async fn done_thread(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let t = load_thread(&s, &u, &id).await?;
    sqlx::query("UPDATE notification_threads SET done = TRUE, unread = FALSE, last_read_at = now() WHERE id = $1").bind(t.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn sub_json(s: &AppState, t: &ThreadRow) -> ApiResult<Value> {
    let row: Option<(bool, bool, Option<String>, DateTime<Utc>)> =
        sqlx::query_as("SELECT subscribed, ignored, reason, created_at FROM notification_subscriptions WHERE user_id = $1 AND subject_key = $2")
            .bind(t.user_id)
            .bind(&t.subject_key)
            .fetch_optional(&s.db)
            .await?;
    let url = thread_url(s, t.id);
    let (subscribed, ignored, reason, created) = row.unwrap_or((true, false, Some(t.reason.clone()), t.created_at));
    Ok(json!({
        "subscribed": subscribed, "ignored": ignored, "reason": reason,
        "created_at": crate::json::ts(created),
        "url": format!("{url}/subscription"), "thread_url": url,
    }))
}

async fn get_sub(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let t = load_thread(&s, &u, &id).await?;
    Ok(Json(sub_json(&s, &t).await?))
}

#[derive(Deserialize, Default)]
struct SubBody {
    ignored: Option<bool>,
}

/// Sets the caller's subscription to a thread's subject (`ignored: true` mutes it entirely).
pub async fn set_thread_subscription(s: &AppState, t: &ThreadRow, subscribed: bool, ignored: bool) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO notification_subscriptions (user_id, subject_key, repo_id, subscribed, ignored, reason) VALUES ($1,$2,$3,$4,$5,'manual')
         ON CONFLICT (user_id, subject_key) DO UPDATE SET subscribed = $4, ignored = $5",
    )
    .bind(t.user_id)
    .bind(&t.subject_key)
    .bind(t.repo_id)
    .bind(subscribed)
    .bind(ignored)
    .execute(&s.db)
    .await?;
    Ok(())
}

async fn put_sub(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>, Json(b): Json<SubBody>) -> ApiResult<Json<Value>> {
    let t = load_thread(&s, &u, &id).await?;
    let ignored = b.ignored.unwrap_or(false);
    set_thread_subscription(&s, &t, !ignored, ignored).await?;
    Ok(Json(sub_json(&s, &t).await?))
}

async fn delete_sub(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let t = load_thread(&s, &u, &id).await?;
    set_thread_subscription(&s, &t, false, false).await?;
    Ok(no_content())
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_mentions() {
        assert_eq!(super::mentions("hi @Alice and @bob-x, not a@b.c or `@x`"), vec!["alice", "bob-x"]);
    }
}
