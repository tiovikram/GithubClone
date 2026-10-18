//! Issue extras: pinned comments (`/issues/comments/{id}/pin`), issue dependencies
//! (`/issues/{n}/dependencies/blocked_by|blocking`) and issue field values.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::issue::{self as ji, IssueRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let i = "/repos/{owner}/{repo}/issues";
    Router::new()
        .route(&format!("{i}/comments/{{comment_id}}/pin"), put(pin).delete(unpin))
        .route(&format!("{i}/{{number}}/dependencies/blocked_by"), get(list_blocked_by).post(add_blocked_by))
        .route(&format!("{i}/{{number}}/dependencies/blocked_by/{{issue_id}}"), delete(remove_blocked_by))
        .route(&format!("{i}/{{number}}/dependencies/blocking"), get(list_blocking))
        .route(&format!("{i}/{{number}}/issue-field-values"), get(field_values))
}

// ---------------------------------------------------------------------------------------------
// Pinned comments

/// `pin` object of a pinned comment (`nullable-pinned-issue-comment`), if pinned.
pub async fn pin_of(s: &AppState, comment_id: i64) -> ApiResult<Option<Value>> {
    let row: Option<(chrono::DateTime<chrono::Utc>, Option<i64>)> =
        sqlx::query_as("SELECT pinned_at, pinned_by FROM issue_comment_pins WHERE comment_id = $1").bind(comment_id).fetch_optional(&s.db).await?;
    let Some((at, by)) = row else { return Ok(None) };
    let user = match by {
        Some(id) => db::user_by_id(&s.db, id).await.ok(),
        None => None,
    };
    Ok(Some(json!({
        "pinned_at": crate::json::ts(at),
        "pinned_by": user.map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), &u)),
    })))
}

/// Pins a comment on its issue (one pinned comment per issue; pinning replaces). Triage+.
pub async fn pin_comment(s: &AppState, ctx: &RepoCtx, actor: &User, comment_id: i64) -> ApiResult<ji::CommentRow> {
    ctx.require(Perm::Triage)?;
    let c = super::issue_comments::comment_by_id(s, ctx, comment_id).await?;
    let locked: bool = sqlx::query_scalar("SELECT locked FROM issues WHERE id = $1").bind(c.issue_id).fetch_one(&s.db).await?;
    if locked && ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Issue is locked"));
    }
    sqlx::query(
        "INSERT INTO issue_comment_pins (issue_id, comment_id, pinned_by) VALUES ($1,$2,$3)
         ON CONFLICT (issue_id) DO UPDATE SET comment_id = $2, pinned_by = $3, pinned_at = now()",
    )
    .bind(c.issue_id)
    .bind(c.id)
    .bind(actor.id)
    .execute(&s.db)
    .await?;
    Ok(c)
}

async fn pin(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let c = pin_comment(&s, &ctx, &u, id).await?;
    let mut v = super::issue_comments::comment_json(&s, &ctx, c).await?;
    v["pin"] = pin_of(&s, id).await?.unwrap_or(Value::Null);
    Ok(Json(v))
}

async fn unpin(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Triage)?;
    let c = super::issue_comments::comment_by_id(&s, &ctx, id).await?;
    let n = sqlx::query("DELETE FROM issue_comment_pins WHERE comment_id = $1").bind(c.id).execute(&s.db).await?.rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Dependencies

/// Renders issues that may belong to several repositories (grouped per repository).
async fn render_issues(s: &AppState, viewer: Option<&User>, rows: Vec<IssueRow>) -> ApiResult<Vec<Value>> {
    let mut ctxs: HashMap<i64, Option<RepoCtx>> = HashMap::new();
    for r in &rows {
        if !ctxs.contains_key(&r.repo_id) {
            ctxs.insert(r.repo_id, db::load_repo_by_id(&s.db, viewer, r.repo_id).await.ok());
        }
    }
    let mut out = Vec::new();
    for r in rows {
        let Some(Some(ctx)) = ctxs.get(&r.repo_id) else { continue };
        out.push(super::issues::issue_json(s, ctx, r).await?);
    }
    Ok(out)
}

async fn deps(s: &AppState, viewer: Option<&User>, issue_id: i64, blocked_by: bool, p: PageParams) -> ApiResult<(Vec<Value>, i64)> {
    let (sel, filt) = if blocked_by { ("blocking_id", "blocked_id") } else { ("blocked_id", "blocking_id") };
    let sql = format!("SELECT i.* FROM issue_dependencies d JOIN issues i ON i.id = d.{sel} WHERE d.{filt} = $1 ORDER BY d.created_at, i.id LIMIT $2 OFFSET $3");
    let rows: Vec<IssueRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(issue_id).bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM issue_dependencies WHERE {filt} = $1"))).bind(issue_id).fetch_one(&s.db).await?;
    Ok((render_issues(s, viewer, rows).await?, total))
}

