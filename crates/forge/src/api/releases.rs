//! Releases and release assets, including the GHES-style uploads endpoint
//! (`/api/uploads/repos/{o}/{r}/releases/{id}/assets`) and browser downloads
//! (`/{o}/{r}/releases/download/{tag}/{name}`). Asset bytes are streamed into object storage at
//! `releases/{repo_id}/{asset_id}`; nothing is buffered in memory.
use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{OriginalUri, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;
use object_store::path::Path as ObjPath;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::plat::{self, CreateReaction, ReactionQuery};
use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::plumbing;
use crate::json::release::{AssetRow, ReleaseRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/releases", get(list).post(create))
        .route("/repos/{owner}/{repo}/releases/latest", get(latest))
        .route("/repos/{owner}/{repo}/releases/generate-notes", post(generate_notes_handler))
        .route("/repos/{owner}/{repo}/releases/tags/{tag}", get(by_tag))
        .route("/repos/{owner}/{repo}/releases/assets/{asset_id}", get(get_asset).patch(update_asset).delete(delete_asset))
        .route("/repos/{owner}/{repo}/releases/{release_id}", get(get_one).patch(update).delete(delete))
        .route("/repos/{owner}/{repo}/releases/{release_id}/assets", get(list_assets).post(upload))
        .route("/repos/{owner}/{repo}/releases/{release_id}/reactions", get(list_reactions).post(create_reaction))
        .route("/repos/{owner}/{repo}/releases/{release_id}/reactions/{reaction_id}", axum::routing::delete(delete_reaction))
}

/// Mounted at `/api/uploads` (GHES uploads endpoint).
pub fn upload_routes() -> Router<AppState> {
    Router::new().route("/repos/{owner}/{repo}/releases/{release_id}/assets", post(upload))
}

/// Browser download routes (outside /api).
pub fn download_routes() -> Router<AppState> {
    Router::new()
        .route("/{owner}/{repo}/releases/latest/download/{name}", get(download_latest))
        .route("/{owner}/{repo}/releases/download/{*rest}", get(download_browser))
}

pub const RELEASE_REACTIONS: [&str; 6] = ["+1", "laugh", "heart", "hooray", "rocket", "eyes"];

// ---------------------------------------------------------------------------------------------
// Service layer

/// Loads a release by id; drafts are only visible to users with push access.
pub async fn load_release(db: &sqlx::PgPool, ctx: &RepoCtx, id: i64) -> ApiResult<ReleaseRow> {
    let r: ReleaseRow = sqlx::query_as("SELECT * FROM releases WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if r.draft && ctx.perm < Perm::Write {
        return Err(ApiError::not_found());
    }
    Ok(r)
}

/// Release by tag name. Published releases always; drafts only when `include_drafts`
/// (callers pass `ctx.perm >= Write`).
pub async fn release_by_tag(db: &sqlx::PgPool, ctx: &RepoCtx, tag: &str, include_drafts: bool) -> ApiResult<Option<ReleaseRow>> {
    Ok(sqlx::query_as("SELECT * FROM releases WHERE repo_id = $1 AND tag_name = $2 AND (NOT draft OR $3) ORDER BY draft LIMIT 1")
        .bind(ctx.repo.id)
        .bind(tag)
        .bind(include_drafts)
        .fetch_optional(db)
        .await?)
}

/// The repository's latest release: the one explicitly marked latest, else the most recently
/// published non-draft, non-prerelease release that did not opt out with `make_latest=false`.
pub async fn latest_release(db: &sqlx::PgPool, repo_id: i64) -> ApiResult<Option<ReleaseRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM releases WHERE repo_id = $1 AND NOT draft AND NOT prerelease
            AND (is_latest OR make_latest <> 'false')
          ORDER BY is_latest DESC, published_at DESC NULLS LAST, id DESC LIMIT 1",
    )
    .bind(repo_id)
    .fetch_optional(db)
    .await?)
}

