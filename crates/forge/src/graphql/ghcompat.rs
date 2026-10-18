//! GraphQL additions for gh commands beyond the original subset: the mutations behind
//! `gh repo archive|unarchive`, `gh issue transfer|pin|unpin|lock|unlock|develop` and
//! `gh pr lock|unlock|revert|update-branch`, plus `Ref.compare`, `Issue.isPinned` and
//! `Issue.linkedBranches`. Thin adapters over the service layer (`api::issue_ops`,
//! `api::repo_ops`, `api::issues`, `pulls`), merged into the schema's mutation root.
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use async_graphql::parser::types::{DocumentOperations, OperationType, Selection};
use async_graphql::{Context, Enum, InputObject, Interface, Object, SimpleObject, ID};
use serde_json::{json, Value};

use super::connection::ConnArgs;
use super::enums::{GitObjectId, LockReason};
use super::issue::{self, GIssue, IssueOrPullRequest};
use super::loaders;
use super::pull::GPull;
use super::repo::{st, GRef, GRepo};
use super::{api_err, gql_err, not_found, Viewer};
use crate::api::{issue_ops, repo_ops};
use crate::db::{RepoCtx, User};
use crate::ids;
use crate::json::issue::IssueRow;

fn actor<'a>(ctx: &'a Context<'_>) -> &'a User {
    &ctx.data_unchecked::<Viewer>().0
}

fn unresolvable(id: &str) -> async_graphql::Error {
    not_found(format!("Could not resolve to a node with the global id of '{id}'."))
}

fn dec(id: &ID, kinds: &[&str]) -> async_graphql::Result<i64> {
    match ids::decode(id) {
        Some((k, v)) if kinds.contains(&k.as_str()) => v.parse().map_err(|_| unresolvable(id)),
        _ => Err(unresolvable(id)),
    }
}

