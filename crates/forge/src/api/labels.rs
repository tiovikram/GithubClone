//! Repository labels and milestones (CRUD), plus GitHub's default label set for new repos.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::issues::{double_opt, invalid, load_ctx, parse_time};
use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::issue::{self as ji, LabelRow, MilestoneRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/labels", get(list_labels).post(create_label_handler))
        .route(
            "/repos/{owner}/{repo}/labels/{name}",
            get(get_label).patch(update_label_handler).delete(delete_label_handler),
        )
        .route("/repos/{owner}/{repo}/milestones", get(list_milestones).post(create_milestone_handler))
        .route(
            "/repos/{owner}/{repo}/milestones/{number}",
            get(get_milestone).patch(update_milestone_handler).delete(delete_milestone_handler),
        )
        .route("/repos/{owner}/{repo}/milestones/{number}/labels", get(milestone_labels))
}

/// GitHub's default labels for new repositories: (name, color, description).
pub const DEFAULT_LABELS: [(&str, &str, &str); 9] = [
    ("bug", "d73a4a", "Something isn't working"),
    ("documentation", "0075ca", "Improvements or additions to documentation"),
    ("duplicate", "cfd3d7", "This issue or pull request already exists"),
    ("enhancement", "a2eeef", "New feature or request"),
    ("good first issue", "7057ff", "Good for newcomers"),
    ("help wanted", "008672", "Extra attention is needed"),
    ("invalid", "e4e669", "This doesn't seem right"),
    ("question", "d876e3", "Further information is requested"),
    ("wontfix", "ffffff", "This will not be worked on"),
];

/// Seeds the default label set and the owner's watch on a freshly created repository.
/// Called at the end of `api::repos::create_repo`.
pub async fn on_repo_created(state: &AppState, repo_id: i64, creator: &User) -> ApiResult<()> {
    let names: Vec<&str> = DEFAULT_LABELS.iter().map(|l| l.0).collect();
    let colors: Vec<&str> = DEFAULT_LABELS.iter().map(|l| l.1).collect();
    let descs: Vec<&str> = DEFAULT_LABELS.iter().map(|l| l.2).collect();
    sqlx::query(
        "INSERT INTO labels (repo_id, name, color, description, is_default)
         SELECT $1, n, c, d, TRUE FROM unnest($2::text[], $3::text[], $4::text[]) AS t(n, c, d)
         ON CONFLICT DO NOTHING",
    )
    .bind(repo_id)
    .bind(&names)
    .bind(&colors)
    .bind(&descs)
    .execute(&state.db)
    .await?;
    sqlx::query("INSERT INTO watches (user_id, repo_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(creator.id)
        .bind(repo_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Labels: service layer.

fn normalize_color(c: &str) -> ApiResult<String> {
    let c = c.trim().trim_start_matches('#').to_lowercase();
    if c.len() == 6 && c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(c)
    } else {
        Err(invalid("Label", "color", c))
    }
}

fn label_conflict(e: sqlx::Error) -> ApiError {
    match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Label", "name", "already_exists"),
        e => e.into(),
    }
}

pub async fn label_by_name(state: &AppState, repo_id: i64, name: &str) -> ApiResult<LabelRow> {
    sqlx::query_as("SELECT * FROM labels WHERE repo_id = $1 AND lower(name) = lower($2)")
        .bind(repo_id)
        .bind(name)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn create_label(state: &AppState, ctx: &RepoCtx, name: &str, color: Option<&str>, description: Option<&str>) -> ApiResult<LabelRow> {
    ctx.require(Perm::Write)?;
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::validation("Label", "name", "missing_field"));
    }
    let color = match color {
        Some(c) => normalize_color(c)?,
        None => "ededed".into(),
    };
    sqlx::query_as("INSERT INTO labels (repo_id, name, color, description) VALUES ($1,$2,$3,$4) RETURNING *")
        .bind(ctx.repo.id)
        .bind(name)
        .bind(color)
        .bind(description.filter(|d| !d.is_empty()))
        .fetch_one(&state.db)
        .await
        .map_err(label_conflict)
}

