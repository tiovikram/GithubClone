//! Enterprise (instance-wide) self-hosted runners (`/enterprises/{enterprise}/actions/runners...`).
//! Site administrators only. The enterprise Actions policy and runner groups are served by
//! `api::actions_settings` (enterprise_routes).
//!
//! Runners live in the runner area's `actions_runners` table (rows with neither repo nor org);
//! when that table is not present (runner area not deployed) the enterprise has no runners and
//! registration tokens cannot be issued (503).
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::ent_admin::table_exists;
use super::{no_content, PageParams};
use crate::enterprise::{self as ent, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let p = "/enterprises/{enterprise}/actions";
    Router::new()
        .route(&format!("{p}/runners"), get(list_runners))
        .route(&format!("{p}/runners/downloads"), get(downloads))
        .route(&format!("{p}/runners/registration-token"), post(registration_token))
        .route(&format!("{p}/runners/remove-token"), post(remove_token))
        .route(&format!("{p}/runners/{{runner_id}}"), get(get_runner).delete(delete_runner))
        .route(&format!("{p}/runners/{{runner_id}}/labels"), get(list_labels).post(add_labels).put(set_labels).delete(remove_custom_labels))
        .route(&format!("{p}/runners/{{runner_id}}/labels/{{name}}"), delete(remove_label))
}


// ---------------------------------------------------------------------------------------------
// Runners (the runner area's actions_runners, enterprise scope)

#[derive(sqlx::FromRow)]
struct RunnerRow {
    id: i64,
    name: String,
    os: String,
    arch: String,
    labels: Vec<String>,
    ephemeral: bool,
    version: Option<String>,
    runner_group_id: i64,
    online: bool,
    busy: bool,
}

const RUNNER_SELECT: &str = "SELECT r.id, r.name, r.os, r.arch, r.labels, r.ephemeral, r.version, r.runner_group_id,
        COALESCE(r.last_seen_at > now() - interval '2 minutes', FALSE) AS online,
        EXISTS (SELECT 1 FROM actions_jobs j WHERE j.runner_id = r.id AND j.status = 'in_progress') AS busy
   FROM actions_runners r WHERE r.repo_id IS NULL AND r.org_id IS NULL";

fn system_labels(r: &RunnerRow) -> Vec<String> {
    vec!["self-hosted".into(), r.os.to_lowercase().replace("macos", "macOS"), r.arch.to_lowercase()]
}

fn is_system(r: &RunnerRow, l: &str) -> bool {
    system_labels(r).iter().any(|x| x.eq_ignore_ascii_case(l))
}

fn labels_json(r: &RunnerRow) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, l) in system_labels(r).into_iter().chain(r.labels.iter().cloned()).enumerate() {
        if seen.insert(l.to_lowercase()) {
            let ty = if is_system(r, &l) { "read-only" } else { "custom" };
            out.push(json!({ "id": i as i64 + 1, "name": l, "type": ty }));
        }
    }
    out
}

fn runner_json(r: &RunnerRow) -> Value {
    json!({
        "id": r.id,
        "runner_group_id": r.runner_group_id,
        "name": r.name,
        "os": r.os,
        "status": if r.online { "online" } else { "offline" },
        "busy": r.busy,
        "labels": labels_json(r),
        "ephemeral": r.ephemeral,
        "version": r.version,
    })
}

async fn runners_available(s: &AppState) -> ApiResult<bool> {
    Ok(table_exists(s, "actions_runners").await? && table_exists(s, "actions_jobs").await?)
}

