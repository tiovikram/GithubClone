//! Pull request reviews and review (diff) comments.
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;

use super::{git, pull_by_number, record_event, repo_for_reading, Pull};
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReviewRow {
    pub id: i64,
    pub issue_id: i64,
    pub user_id: Option<i64>,
    pub body: String,
    pub state: String,
    pub commit_id: Option<String>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub dismissal_message: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReviewCommentRow {
    pub id: i64,
    pub issue_id: i64,
    pub repo_id: i64,
    pub review_id: Option<i64>,
    pub in_reply_to_id: Option<i64>,
    pub user_id: Option<i64>,
    pub body: String,
    pub path: String,
    pub commit_id: String,
    pub original_commit_id: String,
    pub diff_hunk: String,
    pub line: Option<i64>,
    pub original_line: Option<i64>,
    pub start_line: Option<i64>,
    pub side: String,
    pub start_side: Option<String>,
    pub position: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub original_position: Option<i64>,
    pub original_start_line: Option<i64>,
    pub subject_type: String,
}

/// 422 in GitHub's `validation-error-simple` shape (`errors` is a list of strings), used by the
/// review endpoints.
pub fn simple_error(msg: impl Into<String>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        message: "Unprocessable Entity".into(),
        errors: Some(json!([msg.into()])),
    }
}

fn thread_error(field: &str) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        message: "Validation Failed".into(),
        errors: Some(json!([{
            "resource": "PullRequestReviewComment", "code": "custom",
            "field": format!("pull_request_review_thread.{field}"), "message": "could not be resolved"
        }])),
    }
}

