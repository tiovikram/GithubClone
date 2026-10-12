//! Contents API: GET/PUT/DELETE `/repos/{owner}/{repo}/contents/{path}` and README lookup.
use std::path::Path as FsPath;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::git_data::{decode_base64, signature_from, user_signature, PersonInput};
use super::gitapi::{self, reject, GitResult};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::cmd::{GitCmd, Identity};
use crate::git::plumbing::{self, Change};
use crate::git::read::{self, CommitInfo, TreeEntry};
use crate::json::git::{self as gj, RepoUrls};
use crate::state::AppState;

/// GitHub returns file content inline only up to 1 MB.
pub const MAX_INLINE_CONTENT: u64 = 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/contents", get(get_root))
        .route("/repos/{owner}/{repo}/contents/", get(get_root))
        .route("/repos/{owner}/{repo}/contents/{*path}", get(get_content).put(put_content).delete(delete_content))
        .route("/repos/{owner}/{repo}/readme", get(readme_root))
        .route("/repos/{owner}/{repo}/readme/{*dir}", get(readme_dir))
}

#[derive(Deserialize, Default)]
struct RefQuery {
    #[serde(rename = "ref")]
    rev: Option<String>,
}

/// Resolves `?ref=` (default branch when absent) to (ref name as shown in URLs, commit sha).
async fn resolve_ref(s: &AppState, ctx: &RepoCtx, path: &FsPath, rev: Option<&str>) -> ApiResult<(String, String)> {
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    if refs.is_empty() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "This repository is empty."));
    }
    let name = rev.filter(|r| !r.is_empty()).map(str::to_string).unwrap_or_else(|| ctx.repo.default_branch.clone());
    match read::resolve_commit(&s.git.git, path, &name).await.api()? {
        Some(sha) => Ok((name, sha)),
        None if rev.is_some() => Err(ApiError::new(StatusCode::NOT_FOUND, format!("No commit found for the ref {name}"))),
        None => Err(ApiError::not_found()),
    }
}

/// The tree entry at `path` in `commit` (None if absent). `path` must be non-empty.
pub async fn entry_at(git: &GitCmd, repo: &FsPath, commit: &str, path: &str) -> anyhow::Result<Option<TreeEntry>> {
    let entries = read::ls_tree(git, repo, commit, Some(path), false).await?;
    Ok(entries.into_iter().find(|e| e.path == path))
}

/// Entries of the directory `dir` ("" = root) with full paths.
async fn list_dir(git: &GitCmd, repo: &FsPath, tree_sha: &str, dir: &str) -> anyhow::Result<Vec<TreeEntry>> {
    let mut entries = read::ls_tree(git, repo, tree_sha, None, false).await?;
    if !dir.is_empty() {
        for e in &mut entries {
            e.path = format!("{dir}/{}", e.path);
        }
    }
    Ok(entries)
}

fn file_json(u: &RepoUrls, rev: &str, e: &TreeEntry, data: &[u8]) -> Value {
    let mut v = gj::content_item(u, rev, e, false);
    if (data.len() as u64) > MAX_INLINE_CONTENT {
        v["content"] = json!("");
        v["encoding"] = json!("none");
    } else {
        v["content"] = json!(gj::base64_wrapped(data));
        v["encoding"] = json!("base64");
    }
    v
}

