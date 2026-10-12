//! Mutation facade used by the GraphQL resolvers.
//!
//! These are deliberately small, self-contained implementations so gh works end to end on this
//! branch. The REST areas (issues, pulls, labels, ...) own the canonical service functions; when
//! they land, re-point each function here at them (signatures mirror the REST service-layer
//! convention: `(state, repo ctx, actor, input) -> ApiResult<Row>`).

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::json::issue::{IssueRow, LabelRow};
use crate::state::AppState;

use crate::json::issue::CommentRow;
use super::pull::ReviewRow;

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
// Issues, comments, labels: delegate to the issues area (`crate::api::issues`, `issue_comments`,
// `labels`). GraphQL passes node ids; the REST services take names/logins/numbers.

#[derive(Default, Debug, Clone)]
pub struct NewIssue {
    pub title: String,
    pub body: Option<String>,
    pub labels: Vec<i64>,
    pub assignees: Vec<i64>,
    pub milestone: Option<i64>,
}

async fn label_names(state: &AppState, ctx: &RepoCtx, ids: &[i64]) -> ApiResult<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM labels WHERE repo_id = $1 AND id = ANY($2)")
        .bind(ctx.repo.id)
        .bind(ids)
        .fetch_all(&state.db)
        .await?;
    if names.len() != ids.iter().collect::<std::collections::HashSet<_>>().len() {
        return Err(ApiError::validation("Label", "labelIds", "invalid"));
    }
    Ok(names)
}

async fn logins(state: &AppState, ids: &[i64]) -> ApiResult<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar("SELECT login FROM users WHERE id = ANY($1)").bind(ids).fetch_all(&state.db).await?)
}

async fn milestone_number(state: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<i64> {
    sqlx::query_scalar("SELECT number FROM milestones WHERE repo_id = $1 AND id = $2")
        .bind(ctx.repo.id)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::validation("Issue", "milestone", "invalid"))
}

pub async fn create_issue(state: &AppState, ctx: &RepoCtx, actor: &User, input: NewIssue) -> ApiResult<IssueRow> {
    let milestone = match input.milestone {
        Some(m) => Some(milestone_number(state, ctx, m).await?),
        None => None,
    };
    crate::api::issues::create_issue(
        state,
        ctx,
        actor,
        crate::api::issues::CreateIssue {
            title: input.title,
            body: input.body.filter(|b| !b.is_empty()),
            labels: label_names(state, ctx, &input.labels).await?,
            assignees: logins(state, &input.assignees).await?,
            milestone,
        },
    )
    .await
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
    let milestone = match p.milestone {
        Some(Some(m)) => Some(Some(milestone_number(state, ctx, m).await?)),
        Some(None) => Some(None),
        None => None,
    };
    let labels = match &p.labels {
        Some(ids) => Some(label_names(state, ctx, ids).await?),
        None => None,
    };
    let assignees = match &p.assignees {
        Some(ids) => Some(logins(state, ids).await?),
        None => None,
    };
    crate::api::issues::update_issue(
        state,
        ctx,
        actor,
        issue.number,
        crate::api::issues::UpdateIssue {
            title: p.title,
            body: p.body.map(|b| Some(b).filter(|b| !b.is_empty())),
            state: p.state,
            state_reason: p.state_reason.map(Some),
            labels,
            assignees,
            milestone,
        },
    )
    .await
}

pub async fn add_labels(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, label_ids: &[i64]) -> ApiResult<()> {
    let names = label_names(state, ctx, label_ids).await?;
    crate::api::issues::add_labels(state, ctx, actor, issue.number, names).await.map(|_| ())
}

pub async fn remove_labels(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, label_ids: &[i64]) -> ApiResult<()> {
    let names = label_names(state, ctx, label_ids).await?;
    let current: Vec<String> = crate::api::issues::issue_labels(&state.db, issue.id).await?.into_iter().map(|l| l.name).collect();
    for n in names.iter().filter(|n| current.iter().any(|c| c.eq_ignore_ascii_case(n))) {
        crate::api::issues::remove_label(state, ctx, actor, issue.number, n).await?;
    }
    Ok(())
}

pub async fn clear_labels(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow) -> ApiResult<()> {
    crate::api::issues::set_labels(state, ctx, actor, issue.number, Vec::new()).await.map(|_| ())
}

pub async fn add_assignees(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, user_ids: &[i64]) -> ApiResult<()> {
    let l = logins(state, user_ids).await?;
    crate::api::issues::add_assignees(state, ctx, actor, issue.number, l).await.map(|_| ())
}

pub async fn remove_assignees(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, user_ids: &[i64]) -> ApiResult<()> {
    let l = logins(state, user_ids).await?;
    crate::api::issues::remove_assignees(state, ctx, actor, issue.number, l).await.map(|_| ())
}

/// GitHub's `deleteIssue` (no REST equivalent): admins only; counters follow via DB triggers.
pub async fn delete_issue(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow) -> ApiResult<()> {
    let _ = actor;
    ctx.require(Perm::Admin)?;
    if issue.is_pull {
        return Err(ApiError::unprocessable("Pull requests cannot be deleted."));
    }
    sqlx::query("DELETE FROM issues WHERE id = $1").bind(issue.id).execute(&state.db).await?;
    Ok(())
}

