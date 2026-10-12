//! Mutation facade used by the GraphQL resolvers.
//!
//! These are deliberately small, self-contained implementations so gh works end to end on this
//! branch. The REST areas (issues, pulls, labels, ...) own the canonical service functions; when
//! they land, re-point each function here at them (signatures mirror the REST service-layer
//! convention: `(state, repo ctx, actor, input) -> ApiResult<Row>`).
use axum::http::StatusCode;

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::git::cmd::Identity;
use crate::git::plumbing;
use crate::json::issue::{IssueRow, LabelRow};
use crate::state::AppState;

use super::issue::CommentRow;
use super::pull::{PullRow, ReviewRow};

fn identity(u: &User) -> Identity {
    crate::api::repos::identity_for(u)
}

fn gone(msg: &str) -> ApiError {
    ApiError::new(StatusCode::GONE, msg)
}

// ---------------------------------------------------------------------------------------------
// Repositories

#[derive(Default, Debug, Clone)]
pub struct NewRepo {
    pub name: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    /// "public" | "private" | "internal"
    pub visibility: String,
    pub has_issues: Option<bool>,
    pub has_wiki: Option<bool>,
    pub is_template: Option<bool>,
}

/// Creates a repository for `owner` (the actor or an org the actor belongs to).
pub async fn create_repository(state: &AppState, owner: &User, actor: &User, input: NewRepo) -> ApiResult<RepoCtx> {
    if owner.id != actor.id && !actor.site_admin {
        let member: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)")
            .bind(owner.id)
            .bind(actor.id)
            .fetch_one(&state.db)
            .await?;
        if !member || owner.kind != "Organization" {
            return Err(ApiError::forbidden(format!("{} does not have the correct permissions to execute `CreateRepository`", actor.login)));
        }
    }
    let req = crate::api::repos::CreateRepo {
        name: input.name,
        description: input.description,
        homepage: input.homepage,
        private: Some(input.visibility != "public"),
        visibility: Some(input.visibility),
        has_issues: input.has_issues,
        has_wiki: input.has_wiki,
        is_template: input.is_template,
        ..Default::default()
    };
    crate::api::repos::create_repo(state, owner, actor, req).await
}

#[derive(Default, Debug, Clone)]
pub struct RepoPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub has_issues: Option<bool>,
    pub has_wiki: Option<bool>,
    pub has_projects: Option<bool>,
    pub has_discussions: Option<bool>,
    pub is_template: Option<bool>,
}

pub async fn update_repository(state: &AppState, ctx: &RepoCtx, actor: &User, p: RepoPatch) -> ApiResult<RepoCtx> {
    let _ = actor;
    ctx.require(Perm::Admin)?;
    if let Some(n) = &p.name {
        if !crate::api::repos::valid_repo_name(n) {
            return Err(ApiError::validation("Repository", "name", "custom"));
        }
    }
    let r: db::Repo = sqlx::query_as(
        "UPDATE repositories SET name = COALESCE($2, name), description = COALESCE($3, description),
            homepage = COALESCE($4, homepage), has_issues = COALESCE($5, has_issues), has_wiki = COALESCE($6, has_wiki),
            has_projects = COALESCE($7, has_projects), has_discussions = COALESCE($8, has_discussions),
            is_template = COALESCE($9, is_template), updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(&p.name)
    .bind(&p.description)
    .bind(&p.homepage)
    .bind(p.has_issues)
    .bind(p.has_wiki)
    .bind(p.has_projects)
    .bind(p.has_discussions)
    .bind(p.is_template)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Repository", "name", "already_exists"),
        e => e.into(),
    })?;
    Ok(RepoCtx { repo: r, owner: ctx.owner.clone(), perm: ctx.perm })
}

// ---------------------------------------------------------------------------------------------
// Issues

#[derive(Default, Debug, Clone)]
pub struct NewIssue {
    pub title: String,
    pub body: Option<String>,
    pub labels: Vec<i64>,
    pub assignees: Vec<i64>,
    pub milestone: Option<i64>,
}

async fn check_labels(state: &AppState, ctx: &RepoCtx, ids: &[i64]) -> ApiResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE repo_id = $1 AND id = ANY($2)")
        .bind(ctx.repo.id)
        .bind(ids)
        .fetch_one(&state.db)
        .await?;
    if n as usize != ids.iter().collect::<std::collections::HashSet<_>>().len() {
        return Err(ApiError::validation("Label", "labelIds", "invalid"));
    }
    Ok(())
}

