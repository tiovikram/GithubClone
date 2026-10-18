//! Pull requests service layer, shared by REST (`api::pulls`, `api::reviews`) and GraphQL.
//!
//! A pull request is an `issues` row (`is_pull = true`) plus a `pull_requests` row. Git state
//! lives in the *base* repository: `refs/pull/N/head` (the head commit, fetched from the fork for
//! cross-repo PRs) and `refs/pull/N/merge` (a test merge commit while the PR is mergeable). All
//! git computations (diffs, commits, merges) therefore run in the base repo.
pub mod git;
pub mod outdate;
pub mod revert;
pub mod reviews;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::git::plumbing;
use crate::json::issue::IssueRow;
use crate::state::AppState;
use self::git::{RawIdent, ZERO_SHA};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PullRow {
    pub issue_id: i64,
    pub repo_id: i64,
    pub head_repo_id: Option<i64>,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    pub base_sha: String,
    pub merge_base_sha: Option<String>,
    pub draft: bool,
    pub merged: bool,
    pub merged_at: Option<DateTime<Utc>>,
    pub merged_by_id: Option<i64>,
    pub merge_commit_sha: Option<String>,
    pub maintainer_can_modify: bool,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub commits_count: i64,
    pub mergeable: Option<bool>,
    pub rebaseable: Option<bool>,
    pub mergeable_key: Option<String>,
    pub test_merge_sha: Option<String>,
    pub head_ref_deleted: bool,
}

/// A pull request: its issue row and its pull_requests row.
#[derive(Debug, Clone)]
pub struct Pull {
    pub issue: IssueRow,
    pub pr: PullRow,
}

impl Pull {
    pub fn number(&self) -> i64 {
        self.issue.number
    }
    pub fn is_open(&self) -> bool {
        self.issue.state == "open"
    }
    pub fn is_cross_repo(&self) -> bool {
        self.pr.head_repo_id != Some(self.pr.repo_id)
    }
    pub fn head_ref_name(&self) -> String {
        format!("refs/pull/{}/head", self.issue.number)
    }
    /// Commit range of the PR's commits (`merge_base..head`).
    pub fn range(&self) -> String {
        match &self.pr.merge_base_sha {
            Some(mb) => format!("{mb}..{}", self.pr.head_sha),
            None => self.pr.head_sha.clone(),
        }
    }
    pub fn diff_base(&self) -> &str {
        self.pr.merge_base_sha.as_deref().unwrap_or(git::EMPTY_TREE)
    }
}

// ---------------------------------------------------------------------------------------------
// Errors carried through `state.git.write` closures (anyhow) back to ApiError.

#[derive(Debug)]
struct Halt(ApiError);
impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.message)
    }
}
impl std::error::Error for Halt {}

fn halt(e: ApiError) -> anyhow::Error {
    anyhow::Error::new(Halt(e))
}

fn unhalt(e: anyhow::Error) -> ApiError {
    match e.downcast::<Halt>() {
        Ok(h) => h.0,
        Err(e) => e.into(),
    }
}

/// 422 `Validation Failed` with a single custom error message (GitHub's PR errors).
pub fn custom_error(resource: &str, message: impl Into<String>) -> ApiError {
    let message = message.into();
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        message: "Validation Failed".into(),
        errors: Some(json!([{ "resource": resource, "code": "custom", "message": message }])),
    }
}

fn not_mergeable(msg: &str) -> ApiError {
    ApiError::new(StatusCode::METHOD_NOT_ALLOWED, msg)
}

// ---------------------------------------------------------------------------------------------
// Loading

pub async fn pull_by_number(db: &PgPool, repo_id: i64, number: i64) -> ApiResult<Pull> {
    let issue: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE repo_id = $1 AND number = $2 AND is_pull")
        .bind(repo_id)
        .bind(number)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let pr: PullRow = sqlx::query_as("SELECT * FROM pull_requests WHERE issue_id = $1")
        .bind(issue.id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Pull { issue, pr })
}

pub async fn pull_by_issue_id(db: &PgPool, issue_id: i64) -> ApiResult<Pull> {
    let issue: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(issue_id).fetch_one(db).await?;
    let pr: PullRow = sqlx::query_as("SELECT * FROM pull_requests WHERE issue_id = $1").bind(issue_id).fetch_one(db).await?;
    Ok(Pull { issue, pr })
}

/// Attaches pull_requests rows to issue rows (order preserved; rows without a PR are dropped).
pub async fn pulls_for_issues(db: &PgPool, rows: Vec<IssueRow>) -> ApiResult<Vec<Pull>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let prs: Vec<PullRow> = sqlx::query_as("SELECT * FROM pull_requests WHERE issue_id = ANY($1)").bind(&ids).fetch_all(db).await?;
    let mut prs: HashMap<i64, PullRow> = prs.into_iter().map(|p| (p.issue_id, p)).collect();
    Ok(rows.into_iter().filter_map(|issue| prs.remove(&issue.id).map(|pr| Pull { issue, pr })).collect())
}

/// Records an issue event (timeline). `data` carries event-specific fields, e.g.
/// `{"requested_reviewer": <user id>, "review_requester": <user id>}`, `{"from": .., "to": ..}`.
pub async fn record_event<'e, E: sqlx::PgExecutor<'e>>(
    db: E,
    pull: &Pull,
    actor_id: Option<i64>,
    event: &str,
    commit_id: Option<&str>,
    data: Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO issue_events (issue_id, repo_id, actor_id, event, commit_id, data) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(pull.issue.id)
        .bind(pull.issue.repo_id)
        .bind(actor_id)
        .bind(event)
        .bind(commit_id)
        .bind(data)
        .execute(db)
        .await?;
    Ok(())
}

/// Root of a repository's fork network.
async fn network_root(db: &PgPool, repo: &db::Repo) -> ApiResult<i64> {
    let mut id = repo.id;
    let mut parent = repo.fork_parent_id;
    for _ in 0..32 {
        let Some(p) = parent else { break };
        id = p;
        parent = sqlx::query_scalar::<_, Option<i64>>("SELECT fork_parent_id FROM repositories WHERE id = $1")
            .bind(p)
            .fetch_optional(db)
            .await?
            .flatten();
    }
    Ok(id)
}

/// The head repository context of a PR (None if it was deleted or is invisible to `viewer`).
pub async fn head_repo(state: &AppState, ctx: &RepoCtx, viewer: Option<&User>, pull: &Pull) -> ApiResult<Option<RepoCtx>> {
    match pull.pr.head_repo_id {
        Some(id) if id == ctx.repo.id => Ok(Some(ctx.clone())),
        Some(id) => Ok(db::load_repo_by_id(&state.db, viewer, id).await.ok()),
        None => Ok(None),
    }
}

/// Commit identity for a forge user (same as API commits).
pub fn identity(u: &User) -> RawIdent {
    RawIdent::from(&crate::api::repos::identity_for(u))
}

// ---------------------------------------------------------------------------------------------
// Create

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CreatePull {
    pub title: Option<String>,
    pub body: Option<String>,
    /// `branch` or `owner:branch`
    #[serde(default)]
    pub head: String,
    /// `owner/repo` of the head repository (optional)
    pub head_repo: Option<String>,
    #[serde(default)]
    pub base: String,
    pub draft: Option<bool>,
    pub maintainer_can_modify: Option<bool>,
    /// Convert this issue number into a pull request.
    pub issue: Option<i64>,
}

