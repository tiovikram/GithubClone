//! Code and commit search, served straight from git (`git grep` / `git log`) over the default
//! branch of the repositories in scope. Scope comes from `repo:`/`user:`/`org:` qualifiers;
//! without one, the most recently pushed visible repositories are searched.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::QueryBuilder;

use super::search::{login_value, parse_query, push_visible, search_response, with_score, SearchQuery, Term};
use crate::auth::MaybeUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::ApiResult;
use crate::json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/search/code", get(search_code_handler))
        .route("/search/commits", get(search_commits_handler))
}

/// Upper bounds that keep a search request cheap.
const MAX_REPOS: i64 = 20;
const MAX_COMMITS_PER_REPO: usize = 1000;

/// Repositories a code/commit search covers, visible to the viewer.
async fn scope_repos(s: &AppState, viewer: Option<&User>, terms: &[Term]) -> ApiResult<Vec<RepoCtx>> {
    let mut out: Vec<RepoCtx> = Vec::new();
    let repos: Vec<&Term> = terms.iter().filter(|t| t.key.as_deref() == Some("repo") && !t.negated).collect();
    // Each repo costs a full-tree `git grep`/`git log`: cap explicit `repo:` qualifiers too.
    for t in repos.iter().take(MAX_REPOS as usize) {
        if let Some((o, n)) = t.value.split_once('/') {
            if let Ok(ctx) = db::load_repo(&s.db, viewer, o, n).await {
                out.push(ctx);
            }
        }
    }
    if !repos.is_empty() {
        return Ok(out);
    }
    let owners: Vec<String> = terms
        .iter()
        .filter(|t| matches!(t.key.as_deref(), Some("user" | "org" | "owner")) && !t.negated)
        .map(|t| login_value(&t.value, viewer).to_lowercase())
        .collect();
    let mut qb = QueryBuilder::new("SELECT r.id FROM repositories r JOIN users o ON o.id = r.owner_id WHERE ");
    push_visible(&mut qb, viewer);
    if !owners.is_empty() {
        qb.push(" AND lower(o.login) = ANY(").push_bind(owners).push(")");
    }
    for t in terms.iter().filter(|t| t.key.as_deref() == Some("is")) {
        match (t.value.to_lowercase().as_str(), t.negated) {
            ("public", false) | ("private", true) => {
                qb.push(" AND NOT r.private");
            }
            ("private", false) | ("public", true) => {
                qb.push(" AND r.private");
            }
            _ => {}
        }
    }
    qb.push(" AND r.pushed_at IS NOT NULL ORDER BY r.pushed_at DESC LIMIT ").push_bind(MAX_REPOS);
    let ids: Vec<i64> = qb.build_query_scalar().fetch_all(&s.db).await?;
    let ctxs = super::issues::repo_ctxs(s, viewer, &ids).await?;
    Ok(ids.iter().filter_map(|id| ctxs.get(id).cloned()).filter(|c| c.perm >= Perm::Read).collect())
}

fn text_terms(terms: &[Term]) -> Vec<String> {
    terms.iter().filter(|t| t.key.is_none() && t.value != "OR" && t.value != "NOT").map(|t| t.value.clone()).collect()
}

fn qual<'a>(terms: &'a [Term], key: &str) -> Vec<&'a Term> {
    terms.iter().filter(|t| t.key.as_deref() == Some(key)).collect()
}

fn encode_path(p: &str) -> String {
    p.split('/').map(json::issue::urlencode).collect::<Vec<_>>().join("/")
}

// ---------------------------------------------------------------------------------------------
// Code search.

fn language_of(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_lowercase())?;
    Some(match ext.as_str() {
        "rs" => "Rust",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "py" => "Python",
        "go" => "Go",
        "rb" => "Ruby",
        "java" => "Java",
        "c" | "h" => "C",
        "cc" | "cpp" | "hpp" => "C++",
        "cs" => "C#",
        "md" | "markdown" => "Markdown",
        "json" => "JSON",
        "yml" | "yaml" => "YAML",
        "toml" => "TOML",
        "sh" | "bash" => "Shell",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        _ => return None,
    })
}

struct CodeHit {
    repo: usize,
    commit: String,
    path: String,
    sha: String,
    size: Option<i64>,
}

