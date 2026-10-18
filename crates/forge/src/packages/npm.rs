//! npm registry (GitHub's `npm.pkg.github.com`), served at `{base}/npm/`. Packages are scoped
//! to their owner: `@owner/name` (the REST API calls it `name`). Clients configure
//!
//! ```text
//! @owner:registry={base}/npm/
//! //{host}/npm/:_authToken=<PAT>
//! ```
//!
//! Supported: `npm publish` (tarballs go to the blob store), `npm install`/`view` (packuments and
//! tarballs), `npm dist-tag`, `npm unpublish` (soft deletes, restorable through the REST API),
//! `npm whoami` and `npm login` (legacy auth: exchanges login + password for a PAT).
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use base64::Engine;
use bytes::Bytes;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use serde_json::{json, Map, Value};
use sha1::Sha1;
use sha2::{Digest, Sha512};

use super::{PackageRow, PkgPerm, VersionRow};
use crate::db::{self, User};
use crate::error::ApiError;
use crate::state::AppState;

pub const PREFIX: &str = "/npm";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PREFIX, any(dispatch))
        .route(&format!("{PREFIX}/"), any(dispatch))
        .route(&format!("{PREFIX}/{{*rest}}"), any(dispatch))
        .layer(axum::extract::DefaultBodyLimit::disable())
}

fn max_publish_bytes() -> usize {
    std::env::var("FORGE_NPM_MAX_PUBLISH_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(512 << 20)
}

/// The registry URL clients put in `.npmrc`.
pub fn registry_url(state: &AppState) -> String {
    format!("{}{PREFIX}/", state.base_url())
}

struct NpmErr(StatusCode, String);

type NpmResult<T> = Result<T, NpmErr>;

impl IntoResponse for NpmErr {
    fn into_response(self) -> Response {
        let mut res = (self.0, axum::Json(json!({ "error": self.1, "reason": self.1 }))).into_response();
        if self.0 == StatusCode::UNAUTHORIZED {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"npm\""));
        }
        res
    }
}

impl From<ApiError> for NpmErr {
    fn from(e: ApiError) -> Self {
        let msg = if e.status.is_server_error() { "internal server error".to_string() } else { e.message.to_lowercase() };
        NpmErr(e.status, msg)
    }
}

impl From<sqlx::Error> for NpmErr {
    fn from(e: sqlx::Error) -> Self {
        ApiError::from(e).into()
    }
}

fn err(status: StatusCode, msg: impl Into<String>) -> NpmErr {
    NpmErr(status, msg.into())
}

fn not_found() -> NpmErr {
    err(StatusCode::NOT_FOUND, "not found")
}

async fn dispatch(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let raw = parts.uri.path().strip_prefix(PREFIX).unwrap_or("").trim_start_matches('/').to_string();
    let path = percent_encoding::percent_decode_str(&raw).decode_utf8_lossy().to_string();
    let user = match super::oci::registry_user(&state, &mut parts).await {
        Ok(u) => u,
        Err(()) => return err(StatusCode::UNAUTHORIZED, "bad credentials").into_response(),
    };
    let m = parts.method.clone();
    let res = route(&state, user.as_ref(), &m, &path, body).await;
    res.unwrap_or_else(|e| e.into_response())
}