/// Lists releases newest first; drafts included when `include_drafts`.
pub async fn list_releases(db: &sqlx::PgPool, repo_id: i64, include_drafts: bool, p: PageParams) -> ApiResult<(Vec<ReleaseRow>, i64)> {
    let rows = sqlx::query_as(
        "SELECT * FROM releases WHERE repo_id = $1 AND (NOT draft OR $2)
          ORDER BY created_at DESC, id DESC LIMIT $3 OFFSET $4",
    )
    .bind(repo_id)
    .bind(include_drafts)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM releases WHERE repo_id = $1 AND (NOT draft OR $2)")
        .bind(repo_id)
        .bind(include_drafts)
        .fetch_one(db)
        .await?;
    Ok((rows, total))
}

/// Renders releases with authors, assets (+uploaders) and reaction counts, batched.
pub async fn releases_json(s: &AppState, ctx: &RepoCtx, rows: &[ReleaseRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let assets: Vec<AssetRow> = sqlx::query_as("SELECT * FROM release_assets WHERE release_id = ANY($1) ORDER BY id")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let mut user_ids: Vec<i64> = rows.iter().filter_map(|r| r.author_id).collect();
    user_ids.extend(assets.iter().filter_map(|a| a.uploader_id));
    let users = crate::json::issue::users_by_ids(&s.db, &user_ids).await?;
    let reactions = plat::reaction_counts(&s.db, "release", &ids).await?;
    let mut by_release: HashMap<i64, Vec<&AssetRow>> = HashMap::new();
    for a in &assets {
        by_release.entry(a.release_id).or_default().push(a);
    }
    let empty = HashMap::new();
    Ok(rows
        .iter()
        .map(|r| {
            let list: Vec<(&AssetRow, Option<&User>)> = by_release
                .get(&r.id)
                .map(|v| v.iter().map(|a| (*a, a.uploader_id.and_then(|u| users.get(&u)))).collect())
                .unwrap_or_default();
            crate::json::release::release(
                &s.api_url(),
                s.base_url(),
                ctx,
                r,
                r.author_id.and_then(|u| users.get(&u)),
                &list,
                Some(reactions.get(&r.id).unwrap_or(&empty)),
            )
        })
        .collect())
}

pub async fn release_json(s: &AppState, ctx: &RepoCtx, r: &ReleaseRow) -> ApiResult<Value> {
    Ok(releases_json(s, ctx, std::slice::from_ref(r)).await?.remove(0))
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && !tag.starts_with('-')
        && !tag.starts_with('/')
        && !tag.ends_with('/')
        && !tag.ends_with(".lock")
        && !tag.ends_with('.')
        && !tag.contains("..")
        && !tag.contains("@{")
        && !tag.contains("//")
        && !tag.chars().any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
}

/// Creates `refs/tags/{tag}` at `target` (branch, tag or sha) if the tag does not exist yet.
/// Returns 422 when `target` cannot be resolved.
pub async fn ensure_tag(s: &AppState, ctx: &RepoCtx, actor: &User, tag: &str, target: &str) -> ApiResult<()> {
    let refname = format!("refs/tags/{tag}");
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    if refs.contains_key(&refname) {
        return Ok(());
    }
    if !valid_tag(tag) {
        return Err(ApiError::validation("Release", "tag_name", "invalid"));
    }
    let path = s.git.sync(ctx.repo.id).await?;
    if plumbing::rev_parse(&s.git.git, &path, target).await.is_none() {
        return Err(ApiError::validation("Release", "target_commitish", "invalid"));
    }
    let git = s.git.git.clone();
    let target = target.to_string();
    let rn = refname.clone();
    let (_, updates) = s
        .git
        .write(ctx.repo.id, |path| async move {
            let sha = plumbing::rev_parse(&git, &path, &target)
                .await
                .ok_or_else(|| anyhow::anyhow!("target vanished"))?;
            // Another writer may have created the tag meanwhile; keep theirs.
            if plumbing::rev_parse(&git, &path, &rn).await.is_none() {
                plumbing::update_ref(&git, &path, &rn, &sha, None).await?;
            }
            Ok(())
        })
        .await?;
    let changed = updates.changed();
    if !changed.is_empty() {
        if let Err(e) = crate::hooks::after_refs_changed(s, ctx, Some(actor), &changed).await {
            tracing::error!(error = %format!("{e:#}"), "after_refs_changed failed");
        }
    }
    Ok(())
}

#[derive(Deserialize, Default, Clone)]
pub struct CreateRelease {
    pub tag_name: Option<String>,
    pub target_commitish: Option<String>,
    pub name: Option<String>,
    pub body: Option<String>,
    pub draft: Option<bool>,
    pub prerelease: Option<bool>,
    pub generate_release_notes: Option<bool>,
    pub make_latest: Option<Value>,
    pub discussion_category_name: Option<String>,
}

fn make_latest_str(v: &Option<Value>) -> ApiResult<Option<String>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if matches!(s.as_str(), "true" | "false" | "legacy") => Ok(Some(s.clone())),
        Some(Value::Bool(b)) => Ok(Some(b.to_string())),
        _ => Err(ApiError::validation("Release", "make_latest", "invalid")),
    }
}