fn path_matches(path: &str, terms: &[Term]) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = path.to_lowercase();
    for t in terms {
        let v = t.value.to_lowercase();
        let ok = match t.key.as_deref() {
            Some("path") => {
                let v = v.trim_start_matches('/');
                lower.starts_with(v) || lower.contains(&format!("/{v}")) || lower.contains(v)
            }
            Some("filename") => name.eq_ignore_ascii_case(&v) || name.to_lowercase().contains(&v),
            Some("extension") => lower.ends_with(&format!(".{}", v.trim_start_matches('.'))),
            Some("language") => language_of(path).is_some_and(|l| l.eq_ignore_ascii_case(&v)),
            _ => true,
        };
        if ok == t.negated {
            return false;
        }
    }
    true
}

async fn code_hits(s: &AppState, repos: &[RepoCtx], terms: &[Term]) -> Vec<CodeHit> {
    let words = text_terms(terms);
    let in_path = qual(terms, "in").iter().any(|t| t.value.to_lowercase().contains("path"));
    let in_file = qual(terms, "in").is_empty() || qual(terms, "in").iter().any(|t| t.value.to_lowercase().contains("file"));
    let mut hits = Vec::new();
    for (idx, ctx) in repos.iter().enumerate() {
        let Ok(path) = s.git.sync(ctx.repo.id).await else { continue };
        let git = &s.git.git;
        let Ok((true, head, _)) = git.try_run(&path, &["rev-parse", "--verify", "-q", "HEAD^{commit}"]).await else { continue };
        let commit = String::from_utf8_lossy(&head).trim().to_string();
        let Ok((true, tree, _)) = git.try_run(&path, &["ls-tree", "-r", "-l", "-z", &commit]).await else { continue };
        // path -> (blob sha, size)
        let mut files: HashMap<String, (String, Option<i64>)> = HashMap::new();
        for entry in tree.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let e = String::from_utf8_lossy(entry);
            let Some((meta, p)) = e.split_once('\t') else { continue };
            let parts: Vec<&str> = meta.split_whitespace().collect();
            if parts.len() >= 4 && parts[1] == "blob" {
                files.insert(p.to_string(), (parts[2].to_string(), parts[3].parse().ok()));
            }
        }
        let mut matched: Vec<String> = Vec::new();
        if words.is_empty() {
            matched = files.keys().cloned().collect();
        } else {
            if in_file {
                let mut args: Vec<String> = vec!["grep".into(), "-I".into(), "-i".into(), "-l".into(), "-z".into(), "-F".into(), "--all-match".into()];
                for w in &words {
                    args.push("-e".into());
                    args.push(w.clone());
                }
                args.push(commit.clone());
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                if let Ok((_, out, _)) = git.try_run(&path, &argv).await {
                    let prefix = format!("{commit}:");
                    for m in out.split(|b| *b == 0).filter(|e| !e.is_empty()) {
                        let m = String::from_utf8_lossy(m);
                        matched.push(m.strip_prefix(&prefix).unwrap_or(&m).to_string());
                    }
                }
            }
            if in_path {
                for p in files.keys() {
                    let lp = p.to_lowercase();
                    if words.iter().all(|w| lp.contains(&w.to_lowercase())) && !matched.contains(p) {
                        matched.push(p.clone());
                    }
                }
            }
        }
        matched.sort();
        for p in matched {
            if !path_matches(&p, terms) {
                continue;
            }
            let Some((sha, size)) = files.get(&p).cloned() else { continue };
            hits.push(CodeHit { repo: idx, commit: commit.clone(), path: p, sha, size });
        }
    }
    hits
}

async fn search_code_handler(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchQuery>) -> ApiResult<Response> {
    let query = q.query()?;
    let p = q.pages();
    let terms = parse_query(query);
    let repos = scope_repos(&s, u.as_ref(), &terms).await?;
    let hits = code_hits(&s, &repos, &terms).await;
    let total = hits.len() as i64;
    let api = s.api_url();
    let base = s.base_url();
    let repo_json: Vec<Value> = repos.iter().map(|c| json::repo::repo(&api, base, c)).collect();
    let items = hits
        .iter()
        .skip(p.offset() as usize)
        .take(p.per_page() as usize)
        .map(|h| {
            let ctx = &repos[h.repo];
            let full = ctx.full_name();
            let ep = encode_path(&h.path);
            let mut v = json!({
                "name": h.path.rsplit('/').next().unwrap_or(&h.path),
                "path": h.path,
                "sha": h.sha,
                "url": format!("{api}/repos/{full}/contents/{ep}?ref={}", h.commit),
                "git_url": format!("{api}/repos/{full}/git/blobs/{}", h.sha),
                "html_url": format!("{base}/{full}/blob/{}/{ep}", h.commit),
                "repository": repo_json[h.repo],
                "language": language_of(&h.path),
            });
            if let Some(sz) = h.size {
                v["file_size"] = json!(sz);
            }
            with_score(v)
        })
        .collect();
    Ok(search_response(&uri, &s, p, total, items, None))
}