async fn check_milestone(state: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<()> {
    let ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM milestones WHERE repo_id = $1 AND id = $2)")
        .bind(ctx.repo.id)
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if ok { Ok(()) } else { Err(ApiError::validation("Issue", "milestone", "invalid")) }
}

pub async fn create_issue(state: &AppState, ctx: &RepoCtx, actor: &User, input: NewIssue) -> ApiResult<IssueRow> {
    if !ctx.repo.has_issues {
        return Err(gone("Issues are disabled for this repo"));
    }
    if input.title.trim().is_empty() {
        return Err(ApiError::validation("Issue", "title", "missing_field"));
    }
    let triage = ctx.perm >= Perm::Triage;
    if triage {
        check_labels(state, ctx, &input.labels).await?;
        if let Some(m) = input.milestone {
            check_milestone(state, ctx, m).await?;
        }
    }
    let mut tx = state.db.begin().await?;
    let number = db::next_issue_number(&mut *tx, ctx.repo.id).await?;
    let row: IssueRow = sqlx::query_as(
        "INSERT INTO issues (repo_id, number, title, body, user_id, milestone_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(number)
    .bind(&input.title)
    .bind(input.body.as_ref().filter(|b| !b.is_empty()))
    .bind(actor.id)
    .bind(if triage { input.milestone } else { None })
    .fetch_one(&mut *tx)
    .await?;
    if triage {
        sqlx::query("INSERT INTO issue_labels (issue_id, label_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(row.id)
            .bind(&input.labels)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO issue_assignees (issue_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(row.id)
            .bind(&input.assignees)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE repositories SET open_issues_count = open_issues_count + 1 WHERE id = $1")
        .bind(ctx.repo.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(row)
}

#[derive(Default, Debug, Clone)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    /// "open" | "closed"
    pub state: Option<String>,
    /// For closing: "completed" | "not_planned" | "duplicate".
    pub state_reason: Option<String>,
    pub milestone: Option<Option<i64>>,
    pub assignees: Option<Vec<i64>>,
    pub labels: Option<Vec<i64>>,
}

pub async fn update_issue(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, p: IssuePatch) -> ApiResult<IssueRow> {
    let author = issue.user_id == Some(actor.id);
    let triage = ctx.perm >= Perm::Triage;
    if !(triage || author) {
        return Err(ApiError::forbidden(format!("{} does not have the correct permissions to execute `UpdateIssue`", actor.login)));
    }
    if (p.milestone.is_some() || p.assignees.is_some() || p.labels.is_some()) && !triage {
        return Err(ApiError::forbidden("Must have triage access to change labels, assignees or milestones."));
    }
    if let Some(t) = &p.title {
        if t.trim().is_empty() {
            return Err(ApiError::validation("Issue", "title", "missing_field"));
        }
    }
    if let Some(ls) = &p.labels {
        check_labels(state, ctx, ls).await?;
    }
    if let Some(Some(m)) = p.milestone {
        check_milestone(state, ctx, m).await?;
    }
    let mut tx = state.db.begin().await?;
    let new_state = p.state.clone().filter(|s| s != &issue.state);
    let reason: Option<Option<String>> = match new_state.as_deref() {
        Some("closed") => Some(Some(p.state_reason.clone().unwrap_or_else(|| "completed".into()))),
        Some("open") => Some(Some("reopened".into())),
        _ => None,
    };
    let row: IssueRow = sqlx::query_as(
        "UPDATE issues SET title = COALESCE($2, title), body = CASE WHEN $3::bool THEN $4 ELSE body END,
            state = COALESCE($5, state),
            state_reason = CASE WHEN $6::bool THEN $7 ELSE state_reason END,
            closed_at = CASE WHEN $5 = 'closed' THEN now() WHEN $5 = 'open' THEN NULL ELSE closed_at END,
            closed_by_id = CASE WHEN $5 = 'closed' THEN $8 WHEN $5 = 'open' THEN NULL ELSE closed_by_id END,
            milestone_id = CASE WHEN $9::bool THEN $10 ELSE milestone_id END,
            updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(issue.id)
    .bind(&p.title)
    .bind(p.body.is_some())
    .bind(p.body.as_ref().filter(|b| !b.is_empty()))
    .bind(&new_state)
    .bind(reason.is_some())
    .bind(reason.flatten())
    .bind(actor.id)
    .bind(p.milestone.is_some())
    .bind(p.milestone.flatten())
    .fetch_one(&mut *tx)
    .await?;
    if let Some(ls) = &p.labels {
        sqlx::query("DELETE FROM issue_labels WHERE issue_id = $1").bind(issue.id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO issue_labels (issue_id, label_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(issue.id)
            .bind(ls)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(us) = &p.assignees {
        sqlx::query("DELETE FROM issue_assignees WHERE issue_id = $1").bind(issue.id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO issue_assignees (issue_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(issue.id)
            .bind(us)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(s) = new_state.as_deref() {
        let ev = if s == "closed" { "closed" } else { "reopened" };
        sqlx::query("INSERT INTO issue_events (issue_id, repo_id, actor_id, event) VALUES ($1, $2, $3, $4)")
            .bind(issue.id)
            .bind(ctx.repo.id)
            .bind(actor.id)
            .bind(ev)
            .execute(&mut *tx)
            .await?;
        let delta = if s == "closed" { -1 } else { 1 };
        sqlx::query("UPDATE repositories SET open_issues_count = GREATEST(open_issues_count + $2, 0) WHERE id = $1")
            .bind(ctx.repo.id)
            .bind(delta)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(row)
}

pub async fn add_labels(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, label_ids: &[i64]) -> ApiResult<()> {
    let _ = actor;
    ctx.require(Perm::Triage)?;
    check_labels(state, ctx, label_ids).await?;
    sqlx::query("INSERT INTO issue_labels (issue_id, label_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(issue.id)
        .bind(label_ids)
        .execute(&state.db)
        .await?;
    touch(state, issue.id).await
}

pub async fn remove_labels(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, label_ids: &[i64]) -> ApiResult<()> {
    let _ = actor;
    ctx.require(Perm::Triage)?;
    sqlx::query("DELETE FROM issue_labels WHERE issue_id = $1 AND label_id = ANY($2)")
        .bind(issue.id)
        .bind(label_ids)
        .execute(&state.db)
        .await?;
    touch(state, issue.id).await
}

pub async fn add_assignees(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, user_ids: &[i64]) -> ApiResult<()> {
    let _ = actor;
    ctx.require(Perm::Triage)?;
    sqlx::query("INSERT INTO issue_assignees (issue_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(issue.id)
        .bind(user_ids)
        .execute(&state.db)
        .await?;
    touch(state, issue.id).await
}

pub async fn remove_assignees(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, user_ids: &[i64]) -> ApiResult<()> {
    let _ = actor;
    ctx.require(Perm::Triage)?;
    sqlx::query("DELETE FROM issue_assignees WHERE issue_id = $1 AND user_id = ANY($2)")
        .bind(issue.id)
        .bind(user_ids)
        .execute(&state.db)
        .await?;
    touch(state, issue.id).await
}

async fn touch(state: &AppState, issue_id: i64) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(issue_id).execute(&state.db).await?;
    Ok(())
}

pub async fn delete_issue(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow) -> ApiResult<()> {
    let _ = actor;
    ctx.require(Perm::Admin)?;
    if issue.is_pull {
        return Err(ApiError::unprocessable("Pull requests cannot be deleted."));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM issues WHERE id = $1").bind(issue.id).execute(&mut *tx).await?;
    if issue.state == "open" {
        sqlx::query("UPDATE repositories SET open_issues_count = GREATEST(open_issues_count - 1, 0) WHERE id = $1")
            .bind(ctx.repo.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Comments

pub async fn create_comment(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, body: &str) -> ApiResult<CommentRow> {
    if issue.locked && ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Unable to create comment because issue is locked."));
    }
    if body.trim().is_empty() {
        return Err(ApiError::validation("IssueComment", "body", "missing_field"));
    }
    let mut tx = state.db.begin().await?;
    let row: CommentRow = sqlx::query_as(
        "INSERT INTO issue_comments (issue_id, repo_id, user_id, body) VALUES ($1, $2, $3, $4)
         RETURNING id, issue_id, repo_id, user_id, body, created_at, updated_at",
    )
    .bind(issue.id)
    .bind(ctx.repo.id)
    .bind(actor.id)
    .bind(body)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE issues SET comments_count = comments_count + 1, updated_at = now() WHERE id = $1")
        .bind(issue.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn comment_by_id(state: &AppState, id: i64) -> ApiResult<CommentRow> {
    sqlx::query_as("SELECT id, issue_id, repo_id, user_id, body, created_at, updated_at FROM issue_comments WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn update_comment(state: &AppState, ctx: &RepoCtx, actor: &User, comment: &CommentRow, body: &str) -> ApiResult<CommentRow> {
    if comment.user_id != Some(actor.id) && ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Resource not accessible by personal access token"));
    }
    Ok(sqlx::query_as(
        "UPDATE issue_comments SET body = $2, updated_at = now() WHERE id = $1
         RETURNING id, issue_id, repo_id, user_id, body, created_at, updated_at",
    )
    .bind(comment.id)
    .bind(body)
    .fetch_one(&state.db)
    .await?)
}

pub async fn delete_comment(state: &AppState, ctx: &RepoCtx, actor: &User, comment: &CommentRow) -> ApiResult<()> {
    if comment.user_id != Some(actor.id) && ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Resource not accessible by personal access token"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM issue_comments WHERE id = $1").bind(comment.id).execute(&mut *tx).await?;
    sqlx::query("UPDATE issues SET comments_count = GREATEST(comments_count - 1, 0) WHERE id = $1")
        .bind(comment.issue_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Labels

pub async fn create_label(state: &AppState, ctx: &RepoCtx, actor: &User, name: &str, color: &str, description: Option<&str>) -> ApiResult<LabelRow> {
    let _ = actor;
    ctx.require(Perm::Write)?;
    let color = color.trim_start_matches('#').to_lowercase();
    if name.trim().is_empty() {
        return Err(ApiError::validation("Label", "name", "missing_field"));
    }
    if color.len() != 6 || !color.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::validation("Label", "color", "invalid"));
    }
    let row: Option<LabelRow> = sqlx::query_as(
        "INSERT INTO labels (repo_id, name, color, description) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING
         RETURNING id, repo_id, name, color, description, is_default, created_at",
    )
    .bind(ctx.repo.id)
    .bind(name)
    .bind(&color)
    .bind(description)
    .fetch_optional(&state.db)
    .await?;
    row.ok_or_else(|| ApiError::validation("Label", "name", "already_exists"))
}

pub async fn update_label(
    state: &AppState,
    ctx: &RepoCtx,
    actor: &User,
    id: i64,
    name: Option<&str>,
    color: Option<&str>,
    description: Option<&str>,
) -> ApiResult<LabelRow> {
    let _ = actor;
    ctx.require(Perm::Write)?;
    let color = color.map(|c| c.trim_start_matches('#').to_lowercase());
    sqlx::query_as(
        "UPDATE labels SET name = COALESCE($3, name), color = COALESCE($4, color), description = COALESCE($5, description)
          WHERE id = $1 AND repo_id = $2 RETURNING id, repo_id, name, color, description, is_default, created_at",
    )
    .bind(id)
    .bind(ctx.repo.id)
    .bind(name)
    .bind(color)
    .bind(description)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Label", "name", "already_exists"),
        e => e.into(),
    })?
    .ok_or_else(ApiError::not_found)
}

pub async fn delete_label(state: &AppState, ctx: &RepoCtx, actor: &User, id: i64) -> ApiResult<()> {
    let _ = actor;
    ctx.require(Perm::Write)?;
    sqlx::query("DELETE FROM labels WHERE id = $1 AND repo_id = $2").bind(id).bind(ctx.repo.id).execute(&state.db).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Pull requests

#[derive(Default, Debug, Clone)]
pub struct NewPull {
    pub title: String,
    pub body: Option<String>,
    /// `branch` or `owner:branch`.
    pub head: String,
    pub base: String,
    pub draft: bool,
    pub maintainer_can_modify: bool,
    pub head_repo_id: Option<i64>,
}

pub async fn pull_by_issue(state: &AppState, issue_id: i64) -> ApiResult<PullRow> {
    sqlx::query_as("SELECT * FROM pull_requests WHERE issue_id = $1")
        .bind(issue_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Resolves the head repository for `owner:branch` heads (the base repo or a fork in its network).
async fn resolve_head_repo(state: &AppState, ctx: &RepoCtx, actor: &User, head_owner: Option<&str>, head_repo_id: Option<i64>) -> ApiResult<RepoCtx> {
    if let Some(id) = head_repo_id {
        return db::load_repo_by_id(&state.db, Some(actor), id).await;
    }
    let Some(owner) = head_owner.filter(|o| !o.eq_ignore_ascii_case(&ctx.owner.login)) else {
        return Ok(ctx.clone());
    };
    let owner_user = db::user_by_login(&state.db, owner).await.map_err(|_| ApiError::validation("PullRequest", "head", "invalid"))?;
    // A fork of the base (or of its parent) owned by `owner`.
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM repositories WHERE owner_id = $1 AND (fork_parent_id = $2 OR id = $3 OR lower(name) = lower($4))
          ORDER BY (fork_parent_id = $2) DESC NULLS LAST LIMIT 1",
    )
    .bind(owner_user.id)
    .bind(ctx.repo.id)
    .bind(ctx.repo.fork_parent_id.unwrap_or(-1))
    .bind(&ctx.repo.name)
    .fetch_optional(&state.db)
    .await?;
    let id = id.ok_or_else(|| ApiError::validation("PullRequest", "head", "invalid"))?;
    db::load_repo_by_id(&state.db, Some(actor), id).await
}

pub async fn create_pull(state: &AppState, ctx: &RepoCtx, actor: &User, input: NewPull) -> ApiResult<IssueRow> {
    if input.title.trim().is_empty() {
        return Err(ApiError::validation("PullRequest", "title", "missing_field"));
    }
    let (head_owner, head_branch) = match input.head.split_once(':') {
        Some((o, b)) => (Some(o.to_string()), b.to_string()),
        None => (None, input.head.clone()),
    };
    let head_ctx = resolve_head_repo(state, ctx, actor, head_owner.as_deref(), input.head_repo_id).await?;
    let (_, base_refs) = state.git.refs(ctx.repo.id).await?;
    let base_sha = base_refs
        .get(&format!("refs/heads/{}", input.base))
        .cloned()
        .ok_or_else(|| ApiError::validation("PullRequest", "base", "invalid"))?;
    let (_, head_refs) = state.git.refs(head_ctx.repo.id).await?;
    let head_sha = head_refs
        .get(&format!("refs/heads/{head_branch}"))
        .cloned()
        .ok_or_else(|| ApiError::validation("PullRequest", "head", "invalid"))?;
    let same_repo = head_ctx.repo.id == ctx.repo.id;
    if same_repo && head_branch == input.base {
        return Err(ApiError::unprocessable(format!("No commits between {} and {}", input.base, head_branch)));
    }
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT i.number FROM pull_requests p JOIN issues i ON i.id = p.issue_id
          WHERE p.repo_id = $1 AND p.head_repo_id = $2 AND p.head_ref = $3 AND p.base_ref = $4 AND i.state = 'open'",
    )
    .bind(ctx.repo.id)
    .bind(head_ctx.repo.id)
    .bind(&head_branch)
    .bind(&input.base)
    .fetch_optional(&state.db)
    .await?;
    if existing.is_some() {
        return Err(ApiError::unprocessable(format!(
            "A pull request already exists for {}:{}.",
            head_ctx.owner.login, head_branch
        )));
    }
    let head_path = if same_repo { None } else { Some(state.git.sync(head_ctx.repo.id).await?) };

    let mut tx = state.db.begin().await?;
    let number = db::next_issue_number(&mut *tx, ctx.repo.id).await?;
    // Store refs/pull/N/head in the base repository (needed for cross-repo PRs and `gh pr checkout`).
    let git = state.git.git.clone();
    let pull_ref = format!("refs/pull/{number}/head");
    let (hs, bs) = (head_sha.clone(), base_sha.clone());
    let ((merge_base, count), updates) = state
        .git
        .write(ctx.repo.id, |path| async move {
            if let Some(hp) = head_path {
                git.run(&path, &["fetch", "--no-tags", "-q", hp.to_str().unwrap(), &format!("+{hs}:{pull_ref}")]).await?;
            } else {
                git.run(&path, &["update-ref", &pull_ref, &hs]).await?;
            }
            let (ok, out, _) = git.try_run(&path, &["merge-base", &bs, &hs]).await?;
            let mb = ok.then(|| String::from_utf8_lossy(&out).trim().to_string());
            let count = git.run_str(&path, &["rev-list", "--count", &format!("{bs}..{hs}")]).await?.parse::<i64>().unwrap_or(0);
            Ok((mb, count))
        })
        .await?;
    if count == 0 {
        // Roll back the number allocation with the transaction; leave the ref (harmless).
        return Err(ApiError::unprocessable(format!("No commits between {} and {}", input.base, input.head)));
    }
    let row: IssueRow = sqlx::query_as(
        "INSERT INTO issues (repo_id, number, is_pull, title, body, user_id) VALUES ($1, $2, TRUE, $3, $4, $5) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(number)
    .bind(&input.title)
    .bind(input.body.as_ref().filter(|b| !b.is_empty()))
    .bind(actor.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO pull_requests (issue_id, repo_id, head_repo_id, head_ref, head_sha, base_ref, base_sha, merge_base_sha,
            draft, maintainer_can_modify, commits_count)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(row.id)
    .bind(ctx.repo.id)
    .bind(head_ctx.repo.id)
    .bind(&head_branch)
    .bind(&head_sha)
    .bind(&input.base)
    .bind(&base_sha)
    .bind(&merge_base)
    .bind(input.draft)
    .bind(input.maintainer_can_modify)
    .bind(count)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE repositories SET open_issues_count = open_issues_count + 1 WHERE id = $1")
        .bind(ctx.repo.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let _ = crate::hooks::after_refs_changed(state, ctx, Some(actor), &updates.changed()).await;
    Ok(row)
}

#[derive(Default, Debug, Clone)]
pub struct PullPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub base: Option<String>,
    /// "open" | "closed"
    pub state: Option<String>,
    pub draft: Option<bool>,
    pub maintainer_can_modify: Option<bool>,
}

pub async fn update_pull(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, p: PullPatch) -> ApiResult<IssueRow> {
    let pr = pull_by_issue(state, issue.id).await?;
    let author = issue.user_id == Some(actor.id);
    if !(author || ctx.perm >= Perm::Write) {
        return Err(ApiError::forbidden(format!("{} does not have the correct permissions to execute `UpdatePullRequest`", actor.login)));
    }
    if pr.merged && (p.state.is_some() || p.base.is_some()) {
        return Err(ApiError::unprocessable("Pull request is already merged"));
    }
    if let Some(b) = &p.base {
        let (_, refs) = state.git.refs(ctx.repo.id).await?;
        if !refs.contains_key(&format!("refs/heads/{b}")) {
            return Err(ApiError::validation("PullRequest", "base", "invalid"));
        }
    }
    if p.state.as_deref() == Some("open") && issue.state == "closed" {
        // Reopening requires the head branch to still exist.
        if let Some(h) = pr.head_repo_id {
            let (_, refs) = state.git.refs(h).await?;
            if !refs.contains_key(&format!("refs/heads/{}", pr.head_ref)) {
                return Err(ApiError::unprocessable("Could not open the pull request: the head branch was deleted."));
            }
        }
    }
    let row = update_issue(
        state,
        &RepoCtx { perm: ctx.perm.max(if author { Perm::Triage } else { Perm::None }), ..ctx.clone() },
        actor,
        issue,
        IssuePatch { title: p.title.clone(), body: p.body.clone(), state: p.state.clone(), ..Default::default() },
    )
    .await?;
    sqlx::query(
        "UPDATE pull_requests SET base_ref = COALESCE($2, base_ref), draft = COALESCE($3, draft),
            maintainer_can_modify = COALESCE($4, maintainer_can_modify) WHERE issue_id = $1",
    )
    .bind(issue.id)
    .bind(&p.base)
    .bind(p.draft)
    .bind(p.maintainer_can_modify)
    .execute(&state.db)
    .await?;
    // GitHub stores no state_reason for pull requests.
    sqlx::query("UPDATE issues SET state_reason = NULL WHERE id = $1").bind(issue.id).execute(&state.db).await?;
    Ok(row)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

#[derive(Debug, Clone)]
pub struct MergeOpts {
    pub method: MergeMethod,
    pub commit_title: Option<String>,
    pub commit_message: Option<String>,
    pub expected_head_sha: Option<String>,
    pub author_email: Option<String>,
}

/// Merges a pull request in the bare repository (merge-tree + commit-tree), moves the base
/// branch, and records the merge. Returns the new base tip.
pub async fn merge_pull(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, opts: MergeOpts) -> ApiResult<String> {
    ctx.require(Perm::Write)?;
    let pr = pull_by_issue(state, issue.id).await?;
    if pr.merged {
        return Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Pull Request is already merged"));
    }
    if issue.state != "open" {
        return Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Pull Request is not open"));
    }
    if pr.draft {
        return Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Pull Request is still a draft"));
    }
    let allowed = match opts.method {
        MergeMethod::Merge => ctx.repo.allow_merge_commit,
        MergeMethod::Squash => ctx.repo.allow_squash_merge,
        MergeMethod::Rebase => ctx.repo.allow_rebase_merge,
    };
    if !allowed {
        return Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Merge method is not allowed on this repository"));
    }
    let head_path = match pr.head_repo_id {
        Some(h) if h != ctx.repo.id => Some(state.git.sync(h).await?),
        _ => None,
    };
    let head_login = match pr.head_repo_id {
        Some(h) if h != ctx.repo.id => db::load_repo_by_id(&state.db, Some(actor), h).await.map(|c| c.owner.login).unwrap_or_default(),
        _ => ctx.owner.login.clone(),
    };
    let committer = identity(actor);
    let mut author = committer.clone();
    if let Some(e) = &opts.author_email {
        author.email = e.clone();
    }
    let git = state.git.git.clone();
    let base_ref = format!("refs/heads/{}", pr.base_ref);
    let head_ref = format!("refs/heads/{}", pr.head_ref);
    let pull_ref = format!("refs/pull/{}/head", issue.number);
    let number = issue.number;
    let title = issue.title.clone();
    let opts2 = opts.clone();
    let result = state
        .git
        .write(ctx.repo.id, |path| async move {
            let base_sha = plumbing::rev_parse(&git, &path, &base_ref)
                .await
                .ok_or_else(|| anyhow::anyhow!("base branch is missing"))?;
            let head_sha = if let Some(hp) = head_path {
                git.run(&path, &["fetch", "--no-tags", "-q", hp.to_str().unwrap(), &format!("+{head_ref}:{pull_ref}")]).await?;
                plumbing::rev_parse(&git, &path, &pull_ref).await
            } else {
                match plumbing::rev_parse(&git, &path, &head_ref).await {
                    Some(s) => Some(s),
                    None => plumbing::rev_parse(&git, &path, &pull_ref).await,
                }
            }
            .ok_or_else(|| anyhow::anyhow!("head branch is missing"))?;
            if let Some(exp) = &opts2.expected_head_sha {
                if !exp.is_empty() && exp != &head_sha {
                    return Ok(Err(MergeFail::HeadChanged));
                }
            }
            git.run(&path, &["update-ref", &pull_ref, &head_sha]).await?;
            let new_sha = match opts2.method {
                MergeMethod::Merge | MergeMethod::Squash => {
                    let (ok, out, _) = git.try_run(&path, &["merge-tree", "--write-tree", "--no-messages", &base_sha, &head_sha]).await?;
                    if !ok {
                        return Ok(Err(MergeFail::Conflict));
                    }
                    let tree = String::from_utf8_lossy(&out).lines().next().unwrap_or("").trim().to_string();
                    if opts2.method == MergeMethod::Merge {
                        let headline = opts2
                            .commit_title
                            .clone()
                            .filter(|t| !t.is_empty())
                            .unwrap_or_else(|| format!("Merge pull request #{number} from {head_login}/{}", head_ref.trim_start_matches("refs/heads/")));
                        let body = opts2.commit_message.clone().unwrap_or_else(|| title.clone());
                        let msg = if body.is_empty() { format!("{headline}\n") } else { format!("{headline}\n\n{body}\n") };
                        plumbing::commit_tree(&git, &path, &tree, vec![&base_sha, &head_sha], &msg, &author, &committer).await?
                    } else {
                        let headline = opts2.commit_title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| format!("{title} (#{number})"));
                        let body = match opts2.commit_message.clone() {
                            Some(b) => b,
                            None => {
                                let log = git
                                    .run_str(&path, &["log", "--reverse", "--format=* %s", &format!("{base_sha}..{head_sha}")])
                                    .await
                                    .unwrap_or_default();
                                log
                            }
                        };
                        let msg = if body.is_empty() { format!("{headline}\n") } else { format!("{headline}\n\n{body}\n") };
                        plumbing::commit_tree(&git, &path, &tree, vec![&base_sha], &msg, &author, &committer).await?
                    }
                }
                MergeMethod::Rebase => {
                    let list = git.run_str(&path, &["rev-list", "--reverse", "--no-merges", &format!("{base_sha}..{head_sha}")]).await?;
                    let mut cur = base_sha.clone();
                    for c in list.lines().filter(|l| !l.is_empty()) {
                        let parent = git.run_str(&path, &["rev-parse", &format!("{c}^")]).await?;
                        let (ok, out, _) = git
                            .try_run(&path, &["merge-tree", "--write-tree", "--no-messages", &format!("--merge-base={parent}"), &cur, c])
                            .await?;
                        if !ok {
                            return Ok(Err(MergeFail::Conflict));
                        }
                        let tree = String::from_utf8_lossy(&out).lines().next().unwrap_or("").trim().to_string();
                        let info = git.run(&path, &["log", "-1", "--format=%an%x00%ae%x00%at%x00%B", c]).await?;
                        let info = String::from_utf8_lossy(&info).to_string();
                        let f: Vec<&str> = info.splitn(4, '\0').collect();
                        let orig_author = Identity {
                            name: f.first().unwrap_or(&"").to_string(),
                            email: f.get(1).unwrap_or(&"").to_string(),
                            when: f.get(2).and_then(|t| t.parse().ok()),
                        };
                        let msg = f.get(3).map(|m| m.trim_end().to_string() + "\n").unwrap_or_default();
                        cur = plumbing::commit_tree(&git, &path, &tree, vec![&cur], &msg, &orig_author, &committer).await?;
                    }
                    cur
                }
            };
            plumbing::update_ref(&git, &path, &base_ref, &new_sha, Some(&base_sha)).await?;
            Ok(Ok((new_sha, base_sha, head_sha)))
        })
        .await?;
    let ((new_sha, base_sha, head_sha), updates) = match result {
        (Ok(v), u) => (v, u),
        (Err(MergeFail::Conflict), _) => return Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Pull Request is not mergeable")),
        (Err(MergeFail::HeadChanged), _) => {
            return Err(ApiError::conflict("Head branch was modified. Review and try the merge again."));
        }
    };
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE pull_requests SET merged = TRUE, merged_at = now(), merged_by_id = $2, merge_commit_sha = $3,
            head_sha = $4, base_sha = $5 WHERE issue_id = $1",
    )
    .bind(issue.id)
    .bind(actor.id)
    .bind(&new_sha)
    .bind(&head_sha)
    .bind(&base_sha)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE issues SET state = 'closed', closed_at = now(), closed_by_id = $2, updated_at = now() WHERE id = $1")
        .bind(issue.id)
        .bind(actor.id)
        .execute(&mut *tx)
        .await?;
    for ev in ["merged", "closed"] {
        sqlx::query("INSERT INTO issue_events (issue_id, repo_id, actor_id, event, commit_id) VALUES ($1, $2, $3, $4, $5)")
            .bind(issue.id)
            .bind(ctx.repo.id)
            .bind(actor.id)
            .bind(ev)
            .bind(&new_sha)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE repositories SET open_issues_count = GREATEST(open_issues_count - 1, 0), pushed_at = now(), updated_at = now() WHERE id = $1")
        .bind(ctx.repo.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM review_requests WHERE issue_id = $1").bind(issue.id).execute(&mut *tx).await?;
    tx.commit().await?;
    if let Err(e) = crate::hooks::after_refs_changed(state, ctx, Some(actor), &updates.changed()).await {
        tracing::error!(error = %e, "after_refs_changed failed after merge");
    }
    Ok(new_sha)
}

enum MergeFail {
    Conflict,
    HeadChanged,
}

/// `event`: "APPROVE" | "REQUEST_CHANGES" | "COMMENT" | None (pending).
pub async fn create_review(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, event: Option<&str>, body: &str) -> ApiResult<ReviewRow> {
    let pr = pull_by_issue(state, issue.id).await?;
    let review_state = match event {
        Some("APPROVE") => "APPROVED",
        Some("REQUEST_CHANGES") => "CHANGES_REQUESTED",
        Some("COMMENT") => "COMMENTED",
        Some(other) => return Err(ApiError::validation("PullRequestReview", "event", other)),
        None => "PENDING",
    };
    let own = issue.user_id == Some(actor.id);
    if own && review_state == "APPROVED" {
        return Err(ApiError::unprocessable("Can not approve your own pull request"));
    }
    if own && review_state == "CHANGES_REQUESTED" {
        return Err(ApiError::unprocessable("Can not request changes on your own pull request"));
    }
    if matches!(review_state, "CHANGES_REQUESTED" | "COMMENTED") && body.trim().is_empty() {
        return Err(ApiError::unprocessable("Review body is required"));
    }
    let (_, head) = super::pull::pr_oids(state, issue, &pr).await.unwrap_or((pr.base_sha.clone(), pr.head_sha.clone()));
    let mut tx = state.db.begin().await?;
    let row: ReviewRow = sqlx::query_as(
        "INSERT INTO pull_reviews (issue_id, user_id, body, state, commit_id, submitted_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $4 = 'PENDING' THEN NULL ELSE now() END) RETURNING *",
    )
    .bind(issue.id)
    .bind(actor.id)
    .bind(body)
    .bind(review_state)
    .bind(&head)
    .fetch_one(&mut *tx)
    .await?;
    if review_state != "PENDING" {
        sqlx::query("DELETE FROM review_requests WHERE issue_id = $1 AND user_id = $2")
            .bind(issue.id)
            .bind(actor.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(issue.id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let _ = ctx;
    Ok(row)
}

pub async fn request_reviewers(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, user_ids: &[i64], union: bool) -> ApiResult<()> {
    if !(ctx.perm >= Perm::Triage || issue.user_id == Some(actor.id)) {
        return Err(ApiError::forbidden("Resource not accessible by personal access token"));
    }
    if let Some(author) = issue.user_id {
        if user_ids.contains(&author) {
            return Err(ApiError::unprocessable("Review cannot be requested from pull request author."));
        }
    }
    let mut tx = state.db.begin().await?;
    if !union {
        sqlx::query("DELETE FROM review_requests WHERE issue_id = $1").bind(issue.id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO review_requests (issue_id, user_id, requester_id) SELECT $1, unnest($2::bigint[]), $3 ON CONFLICT DO NOTHING")
        .bind(issue.id)
        .bind(user_ids)
        .bind(actor.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
