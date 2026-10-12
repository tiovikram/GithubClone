//! Branches (list/get/rename), merges (`POST /merges`, `POST /merge-upstream`), tags and
//! archive downloads (`/tarball`, `/zipball` redirecting to `/{owner}/{repo}/archive/...`,
//! plus `/{owner}/{repo}/raw/{ref}/{path}` used by contents `download_url`).
use std::collections::HashSet;
use std::process::Stdio;

use axum::body::Body;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::io::ReaderStream;

use super::git_data::user_signature;
use super::gitapi::{self, reject, GitResult};
use super::{PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::objects;
use crate::git::read;
use crate::json::git as gj;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/branches", get(list_branches))
        .route("/repos/{owner}/{repo}/merges", post(merge_h))
        .route("/repos/{owner}/{repo}/merge-upstream", post(merge_upstream_h))
        .route("/repos/{owner}/{repo}/tags", get(list_tags))
        .route("/repos/{owner}/{repo}/tarball", get(tarball_default))
        .route("/repos/{owner}/{repo}/tarball/{*ref}", get(tarball))
        .route("/repos/{owner}/{repo}/zipball", get(zipball_default))
        .route("/repos/{owner}/{repo}/zipball/{*ref}", get(zipball))
}

/// Per-branch routes, mounted behind `ref_dispatch` (branch names may contain `/`).
pub fn branch_ref_routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/branches/{branch}", get(get_branch))
        .route("/repos/{owner}/{repo}/branches/{branch}/rename", post(rename_branch_h))
}

/// Routes outside /api/v3 (merged at the top level in `lib.rs`): archive streaming and raw files.
pub fn web_routes() -> Router<AppState> {
    Router::new()
        .route("/{owner}/{repo}/archive/{*file}", get(archive_download))
        .route("/{owner}/{repo}/raw/{*rest}", get(raw_file))
}

/// Names of protected branches (`crate::protection`).
pub async fn protected_branches(s: &AppState, ctx: &RepoCtx) -> HashSet<String> {
    crate::protection::protected_branches(&s.db, ctx.repo.id).await
}

/// `branch-with-protection` JSON for `name` at `sha`.
pub async fn branch_json(s: &AppState, ctx: &RepoCtx, name: &str, sha: &str) -> ApiResult<Value> {
    let u = gitapi::urls(s, ctx);
    let path = gitapi::sync(s, ctx).await?;
    let c = read::read_commit(&s.git.git, &path, sha).await.api()?.ok_or_else(ApiError::not_found)?;
    let commit = gitapi::commits_json(s, &u, &[c]).await?.pop().unwrap_or(Value::Null);
    let rules = crate::protection::rules(&s.db, ctx.repo.id, name).await?;
    let protected = rules.is_some();
    let contexts = rules.as_ref().map(|r| r.required_contexts()).unwrap_or_default();
    let has_checks = rules.as_ref().is_some_and(|r| r.cfg.get("required_status_checks").is_some_and(|v| v.is_object()));
    let level = match &rules {
        Some(r) if has_checks && r.enforce_admins() => "everyone",
        Some(_) if has_checks => "non_admins",
        _ => "off",
    };
    let checks: Vec<Value> = contexts.iter().map(|c| json!({"context": c, "app_id": null})).collect();
    let n = gj::enc_component(name);
    Ok(json!({
        "name": name,
        "commit": commit,
        "_links": { "self": format!("{}/branches/{n}", u.repo_api), "html": format!("{}/tree/{}", u.html, gj::enc_path(name)) },
        "protected": protected,
        "protection": {
            "enabled": protected,
            "required_status_checks": { "enforcement_level": level, "contexts": contexts, "checks": checks },
        },
        "protection_url": format!("{}/branches/{n}/protection", u.repo_api),
    }))
}