// ---------------------------------------------------------------------------------------------
// Commit search.

struct CommitHit {
    repo: usize,
    sha: String,
    parents: Vec<String>,
    tree: String,
    author_name: String,
    author_email: String,
    author_date: DateTime<Utc>,
    committer_name: String,
    committer_email: String,
    committer_date: DateTime<Utc>,
    message: String,
}

fn parse_git_date(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now())
}

/// Date qualifier check (`>=2020-01-01`, `2020-01-01..2020-02-01`, `2020-01-01`, ...).
fn date_matches(t: DateTime<Utc>, v: &str) -> bool {
    let day = |s: &str, end: bool| -> Option<DateTime<Utc>> {
        let d = super::issues::parse_time(s)?;
        Some(if end && s.len() == 10 { d + chrono::Duration::days(1) } else { d })
    };
    if let Some(r) = v.strip_prefix(">=") {
        return day(r, false).is_some_and(|d| t >= d);
    }
    if let Some(r) = v.strip_prefix("<=") {
        return day(r, true).is_some_and(|d| t < d);
    }
    if let Some(r) = v.strip_prefix('>') {
        return day(r, true).is_some_and(|d| t >= d);
    }
    if let Some(r) = v.strip_prefix('<') {
        return day(r, false).is_some_and(|d| t < d);
    }
    if let Some((a, b)) = v.split_once("..") {
        let lo = a == "*" || day(a, false).is_some_and(|d| t >= d);
        let hi = b == "*" || day(b, true).is_some_and(|d| t < d);
        return lo && hi;
    }
    matches!((day(v, false), day(v, true)), (Some(lo), Some(hi)) if t >= lo && t < hi)
}

fn commit_matches(c: &CommitHit, terms: &[Term], author_logins: &HashMap<String, String>, viewer: Option<&User>) -> bool {
    let msg = c.message.to_lowercase();
    for t in terms {
        let v = t.value.to_lowercase();
        let ok = match t.key.as_deref() {
            None => v == "or" || v == "not" || msg.contains(&v),
            Some("author") => {
                let login = login_value(&t.value, viewer).to_lowercase();
                author_logins.get(&c.author_email.to_lowercase()).is_some_and(|l| l.to_lowercase() == login)
            }
            Some("committer") => {
                let login = login_value(&t.value, viewer).to_lowercase();
                author_logins.get(&c.committer_email.to_lowercase()).is_some_and(|l| l.to_lowercase() == login)
            }
            Some("author-name") => c.author_name.to_lowercase().contains(&v),
            Some("committer-name") => c.committer_name.to_lowercase().contains(&v),
            Some("author-email") => c.author_email.eq_ignore_ascii_case(&v),
            Some("committer-email") => c.committer_email.eq_ignore_ascii_case(&v),
            Some("author-date") => date_matches(c.author_date, &v),
            Some("committer-date") => date_matches(c.committer_date, &v),
            Some("merge") => (c.parents.len() > 1) == (v == "true"),
            Some("hash") => c.sha.starts_with(&v),
            Some("parent") => c.parents.iter().any(|p| p.starts_with(&v)),
            Some("tree") => c.tree.starts_with(&v),
            Some("repo" | "user" | "org" | "owner" | "is" | "sort") => true,
            Some(_) => msg.contains(&format!("{}:{v}", t.key.as_deref().unwrap_or(""))),
        };
        if ok == t.negated {
            return false;
        }
    }
    true
}

