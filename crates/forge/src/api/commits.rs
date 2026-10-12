//! Commits: list, get (with stats/files, diff/patch media types), compare, contributors,
//! branches-where-head.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::gitapi::{self, GitResult};
use super::{PageParams, Paged};
use crate::auth::MaybeUser;
use crate::db::{self, RepoCtx};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::read::{self, CommitInfo, FileChange, LogQuery};
use crate::json::git::{self as gj, RepoUrls};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/commits", get(list_commits))
        .route("/repos/{owner}/{repo}/commits/{ref}", get(get_commit))
        .route("/repos/{owner}/{repo}/commits/{ref}/branches-where-head", get(branches_where_head))
        .route("/repos/{owner}/{repo}/compare/{*basehead}", get(compare))
        .route("/repos/{owner}/{repo}/contributors", get(contributors))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    sha: Option<String>,
    path: Option<String>,
    author: Option<String>,
    committer: Option<String>,
    since: Option<String>,
    until: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// Patterns matching a GitHub `author`/`committer` filter: an email, or a login (the user's
/// emails, including the noreply address forge commits with).
async fn identity_patterns(s: &AppState, who: &str) -> ApiResult<Vec<String>> {
    if who.contains('@') {
        return Ok(vec![format!("<{who}>")]);
    }
    match db::user_by_login(&s.db, who).await {
        Ok(u) => {
            let mut v = vec![format!("<{}+{}@users.noreply.", u.id, u.login)];
            if let Some(e) = u.email.filter(|e| !e.is_empty()) {
                v.push(format!("<{e}>"));
            }
            Ok(v)
        }
        Err(_) => Ok(vec![who.to_string()]),
    }
}

fn valid_date(d: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(d).is_ok() || chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()
}

/// Lists commits reachable from `rev` (newest first) for the commits API.
pub async fn list_commit_infos(s: &AppState, ctx: &RepoCtx, q: &LogQuery) -> ApiResult<(Vec<CommitInfo>, usize)> {
    let path = gitapi::sync(s, ctx).await?;
    let git = &s.git.git;
    let shas = read::rev_list(git, &path, q).await.api()?;
    let total = if q.limit.is_some_and(|l| shas.len() < l) && q.skip == 0 {
        shas.len()
    } else {
        read::rev_count(git, &path, q).await.api()?
    };
    let commits = read::read_commits(git, &path, &shas).await.api()?.into_iter().flatten().collect();
    Ok((commits, total))
}

async fn list_commits(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let rev = q.sha.clone().filter(|x| !x.is_empty()).unwrap_or_else(|| ctx.repo.default_branch.clone());
    let head = read::resolve_commit(&s.git.git, &path, &rev)
        .await
        .api()?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("No commit found for SHA: {rev}")))?;
    for d in [&q.since, &q.until].into_iter().flatten() {
        if !valid_date(d) {
            return Err(ApiError::validation("Commit", "since", "invalid"));
        }
    }
    let pages = PageParams { page: q.page, per_page: q.per_page };
    let mut lq = LogQuery {
        revs: vec![head],
        path: q.path.as_deref().and_then(gitapi::clean_path).filter(|p| !p.is_empty()),
        since: q.since.clone(),
        until: q.until.clone(),
        skip: pages.offset() as usize,
        limit: Some(pages.per_page() as usize),
        ..Default::default()
    };
    if let Some(a) = q.author.as_deref().filter(|a| !a.is_empty()) {
        lq.authors = identity_patterns(&s, a).await?;
    }
    if let Some(c) = q.committer.as_deref().filter(|a| !a.is_empty()) {
        lq.committers = identity_patterns(&s, c).await?;
    }
    let (commits, total) = list_commit_infos(&s, &ctx, &lq).await?;
    let items = gitapi::commits_json(&s, &gitapi::urls(&s, &ctx), &commits).await?;
    Ok(Paged::new(&uri, &s, pages, Some(total as i64), items).into_response())
}

