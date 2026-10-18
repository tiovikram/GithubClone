//! Repository settings and administration endpoints:
//! - toggles: `vulnerability-alerts`, `automated-security-fixes`, `immutable-releases`, `lfs`;
//! - custom property values (`/properties/values`);
//! - `POST /dispatches` (`repository_dispatch` webhook event, consumed by Actions);
//! - `POST /transfer` (immediate transfer + old-name redirect);
//! - `GET /repositories` (public repositories by id, `since` pagination);
//! - `GET /repos/{o}/{r}/teams`, `GET /replicas/caches`, `GET /codeowners/errors`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}";
    Router::new()
        .route(&format!("{r}/vulnerability-alerts"), get(check_vuln).put(enable_vuln).delete(disable_vuln))
        .route(&format!("{r}/automated-security-fixes"), get(check_asf).put(enable_asf).delete(disable_asf))
        .route(&format!("{r}/immutable-releases"), get(check_immutable).put(enable_immutable).delete(disable_immutable))
        .route(&format!("{r}/lfs"), put(enable_lfs).delete(disable_lfs))
        .route(&format!("{r}/properties/values"), get(get_properties).patch(patch_properties))
        .route(&format!("{r}/dispatches"), post(dispatch))
        .route(&format!("{r}/transfer"), post(transfer_h))
        .route(&format!("{r}/teams"), get(list_teams))
        .route(&format!("{r}/replicas/caches"), get(cache_info))
        .route(&format!("{r}/codeowners/errors"), get(codeowners_errors))
        .route("/repositories", get(list_public))
}

// ---------------------------------------------------------------------------------------------
// Settings rows

#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct RepoSettings {
    pub vulnerability_alerts: bool,
    pub automated_security_fixes: bool,
    pub automated_security_fixes_paused: bool,
    pub immutable_releases: bool,
    pub lfs_enabled: bool,
}

/// Current settings of a repository (defaults when never changed).
pub async fn settings(s: &AppState, repo_id: i64) -> ApiResult<RepoSettings> {
    let row: Option<RepoSettings> = sqlx::query_as(
        "SELECT vulnerability_alerts, automated_security_fixes, automated_security_fixes_paused, immutable_releases, lfs_enabled
           FROM repo_settings WHERE repo_id = $1",
    )
    .bind(repo_id)
    .fetch_optional(&s.db)
    .await?;
    Ok(row.unwrap_or(RepoSettings { lfs_enabled: true, ..Default::default() }))
}

/// Sets one boolean setting column. `col` is a fixed identifier from this module.
async fn set_flag(s: &AppState, repo_id: i64, col: &'static str, v: bool) -> ApiResult<()> {
    let sql = format!(
        "INSERT INTO repo_settings (repo_id, {col}) VALUES ($1, $2)
         ON CONFLICT (repo_id) DO UPDATE SET {col} = $2, updated_at = now()"
    );
    sqlx::query(sqlx::AssertSqlSafe(sql)).bind(repo_id).bind(v).execute(&s.db).await?;
    Ok(())
}

/// Whether immutable releases are on (published releases' assets and tags are locked).
pub async fn immutable_releases(s: &AppState, repo_id: i64) -> bool {
    settings(s, repo_id).await.map(|x| x.immutable_releases).unwrap_or(false)
}

/// 422 when the assets of a published release may not change (immutable releases on).
pub async fn check_release_mutable(s: &AppState, repo_id: i64, draft: bool) -> ApiResult<()> {
    if !draft && immutable_releases(s, repo_id).await {
        let mut e = ApiError::unprocessable("Cannot modify assets of an immutable release.");
        e.errors = None;
        return Err(e);
    }
    Ok(())
}

async fn admin_ctx(s: &AppState, u: &User, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    ctx.require(Perm::Admin)?;
    Ok(ctx)
}

async fn check_vuln(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    if settings(&s, ctx.repo.id).await?.vulnerability_alerts {
        Ok(no_content())
    } else {
        Ok(StatusCode::NOT_FOUND.into_response())
    }
}