/// Renames / recolors / re-describes a label. `description: Some(None)` clears it.
pub async fn update_label(
    state: &AppState,
    ctx: &RepoCtx,
    name: &str,
    new_name: Option<&str>,
    color: Option<&str>,
    description: Option<Option<&str>>,
) -> ApiResult<LabelRow> {
    ctx.require(Perm::Write)?;
    let cur = label_by_name(state, ctx.repo.id, name).await?;
    let new_name = new_name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&cur.name);
    let color = match color {
        Some(c) => normalize_color(c)?,
        None => cur.color.clone(),
    };
    let description = match description {
        Some(d) => d.filter(|d| !d.is_empty()).map(str::to_string),
        None => cur.description.clone(),
    };
    sqlx::query_as("UPDATE labels SET name = $2, color = $3, description = $4 WHERE id = $1 RETURNING *")
        .bind(cur.id)
        .bind(new_name)
        .bind(color)
        .bind(description)
        .fetch_one(&state.db)
        .await
        .map_err(label_conflict)
}

pub async fn delete_label(state: &AppState, ctx: &RepoCtx, name: &str) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    let cur = label_by_name(state, ctx.repo.id, name).await?;
    sqlx::query("DELETE FROM labels WHERE id = $1").bind(cur.id).execute(&state.db).await?;
    Ok(())
}

// Labels: handlers.

fn label_json(s: &AppState, ctx: &RepoCtx, l: &LabelRow) -> Value {
    ji::label(&s.api_url(), &ctx.full_name(), l)
}

async fn list_labels(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE repo_id = $1").bind(ctx.repo.id).fetch_one(&s.db).await?;
    let rows: Vec<LabelRow> = sqlx::query_as("SELECT * FROM labels WHERE repo_id = $1 ORDER BY lower(name), id LIMIT $2 OFFSET $3")
        .bind(ctx.repo.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = rows.iter().map(|l| label_json(&s, &ctx, l)).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
struct LabelBody {
    name: Option<String>,
    new_name: Option<String>,
    color: Option<String>,
    #[serde(default, deserialize_with = "double_opt")]
    description: Option<Option<String>>,
}

async fn create_label_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<LabelBody>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let name = b.name.ok_or_else(|| ApiError::validation("Label", "name", "missing_field"))?;
    let l = create_label(&s, &ctx, &name, b.color.as_deref(), b.description.flatten().as_deref()).await?;
    Ok(created(label_json(&s, &ctx, &l)))
}

async fn get_label(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, name)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let l = label_by_name(&s, ctx.repo.id, &name).await?;
    Ok(Json(label_json(&s, &ctx, &l)))
}

async fn update_label_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, name)): Path<(String, String, String)>,
    Json(b): Json<LabelBody>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let new_name = b.new_name.or(b.name);
    let desc = b.description.as_ref().map(|d| d.as_deref());
    let l = update_label(&s, &ctx, &name, new_name.as_deref(), b.color.as_deref(), desc).await?;
    Ok(Json(label_json(&s, &ctx, &l)))
}

async fn delete_label_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, name)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    delete_label(&s, &ctx, &name).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Milestones: service layer.

#[derive(Debug, Default, Clone)]
pub struct MilestoneInput {
    pub title: Option<String>,
    /// "open" | "closed"
    pub state: Option<String>,
    pub description: Option<Option<String>>,
    pub due_on: Option<Option<DateTime<Utc>>>,
}

fn milestone_conflict(e: sqlx::Error) -> ApiError {
    match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Milestone", "title", "already_exists"),
        e => e.into(),
    }
}

fn check_state(state: &Option<String>) -> ApiResult<()> {
    match state.as_deref() {
        None | Some("open") | Some("closed") => Ok(()),
        Some(other) => Err(invalid("Milestone", "state", other)),
    }
}