/// Opens a pull request (or converts an issue). Validates like GitHub (422s), creates
/// `refs/pull/N/head` (+ `refs/pull/N/merge`) in the base repo.
pub async fn create_pull(state: &AppState, ctx: &RepoCtx, actor: &User, input: CreatePull) -> ApiResult<Pull> {
    crate::api::orgs_invitations::require_not_blocked(&state.db, &ctx.owner, actor).await?;
    ctx.require(Perm::Read)?;
    if input.head.is_empty() {
        return Err(ApiError::validation("PullRequest", "head", "missing_field"));
    }
    if input.base.is_empty() {
        return Err(ApiError::validation("PullRequest", "base", "missing_field"));
    }
    let existing_issue = match input.issue {
        Some(n) => {
            let i = crate::json::issue::issue_by_number(&state.db, ctx.repo.id, n).await.map_err(|_| custom_error("PullRequest", "Issue not found"))?;
            if i.is_pull {
                return Err(custom_error("PullRequest", "Issue is already a pull request"));
            }
            if i.state != "open" {
                return Err(custom_error("PullRequest", "Issue is closed"));
            }
            if i.user_id != Some(actor.id) && ctx.perm < Perm::Write {
                return Err(ApiError::forbidden("Must be the issue author or have push access to convert an issue."));
            }
            Some(i)
        }
        None => None,
    };
    let title = input.title.clone().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if existing_issue.is_none() && title.is_none() {
        return Err(ApiError::validation("PullRequest", "title", "missing_field"));
    }

    // Resolve head repository and branch.
    let (head_owner, head_branch) = match input.head.split_once(':') {
        Some((o, b)) => (Some(o.to_string()), b.to_string()),
        None => (None, input.head.clone()),
    };
    let head_branch = head_branch.strip_prefix("refs/heads/").unwrap_or(&head_branch).to_string();
    let base_branch = input.base.strip_prefix("refs/heads/").unwrap_or(&input.base).to_string();
    let invalid_head = || ApiError::validation("PullRequest", "head", "invalid");
    let head_ctx: RepoCtx = if let Some(nwo) = input.head_repo.as_deref().filter(|s| !s.is_empty()) {
        let (o, r) = nwo.split_once('/').ok_or_else(invalid_head)?;
        db::load_repo(&state.db, Some(actor), o, r).await.map_err(|_| invalid_head())?
    } else {
        match head_owner.as_deref() {
            Some(o) if !o.eq_ignore_ascii_case(&ctx.owner.login) => {
                let owner = db::user_by_login(&state.db, o).await.map_err(|_| invalid_head())?;
                let root = network_root(&state.db, &ctx.repo).await?;
                let candidates: Vec<db::Repo> = sqlx::query_as("SELECT * FROM repositories WHERE owner_id = $1 ORDER BY id")
                    .bind(owner.id)
                    .fetch_all(&state.db)
                    .await?;
                let mut found = None;
                for c in candidates {
                    if network_root(&state.db, &c).await? == root {
                        found = Some(c);
                        break;
                    }
                }
                let repo = found.ok_or_else(invalid_head)?;
                db::load_repo_by_id(&state.db, Some(actor), repo.id).await.map_err(|_| invalid_head())?
            }
            _ => ctx.clone(),
        }
    };
    let cross = head_ctx.repo.id != ctx.repo.id;
    if cross && network_root(&state.db, &head_ctx.repo).await? != network_root(&state.db, &ctx.repo).await? {
        return Err(invalid_head());
    }
    let (_, head_refs) = state.git.refs(head_ctx.repo.id).await?;
    let head_sha = head_refs.get(&format!("refs/heads/{head_branch}")).cloned().ok_or_else(invalid_head)?;
    let (_, base_refs) = state.git.refs(ctx.repo.id).await?;
    let base_sha = base_refs
        .get(&format!("refs/heads/{base_branch}"))
        .cloned()
        .ok_or_else(|| ApiError::validation("PullRequest", "base", "invalid"))?;

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pull_requests p JOIN issues i ON i.id = p.issue_id
          WHERE p.repo_id = $1 AND p.head_repo_id = $2 AND p.head_ref = $3 AND p.base_ref = $4 AND i.state = 'open')",
    )
    .bind(ctx.repo.id)
    .bind(head_ctx.repo.id)
    .bind(&head_branch)
    .bind(&base_branch)
    .fetch_one(&state.db)
    .await?;
    if exists {
        return Err(custom_error("PullRequest", format!("A pull request already exists for {}:{}.", head_ctx.owner.login, head_branch)));
    }

    let fork_path = if cross { Some(state.git.sync(head_ctx.repo.id).await?) } else { None };
    let git = state.git.git.clone();
    let db = state.db.clone();
    let repo_id = ctx.repo.id;
    let head_repo_id = head_ctx.repo.id;
    let head_display = input.head.clone();
    let actor_id = actor.id;
    let draft = input.draft.unwrap_or(false);
    let mcm = input.maintainer_can_modify.unwrap_or(false) && cross;
    let body = input.body.clone();
    let issue_conv = existing_issue.clone();
    let (issue_id, _) = state
        .git
        .write(repo_id, |path| async move {
            if let Some(fp) = &fork_path {
                ensure_objects(&git, &path, fp, &head_sha).await?;
            }
            let a = git::analyze(&git, &path, &base_sha, &head_sha).await?;
            if a.commits == 0 {
                return Err(halt(custom_error("PullRequest", format!("No commits between {base_branch} and {head_display}"))));
            }
            let test_merge = match &a.merge_tree {
                Some(tree) => Some(test_merge_commit(&git, &path, tree, &base_sha, &head_sha).await?),
                None => None,
            };
            let mut tx = db.begin().await?;
            let issue_id: i64 = match &issue_conv {
                Some(i) => {
                    sqlx::query("UPDATE issues SET is_pull = TRUE, updated_at = now() WHERE id = $1").bind(i.id).execute(&mut *tx).await?;
                    i.id
                }
                None => {
                    let number = db::next_issue_number(&mut *tx, repo_id).await.map_err(|e| anyhow::anyhow!(e.message))?;
                    let id: i64 = sqlx::query_scalar(
                        "INSERT INTO issues (repo_id, number, is_pull, title, body, user_id) VALUES ($1, $2, TRUE, $3, $4, $5) RETURNING id",
                    )
                    .bind(repo_id)
                    .bind(number)
                    .bind(title.as_deref().unwrap_or(""))
                    .bind(body.as_ref().filter(|b| !b.is_empty()))
                    .bind(actor_id)
                    .fetch_one(&mut *tx)
                    .await?;
            // open_issues_count is maintained by a trigger (migration 0200).
                    id
                }
            };
            sqlx::query(
                "INSERT INTO pull_requests (issue_id, repo_id, head_repo_id, head_ref, head_sha, base_ref, base_sha, merge_base_sha,
                    draft, maintainer_can_modify, additions, deletions, changed_files, commits_count,
                    mergeable, rebaseable, mergeable_key, test_merge_sha)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)",
            )
            .bind(issue_id)
            .bind(repo_id)
            .bind(head_repo_id)
            .bind(&head_branch)
            .bind(&head_sha)
            .bind(&base_branch)
            .bind(&base_sha)
            .bind(&a.merge_base)
            .bind(draft)
            .bind(mcm)
            .bind(a.additions)
            .bind(a.deletions)
            .bind(a.changed_files)
            .bind(a.commits)
            .bind(a.mergeable)
            .bind(a.rebaseable)
            .bind(format!("{base_sha}:{head_sha}"))
            .bind(&test_merge)
            .execute(&mut *tx)
            .await?;
            let number: i64 = sqlx::query_scalar("SELECT number FROM issues WHERE id = $1").bind(issue_id).fetch_one(&mut *tx).await?;
            tx.commit().await?;
            git.run(&path, &["update-ref", &format!("refs/pull/{number}/head"), &head_sha]).await?;
            if let Some(m) = &test_merge {
                git.run(&path, &["update-ref", &format!("refs/pull/{number}/merge"), m]).await?;
            }
            Ok(issue_id)
        })
        .await
        .map_err(unhalt)?;
    let pull = pull_by_issue_id(&state.db, issue_id).await?;
    if existing_issue.is_some() {
        // GitHub records the conversion as a "converted_to_pull_request"-like event; we only
        // keep the issue in sync with the new PR.
        sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(issue_id).execute(&state.db).await?;
    }
    crate::api::events_emit::pull_event(state, ctx, actor, "opened", pull.clone(), json!({})).await;
    Ok(pull)
}

