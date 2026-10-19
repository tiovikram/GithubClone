//! Forge-only package endpoints for the web UI (nested under `/api/forge`, not part of the
//! GitHub API). On GitHub these are web UI pages with no REST equivalent:
//!
//! * `GET /packages/{owner}` (`package_type`, `q` optional): an account's packages of every type.
//! * `GET /packages/{owner}/deleted`: deleted packages the caller can restore (30 days).
//! * `GET /repos/{owner}/{repo}/packages`: packages linked to a repository.
//! * `GET /packages/{owner}/{type}/{name}`: the package page: REST `package` JSON plus the
//!   README (npm: from the latest tarball), the raw visibility (`internal` included), the linked
//!   repository, whether access is inherited and the caller's permission.
//! * `PATCH /packages/{owner}/{type}/{name}` `{visibility?, repository?, inherit_access?}`:
//!   package settings (package admins).
//! * `GET|PUT|DELETE /packages/{owner}/{type}/{name}/access[/{login}]` `{role}`: "Manage access"
//!   (granular per-user roles: read, write, admin).
//! * `GET /packages/{owner}/{type}/{name}/versions/{id}`: a version's digest, size, media type,
//!   tags, manifest metadata and README.
use std::io::Read;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::no_content;
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::packages::{self as pk, PackageRow, PkgPerm, VersionRow};
use crate::state::AppState;

pub fn forge_routes() -> Router<AppState> {
    Router::new()
        .route("/packages/{owner}", get(list_all))
        .route("/packages/{owner}/deleted", get(list_deleted))
        .route("/repos/{owner}/{repo}/packages", get(repo_packages))
        .route("/packages/{owner}/{ptype}/{name}", get(details).patch(update))
        .route("/packages/{owner}/{ptype}/{name}/access", get(list_access))
        .route("/packages/{owner}/{ptype}/{name}/access/{login}", axum::routing::put(grant_access).delete(revoke_access))
        .route("/packages/{owner}/{ptype}/{name}/versions/{id}", get(version_details))
}

fn perm_name(p: PkgPerm) -> &'static str {
    match p {
        PkgPerm::Admin => "admin",
        PkgPerm::Write => "write",
        PkgPerm::Read => "read",
        PkgPerm::None => "none",
    }
}

fn check_type(t: &str) -> ApiResult<()> {
    if pk::TYPES.contains(&t) { Ok(()) } else { Err(ApiError::not_found()) }
}

#[derive(Deserialize, Default)]
struct ListQuery {
    package_type: Option<String>,
    q: Option<String>,
}

async fn list_all(State(s): State<AppState>, MaybeUser(viewer): MaybeUser, Path(owner): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let o = db::user_by_login(&s.db, &owner).await?;
    let ty = q.package_type.filter(|t| !t.is_empty());
    if let Some(t) = &ty {
        check_type(t)?;
    }
    let rows: Vec<PackageRow> = sqlx::query_as(
        "SELECT * FROM packages WHERE owner_id = $1 AND deleted_at IS NULL AND ($2::text IS NULL OR package_type = $2)
           AND ($3::text IS NULL OR name ILIKE '%' || $3 || '%') ORDER BY updated_at DESC, id DESC",
    )
    .bind(o.id)
    .bind(ty)
    .bind(q.q.filter(|x| !x.is_empty()))
    .fetch_all(&s.db)
    .await?;
    let mut readable = Vec::new();
    for p in rows {
        if pk::package_perm(&s, viewer.as_ref(), &p, &o).await? >= PkgPerm::Read {
            readable.push(p);
        }
    }
    Ok(Json(Value::Array(pk::packages_json(&s, viewer.as_ref(), &o, &readable).await?)))
}

