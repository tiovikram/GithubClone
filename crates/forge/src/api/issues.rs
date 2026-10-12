//! Issues: create/get/update, listings (repo, /issues, /user/issues, /orgs/{org}/issues),
//! locking, assignees and the labels of an issue.
//!
//! Every mutation is a `pub async fn` service (shared with GraphQL); handlers only parse,
//! authorize and render. Counters (`repositories.open_issues_count`, `issues.comments_count`)
//! are maintained by triggers (migration 0200), so any writer of `issues` stays consistent.
use std::collections::{HashMap, HashSet};

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use sqlx::{PgConnection, Postgres, QueryBuilder};

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, Repo, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json;
use crate::json::issue::{self as ji, IssueRow, LabelRow, MilestoneRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/issues", get(list_for_viewer))
        .route("/user/issues", get(list_for_user))
        .route("/orgs/{org}/issues", get(list_for_org))
        .route("/repos/{owner}/{repo}/issues", get(list_for_repo).post(create_handler))
        .route("/repos/{owner}/{repo}/issues/{number}", get(get_handler).patch(update_handler))
        .route("/repos/{owner}/{repo}/issues/{number}/lock", put(lock_handler).delete(unlock_handler))
        .route(
            "/repos/{owner}/{repo}/issues/{number}/assignees",
            axum::routing::post(add_assignees_handler).delete(remove_assignees_handler),
        )
        .route("/repos/{owner}/{repo}/issues/{number}/assignees/{assignee}", get(check_assignee_for_issue))
        .route(
            "/repos/{owner}/{repo}/issues/{number}/labels",
            get(list_issue_labels).post(add_labels_handler).put(set_labels_handler).delete(remove_all_labels_handler),
        )
        .route("/repos/{owner}/{repo}/issues/{number}/labels/{name}", axum::routing::delete(remove_label_handler))
        .route("/repos/{owner}/{repo}/assignees", get(list_assignees))
        .route("/repos/{owner}/{repo}/assignees/{assignee}", get(check_assignee))
}

// ---------------------------------------------------------------------------------------------
// Small shared helpers (also used by the other issue-area modules).

/// Deserializes a field that may be absent (`None`), `null` (`Some(None)`) or a value.
pub fn double_opt<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

pub fn invalid(resource: &str, field: &str, value: impl Into<Value>) -> ApiError {
    let mut e = ApiError::validation(resource, field, "invalid");
    e.errors = Some(json!([{ "resource": resource, "field": field, "code": "invalid", "value": value.into() }]));
    e
}

/// Parses a GitHub timestamp query parameter (ISO 8601).
pub fn parse_since(s: &Option<String>) -> ApiResult<Option<DateTime<Utc>>> {
    match s.as_deref().filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => parse_time(s).map(Some).ok_or_else(|| invalid("Issue", "since", s)),
    }
}

pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(t.and_utc());
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
}

/// Splits rows fetched with `LIMIT per_page + 1` into the page and a total hint for `Paged`
/// (`None` = more pages follow; `Some(n)` = this is the last page).
pub fn page_window<T>(mut rows: Vec<T>, p: &PageParams) -> (Vec<T>, Option<i64>) {
    let per = p.per_page() as usize;
    if rows.len() > per {
        rows.truncate(per);
        (rows, None)
    } else {
        let n = rows.len() as i64;
        (rows, Some(p.offset() + n))
    }
}

/// Names from a list of label inputs: strings or `{ "name": ... }` objects.
pub fn label_names(v: &[Value]) -> Vec<String> {
    v.iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.get("name").and_then(|n| n.as_str()).map(str::to_string),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .collect()
}

/// Logins from assignee inputs: strings or `{ "login": ... }` objects.
pub fn login_names(v: &[Value]) -> Vec<String> {
    v.iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.get("login").and_then(|n| n.as_str()).map(str::to_string),
            _ => None,
        })
        .collect()
}

fn value_to_number(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn value_to_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub async fn load_ctx(s: &AppState, viewer: Option<&User>, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    db::load_repo(&s.db, viewer, owner, repo).await
}

/// Renders one issue (or the issue view of a PR).
pub async fn issue_json(s: &AppState, ctx: &RepoCtx, row: IssueRow) -> ApiResult<Value> {
    let full = ji::load_one(&s.db, ctx, row).await?;
    Ok(ji::issue(&s.api_url(), s.base_url(), ctx, &full))
}

pub async fn issues_json(s: &AppState, ctx: &RepoCtx, rows: Vec<IssueRow>) -> ApiResult<Vec<Value>> {
    let full = ji::load_full(&s.db, ctx, rows).await?;
    Ok(full.iter().map(|i| ji::issue(&s.api_url(), s.base_url(), ctx, i)).collect())
}

/// Builds `RepoCtx`s (with the viewer's permission) for a set of repository ids.
pub async fn repo_ctxs(s: &AppState, viewer: Option<&User>, repo_ids: &[i64]) -> ApiResult<HashMap<i64, RepoCtx>> {
    let repos: Vec<Repo> = sqlx::query_as("SELECT * FROM repositories WHERE id = ANY($1)")
        .bind(repo_ids)
        .fetch_all(&s.db)
        .await?;
    let owner_ids: Vec<i64> = repos.iter().map(|r| r.owner_id).collect();
    let owners = ji::users_by_ids(&s.db, &owner_ids).await?;
    let mut out = HashMap::new();
    for repo in repos {
        let Some(owner) = owners.get(&repo.owner_id).cloned() else { continue };
        let perm = db::permission(&s.db, viewer, &repo, &owner).await?;
        out.insert(repo.id, RepoCtx { repo, owner, perm });
    }
    Ok(out)
}

/// Renders issues from many repositories, keeping order; adds `repository` when asked
/// (GitHub includes it on /issues and /user/issues). Returns (issue json, repo id) pairs.
pub async fn issues_json_multi(s: &AppState, viewer: Option<&User>, rows: Vec<IssueRow>, with_repo: bool) -> ApiResult<Vec<Value>> {
    let mut repo_ids: Vec<i64> = rows.iter().map(|r| r.repo_id).collect();
    repo_ids.sort_unstable();
    repo_ids.dedup();
    let ctxs = repo_ctxs(s, viewer, &repo_ids).await?;
    let order: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut by_repo: HashMap<i64, Vec<IssueRow>> = HashMap::new();
    for r in rows {
        by_repo.entry(r.repo_id).or_default().push(r);
    }
    let mut rendered: HashMap<i64, Value> = HashMap::new();
    for (repo_id, rows) in by_repo {
        let Some(ctx) = ctxs.get(&repo_id) else { continue };
        let full = ji::load_full(&s.db, ctx, rows).await?;
        let repo_json = with_repo.then(|| json::repo::repo(&s.api_url(), s.base_url(), ctx));
        for i in &full {
            let mut v = ji::issue(&s.api_url(), s.base_url(), ctx, i);
            if let Some(r) = &repo_json {
                v["repository"] = r.clone();
            }
            rendered.insert(i.row.id, v);
        }
    }
    Ok(order.into_iter().filter_map(|id| rendered.remove(&id)).collect())
}

// ---------------------------------------------------------------------------------------------
// Service layer.

/// Records an issue event (closed, labeled, assigned, referenced, merged ...). `data` holds the
/// type-specific payload: `label {name,color}`, `assignee_id`/`assigner_id`, `milestone {title}`,
/// `rename {from,to}`, `lock_reason`, `state_reason`, `requested_reviewer_id`/`review_requester_id`,
/// `source_issue_id` (cross-referenced). Returns the event id.
pub async fn record_event(
    conn: &mut PgConnection,
    repo_id: i64,
    issue_id: i64,
    actor_id: Option<i64>,
    event: &str,
    commit_id: Option<&str>,
    data: Value,
) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO issue_events (issue_id, repo_id, actor_id, event, commit_id, data) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id",
    )
    .bind(issue_id)
    .bind(repo_id)
    .bind(actor_id)
    .bind(event)
    .bind(commit_id)
    .bind(data)
    .fetch_one(&mut *conn)
    .await?)
}

