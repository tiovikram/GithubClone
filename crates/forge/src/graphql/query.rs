//! Query root.
use std::sync::Arc;

use async_graphql::{Context, Interface, Object, ID};

use super::connection::ConnArgs;
use super::enums::{dt, DateTime, SearchType};
use super::issue::{self, CommentRow, GComment, GIssue, IssueOrPullRequest};
use super::loaders;
use super::pull::{GPull, GReview, ReviewRow};
use super::repo::{self, find_repo, st, GCommit, GLabel, GMilestone, GRef, GRelease, GRepo};
use super::search::{self, SearchResultItemConnection};
use super::user::{GBot, GOrg, GUser, RepositoryOwner};
use super::{not_found, Viewer};
use crate::db::User;
use crate::ids;
use crate::json::issue::{LabelRow, MilestoneRow};

pub struct Query;

#[derive(Interface, Clone)]
#[graphql(name = "Node", field(name = "id", ty = "ID"))]
pub enum Node {
    User(GUser),
    Organization(GOrg),
    Bot(GBot),
    Repository(GRepo),
    Issue(GIssue),
    PullRequest(GPull),
    IssueComment(GComment),
    Label(GLabel),
    Milestone(GMilestone),
    PullRequestReview(GReview),
    Release(GRelease),
    Ref(GRef),
    Commit(GCommit),
    ProjectV2(super::projects::ProjectV2),
    ProjectV2Item(super::projects::ProjectV2Item),
    DraftIssue(super::projects::GDraftIssue),
    Discussion(super::discussions::GDiscussion),
    DiscussionComment(super::discussions::GDiscussionComment),
}

fn user_node(u: User) -> Node {
    let u = Arc::new(u);
    match u.kind.as_str() {
        "Organization" => Node::Organization(GOrg(u)),
        "Bot" => Node::Bot(GBot(u)),
        _ => Node::User(GUser(u)),
    }
}

/// Resolves a global node id (see `crate::ids`) to an object the viewer may see.
pub async fn resolve_node(ctx: &Context<'_>, id: &str) -> async_graphql::Result<Option<Node>> {
    let Some((kind, key)) = ids::decode(id) else { return Ok(None) };
    let s = st(ctx);
    let int = key.parse::<i64>().ok();
    Ok(match (kind.as_str(), int) {
        ("User" | "Organization" | "Bot", Some(i)) => crate::db::user_by_id(&s.db, i).await.ok().map(user_node),
        ("Repository", Some(i)) => loaders::repo(ctx, i).await?.map(|r| Node::Repository(GRepo::new(r))),
        ("Issue" | "PullRequest", Some(i)) => issue::by_id(ctx, i).await?.map(|x| match x {
            IssueOrPullRequest::Issue(i) => Node::Issue(i),
            IssueOrPullRequest::PullRequest(p) => Node::PullRequest(p),
        }),
        ("IssueComment", Some(i)) => {
            let row: Option<CommentRow> =
                sqlx::query_as("SELECT id, issue_id, repo_id, user_id, body, created_at, updated_at FROM issue_comments WHERE id = $1")
                    .bind(i)
                    .fetch_optional(&s.db)
                    .await?;
            match row {
                Some(c) => match issue::by_id(ctx, c.issue_id).await? {
                    Some(IssueOrPullRequest::Issue(is)) => {
                        Some(Node::IssueComment(GComment { row: Arc::new(c), repo: is.repo.clone(), number: is.row.number, is_pull: false }))
                    }
                    Some(IssueOrPullRequest::PullRequest(p)) => {
                        Some(Node::IssueComment(GComment { row: Arc::new(c), repo: p.repo.clone(), number: p.row.number, is_pull: true }))
                    }
                    None => None,
                },
                None => None,
            }
        }
        ("Label", Some(i)) => {
            let row: Option<LabelRow> =
                sqlx::query_as("SELECT id, repo_id, name, color, description, is_default, created_at FROM labels WHERE id = $1")
                    .bind(i)
                    .fetch_optional(&s.db)
                    .await?;
            match row {
                Some(l) => loaders::repo(ctx, l.repo_id).await?.map(|r| Node::Label(GLabel::new(l, &r))),
                None => None,
            }
        }
        ("Milestone", Some(i)) => {
            let row: Option<MilestoneRow> = sqlx::query_as("SELECT * FROM milestones WHERE id = $1").bind(i).fetch_optional(&s.db).await?;
            match row {
                Some(m) => loaders::repo(ctx, m.repo_id).await?.map(|r| Node::Milestone(GMilestone { row: Arc::new(m), repo: r })),
                None => None,
            }
        }
        ("PullRequestReview", Some(i)) => {
            let row: Option<ReviewRow> = sqlx::query_as("SELECT * FROM pull_reviews WHERE id = $1").bind(i).fetch_optional(&s.db).await?;
            // Pending (draft) reviews are visible to their author only.
            let viewer_id = ctx.data_unchecked::<Viewer>().0.id;
            match row.filter(|r| r.state != "PENDING" || r.user_id == Some(viewer_id)) {
                Some(r) => match issue::by_id(ctx, r.issue_id).await? {
                    Some(IssueOrPullRequest::PullRequest(p)) => {
                        Some(Node::PullRequestReview(GReview { row: Arc::new(r), repo: p.repo.clone(), number: p.row.number }))
                    }
                    _ => None,
                },
                None => None,
            }
        }
        ("Release", Some(i)) => {
            let repo_id: Option<i64> = sqlx::query_scalar("SELECT repo_id FROM releases WHERE id = $1").bind(i).fetch_optional(&s.db).await?;
            match repo_id {
                Some(rid) => match loaders::repo(ctx, rid).await? {
                    Some(r) => repo::load_releases(ctx, &r).await?.into_iter().find(|x| x.row.id == i).map(Node::Release),
                    None => None,
                },
                None => None,
            }
        }
        ("Ref", _) | ("Commit", _) => {
            let Some((rid, rest)) = key.split_once(':') else { return Ok(None) };
            let Ok(rid) = rid.parse::<i64>() else { return Ok(None) };
            let Some(r) = loaders::repo(ctx, rid).await? else { return Ok(None) };
            if kind == "Commit" {
                if !crate::git::read::is_sha(rest) {
                    return Ok(None);
                }
                Some(Node::Commit(GCommit::new(r, rest.to_lowercase())))
            } else {
                let refs = repo::repo_refs(ctx, rid).await?;
                refs.get(rest).map(|oid| Node::Ref(GRef { repo: r, name: rest.to_string(), oid: oid.clone() }))
            }
        }
        ("ProjectV2", Some(i)) => super::projects::node_project(ctx, i).await?.map(Node::ProjectV2),
        ("ProjectV2Item", Some(i)) => super::projects::node_item(ctx, i).await?.map(Node::ProjectV2Item),
        ("DraftIssue", Some(i)) => super::projects::node_draft(ctx, i).await?.map(Node::DraftIssue),
        ("Discussion", Some(i)) => super::discussions::discussion_node(ctx, i).await?.map(Node::Discussion),
        ("DiscussionComment", Some(i)) => super::discussions::comment_node(ctx, i).await?.map(Node::DiscussionComment),
        _ => None,
    })
}