async fn list_deleted(State(s): State<AppState>, AuthUser(u): AuthUser, Path(owner): Path<String>) -> ApiResult<Json<Value>> {
    let o = db::user_by_login(&s.db, &owner).await?;
    // The most recently deleted package per name and type (that is the one `restore` brings back).
    let rows: Vec<PackageRow> = sqlx::query_as(
        "SELECT DISTINCT ON (package_type, lower(name)) * FROM packages
          WHERE owner_id = $1 AND deleted_at IS NOT NULL AND deleted_at > now() - make_interval(days => $2::int)
          ORDER BY package_type, lower(name), deleted_at DESC",
    )
    .bind(o.id)
    .bind(pk::RESTORE_DAYS as i32)
    .fetch_all(&s.db)
    .await?;
    let mut out = Vec::new();
    for p in rows {
        if pk::package_perm(&s, Some(&u), &p, &o).await? < PkgPerm::Admin {
            continue;
        }
        let mut v = pk::packages_json(&s, Some(&u), &o, std::slice::from_ref(&p)).await?.remove(0);
        v["deleted_at"] = json!(p.deleted_at.map(crate::json::ts));
        let shadowed = pk::find_active(&s, o.id, &p.package_type, &p.name).await?.is_some();
        v["name_in_use"] = json!(shadowed);
        out.push(v);
    }
    Ok(Json(Value::Array(out)))
}

async fn repo_packages(State(s): State<AppState>, MaybeUser(viewer): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, viewer.as_ref(), &owner, &repo).await?;
    if ctx.perm < Perm::Read {
        return Err(ApiError::not_found());
    }
    let rows: Vec<PackageRow> = sqlx::query_as("SELECT * FROM packages WHERE repo_id = $1 AND deleted_at IS NULL ORDER BY updated_at DESC, id DESC")
        .bind(ctx.repo.id)
        .fetch_all(&s.db)
        .await?;
    let mut out = Vec::new();
    for p in rows {
        let o = db::user_by_id(&s.db, p.owner_id).await?;
        if pk::package_perm(&s, viewer.as_ref(), &p, &o).await? >= PkgPerm::Read {
            out.extend(pk::packages_json(&s, viewer.as_ref(), &o, std::slice::from_ref(&p)).await?);
        }
    }
    Ok(Json(Value::Array(out)))
}

/// The package (active), its owner and the viewer's permission (404 when unreadable).
async fn load(s: &AppState, viewer: Option<&User>, owner: &str, ptype: &str, name: &str) -> ApiResult<(User, PackageRow, PkgPerm)> {
    check_type(ptype)?;
    let o = db::user_by_login(&s.db, owner).await?;
    let (p, perm) = pk::get_readable(s, viewer, &o, ptype, name).await?;
    Ok((o, p, perm))
}

/// A README from an npm tarball (`package/README*`), at most 1 MiB.
fn readme_from_tgz(data: &[u8]) -> Option<String> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut ar = tar::Archive::new(gz);
    for entry in ar.entries().ok()? {
        let Ok(mut e) = entry else { continue };
        let path = e.path().ok()?.to_string_lossy().to_string();
        let mut parts = path.splitn(2, '/');
        let (_, file) = (parts.next()?, parts.next().unwrap_or(""));
        if file.contains('/') || !file.to_ascii_lowercase().starts_with("readme") {
            continue;
        }
        if e.size() > 1 << 20 {
            return None;
        }
        let mut out = String::new();
        e.read_to_string(&mut out).ok()?;
        return Some(out);
    }
    None
}

async fn readme_of(s: &AppState, p: &PackageRow, v: &VersionRow) -> Option<String> {
    if p.package_type == "npm" {
        if let Some(r) = v.metadata.get("readme").and_then(Value::as_str).filter(|r| !r.trim().is_empty()) {
            return Some(r.to_string());
        }
        let data = pk::read_blob(s, &v.digest).await.ok()?;
        return tokio::task::spawn_blocking(move || readme_from_tgz(&data)).await.ok().flatten();
    }
    None
}

async fn latest_version(s: &AppState, p: &PackageRow) -> ApiResult<Option<VersionRow>> {
    if p.package_type == "npm" {
        let tagged: Option<VersionRow> = sqlx::query_as(
            "SELECT v.* FROM package_tags t JOIN package_versions v ON v.id = t.version_id
              WHERE t.package_id = $1 AND t.tag = 'latest' AND v.deleted_at IS NULL",
        )
        .bind(p.id)
        .fetch_optional(&s.db)
        .await?;
        if tagged.is_some() {
            return Ok(tagged);
        }
    }
    Ok(pk::versions(s, p.id, false).await?.into_iter().next())
}

