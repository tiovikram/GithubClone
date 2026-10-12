//! PullRequest and its git-derived fields (commits, files, mergeability), reviews, review requests.
use std::sync::Arc;

use async_graphql::{Context, Object, ID};
use chrono::Utc;
use tokio::sync::OnceCell;

use super::connection::ConnArgs;
use super::enums::*;
use super::issue::{self, IssueCommentConnection, IssueConnection, IssueQuery, ReactionGroup};
use super::loaders::{self, lerr, loader};
use super::repo::{body_html, html_escape, repo_refs, st, viewer, GCommit, GLabel, GMilestone, GRef, GRepo, LabelConnection, ProjectCardConnection, ProjectV2ItemConnection};
use super::repo::CommitInfo;
use super::user::{Actor, AssigneeConnection, GTeam, GUser, RepositoryOwner, RequestedReviewer, UserConnection};
use super::GResult;
use crate::db::{Perm, RepoCtx};
use crate::ids;
use crate::json::issue::IssueRow;
use crate::state::AppState;

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
    pub merged_at: Option<chrono::DateTime<Utc>>,
    pub merged_by_id: Option<i64>,
    pub merge_commit_sha: Option<String>,
    pub maintainer_can_modify: bool,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub commits_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReviewRow {
    pub id: i64,
    pub issue_id: i64,
    pub user_id: Option<i64>,
    pub body: String,
    pub state: String,
    pub commit_id: Option<String>,
    pub submitted_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

/// Git facts about a PR, computed on demand from the base repository.
#[derive(Debug, Clone)]
pub struct PrGit {
    pub base_oid: String,
    pub head_oid: String,
    pub merge_base: Option<String>,
    pub commits: Vec<CommitInfo>,
    pub files: Vec<FileStat>,
    pub mergeable: MergeableState,
}

#[derive(Debug, Clone)]
pub struct FileStat {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
    pub change_type: PatchStatus,
}

#[derive(Clone)]
pub struct GPull {
    pub row: Arc<IssueRow>,
    pub pr: Arc<PullRow>,
    pub repo: Arc<RepoCtx>,
    git: Arc<OnceCell<Option<Arc<PrGit>>>>,
}

crate::gql_connection!(PullRequestConnection, PullRequestEdge, "PullRequestConnection", "PullRequestEdge", GPull);
crate::gql_connection!(PullRequestCommitConnection, PullRequestCommitEdge, "PullRequestCommitConnection", "PullRequestCommitEdge", PullRequestCommit);
crate::gql_connection!(
    PullRequestChangedFileConnection,
    PullRequestChangedFileEdge,
    "PullRequestChangedFileConnection",
    "PullRequestChangedFileEdge",
    PullRequestChangedFile
);
crate::gql_connection!(PullRequestReviewConnection, PullRequestReviewEdge, "PullRequestReviewConnection", "PullRequestReviewEdge", GReview);
crate::gql_connection!(ReviewRequestConnection, ReviewRequestEdge, "ReviewRequestConnection", "ReviewRequestEdge", ReviewRequest);

/// Resolves the current head/base oids of a PR (live branch tips while open).
pub async fn pr_oids(s: &AppState, row: &IssueRow, pr: &PullRow) -> anyhow::Result<(String, String)> {
    let (_, base_refs) = s.git.refs(pr.repo_id).await?;
    let open = row.state == "open" && !pr.merged;
    let mut head = pr.head_sha.clone();
    let mut base = pr.base_sha.clone();
    if open {
        if let Some(b) = base_refs.get(&format!("refs/heads/{}", pr.base_ref)) {
            base = b.clone();
        }
        match pr.head_repo_id {
            Some(h) if h == pr.repo_id => {
                if let Some(x) = base_refs.get(&format!("refs/heads/{}", pr.head_ref)) {
                    head = x.clone();
                }
            }
            _ => {
                // Cross-repo: objects live in the base repo under refs/pull/N/head.
                if let Some(x) = base_refs.get(&format!("refs/pull/{}/head", row.number)) {
                    head = x.clone();
                }
            }
        }
    }
    Ok((base, head))
}

/// Computes commits, changed files and mergeability for a PR.
pub async fn compute_git(s: &AppState, row: &IssueRow, pr: &PullRow) -> anyhow::Result<PrGit> {
    let (base_oid, head_oid) = pr_oids(s, row, pr).await?;
    let path = s.git.sync(pr.repo_id).await?;
    let git = &s.git.git;
    let mb = {
        let (ok, out, _) = git.try_run(&path, &["merge-base", &base_oid, &head_oid]).await?;
        ok.then(|| String::from_utf8_lossy(&out).trim().to_string()).filter(|x| !x.is_empty())
    };
    let from = mb.clone().unwrap_or_else(|| base_oid.clone());
    let range = format!("{from}..{head_oid}");
    let commits = super::repo::git_log(s, pr.repo_id, &["--reverse", &range]).await?;
    let mut files = Vec::new();
    let (ok, numstat, _) = git.try_run(&path, &["diff", "--no-renames", "--numstat", &from, &head_oid]).await?;
    let (_, status, _) = git.try_run(&path, &["diff", "--no-renames", "--name-status", &from, &head_oid]).await?;
    if ok {
        let status = String::from_utf8_lossy(&status).to_string();
        let kinds: std::collections::HashMap<&str, &str> = status
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(k, p)| (p, k))
            .collect();
        for line in String::from_utf8_lossy(&numstat).lines() {
            let mut it = line.splitn(3, '\t');
            let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) else { continue };
            let change_type = match kinds.get(p).map(|k| &k[..1]) {
                Some("A") => PatchStatus::Added,
                Some("D") => PatchStatus::Deleted,
                Some("R") => PatchStatus::Renamed,
                Some("C") => PatchStatus::Copied,
                Some("M") => PatchStatus::Modified,
                _ => PatchStatus::Changed,
            };
            files.push(FileStat {
                path: p.to_string(),
                additions: a.parse().unwrap_or(0),
                deletions: d.parse().unwrap_or(0),
                change_type,
            });
        }
    }
    let mergeable = if row.state == "open" && !pr.merged {
        let (ok, _, _) = git.try_run(&path, &["merge-tree", "--write-tree", "--no-messages", &base_oid, &head_oid]).await?;
        if ok { MergeableState::Mergeable } else { MergeableState::Conflicting }
    } else {
        MergeableState::Unknown
    };
    Ok(PrGit { base_oid, head_oid, merge_base: mb, commits, files, mergeable })
}