/// Records that `blocked` is blocked by `blocking` (issue ids). Needs triage on `ctx` and read
/// access to the blocking issue's repository.
pub async fn add_dependency(s: &AppState, ctx: &RepoCtx, actor: &User, blocked: &IssueRow, blocking_id: i64) -> ApiResult<IssueRow> {
    ctx.require(Perm::Triage)?;
    let blocking: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1")
        .bind(blocking_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| ApiError::validation("IssueDependency", "issue_id", "invalid"))?;
    db::load_repo_by_id(&s.db, Some(actor), blocking.repo_id).await.map_err(|_| ApiError::validation("IssueDependency", "issue_id", "invalid"))?;
    let invalid = |msg: &str| {
        let mut e = ApiError::unprocessable("Validation Failed");
        e.errors = Some(json!([{"resource": "IssueDependency", "field": "issue_id", "code": "custom", "message": msg}]));
        e
    };
    if blocking.id == blocked.id {
        return Err(invalid("An issue cannot be blocked by itself"));
    }
    // No cycles: `blocking` must not (transitively) be blocked by `blocked`.
    let cycle: Option<i64> = sqlx::query_scalar(
        "WITH RECURSIVE up(id) AS (SELECT blocking_id FROM issue_dependencies WHERE blocked_id = $1
            UNION SELECT d.blocking_id FROM issue_dependencies d JOIN up ON d.blocked_id = up.id)
         SELECT 1::bigint FROM up WHERE id = $2 LIMIT 1",
    )
    .bind(blocking.id)
    .bind(blocked.id)
    .fetch_optional(&s.db)
    .await?;
    if cycle.is_some() {
        return Err(invalid("Adding this dependency would create a cycle"));
    }
    let n = sqlx::query("INSERT INTO issue_dependencies (blocked_id, blocking_id, created_by) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(blocked.id)
        .bind(blocking.id)
        .bind(actor.id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(invalid("This dependency already exists"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_dependencies WHERE blocked_id = $1").bind(blocked.id).fetch_one(&s.db).await?;
    if count > 50 {
        sqlx::query("DELETE FROM issue_dependencies WHERE blocked_id = $1 AND blocking_id = $2").bind(blocked.id).bind(blocking.id).execute(&s.db).await?;
        return Err(invalid("An issue can be blocked by at most 50 issues"));
    }
    Ok(blocking)
}

async fn list_blocked_by(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((o, r, n)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let i = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    let (items, total) = deps(&s, u.as_ref(), i.id, true, p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn list_blocking(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((o, r, n)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let i = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    let (items, total) = deps(&s, u.as_ref(), i.id, false, p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
struct AddDep {
    issue_id: Option<i64>,
}

async fn add_blocked_by(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<AddDep>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let i = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    let id = b.issue_id.ok_or_else(|| ApiError::validation("IssueDependency", "issue_id", "missing_field"))?;
    let blocking = add_dependency(&s, &ctx, &u, &i, id).await?;
    let v = render_issues(&s, Some(&u), vec![blocking]).await?.pop().ok_or_else(ApiError::not_found)?;
    let mut res = (StatusCode::CREATED, axum::Json(v)).into_response();
    if let Ok(l) = format!("{}/repos/{}/issues/{n}/dependencies/blocked_by", s.api_url(), ctx.full_name()).parse() {
        res.headers_mut().insert(axum::http::header::LOCATION, l);
    }
    Ok(res)
}

async fn remove_blocked_by(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n, id)): Path<(String, String, i64, i64)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Triage)?;
    let i = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    let blocking: IssueRow = sqlx::query_as(
        "DELETE FROM issue_dependencies d USING issues i WHERE d.blocked_id = $1 AND d.blocking_id = $2 AND i.id = d.blocking_id RETURNING i.*",
    )
    .bind(i.id)
    .bind(id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(ApiError::not_found)?;
    Ok(Json(render_issues(&s, Some(&u), vec![blocking]).await?.pop().ok_or_else(ApiError::not_found)?))
}

/// `issue_dependencies_summary` of an issue.
pub async fn dependencies_summary(s: &AppState, issue_id: i64) -> ApiResult<Value> {
    let (by, open_by, blocking): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM issue_dependencies WHERE blocked_id = $1),
                (SELECT count(*) FROM issue_dependencies d JOIN issues i ON i.id = d.blocking_id WHERE d.blocked_id = $1 AND i.state = 'open'),
                (SELECT count(*) FROM issue_dependencies WHERE blocking_id = $1)",
    )
    .bind(issue_id)
    .fetch_one(&s.db)
    .await?;
    Ok(json!({"blocked_by": open_by, "total_blocked_by": by, "blocking": blocking, "total_blocking": blocking}))
}

// ---------------------------------------------------------------------------------------------
// Issue field values (definitions are organization issue fields)

async fn field_values(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((o, r, n)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let i = ji::issue_by_number(&s.db, ctx.repo.id, n).await?;
    #[derive(sqlx::FromRow)]
    struct Row {
        field_id: i64,
        field_name: String,
        data_type: String,
        value: Option<Value>,
    }
    let rows: Vec<Row> = sqlx::query_as("SELECT field_id, field_name, data_type, value FROM issue_field_values WHERE issue_id = $1 ORDER BY field_id LIMIT $2 OFFSET $3")
        .bind(i.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_field_values WHERE issue_id = $1").bind(i.id).fetch_one(&s.db).await?;
    let items = rows
        .into_iter()
        .map(|r| {
            let mut v = json!({"issue_field_id": r.field_id, "issue_field_name": r.field_name, "node_id": crate::ids::encode("IssueFieldValue", r.field_id),
                "data_type": r.data_type, "value": Value::Null});
            match (r.data_type.as_str(), r.value) {
                ("single_select", Some(opt)) => v["single_select_option"] = opt,
                ("multi_select", Some(opts)) => v["multi_select_options"] = opts,
                (_, Some(val)) => v["value"] = val,
                _ => {}
            }
            v
        })
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}
