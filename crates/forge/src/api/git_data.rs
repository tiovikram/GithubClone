//! Git data API: refs, blobs, trees, commits, annotated tags
//! (`/repos/{owner}/{repo}/git/...`).
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use super::created;
use super::gitapi::{self, reject, GitResult};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::objects::{self, TreeChange};
use crate::git::read::{self, Signature};
use crate::git::plumbing;
use crate::json::git as gj;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/git/ref/{*ref}", get(get_ref))
        .route("/repos/{owner}/{repo}/git/matching-refs/{*ref}", get(matching_refs))
        .route("/repos/{owner}/{repo}/git/refs", get(list_refs).post(create_ref_h))
        .route("/repos/{owner}/{repo}/git/refs/{*ref}", get(get_refs_legacy).patch(update_ref_h).delete(delete_ref_h))
        .route("/repos/{owner}/{repo}/git/blobs", post(create_blob_h))
        .route("/repos/{owner}/{repo}/git/blobs/{sha}", get(get_blob))
        .route("/repos/{owner}/{repo}/git/trees", post(create_tree_h))
        .route("/repos/{owner}/{repo}/git/trees/{*sha}", get(get_tree))
        .route("/repos/{owner}/{repo}/git/commits", post(create_commit_h))
        .route("/repos/{owner}/{repo}/git/commits/{sha}", get(get_commit))
        .route("/repos/{owner}/{repo}/git/tags", post(create_tag_h))
        .route("/repos/{owner}/{repo}/git/tags/{sha}", get(get_tag))
}

/// A ref and the type of the object it points at.
#[derive(Debug, Clone)]
pub struct GitRef {
    pub name: String,
    pub sha: String,
    pub kind: String,
}

/// All refs of a repository (optionally under `prefix`), with object types.
pub async fn list_refs_with_types(s: &AppState, ctx: &RepoCtx, prefix: Option<&str>) -> ApiResult<Vec<GitRef>> {
    let (_, refs) = s.git.refs(ctx.repo.id).await.api()?;
    let mut items: Vec<(String, String)> = refs
        .into_iter()
        .filter(|(n, _)| prefix.is_none_or(|p| n.starts_with(p)))
        .collect();
    items.sort();
    let need_types: Vec<String> = items.iter().filter(|(n, _)| !n.starts_with("refs/heads/")).map(|(_, s)| s.clone()).collect();
    let mut types = std::collections::HashMap::new();
    if !need_types.is_empty() {
        let path = gitapi::sync(s, ctx).await?;
        let info = read::object_info(&s.git.git, &path, &need_types).await.api()?;
        for (sha, i) in need_types.iter().zip(info) {
            if let Some((_, k, _)) = i {
                types.insert(sha.clone(), k);
            }
        }
    }
    Ok(items
        .into_iter()
        .map(|(name, sha)| {
            let kind = types.get(&sha).cloned().unwrap_or_else(|| "commit".into());
            GitRef { name, sha, kind }
        })
        .collect())
}

fn full_ref(r: &str) -> String {
    let r = r.trim_matches('/');
    if r.starts_with("refs/") { r.to_string() } else { format!("refs/{r}") }
}

fn ref_json(s: &AppState, ctx: &RepoCtx, r: &GitRef) -> Value {
    gj::git_ref(&gitapi::urls(s, ctx), &r.name, &r.sha, &r.kind)
}

async fn get_ref(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let name = full_ref(&r);
    let refs = list_refs_with_types(&s, &ctx, Some(&name)).await?;
    let hit = refs.iter().find(|x| x.name == name).ok_or_else(ApiError::not_found)?;
    Ok(Json(ref_json(&s, &ctx, hit)))
}

/// Refs whose name starts with `refs/{ref}` (GitHub matches by string prefix).
async fn matching_refs(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let prefix = format!("refs/{}", r.trim_start_matches('/'));
    let refs = list_refs_with_types(&s, &ctx, Some(&prefix)).await?;
    Ok(Json(Value::Array(refs.iter().map(|r| ref_json(&s, &ctx, r)).collect())))
}

