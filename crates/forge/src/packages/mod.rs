//! Packages (GitHub Packages): the OCI container registry (`oci`, served at `/v2/`), the npm
//! registry (`npm`, served at `/npm/`), and the service layer the REST packages API
//! (`api::packages`) renders.
//!
//! Storage: every package object (image layer, image config, manifest, npm tarball) is a blob in
//! object storage at `packages/blobs/sha256/<hex>`, content-addressed and shared between
//! packages. Postgres holds packages, versions, tags and the blob references; any pod can serve
//! any request. `forge gc` (`gc::run`) purges packages deleted more than 30 days ago and deletes
//! blobs nothing references.
//!
//! Permissions follow GitHub's granular package permissions: the owning user, the owners of the
//! owning organization and the publisher administer a package; a package linked to a repository
//! (`org.opencontainers.image.source`, package.json `repository`) inherits that repository's
//! access (read -> read, write/maintain -> write, admin -> admin); public packages are readable by
//! everyone. New packages are private unless they are linked to a public repository.
pub mod gc;
pub mod npm;
pub mod oci;

use axum::Router;
use bytes::Bytes;
use chrono::{DateTime, Utc};
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use object_store::path::Path as ObjPath;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Registry routes (`/v2/...` and `/npm/...`), merged at the top level of the app.
pub fn routes() -> Router<AppState> {
    Router::new().merge(oci::routes()).merge(npm::routes())
}

/// How long deleted packages and versions can be restored before `forge gc` purges them.
pub const RESTORE_DAYS: i64 = 30;

pub const TYPES: &[&str] = &["npm", "maven", "rubygems", "docker", "nuget", "container"];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PackageRow {
    pub id: i64,
    pub owner_id: i64,
    pub package_type: String,
    pub name: String,
    pub visibility: String,
    pub repo_id: Option<i64>,
    pub inherit_access: bool,
    pub creator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub deleted_by: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VersionRow {
    pub id: i64,
    pub package_id: i64,
    pub name: String,
    pub media_type: String,
    pub digest: String,
    pub size: i64,
    pub metadata: Value,
    pub description: Option<String>,
    pub license: Option<String>,
    pub creator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub deleted_by: Option<i64>,
}

/// A caller's permission on a package, ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PkgPerm {
    None,
    Read,
    Write,
    Admin,
}

// ---------------------------------------------------------------------------------------------
// Blob store

/// `sha256:<64 hex>` -> the hex part.
pub fn digest_hex(d: &str) -> Option<&str> {
    let hex = d.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then_some(hex)
}

pub fn blob_key(digest: &str) -> ObjPath {
    ObjPath::from(format!("packages/blobs/sha256/{}", digest.trim_start_matches("sha256:")))
}

pub fn sha256_digest(data: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(data)))
}

/// Size of a blob if it is in the store.
pub async fn blob_size(state: &AppState, digest: &str) -> ApiResult<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT size FROM package_blobs WHERE digest = $1").bind(digest).fetch_optional(&state.db).await?)
}