/// Normalizes `target` relative to the directory of `link_path`; None if it escapes the repo.
fn resolve_symlink(link_path: &str, target: &str) -> Option<String> {
    if target.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = link_path.split('/').collect();
    parts.pop();
    for comp in target.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `submodule.<name>.url` for the submodule at `path` from `.gitmodules` text.
fn submodule_url(gitmodules: &str, path: &str) -> Option<String> {
    let mut cur_path: Option<String> = None;
    let mut cur_url: Option<String> = None;
    let mut found = None;
    let mut flush = |p: &mut Option<String>, u: &mut Option<String>| {
        if p.as_deref() == Some(path) && found.is_none() {
            found = u.clone();
        }
        *p = None;
        *u = None;
    };
    for line in gitmodules.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            flush(&mut cur_path, &mut cur_url);
        } else if let Some((k, v)) = l.split_once('=') {
            match k.trim() {
                "path" => cur_path = Some(v.trim().to_string()),
                "url" => cur_url = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    flush(&mut cur_path, &mut cur_url);
    found
}

/// What GET /contents found.
enum Found {
    Dir { entry: Option<TreeEntry>, tree: String, items: Vec<TreeEntry> },
    File { entry: TreeEntry, data: Vec<u8> },
    Symlink { entry: TreeEntry, target: String },
    Submodule { entry: TreeEntry, url: String },
}

async fn find(s: &AppState, path: &FsPath, commit: &str, p: &str) -> ApiResult<Found> {
    let git = &s.git.git;
    if p.is_empty() {
        let c = read::read_commit(git, path, commit).await.api()?.ok_or_else(ApiError::not_found)?;
        let items = list_dir(git, path, &c.tree, "").await.api()?;
        return Ok(Found::Dir { entry: None, tree: c.tree, items });
    }
    let e = entry_at(git, path, commit, p).await.api()?.ok_or_else(ApiError::not_found)?;
    match (e.kind.as_str(), e.mode.as_str()) {
        ("tree", _) => {
            let items = list_dir(git, path, &e.sha, p).await.api()?;
            Ok(Found::Dir { tree: e.sha.clone(), entry: Some(e), items })
        }
        ("commit", _) => {
            let gm = read::cat_object(git, path, &format!("{commit}:.gitmodules")).await.api()?;
            let url = gm
                .and_then(|o| submodule_url(&String::from_utf8_lossy(&o.data), p))
                .unwrap_or_else(|| format!("{}/{p}", s.base_url()));
            Ok(Found::Submodule { entry: e, url })
        }
        (_, "120000") => {
            let obj = read::cat_object(git, path, &e.sha).await.api()?.ok_or_else(ApiError::not_found)?;
            let target = String::from_utf8_lossy(&obj.data).to_string();
            // A link to a regular file in the repo serves that file (like GitHub).
            if let Some(tp) = resolve_symlink(p, &target) {
                if let Some(te) = entry_at(git, path, commit, &tp).await.api()? {
                    if te.kind == "blob" && te.mode != "120000" {
                        let o = read::cat_object(git, path, &te.sha).await.api()?.ok_or_else(ApiError::not_found)?;
                        return Ok(Found::File { entry: te, data: o.data });
                    }
                }
            }
            Ok(Found::Symlink { entry: e, target })
        }
        _ => {
            let obj = read::cat_object(git, path, &e.sha).await.api()?.ok_or_else(ApiError::not_found)?;
            Ok(Found::File { entry: e, data: obj.data })
        }
    }
}

async fn get_root(
    st: State<AppState>,
    u: MaybeUser,
    headers: HeaderMap,
    Path((owner, repo)): Path<(String, String)>,
    q: Query<RefQuery>,
) -> ApiResult<Response> {
    contents(st, u, headers, owner, repo, String::new(), q.0).await
}

async fn get_content(
    st: State<AppState>,
    u: MaybeUser,
    headers: HeaderMap,
    Path((owner, repo, path)): Path<(String, String, String)>,
    q: Query<RefQuery>,
) -> ApiResult<Response> {
    contents(st, u, headers, owner, repo, path, q.0).await
}

async fn contents(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    headers: HeaderMap,
    owner: String,
    repo: String,
    p: String,
    q: RefQuery,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let p = gitapi::clean_path(&p).ok_or_else(ApiError::not_found)?;
    let path = gitapi::sync(&s, &ctx).await?;
    let (rev, commit) = resolve_ref(&s, &ctx, &path, q.rev.as_deref()).await?;
    let urls = gitapi::urls(&s, &ctx);
    let media = gitapi::media(&headers);
    let found = find(&s, &path, &commit, &p).await?;
    Ok(match found {
        Found::File { entry, data } => match media {
            Some("raw") => gitapi::typed("application/vnd.github.raw", data),
            Some("html") => gitapi::typed("text/html; charset=utf-8", render_html(&entry.path, &data).into_bytes()),
            _ => Json(file_json(&urls, &rev, &entry, &data)).into_response(),
        },
        Found::Dir { entry, tree, items } => {
            let list: Vec<Value> = items.iter().map(|e| gj::content_item(&urls, &rev, e, true)).collect();
            if media == Some("object") {
                let e = entry.unwrap_or(TreeEntry { mode: "040000".into(), kind: "tree".into(), sha: tree, size: None, path: String::new() });
                let mut v = gj::content_item(&urls, &rev, &e, false);
                v["entries"] = Value::Array(list);
                Json(v).into_response()
            } else {
                Json(Value::Array(list)).into_response()
            }
        }
        Found::Symlink { entry, target } => {
            if media == Some("raw") {
                return Ok(gitapi::typed("application/vnd.github.raw", target.into_bytes()));
            }
            let mut v = gj::content_item(&urls, &rev, &entry, false);
            v["type"] = json!("symlink");
            v["target"] = json!(target);
            Json(v).into_response()
        }
        Found::Submodule { entry, url } => {
            let mut v = gj::content_item(&urls, &rev, &entry, false);
            v["type"] = json!("submodule");
            v["submodule_git_url"] = json!(url);
            Json(v).into_response()
        }
    })
}

/// Minimal HTML rendering for `Accept: application/vnd.github.html` (escaped preformatted text).
fn render_html(path: &str, data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data);
    let esc = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let id = if path.to_ascii_lowercase().contains("readme") { "readme" } else { "file" };
    format!("<div id=\"{id}\" class=\"md\" data-path=\"{}\"><article class=\"markdown-body entry-content container-lg\" itemprop=\"text\"><pre>{esc}</pre></article></div>", path.replace('"', "&quot;"))
}

// ---- README ----

/// Picks the README among directory entries (case-insensitive `README` / `README.*`, markdown first).
pub fn pick_readme(items: &[TreeEntry]) -> Option<&TreeEntry> {
    let score = |e: &TreeEntry| -> Option<u8> {
        if e.kind != "blob" {
            return None;
        }
        let n = e.name().to_ascii_lowercase();
        if n == "readme.md" || n == "readme.markdown" {
            Some(0)
        } else if n.starts_with("readme.") {
            Some(1)
        } else if n == "readme" {
            Some(2)
        } else {
            None
        }
    };
    items.iter().filter_map(|e| score(e).map(|s| (s, e))).min_by_key(|(s, e)| (*s, e.name().to_string())).map(|(_, e)| e)
}

async fn readme_root(
    st: State<AppState>,
    u: MaybeUser,
    headers: HeaderMap,
    Path((owner, repo)): Path<(String, String)>,
    q: Query<RefQuery>,
) -> ApiResult<Response> {
    readme(st, u, headers, owner, repo, String::new(), q.0).await
}

async fn readme_dir(
    st: State<AppState>,
    u: MaybeUser,
    headers: HeaderMap,
    Path((owner, repo, dir)): Path<(String, String, String)>,
    q: Query<RefQuery>,
) -> ApiResult<Response> {
    readme(st, u, headers, owner, repo, dir, q.0).await
}

async fn readme(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    headers: HeaderMap,
    owner: String,
    repo: String,
    dir: String,
    q: RefQuery,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let dir = gitapi::clean_path(&dir).ok_or_else(ApiError::not_found)?;
    let path = gitapi::sync(&s, &ctx).await?;
    let (rev, commit) = resolve_ref(&s, &ctx, &path, q.rev.as_deref()).await?;
    let Found::Dir { items, .. } = find(&s, &path, &commit, &dir).await? else {
        return Err(ApiError::not_found());
    };
    let e = pick_readme(&items).cloned().ok_or_else(ApiError::not_found)?;
    let obj = read::cat_object(&s.git.git, &path, &e.sha).await.api()?.ok_or_else(ApiError::not_found)?;
    Ok(match gitapi::media(&headers) {
        Some("raw") => gitapi::typed("application/vnd.github.raw", obj.data),
        Some("html") => gitapi::typed("text/html; charset=utf-8", render_html(&e.path, &obj.data).into_bytes()),
        _ => Json(file_json(&gitapi::urls(&s, &ctx), &rev, &e, &obj.data)).into_response(),
    })
}

// ---- create / update / delete ----

/// Input for `put_file` / `delete_file`.
#[derive(Debug, Clone)]
pub struct FileWrite {
    pub path: String,
    pub message: String,
    /// New content (None = delete).
    pub content: Option<Vec<u8>>,
    /// Expected current blob sha (required to update or delete).
    pub sha: Option<String>,
    pub branch: Option<String>,
    pub author: Identity,
    pub committer: Identity,
}

/// Result of a contents write.
#[derive(Debug, Clone)]
pub struct FileCommit {
    pub created: bool,
    /// The file after the write (None after delete).
    pub entry: Option<TreeEntry>,
    pub branch: String,
    pub commit: CommitInfo,
}

fn mismatch(path: &str, sha: &str) -> ApiError {
    ApiError::conflict(format!("{path} does not match {sha}"))
}

/// Creates, updates or deletes one file on a branch with a new commit (contents API semantics).
pub async fn write_file(s: &AppState, ctx: &RepoCtx, actor: &User, w: FileWrite) -> ApiResult<FileCommit> {
    let git = s.git.git.clone();
    let branch = w.branch.clone().filter(|b| !b.is_empty()).unwrap_or_else(|| ctx.repo.default_branch.clone());
    crate::protection::check_direct_write(s, ctx, &branch, false).await?;
    let refname = format!("refs/heads/{branch}");
    let b2 = branch.clone();
    let (fc, updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let (ok, out, _) = git.try_run(&path, &["rev-parse", "--verify", "--quiet", "--end-of-options", &refname]).await?;
            let head = String::from_utf8_lossy(&out).trim().to_string();
            let head = (ok && read::is_sha(&head)).then_some(head);
            if head.is_none() {
                let (ok, out, _) = git.try_run(&path, &["for-each-ref", "--count=1", "--format=%(refname)", "refs/heads/"]).await?;
                let any_branch = ok && !out.is_empty();
                if any_branch {
                    return Err(reject(ApiError::new(StatusCode::NOT_FOUND, format!("Branch {b2} not found"))));
                }
            }
            let existing = match &head {
                Some(h) => entry_at(&git, &path, h, &w.path).await?,
                None => None,
            };
            if let Some(e) = &existing {
                if e.kind == "tree" {
                    return Err(reject(ApiError::unprocessable(format!("{} is a directory", w.path))));
                }
            }
            let mut mode = "100644".to_string();
            match (&existing, &w.sha, &w.content) {
                (Some(_), None, Some(_)) => {
                    return Err(reject(ApiError::unprocessable("Invalid request.\n\n\"sha\" wasn't supplied.")));
                }
                (Some(e), Some(sha), _) if !e.sha.eq_ignore_ascii_case(sha) => return Err(reject(mismatch(&w.path, sha))),
                (Some(e), _, _) => {
                    if e.mode == "100755" {
                        mode = e.mode.clone();
                    }
                }
                (None, _, None) => return Err(reject(ApiError::not_found())),
                (None, _, Some(_)) => {}
            }
            let created = existing.is_none();
            let change = match w.content {
                Some(content) => Change::Put { path: w.path.clone(), content, mode },
                None => Change::Delete { path: w.path.clone() },
            };
            let sha = plumbing::commit_changes(&git, &path, head.as_deref(), vec![change], &w.message, &w.author, &w.committer)
                .await
                .map_err(|e| {
                    if format!("{e}").contains("update-index") {
                        reject(ApiError::unprocessable(format!("{} could not be written: a parent path is a file", w.path)))
                    } else {
                        e
                    }
                })?;
            plumbing::update_ref(&git, &path, &refname, &sha, head.as_deref()).await?;
            let entry = entry_at(&git, &path, &sha, &w.path).await?;
            let commit = read::read_commit(&git, &path, &sha).await?.ok_or_else(|| anyhow::anyhow!("commit missing"))?;
            Ok(FileCommit { created, entry, branch: b2, commit })
        })
        .await
        .api()?;
    gitapi::after_write(s, ctx, actor, &updates).await;
    Ok(fc)
}