async fn enable_vuln(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    set_flag(&s, ctx.repo.id, "vulnerability_alerts", true).await?;
    Ok(no_content())
}

async fn disable_vuln(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    set_flag(&s, ctx.repo.id, "vulnerability_alerts", false).await?;
    // Security updates depend on alerts (GitHub disables both).
    set_flag(&s, ctx.repo.id, "automated_security_fixes", false).await?;
    Ok(no_content())
}

async fn check_asf(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    let st = settings(&s, ctx.repo.id).await?;
    Ok(Json(json!({"enabled": st.automated_security_fixes && st.vulnerability_alerts, "paused": st.automated_security_fixes_paused})))
}

async fn enable_asf(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    if !settings(&s, ctx.repo.id).await?.vulnerability_alerts {
        let mut e = ApiError::unprocessable("Vulnerability alerts must be enabled to enable automated security fixes.");
        e.errors = None;
        return Err(e);
    }
    set_flag(&s, ctx.repo.id, "automated_security_fixes", true).await?;
    Ok(no_content())
}

async fn disable_asf(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    set_flag(&s, ctx.repo.id, "automated_security_fixes", false).await?;
    Ok(no_content())
}

async fn check_immutable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    let st = settings(&s, ctx.repo.id).await?;
    Ok(Json(json!({"enabled": st.immutable_releases, "enforced_by_owner": false})))
}

async fn enable_immutable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    set_flag(&s, ctx.repo.id, "immutable_releases", true).await?;
    Ok(no_content())
}

async fn disable_immutable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = admin_ctx(&s, &u, &o, &r).await?;
    set_flag(&s, ctx.repo.id, "immutable_releases", false).await?;
    Ok(no_content())
}

/// Git LFS on/off. GHES restricts this to site admins (`site_admin` scope); forge also lets
/// repository admins toggle it for their own repositories.
async fn lfs_ctx(s: &AppState, u: &User, o: &str, r: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), o, r).await?;
    if ctx.perm < Perm::Admin && !u.site_admin {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "Must have admin rights to Repository."));
    }
    Ok(ctx)
}

async fn enable_lfs(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = lfs_ctx(&s, &u, &o, &r).await?;
    set_flag(&s, ctx.repo.id, "lfs_enabled", true).await?;
    Ok((StatusCode::ACCEPTED, axum::Json(json!({}))).into_response())
}