/// Marks `r` as the latest release when it is published, not a prerelease and did not opt out.
/// `claim` is true when this change should (re)claim "latest": a publish, an explicit
/// `make_latest`, or a prerelease becoming a full release. Plain edits never steal it.
async fn apply_latest(db: &sqlx::PgPool, r: &ReleaseRow, claim: bool) -> ApiResult<()> {
    if claim && !r.draft && !r.prerelease && r.make_latest == "true" {
        sqlx::query("UPDATE releases SET is_latest = (id = $2) WHERE repo_id = $1 AND (is_latest OR id = $2)")
            .bind(r.repo_id)
            .bind(r.id)
            .execute(db)
            .await?;
    } else if r.is_latest && (r.draft || r.prerelease || r.make_latest == "false") {
        sqlx::query("UPDATE releases SET is_latest = FALSE WHERE id = $1").bind(r.id).execute(db).await?;
    }
    Ok(())
}

async fn emit_release_event(s: &AppState, ctx: &RepoCtx, actor: &User, r: &ReleaseRow, action: &str) {
    let body = match release_json(s, ctx, r).await {
        Ok(v) => v,
        Err(_) => return,
    };
    let payload = json!({
        "action": action,
        "release": body,
        "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor),
    });
    if let Err(e) = crate::webhooks::enqueue_event_ctx(s, ctx, "release", payload).await {
        tracing::error!(error = %format!("{e:#}"), "release webhook failed");
    }
}

pub async fn create_release(s: &AppState, ctx: &RepoCtx, actor: &User, input: CreateRelease) -> ApiResult<ReleaseRow> {
    ctx.require(Perm::Write)?;
    let tag = input.tag_name.clone().filter(|t| !t.is_empty()).ok_or_else(|| ApiError::validation("Release", "tag_name", "missing_field"))?;
    if !valid_tag(&tag) {
        return Err(ApiError::validation("Release", "tag_name", "invalid"));
    }
    let target = input.target_commitish.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| ctx.repo.default_branch.clone());
    let draft = input.draft.unwrap_or(false);
    let make_latest = make_latest_str(&input.make_latest)?.unwrap_or_else(|| "true".into());
    let mut body = input.body.clone().filter(|b| !b.is_empty());
    let mut name = input.name.clone().filter(|n| !n.is_empty());
    if input.generate_release_notes.unwrap_or(false) {
        let (gname, gbody) = generate_notes(s, ctx, &tag, Some(&target), None).await?;
        body = Some(match body {
            Some(b) => format!("{b}\n\n{gbody}"),
            None => gbody,
        });
        if name.is_none() {
            name = Some(gname);
        }
    }
    // A published release needs its tag: create it from the target if missing.
    if !draft {
        ensure_tag(s, ctx, actor, &tag, &target).await?;
    } else {
        let path = s.git.sync(ctx.repo.id).await?;
        let (_, refs) = s.git.refs(ctx.repo.id).await?;
        if !refs.contains_key(&format!("refs/tags/{tag}")) && plumbing::rev_parse(&s.git.git, &path, &target).await.is_none() {
            return Err(ApiError::validation("Release", "target_commitish", "invalid"));
        }
    }
    let r: Option<ReleaseRow> = sqlx::query_as(
        "INSERT INTO releases (repo_id, tag_name, target_commitish, name, body, draft, prerelease, make_latest, author_id, published_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9, CASE WHEN $6 THEN NULL ELSE now() END)
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(&tag)
    .bind(&target)
    .bind(&name)
    .bind(&body)
    .bind(draft)
    .bind(input.prerelease.unwrap_or(false))
    .bind(&make_latest)
    .bind(actor.id)
    .fetch_optional(&s.db)
    .await?;
    let r = r.ok_or_else(|| ApiError::validation("Release", "tag_name", "already_exists"))?;
    apply_latest(&s.db, &r, true).await?;
    let r = load_release(&s.db, ctx, r.id).await?;
    emit_release_event(s, ctx, actor, &r, "created").await;
    if !r.draft {
        emit_release_event(s, ctx, actor, &r, "published").await;
        if !r.prerelease {
            emit_release_event(s, ctx, actor, &r, "released").await;
        }
    }
    Ok(r)
}

