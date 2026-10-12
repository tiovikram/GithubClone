//! Pull requests REST API: list/create/get/update, commits, files, merge, update-branch,
//! requested reviewers, PRs associated with a commit. Business logic lives in `crate::pulls`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::issue::IssueRow;
use crate::pulls::{self, CreatePull, MergeInput, MergeMethod, Pull, UpdatePull};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/pulls", get(list).post(create))
        .route("/repos/{owner}/{repo}/pulls/{number}", get(get_one).patch(update))
        .route("/repos/{owner}/{repo}/pulls/{number}/commits", get(commits))
        .route("/repos/{owner}/{repo}/pulls/{number}/files", get(files))
        .route("/repos/{owner}/{repo}/pulls/{number}/merge", get(check_merged).put(merge))
        .route("/repos/{owner}/{repo}/pulls/{number}/update-branch", put(update_branch))
        .route(
            "/repos/{owner}/{repo}/pulls/{number}/requested_reviewers",
            get(list_reviewers).post(add_reviewers).delete(remove_reviewers),
        )
}

/// Per-ref routes, mounted behind `ref_dispatch` (refs may contain `/`).
pub fn commit_ref_routes() -> Router<AppState> {
    Router::new().route("/repos/{owner}/{repo}/commits/{sha}/pulls", get(for_commit))
}

type RepoPath = Path<(String, String)>;
type PullPath = Path<(String, String, i64)>;

pub(crate) async fn render_simple(s: &AppState, ctx: &RepoCtx, viewer: Option<&User>, pulls: Vec<Pull>) -> ApiResult<Vec<Value>> {
    let full = crate::json::pull::load(s, ctx, viewer, pulls, false).await?;
    Ok(full.iter().map(|p| crate::json::pull::simple_listed(&s.api_url(), s.base_url(), ctx, p)).collect())
}

pub(crate) async fn render_full(s: &AppState, ctx: &RepoCtx, viewer: Option<&User>, pull: Pull) -> ApiResult<Value> {
    let pull = pulls::ensure_fresh(s, pull).await?;
    let p = crate::json::pull::load_one(s, ctx, viewer, pull, true).await?;
    Ok(crate::json::pull::full(&s.api_url(), s.base_url(), ctx, &p))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    state: Option<String>,
    head: Option<String>,
    base: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo)): RepoPath, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let pages = PageParams { page: q.page, per_page: q.per_page };
    let state_cond = match q.state.as_deref().unwrap_or("open") {
        "open" => "i.state = 'open'",
        "closed" => "i.state = 'closed'",
        "all" => "TRUE",
        _ => return Err(ApiError::validation("PullRequest", "state", "invalid")),
    };
    let (head_owner, head_ref) = match q.head.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => match h.split_once(':') {
            Some((o, b)) => (Some(o.to_string()), Some(b.to_string()).filter(|b| !b.is_empty())),
            None => (Some(h.to_string()), None),
        },
        None => (None, None),
    };
    let (col, default_dir) = match q.sort.as_deref().unwrap_or("created") {
        "created" => ("i.created_at", "DESC"),
        "updated" => ("i.updated_at", "ASC"),
        "popularity" => ("i.comments_count", "ASC"),
        "long-running" => ("i.created_at", "ASC"),
        _ => return Err(ApiError::validation("PullRequest", "sort", "invalid")),
    };
    let dir = match q.direction.as_deref() {
        Some("asc") => "ASC",
        Some("desc") => "DESC",
        None => default_dir,
        _ => return Err(ApiError::validation("PullRequest", "direction", "invalid")),
    };
    let from = format!(
        "FROM issues i JOIN pull_requests p ON p.issue_id = i.id
           LEFT JOIN repositories hr ON hr.id = p.head_repo_id LEFT JOIN users ho ON ho.id = hr.owner_id
          WHERE i.repo_id = $1 AND i.is_pull AND {state_cond}
            AND ($2::text IS NULL OR lower(ho.login) = lower($2))
            AND ($3::text IS NULL OR p.head_ref = $3)
            AND ($4::text IS NULL OR p.base_ref = $4)"
    );
    let sql = format!("SELECT i.* {from} ORDER BY {col} {dir}, i.id {dir} LIMIT $5 OFFSET $6");
    let rows: Vec<IssueRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ctx.repo.id)
        .bind(&head_owner)
        .bind(&head_ref)
        .bind(&q.base)
        .bind(pages.limit())
        .bind(pages.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}")))
        .bind(ctx.repo.id)
        .bind(&head_owner)
        .bind(&head_ref)
        .bind(&q.base)
        .fetch_one(&s.db)
        .await?;
    let pulls = pulls::pulls_for_issues(&s.db, rows).await?;
    let items = render_simple(&s, &ctx, u.as_ref(), pulls).await?;
    Ok(Paged::new(&uri, &s, pages, Some(total), items))
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): RepoPath, Json(input): Json<CreatePull>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let pull = pulls::create_pull(&s, &ctx, &u, input).await?;
    Ok(created(render_full(&s, &ctx, Some(&u), pull).await?))
}