pub fn require_issues_enabled(ctx: &RepoCtx) -> ApiResult<()> {
    if ctx.repo.has_issues {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::GONE, "Issues are disabled for this repo"))
    }
}

/// Fetches an issue (or PR) by number, 404 otherwise.
pub async fn get_issue(state: &AppState, ctx: &RepoCtx, number: i64) -> ApiResult<IssueRow> {
    ji::issue_by_number(&state.db, ctx.repo.id, number).await
}

/// Current labels of an issue, by name.
pub async fn issue_labels(db: impl sqlx::PgExecutor<'_>, issue_id: i64) -> ApiResult<Vec<LabelRow>> {
    Ok(sqlx::query_as(
        "SELECT l.* FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = $1 ORDER BY l.name",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await?)
}

/// Resolves label names. Missing labels are created (default color) when `create_missing`,
/// which mirrors GitHub for callers with push access; otherwise they are a 422.
pub async fn resolve_labels(conn: &mut PgConnection, repo_id: i64, names: &[String], create_missing: bool) -> ApiResult<Vec<LabelRow>> {
    let mut out: Vec<LabelRow> = Vec::new();
    let mut seen = HashSet::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() || !seen.insert(name.to_lowercase()) {
            continue;
        }
        let found: Option<LabelRow> = sqlx::query_as("SELECT * FROM labels WHERE repo_id = $1 AND lower(name) = lower($2)")
            .bind(repo_id)
            .bind(name)
            .fetch_optional(&mut *conn)
            .await?;
        let label = match found {
            Some(l) => l,
            None if create_missing => {
                sqlx::query("INSERT INTO labels (repo_id, name, color) VALUES ($1, $2, 'ededed') ON CONFLICT DO NOTHING")
                    .bind(repo_id)
                    .bind(name)
                    .execute(&mut *conn)
                    .await?;
                sqlx::query_as("SELECT * FROM labels WHERE repo_id = $1 AND lower(name) = lower($2)")
                    .bind(repo_id)
                    .bind(name)
                    .fetch_one(&mut *conn)
                    .await?
            }
            None => return Err(invalid("Label", "name", name)),
        };
        out.push(label);
    }
    Ok(out)
}

/// SQL predicate (on alias `u`) for users that can be assigned in repo `$repo`/owner `$owner`.
fn push_assignable(qb: &mut QueryBuilder<Postgres>, ctx: &RepoCtx) {
    qb.push("(u.kind <> 'Organization' AND (u.id = ")
        .push_bind(ctx.owner.id)
        .push(" OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = ")
        .push_bind(ctx.repo.id)
        .push(" AND c.user_id = u.id AND c.permission <> 'pull') OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = ")
        .push_bind(ctx.owner.id)
        .push(" AND m.user_id = u.id)))");
}

/// Resolves assignee logins to users that may be assigned. Unknown/unassignable logins are a
/// 422 when `strict` (issue create/update), silently skipped otherwise (add-assignees).
pub async fn resolve_assignees(conn: &mut PgConnection, ctx: &RepoCtx, logins: &[String], strict: bool) -> ApiResult<Vec<User>> {
    let wanted: Vec<String> = logins.iter().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect();
    if wanted.is_empty() {
        return Ok(vec![]);
    }
    let mut qb = QueryBuilder::new("SELECT u.* FROM users u WHERE lower(u.login) = ANY(");
    qb.push_bind(wanted.clone()).push(") AND ");
    push_assignable(&mut qb, ctx);
    let users: Vec<User> = qb.build_query_as().fetch_all(&mut *conn).await?;
    if strict {
        for w in &wanted {
            if !users.iter().any(|u| u.login.to_lowercase() == *w) {
                let orig = logins.iter().find(|l| l.trim().to_lowercase() == *w).cloned().unwrap_or_default();
                return Err(invalid("Issue", "assignees", orig));
            }
        }
    }
    Ok(users)
}

pub async fn milestone_by_number(db: impl sqlx::PgExecutor<'_>, repo_id: i64, number: i64) -> ApiResult<Option<MilestoneRow>> {
    Ok(sqlx::query_as("SELECT * FROM milestones WHERE repo_id = $1 AND number = $2")
        .bind(repo_id)
        .bind(number)
        .fetch_optional(db)
        .await?)
}