/// Full `commit` JSON with `stats` and `files` (diff against the first parent).
pub async fn commit_detail(s: &AppState, ctx: &RepoCtx, sha: &str) -> ApiResult<Value> {
    let path = gitapi::sync(s, ctx).await?;
    let git = &s.git.git;
    let c = read::read_commit(git, &path, sha).await.api()?.ok_or_else(ApiError::not_found)?;
    let files = read::diff(git, &path, c.parents.first().map(String::as_str), &c.sha, true).await.api()?;
    let u = gitapi::urls(s, ctx);
    let mut v = gitapi::commits_json(s, &u, std::slice::from_ref(&c)).await?.pop().unwrap_or(Value::Null);
    v["stats"] = gj::stats(&files);
    v["files"] = files_json(&u, &c.sha, &files);
    Ok(v)
}

fn files_json(u: &RepoUrls, rev: &str, files: &[FileChange]) -> Value {
    Value::Array(files.iter().take(read::MAX_DIFF_FILES).map(|f| gj::diff_entry(u, rev, f)).collect())
}

async fn get_commit(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    headers: HeaderMap,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let git = &s.git.git;
    let sha = read::resolve_commit(git, &path, &r)
        .await
        .api()?
        .ok_or_else(|| ApiError::unprocessable(format!("No commit found for SHA: {r}")))?;
    match gitapi::media(&headers) {
        Some("sha") => return Ok(gitapi::typed("application/vnd.github.sha", sha.into_bytes())),
        Some("diff") => {
            let c = read::read_commit(git, &path, &sha).await.api()?.ok_or_else(ApiError::not_found)?;
            let d = read::diff_text(git, &path, c.parents.first().map(String::as_str), &sha).await.api()?;
            return Ok(gitapi::typed("application/vnd.github.diff; charset=utf-8", d));
        }
        Some("patch") => {
            let p = read::format_patch(git, &path, &["-1", &sha]).await.api()?;
            return Ok(gitapi::typed("application/vnd.github.patch; charset=utf-8", p));
        }
        _ => {}
    }
    Ok(Json(commit_detail(&s, &ctx, &sha).await?).into_response())
}

async fn branches_where_head(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let sha = read::resolve_commit(&s.git.git, &path, &sha)
        .await
        .api()?
        .ok_or_else(|| ApiError::unprocessable(format!("No commit found for SHA: {sha}")))?;
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    let u = gitapi::urls(&s, &ctx);
    let protected = super::branches::protected_branches(&s, &ctx).await;
    let items: Vec<Value> = refs
        .iter()
        .filter(|(n, v)| n.starts_with("refs/heads/") && **v == sha)
        .map(|(n, v)| {
            let name = &n["refs/heads/".len()..];
            gj::short_branch(&u, name, v, protected.contains(name))
        })
        .collect();
    Ok(Json(Value::Array(items)))
}

// ---- compare ----

/// Result of comparing two revisions.
#[derive(Debug, Clone)]
pub struct Comparison {
    pub base: CommitInfo,
    pub head: CommitInfo,
    pub merge_base: CommitInfo,
    pub ahead_by: usize,
    pub behind_by: usize,
    /// Commits in base..head, oldest first (paged).
    pub commits: Vec<CommitInfo>,
    pub files: Vec<FileChange>,
}

impl Comparison {
    pub fn status(&self) -> &'static str {
        match (self.ahead_by, self.behind_by) {
            (0, 0) => "identical",
            (_, 0) => "ahead",
            (0, _) => "behind",
            _ => "diverged",
        }
    }
}

/// Strips a same-repository `owner:` (or `owner:repo:`) prefix from a compare side.
fn strip_owner<'a>(ctx: &RepoCtx, side: &'a str) -> Option<&'a str> {
    match side.split_once(':') {
        None => Some(side),
        Some((owner, rest)) if owner.eq_ignore_ascii_case(&ctx.owner.login) => {
            match rest.split_once(':') {
                Some((repo, r)) if repo.eq_ignore_ascii_case(&ctx.repo.name) => Some(r),
                Some(_) => None,
                None => Some(rest),
            }
        }
        Some(_) => None,
    }
}