#[derive(Deserialize, Default)]
struct BranchQuery {
    protected: Option<bool>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_branches(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<BranchQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    let protected = protected_branches(&s, &ctx).await;
    let urls = gitapi::urls(&s, &ctx);
    let all: Vec<(String, String)> = refs
        .iter()
        .filter_map(|(n, v)| n.strip_prefix("refs/heads/").map(|b| (b.to_string(), v.clone())))
        .filter(|(b, _)| q.protected.is_none_or(|p| protected.contains(b) == p))
        .collect();
    let pages = PageParams { page: q.page, per_page: q.per_page };
    let items: Vec<Value> = all
        .iter()
        .skip(pages.offset() as usize)
        .take(pages.per_page() as usize)
        .map(|(b, v)| gj::short_branch(&urls, b, v, protected.contains(b)))
        .collect();
    Ok(Paged::new(&uri, &s, pages, Some(all.len() as i64), items).into_response())
}

fn branch_not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "Branch not found")
}

async fn get_branch(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, branch)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    let sha = refs.get(&format!("refs/heads/{branch}")).ok_or_else(branch_not_found)?;
    Ok(Json(branch_json(&s, &ctx, &branch, sha).await?))
}

/// Renames a branch (updating HEAD/default branch and open pull requests). Returns the new
/// branch's head sha.
pub async fn rename_branch(s: &AppState, ctx: &RepoCtx, actor: &User, old: &str, new: &str) -> ApiResult<String> {
    let new_ref = format!("refs/heads/{new}");
    let ok = matches!(s.git.git.try_run(&std::env::temp_dir(), &["check-ref-format", &new_ref]).await, Ok((true, _, _)));
    if !ok || new.starts_with('-') {
        return Err(ApiError::validation("Branch", "new_name", "invalid"));
    }
    let git = s.git.git.clone();
    let old_ref = format!("refs/heads/{old}");
    let nr = new_ref.clone();
    let (sha, updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let refs_text = git.run_str(&path, &["for-each-ref", "--format=%(objectname) %(refname)", "refs/heads/"]).await?;
            let refs = crate::git::parse_refs(&refs_text);
            let Some(sha) = refs.get(&old_ref).cloned() else { return Err(reject(branch_not_found())) };
            if old_ref == nr {
                return Ok(sha);
            }
            if refs.contains_key(&nr) || refs.keys().any(|k| k.starts_with(&format!("{nr}/")) || nr.starts_with(&format!("{k}/"))) {
                let mut e = ApiError::validation("Branch", "new_name", "already_exists");
                e.message = format!("Branch {} already exists", nr.trim_start_matches("refs/heads/"));
                return Err(reject(e));
            }
            let script = format!("start\ndelete {old_ref} {sha}\ncreate {nr} {sha}\ncommit\n");
            git.run_input(&path, &["update-ref", "--stdin"], script.as_bytes()).await?;
            let head = git.run_str(&path, &["symbolic-ref", "HEAD"]).await.unwrap_or_default();
            if head == old_ref {
                git.run(&path, &["symbolic-ref", "HEAD", &nr]).await?;
            }
            Ok(sha)
        })
        .await
        .api()?;
    if ctx.repo.default_branch == old {
        sqlx::query("UPDATE repositories SET default_branch = $2, updated_at = now() WHERE id = $1")
            .bind(ctx.repo.id)
            .bind(new)
            .execute(&s.db)
            .await?;
    }
    // Retarget pull requests like GitHub does.
    sqlx::query("UPDATE pull_requests SET base_ref = $3 WHERE repo_id = $1 AND base_ref = $2")
        .bind(ctx.repo.id)
        .bind(old)
        .bind(new)
        .execute(&s.db)
        .await?;
    sqlx::query("UPDATE pull_requests SET head_ref = $3 WHERE head_repo_id = $1 AND head_ref = $2")
        .bind(ctx.repo.id)
        .bind(old)
        .bind(new)
        .execute(&s.db)
        .await?;
    let ctx2 = RepoCtx { repo: db::repo_by_id(&s.db, ctx.repo.id).await?, ..ctx.clone() };
    gitapi::after_write(s, &ctx2, actor, &updates).await;
    Ok(sha)
}