async fn disable_lfs(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = lfs_ctx(&s, &u, &o, &r).await?;
    set_flag(&s, ctx.repo.id, "lfs_enabled", false).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Custom property values

/// `custom-property-value` list of a repository (effective values: explicit or default).
/// Definitions and storage belong to the orgs area (`api::orgs_props`).
pub async fn property_values(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<Value>> {
    if ctx.owner.kind != "Organization" {
        return Ok(vec![]);
    }
    let vals = super::orgs_props::repo_property_values(&s.db, ctx.owner.id, ctx.repo.id).await?;
    Ok(vals.into_iter().map(|(n, v)| json!({"property_name": n, "value": v})).collect())
}

#[derive(Deserialize)]
pub struct PropertyValue {
    pub property_name: Option<String>,
    pub value: Option<Value>,
}

/// Sets (or, with a null value, unsets) custom property values of a repository (admin).
/// Values are validated against the organization's property definitions.
pub async fn set_property_values(s: &AppState, ctx: &RepoCtx, actor: &User, values: Vec<PropertyValue>) -> ApiResult<()> {
    ctx.require(Perm::Admin)?;
    if ctx.owner.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    let mut props = Vec::new();
    for pv in values {
        let name = pv.property_name.filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("CustomPropertyValue", "property_name", "missing_field"))?;
        props.push((name, pv.value.unwrap_or(Value::Null)));
    }
    super::orgs_props::set_repo_property_values(s, &ctx.owner, actor, &[(ctx.repo.id, ctx.full_name())], &props).await
}

async fn get_properties(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    Ok(Json(Value::Array(property_values(&s, &ctx).await?)))
}

#[derive(Deserialize)]
struct PatchProperties {
    properties: Option<Vec<PropertyValue>>,
}

async fn patch_properties(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((o, r)): Path<(String, String)>,
    Json(b): Json<PatchProperties>,
) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let props = b.properties.ok_or_else(|| ApiError::validation("Repository", "properties", "missing_field"))?;
    set_property_values(&s, &ctx, &u, props).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// repository_dispatch

#[derive(Deserialize)]
struct DispatchReq {
    event_type: Option<String>,
    client_payload: Option<Value>,
}

/// Emits a `repository_dispatch` event (webhooks; Actions workflows `on: repository_dispatch`).
pub async fn create_dispatch(s: &AppState, ctx: &RepoCtx, actor: &User, event_type: &str, client_payload: Value) -> ApiResult<()> {
    ctx.require(Perm::Write)?;
    if event_type.is_empty() || event_type.chars().count() > 100 {
        return Err(ApiError::validation("Repository", "event_type", if event_type.is_empty() { "missing_field" } else { "invalid" }));
    }
    match &client_payload {
        Value::Object(m) if m.len() > 10 => {
            let mut e = ApiError::unprocessable("client_payload may not have more than 10 top-level properties");
            e.errors = None;
            return Err(e);
        }
        Value::Object(_) | Value::Null => {}
        _ => return Err(ApiError::validation("Repository", "client_payload", "invalid")),
    }
    let payload = json!({
        "action": event_type,
        "branch": ctx.repo.default_branch,
        "client_payload": if client_payload.is_null() { json!({}) } else { client_payload },
        "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor),
    });
    crate::webhooks::enqueue_event_ctx(s, ctx, "repository_dispatch", payload).await?;
    Ok(())
}

async fn dispatch(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<DispatchReq>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    create_dispatch(&s, &ctx, &u, b.event_type.as_deref().unwrap_or(""), b.client_payload.unwrap_or(Value::Null)).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Transfer

#[derive(Deserialize)]
pub struct TransferReq {
    pub new_owner: Option<String>,
    pub new_name: Option<String>,
    pub team_ids: Option<Vec<i64>>,
}

/// Transfers a repository to another owner (repo admin). Transfers into an organization need
/// permission to create repositories there; transfers to another user account are only
/// performed by site admins (GitHub would ask the new owner to accept). The old `owner/name`
/// keeps resolving to the repository. Returns the repository as it was before the transfer
/// (GitHub's response keeps the original owner).
pub async fn transfer(s: &AppState, ctx: &RepoCtx, actor: &User, req: TransferReq) -> ApiResult<RepoCtx> {
    ctx.require(Perm::Admin)?;
    let login = req.new_owner.filter(|x| !x.is_empty()).ok_or_else(|| ApiError::validation("Repository", "new_owner", "missing_field"))?;
    let target = db::user_by_login(&s.db, &login).await.map_err(|_| ApiError::validation("Repository", "new_owner", "invalid"))?;
    let new_name = req.new_name.filter(|x| !x.is_empty()).unwrap_or_else(|| ctx.repo.name.clone());
    if !super::repos::valid_repo_name(&new_name) {
        return Err(ApiError::validation("Repository", "new_name", "invalid"));
    }
    if target.id == ctx.owner.id && new_name == ctx.repo.name {
        let mut e = ApiError::unprocessable(format!("{} already owns {}", target.login, ctx.repo.name));
        e.errors = None;
        return Err(e);
    }
    let allowed = if actor.site_admin || target.id == actor.id {
        true
    } else if target.kind == "Organization" {
        super::orgs_extra::org_role(&s.db, target.id, actor.id).await?.is_some()
    } else {
        false
    };
    if !allowed {
        return Err(ApiError::forbidden(format!("You don't have the permission to create repositories on {}", target.login)));
    }
    let taken: Option<i64> = sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1 AND lower(name) = lower($2) AND id <> $3")
        .bind(target.id)
        .bind(&new_name)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?;
    if taken.is_some() {
        let mut e = ApiError::unprocessable("Validation Failed");
        e.errors = Some(json!([{"resource": "Repository", "code": "custom", "field": "name",
            "message": format!("{} already has a repository named {new_name}", target.login)}]));
        return Err(e);
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE repositories SET owner_id = $2, name = $3, updated_at = now() WHERE id = $1")
        .bind(ctx.repo.id)
        .bind(target.id)
        .bind(&new_name)
        .execute(&mut *tx)
        .await?;
    // The new owner no longer needs an explicit collaborator row; the old path redirects.
    sqlx::query("DELETE FROM repo_collaborators WHERE repo_id = $1 AND user_id = $2").bind(ctx.repo.id).bind(target.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM repo_redirects WHERE lower(owner_login) = lower($1) AND lower(name) = lower($2)")
        .bind(&target.login)
        .bind(&new_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO repo_redirects (owner_login, name, repo_id) VALUES ($1,$2,$3)
         ON CONFLICT (owner_login, name) DO UPDATE SET repo_id = $3",
    )
    .bind(&ctx.owner.login)
    .bind(&ctx.repo.name)
    .bind(ctx.repo.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let _ = req.team_ids;
    let repo = db::repo_by_id(&s.db, ctx.repo.id).await?;
    let new_ctx = RepoCtx { repo, owner: target.clone(), perm: Perm::Admin };
    let payload = json!({
        "action": "transferred",
        "changes": {"owner": {"from": {(if ctx.owner.kind == "Organization" { "organization" } else { "user" }):
            crate::json::user::simple(&s.api_url(), s.base_url(), &ctx.owner)}}},
        "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor),
    });
    let _ = crate::webhooks::enqueue_event_ctx(s, &new_ctx, "repository", payload).await;
    Ok(new_ctx)
}

async fn transfer_h(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<TransferReq>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let before = crate::json::repo::repo(&s.api_url(), s.base_url(), &ctx);
    transfer(&s, &ctx, &u, b).await?;
    Ok((StatusCode::ACCEPTED, axum::Json(before)).into_response())
}

// ---------------------------------------------------------------------------------------------
// GET /repositories

#[derive(Deserialize)]
struct PublicQuery {
    since: Option<i64>,
    per_page: Option<i64>,
}

async fn list_public(State(s): State<AppState>, MaybeUser(u): MaybeUser, Query(q): Query<PublicQuery>) -> ApiResult<Response> {
    let since = q.since.unwrap_or(0);
    let per = q.per_page.unwrap_or(30).clamp(1, 100);
    let rows: Vec<db::Repo> = sqlx::query_as("SELECT * FROM repositories WHERE NOT private AND id > $1 ORDER BY id LIMIT $2")
        .bind(since)
        .bind(per)
        .fetch_all(&s.db)
        .await?;
    let owner_ids: Vec<i64> = rows.iter().map(|r| r.owner_id).collect();
    let owners = crate::json::issue::users_by_ids(&s.db, &owner_ids).await?;
    let api = s.api_url();
    let items: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            let owner = owners.get(&r.owner_id)?.clone();
            let perm = match &u {
                Some(v) if v.id == owner.id || v.site_admin => Perm::Admin,
                _ => Perm::Read,
            };
            let ctx = RepoCtx { repo: r.clone(), owner, perm };
            let mut v = crate::json::repo::repo(&api, s.base_url(), &ctx);
            if u.is_none() {
                v.as_object_mut().map(|o| o.remove("permissions"));
            }
            Some(v)
        })
        .collect();
    let mut res = axum::Json(Value::Array(items)).into_response();
    if rows.len() as i64 >= per {
        if let Some(last) = rows.last() {
            let link = format!("<{}/repositories?per_page={per}&since={}>; rel=\"next\"", api, last.id);
            if let Ok(v) = HeaderValue::from_str(&link) {
                res.headers_mut().insert(header::LINK, v);
            }
        }
    }
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// Teams with access

/// Team grants on a repository: `(team, permission)`. Team-repository grants are managed by the
/// orgs area (`PUT /orgs/{org}/teams/{slug}/repos/{o}/{r}`); this reads its `team_repos` table
/// when present.
pub async fn team_grants(s: &AppState, repo_id: i64) -> ApiResult<Vec<(super::orgs_extra::TeamRow, String)>> {
    let has: bool = sqlx::query_scalar("SELECT to_regclass('public.team_repos') IS NOT NULL").fetch_one(&s.db).await?;
    if !has {
        return Ok(vec![]);
    }
    let rows: Vec<(i64, String)> =
        match sqlx::query_as("SELECT team_id, permission::text FROM team_repos WHERE repo_id = $1").bind(repo_id).fetch_all(&s.db).await {
            Ok(r) => r,
            Err(_) => return Ok(vec![]),
        };
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let teams: Vec<super::orgs_extra::TeamRow> = sqlx::query_as("SELECT * FROM teams WHERE id = ANY($1) ORDER BY id").bind(&ids).fetch_all(&s.db).await?;
    Ok(teams
        .into_iter()
        .map(|t| {
            let p = rows.iter().find(|r| r.0 == t.id).map(|r| r.1.clone()).unwrap_or_else(|| "pull".into());
            (t, p)
        })
        .collect())
}

async fn list_teams(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(u): AuthUser,
    Path((o, r)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let grants = team_grants(&s, ctx.repo.id).await?;
    let total = grants.len() as i64;
    let items: Vec<Value> = grants
        .iter()
        .skip(p.offset() as usize)
        .take(p.limit() as usize)
        .map(|(t, perm)| {
            let mut t = t.clone();
            t.permission = perm.clone();
            super::orgs_extra::team_json(&s, &ctx.owner, &t, None)
        })
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

// ---------------------------------------------------------------------------------------------
// Cache replicas: the git caches of this pod

async fn cache_info(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Query(_p): Query<PageParams>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Read)?;
    let m = s.git.manifest(ctx.repo.id).await?;
    let path = s.git.repo_path(ctx.repo.id);
    let vf = path.join("forge-version");
    let local: Option<i64> = tokio::fs::read_to_string(&vf).await.ok().and_then(|x| x.trim().parse().ok());
    let mtime = tokio::fs::metadata(&vf).await.ok().and_then(|m| m.modified().ok()).map(chrono::DateTime::<chrono::Utc>::from);
    let status = match local {
        None => "inactive",
        Some(v) if v == m.version => "in_sync",
        Some(_) => "not_in_sync",
    };
    let host = std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| crate::json::repo::host_of(s.base_url()));
    Ok(Json(json!([{
        "host": host,
        "location": s.cfg.cache_dir.display().to_string(),
        "git": {"sync_status": status, "last_sync": crate::json::ts(mtime.unwrap_or_else(chrono::Utc::now))},
    }])))
}

// ---------------------------------------------------------------------------------------------
// CODEOWNERS

const CODEOWNERS_PATHS: [&str; 3] = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

/// The CODEOWNERS file at `rev` (path, content), searched where GitHub looks for it.
pub async fn codeowners_file(s: &AppState, ctx: &RepoCtx, rev: &str) -> ApiResult<Option<(String, String)>> {
    if !crate::git::read::safe_rev(rev) {
        return Ok(None);
    }
    for p in CODEOWNERS_PATHS {
        let spec = format!("{rev}:{p}");
        if let Some(out) = super::plat::git_read(s, ctx.repo.id, &["cat-file", "blob", &spec]).await? {
            return Ok(Some((p.to_string(), String::from_utf8_lossy(&out).into_owned())));
        }
    }
    Ok(None)
}

/// Syntax / owner errors in a CODEOWNERS file (GitHub's `codeowners-errors` entries).
pub async fn codeowners_check(s: &AppState, ctx: &RepoCtx, path: &str, text: &str) -> ApiResult<Vec<Value>> {
    let mut errors = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let content = match line.find(" #") {
            Some(c) => &line[..c],
            None => line,
        };
        let mut tokens: Vec<(usize, &str)> = Vec::new();
        let mut start = None;
        for (j, ch) in content.char_indices() {
            match (ch.is_whitespace(), start) {
                (false, None) => start = Some(j),
                (true, Some(st)) => {
                    tokens.push((st, &content[st..j]));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(st) = start {
            tokens.push((st, &content[st..]));
        }
        let Some(&(pcol, pattern)) = tokens.first() else { continue };
        let mk = |col: usize, kind: &str, msg: String, suggestion: Option<String>| {
            let caret = format!("{}^", " ".repeat(col));
            let mut v = json!({
                "line": lineno, "column": col + 1, "kind": kind, "source": line,
                "message": format!("{msg} on line {lineno}:\n\n  {line}\n  {caret}"),
                "path": path,
            });
            if let Some(sg) = suggestion {
                v["suggestion"] = json!(sg);
            }
            v
        };
        if pattern.starts_with('!') || pattern.contains('[') || pattern.contains(']') || pattern.starts_with("\\#") {
            errors.push(mk(pcol, "Invalid pattern", format!("Invalid pattern `{pattern}`"), None));
            continue;
        }
        for &(col, owner) in &tokens[1..] {
            let ok = if let Some(name) = owner.strip_prefix('@') {
                if let Some((org, team)) = name.split_once('/') {
                    let t: Option<i64> = sqlx::query_scalar(
                        "SELECT t.id FROM teams t JOIN users o ON o.id = t.org_id WHERE lower(o.login) = lower($1) AND lower(t.slug) = lower($2)",
                    )
                    .bind(org)
                    .bind(team)
                    .fetch_optional(&s.db)
                    .await?;
                    t.is_some() && org.eq_ignore_ascii_case(&ctx.owner.login)
                } else {
                    match db::user_by_login(&s.db, name).await {
                        Ok(u) => db::permission(&s.db, Some(&u), &ctx.repo, &ctx.owner).await? >= Perm::Write,
                        Err(_) => false,
                    }
                }
            } else if owner.contains('@') {
                let u: Option<User> = sqlx::query_as(
                    "SELECT u.* FROM users u WHERE lower(u.email) = lower($1)
                     UNION SELECT u.* FROM users u JOIN user_emails e ON e.user_id = u.id WHERE lower(e.email) = lower($1) LIMIT 1",
                )
                .bind(owner)
                .fetch_optional(&s.db)
                .await?;
                match u {
                    Some(u) => db::permission(&s.db, Some(&u), &ctx.repo, &ctx.owner).await? >= Perm::Write,
                    None => false,
                }
            } else {
                errors.push(mk(col, "Invalid owner", format!("Invalid owner `{owner}`"), Some(format!("@{owner}"))));
                continue;
            };
            if !ok {
                errors.push(mk(
                    col,
                    "Unknown owner",
                    format!("Unknown owner `{owner}`: make sure it exists and has write access to the repository"),
                    None,
                ));
            }
        }
    }
    Ok(errors)
}

#[derive(Deserialize)]
struct RefQ {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
}

async fn codeowners_errors(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>, Query(q): Query<RefQ>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let rev = q.git_ref.unwrap_or_else(|| ctx.repo.default_branch.clone());
    let Some((path, text)) = codeowners_file(&s, &ctx, &rev).await? else {
        return Ok(ApiError::not_found().into_response());
    };
    let errors = codeowners_check(&s, &ctx, &path, &text).await?;
    let mut m = Map::new();
    m.insert("errors".into(), Value::Array(errors));
    Ok(axum::Json(Value::Object(m)).into_response())
}
