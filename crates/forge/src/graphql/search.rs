//! `search(query:, type:)`: GitHub search syntax subset used by gh (issues/PRs, repositories, users).
use std::sync::Arc;

use async_graphql::{Context, Object, Union};

use super::connection::{cursor, ConnArgs, PageInfo, Window};
use super::enums::SearchType;
use super::issue::{materialize, IssueOrPullRequest, IssueQuery};
use super::pull::GPull;
use super::repo::{repos_by_ids, st, viewer, GRepo};
use super::user::{GOrg, GUser};
use super::issue::GIssue;
use crate::db::User;

#[derive(Union, Clone)]
pub enum SearchResultItem {
    Issue(GIssue),
    PullRequest(GPull),
    Repository(GRepo),
    User(GUser),
    Organization(GOrg),
}

impl From<IssueOrPullRequest> for SearchResultItem {
    fn from(x: IssueOrPullRequest) -> Self {
        match x {
            IssueOrPullRequest::Issue(i) => SearchResultItem::Issue(i),
            IssueOrPullRequest::PullRequest(p) => SearchResultItem::PullRequest(p),
        }
    }
}

pub struct SearchResultItemConnection {
    kind: SearchType,
    items: Vec<SearchResultItem>,
    window: Window,
}

#[Object]
impl SearchResultItemConnection {
    async fn issue_count(&self) -> i32 {
        if self.kind == SearchType::Issue { self.window.total as i32 } else { 0 }
    }
    async fn repository_count(&self) -> i32 {
        if self.kind == SearchType::Repository { self.window.total as i32 } else { 0 }
    }
    async fn user_count(&self) -> i32 {
        if self.kind == SearchType::User { self.window.total as i32 } else { 0 }
    }
    async fn discussion_count(&self) -> i32 {
        0
    }
    async fn code_count(&self) -> i32 {
        0
    }
    async fn wiki_count(&self) -> i32 {
        0
    }
    async fn nodes(&self) -> Vec<Option<SearchResultItem>> {
        self.items.iter().cloned().map(Some).collect()
    }
    async fn edges(&self) -> Vec<Option<SearchResultItemEdge>> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, n)| Some(SearchResultItemEdge { cursor: cursor(self.window.start + i), node: n.clone() }))
            .collect()
    }
    async fn page_info(&self) -> PageInfo {
        PageInfo::from_window(&self.window, self.items.len())
    }
}

pub struct SearchResultItemEdge {
    cursor: String,
    node: SearchResultItem,
}

#[Object]
impl SearchResultItemEdge {
    async fn cursor(&self) -> &str {
        &self.cursor
    }
    async fn node(&self) -> Option<SearchResultItem> {
        Some(self.node.clone())
    }
}