async fn attach_labels(conn: &mut PgConnection, ctx: &RepoCtx, actor: &User, issue_id: i64, labels: &[LabelRow]) -> ApiResult<()> {
    for l in labels {
        let r = sqlx::query("INSERT INTO issue_labels (issue_id, label_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(issue_id)
            .bind(l.id)
            .execute(&mut *conn)
            .await?;
        if r.rows_affected() > 0 {
            let data = json!({ "label": { "name": l.name, "color": l.color } });
            record_event(conn, ctx.repo.id, issue_id, Some(actor.id), "labeled", None, data).await?;
        }
    }
    Ok(())
}

async fn detach_labels(conn: &mut PgConnection, ctx: &RepoCtx, actor: &User, issue_id: i64, labels: &[LabelRow]) -> ApiResult<()> {
    for l in labels {
        let r = sqlx::query("DELETE FROM issue_labels WHERE issue_id = $1 AND label_id = $2")
            .bind(issue_id)
            .bind(l.id)
            .execute(&mut *conn)
            .await?;
        if r.rows_affected() > 0 {
            let data = json!({ "label": { "name": l.name, "color": l.color } });
            record_event(conn, ctx.repo.id, issue_id, Some(actor.id), "unlabeled", None, data).await?;
        }
    }
    Ok(())
}

async fn attach_assignees(conn: &mut PgConnection, ctx: &RepoCtx, actor: &User, issue_id: i64, users: &[User]) -> ApiResult<()> {
    for u in users {
        let r = sqlx::query("INSERT INTO issue_assignees (issue_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(issue_id)
            .bind(u.id)
            .execute(&mut *conn)
            .await?;
        if r.rows_affected() > 0 {
            let data = json!({ "assignee_id": u.id, "assigner_id": actor.id });
            record_event(conn, ctx.repo.id, issue_id, Some(actor.id), "assigned", None, data).await?;
        }
    }
    Ok(())
}

async fn detach_assignees(conn: &mut PgConnection, ctx: &RepoCtx, actor: &User, issue_id: i64, user_ids: &[i64]) -> ApiResult<()> {
    for &uid in user_ids {
        let r = sqlx::query("DELETE FROM issue_assignees WHERE issue_id = $1 AND user_id = $2")
            .bind(issue_id)
            .bind(uid)
            .execute(&mut *conn)
            .await?;
        if r.rows_affected() > 0 {
            let data = json!({ "assignee_id": uid, "assigner_id": actor.id });
            record_event(conn, ctx.repo.id, issue_id, Some(actor.id), "unassigned", None, data).await?;
        }
    }
    Ok(())
}

async fn set_milestone(conn: &mut PgConnection, ctx: &RepoCtx, actor: &User, issue: &IssueRow, new: Option<&MilestoneRow>) -> ApiResult<()> {
    let old_id = issue.milestone_id;
    if old_id == new.map(|m| m.id) {
        return Ok(());
    }
    if let Some(old) = old_id {
        let title: Option<String> = sqlx::query_scalar("SELECT title FROM milestones WHERE id = $1")
            .bind(old)
            .fetch_optional(&mut *conn)
            .await?;
        let data = json!({ "milestone": { "title": title.unwrap_or_default() } });
        record_event(conn, ctx.repo.id, issue.id, Some(actor.id), "demilestoned", None, data).await?;
    }
    if let Some(m) = new {
        let data = json!({ "milestone": { "title": m.title } });
        record_event(conn, ctx.repo.id, issue.id, Some(actor.id), "milestoned", None, data).await?;
    }
    sqlx::query("UPDATE issues SET milestone_id = $2 WHERE id = $1")
        .bind(issue.id)
        .bind(new.map(|m| m.id))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Issue numbers referenced as `#N` in a text (same repository), deduplicated, in order.
pub fn referenced_numbers(text: &str) -> Vec<i64> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'&' || b[i - 1] == b'/')) {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let ends_ok = j == b.len() || !(b[j].is_ascii_alphanumeric() || b[j] == b'_');
            if j > start && j - start <= 9 && ends_ok {
                if let Ok(n) = text[start..j].parse::<i64>() {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// Records `cross-referenced` events on issues of the same repository mentioned as `#N` in a
/// body written on issue `source`. Best effort: never fails the calling mutation.
pub async fn record_cross_references(state: &AppState, ctx: &RepoCtx, actor: &User, source: &IssueRow, text: &str) {
    let numbers: Vec<i64> = referenced_numbers(text).into_iter().filter(|n| *n != source.number).take(20).collect();
    if numbers.is_empty() {
        return;
    }
    let res: ApiResult<()> = async {
        let targets: Vec<i64> = sqlx::query_scalar("SELECT id FROM issues WHERE repo_id = $1 AND number = ANY($2)")
            .bind(ctx.repo.id)
            .bind(&numbers)
            .fetch_all(&state.db)
            .await?;
        let mut conn = state.db.acquire().await?;
        for t in targets {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM issue_events WHERE issue_id = $1 AND event = 'cross-referenced'
                   AND (data->>'source_issue_id')::bigint = $2)",
            )
            .bind(t)
            .bind(source.id)
            .fetch_one(&mut *conn)
            .await?;
            if !exists {
                let data = json!({ "source_issue_id": source.id });
                record_event(&mut conn, ctx.repo.id, t, Some(actor.id), "cross-referenced", None, data).await?;
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e.message, "recording cross references failed");
    }
}

/// Issue numbers closed by keywords (`fixes #1`, `Closes: #2`, `resolved #3` ...) in a message.
pub fn closing_numbers(text: &str) -> Vec<i64> {
    const KEYWORDS: [&str; 9] = ["close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved"];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
    let mut out = Vec::new();
    for pair in words.windows(2) {
        if KEYWORDS.contains(&pair[0].trim_end_matches(':')) {
            if let Some(n) = referenced_numbers(pair[1]).first() {
                if !out.contains(n) {
                    out.push(*n);
                }
            }
        }
    }
    out
}

/// After a push: records `referenced` events for `#N` mentions in new commit messages and, on
/// the default branch, closes issues named with closing keywords (GitHub behaviour).
/// Best effort; called from `hooks::after_refs_changed`.
pub async fn on_refs_changed(state: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) {
    if let Err(e) = commit_references(state, ctx, actor, changed).await {
        tracing::warn!(error = %e.message, repo = ctx.repo.id, "recording commit references failed");
    }
}

async fn commit_references(state: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) -> ApiResult<()> {
    let zero = |s: &str| s.chars().all(|c| c == '0');
    let default_ref = format!("refs/heads/{}", ctx.repo.default_branch);
    let mut path = None;
    for (refname, old, new) in changed {
        if !refname.starts_with("refs/heads/") || zero(new) {
            continue;
        }
        let path = match &path {
            Some(p) => p,
            None => path.insert(state.git.sync(ctx.repo.id).await?),
        };
        let range = if zero(old) { new.clone() } else { format!("{old}..{new}") };
        let (ok, out, _) = state
            .git
            .git
            .try_run(path, &["log", "-n", "100", "--format=%H%x1f%B%x1e", &range, "--"])
            .await?;
        if !ok {
            continue;
        }
        let log = String::from_utf8_lossy(&out).to_string();
        for rec in log.split('\u{1e}') {
            let Some((sha, msg)) = rec.trim_start_matches('\n').split_once('\u{1f}') else { continue };
            let numbers = referenced_numbers(msg);
            if numbers.is_empty() {
                continue;
            }
            let closing = if *refname == default_ref { closing_numbers(msg) } else { vec![] };
            let issues: Vec<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE repo_id = $1 AND number = ANY($2)")
                .bind(ctx.repo.id)
                .bind(&numbers)
                .fetch_all(&state.db)
                .await?;
            for issue in issues {
                let mut tx = state.db.begin().await?;
                let seen: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM issue_events WHERE issue_id = $1 AND event = 'referenced' AND commit_id = $2)",
                )
                .bind(issue.id)
                .bind(sha)
                .fetch_one(&mut *tx)
                .await?;
                if seen {
                    continue;
                }
                let actor_id = actor.map(|a| a.id);
                record_event(&mut tx, ctx.repo.id, issue.id, actor_id, "referenced", Some(sha), json!({})).await?;
                if closing.contains(&issue.number) && !issue.is_pull && issue.state == "open" {
                    sqlx::query(
                        "UPDATE issues SET state = 'closed', state_reason = 'completed', closed_at = now(), closed_by_id = $2,
                            updated_at = now() WHERE id = $1",
                    )
                    .bind(issue.id)
                    .bind(actor_id)
                    .execute(&mut *tx)
                    .await?;
                    let data = json!({ "state_reason": "completed" });
                    record_event(&mut tx, ctx.repo.id, issue.id, actor_id, "closed", Some(sha), data).await?;
                }
                tx.commit().await?;
            }
        }
    }
    Ok(())
}

/// Input for [`create_issue`]. Labels/assignees/milestone are applied only when the actor has
/// triage access (GitHub silently drops them otherwise).
#[derive(Debug, Default, Clone)]
pub struct CreateIssue {
    pub title: String,
    pub body: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<i64>,
}

/// Inserts a bare `issues` row with the next per-repo number (shared with PRs) inside the
/// caller's transaction. Used by [`create_issue`]; the pulls module can use it for PRs.
pub async fn insert_issue_row(conn: &mut PgConnection, repo_id: i64, author_id: i64, title: &str, body: Option<&str>, is_pull: bool) -> ApiResult<IssueRow> {
    let number = db::next_issue_number(&mut *conn, repo_id).await?;
    Ok(sqlx::query_as(
        "INSERT INTO issues (repo_id, number, is_pull, title, body, user_id) VALUES ($1,$2,$3,$4,$5,$6) RETURNING *",
    )
    .bind(repo_id)
    .bind(number)
    .bind(is_pull)
    .bind(title)
    .bind(body)
    .bind(author_id)
    .fetch_one(&mut *conn)
    .await?)
}

/// Creates an issue. Any user who can read the repository may open one.
pub async fn create_issue(state: &AppState, ctx: &RepoCtx, actor: &User, input: CreateIssue) -> ApiResult<IssueRow> {
    require_issues_enabled(ctx)?;
    if input.title.trim().is_empty() {
        return Err(ApiError::validation("Issue", "title", "missing_field"));
    }
    let triage = ctx.perm >= Perm::Triage;
    let mut tx = state.db.begin().await?;
    let (labels, assignees, milestone) = if triage {
        let assignees = resolve_assignees(&mut tx, ctx, &input.assignees, true).await?;
        let milestone = match input.milestone {
            Some(n) => Some(milestone_by_number(&mut *tx, ctx.repo.id, n).await?.ok_or_else(|| invalid("Issue", "milestone", n))?),
            None => None,
        };
        let labels = resolve_labels(&mut tx, ctx.repo.id, &input.labels, ctx.perm >= Perm::Write).await?;
        (labels, assignees, milestone)
    } else {
        (vec![], vec![], None)
    };
    let row = insert_issue_row(&mut tx, ctx.repo.id, actor.id, &input.title, input.body.as_deref(), false).await?;
    attach_labels(&mut tx, ctx, actor, row.id, &labels).await?;
    attach_assignees(&mut tx, ctx, actor, row.id, &assignees).await?;
    if let Some(m) = &milestone {
        set_milestone(&mut tx, ctx, actor, &row, Some(m)).await?;
    }
    let row: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(row.id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    if let Some(b) = &row.body {
        record_cross_references(state, ctx, actor, &row, b).await;
    }
    Ok(row)
}

/// Input for [`update_issue`]. `None` = leave unchanged; for nullable fields `Some(None)` clears.
#[derive(Debug, Default, Clone)]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub body: Option<Option<String>>,
    /// "open" | "closed"
    pub state: Option<String>,
    /// "completed" | "not_planned" | "reopened" | "duplicate"
    pub state_reason: Option<Option<String>>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub milestone: Option<Option<i64>>,
}

/// Updates an issue (or the issue part of a PR): title/body/state for the author or triagers;
/// labels/assignees/milestone for triagers only (silently ignored otherwise, like GitHub).
pub async fn update_issue(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, input: UpdateIssue) -> ApiResult<IssueRow> {
    let triage = ctx.perm >= Perm::Triage;
    let mut tx = state.db.begin().await?;
    let cur: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE repo_id = $1 AND number = $2 FOR UPDATE")
        .bind(ctx.repo.id)
        .bind(number)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let is_author = cur.user_id == Some(actor.id);
    if !triage && !is_author {
        return Err(ApiError::forbidden("Must have triage access or be the author to update this issue."));
    }
    if let Some(s) = &input.state {
        if s != "open" && s != "closed" {
            return Err(invalid("Issue", "state", s.as_str()));
        }
    }
    if let Some(Some(r)) = &input.state_reason {
        if !matches!(r.as_str(), "completed" | "not_planned" | "reopened" | "duplicate") {
            return Err(invalid("Issue", "state_reason", r.as_str()));
        }
    }
    if let Some(t) = &input.title {
        if t.trim().is_empty() {
            return Err(ApiError::validation("Issue", "title", "missing_field"));
        }
        if *t != cur.title {
            let data = json!({ "rename": { "from": cur.title, "to": t } });
            record_event(&mut tx, ctx.repo.id, cur.id, Some(actor.id), "renamed", None, data).await?;
            sqlx::query("UPDATE issues SET title = $2 WHERE id = $1").bind(cur.id).bind(t).execute(&mut *tx).await?;
        }
    }
    if let Some(b) = &input.body {
        sqlx::query("UPDATE issues SET body = $2 WHERE id = $1").bind(cur.id).bind(b).execute(&mut *tx).await?;
    }
    let reason = input.state_reason.clone().flatten();
    let target = input.state.clone().or_else(|| match reason.as_deref() {
        Some("reopened") if cur.state == "closed" => Some("open".into()),
        _ => None,
    });
    match (cur.state.as_str(), target.as_deref()) {
        ("open", Some("closed")) => {
            let r = reason.clone().filter(|r| r != "reopened").unwrap_or_else(|| "completed".into());
            sqlx::query("UPDATE issues SET state = 'closed', state_reason = $2, closed_at = now(), closed_by_id = $3 WHERE id = $1")
                .bind(cur.id)
                .bind(&r)
                .bind(actor.id)
                .execute(&mut *tx)
                .await?;
            record_event(&mut tx, ctx.repo.id, cur.id, Some(actor.id), "closed", None, json!({ "state_reason": r })).await?;
        }
        ("closed", Some("open")) => {
            sqlx::query("UPDATE issues SET state = 'open', state_reason = 'reopened', closed_at = NULL, closed_by_id = NULL WHERE id = $1")
                .bind(cur.id)
                .execute(&mut *tx)
                .await?;
            record_event(&mut tx, ctx.repo.id, cur.id, Some(actor.id), "reopened", None, json!({ "state_reason": "reopened" }))
                .await?;
        }
        ("closed", _) => {
            if let Some(r) = reason.filter(|r| r != "reopened") {
                sqlx::query("UPDATE issues SET state_reason = $2 WHERE id = $1").bind(cur.id).bind(r).execute(&mut *tx).await?;
            }
        }
        _ => {}
    }
    if triage {
        if let Some(names) = &input.labels {
            let wanted = resolve_labels(&mut tx, ctx.repo.id, names, ctx.perm >= Perm::Write).await?;
            let current = issue_labels(&mut *tx, cur.id).await?;
            let remove: Vec<LabelRow> = current.iter().filter(|c| !wanted.iter().any(|w| w.id == c.id)).cloned().collect();
            detach_labels(&mut tx, ctx, actor, cur.id, &remove).await?;
            attach_labels(&mut tx, ctx, actor, cur.id, &wanted).await?;
        }
        if let Some(logins) = &input.assignees {
            let wanted = resolve_assignees(&mut tx, ctx, logins, true).await?;
            let current: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM issue_assignees WHERE issue_id = $1")
                .bind(cur.id)
                .fetch_all(&mut *tx)
                .await?;
            let remove: Vec<i64> = current.into_iter().filter(|c| !wanted.iter().any(|w| w.id == *c)).collect();
            detach_assignees(&mut tx, ctx, actor, cur.id, &remove).await?;
            attach_assignees(&mut tx, ctx, actor, cur.id, &wanted).await?;
        }
        if let Some(m) = input.milestone {
            let new = match m {
                Some(n) => Some(milestone_by_number(&mut *tx, ctx.repo.id, n).await?.ok_or_else(|| invalid("Issue", "milestone", n))?),
                None => None,
            };
            set_milestone(&mut tx, ctx, actor, &cur, new.as_ref()).await?;
        }
    }
    let row: IssueRow = sqlx::query_as("UPDATE issues SET updated_at = now() WHERE id = $1 RETURNING *")
        .bind(cur.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    if let Some(Some(b)) = &input.body {
        record_cross_references(state, ctx, actor, &row, b).await;
    }
    Ok(row)
}

/// Locks an issue's conversation. `reason`: off-topic | too heated | resolved | spam.
pub async fn lock_issue(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, reason: Option<String>) -> ApiResult<IssueRow> {
    ctx.require(Perm::Triage)?;
    if let Some(r) = &reason {
        if !matches!(r.as_str(), "off-topic" | "too heated" | "resolved" | "spam") {
            return Err(invalid("Issue", "lock_reason", r.as_str()));
        }
    }
    let mut tx = state.db.begin().await?;
    let cur = ji::issue_by_number(&state.db, ctx.repo.id, number).await?;
    let row: IssueRow = sqlx::query_as("UPDATE issues SET locked = TRUE, active_lock_reason = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(cur.id)
        .bind(&reason)
        .fetch_one(&mut *tx)
        .await?;
    if !cur.locked || cur.active_lock_reason != reason {
        record_event(&mut tx, ctx.repo.id, cur.id, Some(actor.id), "locked", None, json!({ "lock_reason": reason })).await?;
    }
    tx.commit().await?;
    Ok(row)
}

pub async fn unlock_issue(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64) -> ApiResult<IssueRow> {
    ctx.require(Perm::Triage)?;
    let mut tx = state.db.begin().await?;
    let cur = ji::issue_by_number(&state.db, ctx.repo.id, number).await?;
    let row: IssueRow =
        sqlx::query_as("UPDATE issues SET locked = FALSE, active_lock_reason = NULL, updated_at = now() WHERE id = $1 RETURNING *")
            .bind(cur.id)
            .fetch_one(&mut *tx)
            .await?;
    if cur.locked {
        record_event(&mut tx, ctx.repo.id, cur.id, Some(actor.id), "unlocked", None, json!({})).await?;
    }
    tx.commit().await?;
    Ok(row)
}

/// Adds assignees (up to 10 like GitHub); logins that can't be assigned are ignored.
pub async fn add_assignees(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, logins: Vec<String>) -> ApiResult<IssueRow> {
    let cur = ji::issue_by_number(&state.db, ctx.repo.id, number).await?;
    if ctx.perm >= Perm::Triage {
        let mut tx = state.db.begin().await?;
        let users = resolve_assignees(&mut tx, ctx, &logins, false).await?;
        let users: Vec<User> = users.into_iter().take(10).collect();
        attach_assignees(&mut tx, ctx, actor, cur.id, &users).await?;
        sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(cur.id).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    ji::issue_by_number(&state.db, ctx.repo.id, number).await
}

pub async fn remove_assignees(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, logins: Vec<String>) -> ApiResult<IssueRow> {
    let cur = ji::issue_by_number(&state.db, ctx.repo.id, number).await?;
    if ctx.perm >= Perm::Triage {
        let lower: Vec<String> = logins.iter().map(|l| l.to_lowercase()).collect();
        let mut tx = state.db.begin().await?;
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT a.user_id FROM issue_assignees a JOIN users u ON u.id = a.user_id
              WHERE a.issue_id = $1 AND lower(u.login) = ANY($2)",
        )
        .bind(cur.id)
        .bind(&lower)
        .fetch_all(&mut *tx)
        .await?;
        detach_assignees(&mut tx, ctx, actor, cur.id, &ids).await?;
        sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(cur.id).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    ji::issue_by_number(&state.db, ctx.repo.id, number).await
}

/// Adds labels (creating missing ones for push access). Returns the issue's labels.
pub async fn add_labels(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, names: Vec<String>) -> ApiResult<Vec<LabelRow>> {
    ctx.require(Perm::Triage)?;
    let cur = ji::issue_by_number(&state.db, ctx.repo.id, number).await?;
    let mut tx = state.db.begin().await?;
    let labels = resolve_labels(&mut tx, ctx.repo.id, &names, ctx.perm >= Perm::Write).await?;
    attach_labels(&mut tx, ctx, actor, cur.id, &labels).await?;
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(cur.id).execute(&mut *tx).await?;
    let out = issue_labels(&mut *tx, cur.id).await?;
    tx.commit().await?;
    Ok(out)
}

/// Replaces all labels. Returns the issue's labels.
pub async fn set_labels(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, names: Vec<String>) -> ApiResult<Vec<LabelRow>> {
    ctx.require(Perm::Triage)?;
    let cur = ji::issue_by_number(&state.db, ctx.repo.id, number).await?;
    let mut tx = state.db.begin().await?;
    let wanted = resolve_labels(&mut tx, ctx.repo.id, &names, ctx.perm >= Perm::Write).await?;
    let current = issue_labels(&mut *tx, cur.id).await?;
    let remove: Vec<LabelRow> = current.into_iter().filter(|c| !wanted.iter().any(|w| w.id == c.id)).collect();
    detach_labels(&mut tx, ctx, actor, cur.id, &remove).await?;
    attach_labels(&mut tx, ctx, actor, cur.id, &wanted).await?;
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(cur.id).execute(&mut *tx).await?;
    let out = issue_labels(&mut *tx, cur.id).await?;
    tx.commit().await?;
    Ok(out)
}

/// Removes one label (404 "Label does not exist" if it isn't on the issue). Returns the rest.
pub async fn remove_label(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, name: &str) -> ApiResult<Vec<LabelRow>> {
    ctx.require(Perm::Triage)?;
    let cur = ji::issue_by_number(&state.db, ctx.repo.id, number).await?;
    let mut tx = state.db.begin().await?;
    let current = issue_labels(&mut *tx, cur.id).await?;
    let Some(l) = current.iter().find(|l| l.name.eq_ignore_ascii_case(name)).cloned() else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "Label does not exist"));
    };
    detach_labels(&mut tx, ctx, actor, cur.id, &[l]).await?;
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(cur.id).execute(&mut *tx).await?;
    let out = issue_labels(&mut *tx, cur.id).await?;
    tx.commit().await?;
    Ok(out)
}

pub async fn remove_all_labels(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64) -> ApiResult<()> {
    set_labels(state, ctx, actor, number, vec![]).await.map(|_| ())
}

// ---------------------------------------------------------------------------------------------
// Handlers: single issue.

#[derive(Deserialize)]
struct CreateBody {
    title: Option<Value>,
    body: Option<String>,
    assignee: Option<String>,
    #[serde(default)]
    assignees: Vec<Value>,
    milestone: Option<Value>,
    #[serde(default)]
    labels: Vec<Value>,
}

async fn create_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<CreateBody>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let Some(title) = b.title.as_ref().and_then(value_to_text) else {
        return Err(ApiError::validation("Issue", "title", "missing_field"));
    };
    let mut assignees = login_names(&b.assignees);
    if let Some(a) = b.assignee.filter(|a| !a.is_empty()) {
        assignees.push(a);
    }
    let milestone = match &b.milestone {
        None | Some(Value::Null) => None,
        Some(v) => Some(value_to_number(v).ok_or_else(|| invalid("Issue", "milestone", v.clone()))?),
    };
    let input = CreateIssue { title, body: b.body, labels: label_names(&b.labels), assignees, milestone };
    let row = create_issue(&s, &ctx, &u, input).await?;
    Ok(created(issue_json(&s, &ctx, row).await?))
}