async fn list_refs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let refs = list_refs_with_types(&s, &ctx, None).await?;
    Ok(Json(Value::Array(refs.iter().map(|r| ref_json(&s, &ctx, r)).collect())))
}

/// Legacy `GET /git/refs/{ref}`: the ref itself on exact match, else refs under the prefix.
async fn get_refs_legacy(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let name = full_ref(&r);
    let refs = list_refs_with_types(&s, &ctx, Some(&name)).await?;
    if let Some(hit) = refs.iter().find(|x| x.name == name) {
        return Ok(Json(ref_json(&s, &ctx, hit)));
    }
    let dir = format!("{name}/");
    let under: Vec<Value> = refs.iter().filter(|x| x.name.starts_with(&dir)).map(|r| ref_json(&s, &ctx, r)).collect();
    if under.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(Json(Value::Array(under)))
}

async fn valid_ref_name(s: &AppState, name: &str) -> bool {
    if !name.starts_with("refs/") || name.matches('/').count() < 2 || name.starts_with('-') || is_managed_ref(name) {
        return false;
    }
    let tmp = std::env::temp_dir();
    matches!(s.git.git.try_run(&tmp, &["check-ref-format", name]).await, Ok((true, _, _)))
}

/// `refs/pull/*` belongs to pull requests and is maintained by forge only (pushes are rejected
/// too, see `security::RECEIVE_PACK_CONFIG`).
fn is_managed_ref(name: &str) -> bool {
    name.starts_with("refs/pull/")
}

fn managed_ref_error() -> ApiError {
    ApiError::unprocessable("Reference cannot be updated")
}

async fn current_ref(git: &crate::git::GitCmd, path: &std::path::Path, name: &str) -> anyhow::Result<Option<String>> {
    let (ok, out, _) = git.try_run(path, &["rev-parse", "--verify", "--quiet", "--end-of-options", name]).await?;
    let sha = String::from_utf8_lossy(&out).trim().to_string();
    Ok((ok && read::is_sha(&sha)).then_some(sha))
}

/// Creates `name` (fully qualified, `refs/...`) pointing at `sha`.
pub async fn create_ref(s: &AppState, ctx: &RepoCtx, actor: &User, name: &str, sha: &str) -> ApiResult<GitRef> {
    if !valid_ref_name(s, name).await {
        return Err(ApiError::unprocessable("Reference name is not valid"));
    }
    if !read::is_sha(sha) {
        return Err(ApiError::unprocessable("Object does not exist"));
    }
    let git = s.git.git.clone();
    let (name_c, sha_c) = (name.to_string(), sha.to_lowercase());
    let (st, rctx, ractor) = (s.clone(), ctx.clone(), crate::rules::Actor::new(ctx, actor));
    let (kind, updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let info = read::object_info(&git, &path, &[sha_c.clone()]).await?.pop().flatten();
            let Some((_, kind, _)) = info else { return Err(reject(ApiError::unprocessable("Object does not exist"))) };
            if current_ref(&git, &path, &name_c).await?.is_some() {
                return Err(reject(ApiError::unprocessable("Reference already exists")));
            }
            let upd = crate::rules::RefUpdate::new(&name_c, None, Some(&sha_c));
            crate::rules::enforce_api(&st, &rctx, &ractor, &path, &[upd], crate::rules::Via::RefApi, None).await.map_err(reject)?;
            // A ref can't be created inside or above an existing ref hierarchy.
            if let Err(e) = plumbing::update_ref(&git, &path, &name_c, &sha_c, None).await {
                tracing::debug!(error = %e, "create ref failed");
                return Err(reject(ApiError::unprocessable("Reference already exists")));
            }
            Ok(kind)
        })
        .await
        .api()?;
    gitapi::after_write(s, ctx, actor, &updates).await;
    Ok(GitRef { name: name.to_string(), sha: sha.to_lowercase(), kind })
}

