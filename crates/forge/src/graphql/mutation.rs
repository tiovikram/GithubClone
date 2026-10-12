//! Mutation root: thin adapters from GitHub's mutation inputs to `services`.
use std::sync::Arc;

use async_graphql::{Context, InputObject, Object, SimpleObject, Union, ID};

use super::connection::cursor;
use super::enums::*;
use super::issue::{self, GComment, GIssue, IssueCommentEdge, IssueOrPullRequest};
use super::loaders;
use super::pull::{GPull, GReview};
use super::query::{resolve_node, Node};
use super::repo::{st, GLabel, GRepo};
use super::services::{self, IssuePatch, MergeInput, MergeMethod, NewIssue, NewPull, NewRepo, PullPatch, RepoPatch};
use super::{api_err, gql_err, not_found, GResult, Viewer};
use crate::db::{RepoCtx, User};
use crate::ids;
use crate::json::issue::IssueRow;

pub struct Mutation;

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

fn dec_all(ids: &Option<Vec<ID>>, kinds: &[&str]) -> async_graphql::Result<Option<Vec<i64>>> {
    match ids {
        Some(v) => Ok(Some(v.iter().map(|i| dec(i, kinds)).collect::<GResult<Vec<_>>>()?)),
        None => Ok(None),
    }
}

const USER_KINDS: &[&str] = &["User", "Bot", "Organization"];

async fn repo_ctx(ctx: &Context<'_>, repo_id: i64, id_for_error: &str) -> async_graphql::Result<Arc<RepoCtx>> {
    loaders::repo(ctx, repo_id).await?.ok_or_else(|| unresolvable(id_for_error))
}