async fn get_handler(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let row = get_issue(&s, &ctx, number).await?;
    Ok(Json(issue_json(&s, &ctx, row).await?))
}

#[derive(Deserialize)]
struct UpdateBody {
    #[serde(default, deserialize_with = "double_opt")]
    title: Option<Option<Value>>,
    #[serde(default, deserialize_with = "double_opt")]
    body: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_opt")]
    assignee: Option<Option<String>>,
    state: Option<String>,
    #[serde(default, deserialize_with = "double_opt")]
    state_reason: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_opt")]
    milestone: Option<Option<Value>>,
    labels: Option<Vec<Value>>,
    assignees: Option<Vec<Value>>,
}

async fn update_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<UpdateBody>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let title = match &b.title {
        None | Some(None) => None,
        Some(Some(v)) => Some(value_to_text(v).ok_or_else(|| invalid("Issue", "title", v.clone()))?),
    };
    let milestone = match &b.milestone {
        None => None,
        Some(None) => Some(None),
        Some(Some(v)) => match v {
            Value::String(s) if s.is_empty() || s == "none" => Some(None),
            _ => Some(Some(value_to_number(v).ok_or_else(|| invalid("Issue", "milestone", v.clone()))?)),
        },
    };
    let assignees = match (&b.assignees, &b.assignee) {
        (Some(a), _) => Some(login_names(a)),
        (None, Some(Some(a))) => Some(vec![a.clone()]),
        (None, Some(None)) => Some(vec![]),
        (None, None) => None,
    };
    let input = UpdateIssue {
        title,
        body: b.body,
        state: b.state,
        state_reason: b.state_reason,
        labels: b.labels.as_deref().map(label_names),
        assignees,
        milestone,
    };
    let row = update_issue(&s, &ctx, &u, number, input).await?;
    Ok(Json(issue_json(&s, &ctx, row).await?))
}

