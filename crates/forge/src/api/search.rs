//! Search: issues & pull requests, repositories, users, labels, topics.
//!
//! The query language follows GitHub's: whitespace-separated terms, `qualifier:value` pairs
//! (values may be quoted), `-qualifier:value` negation, `@me`, ranges (`>=`, `<`, `a..b`).
//! Free text uses Postgres full-text search plus ILIKE so partial words match too.
use axum::extract::{OriginalUri, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Postgres, QueryBuilder};

use super::issues::{issues_json_multi, mention_regex, repo_ctxs};
use super::{PageParams, Paged};
use crate::auth::MaybeUser;
use crate::db::{Perm, User};
use crate::error::{ApiError, ApiResult};
use crate::json;
use crate::json::issue::{IssueRow, LabelRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/search/issues", get(search_issues_handler))
        .route("/search/repositories", get(search_repos_handler))
        .route("/search/users", get(search_users_handler))
        .route("/search/labels", get(search_labels_handler))
        .route("/search/topics", get(search_topics_handler))
}

/// GitHub caps search results at 1000.
const MAX_RESULTS: i64 = 1000;

// ---------------------------------------------------------------------------------------------
// Query parsing.

#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub negated: bool,
    /// Lower-cased qualifier name, `None` for free text.
    pub key: Option<String>,
    pub value: String,
}

/// Splits a search query into terms. Quotes group words; `key:"a b"` keeps the space.
pub fn parse_query(q: &str) -> Vec<Term> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut quoted_any = false;
    for ch in q.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                quoted_any = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() || quoted_any {
                    tokens.push(std::mem::take(&mut cur));
                }
                quoted_any = false;
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || quoted_any {
        tokens.push(cur);
    }
    let mut out = Vec::new();
    for t in tokens {
        // Terms are ANDed; grouping parentheses (gh sends `( words ) qualifiers`) are dropped.
        let t = t.trim_start_matches('(').trim_end_matches(')').to_string();
        if t.is_empty() || t == "AND" {
            continue;
        }
        let (negated, body) = match t.strip_prefix('-') {
            Some(rest) if !rest.is_empty() && rest.contains(':') => (true, rest.to_string()),
            _ => match t.strip_prefix("NOT ") {
                Some(rest) => (true, rest.to_string()),
                None => (false, t.clone()),
            },
        };
        match body.split_once(':') {
            Some((k, v)) if !k.is_empty() && !v.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                out.push(Term { negated, key: Some(k.to_lowercase()), value: v.to_string() });
            }
            _ => out.push(Term { negated: false, key: None, value: t }),
        }
    }
    out
}

fn like_pattern(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Resolves `@me` to the viewer's login.
pub(crate) fn login_value(v: &str, viewer: Option<&User>) -> String {
    if v.eq_ignore_ascii_case("@me") {
        viewer.map(|u| u.login.clone()).unwrap_or_else(|| "\u{0}".into())
    } else {
        v.trim_start_matches('@').to_string()
    }
}

#[derive(Debug, Clone, Copy)]
enum RangeKind {
    Date,
    Int,
}

/// One bound of a range qualifier as SQL literal-free pieces.
enum Bound {
    Date(DateTime<Utc>),
    Int(i64),
}

fn parse_bound(s: &str, kind: RangeKind, end_of_day: bool) -> Option<Bound> {
    match kind {
        RangeKind::Int => s.parse().ok().map(Bound::Int),
        RangeKind::Date => {
            if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                let t = d.and_hms_opt(0, 0, 0)?.and_utc();
                return Some(Bound::Date(if end_of_day { t + Duration::days(1) } else { t }));
            }
            super::issues::parse_time(s).map(Bound::Date)
        }
    }
}

fn push_bound(qb: &mut QueryBuilder<Postgres>, b: Bound) {
    match b {
        Bound::Date(d) => {
            qb.push_bind(d);
        }
        Bound::Int(n) => {
            qb.push_bind(n);
        }
    }
}

