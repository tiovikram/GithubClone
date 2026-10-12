//! Search: code (git grep over default branches), commits (git log --grep), labels, topics.
//! Repository/issue/user search belongs to the issues area.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::PageParams;
use crate::auth::MaybeUser;
use crate::db::{self, Repo, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/search/code", get(search_code))
        .route("/search/commits", get(search_commits))
        .route("/search/labels", get(search_labels))
        .route("/search/topics", get(search_topics))
}

/// Max repositories scanned by a code/commit search without a `repo:` qualifier.
const MAX_REPOS: i64 = 30;
const MAX_RESULTS: usize = 1000;

#[derive(Debug, Default)]
pub struct ParsedQuery {
    pub terms: Vec<String>,
    pub quals: Vec<(String, String)>,
}

impl ParsedQuery {
    pub fn all(&self, k: &str) -> Vec<&str> {
        self.quals.iter().filter(|(q, _)| q == k).map(|(_, v)| v.as_str()).collect()
    }
    pub fn first(&self, k: &str) -> Option<&str> {
        self.all(k).into_iter().next()
    }
}

/// Splits a search query into free-text terms and `key:value` qualifiers (quotes respected).
pub fn parse_query(q: &str) -> ParsedQuery {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in q.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut out = ParsedQuery::default();
    for t in tokens {
        match t.split_once(':') {
            Some((k, v)) if !k.is_empty() && !v.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                out.quals.push((k.to_ascii_lowercase(), v.to_string()));
            }
            _ => {
                if !matches!(t.as_str(), "AND" | "OR" | "NOT") {
                    out.terms.push(t);
                }
            }
        }
    }
    out
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
    repository_id: Option<i64>,
}

impl SearchParams {
    fn pages(&self) -> PageParams {
        PageParams { page: self.page, per_page: self.per_page }
    }
}

fn missing_q() -> ApiError {
    ApiError::validation("Search", "q", "missing")
}

/// JSON search envelope with GitHub-style Link headers.
fn search_response(s: &AppState, uri: &OriginalUri, p: PageParams, total: i64, incomplete: bool, items: Vec<Value>) -> Response {
    let mut res = axum::Json(json!({ "total_count": total, "incomplete_results": incomplete, "items": items })).into_response();
    let per = p.per_page();
    let page = p.page();
    let last = ((total.min(MAX_RESULTS as i64) + per - 1) / per).max(1);
    let url = |n: i64| {
        let mut pairs: Vec<(String, String)> = uri
            .0
            .query()
            .map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect())
            .unwrap_or_default();
        pairs.retain(|(k, _)| k != "page" && k != "per_page");
        pairs.push(("per_page".into(), per.to_string()));
        pairs.push(("page".into(), n.to_string()));
        let q = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        format!("{}{}?{q}", s.base_url(), uri.0.path())
    };
    let mut links = Vec::new();
    if page < last {
        links.push(format!("<{}>; rel=\"next\"", url(page + 1)));
        links.push(format!("<{}>; rel=\"last\"", url(last)));
    }
    if page > 1 {
        links.push(format!("<{}>; rel=\"first\"", url(1)));
        links.push(format!("<{}>; rel=\"prev\"", url(page - 1)));
    }
    if !links.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&links.join(", ")) {
            res.headers_mut().insert(header::LINK, v);
        }
    }
    res
}

/// Repositories a code/commit search covers: `repo:` qualifiers, else `user:`/`org:` owners,
/// else the most recently pushed repositories visible to the viewer (bounded).
async fn target_repos(s: &AppState, viewer: Option<&User>, pq: &ParsedQuery) -> ApiResult<(Vec<RepoCtx>, bool)> {
    let mut out = Vec::new();
    let repos = pq.all("repo");
    if !repos.is_empty() {
        for r in repos {
            if let Some((o, n)) = r.split_once('/') {
                if let Ok(ctx) = db::load_repo(&s.db, viewer, o, n).await {
                    out.push(ctx);
                }
            }
        }
        return Ok((out, false));
    }
    let owners: Vec<String> = pq.all("user").into_iter().chain(pq.all("org")).map(|o| o.to_lowercase()).collect();
    let vid = viewer.map(|u| u.id).unwrap_or(-1);
    let admin = viewer.is_some_and(|u| u.site_admin);
    let rows: Vec<Repo> = sqlx::query_as(
        "SELECT r.* FROM repositories r JOIN users o ON o.id = r.owner_id
          WHERE (cardinality($1::text[]) = 0 OR lower(o.login) = ANY($1))
            AND (NOT r.private OR $3 OR r.owner_id = $2
                 OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = $2)
                 OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = $2))
          ORDER BY r.pushed_at DESC NULLS LAST, r.id DESC LIMIT $4",
    )
    .bind(&owners)
    .bind(vid)
    .bind(admin)
    .bind(MAX_REPOS + 1)
    .fetch_all(&s.db)
    .await?;
    let incomplete = rows.len() as i64 > MAX_REPOS;
    let owner_ids: Vec<i64> = rows.iter().map(|r| r.owner_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &owner_ids).await?;
    for r in rows.into_iter().take(MAX_REPOS as usize) {
        if let Some(o) = users.get(&r.owner_id) {
            let perm = db::permission(&s.db, viewer, &r, o).await?;
            out.push(RepoCtx { repo: r, owner: o.clone(), perm });
        }
    }
    Ok((out, incomplete))
}