#[derive(Deserialize, Default)]
struct LockBody {
    lock_reason: Option<String>,
}

async fn lock_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<Option<LockBody>>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    lock_issue(&s, &ctx, &u, number, b.unwrap_or_default().lock_reason).await?;
    Ok(no_content())
}

async fn unlock_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    unlock_issue(&s, &ctx, &u, number).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
struct AssigneesBody {
    #[serde(default)]
    assignees: Vec<Value>,
}

async fn add_assignees_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<AssigneesBody>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let row = add_assignees(&s, &ctx, &u, number, login_names(&b.assignees)).await?;
    Ok(created(issue_json(&s, &ctx, row).await?))
}

async fn remove_assignees_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<AssigneesBody>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let row = remove_assignees(&s, &ctx, &u, number, login_names(&b.assignees)).await?;
    Ok(Json(issue_json(&s, &ctx, row).await?))
}

/// Whether `login` can be assigned in the repository.
pub async fn is_assignable(state: &AppState, ctx: &RepoCtx, login: &str) -> ApiResult<bool> {
    let mut conn = state.db.acquire().await?;
    Ok(!resolve_assignees(&mut conn, ctx, &[login.to_string()], false).await?.is_empty())
}

async fn check_assignee(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, assignee)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    if is_assignable(&s, &ctx, &assignee).await? { Ok(no_content()) } else { Err(ApiError::not_found()) }
}