pub async fn milestone_row(state: &AppState, repo_id: i64, number: i64) -> ApiResult<MilestoneRow> {
    sqlx::query_as("SELECT * FROM milestones WHERE repo_id = $1 AND number = $2")
        .bind(repo_id)
        .bind(number)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn create_milestone(state: &AppState, ctx: &RepoCtx, actor: &User, input: MilestoneInput) -> ApiResult<MilestoneRow> {
    ctx.require(Perm::Write)?;
    let title = input.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let Some(title) = title else { return Err(ApiError::validation("Milestone", "title", "missing_field")) };
    check_state(&input.state)?;
    let mut tx = state.db.begin().await?;
    // Serialize number allocation per repository.
    sqlx::query("SELECT id FROM repositories WHERE id = $1 FOR UPDATE").bind(ctx.repo.id).execute(&mut *tx).await?;
    let dup: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM milestones WHERE repo_id = $1 AND lower(title) = lower($2))")
        .bind(ctx.repo.id)
        .bind(title)
        .fetch_one(&mut *tx)
        .await?;
    if dup {
        return Err(ApiError::validation("Milestone", "title", "already_exists"));
    }
    let closed = input.state.as_deref() == Some("closed");
    let row: MilestoneRow = sqlx::query_as(
        "INSERT INTO milestones (repo_id, number, title, description, state, due_on, creator_id, closed_at)
         VALUES ($1, (SELECT COALESCE(MAX(number), 0) + 1 FROM milestones WHERE repo_id = $1), $2, $3, $4, $5, $6,
                 CASE WHEN $7 THEN now() END)
         RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(title)
    .bind(input.description.flatten().filter(|d| !d.is_empty()))
    .bind(if closed { "closed" } else { "open" })
    .bind(input.due_on.flatten())
    .bind(actor.id)
    .bind(closed)
    .fetch_one(&mut *tx)
    .await
    .map_err(milestone_conflict)?;
    tx.commit().await?;
    Ok(row)
}

pub async fn update_milestone(state: &AppState, ctx: &RepoCtx, number: i64, input: MilestoneInput) -> ApiResult<MilestoneRow> {
    ctx.require(Perm::Write)?;
    check_state(&input.state)?;
    let cur = milestone_row(state, ctx.repo.id, number).await?;
    let title = input.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&cur.title).to_string();
    let description = match input.description {
        Some(d) => d.filter(|d| !d.is_empty()),
        None => cur.description.clone(),
    };
    let due_on = match input.due_on {
        Some(d) => d,
        None => cur.due_on,
    };
    let new_state = input.state.unwrap_or_else(|| cur.state.clone());
    let closed_at = match (cur.state.as_str(), new_state.as_str()) {
        ("open", "closed") => Some(Utc::now()),
        (_, "open") => None,
        _ => cur.closed_at,
    };
    sqlx::query_as(
        "UPDATE milestones SET title = $2, description = $3, due_on = $4, state = $5, closed_at = $6, updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(cur.id)
    .bind(title)
    .bind(description)
    .bind(due_on)
    .bind(new_state)
    .bind(closed_at)
    .fetch_one(&state.db)
    .await
    .map_err(milestone_conflict)
}

pub async fn delete_milestone(state: &AppState, ctx: &RepoCtx, number: i64) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    let cur = milestone_row(state, ctx.repo.id, number).await?;
    sqlx::query("DELETE FROM milestones WHERE id = $1").bind(cur.id).execute(&state.db).await?;
    Ok(())
}

// Milestones: handlers.