#[derive(Deserialize)]
struct RenameBody {
    new_name: Option<String>,
}

async fn rename_branch_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, branch)): Path<(String, String, String)>,
    Json(b): Json<RenameBody>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let new = b.new_name.filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("Branch", "new_name", "missing_field"))?;
    if ctx.repo.default_branch == branch {
        ctx.require(crate::db::Perm::Admin)?;
    }
    let sha = rename_branch(&s, &ctx, &u, &branch, &new).await?;
    let ctx = gitapi::load(&s, Some(&u), &owner, &repo).await?;
    let v = branch_json(&s, &ctx, &new, &sha).await?;
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

// ---- merges ----

/// Outcome of `merge_branch`.
#[derive(Debug, Clone)]
pub enum MergeOutcome {
    /// Head was already contained in base (204).
    AlreadyMerged,
    /// A merge commit was created on base.
    Merged(read::CommitInfo),
}

/// Merges `head` (branch or sha) into branch `base` with a merge commit (POST /merges).
pub async fn merge_branch(s: &AppState, ctx: &RepoCtx, actor: &User, base: &str, head: &str, message: Option<String>) -> ApiResult<MergeOutcome> {
    crate::protection::check_direct_write(s, ctx, base, true).await?;
    let git = s.git.git.clone();
    let base_ref = format!("refs/heads/{base}");
    let (base_s, head_s) = (base.to_string(), head.to_string());
    let me = user_signature(actor);
    let (out, updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let Some(b) = read::resolve_commit(&git, &path, &base_ref).await? else {
                return Err(reject(ApiError::new(StatusCode::NOT_FOUND, "Base does not exist")));
            };
            let Some(h) = read::resolve_commit(&git, &path, &head_s).await? else {
                return Err(reject(ApiError::new(StatusCode::NOT_FOUND, "Head does not exist")));
            };
            if read::is_ancestor(&git, &path, &h, &b).await? {
                return Ok(MergeOutcome::AlreadyMerged);
            }
            let tree = match merge_trees(&git, &path, &b, &h).await? {
                Some(t) => t,
                None => return Err(reject(ApiError::conflict("Merge conflict"))),
            };
            let msg = message.unwrap_or_else(|| format!("Merge {head_s} into {base_s}"));
            let sha = objects::write_commit(&git, &path, &tree, &[b.clone(), h], &me, &me, &msg, None).await?;
            crate::git::plumbing::update_ref(&git, &path, &base_ref, &sha, Some(&b)).await?;
            let c = read::read_commit(&git, &path, &sha).await?.ok_or_else(|| anyhow::anyhow!("merge commit missing"))?;
            Ok(MergeOutcome::Merged(c))
        })
        .await
        .api()?;
    gitapi::after_write(s, ctx, actor, &updates).await;
    Ok(out)
}

/// `git merge-tree --write-tree`; None on conflicts.
pub async fn merge_trees(git: &crate::git::GitCmd, path: &std::path::Path, base: &str, head: &str) -> anyhow::Result<Option<String>> {
    let (ok, out, err) = git.try_run(path, &["merge-tree", "--write-tree", "--no-messages", base, head]).await?;
    let first = String::from_utf8_lossy(&out).lines().next().unwrap_or("").trim().to_string();
    if ok && read::is_sha(&first) {
        return Ok(Some(first));
    }
    if read::is_sha(&first) {
        return Ok(None); // exit 1 = conflicts
    }
    anyhow::bail!("merge-tree failed: {}", String::from_utf8_lossy(&err))
}

#[derive(Deserialize)]
struct MergeBody {
    base: Option<String>,
    head: Option<String>,
    commit_message: Option<String>,
}