/// Moves `name` to `sha`; non-fast-forward updates require `force`.
pub async fn update_ref(s: &AppState, ctx: &RepoCtx, actor: &User, name: &str, sha: &str, force: bool) -> ApiResult<GitRef> {
    if !read::is_sha(sha) {
        return Err(ApiError::unprocessable("Object does not exist"));
    }
    crate::protection::check_ref_update(s, ctx, name, &sha.to_lowercase()).await?;
    let git = s.git.git.clone();
    let (name_c, sha_c) = (name.to_string(), sha.to_lowercase());
    let (st, rctx, ractor) = (s.clone(), ctx.clone(), crate::rules::Actor::new(ctx, actor));
    let (kind, updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let Some(old) = current_ref(&git, &path, &name_c).await? else {
                return Err(reject(ApiError::unprocessable("Reference does not exist")));
            };
            let info = read::object_info(&git, &path, &[sha_c.clone()]).await?.pop().flatten();
            let Some((_, kind, _)) = info else { return Err(reject(ApiError::unprocessable("Object does not exist"))) };
            if !force && old != sha_c && !read::is_ancestor(&git, &path, &old, &sha_c).await? {
                return Err(reject(ApiError::unprocessable("Update is not a fast forward")));
            }
            let upd = crate::rules::RefUpdate::new(&name_c, Some(&old), Some(&sha_c));
            crate::rules::enforce_api(&st, &rctx, &ractor, &path, &[upd], crate::rules::Via::RefApi, None).await.map_err(reject)?;
            plumbing::update_ref(&git, &path, &name_c, &sha_c, Some(&old)).await?;
            Ok(kind)
        })
        .await
        .api()?;
    gitapi::after_write(s, ctx, actor, &updates).await;
    Ok(GitRef { name: name.to_string(), sha: sha.to_lowercase(), kind })
}

pub async fn delete_ref(s: &AppState, ctx: &RepoCtx, actor: &User, name: &str) -> ApiResult<()> {
    crate::protection::check_ref_delete(s, ctx, name).await?;
    let git = s.git.git.clone();
    let name_c = name.to_string();
    let (st, rctx, ractor) = (s.clone(), ctx.clone(), crate::rules::Actor::new(ctx, actor));
    let ((), updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let Some(old) = current_ref(&git, &path, &name_c).await? else {
                return Err(reject(ApiError::unprocessable("Reference does not exist")));
            };
            let upd = crate::rules::RefUpdate::new(&name_c, Some(&old), None);
            crate::rules::enforce_api(&st, &rctx, &ractor, &path, &[upd], crate::rules::Via::RefApi, None).await.map_err(reject)?;
            git.run(&path, &["update-ref", "-d", &name_c, &old]).await?;
            Ok(())
        })
        .await
        .api()?;
    gitapi::after_write(s, ctx, actor, &updates).await;
    Ok(())
}

#[derive(Deserialize)]
struct CreateRef {
    #[serde(rename = "ref")]
    name: Option<String>,
    sha: Option<String>,
}

async fn create_ref_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<CreateRef>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let name = b.name.ok_or_else(|| ApiError::validation("Reference", "ref", "missing_field"))?;
    let sha = b.sha.ok_or_else(|| ApiError::validation("Reference", "sha", "missing_field"))?;
    let r = create_ref(&s, &ctx, &u, &name, &sha).await?;
    Ok(created(ref_json(&s, &ctx, &r)))
}

#[derive(Deserialize)]
struct UpdateRef {
    sha: Option<String>,
    #[serde(default)]
    force: bool,
}

async fn update_ref_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
    Json(b): Json<UpdateRef>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let sha = b.sha.ok_or_else(|| ApiError::validation("Reference", "sha", "missing_field"))?;
    if is_managed_ref(&full_ref(&r)) {
        return Err(managed_ref_error());
    }
    let r = update_ref(&s, &ctx, &u, &full_ref(&r), &sha, b.force).await?;
    Ok(Json(ref_json(&s, &ctx, &r)))
}

async fn delete_ref_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, r)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    if is_managed_ref(&full_ref(&r)) {
        return Err(managed_ref_error());
    }
    delete_ref(&s, &ctx, &u, &full_ref(&r)).await?;
    Ok(super::no_content())
}

// ---- blobs ----

#[derive(Deserialize)]
struct CreateBlob {
    content: Option<String>,
    encoding: Option<String>,
}

/// Decodes base64 the lenient way GitHub does (whitespace/newlines ignored).
pub fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&clean)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(clean.trim_end_matches('=')))
        .ok()
}