pub async fn create_comment(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, body: &str) -> ApiResult<CommentRow> {
    crate::api::issue_comments::create_comment(state, ctx, actor, issue.number, body).await
}

pub async fn comment_by_id(state: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<CommentRow> {
    crate::api::issue_comments::comment_by_id(state, ctx, id).await
}

pub async fn update_comment(state: &AppState, ctx: &RepoCtx, actor: &User, comment: &CommentRow, body: &str) -> ApiResult<CommentRow> {
    crate::api::issue_comments::update_comment(state, ctx, actor, comment.id, body).await
}

pub async fn delete_comment(state: &AppState, ctx: &RepoCtx, actor: &User, comment: &CommentRow) -> ApiResult<()> {
    crate::api::issue_comments::delete_comment(state, ctx, actor, comment.id).await
}

pub async fn create_label(state: &AppState, ctx: &RepoCtx, actor: &User, name: &str, color: &str, description: Option<&str>) -> ApiResult<LabelRow> {
    let _ = actor;
    crate::api::labels::create_label(state, ctx, name, Some(color), description).await
}

async fn label_name(state: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<String> {
    sqlx::query_scalar("SELECT name FROM labels WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
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
    let cur = label_name(state, ctx, id).await?;
    crate::api::labels::update_label(state, ctx, &cur, name, color, description.map(Some)).await
}

pub async fn delete_label(state: &AppState, ctx: &RepoCtx, actor: &User, id: i64) -> ApiResult<()> {
    let _ = actor;
    let cur = label_name(state, ctx, id).await?;
    crate::api::labels::delete_label(state, ctx, &cur).await
}

// ---------------------------------------------------------------------------------------------
// Pull requests: delegate to the pulls service (`crate::pulls`).

pub use crate::pulls::{MergeInput, MergeMethod};

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

pub async fn create_pull(state: &AppState, ctx: &RepoCtx, actor: &User, input: NewPull) -> ApiResult<IssueRow> {
    let head_repo = match input.head_repo_id {
        Some(id) => Some(db::load_repo_by_id(&state.db, Some(actor), id).await?.full_name()),
        None => None,
    };
    let p = crate::pulls::create_pull(
        state,
        ctx,
        actor,
        crate::pulls::CreatePull {
            title: Some(input.title),
            body: input.body,
            head: input.head,
            head_repo,
            base: input.base,
            draft: Some(input.draft),
            maintainer_can_modify: Some(input.maintainer_can_modify),
            issue: None,
        },
    )
    .await?;
    Ok(p.issue)
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
    let n = issue.number;
    let mut out = None;
    if p.title.is_some() || p.body.is_some() || p.base.is_some() || p.maintainer_can_modify.is_some() {
        out = Some(
            crate::pulls::update_pull(
                state,
                ctx,
                actor,
                n,
                crate::pulls::UpdatePull { title: p.title, body: p.body, state: None, base: p.base, maintainer_can_modify: p.maintainer_can_modify },
            )
            .await?,
        );
    }
    match p.state.as_deref() {
        Some("closed") => out = Some(crate::pulls::close_pull(state, ctx, actor, n).await?),
        Some("open") => out = Some(crate::pulls::reopen_pull(state, ctx, actor, n).await?),
        _ => {}
    }
    match p.draft {
        Some(true) => out = Some(crate::pulls::convert_to_draft(state, ctx, actor, n).await?),
        Some(false) => out = Some(crate::pulls::mark_ready(state, ctx, actor, n).await?),
        None => {}
    }
    match out {
        Some(p) => Ok(p.issue),
        None => Ok(issue.clone()),
    }
}

pub async fn merge_pull(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, input: MergeInput) -> ApiResult<String> {
    Ok(crate::pulls::merge_pull(state, ctx, actor, issue.number, input).await?.sha)
}

/// `event`: "APPROVE" | "REQUEST_CHANGES" | "COMMENT" | None (pending).
pub async fn create_review(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, event: Option<&str>, body: &str, commit_id: Option<String>) -> ApiResult<ReviewRow> {
    crate::pulls::reviews::create_review(
        state,
        ctx,
        actor,
        issue.number,
        crate::pulls::reviews::CreateReview { commit_id, body: Some(body.to_string()), event: event.map(str::to_string), comments: Vec::new() },
    )
    .await
}

pub async fn request_reviewers(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow, user_ids: &[i64], union: bool) -> ApiResult<()> {
    if !union {
        let current: Vec<String> = sqlx::query_scalar(
            "SELECT u.login FROM pull_review_requests r JOIN users u ON u.id = r.user_id WHERE r.issue_id = $1",
        )
        .bind(issue.id)
        .fetch_all(&state.db)
        .await?;
        if !current.is_empty() {
            crate::pulls::remove_requested_reviewers(state, ctx, actor, issue.number, &current).await?;
        }
    }
    let logins: Vec<String> = sqlx::query_scalar("SELECT login FROM users WHERE id = ANY($1)").bind(user_ids).fetch_all(&state.db).await?;
    if !logins.is_empty() {
        crate::pulls::request_reviewers(state, ctx, actor, issue.number, &logins).await?;
    }
    Ok(())
}