async fn load_runner(s: &AppState, e: &str, id: i64) -> ApiResult<RunnerRow> {
    ent::check_enterprise(e)?;
    if !runners_available(s).await? {
        return Err(ApiError::not_found());
    }
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{RUNNER_SELECT} AND r.id = $1"))).bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
struct RunnersQuery {
    name: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn runners_page(s: &AppState, group: Option<i64>, name: Option<&str>, p: PageParams) -> ApiResult<Value> {
    if !runners_available(s).await? {
        return Ok(json!({ "total_count": 0, "runners": [] }));
    }
    let filter = "AND ($1::bigint IS NULL OR r.runner_group_id = $1) AND ($2::text IS NULL OR lower(r.name) = lower($2))";
    let rows: Vec<RunnerRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{RUNNER_SELECT} {filter} ORDER BY r.id LIMIT $3 OFFSET $4")))
        .bind(group)
        .bind(name)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM ({RUNNER_SELECT} {filter}) x"))).bind(group).bind(name).fetch_one(&s.db).await?;
    Ok(json!({ "total_count": total, "runners": rows.iter().map(runner_json).collect::<Vec<_>>() }))
}

async fn list_runners(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Query(q): Query<RunnersQuery>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(runners_page(&s, None, q.name.as_deref(), PageParams { page: q.page, per_page: q.per_page }).await?))
}







async fn get_runner(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    Ok(Json(runner_json(&r)))
}

async fn delete_runner(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let r = load_runner(&s, &e, id).await?;
    if r.busy {
        return Err(ApiError::unprocessable("Bad request - Runner \"{}\" is still running a job\"".replace("{}", &r.name)));
    }
    sqlx::query("DELETE FROM actions_runners WHERE id = $1").bind(r.id).execute(&s.db).await?;
    ent::audit(&s, "business.remove_self_hosted_runner", Some(&u), json!({"runner_id": r.id, "name": r.name})).await;
    Ok(no_content())
}

/// Runner application downloads (actions/runner releases; forge serves no binaries itself).
async fn downloads(SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    const V: &str = "2.321.0";
    let item = |os: &str, arch: &str, ext: &str| {
        let osn = match os {
            "osx" => "osx",
            "win" => "win",
            _ => "linux",
        };
        let file = format!("actions-runner-{osn}-{arch}-{V}.{ext}");
        json!({
            "os": os,
            "architecture": arch,
            "download_url": format!("https://github.com/actions/runner/releases/download/v{V}/{file}"),
            "filename": file,
        })
    };
    Ok(Json(json!([
        item("osx", "x64", "tar.gz"),
        item("osx", "arm64", "tar.gz"),
        item("linux", "x64", "tar.gz"),
        item("linux", "arm", "tar.gz"),
        item("linux", "arm64", "tar.gz"),
        item("win", "x64", "zip"),
        item("win", "arm64", "zip"),
    ])))
}

async fn runner_token(s: &AppState, kind: &str) -> ApiResult<Response> {
    if !table_exists(s, "actions_runner_tokens").await? {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "Self-hosted runners are not available on this instance."));
    }
    let token = crate::auth::random_string(29).to_uppercase();
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    sqlx::query("INSERT INTO actions_runner_tokens (token_hash, kind, repo_id, org_id, expires_at) VALUES ($1, $2, NULL, NULL, $3)")
        .bind(crate::auth::sha256_hex(&token))
        .bind(kind)
        .bind(expires)
        .execute(&s.db)
        .await?;
    Ok((StatusCode::CREATED, axum::Json(json!({ "token": token, "expires_at": crate::json::ts(expires) }))).into_response())
}

async fn registration_token(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    runner_token(&s, "registration").await
}

async fn remove_token(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    runner_token(&s, "remove").await
}

fn labels_response(r: &RunnerRow) -> Json<Value> {
    let l = labels_json(r);
    Json(json!({ "total_count": l.len(), "labels": l }))
}

async fn list_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    Ok(labels_response(&load_runner(&s, &e, id).await?))
}

#[derive(Deserialize)]
struct LabelsBody {
    labels: Option<Vec<String>>,
}

fn checked_labels(b: LabelsBody, allow_empty: bool) -> ApiResult<Vec<String>> {
    let l = b.labels.ok_or_else(|| ApiError::validation("Label", "labels", "missing_field"))?;
    if (!allow_empty && l.is_empty()) || l.len() > 100 || l.iter().any(|x| x.trim().is_empty() || x.len() > 256) {
        return Err(ApiError::validation("Label", "labels", "invalid"));
    }
    Ok(l.into_iter().map(|x| x.trim().to_string()).collect())
}

async fn store_labels(s: &AppState, e: &str, r: &RunnerRow, custom: Vec<String>) -> ApiResult<Json<Value>> {
    let mut out: Vec<String> = Vec::new();
    for l in custom {
        if !is_system(r, &l) && !out.iter().any(|x| x.eq_ignore_ascii_case(&l)) {
            out.push(l);
        }
    }
    sqlx::query("UPDATE actions_runners SET labels = $2 WHERE id = $1").bind(r.id).bind(&out).execute(&s.db).await?;
    Ok(labels_response(&load_runner(s, e, r.id).await?))
}

fn custom_labels(r: &RunnerRow) -> Vec<String> {
    r.labels.iter().filter(|l| !is_system(r, l)).cloned().collect()
}

async fn add_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<LabelsBody>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    let mut l = custom_labels(&r);
    l.extend(checked_labels(b, false)?);
    store_labels(&s, &e, &r, l).await
}

async fn set_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<LabelsBody>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    store_labels(&s, &e, &r, checked_labels(b, true)?).await
}

async fn remove_custom_labels(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    store_labels(&s, &e, &r, vec![]).await
}

async fn remove_label(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id, name)): Path<(String, i64, String)>) -> ApiResult<Json<Value>> {
    let r = load_runner(&s, &e, id).await?;
    let l = custom_labels(&r);
    if !l.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
        return Err(ApiError::not_found());
    }
    store_labels(&s, &e, &r, l.into_iter().filter(|x| !x.eq_ignore_ascii_case(&name)).collect()).await
}
