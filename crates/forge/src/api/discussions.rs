//! Repository discussions (service layer; GitHub exposes them through GraphQL only, see
//! `graphql::discussions`). Discussions take numbers from the repository's issue/PR sequence.
//! Enabling discussions on a repository (`has_discussions`) seeds GitHub's default categories
//! the first time they are needed.
use chrono::{DateTime, Utc};

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::json::issue::LabelRow;
use crate::pulls::custom_error;
use crate::state::AppState;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CategoryRow {
    pub id: i64,
    pub repo_id: i64,
    pub name: String,
    pub slug: String,
    pub emoji: String,
    pub description: String,
    pub is_answerable: bool,
    pub position: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscussionRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub category_id: i64,
    pub user_id: Option<i64>,
    pub title: String,
    pub body: String,
    pub closed: bool,
    pub state_reason: Option<String>,
    pub locked: bool,
    pub answer_comment_id: Option<i64>,
    pub answer_chosen_at: Option<DateTime<Utc>>,
    pub answer_chosen_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscussionCommentRow {
    pub id: i64,
    pub discussion_id: i64,
    pub repo_id: i64,
    pub parent_id: Option<i64>,
    pub user_id: Option<i64>,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// GitHub's default categories: (name, slug, emoji, description, answerable).
const DEFAULT_CATEGORIES: &[(&str, &str, &str, &str, bool)] = &[
    ("Announcements", "announcements", ":mega:", "Updates from maintainers", false),
    ("General", "general", ":speech_balloon:", "Chat about anything and everything here", false),
    ("Ideas", "ideas", ":bulb:", "Share ideas for new features", false),
    ("Polls", "polls", ":ballot_box:", "Take a vote from the community", false),
    ("Q&A", "q-a", ":pray:", "Ask the community for help", true),
    ("Show and tell", "show-and-tell", ":raised_hands:", "Show off something you've made", false),
];

fn require_enabled(ctx: &RepoCtx) -> ApiResult<()> {
    if ctx.repo.has_discussions {
        Ok(())
    } else {
        Err(custom_error("Discussion", "Discussions are disabled for this repository."))
    }
}

/// The repository's categories, seeding the defaults on first use.
pub async fn categories(state: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<CategoryRow>> {
    let rows: Vec<CategoryRow> =
        sqlx::query_as("SELECT * FROM discussion_categories WHERE repo_id = $1 ORDER BY position, id").bind(ctx.repo.id).fetch_all(&state.db).await?;
    if !rows.is_empty() || !ctx.repo.has_discussions {
        return Ok(rows);
    }
    for (i, (name, slug, emoji, desc, answerable)) in DEFAULT_CATEGORIES.iter().enumerate() {
        sqlx::query(
            "INSERT INTO discussion_categories (repo_id, name, slug, emoji, description, is_answerable, position)
             VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (repo_id, slug) DO NOTHING",
        )
        .bind(ctx.repo.id)
        .bind(name)
        .bind(slug)
        .bind(emoji)
        .bind(desc)
        .bind(answerable)
        .bind(i as i32)
        .execute(&state.db)
        .await?;
    }
    Ok(sqlx::query_as("SELECT * FROM discussion_categories WHERE repo_id = $1 ORDER BY position, id").bind(ctx.repo.id).fetch_all(&state.db).await?)
}

pub async fn category_by_id(state: &AppState, id: i64) -> ApiResult<CategoryRow> {
    sqlx::query_as("SELECT * FROM discussion_categories WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or_else(ApiError::not_found)
}

pub async fn by_id(state: &AppState, id: i64) -> ApiResult<Option<DiscussionRow>> {
    Ok(sqlx::query_as("SELECT * FROM discussions WHERE id = $1").bind(id).fetch_optional(&state.db).await?)
}

pub async fn by_number(state: &AppState, ctx: &RepoCtx, number: i64) -> ApiResult<Option<DiscussionRow>> {
    Ok(sqlx::query_as("SELECT * FROM discussions WHERE repo_id = $1 AND number = $2")
        .bind(ctx.repo.id)
        .bind(number)
        .fetch_optional(&state.db)
        .await?)
}

/// Filters for [`list`].
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub category_id: Option<i64>,
    /// Some(true) = closed only, Some(false) = open only.
    pub closed: Option<bool>,
    pub answered: Option<bool>,
    /// Order by created_at instead of updated_at.
    pub by_created: bool,
    pub ascending: bool,
}

/// All discussions of a repository matching `f`, in order.
pub async fn list(state: &AppState, ctx: &RepoCtx, f: &ListFilter) -> ApiResult<Vec<DiscussionRow>> {
    let col = if f.by_created { "created_at" } else { "updated_at" };
    let dir = if f.ascending { "ASC" } else { "DESC" };
    let mut qb = sqlx::QueryBuilder::new("SELECT * FROM discussions WHERE repo_id = ");
    qb.push_bind(ctx.repo.id);
    if let Some(c) = f.category_id {
        qb.push(" AND category_id = ").push_bind(c);
    }
    if let Some(closed) = f.closed {
        qb.push(" AND closed = ").push_bind(closed);
    }
    if let Some(a) = f.answered {
        qb.push(if a { " AND answer_comment_id IS NOT NULL" } else { " AND answer_comment_id IS NULL" });
    }
    qb.push(" ORDER BY ").push(col).push(" ").push(dir).push(", id ").push(dir);
    Ok(qb.build_query_as().fetch_all(&state.db).await?)
}

pub async fn labels(state: &AppState, discussion_id: i64) -> ApiResult<Vec<LabelRow>> {
    Ok(sqlx::query_as("SELECT l.* FROM labels l JOIN discussion_labels dl ON dl.label_id = l.id WHERE dl.discussion_id = $1 ORDER BY l.name")
        .bind(discussion_id)
        .fetch_all(&state.db)
        .await?)
}

pub async fn create(state: &AppState, ctx: &RepoCtx, actor: &User, category_id: i64, title: &str, body: &str) -> ApiResult<DiscussionRow> {
    ctx.require(Perm::Read)?;
    require_enabled(ctx)?;
    if title.trim().is_empty() {
        return Err(ApiError::validation("Discussion", "title", "missing_field"));
    }
    let cat = category_by_id(state, category_id).await?;
    if cat.repo_id != ctx.repo.id {
        return Err(custom_error("Discussion", "The category does not belong to this repository."));
    }
    if cat.slug == "announcements" && ctx.perm < Perm::Maintain {
        return Err(ApiError::forbidden("Only maintainers can post announcements."));
    }
    let mut tx = state.db.begin().await?;
    let number = db::next_issue_number(&mut *tx, ctx.repo.id).await?;
    let row: DiscussionRow = sqlx::query_as(
        "INSERT INTO discussions (repo_id, number, category_id, user_id, title, body) VALUES ($1,$2,$3,$4,$5,$6) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(number)
    .bind(category_id)
    .bind(actor.id)
    .bind(title.trim())
    .bind(body)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

fn can_edit(ctx: &RepoCtx, actor: &User, author: Option<i64>) -> bool {
    ctx.perm >= Perm::Triage || author == Some(actor.id)
}

#[derive(Debug, Clone, Default)]
pub struct DiscussionPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub category_id: Option<i64>,
}

pub async fn update(state: &AppState, ctx: &RepoCtx, actor: &User, d: &DiscussionRow, p: DiscussionPatch) -> ApiResult<DiscussionRow> {
    if !can_edit(ctx, actor, d.user_id) {
        return Err(ApiError::forbidden("Must have triage access or be the author to update this discussion."));
    }
    if let Some(t) = &p.title {
        if t.trim().is_empty() {
            return Err(ApiError::validation("Discussion", "title", "missing_field"));
        }
    }
    if let Some(c) = p.category_id {
        if category_by_id(state, c).await?.repo_id != ctx.repo.id {
            return Err(custom_error("Discussion", "The category does not belong to this repository."));
        }
    }
    Ok(sqlx::query_as(
        "UPDATE discussions SET title = COALESCE($2, title), body = COALESCE($3, body), category_id = COALESCE($4, category_id),
                updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(d.id)
    .bind(p.title.map(|t| t.trim().to_string()))
    .bind(p.body)
    .bind(p.category_id)
    .fetch_one(&state.db)
    .await?)
}

/// Closes (`reason`: resolved | outdated | duplicate) or reopens a discussion.
pub async fn set_closed(state: &AppState, ctx: &RepoCtx, actor: &User, d: &DiscussionRow, closed: bool, reason: Option<&str>) -> ApiResult<DiscussionRow> {
    if !can_edit(ctx, actor, d.user_id) {
        return Err(ApiError::forbidden("Must have triage access or be the author to close this discussion."));
    }
    let reason = if closed { Some(reason.unwrap_or("resolved")) } else if d.closed { Some("reopened") } else { d.state_reason.as_deref() };
    Ok(sqlx::query_as(
        "UPDATE discussions SET closed = $2, state_reason = $3, closed_at = CASE WHEN $2 THEN now() ELSE NULL END, updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(d.id)
    .bind(closed)
    .bind(reason)
    .fetch_one(&state.db)
    .await?)
}

pub async fn delete(state: &AppState, ctx: &RepoCtx, actor: &User, d: &DiscussionRow) -> ApiResult<()> {
    if ctx.perm < Perm::Admin && d.user_id != Some(actor.id) {
        return Err(ApiError::forbidden("Must have admin rights or be the author to delete this discussion."));
    }
    sqlx::query("DELETE FROM discussions WHERE id = $1").bind(d.id).execute(&state.db).await?;
    Ok(())
}

pub async fn comment_by_id(state: &AppState, id: i64) -> ApiResult<Option<DiscussionCommentRow>> {
    Ok(sqlx::query_as("SELECT * FROM discussion_comments WHERE id = $1").bind(id).fetch_optional(&state.db).await?)
}

/// Top-level comments (`parent` None) or the replies to a comment, oldest first.
pub async fn comments(state: &AppState, discussion_id: i64, parent: Option<i64>) -> ApiResult<Vec<DiscussionCommentRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM discussion_comments WHERE discussion_id = $1 AND parent_id IS NOT DISTINCT FROM $2 ORDER BY id",
    )
    .bind(discussion_id)
    .bind(parent)
    .fetch_all(&state.db)
    .await?)
}

/// Adds a comment, or a reply when `reply_to` is a comment of the same discussion (a reply to
/// a reply is attached to its top-level comment, like GitHub).
pub async fn add_comment(state: &AppState, ctx: &RepoCtx, actor: &User, d: &DiscussionRow, body: &str, reply_to: Option<i64>) -> ApiResult<DiscussionCommentRow> {
    ctx.require(Perm::Read)?;
    require_enabled(ctx)?;
    if body.trim().is_empty() {
        return Err(ApiError::validation("DiscussionComment", "body", "missing_field"));
    }
    if d.locked && ctx.perm < Perm::Triage {
        return Err(ApiError::forbidden("This discussion is locked."));
    }
    let parent = match reply_to {
        Some(id) => {
            let c = comment_by_id(state, id).await?.filter(|c| c.discussion_id == d.id).ok_or_else(ApiError::not_found)?;
            Some(c.parent_id.unwrap_or(c.id))
        }
        None => None,
    };
    let mut tx = state.db.begin().await?;
    let c: DiscussionCommentRow = sqlx::query_as(
        "INSERT INTO discussion_comments (discussion_id, repo_id, parent_id, user_id, body) VALUES ($1,$2,$3,$4,$5) RETURNING *",
    )
    .bind(d.id)
    .bind(ctx.repo.id)
    .bind(parent)
    .bind(actor.id)
    .bind(body)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE discussions SET updated_at = now() WHERE id = $1").bind(d.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(c)
}

pub async fn update_comment(state: &AppState, ctx: &RepoCtx, actor: &User, c: &DiscussionCommentRow, body: &str) -> ApiResult<DiscussionCommentRow> {
    if !can_edit(ctx, actor, c.user_id) {
        return Err(ApiError::forbidden("Must have triage access or be the author to update this comment."));
    }
    if body.trim().is_empty() {
        return Err(ApiError::validation("DiscussionComment", "body", "missing_field"));
    }
    Ok(sqlx::query_as("UPDATE discussion_comments SET body = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(c.id)
        .bind(body)
        .fetch_one(&state.db)
        .await?)
}

pub async fn delete_comment(state: &AppState, ctx: &RepoCtx, actor: &User, c: &DiscussionCommentRow) -> ApiResult<()> {
    if !can_edit(ctx, actor, c.user_id) {
        return Err(ApiError::forbidden("Must have triage access or be the author to delete this comment."));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE discussions SET answer_comment_id = NULL, answer_chosen_at = NULL, answer_chosen_by = NULL WHERE answer_comment_id = $1")
        .bind(c.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM discussion_comments WHERE id = $1").bind(c.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Marks (or unmarks) a top-level comment as the answer; needs an answerable category and
/// triage access or authorship of the discussion.
pub async fn set_answer(state: &AppState, ctx: &RepoCtx, actor: &User, c: &DiscussionCommentRow, answer: bool) -> ApiResult<DiscussionRow> {
    let d = by_id(state, c.discussion_id).await?.ok_or_else(ApiError::not_found)?;
    if !can_edit(ctx, actor, d.user_id) {
        return Err(ApiError::forbidden("Must have triage access or be the author of the discussion to choose an answer."));
    }
    if !category_by_id(state, d.category_id).await?.is_answerable {
        return Err(custom_error("Discussion", "This discussion's category does not accept answers."));
    }
    if c.parent_id.is_some() && answer {
        return Err(custom_error("DiscussionComment", "Replies can't be marked as the answer."));
    }
    let row: DiscussionRow = if answer {
        sqlx::query_as("UPDATE discussions SET answer_comment_id = $2, answer_chosen_at = now(), answer_chosen_by = $3 WHERE id = $1 RETURNING *")
            .bind(d.id)
            .bind(c.id)
            .bind(actor.id)
            .fetch_one(&state.db)
            .await?
    } else {
        sqlx::query_as(
            "UPDATE discussions SET answer_comment_id = NULL, answer_chosen_at = NULL, answer_chosen_by = NULL
              WHERE id = $1 AND answer_comment_id = $2 RETURNING *",
        )
        .bind(d.id)
        .bind(c.id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or(d)
    };
    Ok(row)
}