fn version_summary(v: &VersionRow, tags: &[String]) -> Value {
    json!({
        "id": v.id,
        "name": v.name,
        "digest": v.digest,
        "size": v.size,
        "media_type": v.media_type,
        "tags": tags,
        "description": v.description,
        "license": v.license,
        "created_at": crate::json::ts(v.created_at),
        "deleted_at": v.deleted_at.map(crate::json::ts),
    })
}

async fn details(State(s): State<AppState>, MaybeUser(viewer): MaybeUser, Path((owner, ptype, name)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let (o, p, perm) = load(&s, viewer.as_ref(), &owner, &ptype, &name).await?;
    let pkg = pk::packages_json(&s, viewer.as_ref(), &o, std::slice::from_ref(&p)).await?.remove(0);
    let latest = latest_version(&s, &p).await?;
    let (latest_json, readme, description) = match &latest {
        Some(v) => {
            let tags = pk::tags_by_version(&s, &[v.id]).await?;
            (version_summary(v, tags.get(&v.id).map(Vec::as_slice).unwrap_or(&[])), readme_of(&s, &p, v).await, v.description.clone())
        }
        None => (Value::Null, None, None),
    };
    let repo = pk::linked_repo(&s, viewer.as_ref(), &p).await?.filter(|c| c.perm >= Perm::Read).map(|c| c.full_name());
    let creator = match p.creator_id {
        Some(id) => db::user_by_id(&s.db, id).await.ok().map(|u| u.login),
        None => None,
    };
    Ok(Json(json!({
        "package": pkg,
        "visibility": p.visibility,
        "inherit_access": p.inherit_access,
        "viewer_permission": perm_name(perm),
        "repository": repo,
        "creator": creator,
        "latest_version": latest_json,
        "description": description,
        "readme": readme,
        "npm_registry_url": format!("{}/npm/", s.base_url()),
    })))
}

#[derive(Deserialize, Default)]
struct UpdateBody {
    visibility: Option<String>,
    /// `owner/name` or `name` of a repository in the package's namespace; empty string unlinks.
    repository: Option<String>,
    inherit_access: Option<bool>,
}

async fn update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, ptype, name)): Path<(String, String, String)>, Json(b): Json<UpdateBody>) -> ApiResult<Response> {
    let (o, p, perm) = load(&s, Some(&u), &owner, &ptype, &name).await?;
    pk::require(perm, PkgPerm::Admin)?;
    let visibility = match b.visibility.as_deref() {
        None => None,
        Some(v @ ("public" | "private")) => Some(v),
        Some("internal") if o.kind == "Organization" => Some("internal"),
        Some(_) => return Err(ApiError::validation("Package", "visibility", "invalid")),
    };
    let mut repo_id = p.repo_id;
    if let Some(r) = b.repository.as_deref() {
        let r = r.trim();
        if r.is_empty() {
            repo_id = None;
        } else {
            let (ro, rn) = r.split_once('/').unwrap_or((o.login.as_str(), r));
            if !ro.eq_ignore_ascii_case(&o.login) {
                return Err(ApiError::unprocessable("A package can only be linked to a repository owned by the same account."));
            }
            let ctx = db::load_repo(&s.db, Some(&u), ro, rn).await.map_err(|_| ApiError::validation("Package", "repository", "invalid"))?;
            if ctx.perm < Perm::Write || ctx.owner.id != o.id {
                return Err(ApiError::validation("Package", "repository", "invalid"));
            }
            repo_id = Some(ctx.repo.id);
        }
    }
    let row: PackageRow = sqlx::query_as(
        "UPDATE packages SET visibility = COALESCE($2, visibility), repo_id = $3, inherit_access = COALESCE($4, inherit_access), updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(p.id)
    .bind(visibility)
    .bind(repo_id)
    .bind(b.inherit_access)
    .fetch_one(&s.db)
    .await?;
    let v = pk::packages_json(&s, Some(&u), &o, std::slice::from_ref(&row)).await?.remove(0);
    Ok(axum::response::IntoResponse::into_response(axum::Json(json!({
        "package": v,
        "visibility": row.visibility,
        "inherit_access": row.inherit_access,
    }))))
}

async fn list_access(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, ptype, name)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let (_, p, perm) = load(&s, Some(&u), &owner, &ptype, &name).await?;
    pk::require(perm, PkgPerm::Admin)?;
    let rows: Vec<(User, String)> = {
        let r: Vec<(i64, String)> = sqlx::query_as("SELECT user_id, role FROM package_access WHERE package_id = $1 ORDER BY created_at")
            .bind(p.id)
            .fetch_all(&s.db)
            .await?;
        let mut out = Vec::new();
        for (id, role) in r {
            if let Ok(user) = db::user_by_id(&s.db, id).await {
                out.push((user, role));
            }
        }
        out
    };
    let api = s.api_url();
    Ok(Json(Value::Array(
        rows.iter().map(|(user, role)| json!({ "user": crate::json::user::simple(&api, s.base_url(), user), "role": role })).collect(),
    )))
}

