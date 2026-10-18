//! Sub-issues, as on api.github.com: `GET|POST /repos/{o}/{r}/issues/{n}/sub_issues`,
//! `DELETE …/sub_issue`, `PATCH …/sub_issues/priority` and `GET …/parent`. A sub-issue belongs
//! to the same repository owner as its parent, has at most one parent, and the hierarchy is
//! acyclic (at most 8 levels deep, 100 sub-issues per parent).
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::issue::{self as ji, IssueRow};
use crate::state::AppState;

const MAX_CHILDREN: i64 = 100;
const MAX_DEPTH: i64 = 8;

pub fn routes() -> Router<AppState> {
    let i = "/repos/{owner}/{repo}/issues/{number}";
    Router::new()
        .route(&format!("{i}/sub_issues"), get(list).post(add))
        .route(&format!("{i}/sub_issue"), delete(remove))
        .route(&format!("{i}/sub_issues/priority"), patch(reprioritize))
        .route(&format!("{i}/parent"), get(parent))
}

fn invalid(msg: &str) -> ApiError {
    let mut e = ApiError::unprocessable("Validation Failed");
    e.errors = Some(json!([{"resource": "SubIssue", "field": "sub_issue_id", "code": "custom", "message": msg}]));
    e
}

/// Renders issues (possibly from several repositories the viewer can read), keeping order.
async fn render(s: &AppState, viewer: Option<&User>, rows: Vec<IssueRow>) -> ApiResult<Vec<Value>> {
    let mut ctxs: HashMap<i64, Option<RepoCtx>> = HashMap::new();
    let mut out = Vec::new();
    for r in rows {
        if !ctxs.contains_key(&r.repo_id) {
            ctxs.insert(r.repo_id, db::load_repo_by_id(&s.db, viewer, r.repo_id).await.ok());
        }
        let Some(Some(ctx)) = ctxs.get(&r.repo_id) else { continue };
        out.push(super::issues::issue_json(s, ctx, r).await?);
    }
    Ok(out)
}

/// `sub_issues_summary` of an issue: `{total, completed, percent_completed}`.
pub async fn summary(s: &AppState, issue_id: i64) -> ApiResult<Value> {
    let (total, done): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE i.state = 'closed') FROM issue_sub_issues si JOIN issues i ON i.id = si.child_id WHERE si.parent_id = $1",
    )
    .bind(issue_id)
    .fetch_one(&s.db)
    .await?;
    let pct = if total == 0 { 0 } else { done * 100 / total };
    Ok(json!({"total": total, "completed": done, "percent_completed": pct}))
}

