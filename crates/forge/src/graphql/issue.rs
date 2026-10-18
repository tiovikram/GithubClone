//! Issue, IssueComment, reactions, and the shared issue/PR list query engine (also used by search).
use std::sync::Arc;

use async_graphql::{Context, Object, Union, ID};
use chrono::Utc;
use sqlx::{Postgres, QueryBuilder};

use super::connection::{ConnArgs, Window};
use super::enums::*;
use super::loaders::{self, lerr, loader, Assoc, AssigneesOf, CommentReactions, IssueReactions, LabelsOf, MilestoneId, PullOf, RepoId};
use super::pull::{GPull, PullRequestConnection};
use super::repo::{body_html, html_escape, st, viewer, GLabel, GMilestone, GRepo, LabelConnection, ProjectCardConnection, ProjectV2ItemConnection};
use super::user::{Actor, Assignee, AssigneeConnection, GUser, UserConnection};
use crate::db::{Perm, RepoCtx};
use crate::ids;
use crate::json::issue::IssueRow;

#[derive(Clone)]
pub struct GIssue {
    pub row: Arc<IssueRow>,
    pub repo: Arc<RepoCtx>,
}

#[derive(Union, Clone)]
pub enum IssueOrPullRequest {
    Issue(GIssue),
    PullRequest(GPull),
}

crate::gql_connection!(IssueConnection, IssueEdge, "IssueConnection", "IssueEdge", GIssue);
crate::gql_connection!(IssueCommentConnection, IssueCommentEdge, "IssueCommentConnection", "IssueCommentEdge", GComment);

pub use crate::json::issue::CommentRow;

/// Wraps rows into Issue/PullRequest objects (batched repo + PR loads).
pub async fn materialize(ctx: &Context<'_>, rows: Vec<IssueRow>) -> async_graphql::Result<Vec<IssueOrPullRequest>> {
    let ld = loader(ctx);
    let repos = ld.load_many(rows.iter().map(|r| RepoId(r.repo_id))).await.map_err(lerr)?;
    let pulls = ld.load_many(rows.iter().filter(|r| r.is_pull).map(|r| PullOf(r.id))).await.map_err(lerr)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(repo) = repos.get(&RepoId(row.repo_id)).cloned() else { continue };
        if row.is_pull {
            let Some(pr) = pulls.get(&PullOf(row.id)).cloned() else { continue };
            out.push(IssueOrPullRequest::PullRequest(GPull::new(Arc::new(row), pr, repo)));
        } else {
            out.push(IssueOrPullRequest::Issue(GIssue { row: Arc::new(row), repo }));
        }
    }
    Ok(out)
}

pub async fn by_number(ctx: &Context<'_>, repo: &Arc<RepoCtx>, number: i64) -> async_graphql::Result<Option<IssueOrPullRequest>> {
    let row: Option<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE repo_id = $1 AND number = $2")
        .bind(repo.repo.id)
        .bind(number)
        .fetch_optional(&st(ctx).db)
        .await?;
    Ok(materialize(ctx, row.into_iter().collect()).await?.pop())
}