async fn merge_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<MergeBody>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let base = b.base.ok_or_else(|| ApiError::validation("Merge", "base", "missing_field"))?;
    let head = b.head.ok_or_else(|| ApiError::validation("Merge", "head", "missing_field"))?;
    match merge_branch(&s, &ctx, &u, &base, &head, b.commit_message.filter(|m| !m.is_empty())).await? {
        MergeOutcome::AlreadyMerged => Ok(super::no_content()),
        MergeOutcome::Merged(c) => {
            let v = gitapi::commits_json(&s, &gitapi::urls(&s, &ctx), &[c]).await?.pop().unwrap_or(Value::Null);
            Ok(super::created(v))
        }
    }
}

#[derive(Deserialize)]
struct UpstreamBody {
    branch: Option<String>,
}

/// Syncs `branch` of a fork with its upstream (fast-forward, or a merge commit when diverged).
/// Returns (message, merge_type, base_branch).
pub async fn merge_upstream(s: &AppState, ctx: &RepoCtx, actor: &User, branch: &str) -> ApiResult<(String, String, String)> {
    let parent_id = ctx.repo.fork_parent_id.ok_or_else(|| ApiError::unprocessable("This repository is not a fork."))?;
    let parent = db::load_repo_by_id(&s.db, Some(actor), parent_id).await?;
    let (_, prefs) = s.git.refs(parent.repo.id).await.api()?;
    let upstream_ref = format!("refs/heads/{branch}");
    let up_sha = prefs.get(&upstream_ref).cloned().ok_or_else(|| ApiError::unprocessable(format!("Branch {branch} does not exist upstream")))?;
    let parent_path = s.git.sync(parent.repo.id).await.api()?;
    let base_branch = format!("{}:{branch}", parent.owner.login);
    let git = s.git.git.clone();
    let me = user_signature(actor);
    let bb = base_branch.clone();
    let (res, updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let src = parent_path.to_string_lossy().to_string();
            git.run(&path, &["fetch", "-q", "--no-tags", "--no-write-fetch-head", &src, &upstream_ref]).await?;
            let Some(mine) = read::resolve_commit(&git, &path, &upstream_ref).await? else {
                return Err(reject(ApiError::unprocessable(format!("Branch {} does not exist", upstream_ref.trim_start_matches("refs/heads/")))));
            };
            if mine == up_sha || read::is_ancestor(&git, &path, &up_sha, &mine).await? {
                return Ok((format!("This branch is not behind the upstream {bb}."), "none".to_string()));
            }
            if read::is_ancestor(&git, &path, &mine, &up_sha).await? {
                crate::git::plumbing::update_ref(&git, &path, &upstream_ref, &up_sha, Some(&mine)).await?;
                return Ok((format!("Successfully fetched and fast-forwarded from upstream {bb}."), "fast-forward".to_string()));
            }
            let Some(tree) = merge_trees(&git, &path, &mine, &up_sha).await? else {
                return Err(reject(ApiError::conflict("There are merge conflicts")));
            };
            let msg = format!("Merge branch '{}' of {bb}", upstream_ref.trim_start_matches("refs/heads/"));
            let sha = objects::write_commit(&git, &path, &tree, &[mine.clone(), up_sha.clone()], &me, &me, &msg, None).await?;
            crate::git::plumbing::update_ref(&git, &path, &upstream_ref, &sha, Some(&mine)).await?;
            Ok((format!("Successfully fetched and merged from upstream {bb}."), "merge".to_string()))
        })
        .await
        .api()?;
    gitapi::after_write(s, ctx, actor, &updates).await;
    Ok((res.0, res.1, base_branch))
}

async fn merge_upstream_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<UpstreamBody>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let branch = b.branch.ok_or_else(|| ApiError::validation("Repository", "branch", "missing_field"))?;
    let (message, merge_type, base_branch) = merge_upstream(&s, &ctx, &u, &branch).await?;
    Ok(Json(json!({ "message": message, "merge_type": merge_type, "base_branch": base_branch })))
}

// ---- tags ----

