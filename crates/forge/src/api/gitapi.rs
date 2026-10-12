//! Shared helpers for the repository-content / git-object API modules (contents, git_data,
//! commits, branches, forks, statuses): repo loading with permissions, error plumbing out of
//! `GitStore::write`, Accept media types, commit-author resolution.
use std::collections::HashMap;
use std::path::PathBuf;

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::git::objects::InvalidInput;
use crate::git::read::CommitInfo;
use crate::git::RefUpdates;
use crate::json::git::RepoUrls;
use crate::state::AppState;

/// An `ApiError` raised inside a `GitStore::write` closure; the write is rolled back and the
/// error is returned to the client unchanged (see `api_err`).
#[derive(Debug)]
pub struct Rejected(pub ApiError);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.0.status, self.0.message)
    }
}
impl std::error::Error for Rejected {}

/// Wraps an `ApiError` for returning from a git write closure.
pub fn reject(e: ApiError) -> anyhow::Error {
    anyhow::Error::new(Rejected(e))
}

/// Maps an error from a git operation back to an API error (`Rejected` -> its error,
/// `InvalidInput` -> 422, anything else -> 500).
pub fn api_err(e: anyhow::Error) -> ApiError {
    let e = match e.downcast::<Rejected>() {
        Ok(r) => return r.0,
        Err(e) => e,
    };
    match e.downcast::<InvalidInput>() {
        Ok(i) => ApiError::unprocessable(i.0),
        Err(e) => ApiError::internal(format!("{e:#}")),
    }
}

pub trait GitResult<T> {
    fn api(self) -> ApiResult<T>;
}

impl<T> GitResult<T> for anyhow::Result<T> {
    fn api(self) -> ApiResult<T> {
        self.map_err(api_err)
    }
}

/// Loads a repository visible to `viewer` (404 otherwise).
pub async fn load(s: &AppState, viewer: Option<&User>, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    db::load_repo(&s.db, viewer, owner, repo).await
}

/// Loads a repository and requires push access; archived repositories are read-only.
pub async fn load_for_write(s: &AppState, viewer: Option<&User>, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    let ctx = load(s, viewer, owner, repo).await?;
    if viewer.is_none() {
        return Err(if ctx.perm >= Perm::Read { ApiError::unauthorized() } else { ApiError::not_found() });
    }
    ctx.require(Perm::Write)?;
    if ctx.repo.archived {
        return Err(ApiError::forbidden("Repository was archived so is read-only."));
    }
    Ok(ctx)
}

pub fn urls(s: &AppState, ctx: &RepoCtx) -> RepoUrls {
    RepoUrls::new(&s.api_url(), s.base_url(), ctx)
}

/// Syncs the local cache for reading.
pub async fn sync(s: &AppState, ctx: &RepoCtx) -> ApiResult<PathBuf> {
    s.git.sync(ctx.repo.id).await.api()
}

/// True when the repository has no refs at all.
pub async fn is_empty(s: &AppState, ctx: &RepoCtx) -> ApiResult<bool> {
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    Ok(refs.is_empty())
}

pub fn empty_repo_error() -> ApiError {
    ApiError::conflict("Git Repository is empty.")
}

/// Post-write bookkeeping for API writes that moved refs: pushed_at, default branch for a
/// first branch, and `hooks::after_refs_changed` (via `events::on_push`).
pub async fn after_write(s: &AppState, ctx: &RepoCtx, actor: &User, updates: &RefUpdates) {
    if let Err(e) = crate::events::on_push(s, ctx, actor, updates).await {
        tracing::warn!(repo = ctx.repo.id, error = %e, "post-write processing failed");
    }
}

/// The requested `application/vnd.github.<x>` media type suffix (`raw`, `html`, `object`,
/// `diff`, `patch`, `sha`), if any.
pub fn media(headers: &HeaderMap) -> Option<&'static str> {
    let accept = headers.get(header::ACCEPT)?.to_str().ok()?.to_ascii_lowercase();
    for m in ["raw", "html", "object", "diff", "patch", "sha"] {
        if accept.contains(&format!("application/vnd.github.{m}"))
            || accept.contains(&format!("application/vnd.github.v3.{m}"))
            || accept.contains(&format!("application/vnd.github+{m}"))
        {
            return Some(m);
        }
    }
    None
}

