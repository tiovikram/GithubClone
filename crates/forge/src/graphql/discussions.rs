//! Discussions in GraphQL (`gh discussion list|view|create|edit|comment`): the Discussion,
//! DiscussionCategory and DiscussionComment objects, Repository.discussions/discussion/
//! discussionCategories, and the discussion mutations. Adapters over `api::discussions`.
use std::sync::Arc;

use async_graphql::{Context, Enum, InputObject, Object, SimpleObject, ID};

use super::connection::ConnArgs;
use super::enums::{dt, DateTime, OrderDirection, Uri};
use super::issue::{reaction_groups, ReactionGroup};
use super::loaders;
use super::repo::{st, GLabel, GRepo, LabelConnection};
use super::user::Actor;
use super::{api_err, not_found, Viewer};
use crate::api::discussions::{self as svc, CategoryRow, DiscussionCommentRow, DiscussionPatch, DiscussionRow, ListFilter};
use crate::db::{RepoCtx, User};
use crate::ids;

fn actor<'a>(ctx: &'a Context<'_>) -> &'a User {
    &ctx.data_unchecked::<Viewer>().0
}

fn unresolvable(id: &str) -> async_graphql::Error {
    not_found(format!("Could not resolve to a node with the global id of '{id}'."))
}

fn dec(id: &ID, kind: &str) -> async_graphql::Result<i64> {
    match ids::decode(id) {
        Some((k, v)) if k == kind => v.parse().map_err(|_| unresolvable(id)),
        _ => Err(unresolvable(id)),
    }
}

fn html_url(repo: &RepoCtx, ctx: &Context<'_>, number: i64) -> String {
    format!("{}/{}/discussions/{number}", st(ctx).base_url(), repo.full_name())
}

// ---------------------------------------------------------------------------------------------
// Objects

#[derive(Clone)]
pub struct GDiscussion {
    pub row: Arc<DiscussionRow>,
    pub repo: Arc<RepoCtx>,
}

#[derive(Clone)]
pub struct GDiscussionCategory {
    pub row: Arc<CategoryRow>,
    pub repo: Arc<RepoCtx>,
}

#[derive(Clone)]
pub struct GDiscussionComment {
    pub row: Arc<DiscussionCommentRow>,
    pub repo: Arc<RepoCtx>,
}