/// Pushes a range condition on `col` (`>=x`, `>x`, `<x`, `<=x`, `a..b`, `x`). Returns false if
/// the value doesn't parse (the caller then matches nothing).
fn push_range(qb: &mut QueryBuilder<Postgres>, col: &str, v: &str, kind: RangeKind) -> bool {
    let is_date = matches!(kind, RangeKind::Date);
    // (prefix, sql op for dates, bound is end of day, sql op for ints). For dates `>d` means
    // after the whole day d and `<=d` includes the whole day.
    let ops: [(&str, &str, bool, &str); 4] =
        [(">=", ">=", false, ">="), ("<=", "<", true, "<="), (">", ">=", true, ">"), ("<", "<", false, "<")];
    for (prefix, date_op, eod, int_op) in ops {
        if let Some(rest) = v.strip_prefix(prefix) {
            let (op, eod) = if is_date { (date_op, eod) } else { (int_op, false) };
            let Some(b) = parse_bound(rest, kind, eod) else { return false };
            qb.push(format!("{col} {op} "));
            push_bound(qb, b);
            return true;
        }
    }
    if let Some((a, b)) = v.split_once("..") {
        qb.push("(TRUE");
        if a != "*" && !a.is_empty() {
            let Some(lo) = parse_bound(a, kind, false) else { return false };
            qb.push(format!(" AND {col} >= "));
            push_bound(qb, lo);
        }
        if b != "*" && !b.is_empty() {
            let Some(hi) = parse_bound(b, kind, is_date) else { return false };
            qb.push(format!(" AND {col} {} ", if is_date { "<" } else { "<=" }));
            push_bound(qb, hi);
        }
        qb.push(")");
        return true;
    }
    match kind {
        RangeKind::Int => {
            let Some(b) = parse_bound(v, kind, false) else { return false };
            qb.push(format!("{col} = "));
            push_bound(qb, b);
        }
        RangeKind::Date => {
            let (Some(lo), Some(hi)) = (parse_bound(v, kind, false), parse_bound(v, kind, true)) else { return false };
            qb.push(format!("({col} >= "));
            push_bound(qb, lo);
            qb.push(format!(" AND {col} < "));
            push_bound(qb, hi);
            qb.push(")");
        }
    }
    true
}

/// Repositories visible to the viewer (alias `r`).
pub(crate) fn push_visible(qb: &mut QueryBuilder<Postgres>, viewer: Option<&User>) {
    match viewer {
        Some(u) if u.site_admin => {
            qb.push("TRUE");
        }
        Some(u) => {
            qb.push("(NOT r.private OR r.owner_id = ")
                .push_bind(u.id)
                .push(" OR EXISTS(SELECT 1 FROM repo_collaborators vc WHERE vc.repo_id = r.id AND vc.user_id = ")
                .push_bind(u.id)
                .push(") OR EXISTS(SELECT 1 FROM org_members vm WHERE vm.org_id = r.owner_id AND vm.user_id = ")
                .push_bind(u.id)
                .push("))");
        }
        None => {
            qb.push("NOT r.private");
        }
    }
}

fn user_id_sql(qb: &mut QueryBuilder<Postgres>, login: String) {
    qb.push("(SELECT id FROM users WHERE lower(login) = lower(").push_bind(login).push("))");
}

#[derive(Deserialize, Default)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub repository_id: Option<i64>,
}

impl SearchQuery {
    pub(crate) fn pages(&self) -> PageParams {
        PageParams { page: self.page, per_page: self.per_page }
    }
    pub(crate) fn query(&self) -> ApiResult<&str> {
        match self.q.as_deref().map(str::trim) {
            Some(q) if !q.is_empty() => Ok(q),
            _ => {
                let mut e = ApiError::validation("Search", "q", "missing");
                e.errors = Some(json!([{ "resource": "Search", "field": "q", "code": "missing" }]));
                Err(e)
            }
        }
    }
}

/// `{ total_count, incomplete_results, items }` with GitHub's pagination links.
pub(crate) fn search_response(uri: &OriginalUri, s: &AppState, p: PageParams, total: i64, items: Vec<Value>, extra: Option<(&str, Value)>) -> Response {
    let capped = total.min(MAX_RESULTS);
    let paged = Paged::new(uri, s, p, Some(capped), vec![]);
    let mut res = paged.into_response();
    let mut body = json!({ "total_count": total, "incomplete_results": false, "items": items });
    if let Some((k, v)) = extra {
        body[k] = v;
    }
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    *res.body_mut() = axum::body::Body::from(bytes);
    res.headers_mut().remove(axum::http::header::CONTENT_LENGTH);
    res
}