/// Splits a search string into terms, honouring double quotes and dropping grouping parens.
pub fn tokenize(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in q.chars() {
        match ch {
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
    out.into_iter()
        .map(|t| t.trim_matches(|c| c == '(' || c == ')').to_string())
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("AND") && !t.eq_ignore_ascii_case("OR"))
        .collect()
}

pub async fn search(ctx: &Context<'_>, query: String, kind: SearchType, args: ConnArgs) -> async_graphql::Result<SearchResultItemConnection> {
    let args = args.search_normalized();
    let me = viewer(ctx).map(|v| v.login.clone()).unwrap_or_default();
    let terms: Vec<(Option<String>, String, bool)> = tokenize(&query)
        .into_iter()
        .map(|t| {
            let (neg, t) = match t.strip_prefix('-') {
                Some(rest) if rest.contains(':') => (true, rest.to_string()),
                _ => (false, t),
            };
            match t.split_once(':') {
                Some((k, v)) if !k.is_empty() && !v.is_empty() => {
                    let v = if v == "@me" { me.clone() } else { v.to_string() };
                    (Some(k.to_ascii_lowercase()), v, neg)
                }
                _ => (None, t, neg),
            }
        })
        .collect();
    match kind {
        SearchType::Issue => search_issues(ctx, terms, args).await,
        SearchType::Repository => search_repos(ctx, terms, args).await,
        SearchType::User => search_users(ctx, terms, args).await,
        SearchType::Discussion => Ok(SearchResultItemConnection { kind, items: Vec::new(), window: args.window(0) }),
    }
}

async fn search_issues(ctx: &Context<'_>, terms: Vec<(Option<String>, String, bool)>, args: ConnArgs) -> async_graphql::Result<SearchResultItemConnection> {
    let s = st(ctx);
    let mut q = IssueQuery { visible_to: Some(viewer(ctx).map(|v| v.id).unwrap_or(-1)), ..Default::default() };
    let mut repo_ids: Option<Vec<i64>> = None;
    let mut states: Vec<&'static str> = Vec::new();
    let mut order = ("i.created_at", "DESC");
    for (k, v, neg) in terms {
        if neg {
            continue; // negated qualifiers are not supported; ignoring widens results
        }
        let Some(k) = k else {
            q.keywords.push(v);
            continue;
        };
        match (k.as_str(), v.to_ascii_lowercase().as_str()) {
            ("repo", _) => {
                let (o, n) = v.split_once('/').unwrap_or((&v, ""));
                let id = crate::db::load_repo(&s.db, viewer(ctx), o, n).await.ok().map(|c| c.repo.id);
                repo_ids.get_or_insert_with(Vec::new).extend(id.or(Some(-1)));
            }
            ("user" | "org" | "owner", _) => q.owner_logins.push(v.clone()),
            ("is" | "type", "issue") => q.is_pull = Some(false),
            ("is" | "type", "pr" | "pull-request" | "pullrequest") => q.is_pull = Some(true),
            ("is" | "state", "open") => states.push("OPEN"),
            ("is" | "state", "closed") => {
                states.push("CLOSED");
                states.push("MERGED");
            }
            ("is", "merged") | ("state", "merged") => states.push("MERGED"),
            ("is", "unmerged") => states.push("CLOSED"),
            ("is", "draft") => q.draft = Some(true),
            ("draft", d) => q.draft = Some(d == "true"),
            ("author", _) => q.author = Some(v.clone()),
            ("assignee", _) => q.assignee = Some(v.clone()),
            ("mentions", _) => q.mentioned = Some(v.clone()),
            ("label", _) => {
                if v.contains(',') {
                    q.labels_any.extend(v.split(',').map(str::to_string));
                } else {
                    q.labels_all.push(v.clone());
                }
            }
            ("milestone", _) => q.milestone = Some(v.clone()),
            ("no", "label") => q.no_label = true,
            ("no", "assignee") => q.no_assignee = true,
            ("no", "milestone") => q.no_milestone = true,
            ("head", _) => q.head_ref = Some(v.clone()),
            ("base", _) => q.base_ref = Some(v.clone()),
            ("review-requested", _) => q.review_requested = Some(v.clone()),
            ("reviewed-by", _) => q.reviewed_by = Some(v.clone()),
            ("sort", s) => {
                order = match s {
                    "created-asc" => ("i.created_at", "ASC"),
                    "updated-desc" | "updated" => ("i.updated_at", "DESC"),
                    "updated-asc" => ("i.updated_at", "ASC"),
                    "comments-desc" | "comments" => ("(SELECT count(*) FROM issue_comments c WHERE c.issue_id = i.id)", "DESC"),
                    "comments-asc" => ("(SELECT count(*) FROM issue_comments c WHERE c.issue_id = i.id)", "ASC"),
                    _ => ("i.created_at", "DESC"),
                }
            }
            ("in" | "created" | "updated" | "closed" | "merged" | "language" | "archived" | "review" | "status" | "reason", _) => {}
            _ => q.keywords.push(format!("{k}:{v}")),
        }
    }
    if !states.is_empty() {
        q.states = Some(states);
    }
    q.repo_ids = repo_ids;
    q.order = Some(order);
    let (rows, w) = q.run(ctx, &args).await?;
    let items = materialize(ctx, rows).await?.into_iter().map(Into::into).collect();
    Ok(SearchResultItemConnection { kind: SearchType::Issue, items, window: w })
}

async fn search_repos(ctx: &Context<'_>, terms: Vec<(Option<String>, String, bool)>, args: ConnArgs) -> async_graphql::Result<SearchResultItemConnection> {
    let s = st(ctx);
    let mut conds: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    let mut order = "r.updated_at DESC";
    let mut fork_filter = Some("r.fork_parent_id IS NULL".to_string());
    for (k, v, neg) in terms {
        if neg {
            continue;
        }
        let mut bind = |val: String| {
            binds.push(val);
            format!("${}", binds.len() + 1)
        };
        match k.as_deref() {
            Some("user" | "org" | "owner") => {
                let p = bind(v.to_lowercase());
                conds.push(format!("r.owner_id IN (SELECT id FROM users WHERE lower(login) = {p})"));
            }
            Some("repo") => {
                let (o, n) = v.split_once('/').unwrap_or((&v, ""));
                let po = bind(o.to_lowercase());
                let pn = bind(n.to_lowercase());
                conds.push(format!("(r.owner_id IN (SELECT id FROM users WHERE lower(login) = {po}) AND lower(r.name) = {pn})"));
            }
            Some("fork") => {
                fork_filter = match v.as_str() {
                    "true" => None,
                    "only" => Some("r.fork_parent_id IS NOT NULL".into()),
                    _ => Some("r.fork_parent_id IS NULL".into()),
                }
            }
            Some("archived") => conds.push(if v == "true" { "r.archived".into() } else { "NOT r.archived".into() }),
            Some("is") => match v.as_str() {
                "public" => conds.push("NOT r.private".into()),
                "private" => conds.push("r.private".into()),
                "internal" => conds.push("r.visibility = 'internal'".into()),
                "fork" => fork_filter = Some("r.fork_parent_id IS NOT NULL".into()),
                "archived" => conds.push("r.archived".into()),
                "template" => conds.push("r.is_template".into()),
                _ => {}
            },
            Some("language") => {
                let p = bind(v.to_lowercase());
                conds.push(format!("lower(r.language) = {p}"));
            }
            Some("topic") => {
                let p = bind(v.to_lowercase());
                conds.push(format!("{p} = ANY(r.topics)"));
            }
            Some("sort") => {
                order = match v.as_str() {
                    "updated-asc" => "r.updated_at ASC",
                    "stars" | "stars-desc" => "r.stargazers_count DESC",
                    "stars-asc" => "r.stargazers_count ASC",
                    "forks" | "forks-desc" => "r.forks_count DESC",
                    "forks-asc" => "r.forks_count ASC",
                    _ => "r.updated_at DESC",
                }
            }
            Some(_) => {}
            None => {
                let p = bind(format!("%{}%", super::repo::like_escape(&v)));
                conds.push(format!("(r.name ILIKE {p} OR r.description ILIKE {p})"));
            }
        }
    }
    if let Some(f) = fork_filter {
        conds.push(f);
    }
    let visible = "(NOT r.private OR r.owner_id = $1 OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = $1)
                    OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = $1)
                    OR EXISTS(SELECT 1 FROM users a WHERE a.id = $1 AND a.site_admin))";
    let mut where_ = format!("WHERE {visible}");
    for c in &conds {
        where_.push_str(" AND ");
        where_.push_str(c);
    }
    let vid = viewer(ctx).map(|u| u.id).unwrap_or(-1);
    let count_sql = format!("SELECT count(*) FROM repositories r {where_}");
    let mut qc = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql)).bind(vid);
    for b in &binds {
        qc = qc.bind(b.clone());
    }
    let total = qc.fetch_one(&s.db).await?;
    let w = args.window(total as usize);
    let sql = format!("SELECT r.id FROM repositories r {where_} ORDER BY {order}, r.id DESC OFFSET {} LIMIT {}", w.offset(), w.limit());
    let mut qa = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql)).bind(vid);
    for b in &binds {
        qa = qa.bind(b.clone());
    }
    let ids = qa.fetch_all(&s.db).await?;
    let items = repos_by_ids(ctx, &ids).await?.into_iter().map(SearchResultItem::Repository).collect();
    Ok(SearchResultItemConnection { kind: SearchType::Repository, items, window: w })
}

async fn search_users(ctx: &Context<'_>, terms: Vec<(Option<String>, String, bool)>, args: ConnArgs) -> async_graphql::Result<SearchResultItemConnection> {
    let s = st(ctx);
    let words: Vec<String> = terms.into_iter().filter(|(k, _, _)| k.is_none()).map(|(_, v, _)| v).collect();
    let pat = format!("%{}%", super::repo::like_escape(&words.join(" ")));
    let users: Vec<User> = sqlx::query_as(
        "SELECT * FROM users WHERE kind IN ('User', 'Organization') AND (login ILIKE $1 OR name ILIKE $1) ORDER BY lower(login)",
    )
    .bind(pat)
    .fetch_all(&s.db)
    .await?;
    let all: Vec<SearchResultItem> = users
        .into_iter()
        .map(|u| {
            let u = Arc::new(u);
            if u.kind == "Organization" { SearchResultItem::Organization(GOrg(u)) } else { SearchResultItem::User(GUser(u)) }
        })
        .collect();
    let w = args.window(all.len());
    Ok(SearchResultItemConnection { kind: SearchType::User, items: w.slice(&all), window: w })
}