impl GPull {
    pub fn new(row: Arc<IssueRow>, pr: Arc<PullRow>, repo: Arc<RepoCtx>) -> Self {
        GPull { row, pr, repo, git: Arc::new(OnceCell::new()) }
    }

    pub async fn git(&self, ctx: &Context<'_>) -> Option<Arc<PrGit>> {
        self.git
            .get_or_init(|| async {
                match compute_git(st(ctx), &self.row, &self.pr).await {
                    Ok(g) => Some(Arc::new(g)),
                    Err(e) => {
                        tracing::warn!(error = %e, "pull request git info unavailable");
                        None
                    }
                }
            })
            .await
            .clone()
    }

    async fn reviews_all(&self, ctx: &Context<'_>) -> GResult<Vec<ReviewRow>> {
        Ok(sqlx::query_as("SELECT * FROM pull_reviews WHERE issue_id = $1 AND state <> 'PENDING' ORDER BY id")
            .bind(self.row.id)
            .fetch_all(&st(ctx).db)
            .await?)
    }

    /// Latest submitted review per author.
    async fn latest_reviews_rows(&self, ctx: &Context<'_>) -> GResult<Vec<ReviewRow>> {
        let all = self.reviews_all(ctx).await?;
        let mut latest: Vec<ReviewRow> = Vec::new();
        for r in all.into_iter().rev() {
            if !latest.iter().any(|x| x.user_id == r.user_id) {
                latest.push(r);
            }
        }
        latest.reverse();
        Ok(latest)
    }