#[Object]
impl Query {
    /// The issue, pull request or repository at a URL.
    async fn resource(&self, ctx: &Context<'_>, url: super::enums::Uri) -> async_graphql::Result<Option<super::projects::UniformResourceLocatable>> {
        super::projects::resource(ctx, &url.0).await
    }
    /// The currently authenticated user.
    async fn viewer(&self, ctx: &Context<'_>) -> GUser {
        GUser(Arc::new(ctx.data_unchecked::<Viewer>().0.clone()))
    }

    async fn user(&self, ctx: &Context<'_>, login: String) -> async_graphql::Result<Option<GUser>> {
        match crate::db::user_by_login(&st(ctx).db, &login).await {
            Ok(u) if u.kind == "User" => Ok(Some(GUser(Arc::new(u)))),
            _ => Err(not_found(format!("Could not resolve to a User with the login of '{login}'."))),
        }
    }

    async fn organization(&self, ctx: &Context<'_>, login: String) -> async_graphql::Result<Option<GOrg>> {
        match crate::db::user_by_login(&st(ctx).db, &login).await {
            Ok(u) if u.kind == "Organization" => Ok(Some(GOrg(Arc::new(u)))),
            _ => Err(not_found(format!("Could not resolve to an Organization with the login of '{login}'."))),
        }
    }

    async fn repository_owner(&self, ctx: &Context<'_>, login: String) -> async_graphql::Result<Option<RepositoryOwner>> {
        match crate::db::user_by_login(&st(ctx).db, &login).await {
            Ok(u) if u.kind != "Bot" => Ok(Some(RepositoryOwner::from_user(Arc::new(u)))),
            _ => Ok(None),
        }
    }

    async fn repository(
        &self,
        ctx: &Context<'_>,
        owner: String,
        name: String,
        #[graphql(default = true)] follow_renames: bool,
    ) -> async_graphql::Result<Option<GRepo>> {
        let _ = follow_renames;
        Ok(Some(find_repo(ctx, &owner, &name).await?))
    }

    async fn node(&self, ctx: &Context<'_>, id: ID) -> async_graphql::Result<Option<Node>> {
        match resolve_node(ctx, &id).await? {
            Some(n) => Ok(Some(n)),
            None => Err(not_found(format!("Could not resolve to a node with the global id of '{}'", id.as_str()))),
        }
    }

    async fn nodes(&self, ctx: &Context<'_>, ids: Vec<ID>) -> async_graphql::Result<Vec<Option<Node>>> {
        // GitHub caps `nodes(ids:)` at 100.
        if ids.len() > 100 {
            return Err(async_graphql::Error::new("You may not request more than 100 nodes at once."));
        }
        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            out.push(resolve_node(ctx, id).await?);
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    async fn search(
        &self,
        ctx: &Context<'_>,
        query: String,
        #[graphql(name = "type")] kind: SearchType,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<SearchResultItemConnection> {
        search::search(ctx, query, kind, ConnArgs::new(first, after, last, before)).await
    }

    async fn rate_limit(&self, #[graphql(default = false)] dry_run: bool) -> RateLimit {
        let _ = dry_run;
        RateLimit
    }

    async fn meta(&self) -> GitHubMetadata {
        GitHubMetadata
    }
}

pub struct RateLimit;

#[Object]
impl RateLimit {
    async fn limit(&self) -> i32 {
        5000
    }
    async fn cost(&self) -> i32 {
        1
    }
    async fn remaining(&self) -> i32 {
        4999
    }
    async fn used(&self) -> i32 {
        1
    }
    async fn node_count(&self) -> i32 {
        0
    }
    async fn reset_at(&self) -> DateTime {
        dt(chrono::Utc::now() + chrono::Duration::hours(1))
    }
}

pub struct GitHubMetadata;

#[Object]
impl GitHubMetadata {
    async fn is_password_authentication_verifiable(&self) -> bool {
        true
    }
    async fn git_ip_addresses(&self) -> Option<Vec<String>> {
        Some(Vec::new())
    }
}