#[derive(Deserialize, Default, Clone)]
pub struct UpdateRelease {
    pub tag_name: Option<String>,
    pub target_commitish: Option<String>,
    pub name: Option<String>,
    pub body: Option<String>,
    pub draft: Option<bool>,
    pub prerelease: Option<bool>,
    pub make_latest: Option<Value>,
    pub discussion_category_name: Option<String>,
}

pub async fn update_release(s: &AppState, ctx: &RepoCtx, actor: &User, id: i64, input: UpdateRelease) -> ApiResult<ReleaseRow> {
    ctx.require(Perm::Write)?;
    let cur = load_release(&s.db, ctx, id).await?;
    let tag = input.tag_name.clone().filter(|t| !t.is_empty()).unwrap_or(cur.tag_name.clone());
    if !valid_tag(&tag) {
        return Err(ApiError::validation("Release", "tag_name", "invalid"));
    }
    let target = input.target_commitish.clone().filter(|t| !t.is_empty()).unwrap_or(cur.target_commitish.clone());
    let draft = input.draft.unwrap_or(cur.draft);
    let make_latest = make_latest_str(&input.make_latest)?.unwrap_or(cur.make_latest.clone());
    if !draft && (cur.draft || tag != cur.tag_name) {
        ensure_tag(s, ctx, actor, &tag, &target).await?;
    }
    let r: ReleaseRow = sqlx::query_as(
        "UPDATE releases SET tag_name = $2, target_commitish = $3, name = COALESCE($4, name), body = COALESCE($5, body),
            draft = $6, prerelease = COALESCE($7, prerelease), make_latest = $8, updated_at = now(),
            published_at = CASE WHEN $6 THEN NULL ELSE COALESCE(published_at, now()) END
          WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(&tag)
    .bind(&target)
    .bind(&input.name)
    .bind(&input.body)
    .bind(draft)
    .bind(input.prerelease)
    .bind(&make_latest)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Release", "tag_name", "already_exists"),
        e => e.into(),
    })?;
    let claim = (cur.draft && !r.draft) || input.make_latest.is_some() || (cur.prerelease && !r.prerelease);
    apply_latest(&s.db, &r, claim).await?;
    let r = load_release(&s.db, ctx, id).await?;
    let action = if cur.draft && !r.draft { "published" } else { "edited" };
    emit_release_event(s, ctx, actor, &r, action).await;
    Ok(r)
}

pub async fn delete_release(s: &AppState, ctx: &RepoCtx, actor: &User, id: i64) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    let r = load_release(&s.db, ctx, id).await?;
    let keys: Vec<String> = sqlx::query_scalar("SELECT storage_key FROM release_assets WHERE release_id = $1")
        .bind(id)
        .fetch_all(&s.db)
        .await?;
    emit_release_event(s, ctx, actor, &r, "deleted").await;
    sqlx::query("DELETE FROM reactions WHERE subject_type = 'release' AND subject_id = $1").bind(id).execute(&s.db).await?;
    sqlx::query("DELETE FROM releases WHERE id = $1").bind(id).execute(&s.db).await?;
    for k in keys {
        let _ = s.store.delete(&ObjPath::from(k)).await;
    }
    Ok(())
}

