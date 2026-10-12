//! `search(query:, type:)`: GitHub search syntax subset used by gh (issues/PRs, repositories, users).
use std::sync::Arc;

use async_graphql::{Context, Object, Union};

use super::connection::{cursor, ConnArgs, PageInfo, Window};
use super::enums::SearchType;
use super::issue::{materialize, IssueOrPullRequest};
use super::api_err;
use crate::api::PageParams;
use crate::error::ApiResult;
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

/// Rewrites gh's "advanced issue search" syntax (`( keywords ) qualifiers`, `draft:true`) into the
/// flat syntax the search engine understands.
pub fn normalize_issue_query(q: &str) -> String {
    tokenize(q)
        .into_iter()
        .map(|t| match t.to_ascii_lowercase().as_str() {
            "draft:true" => "is:draft".to_string(),
            "draft:false" => "-is:draft".to_string(),
            _ => match t.split_once(':') {
                Some((k, v)) if v.contains(char::is_whitespace) => format!("{k}:\"{v}\""),
                None if t.contains(char::is_whitespace) => format!("\"{t}\""),
                _ => t,
            },
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn search(ctx: &Context<'_>, query: String, kind: SearchType, args: ConnArgs) -> async_graphql::Result<SearchResultItemConnection> {
    let args = args.search_normalized();
    let query = if kind == SearchType::Issue { normalize_issue_query(&query) } else { query };
    let s = st(ctx).clone();
    let v = viewer(ctx).cloned();
    match kind {
        SearchType::Issue => {
            let (rows, window) = paged(&args, |p| {
                let (s, v, q) = (s.clone(), v.clone(), query.clone());
                async move {
                    let r = crate::api::search::search_issues(&s, v.as_ref(), &q, None, None, &p).await?;
                    Ok((r.total, r.rows))
                }
            })
            .await
            .map_err(api_err)?;
            let items = materialize(ctx, rows).await?.into_iter().map(Into::into).collect();
            Ok(SearchResultItemConnection { kind, items, window })
        }
        SearchType::Repository => {
            let (ids, window) = paged(&args, |p| {
                let (s, v, q) = (s.clone(), v.clone(), query.clone());
                async move { crate::api::search::search_repos(&s, v.as_ref(), &q, None, None, &p).await }
            })
            .await
            .map_err(api_err)?;
            let items = repos_by_ids(ctx, &ids).await?.into_iter().map(SearchResultItem::Repository).collect();
            Ok(SearchResultItemConnection { kind, items, window })
        }
        SearchType::User => search_users(ctx, &query, args).await,
        SearchType::Discussion => Ok(SearchResultItemConnection { kind, items: Vec::new(), window: args.window(0) }),
    }
}

/// Adapts a page/per_page search (the REST search engine, capped at 1000 results) to a
/// cursor window.
async fn paged<T, F, Fut>(args: &ConnArgs, f: F) -> ApiResult<(Vec<T>, Window)>
where
    F: Fn(PageParams) -> Fut,
    Fut: std::future::Future<Output = ApiResult<(i64, Vec<T>)>>,
{
    let (total, _) = f(PageParams { page: Some(1), per_page: Some(1) }).await?;
    let w = args.window(total.clamp(0, 1000) as usize);
    let lim = w.end - w.start;
    if lim == 0 {
        return Ok((Vec::new(), w));
    }
    if w.start % lim == 0 {
        let (_, rows) = f(PageParams { page: Some((w.start / lim + 1) as i64), per_page: Some(lim as i64) }).await?;
        return Ok((rows, w));
    }
    if w.end <= 100 {
        let (_, rows) = f(PageParams { page: Some(1), per_page: Some(w.end as i64) }).await?;
        return Ok((rows.into_iter().skip(w.start).collect(), w));
    }
    // Unaligned deep window: fetch the two pages that cover it.
    let mut out = Vec::new();
    let first_page = w.start / lim + 1;
    for page in [first_page, first_page + 1] {
        let (_, rows) = f(PageParams { page: Some(page as i64), per_page: Some(lim as i64) }).await?;
        let base = (page - 1) * lim;
        for (i, r) in rows.into_iter().enumerate() {
            if (w.start..w.end).contains(&(base + i)) {
                out.push(r);
            }
        }
    }
    Ok((out, w))
}

async fn search_users(ctx: &Context<'_>, query: &str, args: ConnArgs) -> async_graphql::Result<SearchResultItemConnection> {
    let s = st(ctx);
    let words: Vec<String> = tokenize(query).into_iter().filter(|t| !t.contains(':')).collect();
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
