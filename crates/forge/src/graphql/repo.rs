//! Repository and the git-backed types hanging off it (refs, commits, status checks),
//! labels, milestones, releases and the (empty) project types.
use std::sync::Arc;

use async_graphql::{Context, Interface, Object, Union, ID};
use chrono::{TimeZone, Utc};
use tokio::sync::OnceCell;

use super::connection::{ConnArgs, PageInfo};
use super::enums::*;
use super::issue::{self, GIssue, IssueConnection, IssueOrPullRequest};
use super::loaders::{self, lerr, loader, RepoId};
use super::pull::{self, GPull, PullRequestConnection};
use super::user::{GUser, RepositoryOwner, UserConnection};
use super::{not_found, Viewer};
use crate::db::{Perm, RepoCtx, User};
use crate::ids;
use crate::json::issue::{LabelRow, MilestoneRow};
use crate::state::AppState;

pub fn st<'a>(ctx: &'a Context<'_>) -> &'a AppState {
    ctx.data_unchecked::<AppState>()
}

pub fn viewer<'a>(ctx: &'a Context<'_>) -> Option<&'a User> {
    ctx.data_opt::<Viewer>().map(|v| &v.0)
}

#[derive(Clone)]
pub struct GRepo {
    pub ctx: Arc<RepoCtx>,
}

impl GRepo {
    pub fn new(ctx: Arc<RepoCtx>) -> Self {
        GRepo { ctx }
    }
    pub fn full_name(&self) -> String {
        self.ctx.full_name()
    }
    fn html_url(&self, ctx: &Context<'_>) -> String {
        format!("{}/{}", st(ctx).base_url(), self.full_name())
    }
}

crate::gql_connection!(RepositoryConnection, RepositoryEdge, "RepositoryConnection", "RepositoryEdge", GRepo);
crate::gql_connection!(LabelConnection, LabelEdge, "LabelConnection", "LabelEdge", GLabel);
crate::gql_connection!(MilestoneConnection, MilestoneEdge, "MilestoneConnection", "MilestoneEdge", GMilestone);
crate::gql_connection!(RefConnection, RefEdge, "RefConnection", "RefEdge", GRef);
crate::gql_connection!(ReleaseConnection, ReleaseEdge, "ReleaseConnection", "ReleaseEdge", GRelease);
crate::gql_connection!(RepositoryTopicConnection, RepositoryTopicEdge, "RepositoryTopicConnection", "RepositoryTopicEdge", RepositoryTopic);
crate::gql_connection!(ProjectConnection, ProjectEdge, "ProjectConnection", "ProjectEdge", Project);
crate::gql_connection!(ProjectCardConnection, ProjectCardEdge, "ProjectCardConnection", "ProjectCardEdge", ProjectCard);
// Projects (v2) live in `graphql::projects`; re-exported for the types that link to them.
pub use super::projects::{ProjectV2Connection, ProjectV2ItemConnection};
crate::gql_connection!(GitActorConnection, GitActorEdge, "GitActorConnection", "GitActorEdge", GitActor);
crate::gql_connection!(
    StatusCheckRollupContextConnection,
    StatusCheckRollupContextEdge,
    "StatusCheckRollupContextConnection",
    "StatusCheckRollupContextEdge",
    StatusCheckRollupContext
);
crate::gql_connection!(ReleaseAssetConnection, ReleaseAssetEdge, "ReleaseAssetConnection", "ReleaseAssetEdge", ReleaseAsset);

/// Lists repositories of `owner` visible to the viewer (User/Organization.repositories).
#[allow(clippy::too_many_arguments)]
pub async fn list_repositories(
    ctx: &Context<'_>,
    owner: &User,
    args: ConnArgs,
    privacy: Option<RepositoryPrivacy>,
    is_fork: Option<bool>,
    owner_affiliations: Option<Vec<RepositoryAffiliation>>,
    affiliations: Option<Vec<RepositoryAffiliation>>,
    order_by: Option<RepositoryOrder>,
) -> async_graphql::Result<RepositoryConnection> {
    let s = st(ctx);
    let affs = owner_affiliations
        .or(affiliations)
        .unwrap_or_else(|| vec![RepositoryAffiliation::Owner, RepositoryAffiliation::Collaborator]);
    let mut conds = Vec::new();
    for a in &affs {
        conds.push(match a {
            RepositoryAffiliation::Owner => "r.owner_id = $2",
            RepositoryAffiliation::Collaborator => {
                "EXISTS(SELECT 1 FROM repo_collaborators c2 WHERE c2.repo_id = r.id AND c2.user_id = $2)"
            }
            RepositoryAffiliation::OrganizationMember => {
                "EXISTS(SELECT 1 FROM org_members m2 WHERE m2.org_id = r.owner_id AND m2.user_id = $2)"
            }
        });
    }
    if conds.is_empty() {
        conds.push("r.owner_id = $2");
    }
    let mut extra = String::new();
    match privacy {
        Some(RepositoryPrivacy::Public) => extra.push_str(" AND NOT r.private"),
        Some(RepositoryPrivacy::Private) => extra.push_str(" AND r.private"),
        None => {}
    }
    match is_fork {
        Some(true) => extra.push_str(" AND r.fork_parent_id IS NOT NULL"),
        Some(false) => extra.push_str(" AND r.fork_parent_id IS NULL"),
        None => {}
    }
    let (col, dir) = match order_by {
        Some(o) => (
            match o.field {
                RepositoryOrderField::CreatedAt => "r.created_at",
                RepositoryOrderField::UpdatedAt => "r.updated_at",
                RepositoryOrderField::PushedAt => "COALESCE(r.pushed_at, r.created_at)",
                RepositoryOrderField::Name => "lower(r.name)",
                RepositoryOrderField::Stargazers => "r.stargazers_count",
            },
            o.direction.sql(),
        ),
        None => ("r.created_at", "ASC"),
    };
    let visible = "(NOT r.private OR r.owner_id = $1 OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = $1)
                    OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = $1)
                    OR EXISTS(SELECT 1 FROM users a WHERE a.id = $1 AND a.site_admin))";
    let from = format!("FROM repositories r WHERE {visible} AND ({}){extra}", conds.join(" OR "));
    let vid = viewer(ctx).map(|u| u.id).unwrap_or(-1);
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}")))
        .bind(vid)
        .bind(owner.id)
        .fetch_one(&s.db)
        .await?;
    let w = args.window(total as usize);
    let ids: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT r.id {from} ORDER BY {col} {dir} NULLS LAST, r.id {dir} OFFSET {} LIMIT {}",
        w.offset(),
        w.limit()
    )))
    .bind(vid)
    .bind(owner.id)
    .fetch_all(&s.db)
    .await?;
    let items = repos_by_ids(ctx, &ids).await?;
    Ok(RepositoryConnection::new(items, w))
}

/// Loads repositories (in the given order), skipping invisible ones.
pub async fn repos_by_ids(ctx: &Context<'_>, ids: &[i64]) -> async_graphql::Result<Vec<GRepo>> {
    let m = loader(ctx).load_many(ids.iter().map(|i| RepoId(*i))).await.map_err(lerr)?;
    Ok(ids.iter().filter_map(|i| m.get(&RepoId(*i)).cloned().map(GRepo::new)).collect())
}

