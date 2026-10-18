//! GitHub Pages: site configuration (`/repos/{o}/{r}/pages`), builds and deployments, and the
//! static site server at `{base}/pages/{owner}/{repo}/...` (GHES's path-based Pages URL when
//! subdomain isolation is off).
//!
//! - `build_type: legacy`: every push to the source branch (and `POST /pages/builds`) builds the
//!   site: the files under the source path of the branch tip are copied to object storage under
//!   `pages/{repo_id}/b{build_id}/`. No Jekyll processing is done (files are served as-is).
//! - `build_type: workflow`: `POST /pages/deployments` publishes an artifact (tar, tar.gz, or a
//!   zip containing `artifact.tar`, as produced by `actions/upload-pages-artifact`). The
//!   artifact must be a release asset or an Actions artifact of the same forge (no outbound
//!   fetches); its files go to `pages/{repo_id}/d{deployment_id}/`.
//!
//! The site is switched atomically (`pages_sites.live_prefix`) and the previous generation is
//! deleted. Pages are served with a sandboxing CSP (opaque origin) because they share the
//! forge's origin.
use std::io::Read;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use object_store::path::Path as ObjPath;
#[allow(unused_imports)]
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/pages";
    Router::new()
        .route(r, get(get_site).post(create_site).put(update_site).delete(delete_site))
        .route(&format!("{r}/builds"), get(list_builds).post(request_build))
        .route(&format!("{r}/builds/latest"), get(latest_build))
        .route(&format!("{r}/builds/{{id}}"), get(get_build))
        .route(&format!("{r}/deployments"), post(create_deployment))
        .route(&format!("{r}/deployments/{{id}}"), get(get_deployment))
        .route(&format!("{r}/deployments/{{id}}/cancel"), post(cancel_deployment))
}