#[derive(Deserialize, Default)]
struct RoleBody {
    role: Option<String>,
}

async fn grant_access(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, ptype, name, login)): Path<(String, String, String, String)>,
    Json(b): Json<RoleBody>,
) -> ApiResult<Response> {
    let (_, p, perm) = load(&s, Some(&u), &owner, &ptype, &name).await?;
    pk::require(perm, PkgPerm::Admin)?;
    let role = b.role.unwrap_or_else(|| "read".into());
    if !matches!(role.as_str(), "read" | "write" | "admin") {
        return Err(ApiError::validation("PackageAccess", "role", "invalid"));
    }
    let target = db::user_by_login(&s.db, &login).await.map_err(|_| ApiError::validation("PackageAccess", "user", "invalid"))?;
    if target.kind != "User" {
        return Err(ApiError::validation("PackageAccess", "user", "invalid"));
    }
    sqlx::query("INSERT INTO package_access (package_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (package_id, user_id) DO UPDATE SET role = EXCLUDED.role")
        .bind(p.id)
        .bind(target.id)
        .bind(&role)
        .execute(&s.db)
        .await?;
    Ok(no_content())
}

async fn revoke_access(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, ptype, name, login)): Path<(String, String, String, String)>) -> ApiResult<Response> {
    let (_, p, perm) = load(&s, Some(&u), &owner, &ptype, &name).await?;
    pk::require(perm, PkgPerm::Admin)?;
    let target = db::user_by_login(&s.db, &login).await?;
    sqlx::query("DELETE FROM package_access WHERE package_id = $1 AND user_id = $2").bind(p.id).bind(target.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn version_details(State(s): State<AppState>, MaybeUser(viewer): MaybeUser, Path((owner, ptype, name, id)): Path<(String, String, String, i64)>) -> ApiResult<Json<Value>> {
    let (_, p, perm) = load(&s, viewer.as_ref(), &owner, &ptype, &name).await?;
    let v = pk::version_by_id(&s, p.id, id).await?;
    if v.deleted_at.is_some() && perm < PkgPerm::Admin {
        return Err(ApiError::not_found());
    }
    let tags = pk::tags_by_version(&s, &[v.id]).await?;
    let mut out = version_summary(&v, tags.get(&v.id).map(Vec::as_slice).unwrap_or(&[]));
    let meta = &v.metadata;
    out["metadata"] = if p.package_type == "npm" {
        json!({
            "dependencies": meta.get("dependencies"),
            "homepage": meta.get("homepage"),
            "keywords": meta.get("keywords"),
            "shasum": meta["dist"].get("shasum"),
            "integrity": meta["dist"].get("integrity"),
        })
    } else {
        json!({
            "config_digest": meta.get("config_digest"),
            "annotations": meta.get("annotations"),
            "children": meta.get("children"),
            "artifact_type": meta.get("artifact_type"),
        })
    };
    out["readme"] = json!(readme_of(&s, &p, &v).await);
    out["publisher"] = json!(match v.creator_id {
        Some(c) => db::user_by_id(&s.db, c).await.ok().map(|u| u.login),
        None => None,
    });
    out["viewer_permission"] = json!(perm_name(perm));
    Ok(Json(out))
}