/// Whether the blob is recorded *and* its object exists (a row whose object went missing, e.g.
/// restored from a database backup without the bucket, is re-filled by the next upload).
pub async fn blob_stored(state: &AppState, digest: &str) -> ApiResult<bool> {
    if blob_size(state, digest).await?.is_none() {
        return Ok(false);
    }
    match state.store.head(&blob_key(digest)).await {
        Ok(_) => Ok(true),
        Err(object_store::Error::NotFound { .. }) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Marks blobs as recently used (protects them from `forge gc` while a push is in flight).
pub async fn touch_blobs(state: &AppState, digests: &[String]) -> ApiResult<()> {
    sqlx::query("UPDATE package_blobs SET last_seen_at = now() WHERE digest = ANY($1) AND last_seen_at < now() - interval '1 minute'")
        .bind(digests)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Stores a small object (a manifest, an npm tarball) in the blob store; returns its digest.
pub async fn put_blob_bytes(state: &AppState, data: Bytes) -> ApiResult<String> {
    let digest = sha256_digest(&data);
    if !blob_stored(state, &digest).await? {
        let len = data.len() as i64;
        state.store.put(&blob_key(&digest), data.into()).await?;
        sqlx::query("INSERT INTO package_blobs (digest, size) VALUES ($1, $2) ON CONFLICT (digest) DO UPDATE SET last_seen_at = now()")
            .bind(&digest)
            .bind(len)
            .execute(&state.db)
            .await?;
    } else {
        touch_blobs(state, std::slice::from_ref(&digest)).await?;
    }
    Ok(digest)
}

/// Reads a whole (small) blob.
pub async fn read_blob(state: &AppState, digest: &str) -> ApiResult<Bytes> {
    let got = state.store.get(&blob_key(digest)).await.map_err(|e| match e {
        object_store::Error::NotFound { .. } => ApiError::not_found(),
        e => ApiError::internal(e),
    })?;
    Ok(got.bytes().await?)
}

// ---------------------------------------------------------------------------------------------
// Lookups and permissions

pub async fn package_by_id(state: &AppState, id: i64) -> ApiResult<Option<PackageRow>> {
    Ok(sqlx::query_as("SELECT * FROM packages WHERE id = $1").bind(id).fetch_optional(&state.db).await?)
}

/// The active package `name` of `package_type` in `owner_id`'s namespace (case-insensitive).
pub async fn find_active(state: &AppState, owner_id: i64, package_type: &str, name: &str) -> ApiResult<Option<PackageRow>> {
    Ok(sqlx::query_as("SELECT * FROM packages WHERE owner_id = $1 AND package_type = $2 AND lower(name) = lower($3) AND deleted_at IS NULL")
        .bind(owner_id)
        .bind(package_type)
        .bind(name)
        .fetch_optional(&state.db)
        .await?)
}

/// Whether `actor` may publish new packages in `owner`'s namespace: the user themselves, or a
/// member of the organization.
pub async fn can_create(state: &AppState, actor: &User, owner: &User) -> ApiResult<bool> {
    if actor.site_admin || actor.id == owner.id {
        return Ok(true);
    }
    if owner.kind == "Organization" {
        return crate::orgs::is_member(&state.db, owner, actor).await;
    }
    Ok(false)
}

/// Namespace-level administration: the user, or an owner of the organization.
pub async fn owner_admin(state: &AppState, actor: Option<&User>, owner: &User) -> ApiResult<bool> {
    let Some(a) = actor else { return Ok(false) };
    if a.site_admin || a.id == owner.id {
        return Ok(true);
    }
    if owner.kind == "Organization" {
        return crate::orgs::is_owner(&state.db, owner, a).await;
    }
    Ok(false)
}

/// The linked repository, resolved for `viewer`.
pub async fn linked_repo(state: &AppState, viewer: Option<&User>, pkg: &PackageRow) -> ApiResult<Option<RepoCtx>> {
    let Some(rid) = pkg.repo_id else { return Ok(None) };
    let Ok(repo) = db::repo_by_id(&state.db, rid).await else { return Ok(None) };
    let owner = db::user_by_id(&state.db, repo.owner_id).await?;
    let perm = db::permission(&state.db, viewer, &repo, &owner).await?;
    Ok(Some(RepoCtx { repo, owner, perm }))
}

pub async fn package_perm(state: &AppState, viewer: Option<&User>, pkg: &PackageRow, owner: &User) -> ApiResult<PkgPerm> {
    let mut best = if pkg.visibility == "public" { PkgPerm::Read } else { PkgPerm::None };
    let Some(v) = viewer else { return Ok(best) };
    if owner_admin(state, Some(v), owner).await? || pkg.creator_id == Some(v.id) {
        return Ok(PkgPerm::Admin);
    }
    if pkg.visibility == "internal" && owner.kind == "Organization" && crate::orgs::is_member(&state.db, owner, v).await? {
        best = best.max(PkgPerm::Read);
    }
    if pkg.inherit_access {
        if let Some(ctx) = linked_repo(state, Some(v), pkg).await? {
            best = best.max(match ctx.perm {
                Perm::Admin => PkgPerm::Admin,
                Perm::Maintain | Perm::Write => PkgPerm::Write,
                Perm::Read | Perm::Triage => PkgPerm::Read,
                Perm::None => PkgPerm::None,
            });
        }
    }
    Ok(best)
}

/// A REST-style permission check: 404 when the caller can't see the package, 403 when they can
/// see it but lack `need`.
pub fn require(perm: PkgPerm, need: PkgPerm) -> ApiResult<()> {
    if perm >= need {
        Ok(())
    } else if perm >= PkgPerm::Read {
        Err(ApiError::forbidden(match need {
            PkgPerm::Admin => "You must have admin permissions on this package.",
            _ => "Permission to write to this package is required.",
        }))
    } else {
        Err(ApiError::not_found())
    }
}

/// Finds or creates the active package `name` in `owner`'s namespace for a publish by `actor`.
/// Returns the package and whether it was created. The caller checks `can_create` first.
pub async fn ensure_package(state: &AppState, actor: &User, owner: &User, package_type: &str, name: &str) -> ApiResult<(PackageRow, bool)> {
    let created: Option<PackageRow> = sqlx::query_as(
        "INSERT INTO packages (owner_id, package_type, name, creator_id) VALUES ($1, $2, $3, $4)
         ON CONFLICT (owner_id, package_type, lower(name)) WHERE deleted_at IS NULL DO NOTHING RETURNING *",
    )
    .bind(owner.id)
    .bind(package_type)
    .bind(name)
    .bind(actor.id)
    .fetch_optional(&state.db)
    .await?;
    if let Some(p) = created {
        return Ok((p, true));
    }
    let p = find_active(state, owner.id, package_type, name).await?.ok_or_else(|| ApiError::conflict("package is being modified concurrently"))?;
    Ok((p, false))
}

/// Parses a repository URL (`https://host/owner/repo(.git)`, `git+https://...`, `git@host:o/r`)
/// pointing at this forge into `(owner, repo)`.
pub fn parse_repo_url(base: &str, url: &str) -> Option<(String, String)> {
    let host = crate::json::repo::host_of(base);
    let u = url.trim().trim_start_matches("git+");
    let path = if let Some(rest) = u.strip_prefix("git@") {
        let (h, p) = rest.split_once(':')?;
        if !h.eq_ignore_ascii_case(&host) {
            return None;
        }
        p.to_string()
    } else {
        let parsed = url::Url::parse(u).ok()?;
        if !parsed.host_str()?.eq_ignore_ascii_case(&host) {
            return None;
        }
        parsed.path().to_string()
    };
    let mut segs = path.trim_matches('/').split('/');
    let owner = segs.next()?.to_string();
    let repo = segs.next()?.trim_end_matches(".git").to_string();
    (!owner.is_empty() && !repo.is_empty()).then_some((owner, repo))
}

/// Links `pkg` to the repository named by `source_url` when it belongs to the package's owner and
/// `actor` can write to it (GitHub links only repositories in the same namespace). A package that
/// was just created takes the repository's visibility.
pub async fn link_source(state: &AppState, actor: &User, pkg: &PackageRow, owner: &User, source_url: &str, just_created: bool) -> ApiResult<PackageRow> {
    let Some((o, r)) = parse_repo_url(state.base_url(), source_url) else { return Ok(pkg.clone()) };
    if !o.eq_ignore_ascii_case(&owner.login) {
        return Ok(pkg.clone());
    }
    let Ok(ctx) = db::load_repo(&state.db, Some(actor), &o, &r).await else { return Ok(pkg.clone()) };
    if ctx.perm < Perm::Write || ctx.owner.id != owner.id || pkg.repo_id == Some(ctx.repo.id) {
        return Ok(pkg.clone());
    }
    let visibility = if just_created && !ctx.repo.private { Some("public") } else { None };
    Ok(sqlx::query_as("UPDATE packages SET repo_id = $2, visibility = COALESCE($3, visibility), updated_at = now() WHERE id = $1 RETURNING *")
        .bind(pkg.id)
        .bind(ctx.repo.id)
        .bind(visibility)
        .fetch_one(&state.db)
        .await?)
}

// ---------------------------------------------------------------------------------------------
// Service layer (REST packages API; registries call these too)

/// Packages of `owner` the viewer can read. `deleted` lists deleted (restorable) ones instead.
pub async fn list_for_owner(
    state: &AppState,
    viewer: Option<&User>,
    owner: &User,
    package_type: &str,
    visibility: Option<&str>,
) -> ApiResult<Vec<PackageRow>> {
    let rows: Vec<PackageRow> = sqlx::query_as(
        "SELECT * FROM packages WHERE owner_id = $1 AND package_type = $2 AND deleted_at IS NULL
           AND ($3::text IS NULL OR visibility = $3) ORDER BY id",
    )
    .bind(owner.id)
    .bind(package_type)
    .bind(visibility)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for p in rows {
        if package_perm(state, viewer, &p, owner).await? >= PkgPerm::Read {
            out.push(p);
        }
    }
    Ok(out)
}

/// The active package, checking the viewer can read it (404 otherwise).
pub async fn get_readable(state: &AppState, viewer: Option<&User>, owner: &User, package_type: &str, name: &str) -> ApiResult<(PackageRow, PkgPerm)> {
    let p = find_active(state, owner.id, package_type, name).await?.ok_or_else(ApiError::not_found)?;
    let perm = package_perm(state, viewer, &p, owner).await?;
    require(perm, PkgPerm::Read)?;
    Ok((p, perm))
}

/// Soft-deletes a package (admin). Its name is free again; it can be restored for 30 days.
pub async fn delete_package(state: &AppState, actor: &User, owner: &User, package_type: &str, name: &str) -> ApiResult<PackageRow> {
    let (p, perm) = get_readable(state, Some(actor), owner, package_type, name).await?;
    require(perm, PkgPerm::Admin)?;
    Ok(sqlx::query_as("UPDATE packages SET deleted_at = now(), deleted_by = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(p.id)
        .bind(actor.id)
        .fetch_one(&state.db)
        .await?)
}

/// Restores the most recently deleted package `name` (admin of the namespace or the package).
pub async fn restore_package(state: &AppState, actor: &User, owner: &User, package_type: &str, name: &str) -> ApiResult<PackageRow> {
    let p: PackageRow = sqlx::query_as(
        "SELECT * FROM packages WHERE owner_id = $1 AND package_type = $2 AND lower(name) = lower($3) AND deleted_at IS NOT NULL
           AND deleted_at > now() - make_interval(days => $4::int) ORDER BY deleted_at DESC LIMIT 1",
    )
    .bind(owner.id)
    .bind(package_type)
    .bind(name)
    .bind(RESTORE_DAYS as i32)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let perm = package_perm(state, Some(actor), &p, owner).await?;
    require(perm, PkgPerm::Admin)?;
    if find_active(state, owner.id, package_type, &p.name).await?.is_some() {
        return Err(ApiError::forbidden("The package namespace is in use by another package. Delete it first to restore this one."));
    }
    sqlx::query_as("UPDATE packages SET deleted_at = NULL, deleted_by = NULL, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(p.id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::forbidden("The package namespace is in use by another package."),
            e => e.into(),
        })
}

/// Versions of a package: active ones, or (`deleted`) the restorable deleted ones. Newest first.
pub async fn versions(state: &AppState, package_id: i64, deleted: bool) -> ApiResult<Vec<VersionRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM package_versions WHERE package_id = $1 AND ((NOT $2 AND deleted_at IS NULL)
            OR ($2 AND deleted_at IS NOT NULL AND deleted_at > now() - make_interval(days => $3::int))) ORDER BY id DESC",
    )
    .bind(package_id)
    .bind(deleted)
    .bind(RESTORE_DAYS as i32)
    .fetch_all(&state.db)
    .await?)
}

