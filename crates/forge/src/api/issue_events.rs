//! Issue events (per issue, per repository, by id) and the issue timeline.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::issues::{get_issue, issues_json_multi, load_ctx, page_window};
use super::{PageParams, Paged};
use crate::auth::MaybeUser;
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::issue::{self as ji, CommentRow, EventRow, IssueRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/issues/{number}/events", get(list_for_issue))
        .route("/repos/{owner}/{repo}/issues/events", get(list_for_repo))
        .route("/repos/{owner}/{repo}/issues/events/{event_id}", get(get_event))
        .route("/repos/{owner}/{repo}/issues/{number}/timeline", get(timeline))
}

/// Events that only appear on the timeline, not in the events API.
const TIMELINE_ONLY: [&str; 2] = ["cross-referenced", "commented"];

/// Renders events of one repository; with `with_issue`, embeds the issue like GitHub does for
/// the repository-wide endpoints.
pub async fn events_json(s: &AppState, viewer: Option<&User>, ctx: &RepoCtx, rows: Vec<EventRow>, with_issue: bool) -> ApiResult<Vec<Value>> {
    let users = ji::event_users(&s.db, &rows).await?;
    let mut issues: HashMap<i64, Value> = HashMap::new();
    if with_issue {
        let mut ids: Vec<i64> = rows.iter().map(|e| e.issue_id).collect();
        ids.sort_unstable();
        ids.dedup();
        let irows: Vec<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE id = ANY($1)").bind(&ids).fetch_all(&s.db).await?;
        let order: Vec<i64> = irows.iter().map(|i| i.id).collect();
        let rendered = issues_json_multi(s, viewer, irows, false).await?;
        issues = order.into_iter().zip(rendered).collect();
    }
    Ok(rows
        .iter()
        .map(|e| ji::event(&s.api_url(), s.base_url(), ctx, e, &users, issues.get(&e.issue_id).cloned()))
        .collect())
}

async fn list_for_issue(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let issue = get_issue(&s, &ctx, number).await?;
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT * FROM issue_events WHERE issue_id = $1 AND event <> ALL($2) ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(issue.id)
    .bind(&TIMELINE_ONLY[..])
    .bind(p.limit() + 1)
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let (rows, total) = page_window(rows, &p);
    Ok(Paged::new(&uri, &s, p, total, events_json(&s, u.as_ref(), &ctx, rows, false).await?))
}

async fn list_for_repo(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT * FROM issue_events WHERE repo_id = $1 AND event <> ALL($2) ORDER BY id DESC LIMIT $3 OFFSET $4",
    )
    .bind(ctx.repo.id)
    .bind(&TIMELINE_ONLY[..])
    .bind(p.limit() + 1)
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let (rows, total) = page_window(rows, &p);
    Ok(Paged::new(&uri, &s, p, total, events_json(&s, u.as_ref(), &ctx, rows, true).await?))
}

async fn get_event(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let row: EventRow = sqlx::query_as("SELECT * FROM issue_events WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let v = events_json(&s, u.as_ref(), &ctx, vec![row], true).await?.pop().ok_or_else(ApiError::not_found)?;
    Ok(Json(v))
}

/// Timeline items of an issue (comments + events), oldest first, each with its sort time.
/// Other areas (pull reviews, commits) can merge their own items into this list.
pub async fn timeline_items(s: &AppState, viewer: Option<&User>, ctx: &RepoCtx, issue: &IssueRow) -> ApiResult<Vec<(DateTime<Utc>, i64, Value)>> {
    let api = s.api_url();
    let base = s.base_url();
    let comments: Vec<CommentRow> = sqlx::query_as("SELECT * FROM issue_comments WHERE issue_id = $1 ORDER BY id")
        .bind(issue.id)
        .fetch_all(&s.db)
        .await?;
    let comments = ji::load_comments(&s.db, ctx, comments).await?;
    let events: Vec<EventRow> = sqlx::query_as("SELECT * FROM issue_events WHERE issue_id = $1 ORDER BY id")
        .bind(issue.id)
        .fetch_all(&s.db)
        .await?;
    let users = ji::event_users(&s.db, &events).await?;

    // Source issues of cross-references (same repository), rendered with their repository.
    let source_ids: Vec<i64> = events
        .iter()
        .filter(|e| e.event == "cross-referenced")
        .filter_map(|e| e.data.get("source_issue_id").and_then(|v| v.as_i64()))
        .collect();
    let mut sources: HashMap<i64, Value> = HashMap::new();
    if !source_ids.is_empty() {
        let rows: Vec<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE id = ANY($1)").bind(&source_ids).fetch_all(&s.db).await?;
        let order: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let rendered = issues_json_multi(s, viewer, rows, true).await?;
        sources = order.into_iter().zip(rendered).collect();
    }

    let mut items: Vec<(DateTime<Utc>, i64, Value)> = Vec::new();
    for c in &comments {
        let mut v = ji::comment(&api, base, ctx, c);
        v["event"] = json!("commented");
        v["actor"] = ji::user_or_ghost(&api, base, c.user.as_ref());
        items.push((c.row.created_at, c.row.id, v));
    }
    for e in &events {
        if e.event == "cross-referenced" {
            let src = e.data.get("source_issue_id").and_then(|v| v.as_i64()).and_then(|id| sources.get(&id));
            let Some(src) = src else { continue };
            let v = json!({
                "event": "cross-referenced",
                "actor": ji::user_or_ghost(&api, base, e.actor_id.and_then(|id| users.get(&id))),
                "created_at": crate::json::ts(e.created_at),
                "updated_at": crate::json::ts(e.created_at),
                "source": { "type": "issue", "issue": src },
            });
            items.push((e.created_at, e.id, v));
            continue;
        }
        items.push((e.created_at, e.id, ji::event(&api, base, ctx, e, &users, None)));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    Ok(items)
}

#[derive(Deserialize, Default)]
struct TimelineQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn timeline(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let issue = get_issue(&s, &ctx, number).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let items = timeline_items(&s, u.as_ref(), &ctx, &issue).await?;
    let total = items.len() as i64;
    let page: Vec<Value> = items.into_iter().skip(p.offset() as usize).take(p.per_page() as usize).map(|(_, _, v)| v).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), page))
}