/// `Accept: application/vnd.github(.v3).diff|patch` media types.
fn wants(headers: &HeaderMap) -> Option<&'static str> {
    let accept = headers.get(header::ACCEPT)?.to_str().ok()?.to_ascii_lowercase();
    if !accept.contains("application/vnd.github") {
        return None;
    }
    if accept.contains(".diff") {
        Some("diff")
    } else if accept.contains(".patch") {
        Some("patch")
    } else {
        None
    }
}

async fn get_one(State(s): State<AppState>, MaybeUser(u): MaybeUser, headers: HeaderMap, Path((owner, repo, number)): PullPath) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let pull = pulls::pull_by_number(&s.db, ctx.repo.id, number).await?;
    if let Some(kind) = wants(&headers) {
        let path = pulls::repo_for_reading(&s, &pull).await?;
        let g = &s.git.git;
        let body = if kind == "diff" {
            pulls::git::raw_diff(g, &path, pull.diff_base(), &pull.pr.head_sha, None).await?
        } else {
            pulls::git::format_patch(g, &path, &pull.range()).await?
        };
        let ct = format!("application/vnd.github.{kind}; charset=utf-8");
        return Ok(([(header::CONTENT_TYPE, ct)], body).into_response());
    }
    Ok(Json(render_full(&s, &ctx, u.as_ref(), pull).await?).into_response())
}

async fn update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number)): PullPath, Json(input): Json<UpdatePull>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let pull = pulls::update_pull(&s, &ctx, &u, number, input).await?;
    Ok(Json(render_full(&s, &ctx, Some(&u), pull).await?))
}

async fn commits(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo, number)): PullPath, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let pull = pulls::pull_by_number(&s.db, ctx.repo.id, number).await?;
    let all = pulls::commits(&s, &pull).await?;
    let total = all.len() as i64;
    let page: Vec<_> = all.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    let mut emails: Vec<String> = page.iter().flat_map(|c| [c.author.email.clone(), c.committer.email.clone()]).collect();
    emails.sort();
    emails.dedup();
    let users = crate::json::pull::users_by_emails(&s.db, &emails).await?;
    let full_name = ctx.full_name();
    let items = page.iter().map(|c| crate::json::pull::commit(&s.api_url(), s.base_url(), &full_name, c, &users)).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn files(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo, number)): PullPath, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let pull = pulls::pull_by_number(&s.db, ctx.repo.id, number).await?;
    let all = pulls::files(&s, &pull).await?;
    let total = all.len().min(3000) as i64;
    let full_name = ctx.full_name();
    let items = all
        .iter()
        .take(3000)
        .skip(p.offset() as usize)
        .take(p.limit() as usize)
        .map(|f| crate::json::pull::diff_entry(&s.api_url(), s.base_url(), &full_name, &pull.pr.head_sha, f))
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn check_merged(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, number)): PullPath) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let pull = pulls::pull_by_number(&s.db, ctx.repo.id, number).await?;
    if pull.pr.merged {
        Ok(no_content())
    } else {
        Err(ApiError::not_found())
    }
}