    fn state(&self) -> PullRequestState {
        if self.pr.merged {
            PullRequestState::Merged
        } else if self.row.state == "closed" {
            PullRequestState::Closed
        } else {
            PullRequestState::Open
        }
    }

    fn review(&self, r: ReviewRow) -> GReview {
        GReview { row: Arc::new(r), repo: self.repo.clone(), number: self.row.number }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn list_pulls(
    ctx: &Context<'_>,
    repo: &Arc<RepoCtx>,
    args: ConnArgs,
    states: Option<Vec<PullRequestState>>,
    labels: Option<Vec<String>>,
    head_ref_name: Option<String>,
    base_ref_name: Option<String>,
    order_by: Option<IssueOrder>,
) -> GResult<PullRequestConnection> {
    let q = IssueQuery {
        repo_id: Some(repo.repo.id),
        is_pull: Some(true),
        states: states.map(|v| v.iter().map(|s| pr_state_str(*s)).collect()),
        labels_any: labels.unwrap_or_default(),
        head_ref: head_ref_name,
        base_ref: base_ref_name,
        order: issue::issue_order(order_by),
        ..Default::default()
    };
    let (rows, w) = q.run(ctx, &args).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let prs = loader(ctx).load_many(ids.iter().map(|i| loaders::PullOf(*i))).await.map_err(lerr)?;
    let items = rows
        .into_iter()
        .filter_map(|row| {
            let pr = prs.get(&loaders::PullOf(row.id))?.clone();
            Some(GPull::new(Arc::new(row), pr, repo.clone()))
        })
        .collect();
    Ok(PullRequestConnection::new(items, w))
}

pub fn pr_state_str(s: PullRequestState) -> &'static str {
    match s {
        PullRequestState::Open => "OPEN",
        PullRequestState::Closed => "CLOSED",
        PullRequestState::Merged => "MERGED",
    }
}

#[allow(clippy::too_many_arguments)]
#[Object(name = "PullRequest")]
impl GPull {
    async fn id(&self) -> ID {
        ID(ids::encode("PullRequest", self.row.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.row.id)
    }
    async fn full_database_id(&self) -> Option<String> {
        Some(self.row.id.to_string())
    }
    async fn number(&self) -> i32 {
        self.row.number as i32
    }
    async fn title(&self) -> String {
        self.row.title.clone()
    }
    #[graphql(name = "titleHTML")]
    async fn title_html(&self) -> Html {
        Html(html_escape(&self.row.title))
    }
    async fn body(&self) -> String {
        self.row.body.clone().unwrap_or_default()
    }
    async fn body_text(&self) -> String {
        self.row.body.clone().unwrap_or_default()
    }
    #[graphql(name = "bodyHTML")]
    async fn body_html(&self) -> Html {
        body_html(self.row.body.as_deref().unwrap_or(""))
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(issue::html_url(ctx, &self.repo, &self.row))
    }
    async fn permalink(&self, ctx: &Context<'_>) -> Uri {
        Uri(issue::html_url(ctx, &self.repo, &self.row))
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/pull/{}", self.repo.full_name(), self.row.number))
    }
    async fn state(&self) -> PullRequestState {
        self.state()
    }
    async fn closed(&self) -> bool {
        self.row.state == "closed"
    }
    async fn closed_at(&self) -> Option<DateTime> {
        dt_opt(self.row.closed_at)
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn published_at(&self) -> Option<DateTime> {
        Some(dt(self.row.created_at))
    }
    async fn last_edited_at(&self) -> Option<DateTime> {
        None
    }
    async fn author(&self, ctx: &Context<'_>) -> GResult<Option<Actor>> {
        issue::author(ctx, &self.row).await
    }
    async fn editor(&self) -> Option<Actor> {
        None
    }
    async fn author_association(&self, ctx: &Context<'_>) -> GResult<CommentAuthorAssociation> {
        issue::association(ctx, self.repo.repo.id, self.row.user_id).await
    }
    async fn includes_created_edit(&self) -> bool {
        false
    }
    async fn assignees(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> GResult<UserConnection> {
        issue::assignees(ctx, &self.row, ConnArgs::new(first, after, last, before)).await
    }
    async fn assigned_actors(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> GResult<AssigneeConnection> {
        issue::assigned_actors(ctx, &self.row, ConnArgs::new(first, after, last, before)).await
    }
    async fn labels(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<LabelOrder>,
    ) -> GResult<Option<LabelConnection>> {
        issue::labels(ctx, &self.row, &self.repo, ConnArgs::new(first, after, last, before), order_by).await
    }
    async fn milestone(&self, ctx: &Context<'_>) -> GResult<Option<GMilestone>> {
        issue::milestone(ctx, &self.row, &self.repo).await
    }
    async fn comments(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<IssueCommentOrder>,
    ) -> GResult<IssueCommentConnection> {
        let _ = order_by;
        issue::comments(ctx, &self.row, &self.repo, ConnArgs::new(first, after, last, before)).await
    }
    async fn total_comments_count(&self, ctx: &Context<'_>) -> GResult<Option<i32>> {
        Ok(loader(ctx).load_one(loaders::CommentCount(self.row.id)).await.map_err(lerr)?.map(|n| n as i32))
    }
    async fn reaction_groups(&self, ctx: &Context<'_>) -> GResult<Option<Vec<ReactionGroup>>> {
        Ok(Some(issue::issue_reaction_groups(ctx, &self.row).await?))
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
    async fn base_repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
    async fn locked(&self) -> bool {
        self.row.locked
    }
    async fn active_lock_reason(&self) -> Option<LockReason> {
        self.row.active_lock_reason.as_deref().and_then(LockReason::parse)
    }
    async fn project_cards(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        archived_states: Option<Vec<Option<ProjectCardArchivedState>>>,
    ) -> ProjectCardConnection {
        let _ = (first, after, last, before, archived_states);
        ProjectCardConnection::empty()
    }
    async fn project_items(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        #[graphql(default = true)] include_archived: bool,
    ) -> ProjectV2ItemConnection {
        let _ = (first, after, last, before, include_archived);
        ProjectV2ItemConnection::empty()
    }
    async fn closing_issues_references(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        #[graphql(default = false)] user_linked_only: bool,
    ) -> Option<IssueConnection> {
        let _ = (first, after, last, before, user_linked_only);
        Some(IssueConnection::empty())
    }
    async fn participants(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> GResult<UserConnection> {
        issue::participants(ctx, &self.row, ConnArgs::new(first, after, last, before)).await
    }

    // --- pull request specific ---

    async fn head_ref_name(&self) -> String {
        self.pr.head_ref.clone()
    }
    async fn base_ref_name(&self) -> String {
        self.pr.base_ref.clone()
    }
    async fn head_ref_oid(&self, ctx: &Context<'_>) -> GitObjectId {
        match self.git(ctx).await {
            Some(g) => GitObjectId(g.head_oid.clone()),
            None => GitObjectId(self.pr.head_sha.clone()),
        }
    }
    async fn base_ref_oid(&self, ctx: &Context<'_>) -> GitObjectId {
        match self.git(ctx).await {
            Some(g) => GitObjectId(g.base_oid.clone()),
            None => GitObjectId(self.pr.base_sha.clone()),
        }
    }
    async fn base_ref(&self, ctx: &Context<'_>) -> GResult<Option<GRef>> {
        let refs = repo_refs(ctx, self.repo.repo.id).await?;
        let name = format!("refs/heads/{}", self.pr.base_ref);
        Ok(refs.get(&name).map(|oid| GRef { repo: self.repo.clone(), name, oid: oid.clone() }))
    }
    async fn head_ref(&self, ctx: &Context<'_>) -> GResult<Option<GRef>> {
        let Some(hid) = self.pr.head_repo_id else { return Ok(None) };
        let Some(hrepo) = loaders::repo(ctx, hid).await? else { return Ok(None) };
        let refs = repo_refs(ctx, hid).await?;
        let name = format!("refs/heads/{}", self.pr.head_ref);
        Ok(refs.get(&name).map(|oid| GRef { repo: hrepo, name, oid: oid.clone() }))
    }
    async fn head_repository(&self, ctx: &Context<'_>) -> GResult<Option<GRepo>> {
        match self.pr.head_repo_id {
            Some(h) => Ok(loaders::repo(ctx, h).await?.map(GRepo::new)),
            None => Ok(None),
        }
    }
    async fn head_repository_owner(&self, ctx: &Context<'_>) -> GResult<Option<RepositoryOwner>> {
        match self.pr.head_repo_id {
            Some(h) => Ok(loaders::repo(ctx, h).await?.map(|r| RepositoryOwner::from_user(Arc::new(r.owner.clone())))),
            None => Ok(None),
        }
    }
    async fn is_cross_repository(&self) -> bool {
        self.pr.head_repo_id != Some(self.pr.repo_id)
    }
    async fn is_draft(&self) -> bool {
        self.pr.draft
    }
    async fn maintainer_can_modify(&self) -> bool {
        self.pr.maintainer_can_modify
    }
    async fn merged(&self) -> bool {
        self.pr.merged
    }
    async fn merged_at(&self) -> Option<DateTime> {
        dt_opt(self.pr.merged_at)
    }
    async fn merged_by(&self, ctx: &Context<'_>) -> GResult<Option<Actor>> {
        Ok(loaders::user(ctx, self.pr.merged_by_id).await?.map(Actor::from_user))
    }
    async fn merge_commit(&self) -> Option<GCommit> {
        self.pr.merge_commit_sha.clone().filter(|_| self.pr.merged).map(|sha| GCommit::new(self.repo.clone(), sha))
    }
    async fn potential_merge_commit(&self) -> Option<GCommit> {
        None
    }
    async fn mergeable(&self, ctx: &Context<'_>) -> MergeableState {
        self.git(ctx).await.map(|g| g.mergeable).unwrap_or(MergeableState::Unknown)
    }
    async fn merge_state_status(&self, ctx: &Context<'_>) -> MergeStateStatus {
        if self.state() != PullRequestState::Open {
            return MergeStateStatus::Unknown;
        }
        match self.git(ctx).await.map(|g| g.mergeable) {
            Some(MergeableState::Conflicting) => MergeStateStatus::Dirty,
            Some(MergeableState::Mergeable) if self.pr.draft => MergeStateStatus::Draft,
            Some(MergeableState::Mergeable) => MergeStateStatus::Clean,
            _ => MergeStateStatus::Unknown,
        }
    }
    async fn can_be_rebased(&self) -> bool {
        true
    }
    async fn additions(&self, ctx: &Context<'_>) -> i32 {
        self.git(ctx).await.map(|g| g.files.iter().map(|f| f.additions).sum::<i64>()).unwrap_or(self.pr.additions) as i32
    }
    async fn deletions(&self, ctx: &Context<'_>) -> i32 {
        self.git(ctx).await.map(|g| g.files.iter().map(|f| f.deletions).sum::<i64>()).unwrap_or(self.pr.deletions) as i32
    }
    async fn changed_files(&self, ctx: &Context<'_>) -> i32 {
        self.git(ctx).await.map(|g| g.files.len() as i64).unwrap_or(self.pr.changed_files) as i32
    }
    async fn commits(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> PullRequestCommitConnection {
        let all: Vec<PullRequestCommit> = self
            .git(ctx)
            .await
            .map(|g| {
                g.commits
                    .iter()
                    .map(|c| PullRequestCommit {
                        commit: GCommit::with_info(self.repo.clone(), c.clone()),
                        pr_id: self.row.id,
                        number: self.row.number,
                        full_name: self.repo.full_name(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        PullRequestCommitConnection::from_all(all, &ConnArgs::new(first, after, last, before))
    }
    async fn files(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> Option<PullRequestChangedFileConnection> {
        let all: Vec<PullRequestChangedFile> =
            self.git(ctx).await.map(|g| g.files.iter().cloned().map(PullRequestChangedFile).collect()).unwrap_or_default();
        Some(PullRequestChangedFileConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn reviews(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        states: Option<Vec<PullRequestReviewState>>,
        author: Option<String>,
    ) -> GResult<Option<PullRequestReviewConnection>> {
        let mut rows = self.reviews_all(ctx).await?;
        if let Some(states) = states {
            rows.retain(|r| states.contains(&PullRequestReviewState::parse(&r.state)));
        }
        if let Some(a) = author {
            let u = crate::db::user_by_login(&st(ctx).db, &a).await.ok();
            rows.retain(|r| u.as_ref().is_some_and(|u| Some(u.id) == r.user_id));
        }
        let all = rows.into_iter().map(|r| self.review(r)).collect();
        Ok(Some(PullRequestReviewConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn latest_reviews(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> GResult<Option<PullRequestReviewConnection>> {
        let all = self.latest_reviews_rows(ctx).await?.into_iter().map(|r| self.review(r)).collect();
        Ok(Some(PullRequestReviewConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn latest_opinionated_reviews(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> GResult<Option<PullRequestReviewConnection>> {
        let all = self
            .latest_reviews_rows(ctx)
            .await?
            .into_iter()
            .filter(|r| r.state == "APPROVED" || r.state == "CHANGES_REQUESTED")
            .map(|r| self.review(r))
            .collect();
        Ok(Some(PullRequestReviewConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn review_decision(&self, ctx: &Context<'_>) -> GResult<Option<PullRequestReviewDecision>> {
        let latest = self.latest_reviews_rows(ctx).await?;
        Ok(if latest.iter().any(|r| r.state == "CHANGES_REQUESTED") {
            Some(PullRequestReviewDecision::ChangesRequested)
        } else if latest.iter().any(|r| r.state == "APPROVED") {
            Some(PullRequestReviewDecision::Approved)
        } else {
            None
        })
    }
    async fn review_requests(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> GResult<Option<ReviewRequestConnection>> {
        let ids: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM review_requests WHERE issue_id = $1 ORDER BY created_at, user_id")
            .bind(self.row.id)
            .fetch_all(&st(ctx).db)
            .await?;
        let users = loader(ctx).load_many(ids.iter().map(|i| loaders::UserId(*i))).await.map_err(lerr)?;
        let all = ids
            .iter()
            .filter_map(|i| users.get(&loaders::UserId(*i)).cloned())
            .map(|u| ReviewRequest { pr_id: self.row.id, reviewer: RequestedReviewer::User(GUser(u)) })
            .collect();
        Ok(Some(ReviewRequestConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn auto_merge_request(&self) -> Option<AutoMergeRequest> {
        None
    }
    async fn viewer_did_author(&self, ctx: &Context<'_>) -> bool {
        issue::viewer_did_author(ctx, self.row.user_id)
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> bool {
        self.repo.perm >= Perm::Triage || issue::viewer_did_author(ctx, self.row.user_id)
    }
    async fn viewer_can_react(&self) -> bool {
        true
    }
    async fn viewer_subscription(&self) -> Option<SubscriptionState> {
        Some(SubscriptionState::Unsubscribed)
    }
    async fn viewer_merge_headline_text(&self, merge_type: Option<PullRequestMergeMethod>) -> String {
        match merge_type.unwrap_or(PullRequestMergeMethod::Merge) {
            PullRequestMergeMethod::Merge => format!("Merge pull request #{} from {}", self.row.number, self.pr.head_ref),
            _ => format!("{} (#{})", self.row.title, self.row.number),
        }
    }
    async fn viewer_merge_body_text(&self, merge_type: Option<PullRequestMergeMethod>) -> String {
        match merge_type.unwrap_or(PullRequestMergeMethod::Merge) {
            PullRequestMergeMethod::Merge => self.row.title.clone(),
            _ => self.row.body.clone().unwrap_or_default(),
        }
    }
    async fn checks_url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/checks", issue::html_url(ctx, &self.repo, &self.row)))
    }
}

#[derive(Clone)]
pub struct PullRequestCommit {
    pub commit: GCommit,
    pub pr_id: i64,
    pub number: i64,
    pub full_name: String,
}

#[Object]
impl PullRequestCommit {
    async fn id(&self) -> ID {
        ID(ids::encode_str("PullRequestCommit", &format!("{}:{}", self.pr_id, self.commit.oid)))
    }
    async fn commit(&self) -> GCommit {
        self.commit.clone()
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/pull/{}/commits/{}", st(ctx).base_url(), self.full_name, self.number, self.commit.oid))
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/pull/{}/commits/{}", self.full_name, self.number, self.commit.oid))
    }
}

#[derive(Clone)]
pub struct PullRequestChangedFile(pub FileStat);

#[Object]
impl PullRequestChangedFile {
    async fn path(&self) -> String {
        self.0.path.clone()
    }
    async fn additions(&self) -> i32 {
        self.0.additions as i32
    }
    async fn deletions(&self) -> i32 {
        self.0.deletions as i32
    }
    async fn change_type(&self) -> PatchStatus {
        self.0.change_type
    }
}

#[derive(Clone)]
pub struct GReview {
    pub row: Arc<ReviewRow>,
    pub repo: Arc<RepoCtx>,
    pub number: i64,
}

#[Object(name = "PullRequestReview")]
impl GReview {
    async fn id(&self) -> ID {
        ID(ids::encode("PullRequestReview", self.row.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.row.id)
    }
    async fn full_database_id(&self) -> Option<String> {
        Some(self.row.id.to_string())
    }
    async fn author(&self, ctx: &Context<'_>) -> GResult<Option<Actor>> {
        Ok(loaders::user(ctx, self.row.user_id).await?.map(Actor::from_user))
    }
    async fn author_association(&self, ctx: &Context<'_>) -> GResult<CommentAuthorAssociation> {
        issue::association(ctx, self.repo.repo.id, self.row.user_id).await
    }
    async fn body(&self) -> String {
        self.row.body.clone()
    }
    async fn body_text(&self) -> String {
        self.row.body.clone()
    }
    #[graphql(name = "bodyHTML")]
    async fn body_html(&self) -> Html {
        body_html(&self.row.body)
    }
    async fn state(&self) -> PullRequestReviewState {
        PullRequestReviewState::parse(&self.row.state)
    }
    async fn submitted_at(&self) -> Option<DateTime> {
        dt_opt(self.row.submitted_at)
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn published_at(&self) -> Option<DateTime> {
        dt_opt(self.row.submitted_at)
    }
    async fn includes_created_edit(&self) -> bool {
        false
    }
    async fn reaction_groups(&self, ctx: &Context<'_>) -> Option<Vec<ReactionGroup>> {
        Some(issue::reaction_groups(ctx, None).await)
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/pull/{}#pullrequestreview-{}", st(ctx).base_url(), self.repo.full_name(), self.number, self.row.id))
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/pull/{}#pullrequestreview-{}", self.repo.full_name(), self.number, self.row.id))
    }
    async fn commit(&self) -> Option<GCommit> {
        self.row.commit_id.clone().map(|c| GCommit::new(self.repo.clone(), c))
    }
    async fn viewer_did_author(&self, ctx: &Context<'_>) -> bool {
        issue::viewer_did_author(ctx, self.row.user_id)
    }
}

#[derive(Clone)]
pub struct ReviewRequest {
    pr_id: i64,
    reviewer: RequestedReviewer,
}

#[Object]
impl ReviewRequest {
    async fn id(&self) -> ID {
        ID(ids::encode("ReviewRequest", self.pr_id))
    }
    async fn as_code_owner(&self) -> bool {
        false
    }
    async fn requested_reviewer(&self) -> Option<RequestedReviewer> {
        Some(self.reviewer.clone())
    }
}

/// Auto-merge is not supported; always null.
pub struct AutoMergeRequest;

#[Object]
impl AutoMergeRequest {
    async fn author_email(&self) -> Option<String> {
        None
    }
    async fn commit_body(&self) -> Option<String> {
        None
    }
    async fn commit_headline(&self) -> Option<String> {
        None
    }
    async fn merge_method(&self) -> PullRequestMergeMethod {
        PullRequestMergeMethod::Merge
    }
    async fn enabled_at(&self) -> Option<DateTime> {
        None
    }
    async fn enabled_by(&self) -> Option<Actor> {
        None
    }
}