fn language_exts(lang: &str) -> Vec<&'static str> {
    match lang.to_ascii_lowercase().as_str() {
        "rust" => vec!["rs"],
        "javascript" | "js" => vec!["js", "mjs", "cjs", "jsx"],
        "typescript" | "ts" => vec!["ts", "tsx"],
        "python" => vec!["py"],
        "go" => vec!["go"],
        "ruby" => vec!["rb"],
        "java" => vec!["java"],
        "c" => vec!["c", "h"],
        "c++" | "cpp" => vec!["cpp", "cc", "hpp", "cxx"],
        "markdown" => vec!["md", "markdown"],
        "shell" => vec!["sh", "bash"],
        "json" => vec!["json"],
        "yaml" => vec!["yml", "yaml"],
        "html" => vec!["html", "htm"],
        "css" => vec!["css"],
        _ => vec![],
    }
}

fn lang_of(path: &str) -> Value {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let l = match ext.as_str() {
        "rs" => "Rust",
        "js" | "mjs" | "cjs" | "jsx" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "py" => "Python",
        "go" => "Go",
        "rb" => "Ruby",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "cc" | "hpp" => "C++",
        "md" => "Markdown",
        "sh" => "Shell",
        "json" => "JSON",
        "yml" | "yaml" => "YAML",
        "html" => "HTML",
        "css" => "CSS",
        "txt" => "Text",
        _ => return Value::Null,
    };
    json!(l)
}

/// Searches file contents on the default branch of each target repository with `git grep`.
pub async fn code_search(s: &AppState, viewer: Option<&User>, q: &str) -> ApiResult<(Vec<Value>, bool)> {
    let pq = parse_query(q);
    if pq.terms.is_empty() {
        return Err(ApiError::validation("Search", "q", "invalid"));
    }
    let (repos, mut incomplete) = target_repos(s, viewer, &pq).await?;
    let mut pathspecs: Vec<String> = Vec::new();
    for p in pq.all("path") {
        let p = p.trim_start_matches('/');
        pathspecs.push(format!(":(glob){p}/**"));
        pathspecs.push(format!(":(glob){p}"));
    }
    for f in pq.all("filename") {
        pathspecs.push(format!(":(glob)**/{f}"));
    }
    for e in pq.all("extension") {
        pathspecs.push(format!(":(glob)**/*.{}", e.trim_start_matches('.')));
    }
    for l in pq.all("language") {
        for e in language_exts(l) {
            pathspecs.push(format!(":(glob)**/*.{e}"));
        }
    }
    let api = s.api_url();
    let base = s.base_url();
    let mut items = Vec::new();
    for ctx in repos {
        let (_, refs) = s.git.refs(ctx.repo.id).await?;
        let refname = format!("refs/heads/{}", ctx.repo.default_branch);
        let Some(commit) = refs.get(&refname).cloned() else { continue };
        let path = s.git.sync(ctx.repo.id).await?;
        let mut args: Vec<String> = vec!["grep".into(), "-l".into(), "-I".into(), "-i".into(), "-F".into(), "--all-match".into()];
        for t in &pq.terms {
            args.push("-e".into());
            args.push(t.clone());
        }
        args.push(commit.clone());
        args.push("--".into());
        args.extend(pathspecs.iter().cloned());
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let (ok, out, _) = s.git.git.try_run(&path, &argv).await?;
        if !ok {
            continue; // exit 1 = no match
        }
        let text = String::from_utf8_lossy(&out);
        let files: Vec<String> = text
            .lines()
            .filter_map(|l| l.split_once(':').map(|(_, p)| p.to_string()))
            .take(100)
            .collect();
        if files.len() == 100 {
            incomplete = true;
        }
        if files.is_empty() {
            continue;
        }
        let input: String = files.iter().map(|f| format!("{commit}:{f}\n")).collect();
        let shas = s.git.git.run_input(&path, &["cat-file", "--batch-check=%(objectname) %(objectsize)"], input.as_bytes()).await?;
        let shas = String::from_utf8_lossy(&shas);
        let full = ctx.full_name();
        let repo_json = crate::json::repo::repo(&api, base, &ctx);
        for (f, line) in files.iter().zip(shas.lines()) {
            let mut parts = line.split_whitespace();
            let sha = parts.next().unwrap_or_default().to_string();
            let size: i64 = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let name = f.rsplit('/').next().unwrap_or(f);
            let enc = crate::json::release::seg;
            let path_enc: String = f.split('/').map(enc).collect::<Vec<_>>().join("/");
            items.push(json!({
                "name": name,
                "path": f,
                "sha": sha,
                "url": format!("{api}/repos/{full}/contents/{path_enc}?ref={commit}"),
                "git_url": format!("{api}/repos/{full}/git/blobs/{sha}"),
                "html_url": format!("{base}/{full}/blob/{commit}/{path_enc}"),
                "repository": repo_json,
                "score": 1.0,
                "file_size": size,
                "language": lang_of(f),
            }));
        }
        if items.len() >= MAX_RESULTS {
            incomplete = true;
            break;
        }
    }
    Ok((items, incomplete))
}