#[derive(Deserialize, Default)]
struct MergeBody {
    commit_title: Option<String>,
    commit_message: Option<String>,
    sha: Option<String>,
    merge_method: Option<String>,
}

async fn merge(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number)): PullPath, Json(body): Json<Option<MergeBody>>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let b = body.unwrap_or_default();
    let method = match b.merge_method.as_deref() {
        None => MergeMethod::Merge,
        Some(m) => MergeMethod::parse(m).ok_or_else(|| ApiError::validation("PullRequest", "merge_method", "invalid"))?,
    };
    let res = pulls::merge_pull(&s, &ctx, &u, number, MergeInput { method, title: b.commit_title, message: b.commit_message, sha: b.sha }).await?;
    Ok(Json(json!({ "sha": res.sha, "merged": res.merged, "message": res.message })))
}

#[derive(Deserialize, Default)]
struct UpdateBranchBody {
    expected_head_sha: Option<String>,
}

async fn update_branch(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number)): PullPath, Json(body): Json<Option<UpdateBranchBody>>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    pulls::update_branch(&s, &ctx, &u, number, body.and_then(|b| b.expected_head_sha)).await?;
    let url = format!("{}/repos/{}/pulls/{number}", s.api_url(), ctx.full_name());
    Ok((StatusCode::ACCEPTED, Json(json!({ "message": "Updating pull request branch.", "url": url }))).into_response())
}

async fn list_reviewers(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, number)): PullPath) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let pull = pulls::pull_by_number(&s.db, ctx.repo.id, number).await?;
    let mut m = pulls::requested_reviewers(&s.db, &[pull.issue.id]).await?;
    let users = m.remove(&pull.issue.id).unwrap_or_default();
    let api = s.api_url();
    Ok(Json(json!({
        "users": users.iter().map(|u| crate::json::user::simple(&api, s.base_url(), u)).collect::<Vec<_>>(),
        "teams": [],
    })))
}

#[derive(Deserialize, Default)]
struct ReviewersBody {
    #[serde(default)]
    reviewers: Vec<String>,
    #[serde(default)]
    team_reviewers: Vec<String>,
}

async fn add_reviewers(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number)): PullPath, Json(b): Json<ReviewersBody>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    if !b.team_reviewers.is_empty() {
        return Err(pulls::custom_error("PullRequest", "Team reviewers are not supported on this server."));
    }
    let pull = pulls::request_reviewers(&s, &ctx, &u, number, &b.reviewers).await?;
    let v = render_simple(&s, &ctx, Some(&u), vec![pull]).await?.pop().unwrap_or(Value::Null);
    Ok(created(v))
}

async fn remove_reviewers(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, number)): PullPath, Json(b): Json<ReviewersBody>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let pull = pulls::remove_requested_reviewers(&s, &ctx, &u, number, &b.reviewers).await?;
    Ok(Json(render_simple(&s, &ctx, Some(&u), vec![pull]).await?.pop().unwrap_or(Value::Null)))
}

/// GET /repos/{owner}/{repo}/commits/{sha}/pulls — PRs whose head is (or that merged as) `sha`.
async fn for_commit(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let rows: Vec<IssueRow> = sqlx::query_as(
        "SELECT i.* FROM issues i JOIN pull_requests p ON p.issue_id = i.id
          WHERE i.repo_id = $1 AND (p.head_sha = $2 OR p.merge_commit_sha = $2 OR p.head_sha LIKE $2 || '%' AND length($2) >= 7)
          ORDER BY i.number DESC LIMIT $3 OFFSET $4",
    )
    .bind(ctx.repo.id)
    .bind(&sha)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let pulls = pulls::pulls_for_issues(&s.db, rows).await?;
    let items = render_simple(&s, &ctx, u.as_ref(), pulls).await?;
    Ok(Paged::new(&uri, &s, p, None, items))
}