async fn route(state: &AppState, user: Option<&User>, m: &Method, path: &str, body: Body) -> NpmResult<Response> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [] | ["-", "ping"] => Ok(ok_json(StatusCode::OK, json!({}))),
        ["-", "whoami"] => {
            let u = user.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "authentication required"))?;
            Ok(ok_json(StatusCode::OK, json!({ "username": u.login })))
        }
        ["-", "user", who] if *m == Method::PUT => login(state, who, body).await,
        ["-", "package", scope, name, "dist-tags"] => dist_tags(state, user, m, scope, name, None, body).await,
        ["-", "package", scope, name, "dist-tags", tag] => dist_tags(state, user, m, scope, name, Some(tag), body).await,
        [scope, name] if scope.starts_with('@') => match *m {
            Method::GET | Method::HEAD => packument(state, user, scope, name).await,
            Method::PUT => publish(state, user, scope, name, body).await,
            _ => Err(err(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")),
        },
        [scope, name, "-rev", _rev] if scope.starts_with('@') => match *m {
            Method::PUT => unpublish_versions(state, user, scope, name, body).await,
            Method::DELETE => unpublish_all(state, user, scope, name).await,
            _ => Err(err(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")),
        },
        [scope, name, "-", file] if scope.starts_with('@') && matches!(*m, Method::GET | Method::HEAD) => tarball(state, user, scope, name, file).await,
        [scope, name, "-", _file, "-rev", _rev] if scope.starts_with('@') && *m == Method::DELETE => {
            // The versions were already removed by the preceding packument PUT; the content is
            // garbage-collected.
            let (_, _, perm) = load(state, user, scope, name).await?;
            need(user, perm, PkgPerm::Admin)?;
            Ok(ok_json(StatusCode::OK, json!({ "ok": true })))
        }
        [scope, name, version] if scope.starts_with('@') && matches!(*m, Method::GET | Method::HEAD) => version_doc(state, user, scope, name, version).await,
        [name, ..] if !name.starts_with('@') && !name.starts_with('-') => {
            Err(err(StatusCode::NOT_FOUND, "packages must be scoped to their owner: @owner/name"))
        }
        _ => Err(not_found()),
    }
}

fn ok_json(status: StatusCode, v: Value) -> Response {
    (status, axum::Json(v)).into_response()
}

fn full_name(owner: &User, name: &str) -> String {
    format!("@{}/{}", owner.login.to_lowercase(), name)
}

fn valid_pkg_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 214
        && !n.starts_with('.')
        && !n.starts_with('_')
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

async fn owner_of(state: &AppState, scope: &str) -> NpmResult<User> {
    db::user_by_login(&state.db, scope.trim_start_matches('@')).await.map_err(|_| not_found())
}

/// The active package and the caller's permission (404 when invisible, 401 when anonymous).
async fn load(state: &AppState, user: Option<&User>, scope: &str, name: &str) -> NpmResult<(User, PackageRow, PkgPerm)> {
    let owner = owner_of(state, scope).await?;
    let pkg = super::find_active(state, owner.id, "npm", name).await?.ok_or_else(not_found)?;
    let perm = super::package_perm(state, user, &pkg, &owner).await?;
    if perm < PkgPerm::Read {
        return Err(if user.is_none() { err(StatusCode::UNAUTHORIZED, "authentication required") } else { not_found() });
    }
    Ok((owner, pkg, perm))
}

fn need(user: Option<&User>, perm: PkgPerm, want: PkgPerm) -> NpmResult<()> {
    if perm >= want {
        return Ok(());
    }
    if user.is_none() {
        return Err(err(StatusCode::UNAUTHORIZED, "authentication required"));
    }
    Err(err(StatusCode::FORBIDDEN, if want == PkgPerm::Admin { "admin permission on the package is required" } else { "write permission on the package is required" }))
}

fn tarball_url(state: &AppState, owner: &User, name: &str, version: &str) -> String {
    format!("{}{PREFIX}/@{}/{name}/-/{name}-{version}.tgz", state.base_url(), owner.login.to_lowercase())
}

fn rev(pkg: &PackageRow, n: usize) -> String {
    format!("{}-{:x}", n + 1, pkg.updated_at.timestamp_micros())
}

/// The version document as served (tarball URL for this host).
fn render_version(state: &AppState, owner: &User, name: &str, v: &VersionRow) -> Value {
    let mut doc = v.metadata.clone();
    doc["name"] = json!(full_name(owner, name));
    doc["version"] = json!(v.name);
    doc["_id"] = json!(format!("{}@{}", full_name(owner, name), v.name));
    doc["dist"]["tarball"] = json!(tarball_url(state, owner, name, &v.name));
    doc
}

async fn dist_tag_map(state: &AppState, pkg: &PackageRow) -> NpmResult<Map<String, Value>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT t.tag, v.name FROM package_tags t JOIN package_versions v ON v.id = t.version_id
          WHERE t.package_id = $1 AND v.deleted_at IS NULL ORDER BY t.tag",
    )
    .bind(pkg.id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|(t, v)| (t, json!(v))).collect())
}