async fn check_assignee_for_issue(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number, assignee)): Path<(String, String, i64, String)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    get_issue(&s, &ctx, number).await?;
    if is_assignable(&s, &ctx, &assignee).await? { Ok(no_content()) } else { Err(ApiError::not_found()) }
}

async fn list_assignees(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let mut qb = QueryBuilder::new("SELECT u.* FROM users u WHERE ");
    push_assignable(&mut qb, &ctx);
    qb.push(" ORDER BY lower(u.login) LIMIT ").push_bind(p.limit() + 1).push(" OFFSET ").push_bind(p.offset());
    let users: Vec<User> = qb.build_query_as().fetch_all(&s.db).await?;
    let (users, total) = page_window(users, &p);
    let items = users.iter().map(|u| json::user::simple(&s.api_url(), s.base_url(), u)).collect();
    Ok(Paged::new(&uri, &s, p, total, items))
}

// Labels on an issue.

fn labels_json(s: &AppState, ctx: &RepoCtx, labels: &[LabelRow]) -> Vec<Value> {
    let full = ctx.full_name();
    labels.iter().map(|l| ji::label(&s.api_url(), &full, l)).collect()
}

/// Accepts every body shape GitHub documents: `{labels: [..]}`, `[..]`, `[{name}]`, `"name"`.
fn parse_label_body(v: Value) -> Vec<String> {
    match v {
        Value::Array(a) => label_names(&a),
        Value::Object(mut o) => match o.remove("labels") {
            Some(Value::Array(a)) => label_names(&a),
            Some(Value::String(s)) => vec![s],
            _ => vec![],
        },
        Value::String(s) => vec![s],
        _ => vec![],
    }
}