/// Generates release notes (name, body) from merged pull requests (or, when there are none,
/// commits) between the previous release tag and `tag` (or `target` when the tag does not
/// exist yet).
pub async fn generate_notes(s: &AppState, ctx: &RepoCtx, tag: &str, target: Option<&str>, previous: Option<&str>) -> ApiResult<(String, String)> {
    let base = s.base_url();
    let full = ctx.full_name();
    let path = s.git.sync(ctx.repo.id).await?;
    let git = &s.git.git;
    let head = match plumbing::rev_parse(git, &path, &format!("refs/tags/{tag}")).await {
        Some(sha) => sha,
        None => {
            let t = target.unwrap_or(&ctx.repo.default_branch);
            plumbing::rev_parse(git, &path, t)
                .await
                .ok_or_else(|| ApiError::validation("Release", "target_commitish", "invalid"))?
        }
    };
    let prev_tag: Option<String> = match previous {
        Some(p) => {
            if plumbing::rev_parse(git, &path, &format!("refs/tags/{p}")).await.is_none() {
                return Err(ApiError::validation("Release", "previous_tag_name", "invalid"));
            }
            Some(p.to_string())
        }
        None => {
            let cands: Vec<String> = sqlx::query_scalar(
                "SELECT tag_name FROM releases WHERE repo_id = $1 AND NOT draft AND tag_name <> $2
                  ORDER BY published_at DESC NULLS LAST, id DESC LIMIT 10",
            )
            .bind(ctx.repo.id)
            .bind(tag)
            .fetch_all(&s.db)
            .await?;
            let mut found = None;
            for c in cands {
                if let Some(sha) = plumbing::rev_parse(git, &path, &format!("refs/tags/{c}")).await {
                    // Only tags that are ancestors of head make a sensible range.
                    let (ok, _, _) = git.try_run(&path, &["merge-base", "--is-ancestor", &sha, &head]).await?;
                    if ok && sha != head {
                        found = Some(c);
                        break;
                    }
                }
            }
            found
        }
    };
    let range = match &prev_tag {
        Some(p) => format!("refs/tags/{p}..{head}"),
        None => head.clone(),
    };
    let out = git.run(&path, &["log", "--format=%H%x00%P%x00%ae%x00%an%x00%s", "-n", "500", &range]).await?;
    let text = String::from_utf8_lossy(&out);
    struct C {
        sha: String,
        merge: bool,
        email: String,
        name: String,
        subject: String,
    }
    let commits: Vec<C> = text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(5, '\0').collect();
            (f.len() == 5).then(|| C {
                sha: f[0].into(),
                merge: f[1].split_whitespace().count() > 1,
                email: f[2].into(),
                name: f[3].into(),
                subject: f[4].into(),
            })
        })
        .collect();
    let shas: Vec<String> = commits.iter().map(|c| c.sha.clone()).collect();
    let prs: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT i.number, i.title, u.login FROM pull_requests p JOIN issues i ON i.id = p.issue_id
           LEFT JOIN users u ON u.id = i.user_id
          WHERE p.repo_id = $1 AND p.merged AND p.merge_commit_sha = ANY($2) ORDER BY i.number",
    )
    .bind(ctx.repo.id)
    .bind(&shas)
    .fetch_all(&s.db)
    .await?;
    let mut lines = Vec::new();
    if !prs.is_empty() {
        for (number, title, login) in &prs {
            let who = login.as_deref().map(|l| format!(" by @{l}")).unwrap_or_default();
            lines.push(format!("* {title}{who} in {base}/{full}/pull/{number}"));
        }
    } else {
        let emails: Vec<String> = commits.iter().map(|c| c.email.clone()).collect();
        let users = plat::users_by_emails(&s.db, &emails).await?;
        for c in commits.iter().filter(|c| !c.merge).take(100) {
            let who = match users.get(&c.email.to_lowercase()) {
                Some(u) => format!("@{}", u.login),
                None => c.name.clone(),
            };
            lines.push(format!("* {} by {who} in {}", c.subject, &c.sha[..7.min(c.sha.len())]));
        }
    }
    let mut body = String::new();
    if !lines.is_empty() {
        body.push_str("## What's Changed\n");
        body.push_str(&lines.join("\n"));
        body.push_str("\n\n");
    }
    let tag_seg = crate::json::release::seg(tag);
    match &prev_tag {
        Some(p) => body.push_str(&format!("**Full Changelog**: {base}/{full}/compare/{}...{tag_seg}", crate::json::release::seg(p))),
        None => body.push_str(&format!("**Full Changelog**: {base}/{full}/commits/{tag_seg}")),
    }
    Ok((tag.to_string(), body))
}