/// Compares `base...head` (three-dot: files are diffed from the merge base).
pub async fn compare_revs(
    s: &AppState,
    ctx: &RepoCtx,
    base: &str,
    head: &str,
    page: Option<PageParams>,
    with_patch: bool,
) -> ApiResult<Comparison> {
    let path = gitapi::sync(s, ctx).await?;
    let git = &s.git.git;
    let b = read::resolve_commit(git, &path, base).await.api()?.ok_or_else(ApiError::not_found)?;
    let h = read::resolve_commit(git, &path, head).await.api()?.ok_or_else(ApiError::not_found)?;
    let mb = read::merge_base(git, &path, &b, &h)
        .await
        .api()?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("No common ancestor between {base} and {head}.")))?;
    let ahead_q = LogQuery { revs: vec![format!("^{b}"), h.clone()], ..Default::default() };
    let behind_q = LogQuery { revs: vec![format!("^{h}"), b.clone()], ..Default::default() };
    let ahead_by = read::rev_count(git, &path, &ahead_q).await.api()?;
    let behind_by = read::rev_count(git, &path, &behind_q).await.api()?;
    let (skip, limit) = match page {
        Some(p) => (p.offset() as usize, p.per_page() as usize),
        None => (0, 250),
    };
    // Oldest first, like GitHub: take the window from the end of the newest-first listing.
    let list_q = LogQuery { revs: vec![format!("^{b}"), h.clone()], reverse: true, ..Default::default() };
    let all = read::rev_list(git, &path, &list_q).await.api()?;
    let window: Vec<String> = all.into_iter().skip(skip).take(limit).collect();
    let commits = read::read_commits(git, &path, &window).await.api()?.into_iter().flatten().collect();
    let files = read::diff(git, &path, Some(&mb), &h, with_patch).await.api()?;
    let mut infos = read::read_commits(git, &path, &[b.clone(), h.clone(), mb.clone()]).await.api()?.into_iter();
    let (Some(Some(bc)), Some(Some(hc)), Some(Some(mbc))) = (infos.next(), infos.next(), infos.next()) else {
        return Err(ApiError::not_found());
    };
    Ok(Comparison { base: bc, head: hc, merge_base: mbc, ahead_by, behind_by, commits, files })
}

#[derive(Deserialize, Default)]
struct CompareQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn compare(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    headers: HeaderMap,
    Path((owner, repo, basehead)): Path<(String, String, String)>,
    Query(q): Query<CompareQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let (spec, media_suffix) = match basehead.rsplit_once('.') {
        Some((x, "diff")) if x.contains("..") => (x.to_string(), Some("diff")),
        Some((x, "patch")) if x.contains("..") => (x.to_string(), Some("patch")),
        _ => (basehead.clone(), None),
    };
    let (base_raw, head_raw) = spec.split_once("...").or_else(|| spec.split_once("..")).ok_or_else(ApiError::not_found)?;
    let base = strip_owner(&ctx, base_raw).ok_or_else(ApiError::not_found)?;
    let head = strip_owner(&ctx, head_raw).ok_or_else(ApiError::not_found)?;
    let media = media_suffix.or(gitapi::media(&headers));
    if matches!(media, Some("diff") | Some("patch")) {
        let path = gitapi::sync(&s, &ctx).await?;
        let git = &s.git.git;
        let b = read::resolve_commit(git, &path, base).await.api()?.ok_or_else(ApiError::not_found)?;
        let h = read::resolve_commit(git, &path, head).await.api()?.ok_or_else(ApiError::not_found)?;
        let mb = read::merge_base(git, &path, &b, &h).await.api()?.ok_or_else(ApiError::not_found)?;
        return Ok(if media == Some("diff") {
            gitapi::typed("application/vnd.github.diff; charset=utf-8", read::diff_text(git, &path, Some(&mb), &h).await.api()?)
        } else {
            let range = format!("{b}..{h}");
            gitapi::typed("application/vnd.github.patch; charset=utf-8", read::format_patch(git, &path, &[&range]).await.api()?)
        });
    }
    let page = (q.page.is_some() || q.per_page.is_some()).then_some(PageParams { page: q.page, per_page: q.per_page });
    let cmp = compare_revs(&s, &ctx, base, head, page, true).await?;
    let urls = gitapi::urls(&s, &ctx);
    Ok(Json(comparison_json(&s, &ctx, &urls, &spec, &cmp).await?).into_response())
}