async fn list(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((o, r, n)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let issue = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    let rows: Vec<IssueRow> = sqlx::query_as(
        "SELECT i.* FROM issue_sub_issues si JOIN issues i ON i.id = si.child_id WHERE si.parent_id = $1 ORDER BY si.position, i.id LIMIT $2 OFFSET $3",
    )
    .bind(issue.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_sub_issues WHERE parent_id = $1").bind(issue.id).fetch_one(&s.db).await?;
    let items = render(&s, u.as_ref(), rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
struct AddBody {
    sub_issue_id: Option<i64>,
    #[serde(default)]
    replace_parent: bool,
}

async fn add(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<AddBody>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let parent = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    ctx.require(Perm::Triage)?;
    let id = b.sub_issue_id.ok_or_else(|| ApiError::validation("SubIssue", "sub_issue_id", "missing_field"))?;
    let child: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1")
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| invalid("The provided sub-issue does not exist"))?;
    let child_ctx = db::load_repo_by_id(&s.db, Some(&u), child.repo_id).await.map_err(|_| invalid("The provided sub-issue does not exist"))?;
    if child_ctx.repo.owner_id != ctx.repo.owner_id {
        return Err(invalid("Sub-issues must belong to the same repository owner as the parent issue"));
    }
    if child.is_pull {
        return Err(invalid("Pull requests cannot be added as sub-issues"));
    }
    if child.id == parent.id {
        return Err(invalid("An issue cannot be a sub-issue of itself"));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('issue_sub_issues', 0))").execute(&mut *tx).await?;
    // The child must not be an ancestor of the parent.
    let ancestors: Vec<i64> = sqlx::query_scalar(
        "WITH RECURSIVE up(id, depth) AS (SELECT parent_id, 1 FROM issue_sub_issues WHERE child_id = $1
            UNION SELECT si.parent_id, up.depth + 1 FROM issue_sub_issues si JOIN up ON si.child_id = up.id WHERE up.depth < 64)
         SELECT id FROM up",
    )
    .bind(parent.id)
    .fetch_all(&mut *tx)
    .await?;
    if ancestors.contains(&child.id) {
        return Err(invalid("Adding this sub-issue would create a cycle"));
    }
    // Depth: levels above the parent + the parent + the child's own subtree.
    let below: i64 = sqlx::query_scalar(
        "WITH RECURSIVE down(id, depth) AS (SELECT child_id, 1 FROM issue_sub_issues WHERE parent_id = $1
            UNION SELECT si.child_id, down.depth + 1 FROM issue_sub_issues si JOIN down ON si.parent_id = down.id WHERE down.depth < 64)
         SELECT COALESCE(MAX(depth), 0)::bigint FROM down",
    )
    .bind(child.id)
    .fetch_one(&mut *tx)
    .await?;
    if ancestors.len() as i64 + 2 + below > MAX_DEPTH {
        return Err(invalid("Sub-issues can be nested at most 8 levels deep"));
    }
    let existing: Option<i64> = sqlx::query_scalar("SELECT parent_id FROM issue_sub_issues WHERE child_id = $1").bind(child.id).fetch_optional(&mut *tx).await?;
    match existing {
        Some(p) if p == parent.id => return Err(invalid("This issue is already a sub-issue of the parent")),
        Some(_) if !b.replace_parent => return Err(invalid("The issue already has a parent issue")),
        Some(_) => {
            sqlx::query("DELETE FROM issue_sub_issues WHERE child_id = $1").bind(child.id).execute(&mut *tx).await?;
        }
        None => {}
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_sub_issues WHERE parent_id = $1").bind(parent.id).fetch_one(&mut *tx).await?;
    if count >= MAX_CHILDREN {
        return Err(invalid("An issue can have at most 100 sub-issues"));
    }
    sqlx::query(
        "INSERT INTO issue_sub_issues (parent_id, child_id, position, created_by)
         VALUES ($1, $2, (SELECT COALESCE(MAX(position), 0) + 1 FROM issue_sub_issues WHERE parent_id = $1), $3)",
    )
    .bind(parent.id)
    .bind(child.id)
    .bind(u.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(parent.id).execute(&mut *tx).await?;
    tx.commit().await?;
    let parent = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    let v = super::issues::issue_json(&s, &ctx, parent).await?;
    let mut res = (StatusCode::CREATED, axum::Json(v)).into_response();
    if let Ok(l) = format!("{}/repos/{}/issues/{n}/sub_issues", s.api_url(), ctx.full_name()).parse() {
        res.headers_mut().insert(axum::http::header::LOCATION, l);
    }
    Ok(res)
}

#[derive(Deserialize)]
struct RemoveBody {
    sub_issue_id: Option<i64>,
}

async fn remove(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<RemoveBody>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let parent = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    ctx.require(Perm::Triage)?;
    let id = b.sub_issue_id.ok_or_else(|| ApiError::validation("SubIssue", "sub_issue_id", "missing_field"))?;
    let n_rows = sqlx::query("DELETE FROM issue_sub_issues WHERE parent_id = $1 AND child_id = $2")
        .bind(parent.id)
        .bind(id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n_rows == 0 {
        return Err(ApiError::bad_request("The provided sub-issue is not a sub-issue of this issue"));
    }
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(parent.id).execute(&s.db).await?;
    let parent = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    Ok(Json(super::issues::issue_json(&s, &ctx, parent).await?))
}

#[derive(Deserialize)]
struct PriorityBody {
    sub_issue_id: Option<i64>,
    after_id: Option<i64>,
    before_id: Option<i64>,
}

async fn reprioritize(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<PriorityBody>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let parent = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    ctx.require(Perm::Triage)?;
    let id = b.sub_issue_id.ok_or_else(|| ApiError::validation("SubIssue", "sub_issue_id", "missing_field"))?;
    let (anchor, after) = match (b.after_id, b.before_id) {
        (Some(a), None) => (a, true),
        (None, Some(bf)) => (bf, false),
        _ => return Err(invalid("Specify exactly one of after_id or before_id")),
    };
    let rows: Vec<(i64, f64)> = sqlx::query_as("SELECT child_id, position FROM issue_sub_issues WHERE parent_id = $1 ORDER BY position, child_id")
        .bind(parent.id)
        .fetch_all(&s.db)
        .await?;
    if !rows.iter().any(|(c, _)| *c == id) {
        return Err(ApiError::not_found());
    }
    let mut order: Vec<i64> = rows.iter().map(|(c, _)| *c).filter(|c| *c != id).collect();
    let at = order.iter().position(|c| *c == anchor).ok_or_else(|| invalid("The anchor issue is not a sub-issue of this issue"))?;
    order.insert(if after { at + 1 } else { at }, id);
    let mut tx = s.db.begin().await?;
    for (i, c) in order.iter().enumerate() {
        sqlx::query("UPDATE issue_sub_issues SET position = $3 WHERE parent_id = $1 AND child_id = $2")
            .bind(parent.id)
            .bind(c)
            .bind((i + 1) as f64)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(super::issues::issue_json(&s, &ctx, parent).await?))
}

async fn parent(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r, n)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let issue = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    let row: Option<IssueRow> =
        sqlx::query_as("SELECT i.* FROM issue_sub_issues si JOIN issues i ON i.id = si.parent_id WHERE si.child_id = $1").bind(issue.id).fetch_optional(&s.db).await?;
    let row = row.ok_or_else(ApiError::not_found)?;
    let v = render(&s, u.as_ref(), vec![row]).await?.pop().ok_or_else(ApiError::not_found)?;
    Ok(Json(v))
}
