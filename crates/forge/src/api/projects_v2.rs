//! GitHub Projects (v2) service layer: projects owned by a user or an organization, their
//! fields (built-in ones like Title/Status plus custom text/number/date/single-select/iteration
//! fields), items (issues, pull requests, draft issues) and item field values.
//!
//! GitHub only exposes Projects v2 through GraphQL; `graphql::projects` adapts these services.
//! Access: a user's projects are managed by that user; an organization's by its admins (members
//! can read and create). Public projects are readable by everyone.
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};

use crate::db::User;
use crate::error::{ApiError, ApiResult};
use crate::json::issue::IssueRow;
use crate::state::AppState;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProjectRow {
    pub id: i64,
    pub owner_id: i64,
    pub number: i64,
    pub title: String,
    pub short_description: Option<String>,
    pub readme: Option<String>,
    pub public: bool,
    pub closed: bool,
    pub closed_at: Option<DateTime<Utc>>,
    pub template: bool,
    pub creator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FieldRow {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub data_type: String,
    pub options: Value,
    pub config: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl FieldRow {
    /// Custom fields hold stored values; the others are derived from the item's content.
    pub fn is_custom(&self) -> bool {
        matches!(self.data_type.as_str(), "TEXT" | "NUMBER" | "DATE" | "SINGLE_SELECT" | "ITERATION")
    }
    /// GraphQL type of the field configuration.
    pub fn gql_kind(&self) -> &'static str {
        match self.data_type.as_str() {
            "SINGLE_SELECT" => "ProjectV2SingleSelectField",
            "ITERATION" => "ProjectV2IterationField",
            _ => "ProjectV2Field",
        }
    }
    pub fn option_name(&self, id: &str) -> Option<String> {
        self.options.as_array()?.iter().find(|o| o["id"] == id).and_then(|o| o["name"].as_str().map(str::to_string))
    }
    pub fn iteration(&self, id: &str) -> Option<Value> {
        self.config["iterations"].as_array()?.iter().find(|i| i["id"] == id).cloned()
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ItemRow {
    pub id: i64,
    pub project_id: i64,
    pub issue_id: Option<i64>,
    pub draft_title: Option<String>,
    pub draft_body: Option<String>,
    pub archived: bool,
    pub creator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    None,
    Read,
    Write,
}

fn forbidden() -> ApiError {
    ApiError::forbidden("You don't have permission to modify this project.")
}

fn invalid(msg: impl Into<String>) -> ApiError {
    ApiError::unprocessable(msg)
}

/// The viewer's access to projects of `owner` (a user or organization row).
pub async fn owner_access(state: &AppState, viewer: Option<&User>, owner: &User) -> ApiResult<Access> {
    let Some(v) = viewer else { return Ok(Access::None) };
    if v.id == owner.id {
        return Ok(Access::Write);
    }
    if owner.kind == "Organization" {
        let role: Option<String> = sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(owner.id)
            .bind(v.id)
            .fetch_optional(&state.db)
            .await?;
        return Ok(match role.as_deref() {
            Some("admin") => Access::Write,
            Some(_) => Access::Read,
            None => Access::None,
        });
    }
    Ok(Access::None)
}

/// Whether `viewer` may create projects for `owner` (the user themselves, or org members).
pub async fn can_create(state: &AppState, viewer: &User, owner: &User) -> ApiResult<bool> {
    Ok(owner_access(state, Some(viewer), owner).await? >= Access::Read && (owner.kind == "Organization" || owner.id == viewer.id))
}

pub async fn access(state: &AppState, viewer: Option<&User>, p: &ProjectRow) -> ApiResult<Access> {
    let owner = crate::db::user_by_id(&state.db, p.owner_id).await?;
    let a = owner_access(state, viewer, &owner).await?;
    Ok(if a == Access::None && p.public { Access::Read } else { a })
}

pub async fn project_by_id(state: &AppState, id: i64) -> ApiResult<Option<ProjectRow>> {
    Ok(sqlx::query_as("SELECT * FROM projects_v2 WHERE id = $1").bind(id).fetch_optional(&state.db).await?)
}

/// A project the viewer can read, by id.
pub async fn readable(state: &AppState, viewer: Option<&User>, id: i64) -> ApiResult<Option<ProjectRow>> {
    match project_by_id(state, id).await? {
        Some(p) if access(state, viewer, &p).await? >= Access::Read => Ok(Some(p)),
        _ => Ok(None),
    }
}

async fn writable(state: &AppState, actor: &User, id: i64) -> ApiResult<ProjectRow> {
    let p = project_by_id(state, id).await?.ok_or_else(ApiError::not_found)?;
    match access(state, Some(actor), &p).await? {
        Access::Write => Ok(p),
        Access::Read => Err(forbidden()),
        Access::None => Err(ApiError::not_found()),
    }
}

pub async fn by_number(state: &AppState, viewer: Option<&User>, owner_id: i64, number: i64) -> ApiResult<Option<ProjectRow>> {
    let p: Option<ProjectRow> = sqlx::query_as("SELECT * FROM projects_v2 WHERE owner_id = $1 AND number = $2")
        .bind(owner_id)
        .bind(number)
        .fetch_optional(&state.db)
        .await?;
    match p {
        Some(p) if access(state, viewer, &p).await? >= Access::Read => Ok(Some(p)),
        _ => Ok(None),
    }
}

/// Projects of an owner the viewer can see, newest first; `query` matches the title and
/// understands `is:open` / `is:closed` / `is:template`.
pub async fn list_for_owner(state: &AppState, viewer: Option<&User>, owner: &User, query: Option<&str>) -> ApiResult<Vec<ProjectRow>> {
    let full = owner_access(state, viewer, owner).await? >= Access::Read;
    let rows: Vec<ProjectRow> = sqlx::query_as("SELECT * FROM projects_v2 WHERE owner_id = $1 AND (public OR $2) ORDER BY number DESC")
        .bind(owner.id)
        .bind(full)
        .fetch_all(&state.db)
        .await?;
    let (mut words, mut state_filter, mut template) = (Vec::new(), None, false);
    for t in query.unwrap_or("").split_whitespace() {
        match t {
            "is:open" => state_filter = Some(false),
            "is:closed" => state_filter = Some(true),
            "is:template" => template = true,
            w => words.push(w.to_lowercase()),
        }
    }
    Ok(rows
        .into_iter()
        .filter(|p| state_filter.is_none_or(|c| p.closed == c))
        .filter(|p| !template || p.template)
        .filter(|p| words.iter().all(|w| p.title.to_lowercase().contains(w.as_str())))
        .collect())
}

/// Projects linked to a repository that the viewer can see.
pub async fn list_for_repo(state: &AppState, viewer: Option<&User>, repo_id: i64) -> ApiResult<Vec<ProjectRow>> {
    let rows: Vec<ProjectRow> = sqlx::query_as(
        "SELECT p.* FROM projects_v2 p JOIN projects_v2_repositories r ON r.project_id = p.id WHERE r.repo_id = $1 ORDER BY p.number DESC",
    )
    .bind(repo_id)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for p in rows {
        if access(state, viewer, &p).await? >= Access::Read {
            out.push(p);
        }
    }
    Ok(out)
}

/// The fields GitHub gives every new project.
const DEFAULT_FIELDS: &[(&str, &str)] = &[
    ("Title", "TITLE"),
    ("Assignees", "ASSIGNEES"),
    ("Status", "SINGLE_SELECT"),
    ("Labels", "LABELS"),
    ("Linked pull requests", "LINKED_PULL_REQUESTS"),
    ("Milestone", "MILESTONE"),
    ("Repository", "REPOSITORY"),
    ("Reviewers", "REVIEWERS"),
];

fn option_id() -> String {
    crate::auth::random_string(8).to_lowercase()
}

fn status_options() -> Value {
    json!([
        { "id": option_id(), "name": "Todo", "color": "GREEN", "description": "This item hasn't been started" },
        { "id": option_id(), "name": "In Progress", "color": "YELLOW", "description": "This is actively being worked on" },
        { "id": option_id(), "name": "Done", "color": "PURPLE", "description": "This has been completed" },
    ])
}

/// Three two-week iterations starting today (GitHub's default iteration configuration).
fn iteration_config(duration: i64, start: NaiveDate) -> Value {
    let iterations: Vec<Value> = (0..3)
        .map(|i| {
            let s = start + chrono::Duration::days(i * duration);
            json!({ "id": option_id(), "title": format!("Iteration {}", i + 1), "start_date": s.to_string(), "duration": duration })
        })
        .collect();
    json!({ "duration": duration, "start_day": start.format("%u").to_string().parse::<i64>().unwrap_or(1), "iterations": iterations })
}

async fn insert_project(conn: &mut sqlx::PgConnection, owner_id: i64, creator: i64, title: &str) -> ApiResult<ProjectRow> {
    // Serialize numbering per owner.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('projects_v2:' || $1::text, 0))").bind(owner_id).execute(&mut *conn).await?;
    Ok(sqlx::query_as(
        "INSERT INTO projects_v2 (owner_id, number, title, creator_id)
         VALUES ($1, (SELECT COALESCE(MAX(number), 0) + 1 FROM projects_v2 WHERE owner_id = $1), $2, $3) RETURNING *",
    )
    .bind(owner_id)
    .bind(title)
    .bind(creator)
    .fetch_one(&mut *conn)
    .await?)
}

async fn insert_field(conn: &mut sqlx::PgConnection, project_id: i64, name: &str, data_type: &str, options: Value, config: Value) -> ApiResult<FieldRow> {
    sqlx::query_as("INSERT INTO projects_v2_fields (project_id, name, data_type, options, config) VALUES ($1,$2,$3,$4,$5) RETURNING *")
        .bind(project_id)
        .bind(name)
        .bind(data_type)
        .bind(options)
        .bind(config)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => invalid(format!("Name has already been taken: {name}")),
            e => e.into(),
        })
}

/// Creates a project with GitHub's default fields.
pub async fn create_project(state: &AppState, actor: &User, owner: &User, title: &str) -> ApiResult<ProjectRow> {
    if title.trim().is_empty() {
        return Err(invalid("Title can't be blank"));
    }
    if !can_create(state, actor, owner).await? {
        return Err(ApiError::forbidden(format!("{} does not have permission to create projects for {}.", actor.login, owner.login)));
    }
    let mut tx = state.db.begin().await?;
    let p = insert_project(&mut tx, owner.id, actor.id, title.trim()).await?;
    for (name, ty) in DEFAULT_FIELDS {
        let options = if *ty == "SINGLE_SELECT" { status_options() } else { json!([]) };
        insert_field(&mut tx, p.id, name, ty, options, json!({})).await?;
    }
    tx.commit().await?;
    Ok(p)
}

#[derive(Debug, Default, Clone)]
pub struct ProjectPatch {
    pub title: Option<String>,
    pub short_description: Option<String>,
    pub readme: Option<String>,
    pub public: Option<bool>,
    pub closed: Option<bool>,
    pub template: Option<bool>,
}

pub async fn update_project(state: &AppState, actor: &User, project_id: i64, p: ProjectPatch) -> ApiResult<ProjectRow> {
    let cur = writable(state, actor, project_id).await?;
    if p.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err(invalid("Title can't be blank"));
    }
    if p.template.is_some() {
        let owner = crate::db::user_by_id(&state.db, cur.owner_id).await?;
        if owner.kind != "Organization" {
            return Err(invalid("Only organization projects can be templates."));
        }
    }
    Ok(sqlx::query_as(
        "UPDATE projects_v2 SET title = COALESCE($2, title), short_description = COALESCE($3, short_description),
            readme = COALESCE($4, readme), public = COALESCE($5, public),
            closed_at = CASE WHEN $6 IS NULL THEN closed_at WHEN $6 THEN COALESCE(closed_at, now()) ELSE NULL END,
            closed = COALESCE($6, closed), template = COALESCE($7, template), updated_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(project_id)
    .bind(p.title.map(|t| t.trim().to_string()))
    .bind(p.short_description)
    .bind(p.readme)
    .bind(p.public)
    .bind(p.closed)
    .bind(p.template)
    .fetch_one(&state.db)
    .await?)
}

/// Deletes a project; returns it as it was.
pub async fn delete_project(state: &AppState, actor: &User, project_id: i64) -> ApiResult<ProjectRow> {
    let p = writable(state, actor, project_id).await?;
    sqlx::query("DELETE FROM projects_v2 WHERE id = $1").bind(p.id).execute(&state.db).await?;
    Ok(p)
}

/// Copies a project's settings, fields (and optionally draft issues) to a new project of `owner`.
pub async fn copy_project(state: &AppState, actor: &User, project_id: i64, owner: &User, title: &str, include_drafts: bool) -> ApiResult<ProjectRow> {
    let src = readable(state, Some(actor), project_id).await?.ok_or_else(ApiError::not_found)?;
    if title.trim().is_empty() {
        return Err(invalid("Title can't be blank"));
    }
    if !can_create(state, actor, owner).await? {
        return Err(ApiError::forbidden(format!("{} does not have permission to create projects for {}.", actor.login, owner.login)));
    }
    let mut tx = state.db.begin().await?;
    let p = insert_project(&mut tx, owner.id, actor.id, title.trim()).await?;
    let p: ProjectRow = sqlx::query_as("UPDATE projects_v2 SET short_description = $2, readme = $3 WHERE id = $1 RETURNING *")
        .bind(p.id)
        .bind(&src.short_description)
        .bind(&src.readme)
        .fetch_one(&mut *tx)
        .await?;
    let fields: Vec<FieldRow> = sqlx::query_as("SELECT * FROM projects_v2_fields WHERE project_id = $1 ORDER BY id").bind(src.id).fetch_all(&mut *tx).await?;
    let mut field_map = std::collections::HashMap::new();
    for f in fields {
        let nf = insert_field(&mut tx, p.id, &f.name, &f.data_type, f.options.clone(), f.config.clone()).await?;
        field_map.insert(f.id, nf.id);
    }
    if include_drafts {
        let drafts: Vec<ItemRow> = sqlx::query_as("SELECT * FROM projects_v2_items WHERE project_id = $1 AND issue_id IS NULL ORDER BY id")
            .bind(src.id)
            .fetch_all(&mut *tx)
            .await?;
        for d in drafts {
            let new_id: i64 = sqlx::query_scalar(
                "INSERT INTO projects_v2_items (project_id, draft_title, draft_body, archived, creator_id) VALUES ($1,$2,$3,$4,$5) RETURNING id",
            )
            .bind(p.id)
            .bind(&d.draft_title)
            .bind(&d.draft_body)
            .bind(d.archived)
            .bind(actor.id)
            .fetch_one(&mut *tx)
            .await?;
            let values: Vec<(i64, Value)> = sqlx::query_as("SELECT field_id, value FROM projects_v2_item_values WHERE item_id = $1").bind(d.id).fetch_all(&mut *tx).await?;
            for (fid, v) in values {
                if let Some(nf) = field_map.get(&fid) {
                    sqlx::query("INSERT INTO projects_v2_item_values (item_id, field_id, value) VALUES ($1,$2,$3)")
                        .bind(new_id)
                        .bind(nf)
                        .bind(v)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
    }
    tx.commit().await?;
    Ok(p)
}

/// Links (or unlinks) a project and a repository of the same owner the actor can administer.
pub async fn set_repo_link(state: &AppState, actor: &User, project_id: i64, repo: &crate::db::RepoCtx, link: bool) -> ApiResult<()> {
    let p = writable(state, actor, project_id).await?;
    repo.require(crate::db::Perm::Write)?;
    if repo.owner.id != p.owner_id {
        return Err(invalid("A project can only be linked to repositories of the same owner."));
    }
    if link {
        sqlx::query("INSERT INTO projects_v2_repositories (project_id, repo_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(p.id)
            .bind(repo.repo.id)
            .execute(&state.db)
            .await?;
    } else {
        sqlx::query("DELETE FROM projects_v2_repositories WHERE project_id = $1 AND repo_id = $2").bind(p.id).bind(repo.repo.id).execute(&state.db).await?;
    }
    Ok(())
}

pub async fn linked_repo_ids(state: &AppState, project_id: i64) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT repo_id FROM projects_v2_repositories WHERE project_id = $1 ORDER BY created_at")
        .bind(project_id)
        .fetch_all(&state.db)
        .await?)
}

// ---------------------------------------------------------------------------------------------
// Fields

pub async fn fields(state: &AppState, project_id: i64) -> ApiResult<Vec<FieldRow>> {
    Ok(sqlx::query_as("SELECT * FROM projects_v2_fields WHERE project_id = $1 ORDER BY id").bind(project_id).fetch_all(&state.db).await?)
}

pub async fn field_by_id(state: &AppState, id: i64) -> ApiResult<Option<FieldRow>> {
    Ok(sqlx::query_as("SELECT * FROM projects_v2_fields WHERE id = $1").bind(id).fetch_optional(&state.db).await?)
}

/// A single-select option for [`create_field`].
#[derive(Debug, Clone)]
pub struct NewOption {
    pub name: String,
    pub color: String,
    pub description: String,
}

/// Creates a custom field (`TEXT`, `NUMBER`, `DATE`, `SINGLE_SELECT`, `ITERATION`).
pub async fn create_field(
    state: &AppState,
    actor: &User,
    project_id: i64,
    name: &str,
    data_type: &str,
    options: Vec<NewOption>,
    iteration_duration: Option<i64>,
    iteration_start: Option<NaiveDate>,
) -> ApiResult<FieldRow> {
    let p = writable(state, actor, project_id).await?;
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("Name can't be blank"));
    }
    let (opts, config) = match data_type {
        "TEXT" | "NUMBER" | "DATE" => (json!([]), json!({})),
        "SINGLE_SELECT" => {
            if options.is_empty() {
                return Err(invalid("Single select fields need at least one option."));
            }
            let opts: Vec<Value> = options
                .iter()
                .map(|o| json!({ "id": option_id(), "name": o.name, "color": o.color, "description": o.description }))
                .collect();
            (Value::Array(opts), json!({}))
        }
        "ITERATION" => (json!([]), iteration_config(iteration_duration.unwrap_or(14).clamp(1, 365), iteration_start.unwrap_or_else(|| Utc::now().date_naive()))),
        other => return Err(invalid(format!("Data type {other} can't be used for custom fields."))),
    };
    let mut conn = state.db.acquire().await?;
    let f = insert_field(&mut conn, p.id, name, data_type, opts, config).await?;
    touch(state, p.id).await?;
    Ok(f)
}

/// Deletes a custom field (built-in fields can't be deleted); returns it as it was.
pub async fn delete_field(state: &AppState, actor: &User, field_id: i64) -> ApiResult<FieldRow> {
    let f = field_by_id(state, field_id).await?.ok_or_else(ApiError::not_found)?;
    writable(state, actor, f.project_id).await?;
    if !f.is_custom() {
        return Err(invalid(format!("The {} field can't be deleted.", f.name)));
    }
    sqlx::query("DELETE FROM projects_v2_fields WHERE id = $1").bind(f.id).execute(&state.db).await?;
    touch(state, f.project_id).await?;
    Ok(f)
}

async fn touch(state: &AppState, project_id: i64) -> ApiResult<()> {
    sqlx::query("UPDATE projects_v2 SET updated_at = now() WHERE id = $1").bind(project_id).execute(&state.db).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Items

pub async fn item_by_id(state: &AppState, id: i64) -> ApiResult<Option<ItemRow>> {
    Ok(sqlx::query_as("SELECT * FROM projects_v2_items WHERE id = $1").bind(id).fetch_optional(&state.db).await?)
}

/// Items of a project in insertion order.
pub async fn items(state: &AppState, project_id: i64) -> ApiResult<Vec<ItemRow>> {
    Ok(sqlx::query_as("SELECT * FROM projects_v2_items WHERE project_id = $1 ORDER BY id").bind(project_id).fetch_all(&state.db).await?)
}

/// Project items of an issue or pull request.
pub async fn items_for_issue(state: &AppState, issue_id: i64) -> ApiResult<Vec<ItemRow>> {
    Ok(sqlx::query_as("SELECT * FROM projects_v2_items WHERE issue_id = $1 ORDER BY id").bind(issue_id).fetch_all(&state.db).await?)
}

/// Adds an issue or pull request (the viewer must be able to read it). Idempotent like GitHub:
/// adding content that is already on the project returns the existing item.
pub async fn add_issue(state: &AppState, actor: &User, project_id: i64, issue: &IssueRow) -> ApiResult<ItemRow> {
    let p = writable(state, actor, project_id).await?;
    let row: Option<ItemRow> = sqlx::query_as(
        "INSERT INTO projects_v2_items (project_id, issue_id, creator_id) VALUES ($1, $2, $3) ON CONFLICT (project_id, issue_id) DO NOTHING RETURNING *",
    )
    .bind(p.id)
    .bind(issue.id)
    .bind(actor.id)
    .fetch_optional(&state.db)
    .await?;
    touch(state, p.id).await?;
    match row {
        Some(r) => Ok(r),
        None => Ok(sqlx::query_as("SELECT * FROM projects_v2_items WHERE project_id = $1 AND issue_id = $2")
            .bind(p.id)
            .bind(issue.id)
            .fetch_one(&state.db)
            .await?),
    }
}

pub async fn add_draft(state: &AppState, actor: &User, project_id: i64, title: &str, body: Option<&str>) -> ApiResult<ItemRow> {
    let p = writable(state, actor, project_id).await?;
    if title.trim().is_empty() {
        return Err(invalid("Title can't be blank"));
    }
    let row = sqlx::query_as("INSERT INTO projects_v2_items (project_id, draft_title, draft_body, creator_id) VALUES ($1,$2,$3,$4) RETURNING *")
        .bind(p.id)
        .bind(title.trim())
        .bind(body.unwrap_or(""))
        .bind(actor.id)
        .fetch_one(&state.db)
        .await?;
    touch(state, p.id).await?;
    Ok(row)
}

/// The item and its project, checking the actor can modify it (and, when given, that the item
/// belongs to `project_id`).
async fn writable_item(state: &AppState, actor: &User, item_id: i64, project_id: Option<i64>) -> ApiResult<ItemRow> {
    let item = item_by_id(state, item_id).await?.ok_or_else(ApiError::not_found)?;
    if project_id.is_some_and(|p| p != item.project_id) {
        return Err(ApiError::not_found());
    }
    writable(state, actor, item.project_id).await?;
    Ok(item)
}

pub async fn update_draft(state: &AppState, actor: &User, item_id: i64, title: Option<&str>, body: Option<&str>) -> ApiResult<ItemRow> {
    let item = writable_item(state, actor, item_id, None).await?;
    if item.issue_id.is_some() {
        return Err(ApiError::not_found());
    }
    if title.is_some_and(|t| t.trim().is_empty()) {
        return Err(invalid("Title can't be blank"));
    }
    Ok(sqlx::query_as(
        "UPDATE projects_v2_items SET draft_title = COALESCE($2, draft_title), draft_body = COALESCE($3, draft_body), updated_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(item.id)
    .bind(title.map(str::trim))
    .bind(body)
    .fetch_one(&state.db)
    .await?)
}

pub async fn set_archived(state: &AppState, actor: &User, project_id: i64, item_id: i64, archived: bool) -> ApiResult<ItemRow> {
    let item = writable_item(state, actor, item_id, Some(project_id)).await?;
    Ok(sqlx::query_as("UPDATE projects_v2_items SET archived = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(item.id)
        .bind(archived)
        .fetch_one(&state.db)
        .await?)
}

pub async fn delete_item(state: &AppState, actor: &User, project_id: i64, item_id: i64) -> ApiResult<ItemRow> {
    let item = writable_item(state, actor, item_id, Some(project_id)).await?;
    sqlx::query("DELETE FROM projects_v2_items WHERE id = $1").bind(item.id).execute(&state.db).await?;
    touch(state, project_id).await?;
    Ok(item)
}

/// A value for a custom field.
#[derive(Debug, Clone)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Date(NaiveDate),
    SingleSelect(String),
    Iteration(String),
}

/// Stored values of an item, by field id.
pub async fn item_values(state: &AppState, item_id: i64) -> ApiResult<Vec<(i64, Value)>> {
    Ok(sqlx::query_as("SELECT field_id, value FROM projects_v2_item_values WHERE item_id = $1 ORDER BY field_id")
        .bind(item_id)
        .fetch_all(&state.db)
        .await?)
}

pub async fn set_value(state: &AppState, actor: &User, project_id: i64, item_id: i64, field_id: i64, v: FieldValue) -> ApiResult<ItemRow> {
    let item = writable_item(state, actor, item_id, Some(project_id)).await?;
    let f = field_by_id(state, field_id).await?.filter(|f| f.project_id == project_id).ok_or_else(ApiError::not_found)?;
    let stored = match (f.data_type.as_str(), v) {
        ("TEXT", FieldValue::Text(t)) => json!({ "text": t }),
        ("NUMBER", FieldValue::Number(n)) => json!({ "number": n }),
        ("DATE", FieldValue::Date(d)) => json!({ "date": d.to_string() }),
        ("SINGLE_SELECT", FieldValue::SingleSelect(o)) => {
            if f.option_name(&o).is_none() {
                return Err(invalid(format!("The single select option Id does not belong to the field {}", f.name)));
            }
            json!({ "option_id": o })
        }
        ("ITERATION", FieldValue::Iteration(i)) => {
            if f.iteration(&i).is_none() {
                return Err(invalid(format!("The iteration Id does not belong to the field {}", f.name)));
            }
            json!({ "iteration_id": i })
        }
        (ty, _) if !f.is_custom() => return Err(invalid(format!("The field of type {} is currently not supported", ty.to_lowercase()))),
        (ty, _) => return Err(invalid(format!("The value does not match the field's data type ({ty})."))),
    };
    sqlx::query(
        "INSERT INTO projects_v2_item_values (item_id, field_id, value) VALUES ($1, $2, $3)
         ON CONFLICT (item_id, field_id) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(item.id)
    .bind(f.id)
    .bind(stored)
    .execute(&state.db)
    .await?;
    Ok(sqlx::query_as("UPDATE projects_v2_items SET updated_at = now() WHERE id = $1 RETURNING *").bind(item.id).fetch_one(&state.db).await?)
}

pub async fn clear_value(state: &AppState, actor: &User, project_id: i64, item_id: i64, field_id: i64) -> ApiResult<ItemRow> {
    let item = writable_item(state, actor, item_id, Some(project_id)).await?;
    field_by_id(state, field_id).await?.filter(|f| f.project_id == project_id).ok_or_else(ApiError::not_found)?;
    sqlx::query("DELETE FROM projects_v2_item_values WHERE item_id = $1 AND field_id = $2").bind(item.id).bind(field_id).execute(&state.db).await?;
    Ok(sqlx::query_as("UPDATE projects_v2_items SET updated_at = now() WHERE id = $1 RETURNING *").bind(item.id).fetch_one(&state.db).await?)
}