/// Loads an Issue/PullRequest subject by node id with its repository context.
async fn subject(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<(Arc<RepoCtx>, IssueRow)> {
    let iid = dec(id, &["Issue", "PullRequest"])?;
    let row: Option<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(iid).fetch_optional(&st(ctx).db).await?;
    let row = row.ok_or_else(|| unresolvable(id))?;
    let repo = repo_ctx(ctx, row.repo_id, id).await?;
    Ok((repo, row))
}

async fn subject_pull(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<(Arc<RepoCtx>, IssueRow)> {
    let (repo, row) = subject(ctx, id).await?;
    if !row.is_pull {
        return Err(gql_err("NOT_FOUND", format!("Could not resolve to PullRequest node with the global id of '{}'.", id.as_str())));
    }
    Ok((repo, row))
}

async fn reload(ctx: &Context<'_>, issue_id: i64) -> async_graphql::Result<Option<IssueOrPullRequest>> {
    issue::by_id(ctx, issue_id).await
}

async fn reload_issue(ctx: &Context<'_>, issue_id: i64) -> async_graphql::Result<Option<GIssue>> {
    Ok(match reload(ctx, issue_id).await? {
        Some(IssueOrPullRequest::Issue(i)) => Some(i),
        _ => None,
    })
}

async fn reload_pull(ctx: &Context<'_>, issue_id: i64) -> async_graphql::Result<Option<GPull>> {
    Ok(match reload(ctx, issue_id).await? {
        Some(IssueOrPullRequest::PullRequest(p)) => Some(p),
        _ => None,
    })
}

// ---------------------------------------------------------------------------------------------
// Inputs

#[derive(InputObject)]
pub struct CreateRepositoryInput {
    pub name: String,
    pub owner_id: Option<ID>,
    pub description: Option<String>,
    pub visibility: RepositoryVisibility,
    pub template: Option<bool>,
    pub homepage_url: Option<Uri>,
    pub has_wiki_enabled: Option<bool>,
    pub has_issues_enabled: Option<bool>,
    pub team_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct CloneTemplateRepositoryInput {
    pub repository_id: ID,
    pub name: String,
    pub owner_id: ID,
    pub description: Option<String>,
    pub visibility: RepositoryVisibility,
    #[graphql(default = false)]
    pub include_all_branches: bool,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateRepositoryInput {
    pub repository_id: ID,
    pub name: Option<String>,
    pub description: Option<String>,
    pub template: Option<bool>,
    pub homepage_url: Option<Uri>,
    pub has_wiki_enabled: Option<bool>,
    pub has_issues_enabled: Option<bool>,
    pub has_projects_enabled: Option<bool>,
    pub has_discussions_enabled: Option<bool>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct CreateIssueInput {
    pub repository_id: ID,
    pub title: String,
    pub body: Option<String>,
    pub assignee_ids: Option<Vec<ID>>,
    pub milestone_id: Option<ID>,
    pub label_ids: Option<Vec<ID>>,
    pub project_ids: Option<Vec<ID>>,
    pub issue_template: Option<String>,
    pub issue_type_id: Option<ID>,
    pub parent_issue_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateIssueInput {
    pub id: ID,
    pub title: Option<String>,
    pub body: Option<String>,
    pub assignee_ids: Option<Vec<ID>>,
    /// `null` removes the milestone; absent leaves it unchanged.
    pub milestone_id: async_graphql::MaybeUndefined<ID>,
    pub label_ids: Option<Vec<ID>>,
    pub state: Option<IssueState>,
    pub project_ids: Option<Vec<ID>>,
    pub issue_type_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct CloseIssueInput {
    pub issue_id: ID,
    pub state_reason: Option<IssueClosedStateReason>,
    pub duplicate_issue_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct ReopenIssueInput {
    pub issue_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct DeleteIssueInput {
    pub issue_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct AddCommentInput {
    pub subject_id: ID,
    pub body: String,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateIssueCommentInput {
    pub id: ID,
    pub body: String,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct DeleteIssueCommentInput {
    pub id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct AddLabelsToLabelableInput {
    pub labelable_id: ID,
    pub label_ids: Vec<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct RemoveLabelsFromLabelableInput {
    pub labelable_id: ID,
    pub label_ids: Vec<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct ClearLabelsFromLabelableInput {
    pub labelable_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct AddAssigneesToAssignableInput {
    pub assignable_id: ID,
    pub assignee_ids: Vec<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct RemoveAssigneesFromAssignableInput {
    pub assignable_id: ID,
    pub assignee_ids: Vec<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct CreateLabelInput {
    pub repository_id: ID,
    pub name: String,
    pub color: String,
    pub description: Option<String>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateLabelInput {
    pub id: ID,
    pub name: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct DeleteLabelInput {
    pub id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct CreatePullRequestInput {
    pub repository_id: ID,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub head_repository_id: Option<ID>,
    pub title: String,
    pub body: Option<String>,
    #[graphql(default = true)]
    pub maintainer_can_modify: bool,
    #[graphql(default = false)]
    pub draft: bool,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdatePullRequestInput {
    pub pull_request_id: ID,
    pub base_ref_name: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<PullRequestUpdateState>,
    pub maintainer_can_modify: Option<bool>,
    pub assignee_ids: Option<Vec<ID>>,
    pub milestone_id: async_graphql::MaybeUndefined<ID>,
    pub label_ids: Option<Vec<ID>>,
    pub project_ids: Option<Vec<ID>>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct ClosePullRequestInput {
    pub pull_request_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct ReopenPullRequestInput {
    pub pull_request_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct MergePullRequestInput {
    pub pull_request_id: ID,
    pub commit_headline: Option<String>,
    pub commit_body: Option<String>,
    pub expected_head_oid: Option<GitObjectId>,
    pub merge_method: Option<PullRequestMergeMethod>,
    pub author_email: Option<String>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct EnablePullRequestAutoMergeInput {
    pub pull_request_id: ID,
    pub commit_headline: Option<String>,
    pub commit_body: Option<String>,
    pub expected_head_oid: Option<GitObjectId>,
    pub merge_method: Option<PullRequestMergeMethod>,
    pub author_email: Option<String>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct DisablePullRequestAutoMergeInput {
    pub pull_request_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct MarkPullRequestReadyForReviewInput {
    pub pull_request_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct ConvertPullRequestToDraftInput {
    pub pull_request_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct DraftPullRequestReviewComment {
    pub path: String,
    pub position: i32,
    pub body: String,
}

#[derive(InputObject)]
pub struct DraftPullRequestReviewThread {
    pub path: String,
    pub line: i32,
    pub side: Option<DiffSide>,
    pub start_line: Option<i32>,
    pub start_side: Option<DiffSide>,
    pub body: String,
}

#[derive(InputObject)]
pub struct AddPullRequestReviewInput {
    pub pull_request_id: ID,
    #[graphql(name = "commitOID")]
    pub commit_oid: Option<GitObjectId>,
    pub body: Option<String>,
    pub event: Option<PullRequestReviewEvent>,
    pub comments: Option<Vec<Option<DraftPullRequestReviewComment>>>,
    pub threads: Option<Vec<Option<DraftPullRequestReviewThread>>>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct RequestReviewsInput {
    pub pull_request_id: ID,
    pub user_ids: Option<Vec<ID>>,
    pub team_ids: Option<Vec<ID>>,
    #[graphql(default = false)]
    pub union: bool,
    pub client_mutation_id: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Payloads

#[derive(SimpleObject)]
pub struct CreateRepositoryPayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(SimpleObject)]
pub struct CloneTemplateRepositoryPayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(SimpleObject)]
pub struct UpdateRepositoryPayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(SimpleObject)]
pub struct CreateIssuePayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

#[derive(SimpleObject)]
pub struct UpdateIssuePayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

#[derive(SimpleObject)]
pub struct CloseIssuePayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

#[derive(SimpleObject)]
pub struct ReopenIssuePayload {
    pub client_mutation_id: Option<String>,
    pub issue: Option<GIssue>,
}

#[derive(SimpleObject)]
pub struct DeleteIssuePayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(SimpleObject)]
pub struct AddCommentPayload {
    pub client_mutation_id: Option<String>,
    pub comment_edge: Option<IssueCommentEdge>,
    pub subject: Option<Node>,
}

#[derive(SimpleObject)]
pub struct UpdateIssueCommentPayload {
    pub client_mutation_id: Option<String>,
    pub issue_comment: Option<GComment>,
}

#[derive(SimpleObject)]
pub struct DeleteIssueCommentPayload {
    pub client_mutation_id: Option<String>,
}

/// GitHub's `Labelable` / `Assignable` are interfaces; issues and PRs are the implementors here.
#[derive(Union, Clone)]
pub enum Labelable {
    Issue(GIssue),
    PullRequest(GPull),
}

#[derive(Union, Clone)]
pub enum Assignable {
    Issue(GIssue),
    PullRequest(GPull),
}

fn as_labelable(x: Option<IssueOrPullRequest>) -> Option<Labelable> {
    x.map(|x| match x {
        IssueOrPullRequest::Issue(i) => Labelable::Issue(i),
        IssueOrPullRequest::PullRequest(p) => Labelable::PullRequest(p),
    })
}

fn as_assignable(x: Option<IssueOrPullRequest>) -> Option<Assignable> {
    x.map(|x| match x {
        IssueOrPullRequest::Issue(i) => Assignable::Issue(i),
        IssueOrPullRequest::PullRequest(p) => Assignable::PullRequest(p),
    })
}

#[derive(SimpleObject)]
pub struct AddLabelsToLabelablePayload {
    pub client_mutation_id: Option<String>,
    pub labelable: Option<Labelable>,
}

#[derive(SimpleObject)]
pub struct RemoveLabelsFromLabelablePayload {
    pub client_mutation_id: Option<String>,
    pub labelable: Option<Labelable>,
}

#[derive(SimpleObject)]
pub struct ClearLabelsFromLabelablePayload {
    pub client_mutation_id: Option<String>,
    pub labelable: Option<Labelable>,
}

#[derive(SimpleObject)]
pub struct AddAssigneesToAssignablePayload {
    pub client_mutation_id: Option<String>,
    pub assignable: Option<Assignable>,
}

#[derive(SimpleObject)]
pub struct RemoveAssigneesFromAssignablePayload {
    pub client_mutation_id: Option<String>,
    pub assignable: Option<Assignable>,
}

#[derive(SimpleObject)]
pub struct CreateLabelPayload {
    pub client_mutation_id: Option<String>,
    pub label: Option<GLabel>,
}

#[derive(SimpleObject)]
pub struct UpdateLabelPayload {
    pub client_mutation_id: Option<String>,
    pub label: Option<GLabel>,
}

#[derive(SimpleObject)]
pub struct DeleteLabelPayload {
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct CreatePullRequestPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct UpdatePullRequestPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct ClosePullRequestPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct ReopenPullRequestPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct MergePullRequestPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct EnablePullRequestAutoMergePayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct DisablePullRequestAutoMergePayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct MarkPullRequestReadyForReviewPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct ConvertPullRequestToDraftPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(SimpleObject)]
pub struct AddPullRequestReviewPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request_review: Option<GReview>,
}

#[derive(SimpleObject)]
pub struct RequestReviewsPayload {
    pub client_mutation_id: Option<String>,
    pub pull_request: Option<GPull>,
}

#[derive(InputObject)]
pub struct ReactionInput {
    pub subject_id: ID,
    pub content: ReactionContent,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct Reaction {
    pub content: ReactionContent,
}

#[derive(SimpleObject)]
pub struct ReactionPayload {
    pub client_mutation_id: Option<String>,
    pub reaction: Option<Reaction>,
    pub subject: Option<Node>,
}

fn reaction_key(c: ReactionContent) -> &'static str {
    ReactionContent::ALL.iter().find(|(x, _)| *x == c).map(|(_, k)| *k).unwrap_or("+1")
}

/// (reactions.subject_type, subject id) for an Issue/PullRequest/IssueComment node the viewer can see.
async fn reaction_subject(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<(&'static str, i64)> {
    match ids::decode(id).map(|(k, _)| k).as_deref() {
        Some("IssueComment") => match resolve_node(ctx, id).await? {
            Some(Node::IssueComment(c)) => Ok(("issue_comment", c.row.id)),
            _ => Err(unresolvable(id)),
        },
        _ => {
            let (_, row) = subject(ctx, id).await?;
            Ok(("issue", row.id))
        }
    }
}

fn visibility_str(v: RepositoryVisibility) -> &'static str {
    match v {
        RepositoryVisibility::Public => "public",
        RepositoryVisibility::Private => "private",
        RepositoryVisibility::Internal => "internal",
    }
}

#[Object(name = "MutationCore")] // merged into the `Mutation` root (graphql::MutationRoot)
impl Mutation {
    async fn create_repository(&self, ctx: &Context<'_>, input: CreateRepositoryInput) -> async_graphql::Result<Option<CreateRepositoryPayload>> {
        let s = st(ctx);
        let me = actor(ctx);
        let owner = match &input.owner_id {
            Some(id) => {
                let oid = dec(id, USER_KINDS)?;
                crate::db::user_by_id(&s.db, oid).await.map_err(|_| unresolvable(id))?
            }
            None => me.clone(),
        };
        if input.visibility == RepositoryVisibility::Internal && owner.kind != "Organization" {
            return Err(gql_err("UNPROCESSABLE", "Internal repositories can only be created within an organization."));
        }
        let rc = services::create_repository(
            s,
            &owner,
            me,
            NewRepo {
                name: input.name,
                description: input.description,
                homepage: input.homepage_url.map(|u| u.0),
                visibility: visibility_str(input.visibility).into(),
                has_issues: input.has_issues_enabled,
                has_wiki: input.has_wiki_enabled,
                is_template: input.template,
            },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(CreateRepositoryPayload { client_mutation_id: input.client_mutation_id, repository: Some(GRepo::new(Arc::new(rc))) }))
    }

    async fn clone_template_repository(&self, ctx: &Context<'_>, input: CloneTemplateRepositoryInput) -> async_graphql::Result<Option<CloneTemplateRepositoryPayload>> {
        let s = st(ctx);
        let me = actor(ctx);
        let tid = dec(&input.repository_id, &["Repository"])?;
        let template = repo_ctx(ctx, tid, &input.repository_id).await?;
        let oid = dec(&input.owner_id, USER_KINDS)?;
        let owner = crate::db::user_by_id(&s.db, oid).await.map_err(|_| unresolvable(&input.owner_id))?;
        let rc = services::create_repository(
            s,
            &owner,
            me,
            NewRepo {
                name: input.name,
                description: input.description.or(template.repo.description.clone()),
                visibility: visibility_str(input.visibility).into(),
                ..Default::default()
            },
        )
        .await
        .map_err(api_err)?;
        // Copy the template's default branch (or all branches) into the new repository.
        let src = s.git.sync(template.repo.id).await.map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let git = s.git.git.clone();
        let refspec = if input.include_all_branches {
            "+refs/heads/*:refs/heads/*".to_string()
        } else {
            format!("+refs/heads/{0}:refs/heads/{0}", template.repo.default_branch)
        };
        s.git
            .write(rc.repo.id, |path| async move {
                git.run(&path, &["fetch", "--no-tags", "-q", src.to_str().unwrap(), &refspec]).await?;
                Ok(())
            })
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        if template.repo.default_branch != rc.repo.default_branch {
            let _ = crate::events::set_default_branch(s, rc.repo.id, &template.repo.default_branch).await;
        }
        let rc = loaders::repo(ctx, rc.repo.id).await?;
        Ok(Some(CloneTemplateRepositoryPayload { client_mutation_id: input.client_mutation_id, repository: rc.map(GRepo::new) }))
    }

    async fn update_repository(&self, ctx: &Context<'_>, input: UpdateRepositoryInput) -> async_graphql::Result<Option<UpdateRepositoryPayload>> {
        let rid = dec(&input.repository_id, &["Repository"])?;
        let rc = repo_ctx(ctx, rid, &input.repository_id).await?;
        let updated = services::update_repository(
            st(ctx),
            &rc,
            actor(ctx),
            RepoPatch {
                name: input.name,
                description: input.description,
                homepage: input.homepage_url.map(|u| u.0),
                has_issues: input.has_issues_enabled,
                has_wiki: input.has_wiki_enabled,
                has_projects: input.has_projects_enabled,
                has_discussions: input.has_discussions_enabled,
                is_template: input.template,
            },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(UpdateRepositoryPayload { client_mutation_id: input.client_mutation_id, repository: Some(GRepo::new(Arc::new(updated))) }))
    }

    async fn create_issue(&self, ctx: &Context<'_>, input: CreateIssueInput) -> async_graphql::Result<Option<CreateIssuePayload>> {
        let rid = dec(&input.repository_id, &["Repository"])?;
        let rc = repo_ctx(ctx, rid, &input.repository_id).await?;
        let row = services::create_issue(
            st(ctx),
            &rc,
            actor(ctx),
            NewIssue {
                title: input.title,
                body: input.body,
                labels: dec_all(&input.label_ids, &["Label"])?.unwrap_or_default(),
                assignees: dec_all(&input.assignee_ids, USER_KINDS)?.unwrap_or_default(),
                milestone: input.milestone_id.as_ref().map(|m| dec(m, &["Milestone"])).transpose()?,
            },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(CreateIssuePayload { client_mutation_id: input.client_mutation_id, issue: reload_issue(ctx, row.id).await? }))
    }

    async fn update_issue(&self, ctx: &Context<'_>, input: UpdateIssueInput) -> async_graphql::Result<Option<UpdateIssuePayload>> {
        let (rc, row) = subject(ctx, &input.id).await?;
        if row.is_pull {
            return Err(unresolvable(&input.id));
        }
        let milestone = match &input.milestone_id {
            async_graphql::MaybeUndefined::Undefined => None,
            async_graphql::MaybeUndefined::Null => Some(None),
            async_graphql::MaybeUndefined::Value(m) => Some(Some(dec(m, &["Milestone"])?)),
        };
        services::update_issue(
            st(ctx),
            &rc,
            actor(ctx),
            &row,
            IssuePatch {
                title: input.title,
                body: input.body,
                state: input.state.map(|s| if s == IssueState::Closed { "closed".into() } else { "open".into() }),
                state_reason: None,
                milestone,
                assignees: dec_all(&input.assignee_ids, USER_KINDS)?,
                labels: dec_all(&input.label_ids, &["Label"])?,
            },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(UpdateIssuePayload { client_mutation_id: input.client_mutation_id, issue: reload_issue(ctx, row.id).await? }))
    }

    async fn close_issue(&self, ctx: &Context<'_>, input: CloseIssueInput) -> async_graphql::Result<Option<CloseIssuePayload>> {
        let (rc, row) = subject(ctx, &input.issue_id).await?;
        if row.is_pull {
            return Err(unresolvable(&input.issue_id));
        }
        let reason = match input.state_reason {
            Some(IssueClosedStateReason::NotPlanned) => "not_planned",
            Some(IssueClosedStateReason::Duplicate) => "duplicate",
            _ if input.duplicate_issue_id.is_some() => "duplicate",
            _ => "completed",
        };
        services::update_issue(
            st(ctx),
            &rc,
            actor(ctx),
            &row,
            IssuePatch { state: Some("closed".into()), state_reason: Some(reason.into()), ..Default::default() },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(CloseIssuePayload { client_mutation_id: input.client_mutation_id, issue: reload_issue(ctx, row.id).await? }))
    }

    async fn reopen_issue(&self, ctx: &Context<'_>, input: ReopenIssueInput) -> async_graphql::Result<Option<ReopenIssuePayload>> {
        let (rc, row) = subject(ctx, &input.issue_id).await?;
        if row.is_pull {
            return Err(unresolvable(&input.issue_id));
        }
        services::update_issue(st(ctx), &rc, actor(ctx), &row, IssuePatch { state: Some("open".into()), ..Default::default() })
            .await
            .map_err(api_err)?;
        Ok(Some(ReopenIssuePayload { client_mutation_id: input.client_mutation_id, issue: reload_issue(ctx, row.id).await? }))
    }

    async fn delete_issue(&self, ctx: &Context<'_>, input: DeleteIssueInput) -> async_graphql::Result<Option<DeleteIssuePayload>> {
        let (rc, row) = subject(ctx, &input.issue_id).await?;
        services::delete_issue(st(ctx), &rc, actor(ctx), &row).await.map_err(api_err)?;
        Ok(Some(DeleteIssuePayload { client_mutation_id: input.client_mutation_id, repository: Some(GRepo::new(rc)) }))
    }

    async fn add_comment(&self, ctx: &Context<'_>, input: AddCommentInput) -> async_graphql::Result<Option<AddCommentPayload>> {
        let (rc, row) = subject(ctx, &input.subject_id).await?;
        let c = services::create_comment(st(ctx), &rc, actor(ctx), &row, &input.body).await.map_err(api_err)?;
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_comments WHERE issue_id = $1").bind(row.id).fetch_one(&st(ctx).db).await?;
        let node = GComment { row: Arc::new(c), repo: rc.clone(), number: row.number, is_pull: row.is_pull };
        let subject = resolve_node(ctx, &input.subject_id).await?;
        Ok(Some(AddCommentPayload {
            client_mutation_id: input.client_mutation_id,
            comment_edge: Some(IssueCommentEdge { cursor: cursor((total.max(1) - 1) as usize), node }),
            subject,
        }))
    }

    async fn update_issue_comment(&self, ctx: &Context<'_>, input: UpdateIssueCommentInput) -> async_graphql::Result<Option<UpdateIssueCommentPayload>> {
        let cid = dec(&input.id, &["IssueComment"])?;
        let s = st(ctx);
        let repo_id: i64 = sqlx::query_scalar("SELECT repo_id FROM issue_comments WHERE id = $1").bind(cid).fetch_optional(&s.db).await?.ok_or_else(|| unresolvable(&input.id))?;
        let rc = repo_ctx(ctx, repo_id, &input.id).await?;
        let c = services::comment_by_id(s, &rc, cid).await.map_err(|_| unresolvable(&input.id))?;
        let parent: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(c.issue_id).fetch_one(&s.db).await?;
        let c = services::update_comment(s, &rc, actor(ctx), &c, &input.body).await.map_err(api_err)?;
        Ok(Some(UpdateIssueCommentPayload {
            client_mutation_id: input.client_mutation_id,
            issue_comment: Some(GComment { row: Arc::new(c), repo: rc, number: parent.number, is_pull: parent.is_pull }),
        }))
    }

    async fn delete_issue_comment(&self, ctx: &Context<'_>, input: DeleteIssueCommentInput) -> async_graphql::Result<Option<DeleteIssueCommentPayload>> {
        let cid = dec(&input.id, &["IssueComment"])?;
        let s = st(ctx);
        let repo_id: i64 = sqlx::query_scalar("SELECT repo_id FROM issue_comments WHERE id = $1").bind(cid).fetch_optional(&s.db).await?.ok_or_else(|| unresolvable(&input.id))?;
        let rc = repo_ctx(ctx, repo_id, &input.id).await?;
        let c = services::comment_by_id(s, &rc, cid).await.map_err(|_| unresolvable(&input.id))?;
        services::delete_comment(s, &rc, actor(ctx), &c).await.map_err(api_err)?;
        Ok(Some(DeleteIssueCommentPayload { client_mutation_id: input.client_mutation_id }))
    }

    async fn add_labels_to_labelable(&self, ctx: &Context<'_>, input: AddLabelsToLabelableInput) -> async_graphql::Result<Option<AddLabelsToLabelablePayload>> {
        let (rc, row) = subject(ctx, &input.labelable_id).await?;
        let labels = dec_all(&Some(input.label_ids), &["Label"])?.unwrap_or_default();
        services::add_labels(st(ctx), &rc, actor(ctx), &row, &labels).await.map_err(api_err)?;
        Ok(Some(AddLabelsToLabelablePayload { client_mutation_id: input.client_mutation_id, labelable: as_labelable(reload(ctx, row.id).await?) }))
    }

    async fn remove_labels_from_labelable(&self, ctx: &Context<'_>, input: RemoveLabelsFromLabelableInput) -> async_graphql::Result<Option<RemoveLabelsFromLabelablePayload>> {
        let (rc, row) = subject(ctx, &input.labelable_id).await?;
        let labels = dec_all(&Some(input.label_ids), &["Label"])?.unwrap_or_default();
        services::remove_labels(st(ctx), &rc, actor(ctx), &row, &labels).await.map_err(api_err)?;
        Ok(Some(RemoveLabelsFromLabelablePayload { client_mutation_id: input.client_mutation_id, labelable: as_labelable(reload(ctx, row.id).await?) }))
    }

    async fn clear_labels_from_labelable(&self, ctx: &Context<'_>, input: ClearLabelsFromLabelableInput) -> async_graphql::Result<Option<ClearLabelsFromLabelablePayload>> {
        let (rc, row) = subject(ctx, &input.labelable_id).await?;
        services::clear_labels(st(ctx), &rc, actor(ctx), &row).await.map_err(api_err)?;
        Ok(Some(ClearLabelsFromLabelablePayload { client_mutation_id: input.client_mutation_id, labelable: as_labelable(reload(ctx, row.id).await?) }))
    }

    async fn add_assignees_to_assignable(&self, ctx: &Context<'_>, input: AddAssigneesToAssignableInput) -> async_graphql::Result<Option<AddAssigneesToAssignablePayload>> {
        let (rc, row) = subject(ctx, &input.assignable_id).await?;
        let users = dec_all(&Some(input.assignee_ids), USER_KINDS)?.unwrap_or_default();
        services::add_assignees(st(ctx), &rc, actor(ctx), &row, &users).await.map_err(api_err)?;
        Ok(Some(AddAssigneesToAssignablePayload { client_mutation_id: input.client_mutation_id, assignable: as_assignable(reload(ctx, row.id).await?) }))
    }

    async fn remove_assignees_from_assignable(
        &self,
        ctx: &Context<'_>,
        input: RemoveAssigneesFromAssignableInput,
    ) -> async_graphql::Result<Option<RemoveAssigneesFromAssignablePayload>> {
        let (rc, row) = subject(ctx, &input.assignable_id).await?;
        let users = dec_all(&Some(input.assignee_ids), USER_KINDS)?.unwrap_or_default();
        services::remove_assignees(st(ctx), &rc, actor(ctx), &row, &users).await.map_err(api_err)?;
        Ok(Some(RemoveAssigneesFromAssignablePayload { client_mutation_id: input.client_mutation_id, assignable: as_assignable(reload(ctx, row.id).await?) }))
    }

    async fn create_label(&self, ctx: &Context<'_>, input: CreateLabelInput) -> async_graphql::Result<Option<CreateLabelPayload>> {
        let rid = dec(&input.repository_id, &["Repository"])?;
        let rc = repo_ctx(ctx, rid, &input.repository_id).await?;
        let l = services::create_label(st(ctx), &rc, actor(ctx), &input.name, &input.color, input.description.as_deref())
            .await
            .map_err(api_err)?;
        Ok(Some(CreateLabelPayload { client_mutation_id: input.client_mutation_id, label: Some(GLabel::new(l, &rc)) }))
    }

    async fn update_label(&self, ctx: &Context<'_>, input: UpdateLabelInput) -> async_graphql::Result<Option<UpdateLabelPayload>> {
        let lid = dec(&input.id, &["Label"])?;
        let repo_id: Option<i64> = sqlx::query_scalar("SELECT repo_id FROM labels WHERE id = $1").bind(lid).fetch_optional(&st(ctx).db).await?;
        let rc = repo_ctx(ctx, repo_id.ok_or_else(|| unresolvable(&input.id))?, &input.id).await?;
        let l = services::update_label(st(ctx), &rc, actor(ctx), lid, input.name.as_deref(), input.color.as_deref(), input.description.as_deref())
            .await
            .map_err(api_err)?;
        Ok(Some(UpdateLabelPayload { client_mutation_id: input.client_mutation_id, label: Some(GLabel::new(l, &rc)) }))
    }

    async fn delete_label(&self, ctx: &Context<'_>, input: DeleteLabelInput) -> async_graphql::Result<Option<DeleteLabelPayload>> {
        let lid = dec(&input.id, &["Label"])?;
        let repo_id: Option<i64> = sqlx::query_scalar("SELECT repo_id FROM labels WHERE id = $1").bind(lid).fetch_optional(&st(ctx).db).await?;
        let rc = repo_ctx(ctx, repo_id.ok_or_else(|| unresolvable(&input.id))?, &input.id).await?;
        services::delete_label(st(ctx), &rc, actor(ctx), lid).await.map_err(api_err)?;
        Ok(Some(DeleteLabelPayload { client_mutation_id: input.client_mutation_id }))
    }

    async fn create_pull_request(&self, ctx: &Context<'_>, input: CreatePullRequestInput) -> async_graphql::Result<Option<CreatePullRequestPayload>> {
        let rid = dec(&input.repository_id, &["Repository"])?;
        let rc = repo_ctx(ctx, rid, &input.repository_id).await?;
        let head_repo_id = input.head_repository_id.as_ref().map(|h| dec(h, &["Repository"])).transpose()?;
        let row = services::create_pull(
            st(ctx),
            &rc,
            actor(ctx),
            NewPull {
                title: input.title,
                body: input.body,
                head: input.head_ref_name,
                base: input.base_ref_name,
                draft: input.draft,
                maintainer_can_modify: input.maintainer_can_modify,
                head_repo_id,
            },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(CreatePullRequestPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn update_pull_request(&self, ctx: &Context<'_>, input: UpdatePullRequestInput) -> async_graphql::Result<Option<UpdatePullRequestPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        let s = st(ctx);
        let me = actor(ctx);
        services::update_pull(
            s,
            &rc,
            me,
            &row,
            PullPatch {
                title: input.title,
                body: input.body,
                base: input.base_ref_name,
                state: input.state.map(|x| if x == PullRequestUpdateState::Closed { "closed".into() } else { "open".into() }),
                draft: None,
                maintainer_can_modify: input.maintainer_can_modify,
            },
        )
        .await
        .map_err(api_err)?;
        let milestone = match &input.milestone_id {
            async_graphql::MaybeUndefined::Undefined => None,
            async_graphql::MaybeUndefined::Null => Some(None),
            async_graphql::MaybeUndefined::Value(m) => Some(Some(dec(m, &["Milestone"])?)),
        };
        let labels = dec_all(&input.label_ids, &["Label"])?;
        let assignees = dec_all(&input.assignee_ids, USER_KINDS)?;
        if milestone.is_some() || labels.is_some() || assignees.is_some() {
            let fresh: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(row.id).fetch_one(&s.db).await?;
            services::update_issue(s, &rc, me, &fresh, IssuePatch { milestone, labels, assignees, ..Default::default() })
                .await
                .map_err(api_err)?;
        }
        Ok(Some(UpdatePullRequestPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn close_pull_request(&self, ctx: &Context<'_>, input: ClosePullRequestInput) -> async_graphql::Result<Option<ClosePullRequestPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        services::update_pull(st(ctx), &rc, actor(ctx), &row, PullPatch { state: Some("closed".into()), ..Default::default() })
            .await
            .map_err(api_err)?;
        Ok(Some(ClosePullRequestPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn reopen_pull_request(&self, ctx: &Context<'_>, input: ReopenPullRequestInput) -> async_graphql::Result<Option<ReopenPullRequestPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        services::update_pull(st(ctx), &rc, actor(ctx), &row, PullPatch { state: Some("open".into()), ..Default::default() })
            .await
            .map_err(api_err)?;
        Ok(Some(ReopenPullRequestPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn merge_pull_request(&self, ctx: &Context<'_>, input: MergePullRequestInput) -> async_graphql::Result<Option<MergePullRequestPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        let method = match input.merge_method.unwrap_or(PullRequestMergeMethod::Merge) {
            PullRequestMergeMethod::Merge => MergeMethod::Merge,
            PullRequestMergeMethod::Squash => MergeMethod::Squash,
            PullRequestMergeMethod::Rebase => MergeMethod::Rebase,
        };
        services::merge_pull(
            st(ctx),
            &rc,
            actor(ctx),
            &row,
            MergeInput {
                method,
                title: input.commit_headline,
                message: input.commit_body,
                sha: input.expected_head_oid.map(|o| o.0).filter(|s| !s.is_empty()),
            },
        )
        .await
        .map_err(api_err)?;
        Ok(Some(MergePullRequestPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn enable_pull_request_auto_merge(&self, ctx: &Context<'_>, input: EnablePullRequestAutoMergeInput) -> async_graphql::Result<Option<EnablePullRequestAutoMergePayload>> {
        let _ = subject_pull(ctx, &input.pull_request_id).await?;
        Err(gql_err("UNPROCESSABLE", "Auto merge is not allowed for this repository"))
    }

    async fn disable_pull_request_auto_merge(&self, ctx: &Context<'_>, input: DisablePullRequestAutoMergeInput) -> async_graphql::Result<Option<DisablePullRequestAutoMergePayload>> {
        let (_, row) = subject_pull(ctx, &input.pull_request_id).await?;
        Ok(Some(DisablePullRequestAutoMergePayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn mark_pull_request_ready_for_review(
        &self,
        ctx: &Context<'_>,
        input: MarkPullRequestReadyForReviewInput,
    ) -> async_graphql::Result<Option<MarkPullRequestReadyForReviewPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        services::update_pull(st(ctx), &rc, actor(ctx), &row, PullPatch { draft: Some(false), ..Default::default() })
            .await
            .map_err(api_err)?;
        Ok(Some(MarkPullRequestReadyForReviewPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn convert_pull_request_to_draft(&self, ctx: &Context<'_>, input: ConvertPullRequestToDraftInput) -> async_graphql::Result<Option<ConvertPullRequestToDraftPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        services::update_pull(st(ctx), &rc, actor(ctx), &row, PullPatch { draft: Some(true), ..Default::default() })
            .await
            .map_err(api_err)?;
        Ok(Some(ConvertPullRequestToDraftPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }

    async fn add_pull_request_review(&self, ctx: &Context<'_>, input: AddPullRequestReviewInput) -> async_graphql::Result<Option<AddPullRequestReviewPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        let event = input.event.map(|e| match e {
            PullRequestReviewEvent::Approve => "APPROVE",
            PullRequestReviewEvent::RequestChanges => "REQUEST_CHANGES",
            PullRequestReviewEvent::Comment => "COMMENT",
            PullRequestReviewEvent::Dismiss => "DISMISS",
        });
        let r = services::create_review(st(ctx), &rc, actor(ctx), &row, event, input.body.as_deref().unwrap_or(""), input.commit_oid.map(|c| c.0))
            .await
            .map_err(api_err)?;
        Ok(Some(AddPullRequestReviewPayload {
            client_mutation_id: input.client_mutation_id,
            pull_request_review: Some(GReview { row: Arc::new(r), repo: rc, number: row.number }),
        }))
    }

    async fn add_reaction(&self, ctx: &Context<'_>, input: ReactionInput) -> async_graphql::Result<Option<ReactionPayload>> {
        let (kind, sid) = reaction_subject(ctx, &input.subject_id).await?;
        let content = reaction_key(input.content);
        crate::api::reactions::create_reaction(st(ctx), kind, sid, actor(ctx), content).await.map_err(api_err)?;
        let subject = resolve_node(ctx, &input.subject_id).await?;
        Ok(Some(ReactionPayload { client_mutation_id: input.client_mutation_id, reaction: Some(Reaction { content: input.content }), subject }))
    }

    async fn remove_reaction(&self, ctx: &Context<'_>, input: ReactionInput) -> async_graphql::Result<Option<ReactionPayload>> {
        let (kind, sid) = reaction_subject(ctx, &input.subject_id).await?;
        sqlx::query("DELETE FROM reactions WHERE subject_type = $1 AND subject_id = $2 AND user_id = $3 AND content = $4")
            .bind(kind)
            .bind(sid)
            .bind(actor(ctx).id)
            .bind(reaction_key(input.content))
            .execute(&st(ctx).db)
            .await?;
        let subject = resolve_node(ctx, &input.subject_id).await?;
        Ok(Some(ReactionPayload { client_mutation_id: input.client_mutation_id, reaction: Some(Reaction { content: input.content }), subject }))
    }

    async fn request_reviews(&self, ctx: &Context<'_>, input: RequestReviewsInput) -> async_graphql::Result<Option<RequestReviewsPayload>> {
        let (rc, row) = subject_pull(ctx, &input.pull_request_id).await?;
        if input.team_ids.as_ref().is_some_and(|t| !t.is_empty()) {
            return Err(gql_err("NOT_FOUND", "Could not resolve to a Team: teams are not supported."));
        }
        let users = dec_all(&input.user_ids, USER_KINDS)?.unwrap_or_default();
        services::request_reviewers(st(ctx), &rc, actor(ctx), &row, &users, input.union).await.map_err(api_err)?;
        Ok(Some(RequestReviewsPayload { client_mutation_id: input.client_mutation_id, pull_request: reload_pull(ctx, row.id).await? }))
    }
}