#[derive(Deserialize, Default)]
struct TagQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_tags(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<TagQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    let pages = PageParams { page: q.page, per_page: q.per_page };
    if !refs.keys().any(|k| k.starts_with("refs/tags/")) {
        return Ok(Paged::new(&uri, &s, pages, Some(0), vec![]).into_response());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let out = s
        .git
        .git
        .run_str(&path, &["for-each-ref", "--sort=-v:refname", "--format=%(refname:strip=2)%00%(objectname)%00%(*objectname)%00%(objecttype)", "refs/tags/"])
        .await
        .api()?;
    let urls = gitapi::urls(&s, &ctx);
    let all: Vec<(String, String)> = out
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\0').collect();
            if p.len() < 4 {
                return None;
            }
            let target = if !p[2].is_empty() { p[2] } else { p[1] };
            Some((p[0].to_string(), target.to_string()))
        })
        .collect();
    let items: Vec<Value> = all
        .iter()
        .skip(pages.offset() as usize)
        .take(pages.per_page() as usize)
        .map(|(n, sha)| gj::tag(&urls, n, sha))
        .collect();
    Ok(Paged::new(&uri, &s, pages, Some(all.len() as i64), items).into_response())
}

// ---- archives ----

async fn archive_redirect(s: &AppState, viewer: Option<&User>, owner: &str, repo: &str, rev: Option<String>, ext: &str) -> ApiResult<Response> {
    let ctx = gitapi::load(s, viewer, owner, repo).await?;
    if gitapi::is_empty(s, &ctx).await? {
        return Err(ApiError::not_found());
    }
    let rev = rev.filter(|r| !r.is_empty()).unwrap_or_else(|| ctx.repo.default_branch.clone());
    let path = gitapi::sync(s, &ctx).await?;
    read::resolve_commit(&s.git.git, &path, &rev).await.api()?.ok_or_else(ApiError::not_found)?;
    let mut loc = format!("{}/{}/archive/{}.{ext}", s.base_url(), ctx.full_name(), gj::enc_path(&rev));
    if ctx.repo.private {
        // The client does not resend Authorization to the redirect target: sign the URL.
        let token = crate::signed_url::archive_token(&s.db, ctx.repo.id, &rev).await.map_err(ApiError::internal)?;
        loc.push_str(&format!("?token={token}"));
    }
    let mut r = StatusCode::FOUND.into_response();
    r.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&loc).map_err(ApiError::internal)?);
    Ok(r)
}

async fn tarball_default(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    archive_redirect(&s, u.as_ref(), &o, &r, None, "tar.gz").await
}
async fn tarball(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r, rev)): Path<(String, String, String)>) -> ApiResult<Response> {
    archive_redirect(&s, u.as_ref(), &o, &r, Some(rev), "tar.gz").await
}
async fn zipball_default(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    archive_redirect(&s, u.as_ref(), &o, &r, None, "zip").await
}
async fn zipball(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r, rev)): Path<(String, String, String)>) -> ApiResult<Response> {
    archive_redirect(&s, u.as_ref(), &o, &r, Some(rev), "zip").await
}

fn plain(status: StatusCode, msg: &str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], format!("{msg}\n")).into_response()
}

#[derive(Deserialize, Default)]
struct ArchiveQuery {
    token: Option<String>,
}

/// The repository for an archive request authorized by a signed `?token=` (issued by the
/// tarball/zipball redirect) instead of credentials.
async fn token_ctx(s: &AppState, owner: &str, repo: &str, rev: &str, token: Option<&str>) -> Option<RepoCtx> {
    let token = token?;
    let owner = db::user_by_login(&s.db, owner).await.ok()?;
    let repo: db::Repo = sqlx::query_as("SELECT * FROM repositories WHERE owner_id = $1 AND lower(name) = lower($2)")
        .bind(owner.id)
        .bind(repo.strip_suffix(".git").unwrap_or(repo))
        .fetch_optional(&s.db)
        .await
        .ok()??;
    if !crate::signed_url::verify_archive_token(&s.db, repo.id, rev, token).await {
        return None;
    }
    Some(RepoCtx { repo, owner, perm: db::Perm::Read })
}