/// Makes sure `sha` exists in the repo at `path`, fetching it from `from` (a fork's cache).
async fn ensure_objects(git: &crate::git::GitCmd, path: &std::path::Path, from: &std::path::Path, sha: &str) -> anyhow::Result<()> {
    if git::has_commit(git, path, sha).await {
        return Ok(());
    }
    let from = from.to_str().unwrap_or_default();
    git.run(path, &["fetch", "-q", "--no-tags", "--no-write-fetch-head", from, sha]).await?;
    Ok(())
}

async fn test_merge_commit(git: &crate::git::GitCmd, path: &std::path::Path, tree: &str, base: &str, head: &str) -> anyhow::Result<String> {
    let who = RawIdent { name: "forge".into(), email: "noreply@forge.local".into(), date: String::new() };
    git::commit_tree(git, path, tree, &[base, head], &format!("Merge {head} into {base}"), &who, &who).await
}

// ---------------------------------------------------------------------------------------------
// Refresh: keep refs/pull/N/{head,merge}, shas, counts and mergeability in sync with branches.

#[derive(Debug, Clone)]
struct RefreshItem {
    pull: Pull,
    /// Previous base sha when the base branch moved in this event (for push-merge detection).
    base_old: Option<String>,
    /// Previous head sha when the head branch moved in this event (force-push detection).
    head_old: Option<String>,
}

#[derive(Debug)]
enum RefreshOutcome {
    Skip,
    Merged { head_sha: String, base_sha: String },
    Updated { head_sha: String, base_sha: String, analysis: git::Analysis, test_merge: Option<String>, force_pushed: Option<String> },
}

/// Re-syncs the given open PRs of one base repository (one git write for all of them) and
/// applies the results. `actor` is credited with push-merges and force-push events.
async fn refresh(state: &AppState, base_repo_id: i64, items: Vec<RefreshItem>, actor: Option<&User>) -> ApiResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    // Head commits of cross-repo PRs come from the forks' committed refs.
    let mut fork_paths: HashMap<i64, (PathBuf, BTreeMap<String, String>)> = HashMap::new();
    for it in &items {
        if let Some(hid) = it.pull.pr.head_repo_id.filter(|h| *h != base_repo_id) {
            if !fork_paths.contains_key(&hid) {
                let Ok((_, refs)) = state.git.refs(hid).await else { continue };
                let path = state.git.sync(hid).await?;
                fork_paths.insert(hid, (path, refs));
            }
        }
    }
    let git = state.git.git.clone();
    let work = items.clone();
    let (outcomes, _) = state
        .git
        .write(base_repo_id, |path| async move {
            let mut out = Vec::new();
            for it in &work {
                let p = &it.pull;
                let n = p.issue.number;
                let head_ref = format!("refs/heads/{}", p.pr.head_ref);
                let head_sha = match p.pr.head_repo_id {
                    Some(h) if h == base_repo_id => plumbing::rev_parse(&git, &path, &head_ref).await,
                    Some(h) => match fork_paths.get(&h) {
                        Some((fp, refs)) => match refs.get(&head_ref) {
                            Some(sha) => {
                                ensure_objects(&git, &path, fp, sha).await?;
                                Some(sha.clone())
                            }
                            None => None,
                        },
                        None => None,
                    },
                    None => None,
                };
                let base_sha = plumbing::rev_parse(&git, &path, &format!("refs/heads/{}", p.pr.base_ref)).await;
                let (Some(head_sha), Some(base_sha)) = (head_sha, base_sha) else {
                    out.push(RefreshOutcome::Skip);
                    continue;
                };
                git.run(&path, &["update-ref", &format!("refs/pull/{n}/head"), &head_sha]).await?;
                let merge_ref = format!("refs/pull/{n}/merge");
                if let Some(old_base) = &it.base_old {
                    let in_new = git::is_ancestor(&git, &path, &head_sha, &base_sha).await?;
                    let in_old = old_base != ZERO_SHA && git::has_commit(&git, &path, old_base).await
                        && git::is_ancestor(&git, &path, &head_sha, old_base).await?;
                    if in_new && !in_old {
                        let _ = git.try_run(&path, &["update-ref", "-d", &merge_ref]).await;
                        out.push(RefreshOutcome::Merged { head_sha, base_sha });
                        continue;
                    }
                }
                let analysis = git::analyze(&git, &path, &base_sha, &head_sha).await?;
                let test_merge = match &analysis.merge_tree {
                    Some(tree) => {
                        let m = test_merge_commit(&git, &path, tree, &base_sha, &head_sha).await?;
                        git.run(&path, &["update-ref", &merge_ref, &m]).await?;
                        Some(m)
                    }
                    None => {
                        let _ = git.try_run(&path, &["update-ref", "-d", &merge_ref]).await;
                        None
                    }
                };
                let force_pushed = match &it.head_old {
                    Some(old) if old != ZERO_SHA && old != &head_sha => {
                        let ff = git::has_commit(&git, &path, old).await && git::is_ancestor(&git, &path, old, &head_sha).await?;
                        (!ff).then(|| old.clone())
                    }
                    _ => None,
                };
                out.push(RefreshOutcome::Updated { head_sha, base_sha, analysis, test_merge, force_pushed });
            }
            Ok(out)
        })
        .await?;
    for (it, outcome) in items.iter().zip(outcomes) {
        match outcome {
            RefreshOutcome::Skip => {}
            RefreshOutcome::Merged { head_sha, base_sha } => {
                mark_merged(state, &it.pull, actor.map(|u| u.id), &head_sha, &base_sha, None).await?;
            }
            RefreshOutcome::Updated { head_sha, base_sha, analysis: a, test_merge, force_pushed } => {
                sqlx::query(
                    "UPDATE pull_requests SET head_sha = $2, base_sha = $3, merge_base_sha = $4, additions = $5, deletions = $6,
                        changed_files = $7, commits_count = $8, mergeable = $9, rebaseable = $10, mergeable_key = $11, test_merge_sha = $12
                      WHERE issue_id = $1",
                )
                .bind(it.pull.issue.id)
                .bind(&head_sha)
                .bind(&base_sha)
                .bind(&a.merge_base)
                .bind(a.additions)
                .bind(a.deletions)
                .bind(a.changed_files)
                .bind(a.commits)
                .bind(a.mergeable)
                .bind(a.rebaseable)
                .bind(format!("{base_sha}:{head_sha}"))
                .bind(&test_merge)
                .execute(&state.db)
                .await?;
                if head_sha != it.pull.pr.head_sha {
                    sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(it.pull.issue.id).execute(&state.db).await?;
                }
                if head_sha != it.pull.pr.head_sha || a.merge_base != it.pull.pr.merge_base_sha {
                    outdate::follow_head(state, base_repo_id, it.pull.issue.id, it.pull.pr.merge_base_sha.as_deref(), a.merge_base.as_deref(), &head_sha).await;
                }
                if let Some(before) = force_pushed {
                    record_event(&state.db, &it.pull, actor.map(|u| u.id), "head_ref_force_pushed", Some(&head_sha), json!({"before": before, "after": head_sha})).await?;
                }
            }
        }
    }
    Ok(())
}

