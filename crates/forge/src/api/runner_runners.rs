//! Self-hosted runners: list/get/delete, registration and remove tokens, JIT configuration,
//! runner application downloads and labels, for repositories, organizations and the
//! enterprise (instance-wide runners; any enterprise slug, site administrators only).
use axum::body::Body;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use super::plat::total_envelope;
use super::{no_content, PageParams};
use crate::auth::AuthUser;
use crate::db::{self, Perm, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::runner::protocol::{self, all_labels};
use crate::runner::token::{self, RunnerRow, RunnerScope};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let mut r = Router::new();
    for base in ["/repos/{owner}/{repo}/actions/runners", "/orgs/{org}/actions/runners"] {
        r = r
            .route(base, get(list))
            .route(&format!("{base}/downloads"), get(downloads))
            .route(&format!("{base}/generate-jitconfig"), post(jitconfig))
            .route(&format!("{base}/registration-token"), post(registration_token))
            .route(&format!("{base}/remove-token"), post(remove_token))
            .route(&format!("{base}/{{runner_id}}"), get(get_one).delete(delete_one))
            .route(&format!("{base}/{{runner_id}}/labels"), get(labels).post(add_labels).put(set_labels).delete(clear_labels))
            .route(&format!("{base}/{{runner_id}}/labels/{{name}}"), delete(remove_label));
    }
    r.route("/enterprises/{enterprise}/actions/runners/generate-jitconfig", post(jitconfig))
}

/// The runner application (`forge runner` is part of the forge binary): serves the running
/// executable. Mounted outside `/api/v3`.
pub fn download_routes() -> Router<AppState> {
    Router::new().route("/_actions/runner/forge", get(runner_binary))
}

/// Resolves the scope from the path parameters and checks the caller administers it.
async fn scope(s: &AppState, u: &User, params: &std::collections::HashMap<String, String>) -> ApiResult<(RunnerScope, String)> {
    if let (Some(owner), Some(repo)) = (params.get("owner"), params.get("repo")) {
        let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
        ctx.require(Perm::Admin)?;
        return Ok((RunnerScope::Repo(ctx.repo.id), format!("{}/{}", s.base_url(), ctx.full_name())));
    }
    if let Some(org) = params.get("org") {
        let o = db::user_by_login(&s.db, org).await?;
        if o.kind != "Organization" {
            return Err(ApiError::not_found());
        }
        let role = super::orgs_extra::org_role(&s.db, o.id, u.id).await?;
        return match role.as_deref() {
            Some("admin") => Ok((RunnerScope::Org(o.id), format!("{}/{}", s.base_url(), o.login))),
            _ if u.site_admin => Ok((RunnerScope::Org(o.id), format!("{}/{}", s.base_url(), o.login))),
            Some(_) => Err(ApiError::forbidden("You must be an organization owner to manage self-hosted runners.")),
            None => Err(ApiError::not_found()),
        };
    }
    if params.contains_key("enterprise") {
        if !u.site_admin {
            return Err(ApiError::forbidden("You must be a site administrator to manage enterprise runners."));
        }
        return Ok((RunnerScope::Enterprise, s.base_url().to_string()));
    }
    Err(ApiError::not_found())
}

type Params = Path<std::collections::HashMap<String, String>>;

/// `online` when the runner polled within the last minute.
fn status_of(r: &RunnerRow) -> &'static str {
    match r.last_seen_at {
        Some(t) if chrono::Utc::now() - t < chrono::Duration::seconds(90) => "online",
        _ => "offline",
    }
}

fn label_id(name: &str) -> i64 {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(name.to_lowercase().as_bytes());
    (u32::from_be_bytes(h[..4].try_into().unwrap()) >> 1) as i64
}

fn labels_json(r: &RunnerRow) -> Vec<Value> {
    all_labels(r)
        .into_iter()
        .enumerate()
        .map(|(i, l)| json!({ "id": label_id(&l), "name": l, "type": if i < 3 { "read-only" } else { "custom" } }))
        .collect()
}

pub fn runner_json(r: &RunnerRow, busy: bool) -> Value {
    json!({
        "id": r.id,
        "runner_group_id": r.runner_group_id,
        "name": r.name,
        "os": r.os,
        "status": status_of(r),
        "busy": busy,
        "labels": labels_json(r),
        "ephemeral": r.ephemeral,
        "version": r.version,
    })
}

async fn busy_ids(s: &AppState, ids: &[i64]) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT DISTINCT runner_id FROM actions_jobs WHERE status = 'in_progress' AND runner_id = ANY($1)").bind(ids).fetch_all(&s.db).await?)
}