pub async fn review_by_id(db: &PgPool, pull: &Pull, id: i64) -> ApiResult<ReviewRow> {
    sqlx::query_as("SELECT * FROM pull_reviews WHERE id = $1 AND issue_id = $2")
        .bind(id)
        .bind(pull.issue.id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn comment_by_id(db: &PgPool, repo_id: i64, id: i64) -> ApiResult<ReviewCommentRow> {
    sqlx::query_as("SELECT * FROM review_comments WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(repo_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Whether `viewer` may see `c` (comments of someone else's pending review are hidden).
pub async fn comment_visible(db: &PgPool, c: &ReviewCommentRow, viewer: Option<&User>) -> ApiResult<bool> {
    if c.user_id.is_some() && c.user_id == viewer.map(|u| u.id) {
        return Ok(true);
    }
    let Some(rid) = c.review_id else { return Ok(true) };
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM pull_reviews WHERE id = $1").bind(rid).fetch_optional(db).await?;
    Ok(state.as_deref() != Some("PENDING"))
}

// ---------------------------------------------------------------------------------------------
// Locating a comment in the diff

/// Where a review comment sits in the PR diff.
#[derive(Debug, Clone, Default)]
pub struct Location {
    pub path: String,
    pub commit_id: String,
    pub diff_hunk: String,
    pub line: Option<i64>,
    pub side: String,
    pub start_line: Option<i64>,
    pub start_side: Option<String>,
    pub position: Option<i64>,
    pub subject_type: String,
}

/// Comment placement as given by the API (`line`/`side`/`start_line`, or legacy `position`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Placement {
    pub path: Option<String>,
    pub position: Option<i64>,
    pub line: Option<i64>,
    pub side: Option<String>,
    pub start_line: Option<i64>,
    pub start_side: Option<String>,
    pub subject_type: Option<String>,
}

/// Resolves a placement against the diff of the PR at `commit_id` (merge base of base and the
/// commit .. commit), computing `diff_hunk` and `position` like GitHub.
pub async fn locate(state: &AppState, pull: &Pull, commit_id: &str, p: &Placement) -> ApiResult<Location> {
    let path = p.path.clone().filter(|s| !s.is_empty()).ok_or_else(|| ApiError::validation("PullRequestReviewComment", "path", "missing_field"))?;
    let repo = repo_for_reading(state, pull).await?;
    let g = &state.git.git;
    // Only full object ids reach git (a revision starting with `-` would be an option).
    if !crate::git::read::is_sha(commit_id) || !git::has_commit(g, &repo, commit_id).await {
        return Err(custom("commit_id", "commit_id is not part of the pull request"));
    }
    let base = if commit_id == pull.pr.head_sha {
        pull.diff_base().to_string()
    } else {
        git::merge_base(g, &repo, &pull.pr.base_sha, commit_id).await?.unwrap_or_else(|| git::EMPTY_TREE.to_string())
    };
    let files = git::diff(g, &repo, &base, commit_id, Some(&path)).await?;
    let file = files.into_iter().find(|f| f.filename() == path || f.old_path.as_deref() == Some(path.as_str())).ok_or_else(|| thread_error("path"))?;
    let mut loc = Location { path: path.clone(), commit_id: commit_id.to_string(), side: "RIGHT".into(), subject_type: "line".into(), ..Default::default() };
    if p.subject_type.as_deref() == Some("file") {
        loc.subject_type = "file".into();
        loc.diff_hunk = file.patch.split('\n').next().unwrap_or("").to_string();
        return Ok(loc);
    }
    let pl = if let Some(line) = p.line {
        let side = p.side.clone().unwrap_or_else(|| "RIGHT".into()).to_uppercase();
        if side != "LEFT" && side != "RIGHT" {
            return Err(ApiError::validation("PullRequestReviewComment", "side", "invalid"));
        }
        loc.side = side.clone();
        git::find_line(&file.patch, line, &side).ok_or_else(|| thread_error("line"))?
    } else if let Some(pos) = p.position {
        let pl = git::find_position(&file.patch, pos).ok_or_else(|| thread_error("diff_hunk"))?;
        loc.side = if pl.kind == '-' { "LEFT".into() } else { "RIGHT".into() };
        pl
    } else {
        return Err(ApiError::validation("PullRequestReviewComment", "line", "missing_field"));
    };
    loc.line = if loc.side == "LEFT" { pl.old_line } else { pl.new_line };
    loc.position = Some(pl.position);
    loc.diff_hunk = git::diff_hunk(&file.patch, &pl);
    if let Some(start) = p.start_line {
        let start_side = p.start_side.clone().map(|s| s.to_uppercase()).unwrap_or_else(|| loc.side.clone());
        if loc.line.is_some_and(|l| start > l && start_side == loc.side) {
            return Err(thread_error("start_line"));
        }
        git::find_line(&file.patch, start, &start_side).ok_or_else(|| thread_error("start_line"))?;
        loc.start_line = Some(start);
        loc.start_side = Some(start_side);
    }
    Ok(loc)
}

fn custom(field: &str, message: &str) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        message: "Validation Failed".into(),
        errors: Some(json!([{ "resource": "PullRequestReviewComment", "code": "custom", "field": field, "message": message }])),
    }
}

async fn insert_comment<'e, E: sqlx::PgExecutor<'e>>(
    db: E,
    pull: &Pull,
    review_id: i64,
    in_reply_to: Option<i64>,
    actor: &User,
    body: &str,
    loc: &Location,
) -> ApiResult<ReviewCommentRow> {
    Ok(sqlx::query_as(
        "INSERT INTO review_comments (issue_id, repo_id, review_id, in_reply_to_id, user_id, body, path, commit_id, original_commit_id,
            diff_hunk, line, original_line, start_line, original_start_line, side, start_side, position, original_position, subject_type)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9,$10,$10,$11,$11,$12,$13,$14,$14,$15) RETURNING *",
    )
    .bind(pull.issue.id)
    .bind(pull.issue.repo_id)
    .bind(review_id)
    .bind(in_reply_to)
    .bind(actor.id)
    .bind(body)
    .bind(&loc.path)
    .bind(&loc.commit_id)
    .bind(&loc.diff_hunk)
    .bind(loc.line)
    .bind(loc.start_line)
    .bind(&loc.side)
    .bind(&loc.start_side)
    .bind(loc.position)
    .bind(&loc.subject_type)
    .fetch_one(db)
    .await?)
}