crate::gql_connection!(DiscussionConnection, DiscussionEdge, "DiscussionConnection", "DiscussionEdge", GDiscussion);
crate::gql_connection!(
    DiscussionCategoryConnection,
    DiscussionCategoryEdge,
    "DiscussionCategoryConnection",
    "DiscussionCategoryEdge",
    GDiscussionCategory
);
crate::gql_connection!(
    DiscussionCommentConnection,
    DiscussionCommentEdge,
    "DiscussionCommentConnection",
    "DiscussionCommentEdge",
    GDiscussionComment
);

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DiscussionState {
    Open,
    Closed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DiscussionStateReason {
    Duplicate,
    Outdated,
    Reopened,
    Resolved,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DiscussionCloseReason {
    Duplicate,
    Outdated,
    Resolved,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DiscussionOrderField {
    CreatedAt,
    UpdatedAt,
}

#[derive(InputObject)]
pub struct DiscussionOrder {
    pub field: DiscussionOrderField,
    pub direction: OrderDirection,
}

async fn category(ctx: &Context<'_>, repo: &Arc<RepoCtx>, id: i64) -> async_graphql::Result<GDiscussionCategory> {
    let row = svc::category_by_id(st(ctx), id).await.map_err(api_err)?;
    Ok(GDiscussionCategory { row: Arc::new(row), repo: repo.clone() })
}

#[Object(name = "Discussion")]
impl GDiscussion {
    pub async fn id(&self) -> ID {
        ID(ids::encode("Discussion", self.row.id))
    }
    async fn database_id(&self) -> i64 {
        self.row.id
    }
    async fn number(&self) -> i64 {
        self.row.number
    }
    async fn title(&self) -> &str {
        &self.row.title
    }
    async fn body(&self) -> &str {
        &self.row.body
    }
    async fn body_text(&self) -> &str {
        &self.row.body
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(html_url(&self.repo, ctx, self.row.number))
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/discussions/{}", self.repo.full_name(), self.row.number))
    }
    async fn closed(&self) -> bool {
        self.row.closed
    }
    async fn state_reason(&self) -> Option<DiscussionStateReason> {
        Some(match self.row.state_reason.as_deref()? {
            "duplicate" => DiscussionStateReason::Duplicate,
            "outdated" => DiscussionStateReason::Outdated,
            "reopened" => DiscussionStateReason::Reopened,
            _ => DiscussionStateReason::Resolved,
        })
    }
    async fn locked(&self) -> bool {
        self.row.locked
    }
    async fn author(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Actor>> {
        Ok(loaders::user(ctx, self.row.user_id).await?.map(Actor::from_user))
    }
    async fn category(&self, ctx: &Context<'_>) -> async_graphql::Result<GDiscussionCategory> {
        category(ctx, &self.repo, self.row.category_id).await
    }
    async fn labels(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> async_graphql::Result<Option<LabelConnection>> {
        let rows = svc::labels(st(ctx), self.row.id).await.map_err(api_err)?;
        let all = rows.into_iter().map(|l| GLabel::new(l, &self.repo)).collect();
        Ok(Some(LabelConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn is_answered(&self) -> bool {
        self.row.answer_comment_id.is_some()
    }
    async fn answer(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GDiscussionComment>> {
        let Some(id) = self.row.answer_comment_id else { return Ok(None) };
        Ok(svc::comment_by_id(st(ctx), id).await.map_err(api_err)?.map(|c| GDiscussionComment { row: Arc::new(c), repo: self.repo.clone() }))
    }
    async fn answer_chosen_at(&self) -> Option<DateTime> {
        self.row.answer_chosen_at.map(dt)
    }
    async fn answer_chosen_by(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Actor>> {
        Ok(loaders::user(ctx, self.row.answer_chosen_by).await?.map(Actor::from_user))
    }
    async fn reaction_groups(&self, ctx: &Context<'_>) -> Option<Vec<ReactionGroup>> {
        Some(reaction_groups(ctx, None).await)
    }
    async fn upvote_count(&self) -> i32 {
        0
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn closed_at(&self) -> Option<DateTime> {
        self.row.closed_at.map(dt)
    }
    async fn comments(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> async_graphql::Result<DiscussionCommentConnection> {
        let rows = svc::comments(st(ctx), self.row.id, None).await.map_err(api_err)?;
        let all = rows.into_iter().map(|c| GDiscussionComment { row: Arc::new(c), repo: self.repo.clone() }).collect();
        Ok(DiscussionCommentConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> bool {
        self.repo.perm >= crate::db::Perm::Triage || self.row.user_id == Some(actor(ctx).id)
    }
}

#[Object(name = "DiscussionCategory")]
impl GDiscussionCategory {
    async fn id(&self) -> ID {
        ID(ids::encode("DiscussionCategory", self.row.id))
    }
    async fn name(&self) -> &str {
        &self.row.name
    }
    async fn slug(&self) -> &str {
        &self.row.slug
    }
    async fn emoji(&self) -> &str {
        &self.row.emoji
    }
    async fn emoji_html(&self) -> String {
        format!("<div>{}</div>", self.row.emoji)
    }
    async fn description(&self) -> Option<&str> {
        Some(self.row.description.as_str()).filter(|d| !d.is_empty())
    }
    async fn is_answerable(&self) -> bool {
        self.row.is_answerable
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
}

#[Object(name = "DiscussionComment")]
impl GDiscussionComment {
    pub async fn id(&self) -> ID {
        ID(ids::encode("DiscussionComment", self.row.id))
    }
    async fn database_id(&self) -> i64 {
        self.row.id
    }
    async fn url(&self, ctx: &Context<'_>) -> async_graphql::Result<Uri> {
        let d = svc::by_id(st(ctx), self.row.discussion_id).await.map_err(api_err)?.ok_or_else(|| not_found("discussion"))?;
        Ok(Uri(format!("{}#discussioncomment-{}", html_url(&self.repo, ctx, d.number), self.row.id)))
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
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn is_answer(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let d = svc::by_id(st(ctx), self.row.discussion_id).await.map_err(api_err)?;
        Ok(d.is_some_and(|d| d.answer_comment_id == Some(self.row.id)))
    }
    async fn upvote_count(&self) -> i32 {
        0
    }
    async fn reaction_groups(&self, ctx: &Context<'_>) -> Option<Vec<ReactionGroup>> {
        Some(reaction_groups(ctx, None).await)
    }
    async fn reply_to(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GDiscussionComment>> {
        let Some(p) = self.row.parent_id else { return Ok(None) };
        Ok(svc::comment_by_id(st(ctx), p).await.map_err(api_err)?.map(|c| GDiscussionComment { row: Arc::new(c), repo: self.repo.clone() }))
    }
    async fn replies(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> async_graphql::Result<DiscussionCommentConnection> {
        let rows = if self.row.parent_id.is_some() {
            Vec::new()
        } else {
            svc::comments(st(ctx), self.row.discussion_id, Some(self.row.id)).await.map_err(api_err)?
        };
        let all = rows.into_iter().map(|c| GDiscussionComment { row: Arc::new(c), repo: self.repo.clone() }).collect();
        Ok(DiscussionCommentConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn discussion(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GDiscussion>> {
        Ok(svc::by_id(st(ctx), self.row.discussion_id).await.map_err(api_err)?.map(|d| GDiscussion { row: Arc::new(d), repo: self.repo.clone() }))
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> bool {
        self.repo.perm >= crate::db::Perm::Triage || self.row.user_id == Some(actor(ctx).id)
    }
}

// ---------------------------------------------------------------------------------------------
// Repository fields (delegated from `repo.rs`) and node resolution (from `query.rs`)

#[allow(clippy::too_many_arguments)]
pub async fn repo_discussions(
    ctx: &Context<'_>,
    repo: &Arc<RepoCtx>,
    args: ConnArgs,
    order_by: Option<DiscussionOrder>,
    category_id: Option<ID>,
    states: Option<Vec<DiscussionState>>,
    answered: Option<bool>,
) -> async_graphql::Result<DiscussionConnection> {
    let category_id = match &category_id {
        Some(id) => Some(dec(id, "DiscussionCategory")?),
        None => None,
    };
    let closed = match states.as_deref() {
        Some([DiscussionState::Open]) => Some(false),
        Some([DiscussionState::Closed]) => Some(true),
        _ => None,
    };
    let f = ListFilter {
        category_id,
        closed,
        answered,
        by_created: order_by.as_ref().is_some_and(|o| o.field == DiscussionOrderField::CreatedAt),
        ascending: order_by.as_ref().is_some_and(|o| o.direction == OrderDirection::Asc),
    };
    let rows = svc::list(st(ctx), repo, &f).await.map_err(api_err)?;
    let all = rows.into_iter().map(|r| GDiscussion { row: Arc::new(r), repo: repo.clone() }).collect();
    Ok(DiscussionConnection::from_all(all, &args))
}

pub async fn repo_discussion(ctx: &Context<'_>, repo: &Arc<RepoCtx>, number: i32) -> async_graphql::Result<Option<GDiscussion>> {
    match svc::by_number(st(ctx), repo, number as i64).await.map_err(api_err)? {
        Some(d) => Ok(Some(GDiscussion { row: Arc::new(d), repo: repo.clone() })),
        None => Err(not_found(format!("Could not resolve to a Discussion with the number of {number}."))),
    }
}

pub async fn repo_categories(ctx: &Context<'_>, repo: &Arc<RepoCtx>, args: ConnArgs) -> async_graphql::Result<DiscussionCategoryConnection> {
    let rows = svc::categories(st(ctx), repo).await.map_err(api_err)?;
    let all = rows.into_iter().map(|r| GDiscussionCategory { row: Arc::new(r), repo: repo.clone() }).collect();
    Ok(DiscussionCategoryConnection::from_all(all, &args))
}

/// node(id:) for Discussion / DiscussionComment ids the viewer can see.
pub async fn discussion_node(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GDiscussion>> {
    let Some(d) = svc::by_id(st(ctx), id).await.map_err(api_err)? else { return Ok(None) };
    Ok(loaders::repo(ctx, d.repo_id).await?.map(|repo| GDiscussion { row: Arc::new(d), repo }))
}

pub async fn comment_node(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GDiscussionComment>> {
    let Some(c) = svc::comment_by_id(st(ctx), id).await.map_err(api_err)? else { return Ok(None) };
    Ok(loaders::repo(ctx, c.repo_id).await?.map(|repo| GDiscussionComment { row: Arc::new(c), repo }))
}

async fn load_discussion(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<GDiscussion> {
    discussion_node(ctx, dec(id, "Discussion")?).await?.ok_or_else(|| unresolvable(id))
}

async fn load_comment(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<GDiscussionComment> {
    comment_node(ctx, dec(id, "DiscussionComment")?).await?.ok_or_else(|| unresolvable(id))
}

// ---------------------------------------------------------------------------------------------
// Mutations

#[derive(InputObject)]
pub struct CreateDiscussionInput {
    pub repository_id: ID,
    pub category_id: ID,
    pub title: String,
    pub body: String,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateDiscussionInput {
    pub discussion_id: ID,
    pub title: Option<String>,
    pub body: Option<String>,
    pub category_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct DeleteDiscussionInput {
    pub id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct CloseDiscussionInput {
    pub discussion_id: ID,
    pub reason: Option<DiscussionCloseReason>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct ReopenDiscussionInput {
    pub discussion_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct DiscussionPayload {
    pub client_mutation_id: Option<String>,
    pub discussion: Option<GDiscussion>,
}

#[derive(InputObject)]
pub struct AddDiscussionCommentInput {
    pub discussion_id: ID,
    pub body: String,
    pub reply_to_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateDiscussionCommentInput {
    pub comment_id: ID,
    pub body: String,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct DeleteDiscussionCommentInput {
    pub id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct MarkDiscussionCommentAsAnswerInput {
    pub id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct DiscussionCommentPayload {
    pub client_mutation_id: Option<String>,
    pub comment: Option<GDiscussionComment>,
}

#[derive(SimpleObject)]
pub struct DiscussionAnswerPayload {
    pub client_mutation_id: Option<String>,
    pub discussion: Option<GDiscussion>,
}

#[derive(Default)]
pub struct DiscussionMutation;

#[Object]
impl DiscussionMutation {
    async fn create_discussion(&self, ctx: &Context<'_>, input: CreateDiscussionInput) -> async_graphql::Result<Option<DiscussionPayload>> {
        let rid = dec(&input.repository_id, "Repository")?;
        let repo = loaders::repo(ctx, rid).await?.ok_or_else(|| unresolvable(&input.repository_id))?;
        let cat = dec(&input.category_id, "DiscussionCategory")?;
        let d = svc::create(st(ctx), &repo, actor(ctx), cat, &input.title, &input.body).await.map_err(api_err)?;
        Ok(Some(DiscussionPayload { client_mutation_id: input.client_mutation_id, discussion: Some(GDiscussion { row: Arc::new(d), repo }) }))
    }

    async fn update_discussion(&self, ctx: &Context<'_>, input: UpdateDiscussionInput) -> async_graphql::Result<Option<DiscussionPayload>> {
        let d = load_discussion(ctx, &input.discussion_id).await?;
        let category_id = match &input.category_id {
            Some(c) => Some(dec(c, "DiscussionCategory")?),
            None => None,
        };
        let row = svc::update(st(ctx), &d.repo, actor(ctx), &d.row, DiscussionPatch { title: input.title, body: input.body, category_id })
            .await
            .map_err(api_err)?;
        Ok(Some(DiscussionPayload { client_mutation_id: input.client_mutation_id, discussion: Some(GDiscussion { row: Arc::new(row), repo: d.repo }) }))
    }

    async fn delete_discussion(&self, ctx: &Context<'_>, input: DeleteDiscussionInput) -> async_graphql::Result<Option<DiscussionPayload>> {
        let d = load_discussion(ctx, &input.id).await?;
        svc::delete(st(ctx), &d.repo, actor(ctx), &d.row).await.map_err(api_err)?;
        Ok(Some(DiscussionPayload { client_mutation_id: input.client_mutation_id, discussion: Some(d) }))
    }

    async fn close_discussion(&self, ctx: &Context<'_>, input: CloseDiscussionInput) -> async_graphql::Result<Option<DiscussionPayload>> {
        let d = load_discussion(ctx, &input.discussion_id).await?;
        let reason = match input.reason {
            Some(DiscussionCloseReason::Duplicate) => "duplicate",
            Some(DiscussionCloseReason::Outdated) => "outdated",
            _ => "resolved",
        };
        let row = svc::set_closed(st(ctx), &d.repo, actor(ctx), &d.row, true, Some(reason)).await.map_err(api_err)?;
        Ok(Some(DiscussionPayload { client_mutation_id: input.client_mutation_id, discussion: Some(GDiscussion { row: Arc::new(row), repo: d.repo }) }))
    }

    async fn reopen_discussion(&self, ctx: &Context<'_>, input: ReopenDiscussionInput) -> async_graphql::Result<Option<DiscussionPayload>> {
        let d = load_discussion(ctx, &input.discussion_id).await?;
        let row = svc::set_closed(st(ctx), &d.repo, actor(ctx), &d.row, false, None).await.map_err(api_err)?;
        Ok(Some(DiscussionPayload { client_mutation_id: input.client_mutation_id, discussion: Some(GDiscussion { row: Arc::new(row), repo: d.repo }) }))
    }

    async fn add_discussion_comment(&self, ctx: &Context<'_>, input: AddDiscussionCommentInput) -> async_graphql::Result<Option<DiscussionCommentPayload>> {
        let d = load_discussion(ctx, &input.discussion_id).await?;
        let reply_to = match &input.reply_to_id {
            Some(r) => Some(dec(r, "DiscussionComment")?),
            None => None,
        };
        let c = svc::add_comment(st(ctx), &d.repo, actor(ctx), &d.row, &input.body, reply_to).await.map_err(api_err)?;
        Ok(Some(DiscussionCommentPayload { client_mutation_id: input.client_mutation_id, comment: Some(GDiscussionComment { row: Arc::new(c), repo: d.repo }) }))
    }

    async fn update_discussion_comment(&self, ctx: &Context<'_>, input: UpdateDiscussionCommentInput) -> async_graphql::Result<Option<DiscussionCommentPayload>> {
        let c = load_comment(ctx, &input.comment_id).await?;
        let row = svc::update_comment(st(ctx), &c.repo, actor(ctx), &c.row, &input.body).await.map_err(api_err)?;
        Ok(Some(DiscussionCommentPayload { client_mutation_id: input.client_mutation_id, comment: Some(GDiscussionComment { row: Arc::new(row), repo: c.repo }) }))
    }

    async fn delete_discussion_comment(&self, ctx: &Context<'_>, input: DeleteDiscussionCommentInput) -> async_graphql::Result<Option<DiscussionCommentPayload>> {
        let c = load_comment(ctx, &input.id).await?;
        svc::delete_comment(st(ctx), &c.repo, actor(ctx), &c.row).await.map_err(api_err)?;
        Ok(Some(DiscussionCommentPayload { client_mutation_id: input.client_mutation_id, comment: Some(c) }))
    }

    async fn mark_discussion_comment_as_answer(&self, ctx: &Context<'_>, input: MarkDiscussionCommentAsAnswerInput) -> async_graphql::Result<Option<DiscussionAnswerPayload>> {
        let c = load_comment(ctx, &input.id).await?;
        let d = svc::set_answer(st(ctx), &c.repo, actor(ctx), &c.row, true).await.map_err(api_err)?;
        Ok(Some(DiscussionAnswerPayload { client_mutation_id: input.client_mutation_id, discussion: Some(GDiscussion { row: Arc::new(d), repo: c.repo }) }))
    }

    async fn unmark_discussion_comment_as_answer(&self, ctx: &Context<'_>, input: MarkDiscussionCommentAsAnswerInput) -> async_graphql::Result<Option<DiscussionAnswerPayload>> {
        let c = load_comment(ctx, &input.id).await?;
        let d = svc::set_answer(st(ctx), &c.repo, actor(ctx), &c.row, false).await.map_err(api_err)?;
        Ok(Some(DiscussionAnswerPayload { client_mutation_id: input.client_mutation_id, discussion: Some(GDiscussion { row: Arc::new(d), repo: c.repo }) }))
    }
}