pub fn with_score(mut v: Value) -> Value {
    v["score"] = json!(1.0);
    // Search items document `body` as a (non-null) string; omit it rather than send null.
    if let Some(o) = v.as_object_mut() {
        if o.get("body").is_some_and(|b| b.is_null()) {
            o.remove("body");
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------
// Issues and pull requests.

/// Pushes ` AND <cond>` for every term of an issue search (alias `i`, repo `r`, owner `o`).
fn push_issue_terms(qb: &mut QueryBuilder<Postgres>, terms: &[Term], viewer: Option<&User>) {
    // `in:` selects which fields free text matches.
    let ins: Vec<String> = terms
        .iter()
        .filter(|t| t.key.as_deref() == Some("in"))
        .flat_map(|t| t.value.split(',').map(|s| s.trim().to_lowercase()).collect::<Vec<_>>())
        .collect();
    let (in_title, in_body, in_comments) =
        if ins.is_empty() { (true, true, false) } else { (ins.iter().any(|s| s == "title"), ins.iter().any(|s| s == "body"), ins.iter().any(|s| s == "comments")) };

    // Qualifiers of the same kind that GitHub ORs together.
    let repos: Vec<&Term> = terms.iter().filter(|t| t.key.as_deref() == Some("repo") && !t.negated).collect();
    if !repos.is_empty() {
        qb.push(" AND (FALSE");
        for t in repos {
            let (o, n) = t.value.split_once('/').unwrap_or((&t.value, ""));
            qb.push(" OR (lower(o.login) = lower(")
                .push_bind(o.to_string())
                .push(") AND lower(r.name) = lower(")
                .push_bind(n.to_string())
                .push("))");
        }
        qb.push(")");
    }
    let owners: Vec<&Term> = terms.iter().filter(|t| matches!(t.key.as_deref(), Some("user" | "org" | "owner")) && !t.negated).collect();
    if !owners.is_empty() {
        qb.push(" AND lower(o.login) = ANY(");
        qb.push_bind(owners.iter().map(|t| login_value(&t.value, viewer).to_lowercase()).collect::<Vec<_>>());
        qb.push(")");
    }

    let mut text: Vec<&str> = Vec::new();
    for t in terms {
        let Some(key) = t.key.as_deref() else {
            text.push(&t.value);
            continue;
        };
        let v = t.value.as_str();
        let lv = v.to_lowercase();
        let not = if t.negated { "NOT " } else { "" };
        match key {
            "repo" | "user" | "org" | "owner" if !t.negated => {}
            "repo" => {
                let (o, n) = v.split_once('/').unwrap_or((v, ""));
                qb.push(" AND NOT (lower(o.login) = lower(")
                    .push_bind(o.to_string())
                    .push(") AND lower(r.name) = lower(")
                    .push_bind(n.to_string())
                    .push("))");
            }
            "user" | "org" | "owner" => {
                qb.push(" AND lower(o.login) <> lower(").push_bind(login_value(v, viewer)).push(")");
            }
            "in" | "sort" => {}
            "is" | "type" | "state" => {
                let cond = match lv.as_str() {
                    "issue" | "issues" => "NOT i.is_pull",
                    "pr" | "pull-request" | "pullrequest" | "pull_request" => "i.is_pull",
                    "open" => "i.state = 'open'",
                    "closed" => "i.state = 'closed'",
                    "merged" => "EXISTS(SELECT 1 FROM pull_requests pr WHERE pr.issue_id = i.id AND pr.merged)",
                    "unmerged" => "(i.is_pull AND i.state = 'closed' AND NOT EXISTS(SELECT 1 FROM pull_requests pr WHERE pr.issue_id = i.id AND pr.merged))",
                    "draft" => "EXISTS(SELECT 1 FROM pull_requests pr WHERE pr.issue_id = i.id AND pr.draft)",
                    "locked" => "i.locked",
                    "unlocked" => "NOT i.locked",
                    "public" => "NOT r.private",
                    "private" => "r.private",
                    "archived" => "r.archived",
                    _ => "FALSE",
                };
                qb.push(format!(" AND {not}({cond})"));
            }
            "reason" => {
                let r = match lv.as_str() {
                    "completed" => "completed",
                    "not planned" | "not_planned" => "not_planned",
                    "reopened" => "reopened",
                    "duplicate" => "duplicate",
                    _ => "",
                };
                qb.push(format!(" AND {not}(i.state_reason IS NOT DISTINCT FROM ")).push_bind(r.to_string()).push(")");
            }
            "author" => {
                qb.push(format!(" AND {not}(i.user_id IS NOT DISTINCT FROM "));
                user_id_sql(qb, login_value(v, viewer));
                qb.push(")");
            }
            "assignee" => {
                qb.push(format!(" AND {not}EXISTS(SELECT 1 FROM issue_assignees sa WHERE sa.issue_id = i.id AND sa.user_id = "));
                user_id_sql(qb, login_value(v, viewer));
                qb.push(")");
            }
            "label" => {
                let names: Vec<String> = v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
                qb.push(format!(
                    " AND {not}EXISTS(SELECT 1 FROM issue_labels sl JOIN labels l ON l.id = sl.label_id WHERE sl.issue_id = i.id AND lower(l.name) = ANY("
                ))
                .push_bind(names)
                .push("))");
            }
            "milestone" => {
                qb.push(format!(
                    " AND {not}EXISTS(SELECT 1 FROM milestones sm WHERE sm.id = i.milestone_id AND lower(sm.title) = lower("
                ))
                .push_bind(v.to_string())
                .push("))");
            }
            "no" | "missing" => {
                let cond = match lv.as_str() {
                    "label" => "NOT EXISTS(SELECT 1 FROM issue_labels sl WHERE sl.issue_id = i.id)",
                    "milestone" => "i.milestone_id IS NULL",
                    "assignee" => "NOT EXISTS(SELECT 1 FROM issue_assignees sa WHERE sa.issue_id = i.id)",
                    _ => "TRUE",
                };
                qb.push(format!(" AND {not}({cond})"));
            }
            "mentions" => {
                let re = mention_regex(&login_value(v, viewer));
                qb.push(format!(" AND {not}(i.body ~* "))
                    .push_bind(re.clone())
                    .push(" OR EXISTS(SELECT 1 FROM issue_comments sc WHERE sc.issue_id = i.id AND sc.body ~* ")
                    .push_bind(re)
                    .push("))");
            }
            "commenter" => {
                qb.push(format!(" AND {not}EXISTS(SELECT 1 FROM issue_comments sc WHERE sc.issue_id = i.id AND sc.user_id = "));
                user_id_sql(qb, login_value(v, viewer));
                qb.push(")");
            }
            "involves" => {
                let login = login_value(v, viewer);
                let re = mention_regex(&login);
                qb.push(format!(" AND {not}(i.user_id IS NOT DISTINCT FROM "));
                user_id_sql(qb, login.clone());
                qb.push(" OR EXISTS(SELECT 1 FROM issue_assignees sa WHERE sa.issue_id = i.id AND sa.user_id = ");
                user_id_sql(qb, login.clone());
                qb.push(") OR EXISTS(SELECT 1 FROM issue_comments sc WHERE sc.issue_id = i.id AND sc.user_id = ");
                user_id_sql(qb, login);
                qb.push(") OR i.body ~* ").push_bind(re).push(")");
            }
            "reviewed-by" => {
                qb.push(format!(" AND {not}EXISTS(SELECT 1 FROM pull_reviews sr WHERE sr.issue_id = i.id AND sr.state <> 'PENDING' AND sr.user_id = "));
                user_id_sql(qb, login_value(v, viewer));
                qb.push(")");
            }
            "head" | "base" => {
                let col = if key == "head" { "head_ref" } else { "base_ref" };
                qb.push(format!(" AND {not}EXISTS(SELECT 1 FROM pull_requests pr WHERE pr.issue_id = i.id AND pr.{col} = "))
                    .push_bind(v.to_string())
                    .push(")");
            }
            "created" | "updated" | "closed" => {
                let col = match key {
                    "created" => "i.created_at",
                    "updated" => "i.updated_at",
                    _ => "i.closed_at",
                };
                qb.push(format!(" AND {not}("));
                if !push_range(qb, col, v, RangeKind::Date) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            "merged" => {
                qb.push(format!(" AND {not}EXISTS(SELECT 1 FROM pull_requests pr WHERE pr.issue_id = i.id AND "));
                if !push_range(qb, "pr.merged_at", v, RangeKind::Date) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            "comments" => {
                qb.push(format!(" AND {not}("));
                if !push_range(qb, "i.comments_count", v, RangeKind::Int) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            // Not modelled (no review requests / teams / projects yet): matches nothing.
            "review-requested" | "team-review-requested" | "team" | "project" | "linked" | "review" | "status" => {
                qb.push(format!(" AND {}", if t.negated { "TRUE" } else { "FALSE" }));
            }
            // Unknown qualifiers are plain text, like GitHub.
            _ => text.push(&t.value),
        }
    }
    for word in text {
        let word = word.trim_start_matches('#');
        let pat = like_pattern(word);
        qb.push(" AND (FALSE");
        if in_title {
            qb.push(" OR i.title ILIKE ").push_bind(pat.clone());
        }
        if in_body {
            qb.push(" OR i.body ILIKE ").push_bind(pat.clone());
        }
        if in_title && in_body {
            qb.push(" OR to_tsvector('simple', i.title || ' ' || coalesce(i.body, '')) @@ plainto_tsquery('simple', ")
                .push_bind(word.to_string())
                .push(")");
        }
        if in_comments {
            qb.push(" OR EXISTS(SELECT 1 FROM issue_comments sc WHERE sc.issue_id = i.id AND sc.body ILIKE ").push_bind(pat).push(")");
        }
        if let Ok(n) = word.parse::<i64>() {
            qb.push(" OR i.number = ").push_bind(n);
        }
        qb.push(")");
    }
}

fn push_issue_where(qb: &mut QueryBuilder<Postgres>, terms: &[Term], viewer: Option<&User>) {
    qb.push(" FROM issues i JOIN repositories r ON r.id = i.repo_id JOIN users o ON o.id = r.owner_id WHERE ");
    push_visible(qb, viewer);
    push_issue_terms(qb, terms, viewer);
}

/// Result of an issue search: total match count and the page of rows (in order).
pub struct IssueSearch {
    pub total: i64,
    pub rows: Vec<IssueRow>,
}

/// Searches issues and pull requests visible to `viewer`. `sort`: comments | created | updated |
/// reactions | interactions (default best match ≈ newest); `order`: asc | desc.
/// Usable from GraphQL's `search(type: ISSUE)`.
pub async fn search_issues(state: &AppState, viewer: Option<&User>, q: &str, sort: Option<&str>, order: Option<&str>, p: &PageParams) -> ApiResult<IssueSearch> {
    let terms = parse_query(q);
    // `sort:created-asc` inside the query wins over the parameter, like GitHub.
    let in_q_sort = terms.iter().find(|t| t.key.as_deref() == Some("sort")).map(|t| t.value.to_lowercase());
    let (sort, order) = match &in_q_sort {
        Some(s) => {
            let (f, d) = s.rsplit_once('-').unwrap_or((s.as_str(), "desc"));
            (Some(f.to_string()), Some(d.to_string()))
        }
        None => (sort.map(str::to_string), order.map(str::to_string)),
    };
    let mut count = QueryBuilder::new("SELECT count(*)");
    push_issue_where(&mut count, &terms, viewer);
    let total: i64 = count.build_query_scalar().fetch_one(&state.db).await?;

    let mut qb = QueryBuilder::new("SELECT i.*");
    push_issue_where(&mut qb, &terms, viewer);
    let dir = if order.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let col = match sort.as_deref() {
        Some("comments") => "i.comments_count".to_string(),
        Some("updated") => "i.updated_at".to_string(),
        Some(s) if s.starts_with("reactions") || s == "interactions" => {
            let content = s.strip_prefix("reactions-").map(|c| c.replace("plus", "+").replace("minus", "-"));
            match content {
                Some(c) if json::issue::REACTIONS.contains(&c.as_str()) => format!(
                    "(SELECT count(*) FROM reactions x WHERE x.subject_type = 'issue' AND x.subject_id = i.id AND x.content = '{c}')"
                ),
                _ if s == "interactions" => "(i.comments_count + (SELECT count(*) FROM reactions x WHERE x.subject_type = 'issue' AND x.subject_id = i.id))".into(),
                _ => "(SELECT count(*) FROM reactions x WHERE x.subject_type = 'issue' AND x.subject_id = i.id)".into(),
            }
        }
        _ => "i.created_at".to_string(),
    };
    qb.push(format!(" ORDER BY {col} {dir}, i.id {dir}"));
    let offset = p.offset().min(MAX_RESULTS);
    let limit = p.limit().min(MAX_RESULTS - offset).max(0);
    qb.push(" LIMIT ").push_bind(limit).push(" OFFSET ").push_bind(offset);
    let rows: Vec<IssueRow> = qb.build_query_as().fetch_all(&state.db).await?;
    Ok(IssueSearch { total, rows })
}

async fn search_issues_handler(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchQuery>) -> ApiResult<Response> {
    let query = q.query()?;
    let p = q.pages();
    let res = search_issues(&s, u.as_ref(), query, q.sort.as_deref(), q.order.as_deref(), &p).await?;
    let items = issues_json_multi(&s, u.as_ref(), res.rows, false).await?.into_iter().map(with_score).collect();
    Ok(search_response(&uri, &s, p, res.total, items, Some(("search_type", json!("lexical")))))
}

// ---------------------------------------------------------------------------------------------
// Repositories.

fn push_repo_where(qb: &mut QueryBuilder<Postgres>, terms: &[Term], viewer: Option<&User>) {
    qb.push(" FROM repositories r JOIN users o ON o.id = r.owner_id WHERE ");
    push_visible(qb, viewer);
    let ins: Vec<String> = terms
        .iter()
        .filter(|t| t.key.as_deref() == Some("in"))
        .flat_map(|t| t.value.split(',').map(|s| s.trim().to_lowercase()).collect::<Vec<_>>())
        .collect();
    let field = |f: &str| ins.is_empty() || ins.iter().any(|s| s == f);
    let mut fork_mode = "exclude";
    let owners: Vec<&Term> = terms.iter().filter(|t| matches!(t.key.as_deref(), Some("user" | "org" | "owner")) && !t.negated).collect();
    if !owners.is_empty() {
        qb.push(" AND lower(o.login) = ANY(");
        qb.push_bind(owners.iter().map(|t| login_value(&t.value, viewer).to_lowercase()).collect::<Vec<_>>());
        qb.push(")");
    }
    let mut text: Vec<&str> = Vec::new();
    for t in terms {
        let Some(key) = t.key.as_deref() else {
            text.push(&t.value);
            continue;
        };
        let v = t.value.as_str();
        let lv = v.to_lowercase();
        let not = if t.negated { "NOT " } else { "" };
        match key {
            "user" | "org" | "owner" if !t.negated => {}
            "user" | "org" | "owner" => {
                qb.push(" AND lower(o.login) <> lower(").push_bind(login_value(v, viewer)).push(")");
            }
            "in" | "sort" => {}
            "repo" => {
                let (o, n) = v.split_once('/').unwrap_or((v, ""));
                qb.push(format!(" AND {not}(lower(o.login) = lower("))
                    .push_bind(o.to_string())
                    .push(") AND lower(r.name) = lower(")
                    .push_bind(n.to_string())
                    .push("))");
                fork_mode = "include";
            }
            "fork" => {
                fork_mode = match lv.as_str() {
                    "true" => "include",
                    "only" => "only",
                    _ => "exclude",
                };
            }
            "is" => {
                let cond = match lv.as_str() {
                    "public" => "NOT r.private",
                    "private" | "internal" => "r.private",
                    "template" => "r.is_template",
                    "archived" => "r.archived",
                    "fork" => "r.fork_parent_id IS NOT NULL",
                    _ => "FALSE",
                };
                qb.push(format!(" AND {not}({cond})"));
            }
            "archived" => {
                qb.push(format!(" AND {not}(r.archived = ")).push_bind(lv == "true").push(")");
            }
            "template" => {
                qb.push(format!(" AND {not}(r.is_template = ")).push_bind(lv == "true").push(")");
            }
            "language" => {
                qb.push(format!(" AND {not}(lower(r.language) IS NOT DISTINCT FROM ")).push_bind(lv).push(")");
            }
            "topic" => {
                qb.push(format!(" AND {not}(")).push_bind(lv).push(" = ANY(r.topics))");
            }
            "stars" | "forks" | "size" | "topics" => {
                let col = match key {
                    "stars" => "r.stargazers_count",
                    "forks" => "r.forks_count",
                    "size" => "r.size_kb",
                    _ => "cardinality(r.topics)",
                };
                qb.push(format!(" AND {not}("));
                if !push_range(qb, col, v, RangeKind::Int) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            "created" | "pushed" => {
                let col = if key == "created" { "r.created_at" } else { "r.pushed_at" };
                qb.push(format!(" AND {not}("));
                if !push_range(qb, col, v, RangeKind::Date) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            _ => text.push(&t.value),
        }
    }
    match fork_mode {
        "only" => {
            qb.push(" AND r.fork_parent_id IS NOT NULL");
        }
        "exclude" => {
            qb.push(" AND r.fork_parent_id IS NULL");
        }
        _ => {}
    }
    for word in text {
        let pat = like_pattern(word);
        qb.push(" AND (FALSE");
        if field("name") {
            qb.push(" OR r.name ILIKE ").push_bind(pat.clone());
            if word.contains('/') {
                qb.push(" OR (o.login || '/' || r.name) ILIKE ").push_bind(pat.clone());
            }
        }
        if field("description") {
            qb.push(" OR r.description ILIKE ").push_bind(pat.clone());
        }
        if field("topics") {
            qb.push(" OR ").push_bind(word.to_lowercase()).push(" = ANY(r.topics)");
        }
        qb.push(")");
    }
}

/// Searches repositories visible to `viewer`. Returns (total, repository ids in order).
pub async fn search_repos(state: &AppState, viewer: Option<&User>, q: &str, sort: Option<&str>, order: Option<&str>, p: &PageParams) -> ApiResult<(i64, Vec<i64>)> {
    let terms = parse_query(q);
    let mut count = QueryBuilder::new("SELECT count(*)");
    push_repo_where(&mut count, &terms, viewer);
    let total: i64 = count.build_query_scalar().fetch_one(&state.db).await?;
    let mut qb = QueryBuilder::new("SELECT r.id");
    push_repo_where(&mut qb, &terms, viewer);
    let dir = if order.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let order_sql = match sort {
        Some("stars") => format!("r.stargazers_count {dir}, r.id {dir}"),
        Some("forks") => format!("r.forks_count {dir}, r.id {dir}"),
        Some("updated") => format!("r.updated_at {dir}, r.id {dir}"),
        Some("help-wanted-issues") => format!(
            "(SELECT count(*) FROM issues hi JOIN issue_labels hl ON hl.issue_id = hi.id JOIN labels hlb ON hlb.id = hl.label_id
               WHERE hi.repo_id = r.id AND hi.state = 'open' AND lower(hlb.name) = 'help wanted') {dir}, r.id {dir}"
        ),
        _ => "r.stargazers_count DESC, r.updated_at DESC, r.id DESC".into(),
    };
    qb.push(format!(" ORDER BY {order_sql}"));
    let offset = p.offset().min(MAX_RESULTS);
    qb.push(" LIMIT ").push_bind(p.limit().min(MAX_RESULTS - offset).max(0)).push(" OFFSET ").push_bind(offset);
    let ids: Vec<i64> = qb.build_query_scalar().fetch_all(&state.db).await?;
    Ok((total, ids))
}

async fn search_repos_handler(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchQuery>) -> ApiResult<Response> {
    let query = q.query()?;
    let p = q.pages();
    let (total, ids) = search_repos(&s, u.as_ref(), query, q.sort.as_deref(), q.order.as_deref(), &p).await?;
    let ctxs = repo_ctxs(&s, u.as_ref(), &ids).await?;
    let items = ids
        .iter()
        .filter_map(|id| ctxs.get(id))
        .filter(|c| c.perm >= Perm::Read)
        .map(|ctx| {
            let mut v = with_score(json::repo::repo(&s.api_url(), s.base_url(), ctx));
            if v["pushed_at"].is_null() {
                v["pushed_at"] = v["created_at"].clone();
            }
            v
        })
        .collect();
    Ok(search_response(&uri, &s, p, total, items, None))
}

// ---------------------------------------------------------------------------------------------
// Users.

fn push_user_where(qb: &mut QueryBuilder<Postgres>, terms: &[Term]) {
    qb.push(" FROM users u WHERE TRUE");
    let ins: Vec<String> = terms
        .iter()
        .filter(|t| t.key.as_deref() == Some("in"))
        .flat_map(|t| t.value.split(',').map(|s| s.trim().to_lowercase()).collect::<Vec<_>>())
        .collect();
    let field = |f: &str| ins.is_empty() || ins.iter().any(|s| s == f);
    let mut text: Vec<&str> = Vec::new();
    for t in terms {
        let Some(key) = t.key.as_deref() else {
            text.push(&t.value);
            continue;
        };
        let v = t.value.as_str();
        let not = if t.negated { "NOT " } else { "" };
        match key {
            "in" | "sort" => {}
            "type" => {
                let kind = if v.eq_ignore_ascii_case("org") { "Organization" } else { "User" };
                qb.push(format!(" AND {not}(u.kind = ")).push_bind(kind).push(")");
            }
            "user" | "org" => {
                qb.push(format!(" AND {not}(lower(u.login) = lower(")).push_bind(v.to_string()).push("))");
            }
            "fullname" => {
                qb.push(format!(" AND {not}(u.name ILIKE ")).push_bind(like_pattern(v)).push(")");
            }
            "location" => {
                qb.push(format!(" AND {not}(u.location ILIKE ")).push_bind(like_pattern(v)).push(")");
            }
            "repos" => {
                qb.push(format!(" AND {not}("));
                if !push_range(qb, "(SELECT count(*) FROM repositories ur WHERE ur.owner_id = u.id AND NOT ur.private)", v, RangeKind::Int) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            "created" => {
                qb.push(format!(" AND {not}("));
                if !push_range(qb, "u.created_at", v, RangeKind::Date) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            "followers" => {
                qb.push(format!(" AND {not}("));
                if !push_range(qb, "0", v, RangeKind::Int) {
                    qb.push("FALSE");
                }
                qb.push(")");
            }
            _ => text.push(&t.value),
        }
    }
    for word in text {
        let pat = like_pattern(word);
        qb.push(" AND (FALSE");
        if field("login") {
            qb.push(" OR u.login ILIKE ").push_bind(pat.clone());
        }
        if field("name") || field("fullname") {
            qb.push(" OR u.name ILIKE ").push_bind(pat.clone());
        }
        if field("email") {
            qb.push(" OR u.email ILIKE ").push_bind(pat);
        }
        qb.push(")");
    }
}

async fn search_users_handler(State(s): State<AppState>, uri: OriginalUri, Query(q): Query<SearchQuery>) -> ApiResult<Response> {
    let query = q.query()?;
    let p = q.pages();
    let terms = parse_query(query);
    let mut count = QueryBuilder::new("SELECT count(*)");
    push_user_where(&mut count, &terms);
    let total: i64 = count.build_query_scalar().fetch_one(&s.db).await?;
    let mut qb = QueryBuilder::new("SELECT u.*");
    push_user_where(&mut qb, &terms);
    let dir = if q.order.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let order = match q.sort.as_deref() {
        Some("joined") => format!("u.created_at {dir}, u.id {dir}"),
        Some("repositories") => format!("(SELECT count(*) FROM repositories ur WHERE ur.owner_id = u.id AND NOT ur.private) {dir}, u.id {dir}"),
        _ => {
            // Best match: exact login first, then prefix matches, then the rest.
            let text = terms.iter().find(|t| t.key.is_none()).map(|t| t.value.to_lowercase()).unwrap_or_default();
            qb.push(" ORDER BY (lower(u.login) = ")
                .push_bind(text.clone())
                .push(") DESC, (lower(u.login) LIKE ")
                .push_bind(format!("{}%", text.replace('%', "")))
                .push(") DESC, u.id ASC");
            String::new()
        }
    };
    if !order.is_empty() {
        qb.push(format!(" ORDER BY {order}"));
    }
    let offset = p.offset().min(MAX_RESULTS);
    qb.push(" LIMIT ").push_bind(p.limit().min(MAX_RESULTS - offset).max(0)).push(" OFFSET ").push_bind(offset);
    let users: Vec<User> = qb.build_query_as().fetch_all(&s.db).await?;
    let items = users.iter().map(|u| with_score(json::user::simple(&s.api_url(), s.base_url(), u))).collect();
    Ok(search_response(&uri, &s, p, total, items, None))
}

// ---------------------------------------------------------------------------------------------
// Labels and topics.

async fn search_labels_handler(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchQuery>) -> ApiResult<Response> {
    let query = q.query()?.to_string();
    let Some(repo_id) = q.repository_id else {
        return Err(ApiError::validation("Search", "repository_id", "missing"));
    };
    let ctx = crate::db::load_repo_by_id(&s.db, u.as_ref(), repo_id).await?;
    let p = q.pages();
    let words: Vec<String> = parse_query(&query).into_iter().filter(|t| t.key.is_none()).map(|t| like_pattern(&t.value)).collect();
    let dir = if q.order.as_deref() == Some("asc") { "ASC" } else { "DESC" };
    let order = match q.sort.as_deref() {
        Some("created") | Some("updated") => format!("created_at {dir}, id {dir}"),
        _ => "lower(name) ASC".into(),
    };
    let cond = "repo_id = $1 AND (SELECT bool_and(name ILIKE w OR coalesce(description, '') ILIKE w) FROM unnest($2::text[]) w) IS NOT FALSE";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM labels WHERE {cond}")))
        .bind(ctx.repo.id)
        .bind(&words)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<LabelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM labels WHERE {cond} ORDER BY {order} LIMIT $3 OFFSET $4")))
        .bind(ctx.repo.id)
        .bind(&words)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let full = ctx.full_name();
    let items = rows.iter().map(|l| with_score(json::issue::label(&s.api_url(), &full, l))).collect();
    Ok(search_response(&uri, &s, p, total, items, None))
}

async fn search_topics_handler(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchQuery>) -> ApiResult<Response> {
    let query = q.query()?.to_string();
    let p = q.pages();
    let words: Vec<String> = parse_query(&query).into_iter().filter(|t| t.key.is_none()).map(|t| like_pattern(&t.value)).collect();
    let mut qb = QueryBuilder::new("SELECT t, count(*), min(r.created_at), max(r.updated_at) FROM repositories r, unnest(r.topics) t WHERE ");
    push_visible(&mut qb, u.as_ref());
    qb.push(" AND (SELECT bool_and(t ILIKE w) FROM unnest(").push_bind(words).push("::text[]) w) IS NOT FALSE GROUP BY t ORDER BY count(*) DESC, t");
    let rows: Vec<(String, i64, DateTime<Utc>, DateTime<Utc>)> = qb.build_query_as().fetch_all(&s.db).await?;
    let total = rows.len() as i64;
    let items = rows
        .into_iter()
        .skip(p.offset() as usize)
        .take(p.per_page() as usize)
        .map(|(name, n, created, updated)| {
            json!({
                "name": name,
                "display_name": null,
                "short_description": null,
                "description": null,
                "created_by": null,
                "released": null,
                "created_at": json::ts(created),
                "updated_at": json::ts(updated),
                "featured": false,
                "curated": false,
                "score": 1.0,
                "repository_count": n,
            })
        })
        .collect();
    Ok(search_response(&uri, &s, p, total, items, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qualifiers_and_quotes() {
        let t = parse_query(r#"repo:a/b is:open label:"help wanted" -label:bug fix crash "two words""#);
        assert_eq!(t[0], Term { negated: false, key: Some("repo".into()), value: "a/b".into() });
        assert_eq!(t[2].value, "help wanted");
        assert!(t[3].negated && t[3].key.as_deref() == Some("label"));
        assert_eq!(t[4], Term { negated: false, key: None, value: "fix".into() });
        assert_eq!(t[6].value, "two words");
    }

    #[test]
    fn grouping_parentheses_are_ignored() {
        // gh search issues/prs sends `( keywords ) qualifiers`.
        let t = parse_query("( feature2 ) repo:a/b type:pr");
        assert_eq!(t.len(), 3);
        assert_eq!(t[0], Term { negated: false, key: None, value: "feature2".into() });
        assert_eq!(t[1].key.as_deref(), Some("repo"));
    }

    #[test]
    fn hyphenated_text_is_not_negation() {
        let t = parse_query("-foo bar");
        assert_eq!(t[0].key, None);
        assert_eq!(t[0].value, "-foo");
    }
}