/// The static site server (merged at the top level in `lib.rs`).
pub fn web_routes() -> Router<AppState> {
    Router::new()
        .route("/pages/{owner}/{repo}", get(serve_root))
        .route("/pages/{owner}/{repo}/", get(serve_index))
        .route("/pages/{owner}/{repo}/{*path}", get(serve_path))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SiteRow {
    pub repo_id: i64,
    pub build_type: String,
    pub source_branch: String,
    pub source_path: String,
    pub cname: Option<String>,
    pub https_enforced: bool,
    pub public: bool,
    pub status: Option<String>,
    pub live_prefix: Option<String>,
    pub live_commit: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BuildRow {
    pub id: i64,
    pub repo_id: i64,
    pub status: String,
    pub error: Option<String>,
    pub pusher_id: Option<i64>,
    pub commit: String,
    pub duration: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeploymentRow {
    pub id: i64,
    pub repo_id: i64,
    pub status: String,
    pub artifact_url: String,
    pub environment: String,
    pub pages_build_version: String,
    pub creator_id: Option<i64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn site(s: &AppState, repo_id: i64) -> ApiResult<Option<SiteRow>> {
    Ok(sqlx::query_as("SELECT * FROM pages_sites WHERE repo_id = $1").bind(repo_id).fetch_optional(&s.db).await?)
}

fn html_url(s: &AppState, ctx: &RepoCtx) -> String {
    format!("{}/pages/{}/{}/", s.base_url(), ctx.owner.login, ctx.repo.name)
}

async fn has_custom_404(s: &AppState, site: &SiteRow) -> bool {
    match &site.live_prefix {
        Some(p) => s.store.head(&ObjPath::from(format!("{p}404.html"))).await.is_ok(),
        None => false,
    }
}

async fn site_json(s: &AppState, ctx: &RepoCtx, site: &SiteRow) -> Value {
    let full = ctx.full_name();
    let mut v = json!({
        "url": format!("{}/repos/{full}/pages", s.api_url()),
        "status": site.status,
        "cname": site.cname,
        "custom_404": has_custom_404(s, site).await,
        "html_url": html_url(s, ctx),
        "build_type": site.build_type,
        "public": site.public,
        "https_enforced": site.https_enforced,
        "pending_domain_unverified_at": null,
        "protected_domain_state": null,
    });
    if site.build_type == "legacy" {
        v["source"] = json!({"branch": site.source_branch, "path": site.source_path});
    }
    v
}

fn build_json(s: &AppState, ctx: &RepoCtx, b: &BuildRow, pusher: Option<&User>) -> Value {
    json!({
        "url": format!("{}/repos/{}/pages/builds/{}", s.api_url(), ctx.full_name(), b.id),
        "status": b.status,
        "error": {"message": b.error},
        "pusher": pusher.map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)),
        "commit": b.commit,
        "duration": b.duration,
        "created_at": crate::json::ts(b.created_at),
        "updated_at": crate::json::ts(b.updated_at),
    })
}

async fn builds_json(s: &AppState, ctx: &RepoCtx, rows: &[BuildRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().filter_map(|b| b.pusher_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    Ok(rows.iter().map(|b| build_json(s, ctx, b, b.pusher_id.and_then(|i| users.get(&i)))).collect())
}

fn invalid(msg: impl Into<String>) -> ApiError {
    let mut e = ApiError::unprocessable(msg);
    e.errors = None;
    e
}

// ---------------------------------------------------------------------------------------------
// Service layer

#[derive(Debug, Deserialize, Default)]
pub struct SourceInput {
    pub branch: Option<String>,
    pub path: Option<String>,
}

/// `source` in PUT may be an object or a legacy string (`gh-pages`, `master`, `master /docs`).
fn parse_source(v: &Value) -> ApiResult<SourceInput> {
    match v {
        Value::Object(_) => serde_json::from_value(v.clone()).map_err(|_| invalid("Invalid source")),
        Value::String(st) => {
            let mut parts = st.split_whitespace();
            Ok(SourceInput { branch: parts.next().map(str::to_string), path: parts.next().map(str::to_string) })
        }
        _ => Err(invalid("Invalid source")),
    }
}

async fn check_source(s: &AppState, ctx: &RepoCtx, src: &SourceInput) -> ApiResult<(String, String)> {
    let branch = src.branch.clone().filter(|b| !b.is_empty()).ok_or_else(|| ApiError::validation("Page", "source.branch", "missing_field"))?;
    let path = src.path.clone().unwrap_or_else(|| "/".into());
    if path != "/" && path != "/docs" {
        return Err(invalid("The source path must be either \"/\" or \"/docs\""));
    }
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    if !refs.contains_key(&format!("refs/heads/{branch}")) {
        return Err(invalid(format!("The branch {branch} does not exist")));
    }
    Ok((branch, path))
}

#[derive(Debug, Deserialize, Default)]
pub struct CreateSite {
    pub build_type: Option<String>,
    pub source: Option<SourceInput>,
}

/// Enables Pages (admin). Legacy sites are built immediately from the source branch.
pub async fn create_site_svc(s: &AppState, ctx: &RepoCtx, actor: &User, req: CreateSite) -> ApiResult<SiteRow> {
    ctx.require(Perm::Admin)?;
    if site(s, ctx.repo.id).await?.is_some() {
        return Err(ApiError::conflict("GitHub Pages is already enabled."));
    }
    let build_type = req.build_type.unwrap_or_else(|| if req.source.is_some() { "legacy".into() } else { "workflow".into() });
    let (branch, path) = match (build_type.as_str(), &req.source) {
        ("legacy", Some(src)) => check_source(s, ctx, src).await?,
        ("legacy", None) => return Err(ApiError::validation("Page", "source", "missing_field")),
        ("workflow", Some(src)) => check_source(s, ctx, src).await?,
        ("workflow", None) => (ctx.repo.default_branch.clone(), "/".into()),
        _ => return Err(ApiError::validation("Page", "build_type", "invalid")),
    };
    let row: SiteRow = sqlx::query_as(
        "INSERT INTO pages_sites (repo_id, build_type, source_branch, source_path, public) VALUES ($1,$2,$3,$4,$5) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(&build_type)
    .bind(&branch)
    .bind(&path)
    .bind(!ctx.repo.private)
    .fetch_one(&s.db)
    .await?;
    if build_type == "legacy" {
        start_build(s, ctx, Some(actor)).await?;
    }
    Ok(row)
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateSite {
    pub cname: Option<Option<String>>,
    pub https_enforced: Option<bool>,
    pub build_type: Option<String>,
    pub source: Option<Value>,
    pub public: Option<bool>,
}

pub async fn update_site_svc(s: &AppState, ctx: &RepoCtx, actor: &User, req: UpdateSite) -> ApiResult<()> {
    ctx.require(Perm::Admin)?;
    let cur = site(s, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    if req.cname.is_none() && req.https_enforced.is_none() && req.build_type.is_none() && req.source.is_none() && req.public.is_none() {
        return Err(ApiError::bad_request("At least one of cname, https_enforced, build_type, source or public must be provided"));
    }
    let build_type = match req.build_type {
        Some(b) if b == "legacy" || b == "workflow" => b,
        Some(_) => return Err(ApiError::validation("Page", "build_type", "invalid")),
        None => cur.build_type.clone(),
    };
    let (branch, path) = match &req.source {
        Some(v) => check_source(s, ctx, &parse_source(v)?).await?,
        None => (cur.source_branch.clone(), cur.source_path.clone()),
    };
    let cname = match req.cname {
        Some(Some(c)) if c.is_empty() => None,
        Some(Some(c)) => {
            if !c.contains('.') || c.len() > 253 || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-') {
                return Err(ApiError::validation("Page", "cname", "invalid"));
            }
            Some(c.to_lowercase())
        }
        Some(None) => None,
        None => cur.cname.clone(),
    };
    let https = req.https_enforced.unwrap_or(cur.https_enforced);
    if https && cname.is_some() && req.https_enforced == Some(true) && cur.cname != cname {
        // GitHub cannot enforce HTTPS until a certificate exists for a new custom domain.
        return Err(ApiError::validation("Page", "https_enforced", "invalid"));
    }
    let public = req.public.unwrap_or(cur.public);
    sqlx::query(
        "UPDATE pages_sites SET build_type = $2, source_branch = $3, source_path = $4, cname = $5, https_enforced = $6,
            public = $7, updated_at = now() WHERE repo_id = $1",
    )
    .bind(ctx.repo.id)
    .bind(&build_type)
    .bind(&branch)
    .bind(&path)
    .bind(&cname)
    .bind(https)
    .bind(public)
    .execute(&s.db)
    .await?;
    if build_type == "legacy" && (branch != cur.source_branch || path != cur.source_path || cur.build_type != "legacy") {
        start_build(s, ctx, Some(actor)).await?;
    }
    Ok(())
}

async fn delete_prefix(s: &AppState, prefix: &str) {
    let p = ObjPath::from(prefix.trim_end_matches('/'));
    let objs: Vec<_> = match s.store.list(Some(&p)).try_collect().await {
        Ok(v) => v,
        Err(_) => return,
    };
    for o in objs {
        let _ = s.store.delete(&o.location).await;
    }
}

pub async fn delete_site_svc(s: &AppState, ctx: &RepoCtx) -> ApiResult<()> {
    ctx.require(Perm::Admin)?;
    let cur = site(s, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    sqlx::query("DELETE FROM pages_sites WHERE repo_id = $1").bind(ctx.repo.id).execute(&s.db).await?;
    sqlx::query("DELETE FROM pages_builds WHERE repo_id = $1").bind(ctx.repo.id).execute(&s.db).await?;
    if cur.live_prefix.is_some() {
        delete_prefix(s, &format!("pages/{}/", ctx.repo.id)).await;
    }
    Ok(())
}

/// Atomically makes `prefix` the live site and removes the previous generation.
async fn publish(s: &AppState, repo_id: i64, prefix: &str, commit: &str) -> anyhow::Result<()> {
    let old: Option<Option<String>> = sqlx::query_scalar(
        "UPDATE pages_sites p SET live_prefix = $2, live_commit = $3, status = 'built', updated_at = now()
           FROM (SELECT live_prefix FROM pages_sites WHERE repo_id = $1 FOR UPDATE) old
          WHERE p.repo_id = $1 RETURNING old.live_prefix",
    )
    .bind(repo_id)
    .bind(prefix)
    .bind(commit)
    .fetch_optional(&s.db)
    .await?;
    match old {
        Some(Some(o)) if o != prefix => delete_prefix(s, &o).await,
        Some(_) => {}
        // Site deleted meanwhile: drop the new files.
        None => delete_prefix(s, prefix).await,
    }
    Ok(())
}

/// Queues a legacy build of the source branch tip and runs it in the background.
pub async fn start_build(s: &AppState, ctx: &RepoCtx, pusher: Option<&User>) -> ApiResult<BuildRow> {
    let b: BuildRow = sqlx::query_as("INSERT INTO pages_builds (repo_id, pusher_id, status) VALUES ($1, $2, 'queued') RETURNING *")
        .bind(ctx.repo.id)
        .bind(pusher.map(|p| p.id))
        .fetch_one(&s.db)
        .await?;
    sqlx::query("UPDATE pages_sites SET status = 'building' WHERE repo_id = $1").bind(ctx.repo.id).execute(&s.db).await?;
    let (s2, ctx2, id) = (s.clone(), ctx.clone(), b.id);
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let res = run_build(&s2, &ctx2, id).await;
        let ms = started.elapsed().as_millis() as i32;
        let (status, err) = match &res {
            Ok(_) => ("built", None),
            Err(e) => ("errored", Some(format!("{e:#}"))),
        };
        let _ = sqlx::query("UPDATE pages_builds SET status = $2, error = $3, duration = $4, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(status)
            .bind(&err)
            .bind(ms)
            .execute(&s2.db)
            .await;
        if err.is_some() {
            let _ = sqlx::query("UPDATE pages_sites SET status = 'errored' WHERE repo_id = $1").bind(ctx2.repo.id).execute(&s2.db).await;
        }
    });
    Ok(b)
}

/// Upper bound on the size of a single published file.
const MAX_FILE: u64 = 100 * 1024 * 1024;

async fn run_build(s: &AppState, ctx: &RepoCtx, build_id: i64) -> anyhow::Result<()> {
    let Some(site) = site(s, ctx.repo.id).await.map_err(|e| anyhow::anyhow!(e.message))? else { anyhow::bail!("Pages is not enabled") };
    sqlx::query("UPDATE pages_builds SET status = 'building', updated_at = now() WHERE id = $1").bind(build_id).execute(&s.db).await?;
    let path = s.git.sync(ctx.repo.id).await?;
    let git = &s.git.git;
    let refname = format!("refs/heads/{}", site.source_branch);
    let sha = crate::git::read::resolve_commit(git, &path, &refname).await?.ok_or_else(|| anyhow::anyhow!("The branch {} does not exist", site.source_branch))?;
    sqlx::query("UPDATE pages_builds SET commit = $2 WHERE id = $1").bind(build_id).bind(&sha).execute(&s.db).await?;
    let treeish = if site.source_path == "/docs" { format!("{sha}:docs") } else { sha.clone() };
    let entries = crate::git::read::ls_tree(git, &path, &treeish, None, true).await?;
    let blobs: Vec<_> = entries.into_iter().filter(|e| e.kind == "blob" && e.size.unwrap_or(0) <= MAX_FILE).collect();
    let prefix = format!("pages/{}/b{build_id}/", ctx.repo.id);
    for chunk in blobs.chunks(64) {
        let revs: Vec<String> = chunk.iter().map(|e| e.sha.clone()).collect();
        let objs = crate::git::read::cat_objects(git, &path, &revs).await?;
        for (e, o) in chunk.iter().zip(objs) {
            if let Some(o) = o {
                s.store.put(&ObjPath::from(format!("{prefix}{}", e.path)), PutPayload::from(Bytes::from(o.data))).await?;
            }
        }
    }
    publish(s, ctx.repo.id, &prefix, &sha).await
}

/// Builds legacy sites when their source branch changes (`api::repos_stats::on_refs_changed`).
pub async fn on_refs_changed(s: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) {
    let Ok(Some(site)) = site(s, ctx.repo.id).await else { return };
    if site.build_type != "legacy" {
        return;
    }
    let target = format!("refs/heads/{}", site.source_branch);
    if changed.iter().any(|(r, _, new)| *r == target && new != crate::webhooks::ZERO_SHA) {
        if let Err(e) = start_build(s, ctx, actor).await {
            tracing::warn!(error = %e.message, "pages build failed to start");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Deployments (artifact uploads)

/// Loads the bytes behind an artifact URL of this forge: a release asset (API or browser
/// download URL) of a repository the caller can read.
async fn fetch_artifact(s: &AppState, actor: &User, url: &str) -> ApiResult<Bytes> {
    let api = s.api_url();
    let base = s.base_url();
    let storage_key: Option<String> = if let Some(rest) = url.strip_prefix(&format!("{api}/repos/")) {
        // {o}/{r}/releases/assets/{id}
        let p: Vec<&str> = rest.split('/').collect();
        match p.as_slice() {
            [o, r, "releases", "assets", id] => {
                let ctx = db::load_repo(&s.db, Some(actor), o, r).await?;
                let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
                sqlx::query_scalar(
                    "SELECT a.storage_key FROM release_assets a JOIN releases rel ON rel.id = a.release_id WHERE a.id = $1 AND rel.repo_id = $2 AND a.state = 'uploaded'",
                )
                .bind(id)
                .bind(ctx.repo.id)
                .fetch_optional(&s.db)
                .await?
            }
            _ => None,
        }
    } else if let Some(rest) = url.strip_prefix(&format!("{base}/")) {
        // {o}/{r}/releases/download/{tag}/{name}
        let p: Vec<&str> = rest.splitn(5, '/').collect();
        match p.as_slice() {
            [o, r, "releases", "download", tail] => {
                let ctx = db::load_repo(&s.db, Some(actor), o, r).await?;
                let (tag, name) = tail.rsplit_once('/').ok_or_else(ApiError::not_found)?;
                let dec = |x: &str| percent_encoding::percent_decode_str(x).decode_utf8_lossy().into_owned();
                sqlx::query_scalar(
                    "SELECT a.storage_key FROM release_assets a JOIN releases rel ON rel.id = a.release_id
                      WHERE rel.repo_id = $1 AND rel.tag_name = $2 AND a.name = $3 AND a.state = 'uploaded'",
                )
                .bind(ctx.repo.id)
                .bind(dec(tag))
                .bind(dec(name))
                .fetch_optional(&s.db)
                .await?
            }
            _ => None,
        }
    } else {
        None
    };
    let key = storage_key.ok_or_else(|| invalid("artifact_url must reference an artifact or release asset on this server"))?;
    let got = s.store.get(&ObjPath::from(key)).await?;
    Ok(got.bytes().await?)
}

/// Files of a site artifact: `(path, bytes)`.
fn unpack_artifact(data: &[u8]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    if data.starts_with(b"PK\x03\x04") {
        let entries = unzip(data)?;
        // actions/upload-pages-artifact: a zip holding `artifact.tar`.
        if let Some((_, tar)) = entries.iter().find(|(n, _)| n == "artifact.tar") {
            return untar(tar);
        }
        return Ok(entries);
    }
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
        return untar(&out);
    }
    untar(data)
}

fn clean(name: &str) -> Option<String> {
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || parts.iter().any(|p| *p == "..") {
        return None;
    }
    Some(parts.join("/"))
}

/// Minimal ustar / GNU tar reader (regular files only).
fn untar(data: &[u8]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut off = 0usize;
    let mut long_name: Option<String> = None;
    while off + 512 <= data.len() {
        let h = &data[off..off + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let field = |a: usize, b: usize| String::from_utf8_lossy(&h[a..b]).trim_end_matches('\0').to_string();
        let size = usize::from_str_radix(field(124, 136).trim(), 8).unwrap_or(0);
        let kind = h[156];
        let prefix = field(345, 500);
        let mut name = field(0, 100);
        if !prefix.is_empty() && &h[257..262] == b"ustar" {
            name = format!("{prefix}/{name}");
        }
        let body_start = off + 512;
        let body_end = body_start.checked_add(size).filter(|e| *e <= data.len()).ok_or_else(|| anyhow::anyhow!("truncated tar"))?;
        let body = &data[body_start..body_end];
        match kind {
            b'L' => long_name = Some(String::from_utf8_lossy(body).trim_end_matches('\0').to_string()),
            b'0' | 0 => {
                let n = long_name.take().unwrap_or(name);
                if let Some(c) = clean(&n) {
                    out.push((c, body.to_vec()));
                }
            }
            _ => long_name = None,
        }
        off = body_start + size.div_ceil(512) * 512;
    }
    Ok(out)
}

/// Minimal zip reader (stored and deflated entries, via the central directory).
fn unzip(data: &[u8]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let u16le = |o: usize| -> usize { data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize).unwrap_or(0) };
    let u32le = |o: usize| -> usize { data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).unwrap_or(0) };
    let eocd = (0..data.len().saturating_sub(21)).rev().find(|&i| data[i..].starts_with(b"PK\x05\x06")).ok_or_else(|| anyhow::anyhow!("not a zip"))?;
    let count = u16le(eocd + 10);
    let mut p = u32le(eocd + 16);
    let mut out = Vec::new();
    for _ in 0..count {
        if !data.get(p..).is_some_and(|d| d.starts_with(b"PK\x01\x02")) {
            anyhow::bail!("bad zip directory");
        }
        let method = u16le(p + 10);
        let csize = u32le(p + 20);
        let nlen = u16le(p + 28);
        let elen = u16le(p + 30);
        let clen = u16le(p + 32);
        let local = u32le(p + 42);
        let name = String::from_utf8_lossy(data.get(p + 46..p + 46 + nlen).unwrap_or_default()).into_owned();
        p += 46 + nlen + elen + clen;
        let lstart = local + 30 + u16le(local + 26) + u16le(local + 28);
        let raw = data.get(lstart..lstart + csize).ok_or_else(|| anyhow::anyhow!("truncated zip"))?;
        let body = match method {
            0 => raw.to_vec(),
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(raw).read_to_end(&mut v)?;
                v
            }
            m => anyhow::bail!("unsupported zip compression method {m}"),
        };
        if name.ends_with('/') {
            continue;
        }
        if let Some(c) = clean(&name) {
            out.push((c, body));
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct DeployReq {
    artifact_url: Option<String>,
    artifact_id: Option<i64>,
    environment: Option<String>,
    pages_build_version: Option<String>,
    oidc_token: Option<String>,
}

pub async fn create_deployment_svc(s: &AppState, ctx: &RepoCtx, actor: &User, artifact_url: &str, environment: &str, version: &str) -> ApiResult<DeploymentRow> {
    ctx.require(Perm::Write)?;
    let st = site(s, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    if st.build_type != "workflow" {
        return Err(ApiError::bad_request("Pages is not configured to deploy from a GitHub Actions workflow (build_type is legacy)"));
    }
    let data = fetch_artifact(s, actor, artifact_url).await?;
    let d: DeploymentRow = sqlx::query_as(
        "INSERT INTO pages_deployments (repo_id, artifact_url, environment, pages_build_version, creator_id) VALUES ($1,$2,$3,$4,$5) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(artifact_url)
    .bind(environment)
    .bind(version)
    .bind(actor.id)
    .fetch_one(&s.db)
    .await?;
    let (s2, repo_id, id, version) = (s.clone(), ctx.repo.id, d.id, version.to_string());
    tokio::spawn(async move {
        let set = |status: &'static str, err: Option<String>| {
            let s3 = s2.clone();
            async move {
                // A cancelled deployment stays cancelled.
                let _ = sqlx::query("UPDATE pages_deployments SET status = $2, error = $3, updated_at = now() WHERE id = $1 AND status <> 'deployment_cancelled'")
                    .bind(id)
                    .bind(status)
                    .bind(err)
                    .execute(&s3.db)
                    .await;
            }
        };
        let res: anyhow::Result<()> = async {
            let files = unpack_artifact(&data)?;
            set("syncing_files", None).await;
            let prefix = format!("pages/{repo_id}/d{id}/");
            for (name, body) in files {
                s2.store.put(&ObjPath::from(format!("{prefix}{name}")), PutPayload::from(Bytes::from(body))).await?;
            }
            set("finished_file_sync", None).await;
            let cancelled: bool = sqlx::query_scalar("SELECT status = 'deployment_cancelled' FROM pages_deployments WHERE id = $1").bind(id).fetch_one(&s2.db).await?;
            if cancelled {
                delete_prefix(&s2, &prefix).await;
                return Ok(());
            }
            set("updating_pages", None).await;
            publish(&s2, repo_id, &prefix, &version).await?;
            set("succeed", None).await;
            Ok(())
        }
        .await;
        if let Err(e) = res {
            set("deployment_failed", Some(format!("{e:#}"))).await;
        }
    });
    Ok(d)
}

fn deployment_json(s: &AppState, ctx: &RepoCtx, d: &DeploymentRow) -> Value {
    json!({
        "id": d.id,
        "status_url": format!("{}/repos/{}/pages/deployments/{}", s.api_url(), ctx.full_name(), d.id),
        "page_url": html_url(s, ctx),
    })
}

// ---------------------------------------------------------------------------------------------
// Handlers

async fn get_site(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let st = site(&s, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(site_json(&s, &ctx, &st).await))
}

async fn create_site(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<CreateSite>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let st = create_site_svc(&s, &ctx, &u, b).await?;
    Ok((StatusCode::CREATED, axum::Json(site_json(&s, &ctx, &st).await)).into_response())
}

async fn update_site(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<UpdateSite>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    update_site_svc(&s, &ctx, &u, b).await?;
    Ok(no_content())
}

async fn delete_site(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    delete_site_svc(&s, &ctx).await?;
    Ok(no_content())
}

async fn site_ctx(s: &AppState, u: Option<&User>, o: &str, r: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, u, o, r).await?;
    site(s, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(ctx)
}

async fn list_builds(
    State(s): State<AppState>,
    uri: axum::extract::OriginalUri,
    AuthUser(u): AuthUser,
    Path((o, r)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = site_ctx(&s, Some(&u), &o, &r).await?;
    let rows: Vec<BuildRow> = sqlx::query_as("SELECT * FROM pages_builds WHERE repo_id = $1 ORDER BY id DESC LIMIT $2 OFFSET $3")
        .bind(ctx.repo.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM pages_builds WHERE repo_id = $1").bind(ctx.repo.id).fetch_one(&s.db).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), builds_json(&s, &ctx, &rows).await?))
}

async fn request_build(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = site_ctx(&s, Some(&u), &o, &r).await?;
    ctx.require(Perm::Write)?;
    let b = start_build(&s, &ctx, Some(&u)).await?;
    let url = format!("{}/repos/{}/pages/builds/{}", s.api_url(), ctx.full_name(), b.id);
    Ok((StatusCode::CREATED, axum::Json(json!({"url": url, "status": "queued"}))).into_response())
}

async fn latest_build(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = site_ctx(&s, Some(&u), &o, &r).await?;
    let b: BuildRow = sqlx::query_as("SELECT * FROM pages_builds WHERE repo_id = $1 ORDER BY id DESC LIMIT 1")
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(builds_json(&s, &ctx, &[b]).await?.remove(0)))
}

async fn get_build(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = site_ctx(&s, Some(&u), &o, &r).await?;
    let b: BuildRow = sqlx::query_as("SELECT * FROM pages_builds WHERE repo_id = $1 AND id = $2")
        .bind(ctx.repo.id)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(builds_json(&s, &ctx, &[b]).await?.remove(0)))
}

async fn create_deployment(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<DeployReq>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let artifact = match (b.artifact_url.filter(|x| !x.is_empty()), b.artifact_id) {
        (Some(url), _) => url,
        (None, Some(id)) => format!("{}/repos/{}/actions/artifacts/{id}", s.api_url(), ctx.full_name()),
        (None, None) => return Err(ApiError::validation("PageDeployment", "artifact_url", "missing_field")),
    };
    let version = b.pages_build_version.filter(|x| !x.is_empty()).ok_or_else(|| ApiError::validation("PageDeployment", "pages_build_version", "missing_field"))?;
    // GitHub requires the workflow's OIDC token; forge has no OIDC issuer yet, so only its
    // presence is checked.
    b.oidc_token.filter(|x| !x.is_empty()).ok_or_else(|| ApiError::validation("PageDeployment", "oidc_token", "missing_field"))?;
    let env = b.environment.unwrap_or_else(|| "github-pages".into());
    let d = create_deployment_svc(&s, &ctx, &u, &artifact, &env, &version).await?;
    Ok(axum::Json(deployment_json(&s, &ctx, &d)).into_response())
}

async fn load_deployment(s: &AppState, ctx: &RepoCtx, id: &str) -> ApiResult<DeploymentRow> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    sqlx::query_as("SELECT * FROM pages_deployments WHERE repo_id = $1 AND id = $2")
        .bind(ctx.repo.id)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_deployment(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let d = load_deployment(&s, &ctx, &id).await?;
    Ok(Json(json!({"status": d.status})))
}

async fn cancel_deployment(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Write)?;
    let d = load_deployment(&s, &ctx, &id).await?;
    sqlx::query(
        "UPDATE pages_deployments SET status = 'deployment_cancelled', updated_at = now()
          WHERE id = $1 AND status IN ('deployment_in_progress', 'syncing_files', 'finished_file_sync')",
    )
    .bind(d.id)
    .execute(&s.db)
    .await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Static site server

async fn serve_root(Path((o, r)): Path<(String, String)>) -> Redirect {
    Redirect::permanent(&format!("/pages/{o}/{r}/"))
}

async fn serve_index(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> Response {
    serve(&s, u.as_ref(), &o, &r, "").await
}

async fn serve_path(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r, p)): Path<(String, String, String)>) -> Response {
    serve(&s, u.as_ref(), &o, &r, &p).await
}

fn not_found_page() -> Response {
    (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], "<h1>404</h1><p>There isn't a site here.</p>").into_response()
}

async fn serve(s: &AppState, u: Option<&User>, o: &str, r: &str, path: &str) -> Response {
    let Ok(ctx) = db::load_repo(&s.db, u, o, r).await else { return not_found_page() };
    let Ok(Some(st)) = site(s, ctx.repo.id).await else { return not_found_page() };
    // Private Pages need read access (load_repo already checked it for private repos).
    if !st.public && u.is_none() {
        return not_found_page();
    }
    let Some(prefix) = st.live_prefix.clone() else { return not_found_page() };
    let Some(clean_path) = (if path.is_empty() { Some(String::new()) } else { clean(path) }) else { return not_found_page() };
    let mut candidates = Vec::new();
    if path.is_empty() || path.ends_with('/') {
        candidates.push(format!("{clean_path}{}index.html", if clean_path.is_empty() { "" } else { "/" }));
    } else {
        candidates.push(clean_path.clone());
        candidates.push(format!("{clean_path}.html"));
        candidates.push(format!("{clean_path}/index.html"));
    }
    for (i, c) in candidates.iter().enumerate() {
        if let Ok(got) = s.store.get(&ObjPath::from(format!("{prefix}{c}"))).await {
            // A directory without trailing slash redirects, like GitHub Pages.
            if i == 2 {
                return Redirect::permanent(&format!("/pages/{o}/{r}/{clean_path}/")).into_response();
            }
            return file_response(c, StatusCode::OK, got.into_stream());
        }
    }
    if let Ok(got) = s.store.get(&ObjPath::from(format!("{prefix}404.html"))).await {
        return file_response("404.html", StatusCode::NOT_FOUND, got.into_stream());
    }
    not_found_page()
}

fn file_response<S>(name: &str, status: StatusCode, stream: S) -> Response
where
    S: futures::Stream<Item = Result<Bytes, object_store::Error>> + Send + 'static,
{
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let mut res = Response::new(Body::from_stream(stream));
    *res.status_mut() = status;
    let h = res.headers_mut();
    let ct = if mime.type_() == "text" { format!("{}; charset=utf-8", mime.essence_str()) } else { mime.essence_str().to_string() };
    if let Ok(v) = HeaderValue::from_str(&ct) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=600"));
    // Pages share the forge's origin: run them in an opaque origin so their scripts can't use
    // the visitor's forge session.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox allow-scripts allow-forms allow-popups allow-modals allow-downloads"),
    );
    res
}

#[cfg(test)]
mod tests {
    #[test]
    fn tar_roundtrip() {
        // A one-file ustar archive.
        let mut h = [0u8; 512];
        h[..9].copy_from_slice(b"index.htm");
        h[124..135].copy_from_slice(b"00000000005");
        h[156] = b'0';
        h[257..262].copy_from_slice(b"ustar");
        let mut data = h.to_vec();
        let mut body = b"hello".to_vec();
        body.resize(512, 0);
        data.extend(body);
        data.extend([0u8; 1024]);
        let files = super::untar(&data).unwrap();
        assert_eq!(files, vec![("index.htm".to_string(), b"hello".to_vec())]);
        assert!(super::clean("../x").is_none());
        assert_eq!(super::clean("./a//b").as_deref(), Some("a/b"));
    }
}