async fn search_code(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchParams>) -> ApiResult<Response> {
    let query = q.q.clone().filter(|x| !x.trim().is_empty()).ok_or_else(missing_q)?;
    let (items, incomplete) = code_search(&s, u.as_ref(), &query).await?;
    let p = q.pages();
    let total = items.len() as i64;
    let page: Vec<Value> = items.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    Ok(search_response(&s, &uri, p, total, incomplete, page))
}

fn date_filter(v: &str) -> Vec<String> {
    // Supports `>=D`, `>D`, `<=D`, `<D`, `D..D`, and plain `D` (a day).
    let mut out = Vec::new();
    if let Some((a, b)) = v.split_once("..") {
        if a != "*" {
            out.push(format!("--since={a}"));
        }
        if b != "*" {
            out.push(format!("--until={b}T23:59:59Z"));
        }
    } else if let Some(d) = v.strip_prefix(">=").or_else(|| v.strip_prefix('>')) {
        out.push(format!("--since={d}"));
    } else if let Some(d) = v.strip_prefix("<=").or_else(|| v.strip_prefix('<')) {
        out.push(format!("--until={d}"));
    } else {
        out.push(format!("--since={v}T00:00:00Z"));
        out.push(format!("--until={v}T23:59:59Z"));
    }
    out
}