fn sanitize_asset_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.+@~".contains(c) { c } else { '.' })
        .collect();
    cleaned.trim_matches('.').to_string()
}

pub fn asset_key(repo_id: i64, asset_id: i64) -> String {
    format!("releases/{repo_id}/{asset_id}")
}

/// Streams `body` into object storage as a new asset of `release`.
pub async fn upload_asset(
    s: &AppState,
    ctx: &RepoCtx,
    actor: &User,
    release: &ReleaseRow,
    name: &str,
    label: Option<String>,
    content_type: &str,
    body: Body,
) -> ApiResult<AssetRow> {
    ctx.require(Perm::Write)?;
    let name = sanitize_asset_name(name);
    if name.is_empty() {
        return Err(ApiError::validation("ReleaseAsset", "name", "missing_field"));
    }
    let row: Option<AssetRow> = sqlx::query_as(
        "INSERT INTO release_assets (release_id, name, label, content_type, size, storage_key, uploader_id, state)
         VALUES ($1, $2, $3, $4, 0, '', $5, 'open') ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(release.id)
    .bind(&name)
    .bind(label.filter(|l| !l.is_empty()))
    .bind(content_type)
    .bind(actor.id)
    .fetch_optional(&s.db)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::validation("ReleaseAsset", "name", "already_exists"));
    };
    let key = asset_key(ctx.repo.id, row.id);
    let res: anyhow::Result<(i64, String)> = async {
        let mut w = object_store::buffered::BufWriter::new(s.store.clone(), ObjPath::from(key.clone()));
        let mut hasher = Sha256::new();
        let mut size: i64 = 0;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let _ = w.abort().await;
                    anyhow::bail!("upload interrupted: {e}");
                }
            };
            hasher.update(&chunk);
            size += chunk.len() as i64;
            // GitHub caps release assets at 2 GiB; without a cap a writer could fill storage.
            if size as u64 > crate::security::max_asset_bytes() {
                let _ = w.abort().await;
                anyhow::bail!("asset exceeds the maximum size");
            }
            if let Err(e) = w.write_all(&chunk).await {
                let _ = w.abort().await;
                return Err(e.into());
            }
        }
        w.shutdown().await?;
        Ok((size, format!("sha256:{}", hex::encode(hasher.finalize()))))
    }
    .await;
    match res {
        Ok((size, digest)) => {
            let row: AssetRow = sqlx::query_as(
                "UPDATE release_assets SET size = $2, storage_key = $3, digest = $4, state = 'uploaded', updated_at = now()
                  WHERE id = $1 RETURNING *",
            )
            .bind(row.id)
            .bind(size)
            .bind(&key)
            .bind(&digest)
            .fetch_one(&s.db)
            .await?;
            Ok(row)
        }
        Err(e) => {
            sqlx::query("DELETE FROM release_assets WHERE id = $1").bind(row.id).execute(&s.db).await?;
            let _ = s.store.delete(&ObjPath::from(key)).await;
            Err(ApiError::bad_request(format!("{e:#}")))
        }
    }
}

pub async fn load_asset(db: &sqlx::PgPool, ctx: &RepoCtx, asset_id: i64) -> ApiResult<(AssetRow, ReleaseRow)> {
    let a: AssetRow = sqlx::query_as(
        "SELECT a.* FROM release_assets a JOIN releases r ON r.id = a.release_id WHERE a.id = $1 AND r.repo_id = $2",
    )
    .bind(asset_id)
    .bind(ctx.repo.id)
    .fetch_optional(db)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let r = load_release(db, ctx, a.release_id).await?;
    Ok((a, r))
}