fn file_commit_json(u: &RepoUrls, fc: &FileCommit) -> Value {
    let content = fc
        .entry
        .as_ref()
        .map(|e| gj::content_item(u, &fc.branch, e, false))
        .unwrap_or(Value::Null);
    json!({ "content": content, "commit": gj::git_commit(u, &fc.commit) })
}

#[derive(Deserialize)]
struct PutBody {
    message: Option<String>,
    content: Option<String>,
    sha: Option<String>,
    branch: Option<String>,
    committer: Option<PersonInput>,
    author: Option<PersonInput>,
}

fn identities(actor: &User, author: Option<&PersonInput>, committer: Option<&PersonInput>) -> ApiResult<(Identity, Identity)> {
    let me = user_signature(actor);
    let c = signature_from(committer.or(author), &me)?;
    let a = signature_from(author.or(committer), &me)?;
    let id = |s: crate::git::read::Signature| Identity { name: s.name, email: s.email, when: Some(s.time) };
    Ok((id(a), id(c)))
}

fn valid_write_path(p: &str) -> ApiResult<String> {
    let clean = gitapi::clean_path(p).filter(|c| !c.is_empty() && c.split('/').all(|x| !x.eq_ignore_ascii_case(".git")));
    clean.ok_or_else(|| ApiError::validation("Content", "path", "invalid"))
}