/// Stores a blob; returns its sha.
pub async fn create_blob(s: &AppState, ctx: &RepoCtx, data: Vec<u8>) -> ApiResult<String> {
    let git = s.git.git.clone();
    let (sha, _) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let sha = objects::write_blob(&git, &path, &data).await?;
            objects::pack_loose(&git, &path).await?;
            Ok(sha)
        })
        .await
        .api()?;
    Ok(sha)
}

async fn create_blob_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<CreateBlob>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let content = b.content.ok_or_else(|| ApiError::validation("Blob", "content", "missing_field"))?;
    let data = match b.encoding.as_deref().unwrap_or("utf-8").to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" => content.into_bytes(),
        "base64" => decode_base64(&content).ok_or_else(|| ApiError::validation("Blob", "content", "invalid"))?,
        _ => return Err(ApiError::validation("Blob", "encoding", "invalid")),
    };
    let sha = create_blob(&s, &ctx, data).await?;
    let url = format!("{}/git/blobs/{sha}", gitapi::urls(&s, &ctx).repo_api);
    Ok(created(json!({ "url": url, "sha": sha })))
}

async fn get_blob(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    headers: HeaderMap,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if !read::is_sha(&sha) {
        return Err(ApiError::not_found());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let obj = read::cat_object(&s.git.git, &path, &sha).await.api()?.filter(|o| o.kind == "blob").ok_or_else(ApiError::not_found)?;
    if gitapi::media(&headers) == Some("raw") {
        return Ok(gitapi::typed("application/vnd.github.raw", obj.data));
    }
    Ok(Json(gj::blob(&gitapi::urls(&s, &ctx), &obj.sha, &obj.data)).into_response())
}

// ---- trees ----

#[derive(Deserialize, Default)]
struct TreeQuery {
    recursive: Option<String>,
}

/// GitHub caps tree listings at 100,000 entries (`truncated: true` beyond).
const MAX_TREE_ENTRIES: usize = 100_000;

async fn get_tree(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
    Query(q): Query<TreeQuery>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let git = &s.git.git;
    let (tree_sha, _) = read::resolve_object(git, &path, &format!("{sha}^{{tree}}")).await.api()?.ok_or_else(ApiError::not_found)?;
    // Any value (GitHub: "0", "1", "true" ...) enables recursion except an empty one.
    let recursive = q.recursive.as_deref().is_some_and(|v| !v.is_empty());
    let mut entries = read::ls_tree(git, &path, &tree_sha, None, recursive).await.api()?;
    let truncated = entries.len() > MAX_TREE_ENTRIES;
    entries.truncate(MAX_TREE_ENTRIES);
    Ok(Json(gj::git_tree(&gitapi::urls(&s, &ctx), &tree_sha, &entries, truncated)))
}

#[derive(Deserialize)]
struct CreateTree {
    tree: Option<Vec<Value>>,
    base_tree: Option<String>,
}

fn parse_tree_items(items: Vec<Value>) -> ApiResult<Vec<TreeChange>> {
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let o = it.as_object().ok_or_else(|| ApiError::validation("Tree", "tree", "invalid"))?;
        let get = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let path = get("path").ok_or_else(|| ApiError::validation("Tree", "tree.path", "missing_field"))?;
        let mode = get("mode").unwrap_or_else(|| "100644".into());
        let kind = get("type").unwrap_or_else(|| read::kind_for_mode(&mode).into());
        let sha = match o.get("sha") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        let content = get("content").map(String::into_bytes);
        let explicit_delete = matches!(o.get("sha"), Some(Value::Null));
        if sha.is_none() && content.is_none() && !explicit_delete {
            let mut e = ApiError::unprocessable("Must supply either tree.sha or tree.content");
            e.errors = None;
            return Err(e);
        }
        let mode = if mode.len() == 5 { format!("0{mode}") } else { mode };
        out.push(TreeChange { path: path.trim_matches('/').to_string(), mode, kind, sha, content });
    }
    Ok(out)
}