async fn asset_json(s: &AppState, ctx: &RepoCtx, a: &AssetRow, r: &ReleaseRow) -> ApiResult<Value> {
    let uploader = match a.uploader_id {
        Some(id) => db::user_by_id(&s.db, id).await.ok(),
        None => None,
    };
    Ok(crate::json::release::asset(&s.api_url(), s.base_url(), ctx, &r.tag_name, a, uploader.as_ref()))
}

/// Streams an asset's bytes from object storage and counts the download.
pub async fn stream_asset(s: &AppState, a: &AssetRow, content_type: &str) -> ApiResult<Response> {
    let got = s.store.get(&ObjPath::from(a.storage_key.clone())).await.map_err(|e| match e {
        object_store::Error::NotFound { .. } => ApiError::not_found(),
        e => ApiError::internal(e),
    })?;
    sqlx::query("UPDATE release_assets SET download_count = download_count + 1 WHERE id = $1")
        .bind(a.id)
        .execute(&s.db)
        .await?;
    let stream = got.into_stream();
    let mut res = Response::new(Body::from_stream(stream));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(a.size as u64));
    let disp = format!("attachment; filename={}", crate::json::release::seg(&a.name));
    if let Ok(v) = HeaderValue::from_str(&disp) {
        h.insert(header::CONTENT_DISPOSITION, v);
    } else {
        h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    }
    // The uploader chooses the content type (e.g. text/html): never let it render on our origin.
    crate::security::harden_user_content(&mut res);
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// Handlers

async fn ctx_for(s: &AppState, u: Option<&User>, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    db::load_repo(&s.db, u, owner, repo).await
}

async fn list(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let (rows, total) = list_releases(&s.db, ctx.repo.id, ctx.perm >= Perm::Write, p).await?;
    let items = releases_json(&s, &ctx, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(input): Json<CreateRelease>,
) -> ApiResult<Response> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    let r = create_release(&s, &ctx, &u, input).await?;
    Ok(created(release_json(&s, &ctx, &r).await?))
}

async fn get_one(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let r = load_release(&s.db, &ctx, id).await?;
    Ok(Json(release_json(&s, &ctx, &r).await?))
}

async fn latest(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let r = latest_release(&s.db, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(release_json(&s, &ctx, &r).await?))
}

async fn by_tag(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, tag)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let r = release_by_tag(&s.db, &ctx, &tag, ctx.perm >= Perm::Write).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(release_json(&s, &ctx, &r).await?))
}

async fn update(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(input): Json<UpdateRelease>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    let r = update_release(&s, &ctx, &u, id, input).await?;
    Ok(Json(release_json(&s, &ctx, &r).await?))
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    delete_release(&s, &ctx, &u, id).await?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct NotesReq {
    tag_name: Option<String>,
    target_commitish: Option<String>,
    previous_tag_name: Option<String>,
    #[allow(dead_code)]
    configuration_file_path: Option<String>,
}

async fn generate_notes_handler(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<NotesReq>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Write)?;
    let tag = req.tag_name.filter(|t| !t.is_empty()).ok_or_else(|| ApiError::validation("Release", "tag_name", "missing_field"))?;
    let (name, body) = generate_notes(
        &s,
        &ctx,
        &tag,
        req.target_commitish.as_deref().filter(|t| !t.is_empty()),
        req.previous_tag_name.as_deref().filter(|t| !t.is_empty()),
    )
    .await?;
    Ok(Json(json!({ "name": name, "body": body })))
}

async fn list_assets(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let r = load_release(&s.db, &ctx, id).await?;
    let rows: Vec<AssetRow> = sqlx::query_as("SELECT * FROM release_assets WHERE release_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM release_assets WHERE release_id = $1").bind(id).fetch_one(&s.db).await?;
    let ids: Vec<i64> = rows.iter().filter_map(|a| a.uploader_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let items = rows
        .iter()
        .map(|a| crate::json::release::asset(&s.api_url(), s.base_url(), &ctx, &r.tag_name, a, a.uploader_id.and_then(|i| users.get(&i))))
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize)]
struct UploadQuery {
    name: Option<String>,
    label: Option<String>,
}