/// Searches commit messages (and author/committer/hash qualifiers) on default branches.
pub async fn commit_search(s: &AppState, viewer: Option<&User>, q: &str, sort: Option<&str>, order: Option<&str>) -> ApiResult<(Vec<Value>, bool)> {
    let pq = parse_query(q);
    let (repos, mut incomplete) = target_repos(s, viewer, &pq).await?;
    let mut filters: Vec<String> = vec!["-i".into(), "--all-match".into()];
    for t in &pq.terms {
        filters.push(format!("--grep={t}"));
    }
    for login in pq.all("author") {
        if let Ok(u) = db::user_by_login(&s.db, login).await {
            filters.push(format!("--author={}", u.email.clone().unwrap_or_else(|| format!("{}+{}@users.noreply.forge", u.id, u.login))));
        } else {
            return Ok((vec![], false));
        }
    }
    for login in pq.all("committer") {
        if let Ok(u) = db::user_by_login(&s.db, login).await {
            filters.push(format!("--committer={}", u.email.clone().unwrap_or_else(|| format!("{}+{}@users.noreply.forge", u.id, u.login))));
        } else {
            return Ok((vec![], false));
        }
    }
    for v in pq.all("author-name").into_iter().chain(pq.all("author-email")) {
        filters.push(format!("--author={v}"));
    }
    for v in pq.all("committer-name").into_iter().chain(pq.all("committer-email")) {
        filters.push(format!("--committer={v}"));
    }
    match pq.first("merge") {
        Some("true") => filters.push("--merges".into()),
        Some("false") => filters.push("--no-merges".into()),
        _ => {}
    }
    for d in pq.all("author-date").into_iter().chain(pq.all("committer-date")) {
        filters.extend(date_filter(d));
    }
    let hashes: Vec<String> = pq.all("hash").into_iter().map(|h| h.to_lowercase()).collect();
    let api = s.api_url();
    let base = s.base_url();
    struct Hit {
        date: String,
        v: Value,
        email: String,
    }
    let mut hits: Vec<Hit> = Vec::new();
    for ctx in repos {
        let (_, refs) = s.git.refs(ctx.repo.id).await?;
        let refname = format!("refs/heads/{}", ctx.repo.default_branch);
        if !refs.contains_key(&refname) {
            continue;
        }
        let path = s.git.sync(ctx.repo.id).await?;
        let mut args: Vec<String> = vec!["log".into(), "-n".into(), "200".into(), "--format=%x1e%H%x00%T%x00%P%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%B".into()];
        args.extend(filters.iter().cloned());
        args.push(refname);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let (ok, out, _) = s.git.git.try_run(&path, &argv).await?;
        if !ok {
            continue;
        }
        let text = String::from_utf8_lossy(&out);
        let full = ctx.full_name();
        let repo_json = crate::json::repo::repo(&api, base, &ctx);
        let mut n = 0;
        for rec in text.split('\x1e').filter(|r| !r.trim().is_empty()) {
            let f: Vec<&str> = rec.splitn(10, '\0').collect();
            if f.len() < 10 {
                continue;
            }
            let sha = f[0].trim();
            if !hashes.is_empty() && !hashes.iter().any(|h| sha.starts_with(h.as_str())) {
                continue;
            }
            n += 1;
            let url = format!("{api}/repos/{full}/commits/{sha}");
            let parents: Vec<Value> = f[2]
                .split_whitespace()
                .map(|p| json!({"url": format!("{api}/repos/{full}/commits/{p}"), "html_url": format!("{base}/{full}/commit/{p}"), "sha": p}))
                .collect();
            hits.push(Hit {
                date: if sort == Some("committer-date") { f[8].to_string() } else { f[5].to_string() },
                email: f[4].to_string(),
                v: json!({
                    "url": url,
                    "sha": sha,
                    "html_url": format!("{base}/{full}/commit/{sha}"),
                    "comments_url": format!("{url}/comments"),
                    "commit": {
                        "author": {"name": f[3], "email": f[4], "date": crate::json::ts(chrono::DateTime::parse_from_rfc3339(f[5]).map(|d| d.with_timezone(&chrono::Utc)).unwrap_or_default())},
                        "committer": {"name": f[6], "email": f[7], "date": crate::json::ts(chrono::DateTime::parse_from_rfc3339(f[8]).map(|d| d.with_timezone(&chrono::Utc)).unwrap_or_default())},
                        "comment_count": 0,
                        "message": f[9].trim_end(),
                        "tree": {"sha": f[1], "url": format!("{api}/repos/{full}/git/trees/{}", f[1])},
                        "url": format!("{api}/repos/{full}/git/commits/{sha}"),
                        "verification": {"verified": false, "reason": "unsigned", "payload": null, "signature": null, "verified_at": null},
                    },
                    "author": null,
                    "committer": null,
                    "parents": parents,
                    "repository": repo_json,
                    "score": 1.0,
                    "node_id": crate::ids::encode_str("Commit", sha),
                }),
            });
        }
        if n >= 200 {
            incomplete = true;
        }
    }
    if sort.is_some() {
        hits.sort_by(|a, b| a.date.cmp(&b.date));
        if order != Some("asc") {
            hits.reverse();
        }
    }
    let emails: Vec<String> = hits.iter().map(|h| h.email.clone()).collect();
    let users = super::plat::users_by_emails(&s.db, &emails).await?;
    Ok((
        hits.into_iter()
            .map(|mut h| {
                if let Some(u) = users.get(&h.email.to_lowercase()) {
                    let uj = crate::json::user::simple(&api, base, u);
                    h.v["author"] = uj.clone();
                    h.v["committer"] = uj;
                }
                h.v
            })
            .collect(),
        incomplete,
    ))
}