/// Streams `git archive` for `/{owner}/{repo}/archive/{ref}.{tar.gz|tgz|tar|zip}`.
async fn archive_download(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, file)): Path<(String, String, String)>,
    Query(q): Query<ArchiveQuery>,
) -> Response {
    let (rev, format, ext) = if let Some(r) = file.strip_suffix(".tar.gz") {
        (r, "tar.gz", "tar.gz")
    } else if let Some(r) = file.strip_suffix(".tgz") {
        (r, "tar.gz", "tar.gz")
    } else if let Some(r) = file.strip_suffix(".zip") {
        (r, "zip", "zip")
    } else if let Some(r) = file.strip_suffix(".tar") {
        (r, "tar", "tar")
    } else {
        return plain(StatusCode::NOT_FOUND, "Not Found");
    };
    let ctx = match gitapi::load(&s, u.as_ref(), &owner, &repo).await {
        Ok(c) => c,
        Err(_) => match token_ctx(&s, &owner, &repo, rev, q.token.as_deref()).await {
            Some(c) => c,
            None => return plain(StatusCode::NOT_FOUND, "Not Found"),
        },
    };
    let Ok(path) = gitapi::sync(&s, &ctx).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable");
    };
    let git = &s.git.git;
    let Ok(Some(sha)) = read::resolve_commit(git, &path, rev).await else {
        return plain(StatusCode::NOT_FOUND, "Not Found");
    };
    let prefix = format!("{}-{}-{}/", ctx.owner.login, ctx.repo.name, &sha[..7]);
    let mut cmd = git.command(&path);
    cmd.args(["archive", &format!("--format={format}"), &format!("--prefix={prefix}"), &sha])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "git error");
    };
    let stdout = child.stdout.take().unwrap();
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    let ctype = match format {
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        _ => "application/x-gzip",
    };
    let fname = format!("{}-{}.{ext}", ctx.repo.name, rev.replace('/', "-"));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ctype)
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename={fname}"))
        .header("x-forge-archive-sha", sha)
        .body(Body::from_stream(ReaderStream::new(stdout)))
        .unwrap()
}

/// `/{owner}/{repo}/raw/{ref}/{path}`: raw file bytes (refs may contain slashes).
async fn raw_file(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, rest)): Path<(String, String, String)>,
) -> Response {
    let Ok(ctx) = gitapi::load(&s, u.as_ref(), &owner, &repo).await else {
        return plain(StatusCode::NOT_FOUND, "404: Not Found");
    };
    let Ok((_, refs)) = s.git.refs(ctx.repo.id).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable");
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let mut found = None;
    for i in 1..parts.len() {
        let r = parts[..i].join("/");
        let known = refs.contains_key(&format!("refs/heads/{r}")) || refs.contains_key(&format!("refs/tags/{r}")) || read::is_sha(&r);
        if known {
            found = Some((r, parts[i..].join("/")));
            break;
        }
    }
    let Some((rev, file)) = found else { return plain(StatusCode::NOT_FOUND, "404: Not Found") };
    let Some(file) = gitapi::clean_path(&file).filter(|f| !f.is_empty()) else {
        return plain(StatusCode::NOT_FOUND, "404: Not Found");
    };
    let Ok(path) = gitapi::sync(&s, &ctx).await else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable");
    };
    let git = &s.git.git;
    let Ok(Some(sha)) = read::resolve_commit(git, &path, &rev).await else {
        return plain(StatusCode::NOT_FOUND, "404: Not Found");
    };
    match read::cat_object(git, &path, &format!("{sha}:{file}")).await {
        Ok(Some(o)) if o.kind == "blob" => {
            let mime = if o.data.iter().take(8000).any(|b| *b == 0) { "application/octet-stream" } else { "text/plain; charset=utf-8" };
            ([(header::CONTENT_TYPE, mime)], o.data).into_response()
        }
        _ => plain(StatusCode::NOT_FOUND, "404: Not Found"),
    }
}