async fn milestones_json(s: &AppState, ctx: &RepoCtx, rows: &[MilestoneRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
    let full = ji::load_milestones(&s.db, &ids).await?;
    let fname = ctx.full_name();
    Ok(rows.iter().filter_map(|m| full.get(&m.id)).map(|m| ji::milestone(&s.api_url(), s.base_url(), &fname, m)).collect())
}

async fn milestone_json(s: &AppState, ctx: &RepoCtx, row: MilestoneRow) -> ApiResult<Value> {
    milestones_json(s, ctx, &[row]).await?.pop().ok_or_else(ApiError::not_found)
}

#[derive(Deserialize, Default)]
struct ListMilestones {
    state: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_milestones(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListMilestones>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let state_cond = match q.state.as_deref().unwrap_or("open") {
        "open" => "AND m.state = 'open'",
        "closed" => "AND m.state = 'closed'",
        "all" => "",
        other => return Err(invalid("Milestone", "state", other)),
    };
    let dir = if q.direction.as_deref() == Some("desc") { "DESC" } else { "ASC" };
    let order = match q.sort.as_deref() {
        Some("completeness") => format!(
            "(SELECT CASE WHEN count(*) = 0 THEN 0 ELSE count(*) FILTER (WHERE i.state = 'closed')::float8 / count(*) END
                FROM issues i WHERE i.milestone_id = m.id) {dir}, m.number {dir}"
        ),
        _ => format!("m.due_on {dir} NULLS LAST, m.number {dir}"),
    };
    let base = format!("FROM milestones m WHERE m.repo_id = $1 {state_cond}");
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {base}")))
        .bind(ctx.repo.id)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<MilestoneRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT m.* {base} ORDER BY {order} LIMIT $2 OFFSET $3")))
        .bind(ctx.repo.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = milestones_json(&s, &ctx, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
struct MilestoneBody {
    title: Option<String>,
    state: Option<String>,
    #[serde(default, deserialize_with = "double_opt")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_opt")]
    due_on: Option<Option<String>>,
}

impl MilestoneBody {
    fn into_input(self) -> ApiResult<MilestoneInput> {
        let due_on = match self.due_on {
            None => None,
            Some(None) => Some(None),
            Some(Some(s)) if s.is_empty() => Some(None),
            Some(Some(s)) => Some(Some(parse_time(&s).ok_or_else(|| invalid("Milestone", "due_on", s.as_str()))?)),
        };
        Ok(MilestoneInput { title: self.title, state: self.state, description: self.description, due_on })
    }
}

async fn create_milestone_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<MilestoneBody>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let row = create_milestone(&s, &ctx, &u, b.into_input()?).await?;
    Ok(created(milestone_json(&s, &ctx, row).await?))
}

async fn get_milestone(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let row = milestone_row(&s, ctx.repo.id, number).await?;
    Ok(Json(milestone_json(&s, &ctx, row).await?))
}

async fn update_milestone_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Json(b): Json<MilestoneBody>,
) -> ApiResult<Json<Value>> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    let row = update_milestone(&s, &ctx, number, b.into_input()?).await?;
    Ok(Json(milestone_json(&s, &ctx, row).await?))
}

async fn delete_milestone_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = load_ctx(&s, Some(&u), &owner, &repo).await?;
    delete_milestone(&s, &ctx, number).await?;
    Ok(no_content())
}

/// Labels used by issues in a milestone.
async fn milestone_labels(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, number)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = load_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let m = milestone_row(&s, ctx.repo.id, number).await?;
    let rows: Vec<LabelRow> = sqlx::query_as(
        "SELECT l.* FROM labels l WHERE l.id IN (
            SELECT il.label_id FROM issue_labels il JOIN issues i ON i.id = il.issue_id WHERE i.milestone_id = $1)
          ORDER BY lower(l.name) LIMIT $2 OFFSET $3",
    )
    .bind(m.id)
    .bind(p.limit() + 1)
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let (rows, total) = super::issues::page_window(rows, &p);
    let items = rows.iter().map(|l| label_json(&s, &ctx, l)).collect();
    Ok(Paged::new(&uri, &s, p, total, items))
}
