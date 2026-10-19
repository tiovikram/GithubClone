//! Pull request review threads: `PullRequest.reviewThreads`, the `PullRequestReviewThread` and
//! `PullRequestReviewComment` objects, and the `resolveReviewThread` / `unresolveReviewThread`
//! mutations. A thread is a top-level review comment plus its replies (`in_reply_to_id`);
//! resolution is stored per root comment (`review_thread_resolutions`).
use std::sync::Arc;

use async_graphql::{Context, InputObject, Object, SimpleObject, ID};

use super::connection::ConnArgs;
use super::enums::{dt, DateTime, Uri};
use super::loaders;
use super::repo::{st, viewer};
use super::user::{Actor, GUser};
use super::{gql_err, not_found, Viewer};
use crate::db::{Perm, RepoCtx, User};
use crate::ids;
use crate::pulls::reviews::ReviewCommentRow;

crate::gql_connection!(
    PullRequestReviewThreadConnection,
    PullRequestReviewThreadEdge,
    "PullRequestReviewThreadConnection",
    "PullRequestReviewThreadEdge",
    GThread
);
crate::gql_connection!(
    PullRequestReviewCommentConnection,
    PullRequestReviewCommentEdge,
    "PullRequestReviewCommentConnection",
    "PullRequestReviewCommentEdge",
    GReviewComment
);

#[derive(Clone)]
pub struct GThread {
    root: Arc<ReviewCommentRow>,
    comments: Arc<Vec<ReviewCommentRow>>,
    resolved_by: Option<Option<i64>>,
    repo: Arc<RepoCtx>,
    pr_author: Option<i64>,
    number: i64,
}

#[derive(Clone)]
pub struct GReviewComment {
    row: Arc<ReviewCommentRow>,
    repo: Arc<RepoCtx>,
    number: i64,
}

/// Visible review comments of a pull request (hiding other users' pending reviews), by id.
async fn visible_comments(ctx: &Context<'_>, issue_id: i64) -> async_graphql::Result<Vec<ReviewCommentRow>> {
    let me = viewer(ctx).map(|u| u.id).unwrap_or(0);
    Ok(sqlx::query_as(
        "SELECT c.* FROM review_comments c LEFT JOIN pull_reviews r ON r.id = c.review_id
         WHERE c.issue_id = $1 AND (r.state IS DISTINCT FROM 'PENDING' OR c.user_id = $2) ORDER BY c.id",
    )
    .bind(issue_id)
    .bind(me)
    .fetch_all(&st(ctx).db)
    .await?)
}

/// The threads of a pull request (`issue_id`), oldest first.
pub async fn threads(ctx: &Context<'_>, repo: Arc<RepoCtx>, issue_id: i64, number: i64, pr_author: Option<i64>) -> async_graphql::Result<Vec<GThread>> {
    let all = visible_comments(ctx, issue_id).await?;
    let resolved: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT t.root_comment_id, t.resolved_by FROM review_thread_resolutions t JOIN review_comments c ON c.id = t.root_comment_id WHERE c.issue_id = $1",
    )
    .bind(issue_id)
    .fetch_all(&st(ctx).db)
    .await?;
    // Replies may chain (a reply to a reply); walk up to the root.
    let parent_of: std::collections::HashMap<i64, Option<i64>> = all.iter().map(|c| (c.id, c.in_reply_to_id)).collect();
    let root_of = |mut id: i64| {
        for _ in 0..64 {
            match parent_of.get(&id).copied().flatten() {
                Some(p) if parent_of.contains_key(&p) => id = p,
                _ => break,
            }
        }
        id
    };
    let mut out: Vec<GThread> = Vec::new();
    for c in all.iter().filter(|c| c.in_reply_to_id.is_none() || !parent_of.contains_key(&c.in_reply_to_id.unwrap_or(0))) {
        let members: Vec<ReviewCommentRow> = all.iter().filter(|x| root_of(x.id) == c.id).cloned().collect();
        out.push(GThread {
            root: Arc::new(c.clone()),
            comments: Arc::new(members),
            resolved_by: resolved.iter().find(|(r, _)| *r == c.id).map(|(_, by)| *by),
            repo: repo.clone(),
            pr_author,
            number,
        });
    }
    Ok(out)
}

