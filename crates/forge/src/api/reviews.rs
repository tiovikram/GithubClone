//! Pull request reviews and review comments REST API. Logic lives in `crate::pulls::reviews`.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::pull as pj;
use crate::pulls::reviews::{self, CreateReview, NewReviewComment, ReviewCommentRow, ReviewRow};
use crate::pulls::{self, Pull};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/pulls/{number}/reviews", get(list_reviews).post(create_review))
        .route(
            "/repos/{owner}/{repo}/pulls/{number}/reviews/{id}",
            get(get_review).put(update_review).delete(delete_review),
        )
        .route("/repos/{owner}/{repo}/pulls/{number}/reviews/{id}/comments", get(review_comments))
        .route("/repos/{owner}/{repo}/pulls/{number}/reviews/{id}/events", post(submit_review))
        .route("/repos/{owner}/{repo}/pulls/{number}/reviews/{id}/dismissals", put(dismiss_review))
        .route("/repos/{owner}/{repo}/pulls/{number}/comments", get(list_comments).post(create_comment))
        .route("/repos/{owner}/{repo}/pulls/{number}/comments/{id}/replies", post(reply))
        .route("/repos/{owner}/{repo}/pulls/comments", get(list_repo_comments))
        .route(
            "/repos/{owner}/{repo}/pulls/comments/{id}",
            get(get_comment).patch(update_comment).delete(delete_comment),
        )
}

type PullPath = Path<(String, String, i64)>;
type SubPath = Path<(String, String, i64, i64)>;

async fn render_reviews(s: &AppState, ctx: &RepoCtx, number: i64, rows: &[ReviewRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.user_id).collect();
    let a = pj::authors(&s.db, ctx, &ids).await?;
    Ok(rows.iter().map(|r| pj::review(&s.api_url(), s.base_url(), ctx, number, r, &a)).collect())
}

async fn render_review(s: &AppState, ctx: &RepoCtx, number: i64, r: &ReviewRow) -> ApiResult<Json<Value>> {
    Ok(Json(render_reviews(s, ctx, number, std::slice::from_ref(r)).await?.pop().unwrap_or(Value::Null)))
}

/// Renders review comments (any PRs of `ctx`). `legacy` = `review-comment` schema.
async fn render_comments(s: &AppState, ctx: &RepoCtx, rows: &[ReviewCommentRow], legacy: bool) -> ApiResult<Vec<Value>> {
    let issue_ids: Vec<i64> = rows.iter().map(|c| c.issue_id).collect();
    let numbers: Vec<(i64, i64)> = sqlx::query_as("SELECT id, number FROM issues WHERE id = ANY($1)").bind(&issue_ids).fetch_all(&s.db).await?;
    let numbers: HashMap<i64, i64> = numbers.into_iter().collect();
    let uids: Vec<i64> = rows.iter().filter_map(|c| c.user_id).collect();
    let a = pj::authors(&s.db, ctx, &uids).await?;
    let ids: Vec<i64> = rows.iter().map(|c| c.id).collect();
    let reactions = crate::json::issue::reaction_counts(&s.db, "review_comment", &ids).await?;
    let empty = HashMap::new();
    let api = s.api_url();
    Ok(rows
        .iter()
        .map(|c| {
            let n = numbers.get(&c.issue_id).copied().unwrap_or(0);
            let r = reactions.get(&c.id).unwrap_or(&empty);
            if legacy {
                pj::review_comment_legacy(&api, s.base_url(), ctx, n, c, &a, r)
            } else {
                pj::review_comment(&api, s.base_url(), ctx, n, c, &a, r)
            }
        })
        .collect())
}

async fn render_comment(s: &AppState, ctx: &RepoCtx, c: &ReviewCommentRow) -> ApiResult<Value> {
    Ok(render_comments(s, ctx, std::slice::from_ref(c), false).await?.pop().unwrap_or(Value::Null))
}

async fn load(s: &AppState, viewer: Option<&User>, owner: &str, repo: &str, number: i64) -> ApiResult<(RepoCtx, Pull)> {
    let ctx = db::load_repo(&s.db, viewer, owner, repo).await?;
    let pull = pulls::pull_by_number(&s.db, ctx.repo.id, number).await?;
    Ok((ctx, pull))
}

// ---------------------------------------------------------------------------------------------
// Reviews

async fn list_reviews(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo, number)): PullPath, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (ctx, pull) = load(&s, u.as_ref(), &owner, &repo, number).await?;
    let (rows, total) = reviews::list_reviews(&s.db, &pull, u.as_ref(), p.limit(), p.offset()).await?;
    let items = render_reviews(&s, &ctx, number, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create_review(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number)): PullPath, Json(input): Json<CreateReview>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let r = reviews::create_review(&s, &ctx, &u, number, input).await?;
    render_review(&s, &ctx, number, &r).await
}

async fn visible_review(s: &AppState, viewer: Option<&User>, pull: &Pull, id: i64) -> ApiResult<ReviewRow> {
    let r = reviews::review_by_id(&s.db, pull, id).await?;
    if r.state == "PENDING" && r.user_id != viewer.map(|u| u.id) {
        return Err(ApiError::not_found());
    }
    Ok(r)
}

async fn get_review(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, number, id)): SubPath) -> ApiResult<Json<Value>> {
    let (ctx, pull) = load(&s, u.as_ref(), &owner, &repo, number).await?;
    let r = visible_review(&s, u.as_ref(), &pull, id).await?;
    render_review(&s, &ctx, number, &r).await
}

#[derive(Deserialize, Default)]
struct BodyOnly {
    #[serde(default)]
    body: String,
}