async fn commit_hits(s: &AppState, repos: &[RepoCtx]) -> Vec<CommitHit> {
    const FMT: &str = "--format=%H%x1f%P%x1f%T%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%ce%x1f%cI%x1f%B%x1e";
    let max = format!("-n{MAX_COMMITS_PER_REPO}");
    let mut out = Vec::new();
    for (idx, ctx) in repos.iter().enumerate() {
        let Ok(path) = s.git.sync(ctx.repo.id).await else { continue };
        let Ok((true, log, _)) = s.git.git.try_run(&path, &["log", &max, FMT, "HEAD", "--"]).await else { continue };
        for rec in String::from_utf8_lossy(&log).split('\u{1e}') {
            let f: Vec<&str> = rec.trim_start_matches('\n').splitn(10, '\u{1f}').collect();
            if f.len() < 10 || f[0].len() < 40 {
                continue;
            }
            out.push(CommitHit {
                repo: idx,
                sha: f[0].to_string(),
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                tree: f[2].to_string(),
                author_name: f[3].to_string(),
                author_email: f[4].to_string(),
                author_date: parse_git_date(f[5]),
                committer_name: f[6].to_string(),
                committer_email: f[7].to_string(),
                committer_date: parse_git_date(f[8]),
                message: f[9].trim_end().to_string(),
            });
        }
    }
    out
}

/// Maps commit e-mails to forge users (account e-mail or the noreply address forge uses).
async fn users_by_email(s: &AppState, emails: &[String]) -> ApiResult<HashMap<String, User>> {
    let mut lower: Vec<String> = emails.iter().map(|e| e.to_lowercase()).collect();
    lower.sort();
    lower.dedup();
    let users: Vec<User> = sqlx::query_as(
        "SELECT * FROM users WHERE lower(email) = ANY($1) OR (id::text || '+' || lower(login) || '@users.noreply.forge') = ANY($1)",
    )
    .bind(&lower)
    .fetch_all(&s.db)
    .await?;
    let mut out = HashMap::new();
    for u in users {
        if let Some(e) = &u.email {
            out.insert(e.to_lowercase(), u.clone());
        }
        out.insert(format!("{}+{}@users.noreply.forge", u.id, u.login.to_lowercase()), u);
    }
    Ok(out)
}

async fn search_commits_handler(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchQuery>) -> ApiResult<Response> {
    let query = q.query()?;
    let p = q.pages();
    let terms = parse_query(query);
    let repos = scope_repos(&s, u.as_ref(), &terms).await?;
    let mut hits = commit_hits(&s, &repos).await;
    let emails: Vec<String> = hits.iter().flat_map(|h| [h.author_email.clone(), h.committer_email.clone()]).collect();
    let users = users_by_email(&s, &emails).await?;
    let logins: HashMap<String, String> = users.iter().map(|(e, u)| (e.clone(), u.login.clone())).collect();
    hits.retain(|h| commit_matches(h, &terms, &logins, u.as_ref()));
    let asc = q.order.as_deref() == Some("asc");
    match q.sort.as_deref() {
        Some("author-date") => hits.sort_by_key(|h| h.author_date),
        _ => hits.sort_by_key(|h| h.committer_date),
    }
    if !asc {
        hits.reverse();
    }
    let total = hits.len() as i64;
    let api = s.api_url();
    let base = s.base_url();
    let repo_json: Vec<Value> = repos.iter().map(|c| json::repo::repo(&api, base, c)).collect();
    let items = hits
        .iter()
        .skip(p.offset() as usize)
        .take(p.per_page() as usize)
        .map(|h| {
            let full = repos[h.repo].full_name();
            let url = format!("{api}/repos/{full}/commits/{}", h.sha);
            let author = users.get(&h.author_email.to_lowercase()).map(|u| json::user::simple(&api, base, u));
            let committer = json!({ "name": h.committer_name, "email": h.committer_email, "date": json::ts(h.committer_date) });
            with_score(json!({
                "url": url,
                "sha": h.sha,
                "node_id": crate::ids::encode_str("Commit", &h.sha),
                "html_url": format!("{base}/{full}/commit/{}", h.sha),
                "comments_url": format!("{url}/comments"),
                "commit": {
                    "url": format!("{api}/repos/{full}/git/commits/{}", h.sha),
                    "author": { "name": h.author_name, "email": h.author_email, "date": json::ts(h.author_date) },
                    "committer": { "name": h.committer_name, "email": h.committer_email, "date": json::ts(h.committer_date) },
                    "message": h.message,
                    "comment_count": 0,
                    "tree": { "sha": h.tree, "url": format!("{api}/repos/{full}/git/trees/{}", h.tree) },
                },
                "author": author,
                "committer": committer,
                "parents": h.parents.iter().map(|p| json!({
                    "url": format!("{api}/repos/{full}/commits/{p}"),
                    "html_url": format!("{base}/{full}/commit/{p}"),
                    "sha": p,
                })).collect::<Vec<_>>(),
                "repository": repo_json[h.repo],
            }))
        })
        .collect();
    Ok(search_response(&uri, &s, p, total, items, None))
}