/// Marks a PR merged (merge commit `merge_sha`), closes its issue and records events.
async fn mark_merged(state: &AppState, pull: &Pull, actor_id: Option<i64>, head_sha: &str, merge_sha: &str, head_deleted: Option<&str>) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    let updated = sqlx::query(
        "UPDATE pull_requests SET merged = TRUE, merged_at = now(), merged_by_id = $2, merge_commit_sha = $3, head_sha = $4,
            mergeable = NULL, rebaseable = NULL, mergeable_key = NULL, test_merge_sha = NULL
          WHERE issue_id = $1 AND NOT merged",
    )
    .bind(pull.issue.id)
    .bind(actor_id)
    .bind(merge_sha)
    .bind(head_sha)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(());
    }
    let old_state: String = sqlx::query_scalar("SELECT state FROM issues WHERE id = $1 FOR UPDATE").bind(pull.issue.id).fetch_one(&mut *tx).await?;
    let was_open = old_state == "open";
    sqlx::query("UPDATE issues SET state = 'closed', closed_at = COALESCE(closed_at, now()), closed_by_id = $2, updated_at = now() WHERE id = $1")
        .bind(pull.issue.id)
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
    if was_open {
            // open_issues_count is maintained by a trigger (migration 0200).
    }
    record_event(&mut *tx, pull, actor_id, "referenced", Some(merge_sha), json!({})).await?;
    record_event(&mut *tx, pull, actor_id, "merged", Some(merge_sha), json!({})).await?;
    if was_open {
        record_event(&mut *tx, pull, actor_id, "closed", Some(merge_sha), json!({})).await?;
    }
    if head_deleted.is_some() {
        sqlx::query("UPDATE pull_requests SET head_ref_deleted = TRUE WHERE issue_id = $1").bind(pull.issue.id).execute(&mut *tx).await?;
        record_event(&mut *tx, pull, actor_id, "head_ref_deleted", None, json!({})).await?;
    }
    sqlx::query("DELETE FROM pull_review_requests WHERE issue_id = $1").bind(pull.issue.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Recomputes git state for one open PR if its cached mergeability is missing or stale.
pub async fn ensure_fresh(state: &AppState, pull: Pull) -> ApiResult<Pull> {
    if !pull.is_open() || pull.pr.merged {
        return Ok(pull);
    }
    let key = format!("{}:{}", pull.pr.base_sha, pull.pr.head_sha);
    if pull.pr.mergeable_key.as_deref() == Some(key.as_str()) {
        return Ok(pull);
    }
    let id = pull.issue.id;
    if let Err(e) = refresh(state, pull.pr.repo_id, vec![RefreshItem { pull, base_old: None, head_old: None }], None).await {
        tracing::warn!(error = %e.message, "pull refresh failed");
    }
    pull_by_issue_id(&state.db, id).await
}

/// Hook: called after refs of `ctx` changed (push, API write, merge). Keeps PR heads, bases,
/// refs/pull/N/* and merge state in sync. Errors are logged, never propagated.
pub async fn on_refs_changed(state: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) -> anyhow::Result<()> {
    if let Err(e) = handle_refs_changed(state, ctx, actor, changed).await {
        tracing::error!(error = %e.message, repo = ctx.repo.id, "pull request ref sync failed");
    }
    Ok(())
}

async fn handle_refs_changed(state: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) -> ApiResult<()> {
    let branches: Vec<(String, String, String)> = changed
        .iter()
        .filter_map(|(r, o, n)| r.strip_prefix("refs/heads/").map(|b| (b.to_string(), o.clone(), n.clone())))
        .collect();
    if branches.is_empty() {
        return Ok(());
    }
    let actor_id = actor.map(|u| u.id);
    let deleted: Vec<String> = branches.iter().filter(|(_, _, n)| n == ZERO_SHA).map(|(b, _, _)| b.clone()).collect();
    let created: Vec<String> = branches.iter().filter(|(_, o, _)| o == ZERO_SHA).map(|(b, _, _)| b.clone()).collect();
    let moved: HashMap<String, (String, String)> =
        branches.iter().filter(|(_, _, n)| n != ZERO_SHA).map(|(b, o, n)| (b.clone(), (o.clone(), n.clone()))).collect();

    // Deleted head branches: mark, record head_ref_deleted, close open PRs (GitHub behaviour).
    if !deleted.is_empty() {
        let ids: Vec<i64> = sqlx::query_scalar(
            "UPDATE pull_requests SET head_ref_deleted = TRUE WHERE head_repo_id = $1 AND head_ref = ANY($2) AND NOT head_ref_deleted RETURNING issue_id",
        )
        .bind(ctx.repo.id)
        .bind(&deleted)
        .fetch_all(&state.db)
        .await?;
        for id in ids {
            let p = pull_by_issue_id(&state.db, id).await?;
            if p.is_open() {
                close_issue_row(state, &p, actor_id).await?;
            }
            record_event(&state.db, &p, actor_id, "head_ref_deleted", None, json!({})).await?;
        }
        // Deleted base branches close their open PRs.
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT p.issue_id FROM pull_requests p JOIN issues i ON i.id = p.issue_id
              WHERE p.repo_id = $1 AND p.base_ref = ANY($2) AND i.state = 'open'",
        )
        .bind(ctx.repo.id)
        .bind(&deleted)
        .fetch_all(&state.db)
        .await?;
        for id in ids {
            let p = pull_by_issue_id(&state.db, id).await?;
            close_issue_row(state, &p, actor_id).await?;
        }
    }
    if !created.is_empty() {
        let ids: Vec<i64> = sqlx::query_scalar(
            "UPDATE pull_requests SET head_ref_deleted = FALSE WHERE head_repo_id = $1 AND head_ref = ANY($2) AND head_ref_deleted RETURNING issue_id",
        )
        .bind(ctx.repo.id)
        .bind(&created)
        .fetch_all(&state.db)
        .await?;
        for id in ids {
            let p = pull_by_issue_id(&state.db, id).await?;
            record_event(&state.db, &p, actor_id, "head_ref_restored", None, json!({})).await?;
        }
    }
    if moved.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = moved.keys().cloned().collect();
    let rows: Vec<IssueRow> = sqlx::query_as(
        "SELECT i.* FROM issues i JOIN pull_requests p ON p.issue_id = i.id
          WHERE i.state = 'open' AND NOT p.merged
            AND ((p.head_repo_id = $1 AND p.head_ref = ANY($2)) OR (p.repo_id = $1 AND p.base_ref = ANY($2)))",
    )
    .bind(ctx.repo.id)
    .bind(&names)
    .fetch_all(&state.db)
    .await?;
    let pulls = pulls_for_issues(&state.db, rows).await?;
    let mut by_repo: BTreeMap<i64, Vec<RefreshItem>> = BTreeMap::new();
    for p in pulls {
        let head_old = (p.pr.head_repo_id == Some(ctx.repo.id)).then(|| moved.get(&p.pr.head_ref).map(|(o, _)| o.clone())).flatten();
        let base_old = (p.pr.repo_id == ctx.repo.id).then(|| moved.get(&p.pr.base_ref).map(|(o, _)| o.clone())).flatten();
        by_repo.entry(p.pr.repo_id).or_default().push(RefreshItem { pull: p, base_old, head_old });
    }
    for (repo_id, items) in by_repo {
        refresh(state, repo_id, items, actor).await?;
    }
    Ok(())
}

