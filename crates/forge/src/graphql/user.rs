//! Users, organizations, bots and the Actor / RepositoryOwner interfaces.
use std::sync::Arc;

use async_graphql::{Context, Interface, Object, Union, ID};

use super::connection::ConnArgs;
use super::enums::{dt, DateTime, ProjectV2Order, RepositoryAffiliation, RepositoryOrder, RepositoryPrivacy, TeamOrder, Uri};
use super::repo::{self, ProjectV2Connection, RepositoryConnection};
use super::{GResult, Viewer};
use crate::db::User;
use crate::ids;
use crate::state::AppState;

fn base(ctx: &Context<'_>) -> String {
    ctx.data_unchecked::<AppState>().base_url().to_string()
}

fn avatar(ctx: &Context<'_>, login: &str) -> Uri {
    Uri(format!("{}/avatars/{login}", base(ctx)))
}

/// A user account (`kind == "User"`).
#[derive(Clone)]
pub struct GUser(pub Arc<User>);

/// An organization account.
#[derive(Clone)]
pub struct GOrg(pub Arc<User>);

/// A bot account.
#[derive(Clone)]
pub struct GBot(pub Arc<User>);

/// Never instantiated; present because GitHub's unions reference it.
#[derive(Clone)]
pub struct GMannequin(pub Arc<User>);

/// Organization team (forge has no teams yet; type exists for unions/introspection).
#[derive(Clone)]
pub struct GTeam {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub org: Arc<User>,
}

#[derive(Interface, Clone)]
#[graphql(
    name = "Actor",
    field(name = "login", ty = "String"),
    field(name = "avatar_url", ty = "Uri", arg(name = "size", ty = "Option<i32>")),
    field(name = "resource_path", ty = "Uri"),
    field(name = "url", ty = "Uri")
)]
pub enum Actor {
    User(GUser),
    Bot(GBot),
    Organization(GOrg),
    Mannequin(GMannequin),
}

impl Actor {
    pub fn from_user(u: Arc<User>) -> Actor {
        match u.kind.as_str() {
            "Organization" => Actor::Organization(GOrg(u)),
            "Bot" => Actor::Bot(GBot(u)),
            _ => Actor::User(GUser(u)),
        }
    }
}

#[derive(Interface, Clone)]
#[graphql(
    name = "RepositoryOwner",
    field(name = "id", ty = "ID"),
    field(name = "login", ty = "String"),
    field(name = "avatar_url", ty = "Uri", arg(name = "size", ty = "Option<i32>")),
    field(name = "resource_path", ty = "Uri"),
    field(name = "url", ty = "Uri"),
    field(
        name = "repositories",
        ty = "RepositoryConnection",
        arg(name = "first", ty = "Option<i32>"),
        arg(name = "after", ty = "Option<String>"),
        arg(name = "last", ty = "Option<i32>"),
        arg(name = "before", ty = "Option<String>"),
        arg(name = "privacy", ty = "Option<RepositoryPrivacy>"),
        arg(name = "is_fork", ty = "Option<bool>"),
        arg(name = "owner_affiliations", ty = "Option<Vec<RepositoryAffiliation>>"),
        arg(name = "affiliations", ty = "Option<Vec<RepositoryAffiliation>>"),
        arg(name = "order_by", ty = "Option<RepositoryOrder>")
    )
)]
pub enum RepositoryOwner {
    User(GUser),
    Organization(GOrg),
}

impl RepositoryOwner {
    pub fn from_user(u: Arc<User>) -> RepositoryOwner {
        match u.kind.as_str() {
            "Organization" => RepositoryOwner::Organization(GOrg(u)),
            _ => RepositoryOwner::User(GUser(u)),
        }
    }
}

/// `Assignee` union (github.com actor APIs; also used by assignedActors).
#[derive(Union, Clone)]
pub enum Assignee {
    Bot(GBot),
    Mannequin(GMannequin),
    Organization(GOrg),
    User(GUser),
}

/// `RequestedReviewer` union. Includes `Bot` because gh always spreads `...on Bot`.
#[derive(Union, Clone)]
pub enum RequestedReviewer {
    User(GUser),
    Team(GTeam),
    Bot(GBot),
    Mannequin(GMannequin),
}