async fn update_review(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number, id)): SubPath, Json(b): Json<BodyOnly>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let r = reviews::update_review(&s, &ctx, &u, number, id, &b.body).await?;
    render_review(&s, &ctx, number, &r).await
}

async fn delete_review(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number, id)): SubPath) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let r = reviews::delete_pending_review(&s, &ctx, &u, number, id).await?;
    render_review(&s, &ctx, number, &r).await
}

#[derive(Deserialize, Default)]
struct SubmitBody {
    body: Option<String>,
    event: Option<String>,
}

async fn submit_review(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number, id)): SubPath, Json(b): Json<SubmitBody>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let event = b.event.as_deref().filter(|e| !e.is_empty()).ok_or_else(|| reviews::simple_error("Event is required"))?;
    let r = reviews::submit_review(&s, &ctx, &u, number, id, Some(event), b.body).await?;
    render_review(&s, &ctx, number, &r).await
}

#[derive(Deserialize, Default)]
struct DismissBody {
    #[serde(default)]
    message: String,
}

async fn dismiss_review(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number, id)): SubPath, Json(b): Json<DismissBody>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let r = reviews::dismiss_review(&s, &ctx, &u, number, id, &b.message).await?;
    render_review(&s, &ctx, number, &r).await
}

async fn review_comments(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo, number, id)): SubPath, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (ctx, pull) = load(&s, u.as_ref(), &owner, &repo, number).await?;
    let r = visible_review(&s, u.as_ref(), &pull, id).await?;
    let rows: Vec<ReviewCommentRow> = sqlx::query_as("SELECT * FROM review_comments WHERE review_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(r.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM review_comments WHERE review_id = $1").bind(r.id).fetch_one(&s.db).await?;
    let items = render_comments(&s, &ctx, &rows, true).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

// ---------------------------------------------------------------------------------------------
// Review comments

#[derive(Deserialize, Default)]
struct CommentsQuery {
    sort: Option<String>,
    direction: Option<String>,
    since: Option<DateTime<Utc>>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn query_comments(s: &AppState, repo_id: i64, issue_id: Option<i64>, viewer: Option<&User>, q: &CommentsQuery) -> ApiResult<(Vec<ReviewCommentRow>, i64)> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let col = match q.sort.as_deref() {
        Some("updated") => "rc.updated_at",
        _ => "rc.created_at",
    };
    let dir = match q.direction.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };
    let from = "FROM review_comments rc LEFT JOIN pull_reviews r ON r.id = rc.review_id
                 WHERE rc.repo_id = $1 AND ($2::bigint IS NULL OR rc.issue_id = $2)
                   AND (r.state IS NULL OR r.state <> 'PENDING' OR rc.user_id = $3)
                   AND ($4::timestamptz IS NULL OR rc.updated_at >= $4)";
    let rows: Vec<ReviewCommentRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT rc.* {from} ORDER BY {col} {dir}, rc.id {dir} LIMIT $5 OFFSET $6")))
        .bind(repo_id)
        .bind(issue_id)
        .bind(viewer.map(|u| u.id).unwrap_or(-1))
        .bind(q.since)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}")))
        .bind(repo_id)
        .bind(issue_id)
        .bind(viewer.map(|u| u.id).unwrap_or(-1))
        .bind(q.since)
        .fetch_one(&s.db)
        .await?;
    Ok((rows, total))
}

async fn list_comments(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo, number)): PullPath, Query(q): Query<CommentsQuery>) -> ApiResult<Paged> {
    let (ctx, pull) = load(&s, u.as_ref(), &owner, &repo, number).await?;
    let (rows, total) = query_comments(&s, ctx.repo.id, Some(pull.issue.id), u.as_ref(), &q).await?;
    let items = render_comments(&s, &ctx, &rows, false).await?;
    Ok(Paged::new(&uri, &s, PageParams { page: q.page, per_page: q.per_page }, Some(total), items))
}

async fn list_repo_comments(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>, Query(q): Query<CommentsQuery>) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let (rows, total) = query_comments(&s, ctx.repo.id, None, u.as_ref(), &q).await?;
    let items = render_comments(&s, &ctx, &rows, false).await?;
    Ok(Paged::new(&uri, &s, PageParams { page: q.page, per_page: q.per_page }, Some(total), items))
}

async fn create_comment(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number)): PullPath, Json(input): Json<NewReviewComment>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let c = reviews::add_review_comment(&s, &ctx, &u, number, input).await?;
    Ok(created(render_comment(&s, &ctx, &c).await?))
}

async fn reply(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number, id)): SubPath, Json(b): Json<BodyOnly>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let input = NewReviewComment { body: b.body, in_reply_to: Some(id), ..Default::default() };
    let c = reviews::add_review_comment(&s, &ctx, &u, number, input).await?;
    Ok(created(render_comment(&s, &ctx, &c).await?))
}

async fn get_comment(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): PullPath) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let c = reviews::comment_by_id(&s.db, ctx.repo.id, id).await?;
    if !reviews::comment_visible(&s.db, &c, u.as_ref()).await? {
        return Err(ApiError::not_found());
    }
    Ok(Json(render_comment(&s, &ctx, &c).await?))
}

async fn update_comment(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): PullPath, Json(b): Json<BodyOnly>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let c = reviews::update_review_comment(&s, &ctx, &u, id, &b.body).await?;
    Ok(Json(render_comment(&s, &ctx, &c).await?))
}

async fn delete_comment(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): PullPath) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    reviews::delete_review_comment(&s, &ctx, &u, id).await?;
    Ok(no_content())
}