async fn packument(state: &AppState, user: Option<&User>, scope: &str, name: &str) -> NpmResult<Response> {
    let (owner, pkg, _) = load(state, user, scope, name).await?;
    let mut versions = super::versions(state, pkg.id, false).await?;
    versions.reverse(); // oldest first
    let mut tags = dist_tag_map(state, &pkg).await?;
    if !tags.contains_key("latest") {
        if let Some(v) = versions.last() {
            tags.insert("latest".into(), json!(v.name));
        }
    }
    let mut vmap = Map::new();
    let mut time = Map::new();
    time.insert("created".into(), json!(crate::json::ts(pkg.created_at)));
    time.insert("modified".into(), json!(crate::json::ts(pkg.updated_at)));
    for v in &versions {
        vmap.insert(v.name.clone(), render_version(state, &owner, name, v));
        time.insert(v.name.clone(), json!(crate::json::ts(v.created_at)));
    }
    let latest = tags.get("latest").and_then(|l| l.as_str()).and_then(|l| versions.iter().find(|v| v.name == l));
    let mut doc = json!({
        "_id": full_name(&owner, name),
        "_rev": rev(&pkg, versions.len()),
        "name": full_name(&owner, name),
        "dist-tags": tags,
        "versions": vmap,
        "time": time,
    });
    if let Some(l) = latest {
        for k in ["description", "readme", "license", "repository", "homepage", "keywords", "author", "bugs"] {
            if let Some(x) = l.metadata.get(k) {
                doc[k] = x.clone();
            }
        }
    }
    Ok(ok_json(StatusCode::OK, doc))
}

async fn version_doc(state: &AppState, user: Option<&User>, scope: &str, name: &str, version: &str) -> NpmResult<Response> {
    let (owner, pkg, _) = load(state, user, scope, name).await?;
    let version = if super::versions(state, pkg.id, false).await?.iter().any(|v| v.name == version) {
        version.to_string()
    } else {
        dist_tag_map(state, &pkg).await?.get(version).and_then(|v| v.as_str()).map(str::to_string).ok_or_else(not_found)?
    };
    let v: VersionRow = sqlx::query_as("SELECT * FROM package_versions WHERE package_id = $1 AND name = $2 AND deleted_at IS NULL")
        .bind(pkg.id)
        .bind(&version)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(not_found)?;
    Ok(ok_json(StatusCode::OK, render_version(state, &owner, name, &v)))
}

async fn tarball(state: &AppState, user: Option<&User>, scope: &str, name: &str, file: &str) -> NpmResult<Response> {
    let (_, pkg, _) = load(state, user, scope, name).await?;
    let version = file
        .strip_prefix(&format!("{name}-"))
        .and_then(|f| f.strip_suffix(".tgz"))
        .ok_or_else(not_found)?;
    let v: VersionRow = sqlx::query_as("SELECT * FROM package_versions WHERE package_id = $1 AND name = $2 AND deleted_at IS NULL")
        .bind(pkg.id)
        .bind(version)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(not_found)?;
    let got = state.store.get(&super::blob_key(&v.digest)).await.map_err(|_| not_found())?;
    let mut res = Response::new(Body::from_stream(got.into_stream()));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(v.size as u64));
    crate::security::harden_user_content(&mut res);
    Ok(res)
}

fn repository_url(doc: &Value) -> Option<String> {
    match &doc["repository"] {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("url").and_then(|u| u.as_str()).map(str::to_string),
        _ => None,
    }
}

fn license_of(doc: &Value) -> Option<String> {
    match &doc["license"] {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("type").and_then(|u| u.as_str()).map(str::to_string),
        _ => None,
    }
}