/// Creates a tree (optionally on top of `base_tree`); returns its sha and top-level entries.
pub async fn create_tree(
    s: &AppState,
    ctx: &RepoCtx,
    base_tree: Option<String>,
    changes: Vec<TreeChange>,
) -> ApiResult<(String, Vec<read::TreeEntry>)> {
    let git = s.git.git.clone();
    let (res, _) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let base = match base_tree {
                Some(b) => {
                    let r = read::resolve_object(&git, &path, &format!("{b}^{{tree}}")).await?;
                    let Some((t, _)) = r else {
                        return Err(reject(ApiError::unprocessable(format!("base_tree {b} is not a valid tree oid"))));
                    };
                    Some(t)
                }
                None => None,
            };
            let sha = objects::build_tree(&git, &path, base.as_deref(), changes).await?;
            objects::pack_loose(&git, &path).await?;
            let entries = read::ls_tree(&git, &path, &sha, None, false).await?;
            Ok((sha, entries))
        })
        .await
        .api()?;
    Ok(res)
}

async fn create_tree_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<CreateTree>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let items = b.tree.ok_or_else(|| ApiError::validation("Tree", "tree", "missing_field"))?;
    let changes = parse_tree_items(items)?;
    let (sha, entries) = create_tree(&s, &ctx, b.base_tree, changes).await?;
    Ok(created(gj::git_tree(&gitapi::urls(&s, &ctx), &sha, &entries, false)))
}

// ---- commits ----

async fn get_commit(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    if !read::is_sha(&sha) {
        return Err(ApiError::not_found());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let c = read::read_commit(&s.git.git, &path, &sha).await.api()?.ok_or_else(ApiError::not_found)?;
    Ok(Json(gj::git_commit(&gitapi::urls(&s, &ctx), &c)))
}

/// `{name, email, date}` as sent by clients.
#[derive(Deserialize, Clone, Default)]
pub struct PersonInput {
    pub name: Option<String>,
    pub email: Option<String>,
    pub date: Option<String>,
}

/// Builds a signature from client input, defaulting to `fallback` (and now).
pub fn signature_from(input: Option<&PersonInput>, fallback: &Signature) -> ApiResult<Signature> {
    let Some(p) = input else { return Ok(fallback.clone()) };
    let (time, tz) = match p.date.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => read::parse_iso_date(d).ok_or_else(|| ApiError::validation("Commit", "date", "invalid"))?,
        None => (fallback.time, fallback.tz.clone()),
    };
    Ok(Signature {
        name: p.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| fallback.name.clone()),
        email: p.email.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| fallback.email.clone()),
        time,
        tz,
    })
}

/// The acting user's signature at the current time.
pub fn user_signature(u: &User) -> Signature {
    let id = super::repos::identity_for(u);
    Signature { name: id.name, email: id.email, time: chrono::Utc::now().timestamp(), tz: "+0000".into() }
}

#[derive(Deserialize)]
struct CreateCommit {
    message: Option<String>,
    tree: Option<String>,
    #[serde(default)]
    parents: Vec<String>,
    author: Option<PersonInput>,
    committer: Option<PersonInput>,
    signature: Option<String>,
}

/// Creates a commit object (no ref moves). Returns the parsed commit.
pub async fn create_commit(
    s: &AppState,
    ctx: &RepoCtx,
    tree: &str,
    parents: Vec<String>,
    author: Signature,
    committer: Signature,
    message: String,
    signature: Option<String>,
) -> ApiResult<read::CommitInfo> {
    let git = s.git.git.clone();
    let tree = tree.to_string();
    let (c, _) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let t = read::object_info(&git, &path, &[tree.clone()]).await?.pop().flatten();
            if !read::is_sha(&tree) || !matches!(t, Some((_, ref k, _)) if k == "tree") {
                return Err(reject(ApiError::unprocessable("Tree SHA does not exist")));
            }
            if !parents.is_empty() {
                let info = read::object_info(&git, &path, &parents).await?;
                if parents.iter().zip(info).any(|(p, i)| !read::is_sha(p) || !matches!(i, Some((_, ref k, _)) if k == "commit")) {
                    return Err(reject(ApiError::unprocessable("Parent SHA does not exist or is not a commit object")));
                }
            }
            let parents: Vec<String> = parents.iter().map(|p| p.to_lowercase()).collect();
            let sha = objects::write_commit(&git, &path, &tree, &parents, &author, &committer, &message, signature.as_deref()).await?;
            objects::pack_loose(&git, &path).await?;
            read::read_commit(&git, &path, &sha).await?.ok_or_else(|| anyhow::anyhow!("created commit missing"))
        })
        .await
        .api()?;
    Ok(c)
}