/// `commit-comparison` JSON.
pub async fn comparison_json(s: &AppState, ctx: &RepoCtx, u: &RepoUrls, basehead: &str, c: &Comparison) -> ApiResult<Value> {
    let mut all = vec![c.base.clone(), c.merge_base.clone()];
    all.extend(c.commits.iter().cloned());
    let mut rendered = gitapi::commits_json(s, u, &all).await?.into_iter();
    let base_json = rendered.next().unwrap_or(Value::Null);
    let mb_json = rendered.next().unwrap_or(Value::Null);
    let commits: Vec<Value> = rendered.collect();
    let bh = gj::enc_path(basehead);
    let owner = &ctx.owner.login;
    Ok(json!({
        "url": format!("{}/compare/{bh}", u.repo_api),
        "html_url": format!("{}/compare/{bh}", u.html),
        "permalink_url": format!("{}/compare/{owner}:{}...{owner}:{}", u.html, &c.base.sha[..7], &c.head.sha[..7]),
        "diff_url": format!("{}/compare/{bh}.diff", u.html),
        "patch_url": format!("{}/compare/{bh}.patch", u.html),
        "base_commit": base_json,
        "merge_base_commit": mb_json,
        "status": c.status(),
        "ahead_by": c.ahead_by,
        "behind_by": c.behind_by,
        "total_commits": c.ahead_by,
        "commits": commits,
        "files": files_json(u, &c.head.sha, &c.files),
    }))
}

// ---- contributors ----

#[derive(Deserialize, Default)]
struct ContribQuery {
    anon: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn contributors(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ContribQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let (head, refs) = s.git.refs(ctx.repo.id).await.api()?;
    if !refs.contains_key(&head) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let out = s.git.git.run(&path, &["shortlog", "-sne", &head]).await.api()?;
    let text = String::from_utf8_lossy(&out);
    let mut rows: Vec<(u64, String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((n, ident)) = line.split_once('\t') else { continue };
        let sig = read::Signature::parse(ident);
        rows.push((n.trim().parse().unwrap_or(0), sig.name, sig.email));
    }
    let emails: Vec<String> = rows.iter().map(|r| r.2.clone()).collect();
    let users = gitapi::users_by_email(&s, &emails).await?;
    let anon = matches!(q.anon.as_deref(), Some("1" | "true"));
    let mut by_user: HashMap<i64, (crate::db::User, u64)> = HashMap::new();
    let mut anon_rows = Vec::new();
    for (n, name, email) in rows {
        match users.get(&email.to_lowercase()) {
            Some(user) => by_user.entry(user.id).or_insert_with(|| (user.clone(), 0)).1 += n,
            None if anon => anon_rows.push(json!({"email": email, "name": name, "type": "Anonymous", "contributions": n})),
            None => {}
        }
    }
    let mut list: Vec<(u64, Value)> = by_user
        .into_values()
        .map(|(user, n)| {
            let mut v = crate::json::user::simple(&s.api_url(), s.base_url(), &user);
            if let Some(o) = v.as_object_mut() {
                // `contributor` has non-nullable name/email; GitHub omits them for users.
                o.remove("name");
                o.remove("email");
            }
            v["contributions"] = json!(n);
            (n, v)
        })
        .collect();
    list.extend(anon_rows.into_iter().map(|v| (v["contributions"].as_u64().unwrap_or(0), v)));
    list.sort_by(|a, b| b.0.cmp(&a.0));
    let pages = PageParams { page: q.page, per_page: q.per_page };
    let total = list.len() as i64;
    let items: Vec<Value> = list.into_iter().skip(pages.offset() as usize).take(pages.per_page() as usize).map(|x| x.1).collect();
    Ok(Paged::new(&uri, &s, pages, Some(total), items).into_response())
}