/// A non-JSON response body with the given content type.
pub fn typed(content_type: &str, body: Vec<u8>) -> Response {
    let mut r = (StatusCode::OK, body).into_response();
    if let Ok(v) = content_type.parse() {
        r.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    // Raw/HTML/diff bodies are repository content: no sniffing, no script execution.
    crate::security::harden_user_content(&mut r);
    r
}

/// Forge users matching commit emails (exact email, or forge/GitHub noreply addresses
/// `{id}+{login}@users.noreply.*`). Keys are lowercased emails.
pub async fn users_by_email(s: &AppState, emails: &[String]) -> ApiResult<HashMap<String, User>> {
    let mut lower: Vec<String> = emails.iter().map(|e| e.to_lowercase()).collect();
    lower.sort();
    lower.dedup();
    if lower.is_empty() {
        return Ok(HashMap::new());
    }
    let mut ids: Vec<i64> = Vec::new();
    for e in &lower {
        if let Some((local, domain)) = e.split_once('@') {
            if domain.starts_with("users.noreply.") {
                if let Some((id, _)) = local.split_once('+') {
                    if let Ok(id) = id.parse() {
                        ids.push(id);
                    }
                }
            }
        }
    }
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE lower(email) = ANY($1) OR id = ANY($2)")
        .bind(&lower)
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let mut out = HashMap::new();
    for e in &lower {
        let hit = users.iter().find(|u| u.email.as_deref().map(str::to_lowercase).as_deref() == Some(e.as_str())).or_else(|| {
            let (local, domain) = e.split_once('@')?;
            if !domain.starts_with("users.noreply.") {
                return None;
            }
            let (id, login) = local.split_once('+')?;
            let id: i64 = id.parse().ok()?;
            users.iter().find(|u| u.id == id && u.login.eq_ignore_ascii_case(login))
        });
        if let Some(u) = hit {
            out.insert(e.clone(), u.clone());
        }
    }
    Ok(out)
}

/// Number of commit comments per sha (one query for the whole batch).
pub async fn commit_comment_counts(s: &AppState, repo_id: i64, shas: Vec<String>) -> ApiResult<HashMap<String, i64>> {
    if shas.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT commit_id, count(*) FROM commit_comments WHERE repo_id = $1 AND commit_id = ANY($2) GROUP BY commit_id",
    )
    .bind(repo_id)
    .bind(&shas)
    .fetch_all(&s.db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Renders `commit`-schema JSON for many commits, resolving author/committer users in one query.
pub async fn commits_json(s: &AppState, u: &RepoUrls, commits: &[CommitInfo]) -> ApiResult<Vec<serde_json::Value>> {
    let emails: Vec<String> = commits.iter().flat_map(|c| [c.author.email.clone(), c.committer.email.clone()]).collect();
    let users = users_by_email(s, &emails).await?;
    let counts = commit_comment_counts(s, u.repo_id, commits.iter().map(|c| c.sha.clone()).collect()).await?;
    Ok(commits
        .iter()
        .map(|c| {
            let a = users.get(&c.author.email.to_lowercase());
            let m = users.get(&c.committer.email.to_lowercase());
            crate::json::git::commit(u, c, a, m, counts.get(&c.sha).copied().unwrap_or(0))
        })
        .collect())
}

/// Splits `owner/repo` path params and URL-decodes nothing else; validates a user-supplied
/// repository-relative path. Returns the normalized path ("" = root) or None if unsafe.
pub fn clean_path(p: &str) -> Option<String> {
    let trimmed = p.trim_matches('/');
    if trimmed.is_empty() {
        return Some(String::new());
    }
    let mut out = Vec::new();
    for comp in trimmed.split('/') {
        match comp {
            "" | "." => continue,
            ".." => return None,
            c if c.chars().any(|ch| ch.is_control()) => return None,
            c => out.push(c),
        }
    }
    Some(out.join("/"))
}