async fn close_issue_row(state: &AppState, pull: &Pull, actor_id: Option<i64>) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    let n = sqlx::query("UPDATE issues SET state = 'closed', closed_at = now(), closed_by_id = $2, updated_at = now() WHERE id = $1 AND state = 'open'")
        .bind(pull.issue.id)
        .bind(actor_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
            // open_issues_count is maintained by a trigger (migration 0200).
        record_event(&mut *tx, pull, actor_id, "closed", None, json!({})).await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Update / close / reopen / draft

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct UpdatePull {
    pub title: Option<String>,
    pub body: Option<String>,
    /// "open" | "closed"
    pub state: Option<String>,
    pub base: Option<String>,
    pub maintainer_can_modify: Option<bool>,
}

/// Closing/reopening: the author or anyone with Triage (GitHub's triage role).
fn can_edit(ctx: &RepoCtx, actor: &User, pull: &Pull) -> ApiResult<()> {
    if pull.issue.user_id == Some(actor.id) || ctx.perm >= Perm::Triage {
        Ok(())
    } else {
        ctx.require(Perm::Write)
    }
}

/// Title, body, base, draft state and maintainer edits: the author or Write (triage is not
/// enough on GitHub).
fn can_edit_content(ctx: &RepoCtx, actor: &User, pull: &Pull) -> ApiResult<()> {
    if pull.issue.user_id == Some(actor.id) {
        Ok(())
    } else {
        ctx.require(Perm::Write)
    }
}

pub async fn update_pull(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, input: UpdatePull) -> ApiResult<Pull> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    can_edit(ctx, actor, &pull)?;
    if let Some(s) = input.state.as_deref() {
        if s != "open" && s != "closed" {
            return Err(ApiError::validation("PullRequest", "state", "invalid"));
        }
    }
    let base_name = input.base.as_deref().map(|b| b.strip_prefix("refs/heads/").unwrap_or(b));
    let content_change = input.title.as_ref().is_some_and(|t| *t != pull.issue.title)
        || input.body.as_ref().is_some_and(|b| Some(b.as_str()).filter(|b| !b.is_empty()) != pull.issue.body.as_deref())
        || base_name.is_some_and(|b| b != pull.pr.base_ref)
        || input.maintainer_can_modify.is_some_and(|m| m != pull.pr.maintainer_can_modify);
    if content_change {
        can_edit_content(ctx, actor, &pull)?;
    }
    if let Some(t) = &input.title {
        if t.trim().is_empty() {
            return Err(ApiError::validation("PullRequest", "title", "missing_field"));
        }
        if t != &pull.issue.title {
            sqlx::query("UPDATE issues SET title = $2, updated_at = now() WHERE id = $1").bind(pull.issue.id).bind(t).execute(&state.db).await?;
            record_event(&state.db, &pull, Some(actor.id), "renamed", None, json!({"from": pull.issue.title, "to": t})).await?;
        }
    }
    if let Some(b) = &input.body {
        sqlx::query("UPDATE issues SET body = $2, updated_at = now() WHERE id = $1")
            .bind(pull.issue.id)
            .bind(Some(b).filter(|b| !b.is_empty()))
            .execute(&state.db)
            .await?;
    }
    if let Some(m) = input.maintainer_can_modify {
        sqlx::query("UPDATE pull_requests SET maintainer_can_modify = $2 WHERE issue_id = $1")
            .bind(pull.issue.id)
            .bind(m && pull.is_cross_repo())
            .execute(&state.db)
            .await?;
    }
    if let Some(base) = input.base.as_deref().map(|b| b.strip_prefix("refs/heads/").unwrap_or(b)).filter(|b| *b != pull.pr.base_ref) {
        if !pull.is_open() {
            return Err(custom_error("PullRequest", "Cannot change the base branch of a closed pull request."));
        }
        let (_, refs) = state.git.refs(ctx.repo.id).await?;
        let sha = refs.get(&format!("refs/heads/{base}")).ok_or_else(|| ApiError::validation("PullRequest", "base", "invalid"))?;
        if pull.pr.head_repo_id == Some(ctx.repo.id) && base == pull.pr.head_ref {
            return Err(ApiError::validation("PullRequest", "base", "invalid"));
        }
        sqlx::query("UPDATE pull_requests SET base_ref = $2, base_sha = $3, mergeable_key = NULL WHERE issue_id = $1")
            .bind(pull.issue.id)
            .bind(base)
            .bind(sha)
            .execute(&state.db)
            .await?;
        sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(pull.issue.id).execute(&state.db).await?;
        record_event(&state.db, &pull, Some(actor.id), "base_ref_changed", None, json!({"from": pull.pr.base_ref, "to": base})).await?;
        let p = pull_by_issue_id(&state.db, pull.issue.id).await?;
        refresh(state, ctx.repo.id, vec![RefreshItem { pull: p, base_old: None, head_old: None }], Some(actor)).await?;
    }
    match input.state.as_deref() {
        Some("closed") if pull.is_open() => {
            close_pull(state, ctx, actor, number).await?;
        }
        Some("open") if !pull.is_open() => {
            reopen_pull(state, ctx, actor, number).await?;
        }
        _ => {}
    }
    pull_by_issue_id(&state.db, pull.issue.id).await
}

pub async fn close_pull(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64) -> ApiResult<Pull> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    can_edit(ctx, actor, &pull)?;
    let was_open = pull.is_open();
    close_issue_row(state, &pull, Some(actor.id)).await?;
    let p = pull_by_issue_id(&state.db, pull.issue.id).await?;
    if was_open {
        crate::api::events_emit::pull_event(state, ctx, actor, "closed", p.clone(), json!({})).await;
    }
    Ok(p)
}

pub async fn reopen_pull(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64) -> ApiResult<Pull> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    can_edit(ctx, actor, &pull)?;
    if pull.pr.merged {
        return Err(custom_error("PullRequest", "state cannot be changed. The pull request has already been merged."));
    }
    if pull.is_open() {
        return Ok(pull);
    }
    if pull.pr.head_ref_deleted {
        return Err(custom_error("PullRequest", "state cannot be changed. The head branch has been deleted."));
    }
    let mut tx = state.db.begin().await?;
    let n = sqlx::query("UPDATE issues SET state = 'open', closed_at = NULL, closed_by_id = NULL, state_reason = NULL, updated_at = now() WHERE id = $1 AND state = 'closed'")
        .bind(pull.issue.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
            // open_issues_count is maintained by a trigger (migration 0200).
        record_event(&mut *tx, &pull, Some(actor.id), "reopened", None, json!({})).await?;
    }
    tx.commit().await?;
    let p = pull_by_issue_id(&state.db, pull.issue.id).await?;
    refresh(state, ctx.repo.id, vec![RefreshItem { pull: p, base_old: None, head_old: None }], Some(actor)).await?;
    let p = pull_by_issue_id(&state.db, pull.issue.id).await?;
    if n > 0 {
        crate::api::events_emit::pull_event(state, ctx, actor, "reopened", p.clone(), json!({})).await;
    }
    Ok(p)
}

