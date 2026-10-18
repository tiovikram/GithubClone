//! GitHub Projects (v2) over GraphQL, the API behind `gh project ...`: the `ProjectV2` object and
//! its items, fields and field values, the owner entry points (`projectV2(number:)`,
//! `projectsV2`), `Issue/PullRequest.projectItems`, `Repository.projectsV2`, and the project
//! mutations. Thin adapters over `crate::api::projects_v2`.
use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::{Context, Enum, InputObject, Object, SimpleObject, Union, ID};
use chrono::NaiveDate;

use super::connection::ConnArgs;
use super::enums::{dt, dt_opt, DateTime, OrderDirection, ProjectV2Order, ProjectV2OrderField, Uri};
use super::issue::{self, GIssue, IssueOrPullRequest};
use super::loaders;
use super::pull::GPull;
use super::repo::{st, viewer, GLabel, GMilestone, GRepo, LabelConnection};
use super::user::{GOrg, GUser, TeamConnection, UserConnection};
use super::{api_err, gql_err, not_found, Viewer};
use crate::api::projects_v2::{self as svc, FieldRow, FieldValue, ItemRow, NewOption, ProjectPatch, ProjectRow};
use crate::db::User;
use crate::ids;
use crate::json::issue::{IssueRow, LabelRow, MilestoneRow};

fn actor<'a>(ctx: &'a Context<'_>) -> &'a User {
    &ctx.data_unchecked::<Viewer>().0
}

fn unresolvable(id: &str) -> async_graphql::Error {
    not_found(format!("Could not resolve to a node with the global id of '{id}'."))
}

fn dec(id: &str, kinds: &[&str]) -> async_graphql::Result<i64> {
    match ids::decode(id) {
        Some((k, v)) if kinds.contains(&k.as_str()) => v.parse().map_err(|_| unresolvable(id)),
        _ => Err(unresolvable(id)),
    }
}

const FIELD_KINDS: &[&str] = &["ProjectV2Field", "ProjectV2SingleSelectField", "ProjectV2IterationField"];

/// GitHub's `Date` scalar (`YYYY-MM-DD`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Date(pub String);
async_graphql::scalar!(Date, "Date", "An ISO-8601 encoded date string.");

fn parse_date(s: &str) -> async_graphql::Result<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").map_err(|_| gql_err("UNPROCESSABLE", format!("Invalid date: {s}")))
}