crate::gql_connection!(UserConnection, UserEdge, "UserConnection", "UserEdge", GUser);
crate::gql_connection!(OrganizationConnection, OrganizationEdge, "OrganizationConnection", "OrganizationEdge", GOrg);
crate::gql_connection!(TeamConnection, TeamEdge, "TeamConnection", "TeamEdge", GTeam);
crate::gql_connection!(AssigneeConnection, AssigneeEdge, "AssigneeConnection", "AssigneeEdge", Assignee);
crate::gql_connection!(ActorConnection, ActorEdge, "ActorConnection", "ActorEdge", Actor);

#[allow(clippy::too_many_arguments)]
#[Object(name = "User")]
impl GUser {
    async fn id(&self) -> ID {
        ID(ids::encode("User", self.0.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.0.id)
    }
    async fn login(&self) -> String {
        self.0.login.clone()
    }
    async fn name(&self) -> Option<String> {
        self.0.name.clone().filter(|n| !n.is_empty())
    }
    async fn email(&self) -> String {
        self.0.email.clone().unwrap_or_default()
    }
    async fn bio(&self) -> Option<String> {
        self.0.bio.clone()
    }
    async fn company(&self) -> Option<String> {
        self.0.company.clone()
    }
    async fn location(&self) -> Option<String> {
        self.0.location.clone()
    }
    async fn website_url(&self) -> Option<Uri> {
        Some(self.0.blog.clone()).filter(|b| !b.is_empty()).map(Uri)
    }
    async fn twitter_username(&self) -> Option<String> {
        self.0.twitter_username.clone()
    }
    async fn avatar_url(&self, ctx: &Context<'_>, size: Option<i32>) -> Uri {
        let _ = size;
        avatar(ctx, &self.0.login)
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/{}", self.0.login))
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}", base(ctx), self.0.login))
    }
    async fn created_at(&self) -> DateTime {
        dt(self.0.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.0.updated_at)
    }
    async fn is_site_admin(&self) -> bool {
        self.0.site_admin
    }
    async fn is_viewer(&self, ctx: &Context<'_>) -> bool {
        ctx.data_opt::<Viewer>().is_some_and(|v| v.0.id == self.0.id)
    }
    async fn is_employee(&self) -> bool {
        false
    }
    async fn is_hireable(&self) -> bool {
        false
    }
    async fn organizations(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> GResult<OrganizationConnection> {
        let st = ctx.data_unchecked::<AppState>();
        let orgs: Vec<User> = sqlx::query_as(
            "SELECT u.* FROM org_members m JOIN users u ON u.id = m.org_id WHERE m.user_id = $1 ORDER BY lower(u.login)",
        )
        .bind(self.0.id)
        .fetch_all(&st.db)
        .await?;
        let all = orgs.into_iter().map(|o| GOrg(Arc::new(o))).collect();
        Ok(OrganizationConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn repositories(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        privacy: Option<RepositoryPrivacy>,
        is_fork: Option<bool>,
        owner_affiliations: Option<Vec<RepositoryAffiliation>>,
        affiliations: Option<Vec<RepositoryAffiliation>>,
        order_by: Option<RepositoryOrder>,
    ) -> GResult<RepositoryConnection> {
        repo::list_repositories(
            ctx,
            &self.0,
            ConnArgs::new(first, after, last, before),
            privacy,
            is_fork,
            owner_affiliations,
            affiliations,
            order_by,
        )
        .await
    }
    async fn projects_v2(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
        order_by: Option<ProjectV2Order>,
    ) -> ProjectV2Connection {
        let _ = (first, after, last, before, query, order_by);
        ProjectV2Connection::empty()
    }
}

#[allow(clippy::too_many_arguments)]
#[Object(name = "Organization")]
impl GOrg {
    async fn id(&self) -> ID {
        ID(ids::encode("Organization", self.0.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.0.id)
    }
    async fn login(&self) -> String {
        self.0.login.clone()
    }
    async fn name(&self) -> Option<String> {
        self.0.name.clone()
    }
    async fn description(&self) -> Option<String> {
        self.0.bio.clone()
    }
    async fn email(&self) -> Option<String> {
        self.0.email.clone()
    }
    async fn location(&self) -> Option<String> {
        self.0.location.clone()
    }
    async fn website_url(&self) -> Option<Uri> {
        Some(self.0.blog.clone()).filter(|b| !b.is_empty()).map(Uri)
    }
    async fn avatar_url(&self, ctx: &Context<'_>, size: Option<i32>) -> Uri {
        let _ = size;
        avatar(ctx, &self.0.login)
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/{}", self.0.login))
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}", base(ctx), self.0.login))
    }
    async fn created_at(&self) -> DateTime {
        dt(self.0.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.0.updated_at)
    }
    async fn viewer_is_a_member(&self, ctx: &Context<'_>) -> GResult<bool> {
        let st = ctx.data_unchecked::<AppState>();
        let Some(v) = ctx.data_opt::<Viewer>() else { return Ok(false) };
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)")
            .bind(self.0.id)
            .bind(v.0.id)
            .fetch_one(&st.db)
            .await?)
    }
    async fn repositories(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        privacy: Option<RepositoryPrivacy>,
        is_fork: Option<bool>,
        owner_affiliations: Option<Vec<RepositoryAffiliation>>,
        affiliations: Option<Vec<RepositoryAffiliation>>,
        order_by: Option<RepositoryOrder>,
    ) -> GResult<RepositoryConnection> {
        repo::list_repositories(
            ctx,
            &self.0,
            ConnArgs::new(first, after, last, before),
            privacy,
            is_fork,
            owner_affiliations,
            affiliations,
            order_by,
        )
        .await
    }
    async fn projects_v2(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
        order_by: Option<ProjectV2Order>,
    ) -> ProjectV2Connection {
        let _ = (first, after, last, before, query, order_by);
        ProjectV2Connection::empty()
    }
    async fn projects(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        states: Option<Vec<super::enums::ProjectState>>,
        order_by: Option<super::enums::ProjectOrder>,
        search: Option<String>,
    ) -> repo::ProjectConnection {
        let _ = (first, after, last, before, states, order_by, search);
        repo::ProjectConnection::empty()
    }
    async fn teams(
        &self,
        first: Option<i32>,
        after: Option<String>,
        order_by: Option<TeamOrder>,
        query: Option<String>,
    ) -> TeamConnection {
        let _ = (first, after, order_by, query);
        TeamConnection::empty()
    }
    async fn team(&self, slug: String) -> Option<GTeam> {
        let _ = slug;
        None
    }
    async fn members_with_role(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> GResult<UserConnection> {
        let st = ctx.data_unchecked::<AppState>();
        let users: Vec<User> = sqlx::query_as(
            "SELECT u.* FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 ORDER BY lower(u.login)",
        )
        .bind(self.0.id)
        .fetch_all(&st.db)
        .await?;
        let all = users.into_iter().map(|u| GUser(Arc::new(u))).collect();
        Ok(UserConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
}

#[Object(name = "Bot")]
impl GBot {
    async fn id(&self) -> ID {
        ID(ids::encode("Bot", self.0.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.0.id)
    }
    async fn login(&self) -> String {
        self.0.login.clone()
    }
    async fn avatar_url(&self, ctx: &Context<'_>, size: Option<i32>) -> Uri {
        let _ = size;
        avatar(ctx, &self.0.login)
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/apps/{}", self.0.login))
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/apps/{}", base(ctx), self.0.login))
    }
    async fn created_at(&self) -> DateTime {
        dt(self.0.created_at)
    }
}

#[Object(name = "Mannequin")]
impl GMannequin {
    async fn id(&self) -> ID {
        ID(ids::encode("Mannequin", self.0.id))
    }
    async fn login(&self) -> String {
        self.0.login.clone()
    }
    async fn avatar_url(&self, ctx: &Context<'_>, size: Option<i32>) -> Uri {
        let _ = size;
        avatar(ctx, &self.0.login)
    }
    async fn resource_path(&self) -> Uri {
        Uri(format!("/{}", self.0.login))
    }
    async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}", base(ctx), self.0.login))
    }
}

#[Object(name = "Team")]
impl GTeam {
    async fn id(&self) -> ID {
        ID(ids::encode("Team", self.id))
    }
    async fn name(&self) -> String {
        self.name.clone()
    }
    async fn slug(&self) -> String {
        self.slug.clone()
    }
    async fn combined_slug(&self) -> String {
        format!("{}/{}", self.org.login, self.slug)
    }
    async fn organization(&self) -> GOrg {
        GOrg(self.org.clone())
    }
}