async fn set_draft(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, draft: bool) -> ApiResult<Pull> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    can_edit_content(ctx, actor, &pull)?;
    if !pull.is_open() {
        return Err(custom_error("PullRequest", "Pull request is closed"));
    }
    if pull.pr.draft != draft {
        sqlx::query("UPDATE pull_requests SET draft = $2 WHERE issue_id = $1").bind(pull.issue.id).bind(draft).execute(&state.db).await?;
        sqlx::query("UPDATE issues SET updated_at = now() WHERE id = $1").bind(pull.issue.id).execute(&state.db).await?;
        let ev = if draft { "convert_to_draft" } else { "ready_for_review" };
        record_event(&state.db, &pull, Some(actor.id), ev, None, json!({})).await?;
    }
    pull_by_issue_id(&state.db, pull.issue.id).await
}

/// GraphQL `markPullRequestReadyForReview`.
pub async fn mark_ready(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64) -> ApiResult<Pull> {
    set_draft(state, ctx, actor, number, false).await
}

/// GraphQL `convertPullRequestToDraft`.
pub async fn convert_to_draft(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64) -> ApiResult<Pull> {
    set_draft(state, ctx, actor, number, true).await
}

// ---------------------------------------------------------------------------------------------
// Merge

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn parse(s: &str) -> Option<MergeMethod> {
        match s.to_ascii_lowercase().as_str() {
            "merge" => Some(MergeMethod::Merge),
            "squash" => Some(MergeMethod::Squash),
            "rebase" => Some(MergeMethod::Rebase),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MergeInput {
    pub method: MergeMethod,
    /// First line of the commit message (defaults like GitHub's).
    pub title: Option<String>,
    /// Commit message body.
    pub message: Option<String>,
    /// Expected head sha (409 when it doesn't match).
    pub sha: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MergeResult {
    pub sha: String,
    pub merged: bool,
    pub message: String,
}

/// Merges a pull request into its base branch (merge commit, squash or rebase), all inside the
/// base repo's git write, then marks it merged and closes it.
pub async fn merge_pull(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, input: MergeInput) -> ApiResult<MergeResult> {
    ctx.require(Perm::Write)?;
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    if pull.pr.merged || !pull.is_open() {
        return Err(not_mergeable("Pull Request is not mergeable"));
    }
    if pull.pr.draft {
        return Err(not_mergeable("Pull Request is still a draft"));
    }
    let r = &ctx.repo;
    match input.method {
        MergeMethod::Merge if !r.allow_merge_commit => return Err(not_mergeable("Merge commits are not allowed on this repository.")),
        MergeMethod::Squash if !r.allow_squash_merge => return Err(not_mergeable("Squash merges are not allowed on this repository.")),
        MergeMethod::Rebase if !r.allow_rebase_merge => return Err(not_mergeable("Rebase merges are not allowed on this repository.")),
        _ => {}
    }
    if let Some(sha) = &input.sha {
        if sha != &pull.pr.head_sha {
            return Err(ApiError::conflict("Head branch was modified. Review and try the merge again."));
        }
    }
    crate::protection::check_pull_merge(state, ctx, &pull, input.method).await?;
    let author = match pull.issue.user_id {
        Some(id) => db::user_by_id(&state.db, id).await.ok(),
        None => None,
    };
    let head_ctx = head_repo(state, ctx, Some(actor), &pull).await?;
    let head_label = match &head_ctx {
        Some(h) => format!("{}/{}", h.owner.login, pull.pr.head_ref),
        None => pull.pr.head_ref.clone(),
    };
    let cross = pull.is_cross_repo();
    let fork = match (&head_ctx, cross) {
        (Some(h), true) => {
            let (_, refs) = state.git.refs(h.repo.id).await?;
            let sha = refs.get(&format!("refs/heads/{}", pull.pr.head_ref)).cloned();
            Some((state.git.sync(h.repo.id).await?, sha))
        }
        _ => None,
    };
    let delete_head = !cross && r.delete_branch_on_merge && pull.pr.head_ref != r.default_branch;

    let git = state.git.git.clone();
    let committer = identity(actor);
    let pr_author = author.as_ref().map(identity).unwrap_or_else(|| committer.clone());
    let p = pull.clone();
    let (st, rctx, ractor) = (state.clone(), ctx.clone(), crate::rules::Actor::new(ctx, actor));
    let (res, updates) = state
        .git
        .write(ctx.repo.id, |path| async move {
            let n = p.issue.number;
            // Current head: the branch for same-repo PRs, the fork's branch otherwise.
            let head_sha = if let Some((fp, fork_sha)) = &fork {
                let sha = fork_sha.clone().unwrap_or_else(|| p.pr.head_sha.clone());
                ensure_objects(&git, &path, fp, &sha).await?;
                sha
            } else if p.is_cross_repo() {
                p.pr.head_sha.clone()
            } else {
                plumbing::rev_parse(&git, &path, &format!("refs/heads/{}", p.pr.head_ref))
                    .await
                    .unwrap_or_else(|| p.pr.head_sha.clone())
            };
            if !git::has_commit(&git, &path, &head_sha).await {
                return Err(halt(not_mergeable("Pull Request is not mergeable")));
            }
            if let Some(expected) = &input.sha {
                if expected != &head_sha {
                    return Err(halt(ApiError::conflict("Head branch was modified. Review and try the merge again.")));
                }
            }
            let base_ref = format!("refs/heads/{}", p.pr.base_ref);
            let base_sha = plumbing::rev_parse(&git, &path, &base_ref)
                .await
                .ok_or_else(|| halt(not_mergeable("Base branch was deleted")))?;
            let message = |default_title: String, default_body: String| {
                let title = input.title.clone().filter(|t| !t.is_empty()).unwrap_or(default_title);
                let body = input.message.clone().unwrap_or(default_body);
                if body.trim().is_empty() { format!("{title}\n") } else { format!("{title}\n\n{body}\n") }
            };
            let new_sha = match input.method {
                MergeMethod::Merge => {
                    let tree = git::merge_tree(&git, &path, &base_sha, &head_sha, None)
                        .await?
                        .ok_or_else(|| halt(not_mergeable("Pull Request is not mergeable")))?;
                    let msg = message(format!("Merge pull request #{n} from {head_label}"), p.issue.title.clone());
                    git::commit_tree(&git, &path, &tree, &[&base_sha, &head_sha], &msg, &committer, &committer).await?
                }
                MergeMethod::Squash => {
                    let tree = git::merge_tree(&git, &path, &base_sha, &head_sha, None)
                        .await?
                        .ok_or_else(|| halt(not_mergeable("Pull Request is not mergeable")))?;
                    let mb = git::merge_base(&git, &path, &base_sha, &head_sha).await?;
                    let range = match &mb {
                        Some(m) => format!("{m}..{head_sha}"),
                        None => head_sha.clone(),
                    };
                    let commits = git::list_commits(&git, &path, &range, 250).await?;
                    let body = commits.iter().map(|c| format!("* {}", c.message.trim_end())).collect::<Vec<_>>().join("\n\n");
                    let msg = message(format!("{} (#{n})", p.issue.title), body);
                    git::commit_tree(&git, &path, &tree, &[&base_sha], &msg, &pr_author, &committer).await?
                }
                MergeMethod::Rebase => {
                    let mb = git::merge_base(&git, &path, &base_sha, &head_sha)
                        .await?
                        .ok_or_else(|| halt(not_mergeable("Pull Request is not mergeable")))?;
                    git::rebase(&git, &path, &base_sha, &mb, &head_sha, &committer)
                        .await?
                        .ok_or_else(|| halt(not_mergeable("This branch can't be rebased")))?
                }
            };
            let upd = crate::rules::RefUpdate::new(&base_ref, Some(&base_sha), Some(&new_sha));
            crate::rules::enforce_api(&st, &rctx, &ractor, &path, &[upd], crate::rules::Via::Merge(Some(input.method)), Some(&p))
                .await
                .map_err(halt)?;
            plumbing::update_ref(&git, &path, &base_ref, &new_sha, Some(&base_sha)).await?;
            git.run(&path, &["update-ref", &format!("refs/pull/{n}/head"), &head_sha]).await?;
            let _ = git.try_run(&path, &["update-ref", "-d", &format!("refs/pull/{n}/merge")]).await;
            let mut deleted = None;
            if delete_head {
                let head_ref = format!("refs/heads/{}", p.pr.head_ref);
                if plumbing::rev_parse(&git, &path, &head_ref).await.as_deref() == Some(head_sha.as_str()) {
                    plumbing::delete_ref(&git, &path, &head_ref).await?;
                    deleted = Some(head_ref);
                }
            }
            Ok((new_sha, head_sha, deleted))
        })
        .await
        .map_err(unhalt)?;
    let (new_sha, head_sha, deleted) = res;
    mark_merged(state, &pull, Some(actor.id), &head_sha, &new_sha, deleted.as_deref()).await?;
    crate::hooks::after_refs_changed(state, ctx, Some(actor), &updates.changed()).await?;
    if let Ok(p) = pull_by_issue_id(&state.db, pull.issue.id).await {
        crate::api::events_emit::pull_event(state, ctx, actor, "closed", p, json!({})).await;
    }
    Ok(MergeResult { sha: new_sha, merged: true, message: "Pull Request successfully merged".into() })
}

/// Merges the base branch into the PR's head branch (PUT /pulls/{n}/update-branch).
pub async fn update_branch(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, expected_head_sha: Option<String>) -> ApiResult<()> {
    update_branch_with(state, ctx, actor, number, expected_head_sha, false).await
}

/// Brings the PR's head branch up to date with its base: a merge commit, or (`rebase`; GraphQL
/// `updatePullRequestBranch(updateMethod: REBASE)`) the PR's commits replayed onto the base.
pub async fn update_branch_with(
    state: &AppState,
    ctx: &RepoCtx,
    actor: &User,
    number: i64,
    expected_head_sha: Option<String>,
    rebase: bool,
) -> ApiResult<()> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    if !pull.is_open() {
        return Err(custom_error("PullRequest", "Pull request is closed"));
    }
    let head_ctx = head_repo(state, ctx, Some(actor), &pull).await?.ok_or_else(|| custom_error("PullRequest", "Head repository is missing"))?;
    if head_ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Resource not accessible by personal access token"));
    }
    let (_, head_refs) = state.git.refs(head_ctx.repo.id).await?;
    let head_ref = format!("refs/heads/{}", pull.pr.head_ref);
    let head_sha = head_refs.get(&head_ref).cloned().ok_or_else(|| custom_error("PullRequest", "Head branch was deleted"))?;
    if let Some(e) = expected_head_sha.as_deref().filter(|e| !e.is_empty()) {
        if e != head_sha {
            return Err(custom_error("PullRequest", "expected head sha didn’t match current head ref."));
        }
    }
    let (_, base_refs) = state.git.refs(ctx.repo.id).await?;
    let base_sha = base_refs
        .get(&format!("refs/heads/{}", pull.pr.base_ref))
        .cloned()
        .ok_or_else(|| custom_error("PullRequest", "Base branch was deleted"))?;
    let base_path = if head_ctx.repo.id != ctx.repo.id { Some(state.git.sync(ctx.repo.id).await?) } else { None };
    let git = state.git.git.clone();
    let who = identity(actor);
    let msg = if base_path.is_some() {
        format!("Merge branch '{}' of {} into {}\n", pull.pr.base_ref, ctx.full_name(), pull.pr.head_ref)
    } else {
        format!("Merge branch '{}' into {}\n", pull.pr.base_ref, pull.pr.head_ref)
    };
    let (st, rctx, ractor) = (state.clone(), head_ctx.clone(), crate::rules::Actor::new(&head_ctx, actor));
    let (_, updates) = state
        .git
        .write(head_ctx.repo.id, |path| async move {
            let enforce = |sha: String| {
                let (st, rctx, ractor, path, head_ref, head_sha) = (st.clone(), rctx.clone(), ractor.clone(), path.clone(), head_ref.clone(), head_sha.clone());
                async move {
                    let upd = crate::rules::RefUpdate::new(&head_ref, Some(&head_sha), Some(&sha));
                    crate::rules::enforce_api(&st, &rctx, &ractor, &path, &[upd], crate::rules::Via::Commit, None).await.map_err(halt)
                }
            };
            if let Some(bp) = &base_path {
                ensure_objects(&git, &path, bp, &base_sha).await?;
            }
            let cur = plumbing::rev_parse(&git, &path, &head_ref).await;
            if cur.as_deref() != Some(head_sha.as_str()) {
                return Err(halt(custom_error("PullRequest", "expected head sha didn’t match current head ref.")));
            }
            if git::is_ancestor(&git, &path, &base_sha, &head_sha).await? {
                return Err(halt(custom_error("PullRequest", "There are no new commits on the base branch.")));
            }
            if rebase {
                let mb = git::merge_base(&git, &path, &base_sha, &head_sha).await?.unwrap_or_else(|| base_sha.clone());
                let sha = git::rebase(&git, &path, &base_sha, &mb, &head_sha, &who)
                    .await?
                    .ok_or_else(|| halt(custom_error("PullRequest", "merge conflict between base and head")))?;
                enforce(sha.clone()).await?;
                plumbing::update_ref(&git, &path, &head_ref, &sha, Some(&head_sha)).await?;
                return Ok(());
            }
            let tree = git::merge_tree(&git, &path, &head_sha, &base_sha, None)
                .await?
                .ok_or_else(|| halt(custom_error("PullRequest", "merge conflict between base and head")))?;
            let sha = git::commit_tree(&git, &path, &tree, &[&head_sha, &base_sha], &msg, &who, &who).await?;
            enforce(sha.clone()).await?;
            plumbing::update_ref(&git, &path, &head_ref, &sha, Some(&head_sha)).await?;
            Ok(())
        })
        .await
        .map_err(unhalt)?;
    crate::hooks::after_refs_changed(state, &head_ctx, Some(actor), &updates.changed()).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Requested reviewers

/// Users who may be asked for a review: owner, collaborators, org members.
async fn is_collaborator(db: &PgPool, ctx: &RepoCtx, user: &User) -> ApiResult<bool> {
    if user.id == ctx.owner.id {
        return Ok(true);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM repo_collaborators WHERE repo_id = $1 AND user_id = $3)
             OR EXISTS(SELECT 1 FROM org_members WHERE org_id = $2 AND user_id = $3)",
    )
    .bind(ctx.repo.id)
    .bind(ctx.owner.id)
    .bind(user.id)
    .fetch_one(db)
    .await?)
}