fn location_of(c: &ReviewCommentRow) -> Location {
    Location {
        path: c.path.clone(),
        commit_id: c.commit_id.clone(),
        diff_hunk: c.diff_hunk.clone(),
        line: c.line,
        side: c.side.clone(),
        start_line: c.start_line,
        start_side: c.start_side.clone(),
        position: c.position,
        subject_type: c.subject_type.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// Review comments

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct NewReviewComment {
    #[serde(default)]
    pub body: String,
    pub commit_id: Option<String>,
    #[serde(flatten)]
    pub placement: Placement,
    /// Reply to this comment id (placement is copied from the thread's first comment).
    pub in_reply_to: Option<i64>,
}

/// Locked conversations accept reviews and review comments only from users with Write.
fn check_unlocked(ctx: &RepoCtx, pull: &Pull) -> ApiResult<()> {
    if pull.issue.locked && ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Unable to create comment because issue is locked."));
    }
    Ok(())
}

/// Adds a single review comment (wrapped in its own COMMENTED review, like GitHub), or a reply.
pub async fn add_review_comment(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, input: NewReviewComment) -> ApiResult<ReviewCommentRow> {
    ctx.require(Perm::Read)?;
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    check_unlocked(ctx, &pull)?;
    if input.body.trim().is_empty() {
        return Err(ApiError::validation("PullRequestReviewComment", "body", "missing_field"));
    }
    let (loc, reply_to) = match input.in_reply_to {
        Some(parent_id) => {
            let parent = comment_by_id(&state.db, ctx.repo.id, parent_id).await?;
            if parent.issue_id != pull.issue.id || !comment_visible(&state.db, &parent, Some(actor)).await? {
                return Err(ApiError::not_found());
            }
            let root_id = parent.in_reply_to_id.unwrap_or(parent.id);
            let root = if root_id == parent.id { parent } else { comment_by_id(&state.db, ctx.repo.id, root_id).await? };
            (location_of(&root), Some(root.id))
        }
        None => {
            let commit = input.commit_id.clone().filter(|c| !c.is_empty()).ok_or_else(|| ApiError::validation("PullRequestReviewComment", "commit_id", "missing_field"))?;
            (locate(state, &pull, &commit, &input.placement).await?, None)
        }
    };
    let mut tx = state.db.begin().await?;
    let review_id: i64 = sqlx::query_scalar(
        "INSERT INTO pull_reviews (issue_id, user_id, body, state, commit_id, submitted_at) VALUES ($1, $2, '', 'COMMENTED', $3, now()) RETURNING id",
    )
    .bind(pull.issue.id)
    .bind(actor.id)
    .bind(&loc.commit_id)
    .fetch_one(&mut *tx)
    .await?;
    let c = insert_comment(&mut *tx, &pull, review_id, reply_to, actor, &input.body, &loc).await?;
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(pull.issue.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(c)
}

fn can_modify_comment(ctx: &RepoCtx, actor: &User, c: &ReviewCommentRow) -> ApiResult<()> {
    if c.user_id == Some(actor.id) {
        Ok(())
    } else {
        ctx.require(Perm::Write)
    }
}

pub async fn update_review_comment(state: &AppState, ctx: &RepoCtx, actor: &User, id: i64, body: &str) -> ApiResult<ReviewCommentRow> {
    let c = comment_by_id(&state.db, ctx.repo.id, id).await?;
    if !comment_visible(&state.db, &c, Some(actor)).await? {
        return Err(ApiError::not_found());
    }
    can_modify_comment(ctx, actor, &c)?;
    if body.trim().is_empty() {
        return Err(ApiError::validation("PullRequestReviewComment", "body", "missing_field"));
    }
    Ok(sqlx::query_as("UPDATE review_comments SET body = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(id)
        .bind(body)
        .fetch_one(&state.db)
        .await?)
}

pub async fn delete_review_comment(state: &AppState, ctx: &RepoCtx, actor: &User, id: i64) -> ApiResult<()> {
    let c = comment_by_id(&state.db, ctx.repo.id, id).await?;
    if !comment_visible(&state.db, &c, Some(actor)).await? {
        return Err(ApiError::not_found());
    }
    can_modify_comment(ctx, actor, &c)?;
    sqlx::query("DELETE FROM review_comments WHERE id = $1").bind(id).execute(&state.db).await?;
    sqlx::query("DELETE FROM reactions WHERE subject_type = 'review_comment' AND subject_id = $1").bind(id).execute(&state.db).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Reviews

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DraftComment {
    #[serde(default)]
    pub body: String,
    #[serde(flatten)]
    pub placement: Placement,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CreateReview {
    pub commit_id: Option<String>,
    pub body: Option<String>,
    /// APPROVE | REQUEST_CHANGES | COMMENT; absent = PENDING
    pub event: Option<String>,
    #[serde(default)]
    pub comments: Vec<DraftComment>,
}

/// Maps a REST/GraphQL review event to a stored state.
pub fn event_state(event: Option<&str>) -> ApiResult<&'static str> {
    match event.map(|e| e.to_ascii_uppercase()) {
        None => Ok("PENDING"),
        Some(e) if e.is_empty() || e == "PENDING" => Ok("PENDING"),
        Some(e) if e == "APPROVE" || e == "APPROVED" => Ok("APPROVED"),
        Some(e) if e == "REQUEST_CHANGES" || e == "CHANGES_REQUESTED" => Ok("CHANGES_REQUESTED"),
        Some(e) if e == "COMMENT" || e == "COMMENTED" => Ok("COMMENTED"),
        Some(_) => Err(simple_error("Unexpected event")),
    }
}

fn check_submit(pull: &Pull, actor: &User, state: &str, body: &str, has_comments: bool) -> ApiResult<()> {
    let own = pull.issue.user_id == Some(actor.id);
    match state {
        "APPROVED" if own => Err(simple_error("Review Can not approve your own pull request")),
        "CHANGES_REQUESTED" if own => Err(simple_error("Review Can not request changes on your own pull request")),
        "CHANGES_REQUESTED" if body.trim().is_empty() => Err(simple_error("Review body is required for REQUEST_CHANGES")),
        "COMMENTED" if body.trim().is_empty() && !has_comments => Err(simple_error("Review body is required for COMMENT")),
        _ => Ok(()),
    }
}

async fn after_submit(db: &PgPool, pull: &Pull, actor: &User) -> ApiResult<()> {
    sqlx::query("DELETE FROM pull_review_requests WHERE issue_id = $1 AND user_id = $2").bind(pull.issue.id).bind(actor.id).execute(db).await?;
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(pull.issue.id).execute(db).await?;
    Ok(())
}

pub async fn create_review(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, input: CreateReview) -> ApiResult<ReviewRow> {
    ctx.require(Perm::Read)?;
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    check_unlocked(ctx, &pull)?;
    let st = event_state(input.event.as_deref())?;
    let body = input.body.clone().unwrap_or_default();
    check_submit(&pull, actor, st, &body, !input.comments.is_empty())?;
    if st == "PENDING" {
        let has: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pull_reviews WHERE issue_id = $1 AND user_id = $2 AND state = 'PENDING')")
            .bind(pull.issue.id)
            .bind(actor.id)
            .fetch_one(&state.db)
            .await?;
        if has {
            return Err(simple_error("User can only have one pending review per pull request"));
        }
    }
    let commit = input.commit_id.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| pull.pr.head_sha.clone());
    // Stored and later handed to git (GraphQL `PullRequestReview.commit`): full object ids only.
    if !crate::git::read::is_sha(&commit) {
        return Err(ApiError::validation("PullRequestReview", "commit_id", "invalid"));
    }
    let commit = commit.to_lowercase();
    let mut locs = Vec::with_capacity(input.comments.len());
    for c in &input.comments {
        if c.body.trim().is_empty() {
            return Err(simple_error("Comment body is required"));
        }
        locs.push(locate(state, &pull, &commit, &c.placement).await.map_err(|e| {
            if e.status == StatusCode::UNPROCESSABLE_ENTITY { simple_error("Pull request review thread line must be part of the diff") } else { e }
        })?);
    }
    let mut tx = state.db.begin().await?;
    let review: ReviewRow = sqlx::query_as(
        "INSERT INTO pull_reviews (issue_id, user_id, body, state, commit_id, submitted_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $4 = 'PENDING' THEN NULL ELSE now() END) RETURNING *",
    )
    .bind(pull.issue.id)
    .bind(actor.id)
    .bind(&body)
    .bind(st)
    .bind(&commit)
    .fetch_one(&mut *tx)
    .await?;
    for (c, loc) in input.comments.iter().zip(&locs) {
        insert_comment(&mut *tx, &pull, review.id, None, actor, &c.body, loc).await?;
    }
    tx.commit().await?;
    if st != "PENDING" {
        after_submit(&state.db, &pull, actor).await?;
    }
    Ok(review)
}

pub async fn submit_review(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, review_id: i64, event: Option<&str>, body: Option<String>) -> ApiResult<ReviewRow> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    let r = review_by_id(&state.db, &pull, review_id).await?;
    if r.user_id != Some(actor.id) {
        return Err(ApiError::not_found());
    }
    if r.state != "PENDING" {
        return Err(simple_error("Can not submit a non-pending review"));
    }
    check_unlocked(ctx, &pull)?;
    let st = event_state(event)?;
    if st == "PENDING" {
        return Err(simple_error("Unexpected event"));
    }
    let body = body.unwrap_or(r.body.clone());
    let n_comments: i64 = sqlx::query_scalar("SELECT count(*) FROM review_comments WHERE review_id = $1").bind(r.id).fetch_one(&state.db).await?;
    check_submit(&pull, actor, st, &body, n_comments > 0)?;
    let r: ReviewRow = sqlx::query_as("UPDATE pull_reviews SET state = $2, body = $3, submitted_at = now(), updated_at = now() WHERE id = $1 RETURNING *")
        .bind(r.id)
        .bind(st)
        .bind(&body)
        .fetch_one(&state.db)
        .await?;
    sqlx::query("UPDATE review_comments SET created_at = now(), updated_at = now() WHERE review_id = $1").bind(r.id).execute(&state.db).await?;
    after_submit(&state.db, &pull, actor).await?;
    Ok(r)
}

pub async fn update_review(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, review_id: i64, body: &str) -> ApiResult<ReviewRow> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    let r = review_by_id(&state.db, &pull, review_id).await?;
    if r.user_id != Some(actor.id) {
        if r.state == "PENDING" {
            return Err(ApiError::not_found());
        }
        ctx.require(Perm::Write)?;
    }
    Ok(sqlx::query_as("UPDATE pull_reviews SET body = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(r.id)
        .bind(body)
        .fetch_one(&state.db)
        .await?)
}

pub async fn delete_pending_review(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, review_id: i64) -> ApiResult<ReviewRow> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    let r = review_by_id(&state.db, &pull, review_id).await?;
    if r.user_id != Some(actor.id) {
        return Err(ApiError::not_found());
    }
    if r.state != "PENDING" {
        return Err(simple_error("Can not delete a non-pending pull request review"));
    }
    sqlx::query("DELETE FROM pull_reviews WHERE id = $1").bind(r.id).execute(&state.db).await?;
    Ok(r)
}