/// `npm publish`: a packument with the new version(s) and their tarballs as base64 attachments.
async fn publish(state: &AppState, user: Option<&User>, scope: &str, name: &str, body: Body) -> NpmResult<Response> {
    let actor = user.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "authentication required"))?;
    if !valid_pkg_name(name) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid package name"));
    }
    let owner = owner_of(state, scope).await?;
    let existing = super::find_active(state, owner.id, "npm", name).await?;
    match &existing {
        Some(p) => {
            let perm = super::package_perm(state, Some(actor), p, &owner).await?;
            if perm < PkgPerm::Read {
                return Err(not_found());
            }
            need(user, perm, PkgPerm::Write)?;
        }
        None => {
            if !super::can_create(state, actor, &owner).await? {
                return Err(err(StatusCode::FORBIDDEN, format!("{} may not publish packages for {}", actor.login, owner.login)));
            }
        }
    }
    let data = axum::body::to_bytes(body, max_publish_bytes()).await.map_err(|_| err(StatusCode::PAYLOAD_TOO_LARGE, "request too large"))?;
    let doc: Value = serde_json::from_slice(&data).map_err(|_| err(StatusCode::BAD_REQUEST, "invalid JSON"))?;
    let want = full_name(&owner, name);
    if doc["name"].as_str().is_some_and(|n| !n.eq_ignore_ascii_case(&want)) {
        return Err(err(StatusCode::BAD_REQUEST, "package name in the document does not match the URL"));
    }
    let versions = doc["versions"].as_object().cloned().unwrap_or_default();
    if versions.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "no versions to publish"));
    }
    let attachments = doc["_attachments"].as_object().cloned().unwrap_or_default();

    // Validate and store every tarball before touching the database.
    struct NewVersion {
        version: String,
        doc: Value,
        digest: String,
        size: i64,
    }
    let mut new_versions = Vec::new();
    for (ver, vdoc) in &versions {
        if ver.is_empty() || ver.len() > 256 || ver.contains('/') {
            return Err(err(StatusCode::BAD_REQUEST, "invalid version"));
        }
        if let Some(p) = &existing {
            let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM package_versions WHERE package_id = $1 AND name = $2)")
                .bind(p.id)
                .bind(ver)
                .fetch_one(&state.db)
                .await?;
            if taken {
                return Err(err(StatusCode::CONFLICT, format!("cannot publish over the previously published version {ver}")));
            }
        }
        let file = vdoc["dist"]["tarball"].as_str().and_then(|t| t.rsplit('/').next()).map(str::to_string);
        let att = file
            .as_ref()
            .and_then(|f| attachments.iter().find(|(k, _)| k.ends_with(f.as_str())))
            .or_else(|| attachments.iter().find(|(k, _)| k.ends_with(&format!("-{ver}.tgz"))))
            .or_else(|| if attachments.len() == 1 && versions.len() == 1 { attachments.iter().next() } else { None })
            .map(|(_, a)| a.clone())
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("missing tarball attachment for {ver}")))?;
        let tgz = base64::engine::general_purpose::STANDARD
            .decode(att["data"].as_str().unwrap_or("").as_bytes())
            .map_err(|_| err(StatusCode::BAD_REQUEST, "attachment is not base64"))?;
        if att["length"].as_u64().is_some_and(|l| l != tgz.len() as u64) {
            return Err(err(StatusCode::BAD_REQUEST, "attachment length mismatch"));
        }
        let shasum = hex::encode(Sha1::digest(&tgz));
        if vdoc["dist"]["shasum"].as_str().is_some_and(|s| !s.eq_ignore_ascii_case(&shasum)) {
            return Err(err(StatusCode::BAD_REQUEST, "shasum does not match the tarball"));
        }
        let integrity = format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&tgz)));
        if vdoc["dist"]["integrity"].as_str().is_some_and(|i| i.starts_with("sha512-") && i != integrity) {
            return Err(err(StatusCode::BAD_REQUEST, "integrity does not match the tarball"));
        }
        let size = tgz.len() as i64;
        let digest = super::put_blob_bytes(state, Bytes::from(tgz)).await?;
        let mut vd = vdoc.clone();
        if let Some(o) = vd.as_object_mut() {
            o.remove("_attachments");
        }
        vd["dist"]["shasum"] = json!(shasum);
        vd["dist"]["integrity"] = json!(integrity);
        vd["_npmUser"] = json!({ "name": actor.login, "email": actor.email });
        new_versions.push(NewVersion { version: ver.clone(), doc: vd, digest, size });
    }

    let (mut pkg, created) = match existing {
        Some(p) => (p, false),
        None => super::ensure_package(state, actor, &owner, "npm", name).await?,
    };
    if pkg.repo_id.is_none() {
        if let Some(url) = new_versions.iter().find_map(|v| repository_url(&v.doc)).or_else(|| repository_url(&doc)) {
            pkg = super::link_source(state, actor, &pkg, &owner, &url, created).await?;
        }
    }
    let mut tx = state.db.begin().await?;
    for v in &new_versions {
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO package_versions (package_id, name, media_type, digest, size, metadata, description, license, creator_id)
             VALUES ($1, $2, 'application/octet-stream', $3, $4, $5, $6, $7, $8) ON CONFLICT (package_id, name) DO NOTHING RETURNING id",
        )
        .bind(pkg.id)
        .bind(&v.version)
        .bind(&v.digest)
        .bind(v.size)
        .bind(&v.doc)
        .bind(v.doc["description"].as_str())
        .bind(license_of(&v.doc))
        .bind(actor.id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(id) = id else {
            return Err(err(StatusCode::CONFLICT, format!("cannot publish over the previously published version {}", v.version)));
        };
        sqlx::query("INSERT INTO package_version_blobs (version_id, digest) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(id)
            .bind(&v.digest)
            .execute(&mut *tx)
            .await?;
    }
    let mut tags = doc["dist-tags"].as_object().cloned().unwrap_or_default();
    if tags.is_empty() {
        if let Some(v) = new_versions.last() {
            tags.insert("latest".into(), json!(v.version));
        }
    }
    for (tag, ver) in &tags {
        let Some(ver) = ver.as_str() else { continue };
        sqlx::query(
            "INSERT INTO package_tags (package_id, tag, version_id)
             SELECT $1, $2, id FROM package_versions WHERE package_id = $1 AND name = $3 AND deleted_at IS NULL
             ON CONFLICT (package_id, tag) DO UPDATE SET version_id = EXCLUDED.version_id, updated_at = now()",
        )
        .bind(pkg.id)
        .bind(tag)
        .bind(ver)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE packages SET updated_at = now() WHERE id = $1").bind(pkg.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(ok_json(StatusCode::CREATED, json!({ "ok": true, "id": full_name(&owner, name), "success": true })))
}

/// `npm unpublish @o/p@1.0.0`: the client PUTs the packument without the removed versions.
async fn unpublish_versions(state: &AppState, user: Option<&User>, scope: &str, name: &str, body: Body) -> NpmResult<Response> {
    let (_, pkg, perm) = load(state, user, scope, name).await?;
    let data = axum::body::to_bytes(body, 64 << 20).await.map_err(|_| err(StatusCode::PAYLOAD_TOO_LARGE, "request too large"))?;
    let doc: Value = serde_json::from_slice(&data).map_err(|_| err(StatusCode::BAD_REQUEST, "invalid JSON"))?;
    let keep: Vec<String> = doc["versions"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    let current = super::versions(state, pkg.id, false).await?;
    let removed: Vec<&VersionRow> = current.iter().filter(|v| !keep.contains(&v.name)).collect();
    if !removed.is_empty() {
        need(user, perm, PkgPerm::Admin)?;
    } else {
        need(user, perm, PkgPerm::Write)?;
    }
    let actor = user.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "authentication required"))?;
    for v in removed {
        super::delete_version(state, actor, &pkg, perm, v.id).await?;
    }
    if let Some(tags) = doc["dist-tags"].as_object() {
        for (tag, ver) in tags {
            if let Some(ver) = ver.as_str() {
                sqlx::query(
                    "INSERT INTO package_tags (package_id, tag, version_id)
                     SELECT $1, $2, id FROM package_versions WHERE package_id = $1 AND name = $3 AND deleted_at IS NULL
                     ON CONFLICT (package_id, tag) DO UPDATE SET version_id = EXCLUDED.version_id, updated_at = now()",
                )
                .bind(pkg.id)
                .bind(tag)
                .bind(ver)
                .execute(&state.db)
                .await?;
            }
        }
    }
    Ok(ok_json(StatusCode::OK, json!({ "ok": true })))
}