async fn create_commit_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<CreateCommit>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let message = b.message.ok_or_else(|| ApiError::validation("Commit", "message", "missing_field"))?;
    let tree = b.tree.ok_or_else(|| ApiError::validation("Commit", "tree", "missing_field"))?;
    let me = user_signature(&u);
    let author = signature_from(b.author.as_ref(), &me)?;
    let committer = signature_from(b.committer.as_ref().or(b.author.as_ref()), &me)?;
    let c = create_commit(&s, &ctx, &tree, b.parents, author, committer, message, b.signature).await?;
    Ok(created(gj::git_commit(&gitapi::urls(&s, &ctx), &c)))
}

// ---- tags ----

async fn get_tag(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    if gitapi::is_empty(&s, &ctx).await? {
        return Err(gitapi::empty_repo_error());
    }
    if !read::is_sha(&sha) {
        return Err(ApiError::not_found());
    }
    let path = gitapi::sync(&s, &ctx).await?;
    let obj = read::cat_object(&s.git.git, &path, &sha).await.api()?.filter(|o| o.kind == "tag").ok_or_else(ApiError::not_found)?;
    Ok(Json(gj::git_tag(&gitapi::urls(&s, &ctx), &read::parse_tag(&obj.sha, &obj.data))))
}

#[derive(Deserialize)]
struct CreateTag {
    tag: Option<String>,
    message: Option<String>,
    object: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    tagger: Option<PersonInput>,
}

/// Creates an annotated tag object (no ref; clients create `refs/tags/x` separately).
pub async fn create_tag(
    s: &AppState,
    ctx: &RepoCtx,
    tag: String,
    message: String,
    object: String,
    kind: String,
    tagger: Signature,
) -> ApiResult<read::TagInfo> {
    let git = s.git.git.clone();
    let (t, _) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let info = read::object_info(&git, &path, &[object.clone()]).await?.pop().flatten();
            match info {
                Some((_, k, _)) if k == kind && read::is_sha(&object) => {}
                _ => return Err(reject(ApiError::unprocessable("Object does not exist"))),
            }
            let sha = objects::write_tag(&git, &path, &object.to_lowercase(), &kind, &tag, &tagger, &message).await?;
            objects::pack_loose(&git, &path).await?;
            let obj = read::cat_object(&git, &path, &sha).await?.ok_or_else(|| anyhow::anyhow!("created tag missing"))?;
            Ok(read::parse_tag(&obj.sha, &obj.data))
        })
        .await
        .api()?;
    Ok(t)
}

async fn create_tag_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<CreateTag>,
) -> ApiResult<Response> {
    let ctx = gitapi::load_for_write(&s, Some(&u), &owner, &repo).await?;
    let tag = b.tag.ok_or_else(|| ApiError::validation("Tag", "tag", "missing_field"))?;
    let message = b.message.ok_or_else(|| ApiError::validation("Tag", "message", "missing_field"))?;
    let object = b.object.ok_or_else(|| ApiError::validation("Tag", "object", "missing_field"))?;
    let kind = b.kind.unwrap_or_else(|| "commit".into());
    if !matches!(kind.as_str(), "commit" | "tree" | "blob" | "tag") {
        return Err(ApiError::validation("Tag", "type", "invalid"));
    }
    if tag.is_empty() || tag.chars().any(|c| c.is_control() || c == ' ') {
        return Err(ApiError::validation("Tag", "tag", "invalid"));
    }
    let tagger = signature_from(b.tagger.as_ref(), &user_signature(&u))?;
    let t = create_tag(&s, &ctx, tag, message, object, kind, tagger).await?;
    Ok(created(gj::git_tag(&gitapi::urls(&s, &ctx), &t)))
}