async fn list_issue_labels(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let issue = get_issue(&s, &ctx, number).await?;
    let labels = issue_labels(&s.db, issue.id).await?;
    let total = labels.len() as i64;
    let page: Vec<LabelRow> = labels.into_iter().skip(p.offset() as usize).take(p.per_page() as usize).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), labels_json(&s, &ctx, &page)))
}

async fn add_labels_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<Value>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let labels = add_labels(&s, &ctx, &u, number, parse_label_body(b)).await?;
    Ok(Json(Value::Array(labels_json(&s, &ctx, &labels))))
}

async fn set_labels_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<Value>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let labels = set_labels(&s, &ctx, &u, number, parse_label_body(b)).await?;
    Ok(Json(Value::Array(labels_json(&s, &ctx, &labels))))
}

async fn remove_all_labels_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    remove_all_labels(&s, &ctx, &u, number).await?;
    Ok(no_content())
}

async fn remove_label_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number, name)): Path<(String, String, i64, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let labels = remove_label(&s, &ctx, &u, number, &name).await?;
    Ok(Json(Value::Array(labels_json(&s, &ctx, &labels))))
}

// ---------------------------------------------------------------------------------------------
// Listings.

#[derive(Deserialize, Default, Clone)]
pub struct ListIssues {
    pub milestone: Option<String>,
    pub state: Option<String>,
    pub assignee: Option<String>,
    pub creator: Option<String>,
    pub mentioned: Option<String>,
    pub labels: Option<String>,
    pub sort: Option<String>,
    pub direction: Option<String>,
    pub since: Option<String>,
    pub filter: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl ListIssues {
    fn pages(&self) -> PageParams {
        PageParams { page: self.page, per_page: self.per_page }
    }
}

/// Regex (Postgres ARE) matching an @-mention of `login`.
pub fn mention_regex(login: &str) -> String {
    let safe: String = login.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    format!("(^|[^A-Za-z0-9_-])@{safe}($|[^A-Za-z0-9_-])")
}

async fn user_id_or_missing(s: &AppState, login: &str) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("SELECT id FROM users WHERE lower(login) = lower($1)")
        .bind(login)
        .fetch_optional(&s.db)
        .await?
        .unwrap_or(-1))
}