/// `npm unpublish @o/p --force`: deletes the whole package (restorable for 30 days).
async fn unpublish_all(state: &AppState, user: Option<&User>, scope: &str, name: &str) -> NpmResult<Response> {
    let (owner, _pkg, perm) = load(state, user, scope, name).await?;
    need(user, perm, PkgPerm::Admin)?;
    let actor = user.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "authentication required"))?;
    super::delete_package(state, actor, &owner, "npm", name).await?;
    Ok(ok_json(StatusCode::OK, json!({ "ok": true })))
}

async fn dist_tags(state: &AppState, user: Option<&User>, m: &Method, scope: &str, name: &str, tag: Option<&str>, body: Body) -> NpmResult<Response> {
    let (_, pkg, perm) = load(state, user, scope, name).await?;
    match (m, tag) {
        (&Method::GET, None) => Ok(ok_json(StatusCode::OK, Value::Object(dist_tag_map(state, &pkg).await?))),
        (&Method::PUT, Some(tag)) | (&Method::POST, Some(tag)) => {
            need(user, perm, PkgPerm::Write)?;
            let data = axum::body::to_bytes(body, 1 << 16).await.map_err(|_| err(StatusCode::PAYLOAD_TOO_LARGE, "request too large"))?;
            let ver: String = serde_json::from_slice(&data).map_err(|_| err(StatusCode::BAD_REQUEST, "body must be a JSON string"))?;
            let n = sqlx::query(
                "INSERT INTO package_tags (package_id, tag, version_id)
                 SELECT $1, $2, id FROM package_versions WHERE package_id = $1 AND name = $3 AND deleted_at IS NULL
                 ON CONFLICT (package_id, tag) DO UPDATE SET version_id = EXCLUDED.version_id, updated_at = now()",
            )
            .bind(pkg.id)
            .bind(tag)
            .bind(&ver)
            .execute(&state.db)
            .await?;
            if n.rows_affected() == 0 {
                return Err(err(StatusCode::NOT_FOUND, format!("version {ver} not found")));
            }
            Ok(ok_json(StatusCode::CREATED, json!({ "ok": true })))
        }
        (&Method::DELETE, Some(tag)) => {
            need(user, perm, PkgPerm::Write)?;
            sqlx::query("DELETE FROM package_tags WHERE package_id = $1 AND tag = $2").bind(pkg.id).bind(tag).execute(&state.db).await?;
            Ok(ok_json(StatusCode::OK, json!({ "ok": true })))
        }
        _ => Err(err(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")),
    }
}

/// `npm login --auth-type=legacy`: login (or email) + password -> a new PAT.
async fn login(state: &AppState, who: &str, body: Body) -> NpmResult<Response> {
    let data = axum::body::to_bytes(body, 1 << 16).await.map_err(|_| err(StatusCode::PAYLOAD_TOO_LARGE, "request too large"))?;
    let doc: Value = serde_json::from_slice(&data).map_err(|_| err(StatusCode::BAD_REQUEST, "invalid JSON"))?;
    let name = doc["name"].as_str().map(str::to_string).unwrap_or_else(|| who.trim_start_matches("org.couchdb.user:").to_string());
    let password = doc["password"].as_str().unwrap_or("");
    let user = match crate::auth::user_for_token(state, password).await? {
        Some(u) => Some(u),
        None => crate::auth::user_for_password(state, &name, password, None).await?,
    };
    let user = user.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid login or password"))?;
    let (_, token) = crate::forge_api::create_token_for(state, user.id, "npm login").await?;
    Ok(ok_json(StatusCode::CREATED, json!({ "ok": true, "id": format!("org.couchdb.user:{}", user.login), "token": token })))
}