async fn search_commits(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchParams>) -> ApiResult<Response> {
    let query = q.q.clone().filter(|x| !x.trim().is_empty()).ok_or_else(missing_q)?;
    let (items, incomplete) = commit_search(&s, u.as_ref(), &query, q.sort.as_deref(), q.order.as_deref()).await?;
    let p = q.pages();
    let total = items.len() as i64;
    let page: Vec<Value> = items.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    Ok(search_response(&s, &uri, p, total, incomplete, page))
}

async fn search_labels(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<SearchParams>) -> ApiResult<Response> {
    let repo_id = q.repository_id.ok_or_else(|| ApiError::validation("Search", "repository_id", "missing"))?;
    let query = q.q.clone().filter(|x| !x.trim().is_empty()).ok_or_else(missing_q)?;
    let ctx = db::load_repo_by_id(&s.db, u.as_ref(), repo_id).await?;
    let pq = parse_query(&query);
    let pats: Vec<String> = pq.terms.iter().map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_"))).collect();
    let p = q.pages();
    let order = match (q.sort.as_deref(), q.order.as_deref()) {
        (Some("created"), Some("asc")) => "created_at ASC, id ASC",
        (Some("created"), _) => "created_at DESC, id DESC",
        (_, Some("asc")) => "name DESC",
        _ => "lower(name) ASC",
    };
    let sql = format!(
        "SELECT id, name, color, description, is_default FROM labels
          WHERE repo_id = $1 AND (cardinality($2::text[]) = 0
             OR EXISTS(SELECT 1 FROM unnest($2::text[]) p WHERE lower(name) LIKE p OR lower(coalesce(description, '')) LIKE p))
          ORDER BY {order}"
    );
    let rows: Vec<(i64, String, String, Option<String>, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(ctx.repo.id).bind(&pats).fetch_all(&s.db).await?;
    let api = s.api_url();
    let full = ctx.full_name();
    let total = rows.len() as i64;
    let items: Vec<Value> = rows
        .into_iter()
        .skip(p.offset() as usize)
        .take(p.limit() as usize)
        .map(|(id, name, color, description, default)| {
            json!({
                "id": id,
                "node_id": crate::ids::encode("Label", id),
                "url": format!("{api}/repos/{full}/labels/{}", crate::json::issue::urlencode(&name)),
                "name": name,
                "color": color,
                "default": default,
                "description": description,
                "archived_at": null,
                "archived_by": null,
                "score": 1.0,
            })
        })
        .collect();
    Ok(search_response(&s, &uri, p, total, false, items))
}

async fn search_topics(State(s): State<AppState>, uri: OriginalUri, _u: MaybeUser, Query(q): Query<SearchParams>) -> ApiResult<Response> {
    let query = q.q.clone().filter(|x| !x.trim().is_empty()).ok_or_else(missing_q)?;
    let pq = parse_query(&query);
    let pats: Vec<String> = pq.terms.iter().map(|t| format!("%{}%", t.to_lowercase())).collect();
    let rows: Vec<(String, i64, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT t, count(*), min(r.created_at), max(r.updated_at) FROM repositories r, unnest(r.topics) t
          WHERE NOT r.private AND (cardinality($1::text[]) = 0 OR EXISTS(SELECT 1 FROM unnest($1::text[]) p WHERE t LIKE p))
          GROUP BY t ORDER BY count(*) DESC, t LIMIT 1000",
    )
    .bind(&pats)
    .fetch_all(&s.db)
    .await?;
    let p = q.pages();
    let total = rows.len() as i64;
    let exact: HashMap<String, ()> = pq.terms.iter().map(|t| (t.to_lowercase(), ())).collect();
    let items: Vec<Value> = rows
        .into_iter()
        .skip(p.offset() as usize)
        .take(p.limit() as usize)
        .map(|(name, n, c, u)| {
            json!({
                "name": name,
                "display_name": null,
                "short_description": null,
                "description": null,
                "created_by": null,
                "released": null,
                "created_at": crate::json::ts(c),
                "updated_at": crate::json::ts(u),
                "featured": false,
                "curated": false,
                "score": if exact.contains_key(&name) { 2.0 } else { 1.0 },
                "repository_count": n,
            })
        })
        .collect();
    Ok(search_response(&s, &uri, p, total, false, items))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse() {
        let q = super::parse_query("feature repo:a/b \"two words\" path:src");
        assert_eq!(q.terms, vec!["feature", "two words"]);
        assert_eq!(q.first("repo"), Some("a/b"));
        assert_eq!(q.first("path"), Some("src"));
    }
}