pub async fn request_reviewers(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, logins: &[String]) -> ApiResult<Pull> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    can_edit(ctx, actor, &pull)?;
    let mut users = Vec::new();
    for l in logins {
        let not_collab = || {
            custom_error(
                "PullRequest",
                format!("Reviews may only be requested from collaborators. One or more of the users or teams you specified is not a collaborator of the {} repository.", ctx.full_name()),
            )
        };
        let u = db::user_by_login(&state.db, l).await.map_err(|_| not_collab())?;
        if Some(u.id) == pull.issue.user_id {
            return Err(custom_error("PullRequest", "Review cannot be requested from pull request author."));
        }
        if !is_collaborator(&state.db, ctx, &u).await? {
            return Err(not_collab());
        }
        users.push(u);
    }
    for u in users {
        let added = sqlx::query("INSERT INTO pull_review_requests (issue_id, user_id, requester_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(pull.issue.id)
            .bind(u.id)
            .bind(actor.id)
            .execute(&state.db)
            .await?
            .rows_affected();
        if added > 0 {
            record_event(&state.db, &pull, Some(actor.id), "review_requested", None, json!({"requested_reviewer": u.id, "review_requester": actor.id}))
                .await?;
            let reviewer = crate::json::user::simple(&state.api_url(), state.base_url(), &u);
            crate::api::events_emit::pull_event(state, ctx, actor, "review_requested", pull.clone(), json!({"requested_reviewer": reviewer})).await;
        }
    }
    pull_by_issue_id(&state.db, pull.issue.id).await
}