pub async fn dismiss_review(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, review_id: i64, message: &str) -> ApiResult<ReviewRow> {
    ctx.require(Perm::Write)?;
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    let r = review_by_id(&state.db, &pull, review_id).await?;
    if r.state != "APPROVED" && r.state != "CHANGES_REQUESTED" {
        return Err(simple_error(format!("Can not dismiss a {} pull request review", r.state.to_lowercase().replace('_', " "))));
    }
    if message.trim().is_empty() {
        return Err(simple_error("Message is required"));
    }
    let updated: ReviewRow = sqlx::query_as("UPDATE pull_reviews SET state = 'DISMISSED', dismissal_message = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(r.id)
        .bind(message)
        .fetch_one(&state.db)
        .await?;
    record_event(
        &state.db,
        &pull,
        Some(actor.id),
        "review_dismissed",
        None,
        json!({"dismissed_review": {"state": r.state.to_lowercase(), "review_id": r.id, "dismissal_message": message, "dismissal_commit_id": null}}),
    )
    .await?;
    Ok(updated)
}

/// Reviews of a PR visible to `viewer` (others' pending reviews are hidden).
pub async fn list_reviews(db: &PgPool, pull: &Pull, viewer: Option<&User>, limit: i64, offset: i64) -> ApiResult<(Vec<ReviewRow>, i64)> {
    let vid = viewer.map(|u| u.id).unwrap_or(-1);
    let rows: Vec<ReviewRow> = sqlx::query_as(
        "SELECT * FROM pull_reviews WHERE issue_id = $1 AND (state <> 'PENDING' OR user_id = $2) ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(pull.issue.id)
    .bind(vid)
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM pull_reviews WHERE issue_id = $1 AND (state <> 'PENDING' OR user_id = $2)")
        .bind(pull.issue.id)
        .bind(vid)
        .fetch_one(db)
        .await?;
    Ok((rows, total))
}