pub async fn version_by_id(state: &AppState, package_id: i64, id: i64) -> ApiResult<VersionRow> {
    sqlx::query_as("SELECT * FROM package_versions WHERE package_id = $1 AND id = $2")
        .bind(package_id)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Soft-deletes a version (admin).
pub async fn delete_version(state: &AppState, actor: &User, pkg: &PackageRow, perm: PkgPerm, version_id: i64) -> ApiResult<VersionRow> {
    require(perm, PkgPerm::Admin)?;
    let v = version_by_id(state, pkg.id, version_id).await?;
    if v.deleted_at.is_some() {
        return Err(ApiError::not_found());
    }
    let v = sqlx::query_as("UPDATE package_versions SET deleted_at = now(), deleted_by = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(v.id)
        .bind(actor.id)
        .fetch_one(&state.db)
        .await?;
    sqlx::query("UPDATE packages SET updated_at = now() WHERE id = $1").bind(pkg.id).execute(&state.db).await?;
    Ok(v)
}

/// Restores a deleted version (admin, within 30 days).
pub async fn restore_version(state: &AppState, pkg: &PackageRow, perm: PkgPerm, version_id: i64) -> ApiResult<VersionRow> {
    require(perm, PkgPerm::Admin)?;
    let v = version_by_id(state, pkg.id, version_id).await?;
    match v.deleted_at {
        Some(t) if t > Utc::now() - chrono::Duration::days(RESTORE_DAYS) => {}
        _ => return Err(ApiError::not_found()),
    }
    let v = sqlx::query_as("UPDATE package_versions SET deleted_at = NULL, deleted_by = NULL, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(v.id)
        .fetch_one(&state.db)
        .await?;
    sqlx::query("UPDATE packages SET updated_at = now() WHERE id = $1").bind(pkg.id).execute(&state.db).await?;
    Ok(v)
}

/// Number of active versions per package.
pub async fn version_counts(state: &AppState, ids: &[i64]) -> ApiResult<std::collections::HashMap<i64, i64>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT package_id, count(*) FROM package_versions WHERE package_id = ANY($1) AND deleted_at IS NULL GROUP BY package_id",
    )
    .bind(ids)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Tags pointing at each version.
pub async fn tags_by_version(state: &AppState, version_ids: &[i64]) -> ApiResult<std::collections::HashMap<i64, Vec<String>>> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT version_id, tag FROM package_tags WHERE version_id = ANY($1) ORDER BY tag")
        .bind(version_ids)
        .fetch_all(&state.db)
        .await?;
    let mut out: std::collections::HashMap<i64, Vec<String>> = Default::default();
    for (v, t) in rows {
        out.entry(v).or_default().push(t);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// JSON

const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

pub fn enc(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT).to_string()
}

fn namespace(owner: &User) -> &'static str {
    if owner.kind == "Organization" { "orgs" } else { "users" }
}

pub fn package_url(state: &AppState, owner: &User, p: &PackageRow) -> String {
    format!("{}/{}/{}/packages/{}/{}", state.api_url(), namespace(owner), owner.login, p.package_type, enc(&p.name))
}

pub fn package_html_url(state: &AppState, owner: &User, p: &PackageRow) -> String {
    format!("{}/{}/{}/packages/{}/package/{}", state.base_url(), namespace(owner), owner.login, p.package_type, enc(&p.name))
}

/// `package` schema.
pub fn package_json(state: &AppState, owner: &User, p: &PackageRow, version_count: i64, repo: Option<&RepoCtx>) -> Value {
    let api = state.api_url();
    json!({
        "id": p.id,
        "name": p.name,
        "package_type": p.package_type,
        "owner": crate::json::user::simple(&api, state.base_url(), owner),
        "version_count": version_count,
        "visibility": if p.visibility == "public" { "public" } else { "private" },
        "url": package_url(state, owner, p),
        "html_url": package_html_url(state, owner, p),
        "repository": repo.filter(|c| c.perm >= Perm::Read).map(|c| crate::json::repo::repo(&api, state.base_url(), c)).unwrap_or(Value::Null),
        "created_at": crate::json::ts(p.created_at),
        "updated_at": crate::json::ts(p.updated_at),
    })
}

/// `package-version` schema.
pub fn version_json(state: &AppState, owner: &User, p: &PackageRow, v: &VersionRow, tags: &[String]) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), json!(v.id));
    o.insert("name".into(), json!(v.name));
    o.insert("url".into(), json!(format!("{}/versions/{}", package_url(state, owner, p), v.id)));
    o.insert("package_html_url".into(), json!(package_html_url(state, owner, p)));
    o.insert(
        "html_url".into(),
        json!(format!("{}/{}/{}/packages/{}/{}/{}", state.base_url(), namespace(owner), owner.login, p.package_type, enc(&p.name), v.id)),
    );
    o.insert("created_at".into(), json!(crate::json::ts(v.created_at)));
    o.insert("updated_at".into(), json!(crate::json::ts(v.updated_at)));
    if let Some(d) = v.deleted_at {
        o.insert("deleted_at".into(), json!(crate::json::ts(d)));
    }
    if let Some(l) = &v.license {
        o.insert("license".into(), json!(l));
    }
    if let Some(d) = &v.description {
        o.insert("description".into(), json!(d));
    }
    let mut meta = json!({ "package_type": p.package_type });
    if p.package_type == "container" || p.package_type == "docker" {
        meta["container"] = json!({ "tags": tags });
    }
    o.insert("metadata".into(), meta);
    Value::Object(o)
}

/// Renders packages (batch-loading version counts and linked repositories).
pub async fn packages_json(state: &AppState, viewer: Option<&User>, owner: &User, rows: &[PackageRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let counts = version_counts(state, &ids).await?;
    let mut repo_ids: Vec<i64> = rows.iter().filter_map(|p| p.repo_id).collect();
    repo_ids.sort_unstable();
    repo_ids.dedup();
    let ctxs = crate::api::issues::repo_ctxs(state, viewer, &repo_ids).await?;
    Ok(rows
        .iter()
        .map(|p| package_json(state, owner, p, counts.get(&p.id).copied().unwrap_or(0), p.repo_id.and_then(|r| ctxs.get(&r))))
        .collect())
}