/// Loads an Issue or PullRequest by database id, checking repository visibility.
pub async fn by_id(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<IssueOrPullRequest>> {
    let row = loader(ctx).load_one(loaders::IssueId(id)).await.map_err(lerr)?;
    match row {
        Some(r) => Ok(materialize(ctx, vec![(*r).clone()]).await?.pop()),
        None => Ok(None),
    }
}

/// Filters shared by repository.issues / pullRequests and search.
#[derive(Default, Clone, Debug)]
pub struct IssueQuery {
    pub repo_id: Option<i64>,
    /// Restrict search to these repos (owner/name pairs resolved by caller), or by owner login.
    pub repo_ids: Option<Vec<i64>>,
    pub owner_logins: Vec<String>,
    pub is_pull: Option<bool>,
    /// Any of OPEN / CLOSED / MERGED.
    pub states: Option<Vec<&'static str>>,
    pub labels_any: Vec<String>,
    pub labels_all: Vec<String>,
    pub no_label: bool,
    pub assignee: Option<String>,
    pub no_assignee: bool,
    pub author: Option<String>,
    pub mentioned: Option<String>,
    pub milestone: Option<String>,
    pub no_milestone: bool,
    pub since: Option<chrono::DateTime<Utc>>,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub draft: Option<bool>,
    pub review_requested: Option<String>,
    pub reviewed_by: Option<String>,
    pub keywords: Vec<String>,
    pub numbers: Vec<i64>,
    /// ORDER BY column expression and direction.
    pub order: Option<(&'static str, &'static str)>,
    /// When set, only rows in repositories this user can see (search across repos).
    pub visible_to: Option<i64>,
    pub impossible: bool,
}

impl IssueQuery {
    fn push_where(&self, qb: &mut QueryBuilder<Postgres>) {
        qb.push(" FROM issues i LEFT JOIN pull_requests p ON p.issue_id = i.id JOIN repositories r ON r.id = i.repo_id WHERE TRUE");
        if self.impossible {
            qb.push(" AND FALSE");
        }
        if let Some(id) = self.repo_id {
            qb.push(" AND i.repo_id = ").push_bind(id);
        }
        if let Some(ids) = &self.repo_ids {
            qb.push(" AND i.repo_id = ANY(").push_bind(ids.clone()).push(")");
        }
        if !self.owner_logins.is_empty() {
            let lower: Vec<String> = self.owner_logins.iter().map(|s| s.to_lowercase()).collect();
            qb.push(" AND r.owner_id IN (SELECT id FROM users WHERE lower(login) = ANY(").push_bind(lower).push("))");
        }
        if let Some(v) = self.visible_to {
            qb.push(" AND forge_repo_readable(r.id, ").push_bind(v).push(")");
        }
        if let Some(p) = self.is_pull {
            qb.push(" AND i.is_pull = ").push_bind(p);
        }
        if let Some(states) = &self.states {
            let v: Vec<String> = states.iter().map(|s| s.to_string()).collect();
            qb.push(" AND ((i.state = 'open' AND 'OPEN' = ANY(")
                .push_bind(v.clone())
                .push(")) OR (i.state = 'closed' AND NOT COALESCE(p.merged, FALSE) AND 'CLOSED' = ANY(")
                .push_bind(v.clone())
                .push(")) OR (COALESCE(p.merged, FALSE) AND 'MERGED' = ANY(")
                .push_bind(v)
                .push(")))");
        }
        if !self.labels_any.is_empty() {
            let v: Vec<String> = self.labels_any.iter().map(|s| s.to_lowercase()).collect();
            qb.push(" AND EXISTS(SELECT 1 FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = i.id AND lower(l.name) = ANY(")
                .push_bind(v)
                .push("))");
        }
        for l in &self.labels_all {
            qb.push(" AND EXISTS(SELECT 1 FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = i.id AND lower(l.name) = lower(")
                .push_bind(l.clone())
                .push("))");
        }
        if self.no_label {
            qb.push(" AND NOT EXISTS(SELECT 1 FROM issue_labels il WHERE il.issue_id = i.id)");
        }
        match self.assignee.as_deref() {
            Some("*") => {
                qb.push(" AND EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id)");
            }
            Some("none") => {
                qb.push(" AND NOT EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id)");
            }
            Some(a) => {
                qb.push(" AND EXISTS(SELECT 1 FROM issue_assignees a JOIN users u ON u.id = a.user_id WHERE a.issue_id = i.id AND lower(u.login) = lower(")
                    .push_bind(a.to_string())
                    .push("))");
            }
            None => {}
        }
        if self.no_assignee {
            qb.push(" AND NOT EXISTS(SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id)");
        }
        if let Some(a) = &self.author {
            let a = a.strip_prefix("app/").unwrap_or(a).to_string();
            qb.push(" AND EXISTS(SELECT 1 FROM users u WHERE u.id = i.user_id AND lower(u.login) = lower(").push_bind(a).push("))");
        }
        if let Some(m) = &self.mentioned {
            let pat = format!("%@{}%", super::repo::like_escape(m));
            qb.push(" AND (i.body ILIKE ")
                .push_bind(pat.clone())
                .push(" OR EXISTS(SELECT 1 FROM issue_comments ic WHERE ic.issue_id = i.id AND ic.body ILIKE ")
                .push_bind(pat)
                .push("))");
        }
        match self.milestone.as_deref() {
            Some("*") => {
                qb.push(" AND i.milestone_id IS NOT NULL");
            }
            Some("none") => {
                qb.push(" AND i.milestone_id IS NULL");
            }
            Some(m) => {
                qb.push(" AND EXISTS(SELECT 1 FROM milestones ms WHERE ms.id = i.milestone_id AND (ms.number::text = ")
                    .push_bind(m.to_string())
                    .push(" OR ms.id::text = ")
                    .push_bind(m.to_string())
                    .push(" OR lower(ms.title) = lower(")
                    .push_bind(m.to_string())
                    .push(")))");
            }
            None => {}
        }
        if self.no_milestone {
            qb.push(" AND i.milestone_id IS NULL");
        }
        if let Some(t) = self.since {
            qb.push(" AND i.updated_at >= ").push_bind(t);
        }
        if let Some(h) = &self.head_ref {
            qb.push(" AND p.head_ref = ").push_bind(h.clone());
        }
        if let Some(b) = &self.base_ref {
            qb.push(" AND p.base_ref = ").push_bind(b.clone());
        }
        if let Some(d) = self.draft {
            qb.push(" AND COALESCE(p.draft, FALSE) = ").push_bind(d);
        }
        if let Some(rr) = &self.review_requested {
            qb.push(" AND EXISTS(SELECT 1 FROM pull_review_requests rq JOIN users u ON u.id = rq.user_id WHERE rq.issue_id = i.id AND lower(u.login) = lower(")
                .push_bind(rr.clone())
                .push("))");
        }
        if let Some(rb) = &self.reviewed_by {
            qb.push(" AND EXISTS(SELECT 1 FROM pull_reviews pr2 JOIN users u ON u.id = pr2.user_id WHERE pr2.issue_id = i.id AND lower(u.login) = lower(")
                .push_bind(rb.clone())
                .push("))");
        }
        for k in &self.keywords {
            let pat = format!("%{}%", super::repo::like_escape(k));
            qb.push(" AND (i.title ILIKE ").push_bind(pat.clone()).push(" OR i.body ILIKE ").push_bind(pat).push(")");
        }
        if !self.numbers.is_empty() {
            qb.push(" AND i.number = ANY(").push_bind(self.numbers.clone()).push(")");
        }
    }

    /// Runs the query for a connection window; returns rows and the window.
    pub async fn run(&self, ctx: &Context<'_>, args: &ConnArgs) -> async_graphql::Result<(Vec<IssueRow>, Window)> {
        let db = &st(ctx).db;
        let mut qc = QueryBuilder::<Postgres>::new("SELECT count(*)");
        self.push_where(&mut qc);
        let total: i64 = qc.build_query_scalar().fetch_one(db).await?;
        let w = args.window(total as usize);
        let (col, dir) = self.order.unwrap_or(("i.created_at", "ASC"));
        let mut q = QueryBuilder::<Postgres>::new("SELECT i.*");
        self.push_where(&mut q);
        q.push(format!(" ORDER BY {col} {dir}, i.id {dir} OFFSET {} LIMIT {}", w.offset(), w.limit()));
        let rows: Vec<IssueRow> = q.build_query_as().fetch_all(db).await?;
        Ok((rows, w))
    }
}

pub fn issue_order(o: Option<IssueOrder>) -> Option<(&'static str, &'static str)> {
    o.map(|o| {
        (
            match o.field {
                IssueOrderField::CreatedAt => "i.created_at",
                IssueOrderField::UpdatedAt => "i.updated_at",
                IssueOrderField::Comments => "(SELECT count(*) FROM issue_comments c WHERE c.issue_id = i.id)",
            },
            o.direction.sql(),
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn list_issues(
    ctx: &Context<'_>,
    repo: &Arc<RepoCtx>,
    args: ConnArgs,
    order_by: Option<IssueOrder>,
    states: Option<Vec<IssueState>>,
    labels: Option<Vec<String>>,
    filter_by: Option<IssueFilters>,
) -> async_graphql::Result<IssueConnection> {
    let f = filter_by.unwrap_or_default();
    let states = states.or(f.states.clone()).map(|v| {
        v.iter()
            .map(|s| match s {
                IssueState::Open => "OPEN",
                IssueState::Closed => "CLOSED",
            })
            .collect()
    });
    let mut labels_any = labels.unwrap_or_default();
    labels_any.extend(f.labels.clone().unwrap_or_default());
    let viewer_login = viewer(ctx).map(|v| v.login.clone());
    let me = |s: Option<String>| s.map(|x| if x == "@me" { viewer_login.clone().unwrap_or(x) } else { x });
    let q = IssueQuery {
        repo_id: Some(repo.repo.id),
        is_pull: Some(false),
        states,
        labels_any,
        assignee: me(f.assignee.clone()),
        author: me(f.created_by.clone()),
        mentioned: me(f.mentioned.clone()),
        milestone: f.milestone_number.clone().or(f.milestone.clone()),
        since: f.since.as_ref().and_then(|d| chrono::DateTime::parse_from_rfc3339(&d.0).ok()).map(|d| d.with_timezone(&Utc)),
        order: issue_order(order_by),
        ..Default::default()
    };
    let (rows, w) = q.run(ctx, &args).await?;
    let items = rows.into_iter().map(|row| GIssue { row: Arc::new(row), repo: repo.clone() }).collect();
    Ok(IssueConnection::new(items, w))
}

// ---------------------------------------------------------------------------------------------
// Field helpers shared by Issue and PullRequest

pub fn html_url(ctx: &Context<'_>, repo: &RepoCtx, row: &IssueRow) -> String {
    format!(
        "{}/{}/{}/{}",
        st(ctx).base_url(),
        repo.full_name(),
        if row.is_pull { "pull" } else { "issues" },
        row.number
    )
}

pub async fn author(ctx: &Context<'_>, row: &IssueRow) -> async_graphql::Result<Option<Actor>> {
    Ok(loaders::user(ctx, row.user_id).await?.map(Actor::from_user))
}

pub async fn association(ctx: &Context<'_>, repo_id: i64, user_id: Option<i64>) -> async_graphql::Result<CommentAuthorAssociation> {
    let Some(uid) = user_id else { return Ok(CommentAuthorAssociation::NoAssociation) };
    let a = loader(ctx).load_one(Assoc(repo_id, uid)).await.map_err(lerr)?;
    Ok(CommentAuthorAssociation::parse(a.as_deref().unwrap_or("NONE")))
}

pub async fn assignees(ctx: &Context<'_>, row: &IssueRow, args: ConnArgs) -> async_graphql::Result<UserConnection> {
    let users = loader(ctx).load_one(AssigneesOf(row.id)).await.map_err(lerr)?.unwrap_or_default();
    let all = users.iter().cloned().map(GUser).collect();
    Ok(UserConnection::from_all(all, &args))
}

pub async fn assigned_actors(ctx: &Context<'_>, row: &IssueRow, args: ConnArgs) -> async_graphql::Result<AssigneeConnection> {
    let users = loader(ctx).load_one(AssigneesOf(row.id)).await.map_err(lerr)?.unwrap_or_default();
    let all = users.iter().cloned().map(|u| Assignee::User(GUser(u))).collect();
    Ok(AssigneeConnection::from_all(all, &args))
}

pub async fn labels(ctx: &Context<'_>, row: &IssueRow, repo: &RepoCtx, args: ConnArgs, order: Option<LabelOrder>) -> async_graphql::Result<Option<LabelConnection>> {
    let ls = loader(ctx).load_one(LabelsOf(row.id)).await.map_err(lerr)?.unwrap_or_default();
    let mut all: Vec<GLabel> = ls.iter().cloned().map(|l| GLabel::new(l, repo)).collect();
    if let Some(o) = order {
        if o.field == LabelOrderField::CreatedAt {
            all.sort_by_key(|l| (l.row.created_at, l.row.id));
        }
        if o.direction == OrderDirection::Desc {
            all.reverse();
        }
    }
    Ok(Some(LabelConnection::from_all(all, &args)))
}

pub async fn milestone(ctx: &Context<'_>, row: &IssueRow, repo: &Arc<RepoCtx>) -> async_graphql::Result<Option<GMilestone>> {
    let Some(mid) = row.milestone_id else { return Ok(None) };
    Ok(loader(ctx)
        .load_one(MilestoneId(mid))
        .await
        .map_err(lerr)?
        .map(|m| GMilestone { row: m, repo: repo.clone() }))
}

pub async fn comments(ctx: &Context<'_>, row: &IssueRow, repo: &Arc<RepoCtx>, args: ConnArgs) -> async_graphql::Result<IssueCommentConnection> {
    let db = &st(ctx).db;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_comments WHERE issue_id = $1").bind(row.id).fetch_one(db).await?;
    let w = args.window(total as usize);
    let rows: Vec<CommentRow> = sqlx::query_as(
        "SELECT id, issue_id, repo_id, user_id, body, created_at, updated_at FROM issue_comments
          WHERE issue_id = $1 ORDER BY id OFFSET $2 LIMIT $3",
    )
    .bind(row.id)
    .bind(w.offset())
    .bind(w.limit())
    .fetch_all(db)
    .await?;
    let items = rows
        .into_iter()
        .map(|c| GComment { row: Arc::new(c), repo: repo.clone(), number: row.number, is_pull: row.is_pull })
        .collect();
    Ok(IssueCommentConnection::new(items, w))
}

pub async fn reaction_groups(ctx: &Context<'_>, counts: Option<Arc<std::collections::HashMap<String, i64>>>) -> Vec<ReactionGroup> {
    let _ = ctx;
    let counts = counts.unwrap_or_default();
    ReactionContent::ALL
        .iter()
        .map(|(c, k)| ReactionGroup { content: *c, count: counts.get(*k).copied().unwrap_or(0) })
        .collect()
}

pub async fn issue_reaction_groups(ctx: &Context<'_>, row: &IssueRow) -> async_graphql::Result<Vec<ReactionGroup>> {
    let counts = loader(ctx).load_one(IssueReactions(row.id)).await.map_err(lerr)?;
    Ok(reaction_groups(ctx, counts).await)
}

pub fn state_reason(row: &IssueRow, enable_duplicate: bool) -> Option<IssueStateReason> {
    match row.state_reason.as_deref()? {
        "completed" => Some(IssueStateReason::Completed),
        "not_planned" => Some(IssueStateReason::NotPlanned),
        "reopened" => Some(IssueStateReason::Reopened),
        "duplicate" if enable_duplicate => Some(IssueStateReason::Duplicate),
        "duplicate" => Some(IssueStateReason::NotPlanned),
        _ => None,
    }
}

pub fn viewer_did_author(ctx: &Context<'_>, user_id: Option<i64>) -> bool {
    viewer(ctx).is_some_and(|v| Some(v.id) == user_id)
}

#[allow(clippy::too_many_arguments)]
#[Object(name = "Issue")]
impl GIssue {
    pub async fn id(&self) -> ID {
        ID(ids::encode("Issue", self.row.id))
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
    async fn title_html(&self) -> String {
        html_escape(&self.row.title)
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
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(html_url(ctx, &self.repo, &self.row))
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(format!("/{}/issues/{}", self.repo.full_name(), self.row.number))
    }
    async fn state(&self) -> IssueState {
        if self.row.state == "closed" { IssueState::Closed } else { IssueState::Open }
    }
    async fn state_reason(&self, #[graphql(default = false)] enable_duplicate: bool) -> Option<IssueStateReason> {
        state_reason(&self.row, enable_duplicate)
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
    async fn author(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Actor>> {
        author(ctx, &self.row).await
    }
    async fn editor(&self) -> Option<Actor> {
        None
    }
    async fn author_association(&self, ctx: &Context<'_>) -> async_graphql::Result<CommentAuthorAssociation> {
        association(ctx, self.repo.repo.id, self.row.user_id).await
    }
    async fn includes_created_edit(&self) -> bool {
        false
    }
    async fn assignees(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> async_graphql::Result<UserConnection> {
        assignees(ctx, &self.row, ConnArgs::new(first, after, last, before)).await
    }
    async fn assigned_actors(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> async_graphql::Result<AssigneeConnection> {
        assigned_actors(ctx, &self.row, ConnArgs::new(first, after, last, before)).await
    }
    async fn labels(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<LabelOrder>,
    ) -> async_graphql::Result<Option<LabelConnection>> {
        labels(ctx, &self.row, &self.repo, ConnArgs::new(first, after, last, before), order_by).await
    }
    async fn milestone(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<GMilestone>> {
        milestone(ctx, &self.row, &self.repo).await
    }
    async fn comments(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        order_by: Option<IssueCommentOrder>,
    ) -> async_graphql::Result<IssueCommentConnection> {
        let _ = order_by;
        comments(ctx, &self.row, &self.repo, ConnArgs::new(first, after, last, before)).await
    }
    async fn reaction_groups(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Vec<ReactionGroup>>> {
        Ok(Some(issue_reaction_groups(ctx, &self.row).await?))
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
    pub async fn locked(&self) -> bool {
        self.row.locked
    }
    pub async fn active_lock_reason(&self) -> Option<LockReason> {
        self.row.active_lock_reason.as_deref().and_then(LockReason::parse)
    }
    async fn is_pinned(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<bool>> {
        super::ghcompat::is_pinned(ctx, &self.row).await
    }
    async fn linked_branches(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<super::ghcompat::LinkedBranchConnection> {
        super::ghcompat::linked_branches(ctx, &self.row, ConnArgs::new(first, after, last, before)).await
    }
    async fn issue_type(&self) -> Option<IssueType> {
        None
    }
    async fn parent(&self) -> Option<GIssue> {
        None
    }
    async fn sub_issues(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> IssueConnection {
        let _ = (first, after, last, before);
        IssueConnection::empty()
    }
    async fn sub_issues_summary(&self) -> SubIssuesSummary {
        SubIssuesSummary
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
    #[allow(clippy::too_many_arguments)]
    async fn project_items(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        #[graphql(default = true)] include_archived: bool,
    ) -> async_graphql::Result<ProjectV2ItemConnection> {
        let args = super::connection::ConnArgs::new(first, after, last, before);
        super::projects::items_for_issue(ctx, self.row.id, include_archived, args).await
    }
    async fn closed_by_pull_requests_references(
        &self,
        first: Option<i32>,
        after: Option<String>,
        last: Option<i32>,
        before: Option<String>,
        #[graphql(default = false)] include_closed_prs: bool,
        #[graphql(default = false)] order_by_state: bool,
        #[graphql(default = false)] user_linked_only: bool,
    ) -> Option<PullRequestConnection> {
        let _ = (first, after, last, before, include_closed_prs, order_by_state, user_linked_only);
        Some(PullRequestConnection::empty())
    }
    async fn viewer_did_author(&self, ctx: &Context<'_>) -> bool {
        viewer_did_author(ctx, self.row.user_id)
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> bool {
        self.repo.perm >= Perm::Triage || viewer_did_author(ctx, self.row.user_id)
    }
    async fn viewer_can_react(&self) -> bool {
        true
    }
    async fn viewer_subscription(&self) -> Option<SubscriptionState> {
        Some(SubscriptionState::Unsubscribed)
    }
    async fn participants(&self, ctx: &Context<'_>, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> async_graphql::Result<UserConnection> {
        participants(ctx, &self.row, ConnArgs::new(first, after, last, before)).await
    }
}

pub async fn participants(ctx: &Context<'_>, row: &IssueRow, args: ConnArgs) -> async_graphql::Result<UserConnection> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT uid FROM (SELECT user_id AS uid FROM issues WHERE id = $1
           UNION SELECT user_id FROM issue_comments WHERE issue_id = $1) x WHERE uid IS NOT NULL",
    )
    .bind(row.id)
    .fetch_all(&st(ctx).db)
    .await?;
    let m = loader(ctx).load_many(ids.iter().map(|i| loaders::UserId(*i))).await.map_err(lerr)?;
    let mut all: Vec<GUser> = m.into_values().map(GUser).collect();
    all.sort_by_key(|u| u.0.login.to_lowercase());
    Ok(UserConnection::from_all(all, &args))
}

pub struct SubIssuesSummary;

#[Object]
impl SubIssuesSummary {
    async fn total(&self) -> i32 {
        0
    }
    async fn completed(&self) -> i32 {
        0
    }
    async fn percent_completed(&self) -> i32 {
        0
    }
}

/// Issue types are not implemented; `issueType` is always null.
pub struct IssueType;

#[Object]
impl IssueType {
    pub async fn id(&self) -> ID {
        ID(String::new())
    }
    async fn name(&self) -> String {
        String::new()
    }
    async fn description(&self) -> Option<String> {
        None
    }
    async fn color(&self) -> IssueTypeColor {
        IssueTypeColor::Gray
    }
}

#[derive(Clone)]
pub struct ReactionGroup {
    pub content: ReactionContent,
    pub count: i64,
}

#[Object]
impl ReactionGroup {
    async fn content(&self) -> ReactionContent {
        self.content
    }
    async fn users(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> ReactingUserConnection {
        let _ = (first, after, last, before);
        ReactingUserConnection { total: self.count }
    }
    async fn reactors(&self, first: Option<i32>, after: Option<String>, last: Option<i32>, before: Option<String>) -> ReactingUserConnection {
        let _ = (first, after, last, before);
        ReactingUserConnection { total: self.count }
    }
    async fn viewer_has_reacted(&self) -> bool {
        false
    }
}

pub struct ReactingUserConnection {
    total: i64,
}

#[Object]
impl ReactingUserConnection {
    async fn total_count(&self) -> i32 {
        self.total as i32
    }
}

#[derive(Clone)]
pub struct GComment {
    pub row: Arc<CommentRow>,
    pub repo: Arc<RepoCtx>,
    pub number: i64,
    pub is_pull: bool,
}

impl GComment {
    fn html_url(&self, ctx: &Context<'_>) -> String {
        format!(
            "{}/{}/{}/{}#issuecomment-{}",
            st(ctx).base_url(),
            self.repo.full_name(),
            if self.is_pull { "pull" } else { "issues" },
            self.number,
            self.row.id
        )
    }
}

#[Object(name = "IssueComment")]
impl GComment {
    pub async fn id(&self) -> ID {
        ID(ids::encode("IssueComment", self.row.id))
    }
    async fn database_id(&self) -> Option<i64> {
        Some(self.row.id)
    }
    async fn full_database_id(&self) -> Option<String> {
        Some(self.row.id.to_string())
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
    async fn author(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Actor>> {
        Ok(loaders::user(ctx, self.row.user_id).await?.map(Actor::from_user))
    }
    async fn author_association(&self, ctx: &Context<'_>) -> async_graphql::Result<CommentAuthorAssociation> {
        association(ctx, self.repo.repo.id, self.row.user_id).await
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
        (self.row.updated_at > self.row.created_at).then(|| dt(self.row.updated_at))
    }
    async fn includes_created_edit(&self) -> bool {
        false
    }
    async fn is_minimized(&self) -> bool {
        false
    }
    async fn minimized_reason(&self) -> Option<String> {
        None
    }
    async fn reaction_groups(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Vec<ReactionGroup>>> {
        let counts = loader(ctx).load_one(CommentReactions(self.row.id)).await.map_err(lerr)?;
        Ok(Some(reaction_groups(ctx, counts).await))
    }
    pub async fn url(&self, ctx: &Context<'_>) -> Uri {
        Uri(self.html_url(ctx))
    }
    pub async fn resource_path(&self) -> Uri {
        Uri(format!(
            "/{}/{}/{}#issuecomment-{}",
            self.repo.full_name(),
            if self.is_pull { "pull" } else { "issues" },
            self.number,
            self.row.id
        ))
    }
    async fn viewer_did_author(&self, ctx: &Context<'_>) -> bool {
        viewer_did_author(ctx, self.row.user_id)
    }
    async fn viewer_can_update(&self, ctx: &Context<'_>) -> bool {
        self.repo.perm >= Perm::Write || viewer_did_author(ctx, self.row.user_id)
    }
    async fn viewer_can_delete(&self, ctx: &Context<'_>) -> bool {
        self.repo.perm >= Perm::Write || viewer_did_author(ctx, self.row.user_id)
    }
    async fn viewer_can_react(&self) -> bool {
        true
    }
    async fn viewer_can_minimize(&self) -> bool {
        self.repo.perm >= Perm::Triage
    }
    async fn repository(&self) -> GRepo {
        GRepo::new(self.repo.clone())
    }
}