pub async fn remove_requested_reviewers(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, logins: &[String]) -> ApiResult<Pull> {
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    can_edit(ctx, actor, &pull)?;
    for l in logins {
        let Ok(u) = db::user_by_login(&state.db, l).await else { continue };
        let removed = sqlx::query("DELETE FROM pull_review_requests WHERE issue_id = $1 AND user_id = $2")
            .bind(pull.issue.id)
            .bind(u.id)
            .execute(&state.db)
            .await?
            .rows_affected();
        if removed > 0 {
            record_event(&state.db, &pull, Some(actor.id), "review_request_removed", None, json!({"requested_reviewer": u.id, "review_requester": actor.id}))
                .await?;
        }
    }
    pull_by_issue_id(&state.db, pull.issue.id).await
}

/// Requested reviewers per PR issue id, in request order.
pub async fn requested_reviewers(db: &PgPool, issue_ids: &[i64]) -> ApiResult<HashMap<i64, Vec<User>>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT issue_id, user_id FROM pull_review_requests WHERE issue_id = ANY($1) ORDER BY created_at, user_id")
        .bind(issue_ids)
        .fetch_all(db)
        .await?;
    let uids: Vec<i64> = rows.iter().map(|r| r.1).collect();
    let users = crate::json::issue::users_by_ids(db, &uids).await?;
    let mut out: HashMap<i64, Vec<User>> = HashMap::new();
    for (iid, uid) in rows {
        if let Some(u) = users.get(&uid) {
            out.entry(iid).or_default().push(u.clone());
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Status (what gh shows): mergeable state and review decision.

#[derive(Debug, Clone)]
pub struct PullStatus {
    /// None = not computed / closed.
    pub mergeable: Option<bool>,
    pub rebaseable: Option<bool>,
    /// GitHub `mergeable_state`: clean | dirty | unknown | draft | unstable
    pub mergeable_state: &'static str,
    /// GraphQL `mergeStateStatus`: CLEAN | DIRTY | UNKNOWN | DRAFT | UNSTABLE
    pub merge_state_status: &'static str,
    /// GraphQL `mergeable`: MERGEABLE | CONFLICTING | UNKNOWN
    pub mergeable_enum: &'static str,
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED, or None
    pub review_decision: Option<&'static str>,
}

/// Mergeability and review decision for a PR (uses the cached analysis; call `ensure_fresh`
/// first if you need it computed).
pub async fn pull_status(state: &AppState, pull: &Pull) -> ApiResult<PullStatus> {
    let open = pull.is_open() && !pull.pr.merged;
    let key_ok = pull.pr.mergeable_key.as_deref() == Some(format!("{}:{}", pull.pr.base_sha, pull.pr.head_sha).as_str());
    let mergeable = if open && key_ok { pull.pr.mergeable } else { None };
    let rebaseable = if open && key_ok { pull.pr.rebaseable } else { None };
    // Combined commit status of the head: latest per context.
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT ON (context) state FROM commit_statuses WHERE repo_id = $1 AND sha = $2 ORDER BY context, id DESC",
    )
    .bind(pull.pr.repo_id)
    .bind(&pull.pr.head_sha)
    .fetch_all(&state.db)
    .await?;
    let unstable = states.iter().any(|s| s != "success");
    let (blocked, review_required) = if open { crate::protection::merge_blockers(state, pull).await? } else { (false, false) };
    let (ms, mss) = match mergeable {
        None => ("unknown", "UNKNOWN"),
        Some(false) => ("dirty", "DIRTY"),
        Some(true) if pull.pr.draft => ("draft", "DRAFT"),
        Some(true) if blocked => ("blocked", "BLOCKED"),
        Some(true) if unstable => ("unstable", "UNSTABLE"),
        Some(true) => ("clean", "CLEAN"),
    };
    let me = match mergeable {
        None => "UNKNOWN",
        Some(true) => "MERGEABLE",
        Some(false) => "CONFLICTING",
    };
    let mut review_decision = review_decision(&state.db, pull).await?;
    if review_required && review_decision != Some("CHANGES_REQUESTED") {
        review_decision = Some("REVIEW_REQUIRED");
    }
    Ok(PullStatus { mergeable, rebaseable, mergeable_state: ms, merge_state_status: mss, mergeable_enum: me, review_decision })
}

/// Latest decisive review per reviewer: any CHANGES_REQUESTED wins, then APPROVED; outstanding
/// review requests without either give REVIEW_REQUIRED; otherwise None.
pub async fn review_decision(db: &PgPool, pull: &Pull) -> ApiResult<Option<&'static str>> {
    let latest: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT ON (user_id) state FROM pull_reviews
          WHERE issue_id = $1 AND state IN ('APPROVED', 'CHANGES_REQUESTED', 'DISMISSED') AND user_id IS NOT NULL
          ORDER BY user_id, id DESC",
    )
    .bind(pull.issue.id)
    .fetch_all(db)
    .await?;
    if latest.iter().any(|s| s == "CHANGES_REQUESTED") {
        return Ok(Some("CHANGES_REQUESTED"));
    }
    if latest.iter().any(|s| s == "APPROVED") {
        return Ok(Some("APPROVED"));
    }
    let requested: i64 = sqlx::query_scalar("SELECT count(*) FROM pull_review_requests WHERE issue_id = $1").bind(pull.issue.id).fetch_one(db).await?;
    if requested > 0 {
        return Ok(Some("REVIEW_REQUIRED"));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------------------------
// Read helpers (commits, files, diff) on the base repo.

/// Synced base repo path with the PR's head objects available (refs/pull/N/head).
pub async fn repo_for_reading(state: &AppState, pull: &Pull) -> ApiResult<PathBuf> {
    let path = state.git.sync(pull.pr.repo_id).await?;
    if !git::has_commit(&state.git.git, &path, &pull.pr.head_sha).await {
        // refs/pull/N/head missing (e.g. created before a failure): rebuild it.
        if pull.is_open() {
            let _ = refresh(state, pull.pr.repo_id, vec![RefreshItem { pull: pull.clone(), base_old: None, head_old: None }], None).await;
        }
        return Ok(state.git.sync(pull.pr.repo_id).await?);
    }
    Ok(path)
}

pub async fn commits(state: &AppState, pull: &Pull) -> ApiResult<Vec<git::CommitInfo>> {
    let path = repo_for_reading(state, pull).await?;
    Ok(git::list_commits(&state.git.git, &path, &pull.range(), 250).await.unwrap_or_default())
}

pub async fn files(state: &AppState, pull: &Pull) -> ApiResult<Vec<git::FileDiff>> {
    let path = repo_for_reading(state, pull).await?;
    Ok(git::diff(&state.git.git, &path, pull.diff_base(), &pull.pr.head_sha, None).await.unwrap_or_default())
}