async fn repo_ctx(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<Arc<RepoCtx>> {
    let rid = dec(id, &["Repository"])?;
    loaders::repo(ctx, rid).await?.ok_or_else(|| unresolvable(id))
}

/// An Issue/PullRequest by node id (fresh from the database) with its repository.
async fn subject(ctx: &Context<'_>, id: &ID, kinds: &[&str]) -> async_graphql::Result<(Arc<RepoCtx>, IssueRow)> {
    let iid = dec(id, kinds)?;
    let row: Option<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(iid).fetch_optional(&st(ctx).db).await?;
    let row = row.ok_or_else(|| unresolvable(id))?;
    if kinds == ["PullRequest"] && !row.is_pull {
        return Err(not_found(format!("Could not resolve to PullRequest node with the global id of '{}'.", id.as_str())));
    }
    let repo = loaders::repo(ctx, row.repo_id).await?.ok_or_else(|| unresolvable(id))?;
    Ok((repo, row))
}

async fn reload_issue(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GIssue>> {
    Ok(match issue::by_id(ctx, id).await? {
        Some(IssueOrPullRequest::Issue(i)) => Some(i),
        _ => None,
    })
}

async fn reload_pull(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GPull>> {
    Ok(match issue::by_id(ctx, id).await? {
        Some(IssueOrPullRequest::PullRequest(p)) => Some(p),
        _ => None,
    })
}

async fn lockable(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<Lockable>> {
    Ok(issue::by_id(ctx, id).await?.map(|x| match x {
        IssueOrPullRequest::Issue(i) => Lockable::Issue(i),
        IssueOrPullRequest::PullRequest(p) => Lockable::PullRequest(p),
    }))
}

/// Locks/unlocks a Discussion node; `None` when `id` is not a discussion.
async fn discussion_lockable(ctx: &Context<'_>, id: &ID, locked: bool) -> async_graphql::Result<Option<Lockable>> {
    let Some(did) = ids::decode_int(id, "Discussion") else { return Ok(None) };
    let d = super::discussions::discussion_node(ctx, did).await?.ok_or_else(|| unresolvable(id))?;
    let row = crate::api::discussions::set_locked(st(ctx), &d.repo, &d.row, locked).await.map_err(api_err)?;
    Ok(Some(Lockable::Discussion(super::discussions::GDiscussion { row: Arc::new(row), repo: d.repo.clone() })))
}

// ---------------------------------------------------------------------------------------------
// Unknown mutations

/// Field names of the mutation root.
fn mutation_fields() -> &'static HashSet<String> {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut reg = async_graphql::registry::Registry::default();
        <super::MutationRoot as async_graphql::OutputType>::create_type_info(&mut reg);
        reg.types.get("Mutation").and_then(|t| t.fields()).map(|f| f.keys().cloned().collect()).unwrap_or_default()
    })
}

/// GitHub rejects selections of fields that don't exist. In fast validation mode async-graphql
/// resolves an unknown field to `null` instead, so an unimplemented mutation would look like a
/// success to gh. Returns GitHub's `undefinedField` error response for unknown top-level
/// mutation fields of the operation that will run.
pub fn undefined_mutation_fields(req: &async_graphql::Request) -> Option<Value> {
    let doc = async_graphql::parser::parse_query(&req.query).ok()?;
    let ops: Vec<_> = match &doc.operations {
        DocumentOperations::Single(op) => vec![op],
        DocumentOperations::Multiple(m) => match &req.operation_name {
            Some(n) => m.iter().filter(|(k, _)| k.as_str() == n).map(|(_, v)| v).collect(),
            None => m.values().collect(),
        },
    };
    let known = mutation_fields();
    let mut errors = Vec::new();
    for op in ops.into_iter().filter(|op| op.node.ty == OperationType::Mutation) {
        for sel in &op.node.selection_set.node.items {
            let Selection::Field(f) = &sel.node else { continue };
            let name = f.node.name.node.as_str();
            if name.starts_with("__") || known.contains(name) {
                continue;
            }
            errors.push(json!({
                "path": ["mutation", name],
                "extensions": { "code": "undefinedField", "typeName": "Mutation", "fieldName": name },
                "locations": [{ "line": f.pos.line, "column": f.pos.column }],
                "message": format!("Field '{name}' doesn't exist on type 'Mutation'"),
            }));
        }
    }
    (!errors.is_empty()).then(|| json!({ "errors": errors }))
}

// ---------------------------------------------------------------------------------------------
// Types

/// An object that can be locked (GitHub's `Lockable` interface).
#[derive(Interface, Clone)]
#[graphql(
    name = "Lockable",
    field(name = "locked", ty = "bool"),
    field(name = "active_lock_reason", ty = "Option<LockReason>")
)]
pub enum Lockable {
    Issue(GIssue),
    PullRequest(GPull),
    Discussion(super::discussions::GDiscussion),
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PullRequestBranchUpdateMethod {
    Merge,
    Rebase,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ComparisonStatus {
    Diverged,
    Ahead,
    Behind,
    Identical,
}

/// `Ref.compare(headRef:)`: how `headRef` relates to this ref.
#[derive(SimpleObject, Clone)]
pub struct Comparison {
    pub ahead_by: i32,
    pub behind_by: i32,
    pub status: ComparisonStatus,
}

/// A branch linked to an issue (`gh issue develop`).
#[derive(Clone)]
pub struct LinkedBranch {
    id: i64,
    git_ref: GRef,
}

#[Object]
impl LinkedBranch {
    async fn id(&self) -> ID {
        ID(ids::encode("LinkedBranch", self.id))
    }
    #[graphql(name = "ref")]
    async fn git_ref(&self) -> Option<GRef> {
        Some(self.git_ref.clone())
    }
}

crate::gql_connection!(LinkedBranchConnection, LinkedBranchEdge, "LinkedBranchConnection", "LinkedBranchEdge", LinkedBranch);

/// `Issue.isPinned`.
pub async fn is_pinned(ctx: &Context<'_>, row: &IssueRow) -> async_graphql::Result<Option<bool>> {
    if row.is_pull {
        return Ok(Some(false));
    }
    Ok(Some(issue_ops::is_pinned(st(ctx), row.id).await.map_err(api_err)?))
}

/// `Issue.linkedBranches`: links whose branch still exists, in repositories the viewer can see.
pub async fn linked_branches(ctx: &Context<'_>, row: &IssueRow, args: ConnArgs) -> async_graphql::Result<LinkedBranchConnection> {
    let mut all = Vec::new();
    for (l, sha) in issue_ops::linked_branches(st(ctx), row.id).await.map_err(api_err)? {
        if let Some(repo) = loaders::repo(ctx, l.repo_id).await? {
            all.push(LinkedBranch { id: l.id, git_ref: GRef { repo, name: l.ref_name, oid: sha } });
        }
    }
    Ok(LinkedBranchConnection::from_all(all, &args))
}

/// `Ref.compare(headRef:)`. `headRef` is a branch (or `owner:branch` in a fork of the network);
/// None when it can't be resolved.
pub async fn compare(ctx: &Context<'_>, base: &GRef, head_ref: &str) -> async_graphql::Result<Option<Comparison>> {
    let s = st(ctx);
    let (repo_id, branch) = match head_ref.split_once(':') {
        None => (base.repo.repo.id, head_ref.to_string()),
        Some((owner, branch)) => {
            let id: Option<i64> = sqlx::query_scalar(
                "SELECT r.id FROM repositories r JOIN users u ON u.id = r.owner_id
                  WHERE lower(u.login) = lower($1)
                    AND (r.id = $2 OR r.fork_parent_id = $2 OR r.id = $3 OR (r.fork_parent_id IS NOT NULL AND r.fork_parent_id = $3))
                  ORDER BY (r.id = $2) DESC LIMIT 1",
            )
            .bind(owner)
            .bind(base.repo.repo.id)
            .bind(base.repo.repo.fork_parent_id)
            .fetch_optional(&s.db)
            .await?;
            match id {
                Some(id) => (id, branch.to_string()),
                None => return Ok(None),
            }
        }
    };
    if repo_id != base.repo.repo.id && loaders::repo(ctx, repo_id).await?.is_none() {
        return Ok(None);
    }
    let full = if branch.starts_with("refs/") { branch } else { format!("refs/heads/{branch}") };
    let (_, refs) = s.git.refs(repo_id).await.map_err(|e| gql_err("INTERNAL", e.to_string()))?;
    let Some(head_sha) = refs.get(&full).cloned() else { return Ok(None) };
    // Count in whichever repository has both commits (the base repo holds refs/pull/* heads).
    for rid in [base.repo.repo.id, repo_id] {
        let path = s.git.sync(rid).await.map_err(|e| gql_err("INTERNAL", e.to_string()))?;
        let range = format!("{}...{head_sha}", base.oid);
        let (ok, out, _) = s
            .git
            .git
            .try_run(&path, &["rev-list", "--left-right", "--count", &range, "--"])
            .await
            .map_err(|e| gql_err("INTERNAL", e.to_string()))?;
        if !ok {
            continue;
        }
        let text = String::from_utf8_lossy(&out);
        let mut it = text.split_whitespace().map(|n| n.parse::<i32>().unwrap_or(0));
        let (behind, ahead) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
        let status = match (ahead, behind) {
            (0, 0) => ComparisonStatus::Identical,
            (_, 0) => ComparisonStatus::Ahead,
            (0, _) => ComparisonStatus::Behind,
            _ => ComparisonStatus::Diverged,
        };
        return Ok(Some(Comparison { ahead_by: ahead, behind_by: behind, status }));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------------------------
// Inputs and payloads

#[derive(InputObject)]
pub struct ArchiveRepositoryInput {
    pub repository_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UnarchiveRepositoryInput {
    pub repository_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct ArchiveRepositoryPayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(SimpleObject)]
pub struct UnarchiveRepositoryPayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(InputObject)]
pub struct TransferIssueInput {
    pub issue_id: ID,
    pub repository_id: ID,
    pub create_labels_if_missing: Option<bool>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct TransferIssuePayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

#[derive(InputObject)]
pub struct PinIssueInput {
    pub issue_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UnpinIssueInput {
    pub issue_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct PinIssuePayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

#[derive(SimpleObject)]
pub struct UnpinIssuePayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

#[derive(InputObject)]
pub struct LockLockableInput {
    pub lockable_id: ID,
    pub lock_reason: Option<LockReason>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct LockLockablePayload {
    pub client_mutation_id: Option<String>,
    pub locked_record: Option<Lockable>,
}

#[derive(InputObject)]
pub struct UnlockLockableInput {
    pub lockable_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UnlockLockablePayload {
    pub client_mutation_id: Option<String>,
    pub unlocked_record: Option<Lockable>,
}

#[derive(InputObject)]
pub struct RevertPullRequestInput {
    pub pull_request_id: ID,
    pub title: Option<String>,
    pub body: Option<String>,
    pub draft: Option<bool>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct RevertPullRequestPayload {
    pub client_mutation_id: Option<String>,
    /// The pull request that was reverted.
    pub pull_request: Option<GPull>,
    /// The new pull request that reverts it.
    pub revert_pull_request: Option<GPull>,
}

#[derive(InputObject)]
pub struct UpdatePullRequestBranchInput {
    pub pull_request_id: ID,
    pub expected_head_oid: Option<GitObjectId>,
    pub update_method: Option<PullRequestBranchUpdateMethod>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UpdatePullRequestBranchPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(InputObject)]
pub struct CreateLinkedBranchInput {
    pub issue_id: ID,
    pub oid: GitObjectId,
    pub name: Option<String>,
    pub repository_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct CreateLinkedBranchPayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
    pub linked_branch: Option<LinkedBranch>,
}

#[derive(InputObject)]
pub struct DeleteLinkedBranchInput {
    pub linked_branch_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct DeleteLinkedBranchPayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

fn lock_reason_str(r: LockReason) -> &'static str {
    match r {
        LockReason::OffTopic => "off-topic",
        LockReason::TooHeated => "too heated",
        LockReason::Resolved => "resolved",
        LockReason::Spam => "spam",
    }
}

// ---------------------------------------------------------------------------------------------
// Mutations

#[derive(Default)]
pub struct GhMutation;

#[Object]
impl GhMutation {
    async fn archive_repository(&self, ctx: &Context<'_>, input: ArchiveRepositoryInput) -> async_graphql::Result<Option<ArchiveRepositoryPayload>> {
        let rc = repo_ctx(ctx, &input.repository_id).await?;
        let rc = repo_ops::set_archived(st(ctx), &rc, actor(ctx), true).await.map_err(api_err)?;
        Ok(Some(ArchiveRepositoryPayload { client_mutation_id: input.client_mutation_id, repository: Some(GRepo::new(Arc::new(rc))) }))
    }

    async fn unarchive_repository(&self, ctx: &Context<'_>, input: UnarchiveRepositoryInput) -> async_graphql::Result<Option<UnarchiveRepositoryPayload>> {
        let rc = repo_ctx(ctx, &input.repository_id).await?;
        let rc = repo_ops::set_archived(st(ctx), &rc, actor(ctx), false).await.map_err(api_err)?;
        Ok(Some(UnarchiveRepositoryPayload { client_mutation_id: input.client_mutation_id, repository: Some(GRepo::new(Arc::new(rc))) }))
    }

    async fn transfer_issue(&self, ctx: &Context<'_>, input: TransferIssueInput) -> async_graphql::Result<Option<TransferIssuePayload>> {
        let (src, row) = subject(ctx, &input.issue_id, &["Issue"]).await?;
        let dest = repo_ctx(ctx, &input.repository_id).await?;
        let moved = issue_ops::transfer_issue(st(ctx), &src, actor(ctx), &row, &dest, input.create_labels_if_missing.unwrap_or(false))
            .await
            .map_err(api_err)?;
        // Fresh row: the request's dataloader still caches the issue as it was before the move.
        let fresh: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(moved.id).fetch_one(&st(ctx).db).await?;
        let issue = match issue::materialize(ctx, vec![fresh]).await?.pop() {
            Some(IssueOrPullRequest::Issue(i)) => Some(i),
            _ => None,
        };
        Ok(Some(TransferIssuePayload { client_mutation_id: input.client_mutation_id, issue }))
    }

    async fn pin_issue(&self, ctx: &Context<'_>, input: PinIssueInput) -> async_graphql::Result<Option<PinIssuePayload>> {
        let (rc, row) = subject(ctx, &input.issue_id, &["Issue"]).await?;
        issue_ops::pin_issue(st(ctx), &rc, actor(ctx), &row).await.map_err(api_err)?;
        Ok(Some(PinIssuePayload { client_mutation_id: input.client_mutation_id, issue: reload_issue(ctx, row.id).await? }))
    }

    async fn unpin_issue(&self, ctx: &Context<'_>, input: UnpinIssueInput) -> async_graphql::Result<Option<UnpinIssuePayload>> {
        let (rc, row) = subject(ctx, &input.issue_id, &["Issue"]).await?;
        issue_ops::unpin_issue(st(ctx), &rc, actor(ctx), &row).await.map_err(api_err)?;
        Ok(Some(UnpinIssuePayload { client_mutation_id: input.client_mutation_id, issue: reload_issue(ctx, row.id).await? }))
    }

    async fn lock_lockable(&self, ctx: &Context<'_>, input: LockLockableInput) -> async_graphql::Result<Option<LockLockablePayload>> {
        if let Some(d) = discussion_lockable(ctx, &input.lockable_id, true).await? {
            return Ok(Some(LockLockablePayload { client_mutation_id: input.client_mutation_id, locked_record: Some(d) }));
        }
        let (rc, row) = subject(ctx, &input.lockable_id, &["Issue", "PullRequest"]).await?;
        let reason = input.lock_reason.map(|r| lock_reason_str(r).to_string());
        crate::api::issues::lock_issue(st(ctx), &rc, actor(ctx), row.number, reason).await.map_err(api_err)?;
        Ok(Some(LockLockablePayload { client_mutation_id: input.client_mutation_id, locked_record: lockable(ctx, row.id).await? }))
    }

    async fn unlock_lockable(&self, ctx: &Context<'_>, input: UnlockLockableInput) -> async_graphql::Result<Option<UnlockLockablePayload>> {
        if let Some(d) = discussion_lockable(ctx, &input.lockable_id, false).await? {
            return Ok(Some(UnlockLockablePayload { client_mutation_id: input.client_mutation_id, unlocked_record: Some(d) }));
        }
        let (rc, row) = subject(ctx, &input.lockable_id, &["Issue", "PullRequest"]).await?;
        crate::api::issues::unlock_issue(st(ctx), &rc, actor(ctx), row.number).await.map_err(api_err)?;
        Ok(Some(UnlockLockablePayload { client_mutation_id: input.client_mutation_id, unlocked_record: lockable(ctx, row.id).await? }))
    }

    async fn revert_pull_request(&self, ctx: &Context<'_>, input: RevertPullRequestInput) -> async_graphql::Result<Option<RevertPullRequestPayload>> {
        let (rc, row) = subject(ctx, &input.pull_request_id, &["PullRequest"]).await?;
        let revert = crate::pulls::revert::revert_pull(
            st(ctx),
            &rc,
            actor(ctx),
            row.number,
            crate::pulls::revert::RevertInput { title: input.title, body: input.body, draft: input.draft.unwrap_or(false) },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(RevertPullRequestPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request: reload_pull(ctx, row.id).await?,
            revert_pull_request: reload_pull(ctx, revert.issue.id).await?,
        }))
    }

    async fn update_pull_request_branch(&self, ctx: &Context<'_>, input: UpdatePullRequestBranchInput) -> async_graphql::Result<Option<UpdatePullRequestBranchPayload>> {
        let (rc, row) = subject(ctx, &input.pull_request_id, &["PullRequest"]).await?;
        let rebase = input.update_method == Some(PullRequestBranchUpdateMethod::Rebase);
        let expected = input.expected_head_oid.map(|o| o.0).filter(|o| !o.is_empty());
        crate::pulls::update_branch_with(st(ctx), &rc, actor(ctx), row.number, expected, rebase).await.map_err(api_err)?;
        Ok(Some(UpdatePullRequestBranchPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn create_linked_branch(&self, ctx: &Context<'_>, input: CreateLinkedBranchInput) -> async_graphql::Result<Option<CreateLinkedBranchPayload>> {
        let (rc, row) = subject(ctx, &input.issue_id, &["Issue"]).await?;
        let target = match &input.repository_id {
            Some(id) => repo_ctx(ctx, id).await?,
            None => rc.clone(),
        };
        let l = issue_ops::create_linked_branch(st(ctx), &rc, actor(ctx), &row, &target, &input.oid.0, input.name.as_deref())
            .await
            .map_err(api_err)?;
        let linked = LinkedBranch { id: l.id, git_ref: GRef { repo: target, name: l.ref_name, oid: input.oid.0.to_lowercase() } };
        Ok(Some(CreateLinkedBranchPayload {
            client_mutation_id: input.client_mutation_id,
            issue: reload_issue(ctx, row.id).await?,
            linked_branch: Some(linked),
        }))
    }

    async fn delete_linked_branch(&self, ctx: &Context<'_>, input: DeleteLinkedBranchInput) -> async_graphql::Result<Option<DeleteLinkedBranchPayload>> {
        let lid = dec(&input.linked_branch_id, &["LinkedBranch"])?;
        let s = st(ctx);
        let issue_id: Option<i64> = sqlx::query_scalar("SELECT issue_id FROM linked_branches WHERE id = $1").bind(lid).fetch_optional(&s.db).await?;
        let issue_id = issue_id.ok_or_else(|| unresolvable(&input.linked_branch_id))?;
        let (rc, row) = subject(ctx, &ID(ids::encode("Issue", issue_id)), &["Issue"]).await?;
        rc.require(crate::db::Perm::Triage).map_err(api_err)?;
        sqlx::query("DELETE FROM linked_branches WHERE id = $1").bind(lid).execute(&s.db).await?;
        Ok(Some(DeleteLinkedBranchPayload { client_mutation_id: input.client_mutation_id, issue: reload_issue(ctx, row.id).await? }))
    }
}