/// Resolves `owner/name` for the viewer with GitHub's NOT_FOUND error.
pub async fn find_repo(ctx: &Context<'_>, owner: &str, name: &str) -> async_graphql::Result<GRepo> {
    match crate::db::load_repo(&st(ctx).db, viewer(ctx), owner, name).await {
        Ok(c) => Ok(GRepo::new(Arc::new(c))),
        Err(_) => Err(not_found(format!("Could not resolve to a Repository with the name '{owner}/{name}'."))),
    }
}

/// Committed refs of a repository (from Postgres; no git needed).
pub async fn repo_refs(ctx: &Context<'_>, repo_id: i64) -> async_graphql::Result<crate::git::Refs> {
    Ok(st(ctx).git.refs(repo_id).await.map_err(|e| async_graphql::Error::new(e.to_string()))?.1)
}

#[allow(clippy::too_many_arguments)]
#[Object(name = "Repository")]
impl GRepo {
    pub async fn id(&self) -> ID {
        ID(ids::encode("Repository", self.ctx.repo.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.ctx.repo.id)
    }
    async fn name(&self) -> String {
        self.ctx.repo.name.clone()
    }
    async fn name_with_owner(&self) -> String {
        self.full_name()
    }
    async fn owner(&self) -> RepositoryOwner {
        RepositoryOwner::from_user(Arc::new(self.ctx.owner.clone()))
    }
    async fn description(&self) -> Option<String> {
        self.ctx.repo.description.clone()
    }
    #[graphql(name = "descriptionHTML")]
    async fn description_html(&self) -> Html {
        Html(format!("<div>{}</div>", html_escape(self.ctx.repo.description.as_deref().unwrap_or(""))))
    }
    async fn short_description_html(&self) -> Html {
        Html(html_escape(self.ctx.repo.description.as_deref().unwrap_or("")))
    }
    async fn homepage_url(&self) -> Option<Uri> {
        self.ctx.repo.homepage.clone().filter(|h| !h.is_empty()).map(Uri)
    }
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(self.html_url(ctx))
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(format!("/{}", self.full_name()))
    }
    async fn ssh_url(&self, ctx: &Context<'_>) -> GitSshRemote {
        GitSshRemote(format!("git@{}:{}.git", crate::json::repo::host_of(st(ctx).base_url()), self.full_name()))
    }
    async fn mirror_url(&self) -> Option<Uri> {
        None
    }
    async fn security_policy_url(&self) -> Option<Uri> {
        None
    }
    async fn open_graph_image_url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/avatars/{}", st(ctx).base_url(), self.ctx.owner.login))
    }
    async fn uses_custom_open_graph_image(&self) -> bool {
        false
    }
    async fn created_at(&self) -> DateTime {
        dt(self.ctx.repo.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.ctx.repo.updated_at)
    }
    async fn pushed_at(&self) -> Option<DateTime> {
        dt_opt(self.ctx.repo.pushed_at)
    }
    async fn archived_at(&self) -> Option<DateTime> {
        self.ctx.repo.archived.then(|| dt(self.ctx.repo.updated_at))
    }
    async fn is_private(&self) -> bool {
        self.ctx.repo.private
    }
    async fn visibility(&self) -> RepositoryVisibility {
        match self.ctx.repo.visibility.as_str() {
            "internal" => RepositoryVisibility::Internal,
            _ if self.ctx.repo.private => RepositoryVisibility::Private,
            _ => RepositoryVisibility::Public,
        }
    }
    async fn is_fork(&self) -> bool {
        self.ctx.repo.fork_parent_id.is_some()
    }
    async fn is_archived(&self) -> bool {
        self.ctx.repo.archived
    }
    async fn is_disabled(&self) -> bool {
        false
    }
    async fn is_locked(&self) -> bool {
        false
    }
    async fn is_mirror(&self) -> bool {
        false
    }
    async fn is_template(&self) -> bool {
        self.ctx.repo.is_template
    }
    async fn is_empty(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(!repo_refs(ctx, self.ctx.repo.id).await?.keys().any(|r| r.starts_with("refs/heads/")))
    }
    async fn is_in_organization(&self) -> bool {
        self.ctx.owner.kind == "Organization"
    }
    async fn is_user_configuration_repository(&self) -> bool {
        self.ctx.repo.name.eq_ignore_ascii_case(&self.ctx.owner.login)
    }
    async fn is_blank_issues_enabled(&self) -> bool {
        true
    }
    async fn is_security_policy_enabled(&self) -> Option<bool> {
        Some(false)
    }
    async fn has_issues_enabled(&self) -> bool {
        self.ctx.repo.has_issues
    }
    async fn has_projects_enabled(&self) -> bool {
        self.ctx.repo.has_projects
    }
    async fn has_wiki_enabled(&self) -> bool {
        self.ctx.repo.has_wiki
    }
    async fn has_discussions_enabled(&self) -> bool {
        self.ctx.repo.has_discussions
    }
    #[allow(clippy::too_many_arguments)]
    async fn discussions(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<super::discussions::DiscussionOrder>,
        category_id: Option<ID>,
        states: Option<Vec<super::discussions::DiscussionState>>,
        answered: Option<bool>,
    ) -> async_graphql::Result<super::discussions::DiscussionConnection> {
        let args = ConnArgs::new(first, after, last, before);
        super::discussions::repo_discussions(ctx, &self.ctx, args, order_by, category_id, states, answered).await
    }
    async fn discussion(&self, ctx: &Context<'_>, number: i32) -> async_graphql::Result<Option<super::discussions::GDiscussion>> {
        super::discussions::repo_discussion(ctx, &self.ctx, number).await
    }
    async fn discussion_categories(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        filter_by_assignable: Option<bool>,
    ) -> async_graphql::Result<super::discussions::DiscussionCategoryConnection> {
        let _ = filter_by_assignable;
        super::discussions::repo_categories(ctx, &self.ctx, ConnArgs::new(first, after, last, before)).await
    }
    async fn forking_allowed(&self) -> bool {
        true
    }
    async fn merge_commit_allowed(&self) -> bool {
        self.ctx.repo.allow_merge_commit
    }
    async fn squash_merge_allowed(&self) -> bool {
        self.ctx.repo.allow_squash_merge
    }
    async fn rebase_merge_allowed(&self) -> bool {
        self.ctx.repo.allow_rebase_merge
    }
    async fn delete_branch_on_merge(&self) -> bool {
        self.ctx.repo.delete_branch_on_merge
    }
    async fn fork_count(&self) -> i32 {
        self.ctx.repo.forks_count as i32
    }
    async fn stargazer_count(&self) -> i32 {
        self.ctx.repo.stargazers_count as i32
    }
    async fn disk_usage(&self) -> Option<i32> {
        Some(self.ctx.repo.size_kb as i32)
    }
    async fn viewer_permission(&self) -> Option<RepositoryPermission> {
        RepositoryPermission::from_perm(self.ctx.perm)
    }
    async fn viewer_can_administer(&self) -> bool {
        self.ctx.perm >= Perm::Admin
    }
    async fn viewer_can_create_projects(&self) -> bool {
        self.ctx.perm >= Perm::Write
    }
    async fn viewer_can_subscribe(&self, ctx: &Context<'_>) -> bool {
        viewer(ctx).is_some()
    }
    async fn viewer_can_update_topics(&self) -> bool {
        self.ctx.perm >= Perm::Admin
    }
    async fn viewer_has_starred(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let Some(v) = viewer(ctx) else { return Ok(false) };
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM stars WHERE user_id = $1 AND repo_id = $2)")
            .bind(v.id)
            .bind(self.ctx.repo.id)
            .fetch_one(&st(ctx).db)
            .await?)
    }
    async fn viewer_subscription(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<SubscriptionState>> {
        let Some(v) = viewer(ctx) else { return Ok(None) };
        let w: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM watches WHERE user_id = $1 AND repo_id = $2)")
            .bind(v.id)
            .bind(self.ctx.repo.id)
            .fetch_one(&st(ctx).db)
            .await?;
        Ok(Some(if w { SubscriptionState::Subscribed } else { SubscriptionState::Unsubscribed }))
    }
    async fn viewer_default_merge_method(&self) -> PullRequestMergeMethod {
        let r = &self.ctx.repo;
        if r.allow_merge_commit {
            PullRequestMergeMethod::Merge
        } else if r.allow_squash_merge {
            PullRequestMergeMethod::Squash
        } else {
            PullRequestMergeMethod::Rebase
        }
    }
    async fn viewer_default_commit_email(&self, ctx: &Context<'_>) -> Option<String> {
        viewer(ctx).and_then(|v| v.email.clone())
    }
    async fn viewer_possible_commit_emails(&self, ctx: &Context<'_>) -> Option<Vec<String>> {
        viewer(ctx).map(|v| v.email.clone().into_iter().collect())
    }
    async fn parent(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GRepo>> {
        match self.ctx.repo.fork_parent_id {
            Some(pid) => Ok(loaders::repo(ctx, pid).await?.map(GRepo::new)),
            None => Ok(None),
        }
    }
    async fn template_repository(&self) -> Option<GRepo> {
        None
    }
    async fn default_branch_ref(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GRef>> {
        let refs = repo_refs(ctx, self.ctx.repo.id).await?;
        let name = format!("refs/heads/{}", self.ctx.repo.default_branch);
        Ok(refs.get(&name).map(|oid| GRef { repo: self.ctx.clone(), name, oid: oid.clone() }))
    }
    /// A git object by revision expression (`REF:path`) or oid; see `tree.rs`.
    async fn object(&self, ctx: &Context<'_>, expression: Option<String>, oid: Option<GitObjectId>) -> async_graphql::Result<Option<GitObject>> {
        super::tree::resolve(ctx, self.ctx.clone(), expression, oid.map(|o| o.0)).await
    }
    async fn rulesets(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        #[graphql(default = true)] include_parents: bool,
    ) -> async_graphql::Result<super::rulesets::RepositoryRulesetConnection> {
        super::rulesets::for_repo(ctx, self.ctx.clone(), include_parents, ConnArgs::new(first, after, last, before)).await
    }
    #[graphql(name = "ref")]
    async fn ref_(&self, ctx: &Context<'_>, qualified_name: String) -> async_graphql::Result<Option<GRef>> {
        let refs = repo_refs(ctx, self.ctx.repo.id).await?;
        for cand in [qualified_name.clone(), format!("refs/heads/{qualified_name}"), format!("refs/tags/{qualified_name}")] {
            if let Some(oid) = refs.get(&cand) {
                return Ok(Some(GRef { repo: self.ctx.clone(), name: cand, oid: oid.clone() }));
            }
        }
        Ok(None)
    }
    async fn refs(
        &self,
        ctx: &Context<'_>,
        ref_prefix: String,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
        order_by: Option<RefOrder>,
        direction: Option<OrderDirection>,
    ) -> async_graphql::Result<RefConnection> {
        let refs = repo_refs(ctx, self.ctx.repo.id).await?;
        let mut all: Vec<GRef> = refs
            .iter()
            .filter(|(n, _)| n.starts_with(&ref_prefix))
            .filter(|(n, _)| query.as_deref().is_none_or(|q| n[ref_prefix.len()..].contains(q)))
            .map(|(n, o)| GRef { repo: self.ctx.clone(), name: n.clone(), oid: o.clone() })
            .collect();
        let desc = order_by.map(|o| o.direction).or(direction) == Some(OrderDirection::Desc);
        if desc {
            all.reverse();
        }
        Ok(RefConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn labels(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<LabelOrder>,
        query: Option<String>,
    ) -> async_graphql::Result<Option<LabelConnection>> {
        let (col, dir) = match order_by {
            Some(o) => (
                match o.field {
                    LabelOrderField::Name => "lower(name)",
                    LabelOrderField::CreatedAt => "created_at",
                },
                o.direction.sql(),
            ),
            None => ("lower(name)", "ASC"),
        };
        let q = query.filter(|q| !q.is_empty()).map(|q| format!("%{}%", like_escape(&q)));
        let rows: Vec<LabelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, repo_id, name, color, description, is_default, created_at FROM labels
              WHERE repo_id = $1 AND ($2::text IS NULL OR name ILIKE $2 OR description ILIKE $2)
              ORDER BY {col} {dir}, id {dir}"
        )))
        .bind(self.ctx.repo.id)
        .bind(q)
        .fetch_all(&st(ctx).db)
        .await?;
        let all = rows.into_iter().map(|l| GLabel::new(l, &self.ctx)).collect();
        Ok(Some(LabelConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn label(&self, ctx: &Context<'_>, name: String) -> async_graphql::Result<Option<GLabel>> {
        let row: Option<LabelRow> = sqlx::query_as(
            "SELECT id, repo_id, name, color, description, is_default, created_at FROM labels WHERE repo_id = $1 AND lower(name) = lower($2)",
        )
        .bind(self.ctx.repo.id)
        .bind(name)
        .fetch_optional(&st(ctx).db)
        .await?;
        Ok(row.map(|l| GLabel::new(l, &self.ctx)))
    }
    async fn milestones(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        states: Option<Vec<MilestoneState>>,
        order_by: Option<MilestoneOrder>,
        query: Option<String>,
    ) -> async_graphql::Result<Option<MilestoneConnection>> {
        let (col, dir) = match order_by {
            Some(o) => (
                match o.field {
                    MilestoneOrderField::CreatedAt => "created_at",
                    MilestoneOrderField::UpdatedAt => "updated_at",
                    MilestoneOrderField::DueDate => "due_on",
                    MilestoneOrderField::Number => "number",
                },
                o.direction.sql(),
            ),
            None => ("number", "ASC"),
        };
        let states: Option<Vec<String>> = states.map(|v| {
            v.iter()
                .map(|s| match s {
                    MilestoneState::Open => "open".to_string(),
                    MilestoneState::Closed => "closed".to_string(),
                })
                .collect()
        });
        let q = query.filter(|q| !q.is_empty()).map(|q| format!("%{}%", like_escape(&q)));
        let rows: Vec<MilestoneRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT * FROM milestones WHERE repo_id = $1 AND ($2::text[] IS NULL OR state = ANY($2))
               AND ($3::text IS NULL OR title ILIKE $3) ORDER BY {col} {dir} NULLS LAST, id {dir}"
        )))
        .bind(self.ctx.repo.id)
        .bind(states)
        .bind(q)
        .fetch_all(&st(ctx).db)
        .await?;
        let all = rows.into_iter().map(|m| GMilestone { row: Arc::new(m), repo: self.ctx.clone() }).collect();
        Ok(Some(MilestoneConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn milestone(&self, ctx: &Context<'_>, number: i32) -> async_graphql::Result<Option<GMilestone>> {
        let row: Option<MilestoneRow> = sqlx::query_as("SELECT * FROM milestones WHERE repo_id = $1 AND number = $2")
            .bind(self.ctx.repo.id)
            .bind(number as i64)
            .fetch_optional(&st(ctx).db)
            .await?;
        Ok(row.map(|m| GMilestone { row: Arc::new(m), repo: self.ctx.clone() }))
    }
    async fn assignable_users(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
    ) -> async_graphql::Result<UserConnection> {
        let all = assignable(ctx, &self.ctx, query).await?;
        Ok(UserConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn mentionable_users(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
    ) -> async_graphql::Result<UserConnection> {
        let all = assignable(ctx, &self.ctx, query).await?;
        Ok(UserConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn collaborators(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
    ) -> async_graphql::Result<Option<UserConnection>> {
        let all = assignable(ctx, &self.ctx, query).await?;
        Ok(Some(UserConnection::from_all(all, &ConnArgs::new(first, after, last, before))))
    }
    async fn watchers(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<UserConnection> {
        let users: Vec<User> =
            sqlx::query_as("SELECT u.* FROM watches w JOIN users u ON u.id = w.user_id WHERE w.repo_id = $1 ORDER BY w.created_at")
                .bind(self.ctx.repo.id)
                .fetch_all(&st(ctx).db)
                .await?;
        let all = users.into_iter().map(|u| GUser(Arc::new(u))).collect();
        Ok(UserConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn stargazers(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<UserConnection> {
        let users: Vec<User> =
            sqlx::query_as("SELECT u.* FROM stars s JOIN users u ON u.id = s.user_id WHERE s.repo_id = $1 ORDER BY s.created_at")
                .bind(self.ctx.repo.id)
                .fetch_all(&st(ctx).db)
                .await?;
        let all = users.into_iter().map(|u| GUser(Arc::new(u))).collect();
        Ok(UserConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn issues(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<IssueOrder>,
        states: Option<Vec<IssueState>>,
        labels: Option<Vec<String>>,
        filter_by: Option<IssueFilters>,
    ) -> async_graphql::Result<IssueConnection> {
        issue::list_issues(ctx, &self.ctx, ConnArgs::new(first, after, last, before), order_by, states, labels, filter_by).await
    }
    async fn issue(&self, ctx: &Context<'_>, number: i32) -> async_graphql::Result<Option<GIssue>> {
        match issue::by_number(ctx, &self.ctx, number as i64).await? {
            Some(IssueOrPullRequest::Issue(i)) => Ok(Some(i)),
            _ => Err(not_found(format!("Could not resolve to an Issue with the number of {number}."))),
        }
    }
    async fn pull_request(&self, ctx: &Context<'_>, number: i32) -> async_graphql::Result<Option<GPull>> {
        match issue::by_number(ctx, &self.ctx, number as i64).await? {
            Some(IssueOrPullRequest::PullRequest(p)) => Ok(Some(p)),
            _ => Err(not_found(format!("Could not resolve to a PullRequest with the number of {number}."))),
        }
    }
    async fn issue_or_pull_request(&self, ctx: &Context<'_>, number: i32) -> async_graphql::Result<Option<IssueOrPullRequest>> {
        match issue::by_number(ctx, &self.ctx, number as i64).await? {
            Some(x) => Ok(Some(x)),
            None => Err(not_found(format!("Could not resolve to an issue or pull request with the number of {number}."))),
        }
    }
    async fn pull_requests(
        &self,
        ctx: &Context<'_>,
        states: Option<Vec<PullRequestState>>,
        labels: Option<Vec<String>>,
        head_ref_name: Option<String>,
        base_ref_name: Option<String>,
        order_by: Option<IssueOrder>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<PullRequestConnection> {
        pull::list_pulls(
            ctx,
            &self.ctx,
            ConnArgs::new(first, after, last, before),
            states,
            labels,
            head_ref_name,
            base_ref_name,
            order_by,
        )
        .await
    }
    async fn releases(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<ReleaseOrder>,
    ) -> async_graphql::Result<ReleaseConnection> {
        let mut all = load_releases(ctx, &self.ctx).await?;
        let o = order_by.unwrap_or(ReleaseOrder { field: ReleaseOrderField::CreatedAt, direction: OrderDirection::Desc });
        match o.field {
            ReleaseOrderField::CreatedAt => all.sort_by_key(|r| (r.row.created_at, r.row.id)),
            ReleaseOrderField::Name => all.sort_by(|a, b| a.row.name.clone().unwrap_or_default().cmp(&b.row.name.clone().unwrap_or_default())),
        }
        if o.direction == OrderDirection::Desc {
            all.reverse();
        }
        Ok(ReleaseConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn release(&self, ctx: &Context<'_>, tag_name: String) -> async_graphql::Result<Option<GRelease>> {
        Ok(load_releases(ctx, &self.ctx).await?.into_iter().find(|r| r.row.tag_name == tag_name))
    }
    async fn latest_release(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GRelease>> {
        Ok(load_releases(ctx, &self.ctx).await?.into_iter().find(|r| r.is_latest))
    }
    async fn forks(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        affiliations: Option<Vec<RepositoryAffiliation>>,
        owner_affiliations: Option<Vec<RepositoryAffiliation>>,
        privacy: Option<RepositoryPrivacy>,
        order_by: Option<RepositoryOrder>,
    ) -> async_graphql::Result<RepositoryConnection> {
        let _ = (affiliations, owner_affiliations, order_by);
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM repositories WHERE fork_parent_id = $1 ORDER BY created_at, id")
            .bind(self.ctx.repo.id)
            .fetch_all(&st(ctx).db)
            .await?;
        let mut all = repos_by_ids(ctx, &ids).await?;
        if let Some(p) = privacy {
            all.retain(|r| r.ctx.repo.private == (p == RepositoryPrivacy::Private));
        }
        Ok(RepositoryConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn primary_language(&self) -> Option<Language> {
        self.ctx.repo.language.clone().map(|name| Language { name })
    }
    async fn languages(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<LanguageOrder>,
    ) -> Option<LanguageConnection> {
        let _ = (first, after, last, before, order_by);
        let edges: Vec<LanguageEdge> = self
            .ctx
            .repo
            .language
            .clone()
            .map(|name| LanguageEdge { size: (self.ctx.repo.size_kb * 1024) as i32, node: Language { name } })
            .into_iter()
            .collect();
        Some(LanguageConnection { total_size: edges.iter().map(|e| e.size).sum(), edges })
    }
    async fn repository_topics(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> RepositoryTopicConnection {
        let all = self.ctx.repo.topics.iter().map(|t| RepositoryTopic { name: t.clone() }).collect();
        RepositoryTopicConnection::from_all(all, &ConnArgs::new(first, after, last, before))
    }
    async fn license_info(&self) -> Option<License> {
        None
    }
    async fn code_of_conduct(&self) -> Option<CodeOfConduct> {
        None
    }
    async fn contact_links(&self) -> Option<Vec<RepositoryContactLink>> {
        Some(Vec::new())
    }
    async fn funding_links(&self) -> Vec<FundingLink> {
        Vec::new()
    }
    async fn issue_templates(&self) -> Option<Vec<IssueTemplate>> {
        Some(Vec::new())
    }
    async fn projects(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        states: Option<Vec<ProjectState>>,
        order_by: Option<ProjectOrder>,
        search: Option<String>,
    ) -> ProjectConnection {
        let _ = (first, after, last, before, states, order_by, search);
        ProjectConnection::empty()
    }
    #[allow(clippy::too_many_arguments)]
    async fn projects_v2(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
        order_by: Option<ProjectV2Order>,
    ) -> async_graphql::Result<ProjectV2Connection> {
        super::projects::projects_for_repo(ctx, self.ctx.repo.id, query, order_by, ConnArgs::new(first, after, last, before)).await
    }
}

/// Users who can be assigned: owner (or org members), collaborators.
async fn assignable(ctx: &Context<'_>, repo: &RepoCtx, query: Option<String>) -> async_graphql::Result<Vec<GUser>> {
    let q = query.filter(|q| !q.is_empty()).map(|q| format!("%{}%", like_escape(&q)));
    let users: Vec<User> = sqlx::query_as(
        "SELECT * FROM users u WHERE u.kind = 'User' AND (u.id = $2
            OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = $1 AND c.user_id = u.id)
            OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = $2 AND m.user_id = u.id))
           AND ($3::text IS NULL OR u.login ILIKE $3 OR u.name ILIKE $3)
         ORDER BY lower(u.login)",
    )
    .bind(repo.repo.id)
    .bind(repo.owner.id)
    .bind(q)
    .fetch_all(&st(ctx).db)
    .await?;
    Ok(users.into_iter().map(|u| GUser(Arc::new(u))).collect())
}

pub fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Very small markdown-ish rendering for `bodyHTML` (escaped paragraphs).
pub fn body_html(s: &str) -> Html {
    if s.is_empty() {
        return Html(String::new());
    }
    Html(
        s.split("\n\n")
            .map(|p| format!("<p dir=\"auto\">{}</p>", html_escape(p).replace('\n', "<br>\n")))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

// ---------------------------------------------------------------------------------------------
// Refs, commits, status checks

#[derive(Clone)]
pub struct GRef {
    pub repo: Arc<RepoCtx>,
    /// Fully qualified name, e.g. `refs/heads/main`.
    pub name: String,
    pub oid: String,
}

#[Object(name = "Ref")]
impl GRef {
    pub async fn id(&self) -> ID {
        ID(ids::encode_str("Ref", &format!("{}:{}", self.repo.repo.id, self.name)))
    }
    async fn name(&self) -> String {
        short_ref(&self.name).to_string()
    }
    async fn prefix(&self) -> String {
        self.name[..self.name.len() - short_ref(&self.name).len()].to_string()
    }
    async fn target(&self) -> Option<GitObject> {
        Some(GitObject::Commit(GCommit::new(self.repo.clone(), self.oid.clone())))
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
    async fn compare(&self, ctx: &Context<'_>, head_ref: String) -> async_graphql::Result<Option<super::ghcompat::Comparison>> {
        super::ghcompat::compare(ctx, self, &head_ref).await
    }
    async fn branch_protection_rule(&self, ctx: &Context<'_>) -> Option<BranchProtectionRule> {
        let branch = self.name.strip_prefix("refs/heads/")?;
        let st = ctx.data_unchecked::<AppState>();
        let r = crate::protection::rules(&st.db, self.repo.repo.id, branch).await.ok()??;
        Some(BranchProtectionRule { repo_id: self.repo.repo.id, rules: r })
    }
}

pub fn short_ref(name: &str) -> &str {
    name.strip_prefix("refs/heads/").or_else(|| name.strip_prefix("refs/tags/")).unwrap_or(name)
}

pub struct BranchProtectionRule {
    repo_id: i64,
    rules: crate::protection::Rules,
}

#[Object]
impl BranchProtectionRule {
    async fn id(&self) -> ID {
        ID(ids::encode_str("BranchProtectionRule", &format!("{}:{}", self.repo_id, self.rules.branch)))
    }
    async fn pattern(&self) -> String {
        self.rules.branch.clone()
    }
    async fn required_approving_review_count(&self) -> Option<i32> {
        self.rules.requires_pull_request().then(|| self.rules.required_approvals() as i32)
    }
    async fn requires_approving_reviews(&self) -> bool {
        self.rules.requires_pull_request()
    }
    async fn dismisses_stale_reviews(&self) -> bool {
        self.rules.dismiss_stale_reviews()
    }
    async fn requires_status_checks(&self) -> bool {
        self.rules.cfg.get("required_status_checks").is_some_and(|v| v.is_object())
    }
    async fn requires_strict_status_checks(&self) -> bool {
        self.rules.strict()
    }
    async fn required_status_check_contexts(&self) -> Vec<String> {
        self.rules.required_contexts()
    }
    async fn is_admin_enforced(&self) -> bool {
        self.rules.enforce_admins()
    }
    async fn allows_force_pushes(&self) -> bool {
        self.rules.allow_force_pushes()
    }
    async fn allows_deletions(&self) -> bool {
        self.rules.allow_deletions()
    }
    async fn requires_linear_history(&self) -> bool {
        self.rules.linear_history()
    }
    async fn lock_branch(&self) -> bool {
        self.rules.lock_branch()
    }
}

#[derive(Interface, Clone)]
#[graphql(
    name = "GitObject",
    field(name = "id", ty = "ID"),
    field(name = "oid", ty = "GitObjectId"),
    field(name = "abbreviated_oid", ty = "String"),
    field(name = "commit_url", ty = "Uri")
)]
pub enum GitObject {
    Commit(GCommit),
    Tree(super::tree::GTree),
    Blob(super::tree::GBlob),
}

#[derive(Clone, Debug, Default)]
pub struct CommitInfo {
    pub oid: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_time: i64,
    pub parents: Vec<String>,
    pub message: String,
}

impl CommitInfo {
    pub fn headline(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
    pub fn body(&self) -> String {
        let mut it = self.message.splitn(2, '\n');
        it.next();
        it.next().unwrap_or("").trim_start_matches('\n').trim_end().to_string()
    }
}

const LOG_FORMAT: &str = "--format=%H%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%P%x00%B%x1e";

pub fn parse_log(out: &[u8]) -> Vec<CommitInfo> {
    String::from_utf8_lossy(out)
        .split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            if rec.is_empty() {
                return None;
            }
            let f: Vec<&str> = rec.splitn(9, '\0').collect();
            if f.len() < 9 {
                return None;
            }
            Some(CommitInfo {
                oid: f[0].to_string(),
                author_name: f[1].to_string(),
                author_email: f[2].to_string(),
                author_time: f[3].parse().unwrap_or(0),
                committer_name: f[4].to_string(),
                committer_email: f[5].to_string(),
                committer_time: f[6].parse().unwrap_or(0),
                parents: f[7].split_whitespace().map(str::to_string).collect(),
                message: f[8].trim_end_matches('\n').to_string(),
            })
        })
        .collect()
}

/// `git log` with forge's record format in the repo's synced cache. `args` are revisions/options.
pub async fn git_log(s: &AppState, repo_id: i64, args: &[&str]) -> anyhow::Result<Vec<CommitInfo>> {
    let path = s.git.sync(repo_id).await?;
    let mut all = vec!["log", LOG_FORMAT];
    all.extend_from_slice(args);
    let (ok, out, _) = s.git.git.try_run(&path, &all).await?;
    Ok(if ok { parse_log(&out) } else { Vec::new() })
}

#[derive(Clone)]
pub struct GCommit {
    pub repo: Arc<RepoCtx>,
    pub oid: String,
    info: Arc<OnceCell<Option<CommitInfo>>>,
}

impl GCommit {
    pub fn new(repo: Arc<RepoCtx>, oid: String) -> Self {
        GCommit { repo, oid, info: Arc::new(OnceCell::new()) }
    }
    pub fn with_info(repo: Arc<RepoCtx>, info: CommitInfo) -> Self {
        GCommit { repo, oid: info.oid.clone(), info: Arc::new(OnceCell::new_with(Some(Some(info)))) }
    }
    async fn info(&self, ctx: &Context<'_>) -> Option<&CommitInfo> {
        self.info
            .get_or_init(|| async {
                // The oid can come from a client-supplied node id or a stored review commit id:
                // only a full hex object id may reach git (anything else could be parsed as an
                // option, e.g. `--output=<file>`).
                if !crate::git::read::is_sha(&self.oid) {
                    return None;
                }
                git_log(st(ctx), self.repo.repo.id, &["-1", "--end-of-options", &self.oid]).await.ok().and_then(|mut v| v.pop())
            })
            .await
            .as_ref()
    }
}

fn ts_dt(t: i64) -> DateTime {
    dt(Utc.timestamp_opt(t, 0).single().unwrap_or_default())
}

#[Object(name = "Commit")]
impl GCommit {
    pub async fn id(&self) -> ID {
        ID(ids::encode_str("Commit", &format!("{}:{}", self.repo.repo.id, self.oid)))
    }
    pub async fn oid(&self) -> GitObjectId {
        GitObjectId(self.oid.clone())
    }
    pub async fn abbreviated_oid(&self) -> String {
        self.oid.chars().take(7).collect()
    }
    pub async fn commit_url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/commit/{}", st(ctx).base_url(), self.repo.full_name(), self.oid))
    }
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/commit/{}", st(ctx).base_url(), self.repo.full_name(), self.oid))
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/commit/{}", self.repo.full_name(), self.oid))
    }
    async fn message(&self, ctx: &Context<'_>) -> String {
        self.info(ctx).await.map(|i| i.message.clone()).unwrap_or_default()
    }
    async fn message_headline(&self, ctx: &Context<'_>) -> String {
        self.info(ctx).await.map(|i| i.headline().to_string()).unwrap_or_default()
    }
    async fn message_body(&self, ctx: &Context<'_>) -> String {
        self.info(ctx).await.map(|i| i.body()).unwrap_or_default()
    }
    async fn authored_date(&self, ctx: &Context<'_>) -> DateTime {
        ts_dt(self.info(ctx).await.map(|i| i.author_time).unwrap_or(0))
    }
    async fn committed_date(&self, ctx: &Context<'_>) -> DateTime {
        ts_dt(self.info(ctx).await.map(|i| i.committer_time).unwrap_or(0))
    }
    async fn author(&self, ctx: &Context<'_>) -> Option<GitActor> {
        self.info(ctx).await.map(|i| GitActor { name: i.author_name.clone(), email: i.author_email.clone(), time: i.author_time })
    }
    async fn committer(&self, ctx: &Context<'_>) -> Option<GitActor> {
        self.info(ctx)
            .await
            .map(|i| GitActor { name: i.committer_name.clone(), email: i.committer_email.clone(), time: i.committer_time })
    }
    async fn authors(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>) -> GitActorConnection {
        let all: Vec<GitActor> = self
            .info(ctx)
            .await
            .map(|i| GitActor { name: i.author_name.clone(), email: i.author_email.clone(), time: i.author_time })
            .into_iter()
            .collect();
        GitActorConnection::from_all(all, &ConnArgs::new(first, after, None, None))
    }
    async fn status_check_rollup(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<StatusCheckRollup>> {
        let rows: Vec<StatusRow> = sqlx::query_as(
            "SELECT DISTINCT ON (context) id, state, context, description, target_url, created_at FROM commit_statuses
              WHERE repo_id = $1 AND sha = $2 ORDER BY context, id DESC",
        )
        .bind(self.repo.repo.id)
        .bind(&self.oid)
        .fetch_all(&st(ctx).db)
        .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(StatusCheckRollup { rows: Arc::new(rows), repo: self.repo.clone() }))
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
}

#[derive(Clone)]
pub struct GitActor {
    pub name: String,
    pub email: String,
    pub time: i64,
}

#[Object(name = "GitActor")]
impl GitActor {
    async fn name(&self) -> Option<String> {
        Some(self.name.clone())
    }
    async fn email(&self) -> Option<String> {
        Some(self.email.clone())
    }
    async fn date(&self) -> Option<DateTime> {
        Some(ts_dt(self.time))
    }
    pub async fn avatar_url(&self, ctx: &Context<'_>, size: Option<i32>) -> Uri {
        let _ = size;
        Uri(format!("{}/avatars/{}", st(ctx).base_url(), self.name))
    }
    async fn user(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GUser>> {
        let s = st(ctx);
        let mut u: Option<User> = sqlx::query_as("SELECT * FROM users WHERE lower(email) = lower($1) AND kind = 'User' LIMIT 1")
            .bind(&self.email)
            .fetch_optional(&s.db)
            .await?;
        if u.is_none() {
            // forge's noreply addresses: <id>+<login>@users.noreply.forge
            if let Some((idpart, _)) = self.email.split_once('+') {
                if let Ok(id) = idpart.parse::<i64>() {
                    u = sqlx::query_as("SELECT * FROM users WHERE id = $1").bind(id).fetch_optional(&s.db).await?;
                }
            }
        }
        Ok(u.map(|u| GUser(Arc::new(u))))
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct StatusRow {
    pub id: i64,
    pub state: String,
    pub context: String,
    pub description: Option<String>,
    pub target_url: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Clone)]
pub struct StatusCheckRollup {
    rows: Arc<Vec<StatusRow>>,
    repo: Arc<RepoCtx>,
}

#[Object]
impl StatusCheckRollup {
    pub async fn id(&self) -> ID {
        ID(ids::encode("StatusCheckRollup", self.rows.first().map(|r| r.id).unwrap_or(0)))
    }
    async fn state(&self) -> StatusState {
        combined_state(&self.rows)
    }
    async fn contexts(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> StatusCheckRollupContextConnection {
        let all = self
            .rows
            .iter()
            .map(|r| StatusCheckRollupContext::StatusContext(StatusContext { row: r.clone(), repo: self.repo.clone() }))
            .collect();
        StatusCheckRollupContextConnection::from_all(all, &ConnArgs::new(first, after, last, before))
    }
}

pub fn combined_state(rows: &[StatusRow]) -> StatusState {
    if rows.iter().any(|r| r.state == "error" || r.state == "failure") {
        StatusState::Failure
    } else if rows.iter().any(|r| r.state == "pending") {
        StatusState::Pending
    } else {
        StatusState::Success
    }
}

#[derive(Union, Clone)]
pub enum StatusCheckRollupContext {
    CheckRun(CheckRun),
    StatusContext(StatusContext),
}

#[derive(Clone)]
pub struct StatusContext {
    row: StatusRow,
    repo: Arc<RepoCtx>,
}

#[Object]
impl StatusContext {
    pub async fn id(&self) -> ID {
        ID(ids::encode("StatusContext", self.row.id))
    }
    async fn context(&self) -> String {
        self.row.context.clone()
    }
    async fn state(&self) -> StatusState {
        StatusState::parse(&self.row.state)
    }
    async fn description(&self) -> Option<String> {
        self.row.description.clone()
    }
    async fn target_url(&self) -> Option<Uri> {
        self.row.target_url.clone().filter(|u| !u.is_empty()).map(Uri)
    }
    pub async fn avatar_url(&self, ctx: &Context<'_>) -> Option<Uri> {
        Some(Uri(format!("{}/avatars/{}", st(ctx).base_url(), self.repo.owner.login)))
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn is_required(&self, pull_request_id: Option<ID>, pull_request_number: Option<i32>) -> bool {
        let _ = (pull_request_id, pull_request_number);
        false
    }
}

/// Check runs (GitHub Actions) are not implemented by forge; the type exists for gh's queries.
#[derive(Clone)]
pub struct CheckRun {
    pub id: i64,
    pub name: String,
}

#[Object]
impl CheckRun {
    pub async fn id(&self) -> ID {
        ID(ids::encode("CheckRun", self.id))
    }
    async fn name(&self) -> String {
        self.name.clone()
    }
    async fn status(&self) -> CheckStatusState {
        CheckStatusState::Completed
    }
    async fn conclusion(&self) -> Option<CheckConclusionState> {
        None
    }
    async fn started_at(&self) -> Option<DateTime> {
        None
    }
    async fn completed_at(&self) -> Option<DateTime> {
        None
    }
    async fn details_url(&self) -> Option<Uri> {
        None
    }
    async fn check_suite(&self) -> CheckSuite {
        CheckSuite
    }
    async fn is_required(&self, pull_request_id: Option<ID>, pull_request_number: Option<i32>) -> bool {
        let _ = (pull_request_id, pull_request_number);
        false
    }
}

pub struct CheckSuite;

#[Object]
impl CheckSuite {
    async fn workflow_run(&self) -> Option<WorkflowRun> {
        None
    }
}

/// Deliberately without `event` (§5.2).
pub struct WorkflowRun;

#[Object]
impl WorkflowRun {
    async fn workflow(&self) -> Workflow {
        Workflow
    }
}

pub struct Workflow;

#[Object]
impl Workflow {
    async fn name(&self) -> String {
        String::new()
    }
}

// ---------------------------------------------------------------------------------------------
// Labels, milestones

#[derive(Clone)]
pub struct GLabel {
    pub row: Arc<LabelRow>,
    pub full_name: Arc<str>,
}

impl GLabel {
    pub fn new(row: LabelRow, repo: &RepoCtx) -> Self {
        GLabel { row: Arc::new(row), full_name: repo.full_name().into() }
    }
}

#[Object(name = "Label")]
impl GLabel {
    pub async fn id(&self) -> ID {
        ID(ids::encode("Label", self.row.id))
    }
    async fn name(&self) -> String {
        self.row.name.clone()
    }
    async fn color(&self) -> String {
        self.row.color.clone()
    }
    async fn description(&self) -> Option<String> {
        self.row.description.clone()
    }
    async fn is_default(&self) -> bool {
        self.row.is_default
    }
    async fn created_at(&self) -> Option<DateTime> {
        Some(dt(self.row.created_at))
    }
    async fn updated_at(&self) -> Option<DateTime> {
        Some(dt(self.row.created_at))
    }
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/labels/{}", st(ctx).base_url(), self.full_name, crate::json::issue::urlencode(&self.row.name)))
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/labels/{}", self.full_name, crate::json::issue::urlencode(&self.row.name)))
    }
}

#[derive(Clone)]
pub struct GMilestone {
    pub row: Arc<MilestoneRow>,
    pub repo: Arc<RepoCtx>,
}

#[Object(name = "Milestone")]
impl GMilestone {
    pub async fn id(&self) -> ID {
        ID(ids::encode("Milestone", self.row.id))
    }
    async fn number(&self) -> i32 {
        self.row.number as i32
    }
    async fn title(&self) -> String {
        self.row.title.clone()
    }
    async fn description(&self) -> Option<String> {
        self.row.description.clone()
    }
    async fn due_on(&self) -> Option<DateTime> {
        dt_opt(self.row.due_on)
    }
    async fn state(&self) -> MilestoneState {
        if self.row.state == "closed" { MilestoneState::Closed } else { MilestoneState::Open }
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
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/milestone/{}", st(ctx).base_url(), self.repo.full_name(), self.row.number))
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/milestone/{}", self.repo.full_name(), self.row.number))
    }
    async fn progress_percentage(&self, ctx: &Context<'_>) -> async_graphql::Result<f64> {
        let (open, closed): (i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE state = 'open'), count(*) FILTER (WHERE state = 'closed') FROM issues WHERE milestone_id = $1",
        )
        .bind(self.row.id)
        .fetch_one(&st(ctx).db)
        .await?;
        Ok(if open + closed == 0 { 0.0 } else { closed as f64 * 100.0 / (open + closed) as f64 })
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// Releases

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ReleaseRow {
    pub id: i64,
    pub repo_id: i64,
    pub tag_name: String,
    pub target_commitish: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
    pub make_latest: String,
    pub author_id: Option<i64>,
    pub created_at: chrono::DateTime<Utc>,
    pub published_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Clone)]
pub struct GRelease {
    pub row: Arc<ReleaseRow>,
    pub repo: Arc<RepoCtx>,
    pub is_latest: bool,
}

/// All releases of a repo visible to the viewer (drafts only for writers), with `isLatest`.
pub async fn load_releases(ctx: &Context<'_>, repo: &Arc<RepoCtx>) -> async_graphql::Result<Vec<GRelease>> {
    let rows: Vec<ReleaseRow> = sqlx::query_as(
        "SELECT id, repo_id, tag_name, target_commitish, name, body, draft, prerelease, make_latest, author_id, created_at, published_at
           FROM releases WHERE repo_id = $1 ORDER BY created_at, id",
    )
    .bind(repo.repo.id)
    .fetch_all(&st(ctx).db)
    .await?;
    let can_see_drafts = repo.perm >= Perm::Write;
    let latest = rows
        .iter()
        .filter(|r| !r.draft && !r.prerelease && r.make_latest != "false")
        .max_by_key(|r| (r.published_at.unwrap_or(r.created_at), r.id))
        .map(|r| r.id);
    Ok(rows
        .into_iter()
        .filter(|r| !r.draft || can_see_drafts)
        .map(|r| GRelease { is_latest: Some(r.id) == latest, row: Arc::new(r), repo: repo.clone() })
        .collect())
}

/// Deliberately without `immutable` (§5.2).
#[Object(name = "Release")]
impl GRelease {
    pub async fn id(&self) -> ID {
        ID(ids::encode("Release", self.row.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.row.id)
    }
    async fn name(&self) -> Option<String> {
        self.row.name.clone()
    }
    async fn tag_name(&self) -> String {
        self.row.tag_name.clone()
    }
    async fn description(&self) -> Option<String> {
        self.row.body.clone()
    }
    async fn is_draft(&self) -> bool {
        self.row.draft
    }
    async fn is_prerelease(&self) -> bool {
        self.row.prerelease
    }
    async fn is_latest(&self) -> bool {
        self.is_latest
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn published_at(&self) -> Option<DateTime> {
        dt_opt(self.row.published_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.published_at.unwrap_or(self.row.created_at))
    }
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/releases/tag/{}", st(ctx).base_url(), self.repo.full_name(), self.row.tag_name))
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/releases/tag/{}", self.repo.full_name(), self.row.tag_name))
    }
    async fn author(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GUser>> {
        Ok(loaders::user(ctx, self.row.author_id).await?.map(GUser))
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
    async fn release_assets(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>, name: Option<String>) -> async_graphql::Result<ReleaseAssetConnection> {
        let rows: Vec<(i64, String, String, i64, i64, chrono::DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, name, content_type, size, download_count, created_at FROM release_assets
              WHERE release_id = $1 AND ($2::text IS NULL OR name = $2) ORDER BY id",
        )
        .bind(self.row.id)
        .bind(name)
        .fetch_all(&st(ctx).db)
        .await?;
        let base = st(ctx).base_url().to_string();
        let all = rows
            .into_iter()
            .map(|(id, name, content_type, size, download_count, created_at)| ReleaseAsset {
                url: format!("{base}/{}/releases/download/{}/{}", self.repo.full_name(), self.row.tag_name, name),
                id,
                name,
                content_type,
                size,
                download_count,
                created_at,
            })
            .collect();
        Ok(ReleaseAssetConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
}

#[derive(Clone)]
pub struct ReleaseAsset {
    id: i64,
    name: String,
    content_type: String,
    size: i64,
    download_count: i64,
    created_at: chrono::DateTime<Utc>,
    url: String,
}

#[Object]
impl ReleaseAsset {
    pub async fn id(&self) -> ID {
        ID(ids::encode("ReleaseAsset", self.id))
    }
    async fn name(&self) -> String {
        self.name.clone()
    }
    async fn content_type(&self) -> String {
        self.content_type.clone()
    }
    async fn size(&self) -> i32 {
        self.size as i32
    }
    async fn download_count(&self) -> i32 {
        self.download_count as i32
    }
    async fn created_at(&self) -> DateTime {
        dt(self.created_at)
    }
    async fn download_url(&self) -> Uri {
        Uri(self.url.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// Misc small repository types

#[derive(Clone)]
pub struct Language {
    pub name: String,
}

#[Object]
impl Language {
    pub async fn id(&self) -> ID {
        ID(ids::encode_str("Language", &self.name))
    }
    async fn name(&self) -> String {
        self.name.clone()
    }
    async fn color(&self) -> Option<String> {
        None
    }
}

pub struct LanguageConnection {
    edges: Vec<LanguageEdge>,
    total_size: i32,
}

#[Object]
impl LanguageConnection {
    async fn edges(&self) -> Vec<Option<LanguageEdge>> {
        self.edges.iter().cloned().map(Some).collect()
    }
    async fn nodes(&self) -> Vec<Option<Language>> {
        self.edges.iter().map(|e| Some(e.node.clone())).collect()
    }
    async fn total_count(&self) -> i32 {
        self.edges.len() as i32
    }
    async fn total_size(&self) -> i32 {
        self.total_size
    }
    async fn page_info(&self) -> PageInfo {
        PageInfo::empty()
    }
}

#[derive(Clone)]
pub struct LanguageEdge {
    size: i32,
    node: Language,
}

#[Object]
impl LanguageEdge {
    async fn size(&self) -> i32 {
        self.size
    }
    async fn node(&self) -> Language {
        self.node.clone()
    }
    async fn cursor(&self) -> String {
        super::connection::cursor(0)
    }
}

#[derive(Clone)]
pub struct RepositoryTopic {
    name: String,
}

#[Object]
impl RepositoryTopic {
    async fn topic(&self) -> Topic {
        Topic { name: self.name.clone() }
    }
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/topics/{}", st(ctx).base_url(), self.name))
    }
}

pub struct Topic {
    name: String,
}

#[Object]
impl Topic {
    pub async fn id(&self) -> ID {
        ID(ids::encode_str("Topic", &self.name))
    }
    async fn name(&self) -> String {
        self.name.clone()
    }
}

pub struct License;

#[Object]
impl License {
    async fn key(&self) -> String {
        String::new()
    }
    async fn name(&self) -> String {
        String::new()
    }
    async fn nickname(&self) -> Option<String> {
        None
    }
    async fn spdx_id(&self) -> Option<String> {
        None
    }
    pub async fn url(&self) -> Option<Uri> {
        None
    }
}

pub struct CodeOfConduct;

#[Object]
impl CodeOfConduct {
    async fn key(&self) -> String {
        String::new()
    }
    async fn name(&self) -> String {
        String::new()
    }
    pub async fn url(&self) -> Option<Uri> {
        None
    }
}

pub struct RepositoryContactLink;

#[Object]
impl RepositoryContactLink {
    async fn about(&self) -> String {
        String::new()
    }
    async fn name(&self) -> String {
        String::new()
    }
    pub async fn url(&self) -> Uri {
        Uri(String::new())
    }
}

pub struct FundingLink;

#[Object]
impl FundingLink {
    async fn platform(&self) -> FundingPlatform {
        FundingPlatform::Custom
    }
    pub async fn url(&self) -> Uri {
        Uri(String::new())
    }
}

pub struct IssueTemplate;

#[Object]
impl IssueTemplate {
    async fn name(&self) -> String {
        String::new()
    }
    async fn title(&self) -> Option<String> {
        None
    }
    async fn body(&self) -> Option<String> {
        None
    }
    async fn about(&self) -> Option<String> {
        None
    }
    async fn filename(&self) -> Option<String> {
        None
    }
}

// Projects are not implemented; these types exist so gh's queries and introspection work.

#[derive(Clone)]
pub struct Project;

#[Object]
impl Project {
    pub async fn id(&self) -> ID {
        ID(String::new())
    }
    async fn name(&self) -> String {
        String::new()
    }
    async fn number(&self) -> i32 {
        0
    }
    async fn body(&self) -> Option<String> {
        None
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(String::new())
    }
}

#[derive(Clone)]
pub struct ProjectCard;

#[Object]
impl ProjectCard {
    pub async fn id(&self) -> ID {
        ID(String::new())
    }
    async fn project(&self) -> Project {
        Project
    }
    async fn column(&self) -> Option<ProjectColumn> {
        None
    }
}

pub struct ProjectColumn;

#[Object]
impl ProjectColumn {
    pub async fn id(&self) -> ID {
        ID(String::new())
    }
    async fn name(&self) -> String {
        String::new()
    }
}