async fn runner_in(s: &AppState, sc: &RunnerScope, id: &str) -> ApiResult<RunnerRow> {
    let id: i64 = id.parse().map_err(|_| ApiError::not_found())?;
    let (repo_id, org_id) = sc.ids();
    sqlx::query_as("SELECT * FROM actions_runners WHERE id = $1 AND COALESCE(repo_id, 0) = COALESCE($2, 0) AND COALESCE(org_id, 0) = COALESCE($3, 0)")
        .bind(id)
        .bind(repo_id)
        .bind(org_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize, Default)]
struct ListQuery {
    name: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(p): Params, Query(q): Query<ListQuery>) -> ApiResult<Response> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let (repo_id, org_id) = sc.ids();
    let pp = PageParams { page: q.page, per_page: q.per_page };
    let name = q.name.filter(|n| !n.is_empty());
    let filter = "COALESCE(repo_id, 0) = COALESCE($1, 0) AND COALESCE(org_id, 0) = COALESCE($2, 0) AND ($3::text IS NULL OR lower(name) = lower($3))";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM actions_runners WHERE {filter}")))
        .bind(repo_id)
        .bind(org_id)
        .bind(&name)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<RunnerRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM actions_runners WHERE {filter} ORDER BY id LIMIT $4 OFFSET $5")))
        .bind(repo_id)
        .bind(org_id)
        .bind(&name)
        .bind(pp.limit())
        .bind(pp.offset())
        .fetch_all(&s.db)
        .await?;
    let busy = busy_ids(&s, &rows.iter().map(|r| r.id).collect::<Vec<_>>()).await?;
    let items: Vec<Value> = rows.iter().map(|r| runner_json(r, busy.contains(&r.id))).collect();
    let links = super::Paged::new(&uri, &s, pp, Some(total), vec![]).into_response();
    let mut res = Json(total_envelope("runners", items, total)).into_response();
    if let Some(l) = links.headers().get(header::LINK) {
        res.headers_mut().insert(header::LINK, l.clone());
    }
    Ok(res)
}

async fn get_one(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Json<Value>> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let r = runner_in(&s, &sc, p.get("runner_id").map(String::as_str).unwrap_or("")).await?;
    let busy = !busy_ids(&s, &[r.id]).await?.is_empty();
    Ok(Json(runner_json(&r, busy)))
}