async fn put_content(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, p)): Path<(String, String, String)>,
    Json(b): Json<PutBody>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let path = valid_write_path(&p)?;
    let message = b.message.ok_or_else(|| ApiError::validation("Content", "message", "missing_field"))?;
    let content = b.content.ok_or_else(|| ApiError::validation("Content", "content", "missing_field"))?;
    let data = decode_base64(&content).ok_or_else(|| ApiError::unprocessable("content is not valid Base64"))?;
    let (author, committer) = identities(&u, b.author.as_ref(), b.committer.as_ref())?;
    let fc = write_file(
        &s,
        &ctx,
        &u,
        FileWrite { path, message, content: Some(data), sha: b.sha, branch: b.branch, author, committer },
    )
    .await?;
    let body = file_commit_json(&gitapi::urls(&s, &ctx), &fc);
    let status = if fc.created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(body)).into_response())
}

#[derive(Deserialize)]
struct DeleteBody {
    message: Option<String>,
    sha: Option<String>,
    branch: Option<String>,
    committer: Option<PersonInput>,
    author: Option<PersonInput>,
}

async fn delete_content(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, p)): Path<(String, String, String)>,
    Json(b): Json<DeleteBody>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let path = valid_write_path(&p)?;
    let message = b.message.ok_or_else(|| ApiError::validation("Content", "message", "missing_field"))?;
    let sha = b.sha.ok_or_else(|| ApiError::validation("Content", "sha", "missing_field"))?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(ApiError::not_found());
    }
    let (author, committer) = identities(&u, b.author.as_ref(), b.committer.as_ref())?;
    let fc = write_file(
        &s,
        &ctx,
        &u,
        FileWrite { path, message, content: None, sha: Some(sha), branch: b.branch, author, committer },
    )
    .await?;
    Ok(Json(file_commit_json(&gitapi::urls(&s, &ctx), &fc)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlinks_resolve() {
        assert_eq!(resolve_symlink("a/link", "b.txt").as_deref(), Some("a/b.txt"));
        assert_eq!(resolve_symlink("a/link", "../b.txt").as_deref(), Some("b.txt"));
        assert_eq!(resolve_symlink("link", "../b.txt"), None);
        assert_eq!(resolve_symlink("link", "/etc/passwd"), None);
    }

    #[test]
    fn gitmodules() {
        let gm = "[submodule \"lib\"]\n\tpath = vendor/lib\n\turl = https://example.com/lib.git\n[submodule \"x\"]\n\tpath = x\n\turl = ../x.git\n";
        assert_eq!(submodule_url(gm, "vendor/lib").as_deref(), Some("https://example.com/lib.git"));
        assert_eq!(submodule_url(gm, "x").as_deref(), Some("../x.git"));
        assert_eq!(submodule_url(gm, "nope"), None);
    }
}