// ---------------------------------------------------------------------------------------------
// Enums

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectV2ItemType {
    Issue,
    PullRequest,
    DraftIssue,
    Redacted,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectV2FieldType {
    Assignees,
    Date,
    Iteration,
    Labels,
    LinkedPullRequests,
    Milestone,
    Number,
    Repository,
    Reviewers,
    SingleSelect,
    Text,
    Title,
    ParentIssue,
    SubIssuesProgress,
    TrackedBy,
    Tracks,
}

impl ProjectV2FieldType {
    fn parse(s: &str) -> Self {
        match s {
            "ASSIGNEES" => Self::Assignees,
            "DATE" => Self::Date,
            "ITERATION" => Self::Iteration,
            "LABELS" => Self::Labels,
            "LINKED_PULL_REQUESTS" => Self::LinkedPullRequests,
            "MILESTONE" => Self::Milestone,
            "NUMBER" => Self::Number,
            "REPOSITORY" => Self::Repository,
            "REVIEWERS" => Self::Reviewers,
            "SINGLE_SELECT" => Self::SingleSelect,
            "TITLE" => Self::Title,
            "PARENT_ISSUE" => Self::ParentIssue,
            "SUB_ISSUES_PROGRESS" => Self::SubIssuesProgress,
            "TRACKED_BY" => Self::TrackedBy,
            "TRACKS" => Self::Tracks,
            _ => Self::Text,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectV2CustomFieldType {
    Text,
    Number,
    Date,
    SingleSelect,
    Iteration,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectV2SingleSelectFieldOptionColor {
    Blue,
    Gray,
    Green,
    Orange,
    Pink,
    Purple,
    Red,
    Yellow,
}

impl ProjectV2SingleSelectFieldOptionColor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Blue => "BLUE",
            Self::Gray => "GRAY",
            Self::Green => "GREEN",
            Self::Orange => "ORANGE",
            Self::Pink => "PINK",
            Self::Purple => "PURPLE",
            Self::Red => "RED",
            Self::Yellow => "YELLOW",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "BLUE" => Self::Blue,
            "GREEN" => Self::Green,
            "ORANGE" => Self::Orange,
            "PINK" => Self::Pink,
            "PURPLE" => Self::Purple,
            "RED" => Self::Red,
            "YELLOW" => Self::Yellow,
            _ => Self::Gray,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ProjectV2

#[derive(Clone)]
pub struct ProjectV2 {
    pub row: Arc<ProjectRow>,
}

impl ProjectV2 {
    pub fn new(row: ProjectRow) -> Self {
        ProjectV2 { row: Arc::new(row) }
    }
}

#[derive(Union, Clone)]
#[graphql(name = "ProjectV2Owner")]
pub enum ProjectV2Owner {
    User(GUser),
    Organization(GOrg),
}

crate::gql_connection!(ProjectV2Connection, ProjectV2Edge, "ProjectV2Connection", "ProjectV2Edge", ProjectV2);
crate::gql_connection!(ProjectV2ItemConnection, ProjectV2ItemEdge, "ProjectV2ItemConnection", "ProjectV2ItemEdge", ProjectV2Item);
crate::gql_connection!(
    ProjectV2FieldConfigurationConnection,
    ProjectV2FieldConfigurationEdge,
    "ProjectV2FieldConfigurationConnection",
    "ProjectV2FieldConfigurationEdge",
    ProjectV2FieldConfiguration
);
crate::gql_connection!(
    ProjectV2ItemFieldValueConnection,
    ProjectV2ItemFieldValueEdge,
    "ProjectV2ItemFieldValueConnection",
    "ProjectV2ItemFieldValueEdge",
    ProjectV2ItemFieldValue
);
crate::gql_connection!(ProjectV2ViewConnection, ProjectV2ViewEdge, "ProjectV2ViewConnection", "ProjectV2ViewEdge", ProjectV2View);
crate::gql_connection!(RepositoryConnectionV2, RepositoryEdgeV2, "ProjectV2RepositoryConnection", "ProjectV2RepositoryEdge", GRepo);

async fn owner_of(ctx: &Context<'_>, owner_id: i64) -> async_graphql::Result<User> {
    crate::db::user_by_id(&st(ctx).db, owner_id).await.map_err(api_err)
}

#[Object(name = "ProjectV2")]
impl ProjectV2 {
    pub async fn id(&self) -> ID {
        ID(ids::encode("ProjectV2", self.row.id))
    }
    async fn database_id(&self) -> i64 {
        self.row.id
    }
    async fn number(&self) -> i32 {
        self.row.number as i32
    }
    async fn title(&self) -> &str {
        &self.row.title
    }
    async fn short_description(&self) -> Option<&str> {
        self.row.short_description.as_deref()
    }
    async fn readme(&self) -> Option<&str> {
        self.row.readme.as_deref()
    }
    async fn public(&self) -> bool {
        self.row.public
    }
    async fn closed(&self) -> bool {
        self.row.closed
    }
    async fn closed_at(&self) -> Option<DateTime> {
        dt_opt(self.row.closed_at)
    }
    async fn template(&self) -> bool {
        self.row.template
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    pub async fn resource_path(&self, ctx: &Context<'_>) -> async_graphql::Result<Uri> {
        let o = owner_of(ctx, self.row.owner_id).await?;
        let kind = if o.kind == "Organization" { "orgs" } else { "users" };
        Ok(Uri(format!("/{kind}/{}/projects/{}", o.login, self.row.number)))
    }
    pub async fn url(&self, ctx: &Context<'_>) -> async_graphql::Result<Uri> {
        let path = self.resource_path(ctx).await?;
        Ok(Uri(format!("{}{}", st(ctx).base_url(), path.0)))
    }
    async fn owner(&self, ctx: &Context<'_>) -> async_graphql::Result<ProjectV2Owner> {
        let o = Arc::new(owner_of(ctx, self.row.owner_id).await?);
        Ok(if o.kind == "Organization" { ProjectV2Owner::Organization(GOrg(o)) } else { ProjectV2Owner::User(GUser(o)) })
    }
    async fn creator(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GUser>> {
        Ok(loaders::user(ctx, self.row.creator_id).await?.map(GUser))
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        Ok(svc::access(st(ctx), viewer(ctx), &self.row).await.map_err(api_err)? == svc::Access::Write)
    }
    #[allow(clippy::too_many_arguments)]
    async fn items(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        query: Option<String>,
    ) -> async_graphql::Result<ProjectV2ItemConnection> {
        let all = project_items(ctx, &self.row, query.as_deref()).await?;
        Ok(ProjectV2ItemConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn fields(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<ProjectV2FieldConfigurationConnection> {
        let all = svc::fields(st(ctx), self.row.id).await.map_err(api_err)?.into_iter().map(field_config).collect();
        Ok(ProjectV2FieldConfigurationConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn field(&self, ctx: &Context<'_>, name: String) -> async_graphql::Result<Option<ProjectV2FieldConfiguration>> {
        let fields = svc::fields(st(ctx), self.row.id).await.map_err(api_err)?;
        Ok(fields.into_iter().find(|f| f.name.eq_ignore_ascii_case(&name)).map(field_config))
    }
    async fn repositories(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<RepositoryConnectionV2> {
        let mut all = Vec::new();
        for id in svc::linked_repo_ids(st(ctx), self.row.id).await.map_err(api_err)? {
            if let Some(r) = loaders::repo(ctx, id).await? {
                all.push(GRepo::new(r));
            }
        }
        Ok(RepositoryConnectionV2::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn teams(&self, first: Option<i32>, after: Option<String>) -> TeamConnection {
        let _ = (first, after);
        TeamConnection::empty()
    }
    /// The project's views: the implicit default table view "View 1" plus the views created
    /// through the REST API (`POST …/projectsV2/{number}/views`), by number.
    async fn views(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<ProjectV2ViewConnection> {
        let all = project_views(ctx, &self.row).await?;
        Ok(ProjectV2ViewConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn view(&self, ctx: &Context<'_>, number: i32) -> async_graphql::Result<Option<ProjectV2View>> {
        Ok(project_views(ctx, &self.row).await?.into_iter().find(|v| v.row.number == number as i64))
    }
}

// ---------------------------------------------------------------------------------------------
// Items

/// A project item with everything needed to render it.
#[derive(Clone)]
pub struct ProjectV2Item {
    pub row: Arc<ItemRow>,
    pub project: Arc<ProjectRow>,
    /// The issue or pull request (None for drafts, or content the viewer can't see).
    pub content: Option<IssueOrPullRequest>,
}

#[derive(Clone)]
pub struct GDraftIssue {
    pub item: Arc<ItemRow>,
}

#[Object(name = "DraftIssue")]
impl GDraftIssue {
    pub async fn id(&self) -> ID {
        ID(ids::encode("DraftIssue", self.item.id))
    }
    async fn title(&self) -> &str {
        self.item.draft_title.as_deref().unwrap_or("")
    }
    async fn body(&self) -> &str {
        self.item.draft_body.as_deref().unwrap_or("")
    }
    async fn body_text(&self) -> &str {
        self.item.draft_body.as_deref().unwrap_or("")
    }
    async fn created_at(&self) -> DateTime {
        dt(self.item.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.item.updated_at)
    }
    async fn creator(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GUser>> {
        Ok(loaders::user(ctx, self.item.creator_id).await?.map(GUser))
    }
    async fn assignees(&self, first: Option<i32>, after: Option<String>) -> UserConnection {
        let _ = (first, after);
        UserConnection::empty()
    }
}

#[derive(Union, Clone)]
#[graphql(name = "ProjectV2ItemContent")]
pub enum ProjectV2ItemContent {
    DraftIssue(GDraftIssue),
    Issue(GIssue),
    PullRequest(GPull),
}

async fn materialize_items(ctx: &Context<'_>, project: &Arc<ProjectRow>, rows: Vec<ItemRow>) -> async_graphql::Result<Vec<ProjectV2Item>> {
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let content = match r.issue_id {
            Some(iid) => issue::by_id(ctx, iid).await?,
            None => None,
        };
        out.push(ProjectV2Item { row: Arc::new(r), project: project.clone(), content });
    }
    Ok(out)
}

/// Items of a project, filtered GitHub-style by `query`: free text matches the title;
/// `is:archived` / `-is:archived`, `is:issue|pr|draft`, `is:open|closed` and `<field>:<value>`
/// (single-select option or text value, case-insensitive) are understood.
async fn project_items(ctx: &Context<'_>, project: &Arc<ProjectRow>, query: Option<&str>) -> async_graphql::Result<Vec<ProjectV2Item>> {
    let s = st(ctx);
    let rows = svc::items(s, project.id).await.map_err(api_err)?;
    let items = materialize_items(ctx, project, rows).await?;
    let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) else { return Ok(items) };
    let fields = svc::fields(s, project.id).await.map_err(api_err)?;
    let mut out = Vec::new();
    'items: for it in items {
        let values = svc::item_values(s, it.row.id).await.map_err(api_err)?;
        for term in tokenize(q) {
            let term = term.as_str();
            let (neg, term) = match term.strip_prefix('-') {
                Some(t) => (true, t),
                None => (false, term),
            };
            let hit = match term.split_once(':') {
                Some(("is", v)) => match v {
                    "archived" => it.row.archived,
                    "issue" => matches!(it.content, Some(IssueOrPullRequest::Issue(_))),
                    "pr" => matches!(it.content, Some(IssueOrPullRequest::PullRequest(_))),
                    "draft" => it.row.issue_id.is_none(),
                    "open" | "closed" => item_state(&it).is_some_and(|st| st == v),
                    _ => false,
                },
                Some((k, v)) => {
                    let v = v.trim_matches('"').to_lowercase();
                    fields.iter().filter(|f| f.name.eq_ignore_ascii_case(k)).any(|f| {
                        values.iter().find(|(fid, _)| *fid == f.id).is_some_and(|(_, val)| {
                            display_value(f, val).is_some_and(|d| d.to_lowercase() == v)
                        })
                    })
                }
                None => item_title(&it).to_lowercase().contains(&term.to_lowercase()),
            };
            if hit == neg {
                continue 'items;
            }
        }
        out.push(it);
    }
    Ok(out)
}

/// Splits a filter on whitespace, keeping `"quoted words"` together (quotes are dropped).
fn tokenize(q: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in q.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn item_title(it: &ProjectV2Item) -> String {
    match &it.content {
        Some(IssueOrPullRequest::Issue(i)) => i.row.title.clone(),
        Some(IssueOrPullRequest::PullRequest(p)) => p.row.title.clone(),
        None => it.row.draft_title.clone().unwrap_or_default(),
    }
}

fn item_state(it: &ProjectV2Item) -> Option<&str> {
    match &it.content {
        Some(IssueOrPullRequest::Issue(i)) => Some(i.row.state.as_str()),
        Some(IssueOrPullRequest::PullRequest(p)) => Some(p.row.state.as_str()),
        None => None,
    }
}

fn display_value(f: &FieldRow, v: &serde_json::Value) -> Option<String> {
    if let Some(t) = v["text"].as_str() {
        return Some(t.to_string());
    }
    if let Some(o) = v["option_id"].as_str() {
        return f.option_name(o);
    }
    if let Some(d) = v["date"].as_str() {
        return Some(d.to_string());
    }
    if let Some(n) = v["number"].as_f64() {
        return Some(n.to_string());
    }
    if let Some(i) = v["iteration_id"].as_str() {
        return f.iteration(i).and_then(|i| i["title"].as_str().map(str::to_string));
    }
    None
}

fn issue_row(it: &ProjectV2Item) -> Option<(&IssueRow, &Arc<crate::db::RepoCtx>)> {
    match &it.content {
        Some(IssueOrPullRequest::Issue(i)) => Some((&i.row, &i.repo)),
        Some(IssueOrPullRequest::PullRequest(p)) => Some((&p.row, &p.repo)),
        None => None,
    }
}

/// The values of all fields of an item: stored custom values plus the built-in values GitHub
/// derives from the content (Title, Assignees, Labels, Milestone, Repository). Empty values are
/// omitted like on GitHub.
async fn field_values(ctx: &Context<'_>, it: &ProjectV2Item) -> async_graphql::Result<Vec<ProjectV2ItemFieldValue>> {
    let s = st(ctx);
    let fields = svc::fields(s, it.project.id).await.map_err(api_err)?;
    let stored: HashMap<i64, serde_json::Value> = svc::item_values(s, it.row.id).await.map_err(api_err)?.into_iter().collect();
    let content = issue_row(it);
    let mut out = Vec::new();
    for f in fields {
        let cfg = field_config(f.clone());
        let item = it.row.id;
        let f = Arc::new(f);
        match f.data_type.as_str() {
            "TITLE" => out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldTextValue(ProjectV2ItemFieldTextValue {
                item,
                field: cfg,
                text: item_title(it),
            })),
            "ASSIGNEES" => {
                if let Some((row, _)) = content {
                    let users: Vec<User> = sqlx::query_as("SELECT u.* FROM issue_assignees a JOIN users u ON u.id = a.user_id WHERE a.issue_id = $1 ORDER BY lower(u.login)")
                        .bind(row.id)
                        .fetch_all(&s.db)
                        .await?;
                    if !users.is_empty() {
                        let users = users.into_iter().map(|u| GUser(Arc::new(u))).collect();
                        out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldUserValue(ProjectV2ItemFieldUserValue { field: cfg, users }));
                    }
                }
            }
            "LABELS" => {
                if let Some((row, repo)) = content {
                    let labels: Vec<LabelRow> = crate::api::issues::issue_labels(&s.db, row.id).await.map_err(api_err)?;
                    if !labels.is_empty() {
                        let labels = labels.into_iter().map(|l| GLabel::new(l, repo)).collect();
                        out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldLabelValue(ProjectV2ItemFieldLabelValue { field: cfg, labels }));
                    }
                }
            }
            "MILESTONE" => {
                if let Some((row, repo)) = content {
                    if let Some(mid) = row.milestone_id {
                        let m: Option<MilestoneRow> = sqlx::query_as("SELECT * FROM milestones WHERE id = $1").bind(mid).fetch_optional(&s.db).await?;
                        if let Some(m) = m {
                            out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldMilestoneValue(ProjectV2ItemFieldMilestoneValue {
                                field: cfg,
                                milestone: GMilestone { row: Arc::new(m), repo: repo.clone() },
                            }));
                        }
                    }
                }
            }
            "REPOSITORY" => {
                if let Some((_, repo)) = content {
                    out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldRepositoryValue(ProjectV2ItemFieldRepositoryValue {
                        field: cfg,
                        repository: GRepo::new(repo.clone()),
                    }));
                }
            }
            _ => {
                let Some(v) = stored.get(&f.id) else { continue };
                if let Some(t) = v["text"].as_str() {
                    out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldTextValue(ProjectV2ItemFieldTextValue { item, field: cfg, text: t.to_string() }));
                } else if let Some(n) = v["number"].as_f64() {
                    out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldNumberValue(ProjectV2ItemFieldNumberValue { item, field: cfg, number: n }));
                } else if let Some(d) = v["date"].as_str() {
                    out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldDateValue(ProjectV2ItemFieldDateValue { item, field: cfg, date: d.to_string() }));
                } else if let Some(o) = v["option_id"].as_str() {
                    if let Some(name) = f.option_name(o) {
                        out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldSingleSelectValue(ProjectV2ItemFieldSingleSelectValue {
                            item,
                            field: cfg,
                            option_id: o.to_string(),
                            name,
                        }));
                    }
                } else if let Some(i) = v["iteration_id"].as_str() {
                    if let Some(iter) = f.iteration(i) {
                        out.push(ProjectV2ItemFieldValue::ProjectV2ItemFieldIterationValue(ProjectV2ItemFieldIterationValue {
                            item,
                            field: cfg,
                            iteration_id: i.to_string(),
                            title: iter["title"].as_str().unwrap_or_default().to_string(),
                            start_date: iter["start_date"].as_str().unwrap_or_default().to_string(),
                            duration: iter["duration"].as_i64().unwrap_or(14) as i32,
                        }));
                    }
                }
            }
        }
    }
    Ok(out)
}

#[Object(name = "ProjectV2Item")]
impl ProjectV2Item {
    pub async fn id(&self) -> ID {
        ID(ids::encode("ProjectV2Item", self.row.id))
    }
    async fn database_id(&self) -> i64 {
        self.row.id
    }
    #[graphql(name = "type")]
    async fn item_type(&self) -> ProjectV2ItemType {
        match (&self.content, self.row.issue_id) {
            (_, None) => ProjectV2ItemType::DraftIssue,
            (Some(IssueOrPullRequest::Issue(_)), _) => ProjectV2ItemType::Issue,
            (Some(IssueOrPullRequest::PullRequest(_)), _) => ProjectV2ItemType::PullRequest,
            (None, Some(_)) => ProjectV2ItemType::Redacted,
        }
    }
    async fn is_archived(&self) -> bool {
        self.row.archived
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn creator(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GUser>> {
        Ok(loaders::user(ctx, self.row.creator_id).await?.map(GUser))
    }
    async fn project(&self) -> ProjectV2 {
        ProjectV2 { row: self.project.clone() }
    }
    async fn content(&self) -> Option<ProjectV2ItemContent> {
        match (&self.content, self.row.issue_id) {
            (_, None) => Some(ProjectV2ItemContent::DraftIssue(GDraftIssue { item: self.row.clone() })),
            (Some(IssueOrPullRequest::Issue(i)), _) => Some(ProjectV2ItemContent::Issue(i.clone())),
            (Some(IssueOrPullRequest::PullRequest(p)), _) => Some(ProjectV2ItemContent::PullRequest(p.clone())),
            (None, Some(_)) => None,
        }
    }
    async fn field_values(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<ProjectV2ItemFieldValueConnection> {
        let all = field_values(ctx, self).await?;
        Ok(ProjectV2ItemFieldValueConnection::from_all(all, &ConnArgs::new(first, after, last, before)))
    }
    async fn field_value_by_name(&self, ctx: &Context<'_>, name: String) -> async_graphql::Result<Option<ProjectV2ItemFieldValue>> {
        Ok(field_values(ctx, self).await?.into_iter().find(|v| v.field_name().eq_ignore_ascii_case(&name)))
    }
}

// ---------------------------------------------------------------------------------------------
// Fields

#[derive(Clone)]
pub struct GField(pub Arc<FieldRow>);

#[derive(Clone)]
pub struct GSingleSelectField(pub Arc<FieldRow>);

#[derive(Clone)]
pub struct GIterationField(pub Arc<FieldRow>);

#[derive(Union, Clone)]
#[graphql(name = "ProjectV2FieldConfiguration")]
pub enum ProjectV2FieldConfiguration {
    ProjectV2Field(GField),
    ProjectV2SingleSelectField(GSingleSelectField),
    ProjectV2IterationField(GIterationField),
}

impl ProjectV2FieldConfiguration {
    fn row(&self) -> &FieldRow {
        match self {
            Self::ProjectV2Field(f) => &f.0,
            Self::ProjectV2SingleSelectField(f) => &f.0,
            Self::ProjectV2IterationField(f) => &f.0,
        }
    }
}

pub fn field_config(f: FieldRow) -> ProjectV2FieldConfiguration {
    let f = Arc::new(f);
    match f.data_type.as_str() {
        "SINGLE_SELECT" => ProjectV2FieldConfiguration::ProjectV2SingleSelectField(GSingleSelectField(f)),
        "ITERATION" => ProjectV2FieldConfiguration::ProjectV2IterationField(GIterationField(f)),
        _ => ProjectV2FieldConfiguration::ProjectV2Field(GField(f)),
    }
}

fn field_id(f: &FieldRow) -> ID {
    ID(ids::encode(f.gql_kind(), f.id))
}

async fn field_project(ctx: &Context<'_>, f: &FieldRow) -> async_graphql::Result<ProjectV2> {
    let p = svc::project_by_id(st(ctx), f.project_id).await.map_err(api_err)?.ok_or_else(|| not_found("project not found"))?;
    Ok(ProjectV2::new(p))
}

#[Object(name = "ProjectV2Field")]
impl GField {
    pub async fn id(&self) -> ID {
        field_id(&self.0)
    }
    async fn database_id(&self) -> i64 {
        self.0.id
    }
    async fn name(&self) -> &str {
        &self.0.name
    }
    async fn data_type(&self) -> ProjectV2FieldType {
        ProjectV2FieldType::parse(&self.0.data_type)
    }
    async fn created_at(&self) -> DateTime {
        dt(self.0.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.0.updated_at)
    }
    async fn project(&self, ctx: &Context<'_>) -> async_graphql::Result<ProjectV2> {
        field_project(ctx, &self.0).await
    }
}

#[derive(SimpleObject, Clone)]
pub struct ProjectV2SingleSelectFieldOption {
    pub id: String,
    pub name: String,
    #[graphql(name = "nameHTML")]
    pub name_html: String,
    pub color: ProjectV2SingleSelectFieldOptionColor,
    pub description: String,
    #[graphql(name = "descriptionHTML")]
    pub description_html: String,
}

#[Object(name = "ProjectV2SingleSelectField")]
impl GSingleSelectField {
    pub async fn id(&self) -> ID {
        field_id(&self.0)
    }
    async fn database_id(&self) -> i64 {
        self.0.id
    }
    async fn name(&self) -> &str {
        &self.0.name
    }
    async fn data_type(&self) -> ProjectV2FieldType {
        ProjectV2FieldType::SingleSelect
    }
    async fn options(&self, names: Option<Vec<String>>) -> Vec<ProjectV2SingleSelectFieldOption> {
        let html = |s: &str| super::repo::html_escape(s);
        self.0
            .options
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|o| names.as_ref().is_none_or(|n| n.iter().any(|x| Some(x.as_str()) == o["name"].as_str())))
                    .map(|o| {
                        let name = o["name"].as_str().unwrap_or_default().to_string();
                        let description = o["description"].as_str().unwrap_or_default().to_string();
                        ProjectV2SingleSelectFieldOption {
                            id: o["id"].as_str().unwrap_or_default().to_string(),
                            name_html: html(&name),
                            description_html: html(&description),
                            name,
                            description,
                            color: ProjectV2SingleSelectFieldOptionColor::parse(o["color"].as_str().unwrap_or("GRAY")),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    async fn created_at(&self) -> DateTime {
        dt(self.0.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.0.updated_at)
    }
    async fn project(&self, ctx: &Context<'_>) -> async_graphql::Result<ProjectV2> {
        field_project(ctx, &self.0).await
    }
}

#[derive(SimpleObject, Clone)]
pub struct ProjectV2IterationFieldIteration {
    pub id: String,
    pub title: String,
    #[graphql(name = "titleHTML")]
    pub title_html: String,
    pub start_date: Date,
    pub duration: i32,
}

#[derive(SimpleObject, Clone)]
pub struct ProjectV2IterationFieldConfiguration {
    pub duration: i32,
    pub start_day: i32,
    pub iterations: Vec<ProjectV2IterationFieldIteration>,
    pub completed_iterations: Vec<ProjectV2IterationFieldIteration>,
}

#[Object(name = "ProjectV2IterationField")]
impl GIterationField {
    pub async fn id(&self) -> ID {
        field_id(&self.0)
    }
    async fn database_id(&self) -> i64 {
        self.0.id
    }
    async fn name(&self) -> &str {
        &self.0.name
    }
    async fn data_type(&self) -> ProjectV2FieldType {
        ProjectV2FieldType::Iteration
    }
    async fn configuration(&self) -> ProjectV2IterationFieldConfiguration {
        let c = &self.0.config;
        let iterations = c["iterations"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| {
                        let title = i["title"].as_str().unwrap_or_default().to_string();
                        ProjectV2IterationFieldIteration {
                            id: i["id"].as_str().unwrap_or_default().to_string(),
                            title_html: super::repo::html_escape(&title),
                            title,
                            start_date: Date(i["start_date"].as_str().unwrap_or_default().to_string()),
                            duration: i["duration"].as_i64().unwrap_or(14) as i32,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        ProjectV2IterationFieldConfiguration {
            duration: c["duration"].as_i64().unwrap_or(14) as i32,
            start_day: c["start_day"].as_i64().unwrap_or(1) as i32,
            iterations,
            completed_iterations: vec![],
        }
    }
    async fn created_at(&self) -> DateTime {
        dt(self.0.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.0.updated_at)
    }
    async fn project(&self, ctx: &Context<'_>) -> async_graphql::Result<ProjectV2> {
        field_project(ctx, &self.0).await
    }
}

// ---------------------------------------------------------------------------------------------
// Field values

#[derive(Union, Clone)]
#[graphql(name = "ProjectV2ItemFieldValue")]
pub enum ProjectV2ItemFieldValue {
    ProjectV2ItemFieldTextValue(ProjectV2ItemFieldTextValue),
    ProjectV2ItemFieldNumberValue(ProjectV2ItemFieldNumberValue),
    ProjectV2ItemFieldDateValue(ProjectV2ItemFieldDateValue),
    ProjectV2ItemFieldSingleSelectValue(ProjectV2ItemFieldSingleSelectValue),
    ProjectV2ItemFieldIterationValue(ProjectV2ItemFieldIterationValue),
    ProjectV2ItemFieldUserValue(ProjectV2ItemFieldUserValue),
    ProjectV2ItemFieldLabelValue(ProjectV2ItemFieldLabelValue),
    ProjectV2ItemFieldMilestoneValue(ProjectV2ItemFieldMilestoneValue),
    ProjectV2ItemFieldRepositoryValue(ProjectV2ItemFieldRepositoryValue),
}

impl ProjectV2ItemFieldValue {
    fn field_name(&self) -> &str {
        let f = match self {
            Self::ProjectV2ItemFieldTextValue(v) => &v.field,
            Self::ProjectV2ItemFieldNumberValue(v) => &v.field,
            Self::ProjectV2ItemFieldDateValue(v) => &v.field,
            Self::ProjectV2ItemFieldSingleSelectValue(v) => &v.field,
            Self::ProjectV2ItemFieldIterationValue(v) => &v.field,
            Self::ProjectV2ItemFieldUserValue(v) => &v.field,
            Self::ProjectV2ItemFieldLabelValue(v) => &v.field,
            Self::ProjectV2ItemFieldMilestoneValue(v) => &v.field,
            Self::ProjectV2ItemFieldRepositoryValue(v) => &v.field,
        };
        &f.row().name
    }
}

fn value_id(item: i64, field: &ProjectV2FieldConfiguration) -> ID {
    ID(ids::encode_str("ProjectV2ItemFieldValue", &format!("{item}:{}", field.row().id)))
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldTextValue {
    item: i64,
    field: ProjectV2FieldConfiguration,
    text: String,
}

#[Object]
impl ProjectV2ItemFieldTextValue {
    async fn id(&self) -> ID {
        value_id(self.item, &self.field)
    }
    async fn text(&self) -> &str {
        &self.text
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldNumberValue {
    item: i64,
    field: ProjectV2FieldConfiguration,
    number: f64,
}

#[Object]
impl ProjectV2ItemFieldNumberValue {
    async fn id(&self) -> ID {
        value_id(self.item, &self.field)
    }
    async fn number(&self) -> f64 {
        self.number
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldDateValue {
    item: i64,
    field: ProjectV2FieldConfiguration,
    date: String,
}

#[Object]
impl ProjectV2ItemFieldDateValue {
    async fn id(&self) -> ID {
        value_id(self.item, &self.field)
    }
    async fn date(&self) -> Date {
        Date(self.date.clone())
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldSingleSelectValue {
    item: i64,
    field: ProjectV2FieldConfiguration,
    option_id: String,
    name: String,
}

#[Object]
impl ProjectV2ItemFieldSingleSelectValue {
    async fn id(&self) -> ID {
        value_id(self.item, &self.field)
    }
    async fn name(&self) -> Option<&str> {
        Some(&self.name)
    }
    #[graphql(name = "nameHTML")]
    async fn name_html(&self) -> Option<String> {
        Some(super::repo::html_escape(&self.name))
    }
    async fn option_id(&self) -> Option<&str> {
        Some(&self.option_id)
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldIterationValue {
    item: i64,
    field: ProjectV2FieldConfiguration,
    iteration_id: String,
    title: String,
    start_date: String,
    duration: i32,
}

#[Object]
impl ProjectV2ItemFieldIterationValue {
    async fn id(&self) -> ID {
        value_id(self.item, &self.field)
    }
    async fn iteration_id(&self) -> &str {
        &self.iteration_id
    }
    async fn title(&self) -> &str {
        &self.title
    }
    #[graphql(name = "titleHTML")]
    async fn title_html(&self) -> String {
        super::repo::html_escape(&self.title)
    }
    async fn start_date(&self) -> Date {
        Date(self.start_date.clone())
    }
    async fn duration(&self) -> i32 {
        self.duration
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldUserValue {
    field: ProjectV2FieldConfiguration,
    users: Vec<GUser>,
}

#[Object]
impl ProjectV2ItemFieldUserValue {
    async fn users(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> UserConnection {
        UserConnection::from_all(self.users.clone(), &ConnArgs::new(first, after, last, before))
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldLabelValue {
    field: ProjectV2FieldConfiguration,
    labels: Vec<GLabel>,
}

#[Object]
impl ProjectV2ItemFieldLabelValue {
    async fn labels(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> LabelConnection {
        LabelConnection::from_all(self.labels.clone(), &ConnArgs::new(first, after, last, before))
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldMilestoneValue {
    field: ProjectV2FieldConfiguration,
    milestone: GMilestone,
}

#[Object]
impl ProjectV2ItemFieldMilestoneValue {
    async fn milestone(&self) -> Option<GMilestone> {
        Some(self.milestone.clone())
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

#[derive(Clone)]
pub struct ProjectV2ItemFieldRepositoryValue {
    field: ProjectV2FieldConfiguration,
    repository: GRepo,
}

#[Object]
impl ProjectV2ItemFieldRepositoryValue {
    async fn repository(&self) -> Option<GRepo> {
        Some(self.repository.clone())
    }
    async fn field(&self) -> ProjectV2FieldConfiguration {
        self.field.clone()
    }
}

// ---------------------------------------------------------------------------------------------
// Entry points used by the owner/repository/issue types and `node`

/// `User|Organization.projectV2(number:)` (NOT_FOUND error when missing, like GitHub).
pub async fn project_for_owner(ctx: &Context<'_>, owner: &User, number: i32) -> async_graphql::Result<Option<ProjectV2>> {
    match svc::by_number(st(ctx), viewer(ctx), owner.id, number as i64).await.map_err(api_err)? {
        Some(p) => Ok(Some(ProjectV2::new(p))),
        None => Err(not_found(format!("Could not resolve to a ProjectV2 with the number {number}."))),
    }
}

fn sort_projects(rows: &mut [ProjectRow], order_by: Option<ProjectV2Order>) {
    let Some(o) = order_by else { return };
    match o.field {
        ProjectV2OrderField::Title => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        ProjectV2OrderField::Number => rows.sort_by_key(|p| p.number),
        ProjectV2OrderField::UpdatedAt => rows.sort_by_key(|p| p.updated_at),
        ProjectV2OrderField::CreatedAt => rows.sort_by_key(|p| p.created_at),
    }
    if o.direction == OrderDirection::Desc {
        rows.reverse();
    }
}

/// `User|Organization.projectsV2`.
pub async fn projects_for_owner(
    ctx: &Context<'_>,
    owner: &User,
    query: Option<String>,
    order_by: Option<ProjectV2Order>,
    args: ConnArgs,
) -> async_graphql::Result<ProjectV2Connection> {
    let mut rows = svc::list_for_owner(st(ctx), viewer(ctx), owner, query.as_deref()).await.map_err(api_err)?;
    sort_projects(&mut rows, order_by);
    Ok(ProjectV2Connection::from_all(rows.into_iter().map(ProjectV2::new).collect(), &args))
}

/// `Repository.projectsV2`: projects linked to the repository.
pub async fn projects_for_repo(
    ctx: &Context<'_>,
    repo_id: i64,
    query: Option<String>,
    order_by: Option<ProjectV2Order>,
    args: ConnArgs,
) -> async_graphql::Result<ProjectV2Connection> {
    let mut rows = svc::list_for_repo(st(ctx), viewer(ctx), repo_id).await.map_err(api_err)?;
    if let Some(q) = query.map(|q| q.to_lowercase()).filter(|q| !q.is_empty()) {
        rows.retain(|p| p.title.to_lowercase().contains(&q));
    }
    sort_projects(&mut rows, order_by);
    Ok(ProjectV2Connection::from_all(rows.into_iter().map(ProjectV2::new).collect(), &args))
}

/// `Issue|PullRequest.projectItems`: the content's items in projects the viewer can see.
pub async fn items_for_issue(ctx: &Context<'_>, issue_id: i64, include_archived: bool, args: ConnArgs) -> async_graphql::Result<ProjectV2ItemConnection> {
    let s = st(ctx);
    let mut all = Vec::new();
    for r in svc::items_for_issue(s, issue_id).await.map_err(api_err)? {
        if r.archived && !include_archived {
            continue;
        }
        if let Some(p) = svc::readable(s, viewer(ctx), r.project_id).await.map_err(api_err)? {
            all.extend(materialize_items(ctx, &Arc::new(p), vec![r]).await?);
        }
    }
    Ok(ProjectV2ItemConnection::from_all(all, &args))
}

/// `Organization.viewerCanCreateProjects` / `User.viewerCanCreateProjects`.
pub async fn viewer_can_create(ctx: &Context<'_>, owner: &User) -> async_graphql::Result<bool> {
    Ok(svc::can_create(st(ctx), actor(ctx), owner).await.map_err(api_err)?)
}

pub async fn node_project(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<ProjectV2>> {
    Ok(svc::readable(st(ctx), viewer(ctx), id).await.map_err(api_err)?.map(ProjectV2::new))
}

pub async fn node_item(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<ProjectV2Item>> {
    let s = st(ctx);
    let Some(r) = svc::item_by_id(s, id).await.map_err(api_err)? else { return Ok(None) };
    let Some(p) = svc::readable(s, viewer(ctx), r.project_id).await.map_err(api_err)? else { return Ok(None) };
    Ok(materialize_items(ctx, &Arc::new(p), vec![r]).await?.pop())
}

pub async fn node_draft(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GDraftIssue>> {
    Ok(node_item(ctx, id).await?.filter(|i| i.row.issue_id.is_none()).map(|i| GDraftIssue { item: i.row }))
}

/// Something addressable by URL (`Query.resource(url:)`).
#[derive(Union, Clone)]
#[graphql(name = "UniformResourceLocatable")]
pub enum UniformResourceLocatable {
    Issue(GIssue),
    PullRequest(GPull),
    Repository(GRepo),
}

/// `Query.resource(url:)`: `…/owner/repo`, `…/owner/repo/issues/N` or `…/owner/repo/pull/N`.
pub async fn resource(ctx: &Context<'_>, url: &str) -> async_graphql::Result<Option<UniformResourceLocatable>> {
    let path = url.split_once("://").map(|(_, rest)| rest.split_once('/').map(|(_, p)| p).unwrap_or("")).unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let (owner, name) = match parts.as_slice() {
        [o, r, ..] => (*o, r.trim_end_matches(".git")),
        _ => return Ok(None),
    };
    let Ok(rc) = crate::db::load_repo(&st(ctx).db, viewer(ctx), owner, name).await else { return Ok(None) };
    match parts.as_slice() {
        [_, _] => Ok(Some(UniformResourceLocatable::Repository(GRepo::new(Arc::new(rc))))),
        [_, _, kind @ ("issues" | "pull"), n, ..] => {
            let Ok(n) = n.parse::<i64>() else { return Ok(None) };
            let Ok(row) = crate::json::issue::issue_by_number(&st(ctx).db, rc.repo.id, n).await else { return Ok(None) };
            if (*kind == "pull") != row.is_pull && *kind == "pull" {
                return Ok(None);
            }
            Ok(match issue::by_id(ctx, row.id).await? {
                Some(IssueOrPullRequest::Issue(i)) => Some(UniformResourceLocatable::Issue(i)),
                Some(IssueOrPullRequest::PullRequest(p)) => Some(UniformResourceLocatable::PullRequest(p)),
                None => None,
            })
        }
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------------------------
// Mutations

async fn owner_by_node(ctx: &Context<'_>, id: &ID) -> async_graphql::Result<User> {
    let uid = dec(id, &["User", "Organization"])?;
    crate::db::user_by_id(&st(ctx).db, uid).await.map_err(|_| unresolvable(id))
}

fn project_id(id: &ID) -> async_graphql::Result<i64> {
    dec(id, &["ProjectV2"])
}

async fn reload_item(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<ProjectV2Item>> {
    node_item(ctx, id).await
}

#[derive(InputObject)]
pub struct CreateProjectV2Input {
    pub owner_id: ID,
    pub title: String,
    pub repository_id: Option<ID>,
    pub team_id: Option<ID>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct CreateProjectV2Payload {
    pub client_mutation_id: Option<String>,
    pub project_v2: Option<ProjectV2>,
}

#[derive(InputObject)]
pub struct UpdateProjectV2Input {
    pub project_id: ID,
    pub title: Option<String>,
    pub short_description: Option<String>,
    pub readme: Option<String>,
    pub public: Option<bool>,
    pub closed: Option<bool>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UpdateProjectV2Payload {
    pub client_mutation_id: Option<String>,
    pub project_v2: Option<ProjectV2>,
}

#[derive(InputObject)]
pub struct DeleteProjectV2Input {
    pub project_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct DeleteProjectV2Payload {
    pub client_mutation_id: Option<String>,
    pub project_v2: Option<ProjectV2>,
}

#[derive(InputObject)]
pub struct CopyProjectV2Input {
    pub project_id: ID,
    pub owner_id: ID,
    pub title: String,
    pub include_draft_issues: Option<bool>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct CopyProjectV2Payload {
    pub client_mutation_id: Option<String>,
    pub project_v2: Option<ProjectV2>,
}

#[derive(InputObject)]
pub struct MarkProjectV2AsTemplateInput {
    pub project_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct MarkProjectV2AsTemplatePayload {
    pub client_mutation_id: Option<String>,
    pub project_v2: Option<ProjectV2>,
}

#[derive(InputObject)]
pub struct UnmarkProjectV2AsTemplateInput {
    pub project_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UnmarkProjectV2AsTemplatePayload {
    pub client_mutation_id: Option<String>,
    pub project_v2: Option<ProjectV2>,
}

#[derive(InputObject)]
pub struct LinkProjectV2ToRepositoryInput {
    pub project_id: ID,
    pub repository_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct LinkProjectV2ToRepositoryPayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(InputObject)]
pub struct UnlinkProjectV2FromRepositoryInput {
    pub project_id: ID,
    pub repository_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UnlinkProjectV2FromRepositoryPayload {
    pub client_mutation_id: Option<String>,
    pub repository: Option<GRepo>,
}

#[derive(InputObject)]
pub struct LinkProjectV2ToTeamInput {
    pub project_id: ID,
    pub team_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct LinkProjectV2ToTeamPayload {
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct UnlinkProjectV2FromTeamInput {
    pub project_id: ID,
    pub team_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UnlinkProjectV2FromTeamPayload {
    pub client_mutation_id: Option<String>,
}

#[derive(InputObject)]
pub struct ProjectV2SingleSelectFieldOptionInput {
    pub name: String,
    pub color: ProjectV2SingleSelectFieldOptionColor,
    pub description: Option<String>,
}

#[derive(InputObject)]
pub struct ProjectV2IterationFieldConfigurationInput {
    pub duration: i32,
    pub start_date: Date,
    pub iterations: Option<Vec<ProjectV2IterationInput>>,
}

#[derive(InputObject)]
pub struct ProjectV2IterationInput {
    pub title: String,
    pub start_date: Date,
    pub duration: i32,
}

#[derive(InputObject)]
pub struct CreateProjectV2FieldInput {
    pub project_id: ID,
    pub data_type: ProjectV2CustomFieldType,
    pub name: String,
    pub single_select_options: Option<Vec<ProjectV2SingleSelectFieldOptionInput>>,
    pub iteration_configuration: Option<ProjectV2IterationFieldConfigurationInput>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct CreateProjectV2FieldPayload {
    pub client_mutation_id: Option<String>,
    pub project_v2_field: Option<ProjectV2FieldConfiguration>,
}

#[derive(InputObject)]
pub struct DeleteProjectV2FieldInput {
    pub field_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct DeleteProjectV2FieldPayload {
    pub client_mutation_id: Option<String>,
    pub project_v2_field: Option<ProjectV2FieldConfiguration>,
}

#[derive(InputObject)]
pub struct AddProjectV2ItemByIdInput {
    pub project_id: ID,
    pub content_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct AddProjectV2ItemByIdPayload {
    pub client_mutation_id: Option<String>,
    pub item: Option<ProjectV2Item>,
}

#[derive(InputObject)]
pub struct AddProjectV2DraftIssueInput {
    pub project_id: ID,
    pub title: String,
    pub body: Option<String>,
    pub assignee_ids: Option<Vec<ID>>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct AddProjectV2DraftIssuePayload {
    pub client_mutation_id: Option<String>,
    pub project_item: Option<ProjectV2Item>,
}

#[derive(InputObject)]
pub struct UpdateProjectV2DraftIssueInput {
    pub draft_issue_id: ID,
    pub title: Option<String>,
    pub body: Option<String>,
    pub assignee_ids: Option<Vec<ID>>,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UpdateProjectV2DraftIssuePayload {
    pub client_mutation_id: Option<String>,
    pub draft_issue: Option<GDraftIssue>,
}

#[derive(InputObject)]
pub struct ProjectV2FieldValue {
    pub text: Option<String>,
    pub number: Option<f64>,
    pub date: Option<Date>,
    pub single_select_option_id: Option<String>,
    pub iteration_id: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateProjectV2ItemFieldValueInput {
    pub project_id: ID,
    pub item_id: ID,
    pub field_id: ID,
    pub value: ProjectV2FieldValue,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UpdateProjectV2ItemFieldValuePayload {
    pub client_mutation_id: Option<String>,
    pub project_v2_item: Option<ProjectV2Item>,
}

#[derive(InputObject)]
pub struct ClearProjectV2ItemFieldValueInput {
    pub project_id: ID,
    pub item_id: ID,
    pub field_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct ClearProjectV2ItemFieldValuePayload {
    pub client_mutation_id: Option<String>,
    pub project_v2_item: Option<ProjectV2Item>,
}

#[derive(InputObject)]
pub struct ArchiveProjectV2ItemInput {
    pub project_id: ID,
    pub item_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct ArchiveProjectV2ItemPayload {
    pub client_mutation_id: Option<String>,
    pub item: Option<ProjectV2Item>,
}

#[derive(InputObject)]
pub struct UnarchiveProjectV2ItemInput {
    pub project_id: ID,
    pub item_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct UnarchiveProjectV2ItemPayload {
    pub client_mutation_id: Option<String>,
    pub item: Option<ProjectV2Item>,
}

#[derive(InputObject)]
pub struct DeleteProjectV2ItemInput {
    pub project_id: ID,
    pub item_id: ID,
    pub client_mutation_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct DeleteProjectV2ItemPayload {
    pub client_mutation_id: Option<String>,
    pub deleted_item_id: Option<ID>,
}

#[derive(Default)]
pub struct ProjectsMutation;

#[Object]
impl ProjectsMutation {
    async fn create_project_v2(&self, ctx: &Context<'_>, input: CreateProjectV2Input) -> async_graphql::Result<Option<CreateProjectV2Payload>> {
        if input.team_id.is_some() {
            return Err(gql_err("NOT_FOUND", "Could not resolve to a Team: teams are not supported."));
        }
        let owner = owner_by_node(ctx, &input.owner_id).await?;
        let s = st(ctx);
        let p = svc::create_project(s, actor(ctx), &owner, &input.title).await.map_err(api_err)?;
        if let Some(rid) = &input.repository_id {
            let repo_id = dec(rid, &["Repository"])?;
            let rc = loaders::repo(ctx, repo_id).await?.ok_or_else(|| unresolvable(rid))?;
            svc::set_repo_link(s, actor(ctx), p.id, &rc, true).await.map_err(api_err)?;
        }
        Ok(Some(CreateProjectV2Payload { client_mutation_id: input.client_mutation_id, project_v2: Some(ProjectV2::new(p)) }))
    }

    async fn update_project_v2(&self, ctx: &Context<'_>, input: UpdateProjectV2Input) -> async_graphql::Result<Option<UpdateProjectV2Payload>> {
        let pid = project_id(&input.project_id)?;
        let patch = ProjectPatch {
            title: input.title,
            short_description: input.short_description,
            readme: input.readme,
            public: input.public,
            closed: input.closed,
            template: None,
        };
        let p = svc::update_project(st(ctx), actor(ctx), pid, patch).await.map_err(api_err)?;
        Ok(Some(UpdateProjectV2Payload { client_mutation_id: input.client_mutation_id, project_v2: Some(ProjectV2::new(p)) }))
    }

    async fn delete_project_v2(&self, ctx: &Context<'_>, input: DeleteProjectV2Input) -> async_graphql::Result<Option<DeleteProjectV2Payload>> {
        let pid = project_id(&input.project_id)?;
        let p = svc::delete_project(st(ctx), actor(ctx), pid).await.map_err(api_err)?;
        Ok(Some(DeleteProjectV2Payload { client_mutation_id: input.client_mutation_id, project_v2: Some(ProjectV2::new(p)) }))
    }

    async fn copy_project_v2(&self, ctx: &Context<'_>, input: CopyProjectV2Input) -> async_graphql::Result<Option<CopyProjectV2Payload>> {
        let pid = project_id(&input.project_id)?;
        let owner = owner_by_node(ctx, &input.owner_id).await?;
        let p = svc::copy_project(st(ctx), actor(ctx), pid, &owner, &input.title, input.include_draft_issues.unwrap_or(false))
            .await
            .map_err(api_err)?;
        Ok(Some(CopyProjectV2Payload { client_mutation_id: input.client_mutation_id, project_v2: Some(ProjectV2::new(p)) }))
    }

    async fn mark_project_v2_as_template(&self, ctx: &Context<'_>, input: MarkProjectV2AsTemplateInput) -> async_graphql::Result<Option<MarkProjectV2AsTemplatePayload>> {
        let pid = project_id(&input.project_id)?;
        let p = svc::update_project(st(ctx), actor(ctx), pid, ProjectPatch { template: Some(true), ..Default::default() }).await.map_err(api_err)?;
        Ok(Some(MarkProjectV2AsTemplatePayload { client_mutation_id: input.client_mutation_id, project_v2: Some(ProjectV2::new(p)) }))
    }

    async fn unmark_project_v2_as_template(
        &self,
        ctx: &Context<'_>,
        input: UnmarkProjectV2AsTemplateInput,
    ) -> async_graphql::Result<Option<UnmarkProjectV2AsTemplatePayload>> {
        let pid = project_id(&input.project_id)?;
        let p = svc::update_project(st(ctx), actor(ctx), pid, ProjectPatch { template: Some(false), ..Default::default() }).await.map_err(api_err)?;
        Ok(Some(UnmarkProjectV2AsTemplatePayload { client_mutation_id: input.client_mutation_id, project_v2: Some(ProjectV2::new(p)) }))
    }

    async fn link_project_v2_to_repository(
        &self,
        ctx: &Context<'_>,
        input: LinkProjectV2ToRepositoryInput,
    ) -> async_graphql::Result<Option<LinkProjectV2ToRepositoryPayload>> {
        let pid = project_id(&input.project_id)?;
        let repo_id = dec(&input.repository_id, &["Repository"])?;
        let rc = loaders::repo(ctx, repo_id).await?.ok_or_else(|| unresolvable(&input.repository_id))?;
        svc::set_repo_link(st(ctx), actor(ctx), pid, &rc, true).await.map_err(api_err)?;
        Ok(Some(LinkProjectV2ToRepositoryPayload { client_mutation_id: input.client_mutation_id, repository: Some(GRepo::new(rc)) }))
    }

    async fn unlink_project_v2_from_repository(
        &self,
        ctx: &Context<'_>,
        input: UnlinkProjectV2FromRepositoryInput,
    ) -> async_graphql::Result<Option<UnlinkProjectV2FromRepositoryPayload>> {
        let pid = project_id(&input.project_id)?;
        let repo_id = dec(&input.repository_id, &["Repository"])?;
        let rc = loaders::repo(ctx, repo_id).await?.ok_or_else(|| unresolvable(&input.repository_id))?;
        svc::set_repo_link(st(ctx), actor(ctx), pid, &rc, false).await.map_err(api_err)?;
        Ok(Some(UnlinkProjectV2FromRepositoryPayload { client_mutation_id: input.client_mutation_id, repository: Some(GRepo::new(rc)) }))
    }

    async fn link_project_v2_to_team(&self, input: LinkProjectV2ToTeamInput) -> async_graphql::Result<Option<LinkProjectV2ToTeamPayload>> {
        let _ = input;
        Err(gql_err("NOT_FOUND", "Could not resolve to a Team: teams are not supported."))
    }

    async fn unlink_project_v2_from_team(&self, input: UnlinkProjectV2FromTeamInput) -> async_graphql::Result<Option<UnlinkProjectV2FromTeamPayload>> {
        let _ = input;
        Err(gql_err("NOT_FOUND", "Could not resolve to a Team: teams are not supported."))
    }

    async fn create_project_v2_field(&self, ctx: &Context<'_>, input: CreateProjectV2FieldInput) -> async_graphql::Result<Option<CreateProjectV2FieldPayload>> {
        let pid = project_id(&input.project_id)?;
        let data_type = match input.data_type {
            ProjectV2CustomFieldType::Text => "TEXT",
            ProjectV2CustomFieldType::Number => "NUMBER",
            ProjectV2CustomFieldType::Date => "DATE",
            ProjectV2CustomFieldType::SingleSelect => "SINGLE_SELECT",
            ProjectV2CustomFieldType::Iteration => "ITERATION",
        };
        let options = input
            .single_select_options
            .unwrap_or_default()
            .into_iter()
            .map(|o| NewOption { name: o.name, color: o.color.as_str().to_string(), description: o.description.unwrap_or_default() })
            .collect();
        let (duration, start) = match &input.iteration_configuration {
            Some(c) => (Some(c.duration as i64), Some(parse_date(&c.start_date.0)?)),
            None => (None, None),
        };
        let f = svc::create_field(st(ctx), actor(ctx), pid, &input.name, data_type, options, duration, start).await.map_err(api_err)?;
        Ok(Some(CreateProjectV2FieldPayload { client_mutation_id: input.client_mutation_id, project_v2_field: Some(field_config(f)) }))
    }

    async fn delete_project_v2_field(&self, ctx: &Context<'_>, input: DeleteProjectV2FieldInput) -> async_graphql::Result<Option<DeleteProjectV2FieldPayload>> {
        let fid = dec(&input.field_id, FIELD_KINDS)?;
        let f = svc::delete_field(st(ctx), actor(ctx), fid).await.map_err(api_err)?;
        Ok(Some(DeleteProjectV2FieldPayload { client_mutation_id: input.client_mutation_id, project_v2_field: Some(field_config(f)) }))
    }

    async fn add_project_v2_item_by_id(&self, ctx: &Context<'_>, input: AddProjectV2ItemByIdInput) -> async_graphql::Result<Option<AddProjectV2ItemByIdPayload>> {
        let pid = project_id(&input.project_id)?;
        let iid = dec(&input.content_id, &["Issue", "PullRequest"])?;
        // The viewer must be able to see the content.
        let content = issue::by_id(ctx, iid).await?.ok_or_else(|| unresolvable(&input.content_id))?;
        let row = match &content {
            IssueOrPullRequest::Issue(i) => i.row.clone(),
            IssueOrPullRequest::PullRequest(p) => p.row.clone(),
        };
        let item = svc::add_issue(st(ctx), actor(ctx), pid, &row).await.map_err(api_err)?;
        Ok(Some(AddProjectV2ItemByIdPayload { client_mutation_id: input.client_mutation_id, item: reload_item(ctx, item.id).await? }))
    }

    async fn add_project_v2_draft_issue(&self, ctx: &Context<'_>, input: AddProjectV2DraftIssueInput) -> async_graphql::Result<Option<AddProjectV2DraftIssuePayload>> {
        let pid = project_id(&input.project_id)?;
        let item = svc::add_draft(st(ctx), actor(ctx), pid, &input.title, input.body.as_deref()).await.map_err(api_err)?;
        Ok(Some(AddProjectV2DraftIssuePayload { client_mutation_id: input.client_mutation_id, project_item: reload_item(ctx, item.id).await? }))
    }

    async fn update_project_v2_draft_issue(
        &self,
        ctx: &Context<'_>,
        input: UpdateProjectV2DraftIssueInput,
    ) -> async_graphql::Result<Option<UpdateProjectV2DraftIssuePayload>> {
        let did = dec(&input.draft_issue_id, &["DraftIssue"])?;
        let item = svc::update_draft(st(ctx), actor(ctx), did, input.title.as_deref(), input.body.as_deref()).await.map_err(api_err)?;
        Ok(Some(UpdateProjectV2DraftIssuePayload { client_mutation_id: input.client_mutation_id, draft_issue: Some(GDraftIssue { item: Arc::new(item) }) }))
    }

    async fn update_project_v2_item_field_value(
        &self,
        ctx: &Context<'_>,
        input: UpdateProjectV2ItemFieldValueInput,
    ) -> async_graphql::Result<Option<UpdateProjectV2ItemFieldValuePayload>> {
        let pid = project_id(&input.project_id)?;
        let item_id = dec(&input.item_id, &["ProjectV2Item"])?;
        let field_id = dec(&input.field_id, FIELD_KINDS)?;
        let v = input.value;
        let given = [v.text.is_some(), v.number.is_some(), v.date.is_some(), v.single_select_option_id.is_some(), v.iteration_id.is_some()];
        if given.iter().filter(|g| **g).count() != 1 {
            return Err(gql_err("UNPROCESSABLE", "Exactly one value type must be provided."));
        }
        let value = if let Some(t) = v.text {
            FieldValue::Text(t)
        } else if let Some(n) = v.number {
            FieldValue::Number(n)
        } else if let Some(d) = v.date {
            FieldValue::Date(parse_date(&d.0)?)
        } else if let Some(o) = v.single_select_option_id {
            FieldValue::SingleSelect(o)
        } else {
            FieldValue::Iteration(v.iteration_id.unwrap_or_default())
        };
        let item = svc::set_value(st(ctx), actor(ctx), pid, item_id, field_id, value).await.map_err(api_err)?;
        Ok(Some(UpdateProjectV2ItemFieldValuePayload { client_mutation_id: input.client_mutation_id, project_v2_item: reload_item(ctx, item.id).await? }))
    }

    async fn clear_project_v2_item_field_value(
        &self,
        ctx: &Context<'_>,
        input: ClearProjectV2ItemFieldValueInput,
    ) -> async_graphql::Result<Option<ClearProjectV2ItemFieldValuePayload>> {
        let pid = project_id(&input.project_id)?;
        let item_id = dec(&input.item_id, &["ProjectV2Item"])?;
        let field_id = dec(&input.field_id, FIELD_KINDS)?;
        let item = svc::clear_value(st(ctx), actor(ctx), pid, item_id, field_id).await.map_err(api_err)?;
        Ok(Some(ClearProjectV2ItemFieldValuePayload { client_mutation_id: input.client_mutation_id, project_v2_item: reload_item(ctx, item.id).await? }))
    }

    async fn archive_project_v2_item(&self, ctx: &Context<'_>, input: ArchiveProjectV2ItemInput) -> async_graphql::Result<Option<ArchiveProjectV2ItemPayload>> {
        let pid = project_id(&input.project_id)?;
        let item_id = dec(&input.item_id, &["ProjectV2Item"])?;
        let item = svc::set_archived(st(ctx), actor(ctx), pid, item_id, true).await.map_err(api_err)?;
        Ok(Some(ArchiveProjectV2ItemPayload { client_mutation_id: input.client_mutation_id, item: reload_item(ctx, item.id).await? }))
    }

    async fn unarchive_project_v2_item(&self, ctx: &Context<'_>, input: UnarchiveProjectV2ItemInput) -> async_graphql::Result<Option<UnarchiveProjectV2ItemPayload>> {
        let pid = project_id(&input.project_id)?;
        let item_id = dec(&input.item_id, &["ProjectV2Item"])?;
        let item = svc::set_archived(st(ctx), actor(ctx), pid, item_id, false).await.map_err(api_err)?;
        Ok(Some(UnarchiveProjectV2ItemPayload { client_mutation_id: input.client_mutation_id, item: reload_item(ctx, item.id).await? }))
    }

    async fn delete_project_v2_item(&self, ctx: &Context<'_>, input: DeleteProjectV2ItemInput) -> async_graphql::Result<Option<DeleteProjectV2ItemPayload>> {
        let pid = project_id(&input.project_id)?;
        let item_id = dec(&input.item_id, &["ProjectV2Item"])?;
        svc::delete_item(st(ctx), actor(ctx), pid, item_id).await.map_err(api_err)?;
        Ok(Some(DeleteProjectV2ItemPayload { client_mutation_id: input.client_mutation_id, deleted_item_id: Some(input.item_id) }))
    }
}

// ---------------------------------------------------------------------------------------------
// Views (created by the REST API into `projects_v2_views`; view 1 is implicit)

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProjectV2ViewLayout {
    BoardLayout,
    TableLayout,
    RoadmapLayout,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ViewRow {
    pub id: i64,
    pub number: i64,
    pub name: String,
    pub layout: String,
    pub filter: Option<String>,
    pub visible_fields: serde_json::Value,
    pub group_by: serde_json::Value,
    pub vertical_group_by: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
pub struct ProjectV2View {
    row: Arc<ViewRow>,
    project: Arc<ProjectRow>,
}

async fn project_views(ctx: &Context<'_>, project: &Arc<ProjectRow>) -> async_graphql::Result<Vec<ProjectV2View>> {
    let rows: Vec<ViewRow> = sqlx::query_as(
        "SELECT id, number, name, layout, filter, visible_fields, group_by, vertical_group_by, created_at, updated_at
         FROM projects_v2_views WHERE project_id = $1 ORDER BY number",
    )
    .bind(project.id)
    .fetch_all(&st(ctx).db)
    .await
    .map_err(|e| api_err(e.into()))?;
    let mut out = Vec::with_capacity(rows.len() + 1);
    if !rows.iter().any(|v| v.number == 1) {
        // The implicit default view has no row (id 0).
        out.push(ViewRow {
            id: 0,
            number: 1,
            name: "View 1".into(),
            layout: "table".into(),
            filter: None,
            visible_fields: serde_json::json!([]),
            group_by: serde_json::json!([]),
            vertical_group_by: serde_json::json!([]),
            created_at: project.created_at,
            updated_at: project.updated_at,
        });
    }
    out.extend(rows);
    Ok(out.into_iter().map(|row| ProjectV2View { row: Arc::new(row), project: project.clone() }).collect())
}

impl ProjectV2View {
    async fn fields_in(&self, ctx: &Context<'_>, ids: &serde_json::Value, first: Option<i32>, after: Option<String>) -> async_graphql::Result<ProjectV2FieldConfigurationConnection> {
        let want: Vec<i64> = ids.as_array().into_iter().flatten().filter_map(|v| v.as_i64()).collect();
        let fields = svc::fields(st(ctx), self.project.id).await.map_err(api_err)?;
        let all = want.iter().filter_map(|id| fields.iter().find(|f| f.id == *id).cloned()).map(field_config).collect();
        Ok(ProjectV2FieldConfigurationConnection::from_all(all, &ConnArgs::new(first, after, None, None)))
    }
}

#[Object(name = "ProjectV2View")]
impl ProjectV2View {
    async fn id(&self) -> ID {
        if self.row.id == 0 {
            ID(ids::encode_str("ProjectV2View", &format!("{}:1", self.project.id)))
        } else {
            ID(ids::encode("ProjectV2View", self.row.id))
        }
    }
    async fn database_id(&self) -> i64 {
        self.row.id
    }
    async fn number(&self) -> i32 {
        self.row.number as i32
    }
    async fn name(&self) -> &str {
        &self.row.name
    }
    async fn layout(&self) -> ProjectV2ViewLayout {
        match self.row.layout.as_str() {
            "board" => ProjectV2ViewLayout::BoardLayout,
            "roadmap" => ProjectV2ViewLayout::RoadmapLayout,
            _ => ProjectV2ViewLayout::TableLayout,
        }
    }
    async fn filter(&self) -> Option<&str> {
        self.row.filter.as_deref()
    }
    async fn created_at(&self) -> DateTime {
        dt(self.row.created_at)
    }
    async fn updated_at(&self) -> DateTime {
        dt(self.row.updated_at)
    }
    async fn project(&self) -> ProjectV2 {
        ProjectV2 { row: self.project.clone() }
    }
    async fn visible_fields(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>) -> async_graphql::Result<ProjectV2FieldConfigurationConnection> {
        self.fields_in(ctx, &self.row.visible_fields, first, after).await
    }
    async fn group_by_fields(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>) -> async_graphql::Result<ProjectV2FieldConfigurationConnection> {
        self.fields_in(ctx, &self.row.group_by, first, after).await
    }
    async fn vertical_group_by_fields(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>) -> async_graphql::Result<ProjectV2FieldConfigurationConnection> {
        self.fields_in(ctx, &self.row.vertical_group_by, first, after).await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn filter_tokens_keep_quoted_words() {
        assert_eq!(super::tokenize(r#"status:"In Progress" -is:archived  bug"#), vec!["status:In Progress", "-is:archived", "bug"]);
    }
}