async fn delete_one(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Response> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let r = runner_in(&s, &sc, p.get("runner_id").map(String::as_str).unwrap_or("")).await?;
    if !busy_ids(&s, &[r.id]).await?.is_empty() {
        return Err(ApiError::unprocessable("Bad request - Runner \"".to_string() + &r.name + "\" is still running a job\""));
    }
    sqlx::query("DELETE FROM actions_runners WHERE id = $1").bind(r.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn token_response(s: &AppState, kind: &str, sc: &RunnerScope) -> ApiResult<Response> {
    let (t, exp) = token::create_runner_token(&s.db, kind, sc, 3600).await?;
    Ok((StatusCode::CREATED, Json(json!({ "token": t, "expires_at": crate::json::ts(exp) }))).into_response())
}

async fn registration_token(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Response> {
    let (sc, _) = scope(&s, &u, &p).await?;
    token_response(&s, "registration", &sc).await
}

async fn remove_token(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Response> {
    let (sc, _) = scope(&s, &u, &p).await?;
    token_response(&s, "remove", &sc).await
}

#[derive(Deserialize)]
struct JitBody {
    name: Option<String>,
    runner_group_id: Option<i64>,
    labels: Option<Vec<String>>,
    work_folder: Option<String>,
}

/// Creates an ephemeral runner and returns its configuration for `forge runner --jitconfig`.
async fn jitconfig(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params, Json(b): Json<JitBody>) -> ApiResult<Response> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let name = b.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("Runner", "name", "missing_field"))?;
    let labels = b.labels.ok_or_else(|| ApiError::validation("Runner", "labels", "missing_field"))?;
    if labels.is_empty() || labels.len() > 100 {
        return Err(ApiError::validation("Runner", "labels", "invalid"));
    }
    if b.runner_group_id.is_some_and(|g| g != 1) {
        return Err(ApiError::not_found());
    }
    let os = labels.iter().find_map(|l| match l.to_ascii_lowercase().as_str() {
        "linux" => Some("Linux"),
        "macos" => Some("macOS"),
        "windows" => Some("Windows"),
        _ => None,
    });
    let os = os.map(str::to_string).unwrap_or_else(|| if std::env::consts::OS == "macos" { "macOS".into() } else { "Linux".into() });
    let arch = if labels.iter().any(|l| l.eq_ignore_ascii_case("arm64")) { "ARM64" } else { "X64" };
    let (row, cred) = protocol::create_runner(&s, &sc, &name, &labels, &os, arch, None, true, false).await?;
    let cfg = json!({
        "url": s.base_url(),
        "runner_id": row.id,
        "name": row.name,
        "credential": cred,
        "work_folder": b.work_folder.unwrap_or_else(|| "_work".into()),
        "ephemeral": true,
    });
    let encoded = base64::engine::general_purpose::STANDARD.encode(cfg.to_string());
    Ok((StatusCode::CREATED, Json(json!({ "runner": runner_json(&row, false), "encoded_jit_config": encoded }))).into_response())
}

async fn downloads(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Json<Value>> {
    scope(&s, &u, &p).await?;
    let os = match std::env::consts::OS {
        "macos" => "osx",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    Ok(Json(json!([{
        "os": os,
        "architecture": arch,
        "download_url": format!("{}/_actions/runner/forge", s.base_url()),
        "filename": format!("forge-{os}-{arch}"),
    }])))
}

async fn runner_binary() -> ApiResult<Response> {
    let exe = std::env::current_exe().map_err(ApiError::internal)?;
    let f = tokio::fs::File::open(&exe).await.map_err(ApiError::internal)?;
    let stream = tokio_util::io::ReaderStream::new(f);
    Ok(([(header::CONTENT_TYPE, "application/octet-stream"), (header::CONTENT_DISPOSITION, "attachment; filename=forge")], Body::from_stream(stream))
        .into_response())
}

// ---------------------------------------------------------------------------------------------
// Labels

fn labels_envelope(r: &RunnerRow) -> Json<Value> {
    let l = labels_json(r);
    let n = l.len() as i64;
    Json(total_envelope("labels", l, n))
}

#[derive(Deserialize)]
struct LabelsBody {
    labels: Option<Vec<String>>,
}

async fn labels(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Json<Value>> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let r = runner_in(&s, &sc, p.get("runner_id").map(String::as_str).unwrap_or("")).await?;
    Ok(labels_envelope(&r))
}

async fn store_labels(s: &AppState, r: &RunnerRow, labels: Vec<String>) -> ApiResult<RunnerRow> {
    let custom = protocol::custom_labels(&labels, &r.os, &r.arch);
    Ok(sqlx::query_as("UPDATE actions_runners SET labels = $2 WHERE id = $1 RETURNING *").bind(r.id).bind(custom).fetch_one(&s.db).await?)
}

fn check_labels(labels: &Option<Vec<String>>, allow_empty: bool) -> ApiResult<Vec<String>> {
    let l = labels.clone().ok_or_else(|| ApiError::validation("Runner", "labels", "missing_field"))?;
    if (!allow_empty && l.is_empty()) || l.len() > 100 || l.iter().any(|x| x.trim().is_empty() || x.len() > 256) {
        return Err(ApiError::validation("Runner", "labels", "invalid"));
    }
    Ok(l)
}

async fn add_labels(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params, Json(b): Json<LabelsBody>) -> ApiResult<Json<Value>> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let r = runner_in(&s, &sc, p.get("runner_id").map(String::as_str).unwrap_or("")).await?;
    let mut all = r.labels.clone();
    all.extend(check_labels(&b.labels, false)?);
    let r = store_labels(&s, &r, all).await?;
    Ok(labels_envelope(&r))
}

async fn set_labels(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params, Json(b): Json<LabelsBody>) -> ApiResult<Json<Value>> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let r = runner_in(&s, &sc, p.get("runner_id").map(String::as_str).unwrap_or("")).await?;
    let r = store_labels(&s, &r, check_labels(&b.labels, true)?).await?;
    Ok(labels_envelope(&r))
}

async fn clear_labels(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Json<Value>> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let r = runner_in(&s, &sc, p.get("runner_id").map(String::as_str).unwrap_or("")).await?;
    let r = store_labels(&s, &r, vec![]).await?;
    Ok(labels_envelope(&r))
}

async fn remove_label(State(s): State<AppState>, AuthUser(u): AuthUser, Path(p): Params) -> ApiResult<Json<Value>> {
    let (sc, _) = scope(&s, &u, &p).await?;
    let r = runner_in(&s, &sc, p.get("runner_id").map(String::as_str).unwrap_or("")).await?;
    let name = p.get("name").cloned().unwrap_or_default();
    if all_labels(&r)[..3].iter().any(|l| l.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::unprocessable(format!("Cannot remove read-only label '{name}'")));
    }
    if !r.labels.iter().any(|l| l.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::not_found());
    }
    let rest: Vec<String> = r.labels.iter().filter(|l| !l.eq_ignore_ascii_case(&name)).cloned().collect();
    let r = store_labels(&s, &r, rest).await?;
    Ok(labels_envelope(&r))
}