/// Pushes the shared filters (state, labels, since, creator, assignee, mentioned, milestone
/// for a single repo) as ` AND ...` clauses on alias `i`.
async fn push_filters(qb: &mut QueryBuilder<Postgres>, s: &AppState, q: &ListIssues, repo_id: Option<i64>) -> ApiResult<()> {
    match q.state.as_deref().unwrap_or("open") {
        "open" => {
            qb.push(" AND i.state = 'open'");
        }
        "closed" => {
            qb.push(" AND i.state = 'closed'");
        }
        "all" => {}
        other => return Err(invalid("Issue", "state", other)),
    }
    if let Some(labels) = q.labels.as_deref() {
        for name in labels.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            qb.push(" AND EXISTS(SELECT 1 FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = i.id AND lower(l.name) = lower(")
                .push_bind(name.to_string())
                .push("))");
        }
    }
    if let Some(since) = parse_since(&q.since)? {
        qb.push(" AND i.updated_at >= ").push_bind(since);
    }
    if let Some(c) = q.creator.as_deref().filter(|c| !c.is_empty()) {
        let id = user_id_or_missing(s, c).await?;
        qb.push(" AND i.user_id = ").push_bind(id);
    }
    match q.assignee.as_deref().filter(|a| !a.is_empty()) {
        None => {}
        Some("none") => {
            qb.push(" AND NOT EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id)");
        }
        Some("*") => {
            qb.push(" AND EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id)");
        }
        Some(login) => {
            let id = user_id_or_missing(s, login).await?;
            qb.push(" AND EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id AND a.user_id = ").push_bind(id).push(")");
        }
    }
    if let Some(m) = q.mentioned.as_deref().filter(|m| !m.is_empty()) {
        let re = mention_regex(m);
        qb.push(" AND (i.body ~* ")
            .push_bind(re.clone())
            .push(" OR EXISTS(SELECT 1 FROM issue_comments ic WHERE ic.issue_id = i.id AND ic.body ~* ")
            .push_bind(re)
            .push("))");
    }
    if let (Some(m), Some(repo_id)) = (q.milestone.as_deref().filter(|m| !m.is_empty()), repo_id) {
        match m {
            "none" => {
                qb.push(" AND i.milestone_id IS NULL");
            }
            "*" => {
                qb.push(" AND i.milestone_id IS NOT NULL");
            }
            n => {
                let n: i64 = n.parse().map_err(|_| invalid("Issue", "milestone", n))?;
                qb.push(" AND i.milestone_id = (SELECT id FROM milestones WHERE repo_id = ")
                    .push_bind(repo_id)
                    .push(" AND number = ")
                    .push_bind(n)
                    .push(")");
            }
        }
    }
    Ok(())
}

fn push_order(qb: &mut QueryBuilder<Postgres>, q: &ListIssues) {
    let col = match q.sort.as_deref() {
        Some("updated") => "i.updated_at",
        Some("comments") => "i.comments_count",
        _ => "i.created_at",
    };
    let dir = if q.direction.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    qb.push(format!(" ORDER BY {col} {dir}, i.id {dir}"));
}

async fn list_for_repo(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListIssues>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let p = q.pages();
    let mut qb = QueryBuilder::new("SELECT i.* FROM issues i WHERE i.repo_id = ");
    qb.push_bind(ctx.repo.id);
    push_filters(&mut qb, &s, &q, Some(ctx.repo.id)).await?;
    push_order(&mut qb, &q);
    qb.push(" LIMIT ").push_bind(p.limit() + 1).push(" OFFSET ").push_bind(p.offset());
    let rows: Vec<IssueRow> = qb.build_query_as().fetch_all(&s.db).await?;
    let (rows, total) = page_window(rows, &p);
    Ok(Paged::new(&uri, &s, p, total, issues_json(&s, &ctx, rows).await?))
}

/// Which repositories a cross-repo listing covers.
enum Scope {
    /// owned + collaborator + org-member repositories (`GET /issues`)
    All,
    /// owned + collaborator repositories (`GET /user/issues`)
    OwnedAndMember,
    /// repositories of one organization the viewer can access
    Org(i64),
}

async fn list_cross_repo(s: &AppState, uri: &OriginalUri, u: &User, q: ListIssues, scope: Scope) -> ApiResult<Paged> {
    let p = q.pages();
    let mut qb = QueryBuilder::new("SELECT i.* FROM issues i JOIN repositories r ON r.id = i.repo_id WHERE r.has_issues AND (");
    let uid = u.id;
    let push_member = |qb: &mut QueryBuilder<Postgres>| {
        qb.push("r.owner_id = ")
            .push_bind(uid)
            .push(" OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = ")
            .push_bind(uid)
            .push(")");
    };
    match scope {
        Scope::All => {
            push_member(&mut qb);
            qb.push(" OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = ").push_bind(uid).push(")");
        }
        Scope::OwnedAndMember => push_member(&mut qb),
        Scope::Org(org_id) => {
            qb.push("r.owner_id = ").push_bind(org_id).push(" AND (NOT r.private OR ");
            push_member(&mut qb);
            qb.push(" OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = ").push_bind(uid).push("))");
        }
    }
    qb.push(")");
    let re = mention_regex(&u.login);
    match q.filter.as_deref().unwrap_or("assigned") {
        "assigned" => {
            qb.push(" AND EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id AND a.user_id = ").push_bind(uid).push(")");
        }
        "created" => {
            qb.push(" AND i.user_id = ").push_bind(uid);
        }
        "mentioned" => {
            qb.push(" AND i.body ~* ").push_bind(re);
        }
        "subscribed" => {
            qb.push(" AND (i.user_id = ")
                .push_bind(uid)
                .push(" OR EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id AND a.user_id = ")
                .push_bind(uid)
                .push(") OR EXISTS(SELECT 1 FROM issue_comments ic WHERE ic.issue_id = i.id AND ic.user_id = ")
                .push_bind(uid)
                .push(") OR i.body ~* ")
                .push_bind(re)
                .push(")");
        }
        "repos" | "all" => {}
        other => return Err(invalid("Issue", "filter", other)),
    }
    push_filters(&mut qb, s, &q, None).await?;
    push_order(&mut qb, &q);
    qb.push(" LIMIT ").push_bind(p.limit() + 1).push(" OFFSET ").push_bind(p.offset());
    let rows: Vec<IssueRow> = qb.build_query_as().fetch_all(&s.db).await?;
    let (rows, total) = page_window(rows, &p);
    let items = issues_json_multi(s, Some(u), rows, true).await?;
    Ok(Paged::new(uri, s, p, total, items))
}

async fn list_for_viewer(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<ListIssues>) -> ApiResult<Paged> {
    list_cross_repo(&s, &uri, &u, q, Scope::All).await
}

async fn list_for_user(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<ListIssues>) -> ApiResult<Paged> {
    list_cross_repo(&s, &uri, &u, q, Scope::OwnedAndMember).await
}

async fn list_for_org(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(u): AuthUser,
    Path(org): Path<String>,
    Query(q): Query<ListIssues>,
) -> ApiResult<Paged> {
    let o = db::user_by_login(&s.db, &org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    list_cross_repo(&s, &uri, &u, q, Scope::Org(o.id)).await
}