impl GThread {
    fn can_resolve(&self, ctx: &Context<'_>) -> bool {
        let Some(v) = viewer(ctx) else { return false };
        self.repo.perm >= Perm::Write || Some(v.id) == self.pr_author || Some(v.id) == self.root.user_id
    }
}

#[Object(name = "PullRequestReviewThread")]
impl GThread {
    async fn id(&self) -> ID {
        ID(ids::encode("PullRequestReviewThread", self.root.id))
    }
    async fn is_resolved(&self) -> bool {
        self.resolved_by.is_some()
    }
    async fn is_collapsed(&self) -> bool {
        self.resolved_by.is_some()
    }
    /// Outdated: the comment no longer maps onto the current diff.
    async fn is_outdated(&self) -> bool {
        self.root.line.is_none() && self.root.subject_type == "line"
    }
    async fn path(&self) -> &str {
        &self.root.path
    }
    async fn line(&self) -> Option<i64> {
        self.root.line
    }
    async fn original_line(&self) -> Option<i64> {
        self.root.original_line
    }
    async fn start_line(&self) -> Option<i64> {
        self.root.start_line
    }
    async fn original_start_line(&self) -> Option<i64> {
        self.root.original_start_line
    }
    async fn diff_side(&self) -> &str {
        if self.root.side == "LEFT" { "LEFT" } else { "RIGHT" }
    }
    async fn subject_type(&self) -> &str {
        if self.root.subject_type == "file" { "FILE" } else { "LINE" }
    }
    async fn resolved_by(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GUser>> {
        match self.resolved_by {
            Some(by) => Ok(loaders::user(ctx, by).await?.map(GUser)),
            None => Ok(None),
        }
    }
    async fn viewer_can_resolve(&self, ctx: &Context<'_>) -> bool {
        self.resolved_by.is_none() && self.can_resolve(ctx)
    }
    async fn viewer_can_unresolve(&self, ctx: &Context<'_>) -> bool {
        self.resolved_by.is_some() && self.can_resolve(ctx)
    }
    async fn viewer_can_reply(&self, ctx: &Context<'_>) -> bool {
        viewer(ctx).is_some()
    }
    async fn comments(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>, skip: Option<i32>) -> PullRequestReviewCommentConnection {
        let all: Vec<GReviewComment> = self
            .comments
            .iter()
            .skip(skip.unwrap_or(0).max(0) as usize)
            .map(|c| GReviewComment { row: Arc::new(c.clone()), repo: self.repo.clone(), number: self.number })
            .collect();
        PullRequestReviewCommentConnection::from_all(all, &ConnArgs::new(first, after, last, before))
    }
}

#[Object(name = "PullRequestReviewComment")]
impl GReviewComment {
    async fn id(&self) -> ID {
        ID(ids::encode("PullRequestReviewComment", self.row.id))
    }
    async fn database_id(&self) -> i64 {
        self.row.id
    }
    async fn full_database_id(&self) -> String {
        self.row.id.to_string()
    }
    async fn author(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Actor>> {
        Ok(loaders::user(ctx, self.row.user_id).await?.map(Actor::from_user))
    }
    async fn body(&self) -> &str {
        &self.row.body
    }
    async fn body_text(&self) -> &str {
        &self.row.body
    }
    async fn path(&self) -> &str {
        &self.row.path
    }
    async fn line(&self) -> Option<i64> {
        self.row.line
    }
    async fn original_line(&self) -> Option<i64> {
        self.row.original_line
    }
    async fn start_line(&self) -> Option<i64> {
        self.row.start_line
    }
    async fn diff_hunk(&self) -> &str {
        &self.row.diff_hunk
    }
    async fn outdated(&self) -> bool {
        self.row.line.is_none() && self.row.subject_type == "line"
    }
    async fn reply_to(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GReviewComment>> {
        let Some(pid) = self.row.in_reply_to_id else { return Ok(None) };
        let row: Option<ReviewCommentRow> = sqlx::query_as("SELECT * FROM review_comments WHERE id = $1").bind(pid).fetch_optional(&st(ctx).db).await?;
        Ok(row.map(|r| GReviewComment { row: Arc::new(r), repo: self.repo.clone(), number: self.number }))
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/pull/{}#discussion_r{}", st(ctx).base_url(), self.repo.full_name(), self.number, self.row.id))
    }
    async fn viewer_did_author(&self, ctx: &Context<'_>) -> bool {
        viewer(ctx).is_some_and(|v| Some(v.id) == self.row.user_id)
    }
}

// ---------------------------------------------------------------------------------------------
// Mutations

#[derive(InputObject)]
pub struct ResolveReviewThreadInput {
    pub thread_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UnresolveReviewThreadInput {
    pub thread_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct ResolveReviewThreadPayload {
    pub client_mutation_id: Option<String>,
    pub thread: Option<GThread>,
}

#[derive(SimpleObject)]
pub struct UnresolveReviewThreadPayload {
    pub client_mutation_id: Option<String>,
    pub thread: Option<GThread>,
}

#[derive(Default)]
pub struct ReviewThreadMutation;

fn actor<'a>(ctx: &'a Context<'_>) -> &'a User {
    &ctx.data_unchecked::<Viewer>().0
}

/// Loads the thread named by `id` (a PRRT_ id; a root comment's PRRC_ id is accepted too).
async fn load_thread(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<GThread> {
    let unresolvable = || not_found(format!("Could not resolve to a node with the global id of '{}'.", id.as_str()));
    let root_id = match ids::decode(id) {
        Some((k, v)) if k == "PullRequestReviewThread" || k == "PullRequestReviewComment" => v.parse::<i64>().map_err(|_| unresolvable())?,
        _ => return Err(unresolvable()),
    };
    let row: Option<ReviewCommentRow> = sqlx::query_as("SELECT * FROM review_comments WHERE id = $1").bind(root_id).fetch_optional(&st(ctx).db).await?;
    let row = row.ok_or_else(unresolvable)?;
    let repo = loaders::repo(ctx, row.repo_id).await?.ok_or_else(unresolvable)?;
    let (number, author): (i64, Option<i64>) =
        sqlx::query_as("SELECT number, user_id FROM issues WHERE id = $1").bind(row.issue_id).fetch_one(&st(ctx).db).await?;
    threads(ctx, repo, row.issue_id, number, author).await?.into_iter().find(|t| t.root.id == root_id).ok_or_else(unresolvable)
}

#[Object]
impl ReviewThreadMutation {
    async fn resolve_review_thread(&self, ctx: &Context<'_>, input: ResolveReviewThreadInput) -> async_graphql::Result<Option<ResolveReviewThreadPayload>> {
        let t = load_thread(ctx, &input.thread_id).await?;
        if !t.can_resolve(ctx) {
            return Err(gql_err("FORBIDDEN", format!("{} does not have permission to resolve this thread.", actor(ctx).login)));
        }
        sqlx::query("INSERT INTO review_thread_resolutions (root_comment_id, resolved_by) VALUES ($1, $2) ON CONFLICT (root_comment_id) DO NOTHING")
            .bind(t.root.id)
            .bind(actor(ctx).id)
            .execute(&st(ctx).db)
            .await?;
        let thread = load_thread(ctx, &input.thread_id).await?;
        Ok(Some(ResolveReviewThreadPayload { client_mutation_id: input.client_mutation_id, thread: Some(thread) }))
    }

    async fn unresolve_review_thread(&self, ctx: &Context<'_>, input: UnresolveReviewThreadInput) -> async_graphql::Result<Option<UnresolveReviewThreadPayload>> {
        let t = load_thread(ctx, &input.thread_id).await?;
        if !t.can_resolve(ctx) {
            return Err(gql_err("FORBIDDEN", format!("{} does not have permission to unresolve this thread.", actor(ctx).login)));
        }
        sqlx::query("DELETE FROM review_thread_resolutions WHERE root_comment_id = $1").bind(t.root.id).execute(&st(ctx).db).await?;
        let thread = load_thread(ctx, &input.thread_id).await?;
        Ok(Some(UnresolveReviewThreadPayload { client_mutation_id: input.client_mutation_id, thread: Some(thread) }))
    }
}