async fn upload(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Query(q): Query<UploadQuery>,
    req: Request,
) -> ApiResult<Response> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Write)?;
    let r = load_release(&s.db, &ctx, id).await?;
    let name = q.name.filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("ReleaseAsset", "name", "missing_field"))?;
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let a = upload_asset(&s, &ctx, &u, &r, &name, q.label, &content_type, req.into_body()).await?;
    Ok(created(crate::json::release::asset(&s.api_url(), s.base_url(), &ctx, &r.tag_name, &a, Some(&u))))
}

fn wants_octet(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/octet-stream"))
}

async fn get_asset(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    headers: HeaderMap,
    Path((owner, repo, asset_id)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let (a, r) = load_asset(&s.db, &ctx, asset_id).await?;
    if wants_octet(&headers) {
        return stream_asset(&s, &a, "application/octet-stream").await;
    }
    Ok(axum::Json(asset_json(&s, &ctx, &a, &r).await?).into_response())
}

#[derive(Deserialize)]
struct UpdateAsset {
    name: Option<String>,
    label: Option<String>,
    state: Option<String>,
}

async fn update_asset(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, asset_id)): Path<(String, String, i64)>,
    Json(req): Json<UpdateAsset>,
) -> ApiResult<Json<Value>> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Write)?;
    let (a, r) = load_asset(&s.db, &ctx, asset_id).await?;
    let name = req.name.as_deref().map(sanitize_asset_name).filter(|n| !n.is_empty());
    let a: AssetRow = sqlx::query_as(
        "UPDATE release_assets SET name = COALESCE($2, name), label = COALESCE($3, label),
            state = COALESCE($4, state), updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(a.id)
    .bind(name)
    .bind(req.label)
    .bind(req.state.filter(|st| st == "uploaded" || st == "open"))
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("ReleaseAsset", "name", "already_exists"),
        e => e.into(),
    })?;
    Ok(Json(asset_json(&s, &ctx, &a, &r).await?))
}

async fn delete_asset(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, asset_id)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Write)?;
    let (a, _) = load_asset(&s.db, &ctx, asset_id).await?;
    sqlx::query("DELETE FROM release_assets WHERE id = $1").bind(a.id).execute(&s.db).await?;
    let _ = s.store.delete(&ObjPath::from(a.storage_key)).await;
    Ok(no_content())
}

async fn download_browser(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, rest)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let (tag, name) = rest.rsplit_once('/').ok_or_else(ApiError::not_found)?;
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let r = release_by_tag(&s.db, &ctx, tag, ctx.perm >= Perm::Write).await?.ok_or_else(ApiError::not_found)?;
    download_named(&s, &r, name).await
}

async fn download_latest(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, name)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    let r = latest_release(&s.db, ctx.repo.id).await?.ok_or_else(ApiError::not_found)?;
    download_named(&s, &r, &name).await
}

async fn download_named(s: &AppState, r: &ReleaseRow, name: &str) -> ApiResult<Response> {
    let a: AssetRow = sqlx::query_as("SELECT * FROM release_assets WHERE release_id = $1 AND name = $2 AND state = 'uploaded'")
        .bind(r.id)
        .bind(name)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let ct = a.content_type.clone();
    stream_asset(s, &a, &ct).await
}

async fn list_reactions(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Query(q): Query<ReactionQuery>,
) -> ApiResult<Paged> {
    let ctx = ctx_for(&s, u.as_ref(), &owner, &repo).await?;
    load_release(&s.db, &ctx, id).await?;
    let items = plat::list_reactions(&s, "release", id, &q).await?;
    Ok(Paged::new(&uri, &s, PageParams { page: q.page, per_page: q.per_page }, None, items))
}

async fn create_reaction(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(req): Json<CreateReaction>,
) -> ApiResult<Response> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    load_release(&s.db, &ctx, id).await?;
    plat::create_reaction(&s, "release", id, &u, &req.content, &RELEASE_REACTIONS).await
}

async fn delete_reaction(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id, reaction_id)): Path<(String, String, i64, i64)>,
) -> ApiResult<Response> {
    let ctx = ctx_for(&s, Some(&u), &owner, &repo).await?;
    load_release(&s.db, &ctx, id).await?;
    plat::delete_reaction(&s, &ctx, "release", id, reaction_id, &u).await
}
