//! Code scanning: SARIF 2.1.0 uploads processed into analyses and alerts.
//!
//! `POST .../code-scanning/sarifs` takes gzip+base64 SARIF (at most 10 MiB compressed,
//! 100 MiB uncompressed, 20 runs, 25000 results per run), stores the file in object storage
//! (`advsec/sarif/{repo}/{id}.sarif.gz`) and queues `advsec.sarif`. Processing creates one
//! *analysis* per run, keyed into a set by (ref, tool, category; category =
//! `automationDetails.id` up to its last `/`). Results are deduplicated into *alerts* by
//! (tool, rule, fingerprint): `partialFingerprints.primaryLocationLineHash` when present,
//! otherwise forge hashes the rule, path and the source line at the reported commit. In each
//! set, the latest analysis decides: an alert it contains is open there, one it lacks is fixed.
//! An alert is `open` while any set has it open, `fixed` otherwise, `dismissed` once
//! dismissed. Deleting an analysis (only the latest of a set is deletable; the last one of a
//! set needs `confirm_delete`) re-evaluates the set; alerts without any result are removed.
use std::collections::HashMap;
use std::io::Read;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use chrono::{DateTime, Utc};
use object_store::{path::Path as ObjPath, ObjectStoreExt, PutPayload};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::requests::{self, Kind};
use super::{CursorParams, Product};
use crate::auth::AuthUser;
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/code-scanning";
    Router::new()
        .route(&format!("{r}/alerts"), get(repo_list))
        .route(&format!("{r}/alerts/{{alert_number}}"), get(repo_get).patch(repo_update))
        .route(&format!("{r}/alerts/{{alert_number}}/instances"), get(instances))
        .route(&format!("{r}/analyses"), get(analyses))
        .route(&format!("{r}/analyses/{{analysis_id}}"), get(analysis_get).delete(analysis_delete))
        .route(&format!("{r}/default-setup"), get(default_get).patch(default_update))
        .route(&format!("{r}/sarifs"), post(upload))
        .route(&format!("{r}/sarifs/{{sarif_id}}"), get(sarif_status))
        .route("/orgs/{org}/code-scanning/alerts", get(org_list))
        .route("/enterprises/{enterprise}/code-scanning/alerts", get(ent_list))
}

const MAX_COMPRESSED: usize = 10 * 1024 * 1024;
const MAX_UNCOMPRESSED: u64 = 100 * 1024 * 1024;
const MAX_RUNS: usize = 20;
const MAX_RESULTS: usize = 25_000;
pub const DISMISS_REASONS: &[&str] = &["false positive", "won't fix", "used in tests", "mitigated"];

fn storage_key(repo_id: i64, id: &str) -> ObjPath {
    ObjPath::from(format!("advsec/sarif/{repo_id}/{id}.sarif.gz"))
}

// ---------------------------------------------------------------------------------------------
// Upload

#[derive(Deserialize)]
struct UploadBody {
    commit_sha: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    sarif: Option<String>,
    checkout_uri: Option<String>,
    started_at: Option<String>,
    tool_name: Option<String>,
}

fn gunzip_bounded(data: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(MAX_UNCOMPRESSED + 1)
        .read_to_end(&mut out)
        .map_err(|_| ApiError::bad_request("Invalid SARIF: the sarif field must be gzip compressed and base64 encoded."))?;
    if out.len() as u64 > MAX_UNCOMPRESSED {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "The SARIF file is too large."));
    }
    Ok(out)
}

fn valid_ref(r: &str) -> bool {
    (r.starts_with("refs/heads/") || r.starts_with("refs/tags/") || (r.starts_with("refs/pull/") && (r.ends_with("/merge") || r.ends_with("/head"))))
        && crate::git::read::safe_rev(r)
}

/// Validates and stores an upload; returns the sarif id.
pub async fn upload_sarif(s: &AppState, ctx: &RepoCtx, actor: &User, commit_sha: &str, git_ref: &str, sarif_b64: &str, tool_name: Option<&str>, checkout_uri: Option<&str>, started_at: Option<DateTime<Utc>>) -> ApiResult<String> {
    if !crate::git::read::is_sha(commit_sha) || commit_sha.len() != 40 {
        return Err(ApiError::validation("Sarif", "commit_sha", "invalid"));
    }
    if !valid_ref(git_ref) {
        return Err(ApiError::validation("Sarif", "ref", "invalid"));
    }
    let gz = base64::engine::general_purpose::STANDARD
        .decode(sarif_b64.trim())
        .map_err(|_| ApiError::bad_request("Invalid SARIF: the sarif field must be gzip compressed and base64 encoded."))?;
    if gz.len() > MAX_COMPRESSED {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "The SARIF file is too large (10 MB compressed maximum)."));
    }
    let raw = gunzip_bounded(&gz)?;
    let doc: Value = serde_json::from_slice(&raw).map_err(|e| ApiError::bad_request(format!("Invalid SARIF: {e}")))?;
    if doc.get("version").and_then(Value::as_str) != Some("2.1.0") {
        return Err(ApiError::bad_request("Invalid SARIF: only SARIF version 2.1.0 is supported."));
    }
    let runs = doc.get("runs").and_then(Value::as_array).ok_or_else(|| ApiError::bad_request("Invalid SARIF: runs is required."))?;
    if runs.len() > MAX_RUNS {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, format!("The SARIF file has more than {MAX_RUNS} runs.")));
    }
    if runs.iter().any(|r| r.get("results").and_then(Value::as_array).is_some_and(|x| x.len() > MAX_RESULTS)) {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, format!("A run has more than {MAX_RESULTS} results.")));
    }
    let id = uuid::Uuid::new_v4().to_string();
    s.store.put(&storage_key(ctx.repo.id, &id), PutPayload::from(gz)).await?;
    sqlx::query(
        "INSERT INTO code_scanning_sarifs (id, repo_id, commit_sha, ref, tool_name, checkout_uri, started_at, storage_key, uploader_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(&id)
    .bind(ctx.repo.id)
    .bind(commit_sha.to_lowercase())
    .bind(git_ref)
    .bind(tool_name)
    .bind(checkout_uri)
    .bind(started_at)
    .bind(storage_key(ctx.repo.id, &id).to_string())
    .bind(actor.id)
    .execute(&s.db)
    .await?;
    super::enqueue(s, super::Q_SARIF, json!({ "repo_id": ctx.repo.id, "sarif_id": id })).await;
    Ok(id)
}

async fn upload(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<UploadBody>) -> ApiResult<Response> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, true).await?;
    if ctx.repo.archived {
        return Err(ApiError::forbidden("Repository was archived so is read-only."));
    }
    let missing = |f: &str| ApiError::validation("Sarif", f, "missing_field");
    let commit = b.commit_sha.ok_or_else(|| missing("commit_sha"))?;
    let git_ref = b.git_ref.ok_or_else(|| missing("ref"))?;
    let sarif = b.sarif.ok_or_else(|| missing("sarif"))?;
    let started = match b.started_at.as_deref() {
        Some(x) => Some(DateTime::parse_from_rfc3339(x).map_err(|_| ApiError::validation("Sarif", "started_at", "invalid"))?.with_timezone(&Utc)),
        None => None,
    };
    let id = upload_sarif(&s, &ctx, &u, &commit, &git_ref, &sarif, b.tool_name.as_deref(), b.checkout_uri.as_deref(), started).await?;
    let url = format!("{}/repos/{}/code-scanning/sarifs/{id}", s.api_url(), ctx.full_name());
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": id, "url": url }))).into_response())
}

async fn sarif_status(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, false).await?;
    let row: Option<(String, Option<Value>)> = sqlx::query_as("SELECT status, errors FROM code_scanning_sarifs WHERE id = $1 AND repo_id = $2")
        .bind(&id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?;
    let (status, errors) = row.ok_or_else(ApiError::not_found)?;
    let analyses_url = (status == "complete").then(|| format!("{}/repos/{}/code-scanning/analyses?sarif_id={id}", s.api_url(), ctx.full_name()));
    Ok(Json(json!({ "processing_status": status, "analyses_url": analyses_url, "errors": errors })))
}

// ---------------------------------------------------------------------------------------------
// Processing

pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let repo_id = payload["repo_id"].as_i64().unwrap_or(0);
    let id = payload["sarif_id"].as_str().unwrap_or("").to_string();
    let Ok(ctx) = super::system_ctx(&state.db, repo_id).await else { return Ok(()) };
    match process(state, &ctx, &id).await {
        Ok(()) => {
            sqlx::query("UPDATE code_scanning_sarifs SET status = 'complete', errors = NULL WHERE id = $1").bind(&id).execute(&state.db).await?;
        }
        Err(e) => {
            sqlx::query("UPDATE code_scanning_sarifs SET status = 'failed', errors = $2 WHERE id = $1")
                .bind(&id)
                .bind(json!([format!("{e:#}")]))
                .execute(&state.db)
                .await?;
        }
    }
    Ok(())
}

fn text(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(Value::as_str).map(str::to_string)
}

fn security_level(score: Option<f64>) -> Option<&'static str> {
    let s = score?;
    Some(if s >= 9.0 {
        "critical"
    } else if s >= 7.0 {
        "high"
    } else if s >= 4.0 {
        "medium"
    } else {
        "low"
    })
}

fn rule_meta(rule: &Value, level: &str) -> Value {
    let props = rule.get("properties").cloned().unwrap_or(json!({}));
    let score = props.get("security-severity").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|x| x.parse().ok())));
    let tags: Vec<Value> = props.get("tags").and_then(Value::as_array).cloned().unwrap_or_default();
    json!({
        "name": text(rule, "/name").or_else(|| text(rule, "/id")).unwrap_or_default(),
        "severity": level,
        "security_severity_level": security_level(score),
        "description": text(rule, "/shortDescription/text").unwrap_or_default(),
        "full_description": text(rule, "/fullDescription/text").unwrap_or_default(),
        "tags": tags,
        "help": text(rule, "/help/markdown").or_else(|| text(rule, "/help/text")),
        "help_uri": text(rule, "/helpUri"),
    })
}

fn level_name(l: &str) -> &'static str {
    match l {
        "error" => "error",
        "note" => "note",
        "none" => "none",
        _ => "warning",
    }
}

async fn process(state: &AppState, ctx: &RepoCtx, id: &str) -> anyhow::Result<()> {
    let row: (String, String, Option<String>, String) =
        sqlx::query_as("SELECT commit_sha, ref, tool_name, storage_key FROM code_scanning_sarifs WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let (commit, git_ref, tool_override, key) = row;
    let gz = state.store.get(&ObjPath::from(key)).await?.bytes().await?;
    let raw = gunzip_bounded(&gz).map_err(|e| anyhow::anyhow!(e.message))?;
    let doc: Value = serde_json::from_slice(&raw)?;
    let repo_path = state.git.sync(ctx.repo.id).await.ok();
    let mut file_cache: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut created: Vec<i64> = Vec::new();
    for (run_index, run) in doc["runs"].as_array().into_iter().flatten().enumerate() {
        let driver = run.pointer("/tool/driver").cloned().unwrap_or(json!({}));
        let tool_name = tool_override.clone().or_else(|| text(&driver, "/name")).unwrap_or_else(|| "unknown".into());
        let tool_version = text(&driver, "/semanticVersion").or_else(|| text(&driver, "/version"));
        let tool_guid = text(&driver, "/guid");
        let mut rules: Vec<Value> = driver.get("rules").and_then(Value::as_array).cloned().unwrap_or_default();
        for ext in run.pointer("/tool/extensions").and_then(Value::as_array).into_iter().flatten() {
            rules.extend(ext.get("rules").and_then(Value::as_array).cloned().unwrap_or_default());
        }
        let category = text(run, "/automationDetails/id")
            .map(|c| match c.rfind('/') {
                Some(i) => c[..=i].to_string(),
                None => c,
            })
            .unwrap_or_default();
        let results = run.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
        let analysis_id: i64 = sqlx::query_scalar(
            "INSERT INTO code_scanning_analyses (repo_id, sarif_id, run_index, ref, commit_sha, analysis_key, environment, category, tool_name,
                                                 tool_version, tool_guid, results_count, rules_count)
             VALUES ($1,$2,$3,$4,$5,$6,'{}',$7,$8,$9,$10,$11,$12) RETURNING id",
        )
        .bind(ctx.repo.id)
        .bind(id)
        .bind(run_index as i32)
        .bind(&git_ref)
        .bind(&commit)
        .bind(".github/workflows/code-scanning-upload:analyze")
        .bind(&category)
        .bind(&tool_name)
        .bind(&tool_version)
        .bind(&tool_guid)
        .bind(results.len() as i32)
        .bind(rules.len() as i32)
        .fetch_one(&state.db)
        .await?;
        let mut seen: HashMap<String, usize> = HashMap::new();
        for res in &results {
            let rule_idx = res.get("ruleIndex").or_else(|| res.pointer("/rule/index")).and_then(Value::as_u64).map(|i| i as usize);
            let rule_id = text(res, "/ruleId")
                .or_else(|| text(res, "/rule/id"))
                .or_else(|| rule_idx.and_then(|i| rules.get(i)).and_then(|r| text(r, "/id")))
                .unwrap_or_else(|| "unknown".into());
            let rule = rule_idx.and_then(|i| rules.get(i)).or_else(|| rules.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(&rule_id))).cloned().unwrap_or(json!({}));
            let level = level_name(&text(res, "/level").or_else(|| text(&rule, "/defaultConfiguration/level")).unwrap_or_else(|| "warning".into())).to_string();
            let loc = res.pointer("/locations/0/physicalLocation").cloned().unwrap_or(json!({}));
            let path = text(&loc, "/artifactLocation/uri").unwrap_or_default();
            let path = path.strip_prefix("file://").unwrap_or(&path).trim_start_matches("./").to_string();
            let line = |k: &str| loc.pointer(&format!("/region/{k}")).and_then(Value::as_i64).map(|v| v as i32);
            let start_line = line("startLine");
            let start_col = line("startColumn").or(Some(1));
            let end_line = line("endLine").or(start_line);
            let end_col = line("endColumn").or(start_col);
            let message = text(res, "/message/text").unwrap_or_default();
            let fingerprint = match text(res, "/partialFingerprints/primaryLocationLineHash")
                .or_else(|| res.get("fingerprints").and_then(Value::as_object).and_then(|m| m.values().next()).and_then(|v| v.as_str().map(str::to_string)))
            {
                Some(f) => f,
                None => {
                    let src = match (&repo_path, start_line) {
                        (Some(p), Some(l)) if !path.is_empty() => {
                            let lines = match file_cache.get(&path) {
                                Some(x) => x.clone(),
                                None => {
                                    let rev = format!("{commit}:{path}");
                                    let got = crate::git::read::cat_object(&state.git.git, p, &rev).await.ok().flatten().map(|o| {
                                        String::from_utf8_lossy(&o.data).lines().map(str::to_string).collect::<Vec<_>>()
                                    });
                                    file_cache.insert(path.clone(), got.clone());
                                    got
                                }
                            };
                            lines.and_then(|ls| ls.get((l - 1).max(0) as usize).map(|x| x.trim().to_string()))
                        }
                        _ => None,
                    };
                    let basis = format!("{rule_id}\n{path}\n{}", src.unwrap_or_else(|| format!("{message}\n{}", start_line.unwrap_or(0))));
                    let n = seen.entry(basis.clone()).or_insert(0);
                    *n += 1;
                    format!("{}:{}", &hex::encode(Sha256::digest(basis.as_bytes()))[..16], n)
                }
            };
            let meta = rule_meta(&rule, &level);
            let alert_id: i64 = match sqlx::query_scalar::<_, i64>(
                "UPDATE code_scanning_alerts SET rule = $5, tool_version = $6, tool_guid = $7, updated_at = updated_at
                  WHERE repo_id = $1 AND tool_name = $2 AND rule_id = $3 AND fingerprint = $4 RETURNING id",
            )
            .bind(ctx.repo.id)
            .bind(&tool_name)
            .bind(&rule_id)
            .bind(&fingerprint)
            .bind(&meta)
            .bind(&tool_version)
            .bind(&tool_guid)
            .fetch_optional(&state.db)
            .await?
            {
                Some(a) => a,
                None => {
                    let mut tx = state.db.begin().await?;
                    let number = super::next_number(&mut *tx, ctx.repo.id, "code_scanning").await.map_err(|e| anyhow::anyhow!(e.message))?;
                    let a: i64 = sqlx::query_scalar(
                        "INSERT INTO code_scanning_alerts (repo_id, number, tool_name, tool_version, tool_guid, rule_id, rule, fingerprint)
                         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id",
                    )
                    .bind(ctx.repo.id)
                    .bind(number)
                    .bind(&tool_name)
                    .bind(&tool_version)
                    .bind(&tool_guid)
                    .bind(&rule_id)
                    .bind(&meta)
                    .bind(&fingerprint)
                    .fetch_one(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    created.push(a);
                    a
                }
            };
            let classes: Vec<String> = if path.split('/').any(|seg| seg == "test" || seg == "tests" || seg == "__tests__") { vec!["test".into()] } else { vec![] };
            sqlx::query(
                "INSERT INTO code_scanning_results (analysis_id, alert_id, message, path, start_line, end_line, start_column, end_column, classifications)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING",
            )
            .bind(analysis_id)
            .bind(alert_id)
            .bind(&message)
            .bind(&path)
            .bind(start_line)
            .bind(end_line)
            .bind(start_col)
            .bind(end_col)
            .bind(&classes)
            .execute(&state.db)
            .await?;
        }
        recompute_set(state, ctx, &git_ref, &tool_name, &category).await?;
    }
    for id in created {
        emit_webhook(state, ctx, id, "created").await;
    }
    Ok(())
}

/// Re-evaluates the states of alerts that have results in a (ref, tool, category) set.
async fn recompute_set(state: &AppState, ctx: &RepoCtx, git_ref: &str, tool: &str, category: &str) -> anyhow::Result<()> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT x.alert_id FROM code_scanning_results x JOIN code_scanning_analyses a ON a.id = x.analysis_id
          WHERE a.repo_id = $1 AND a.ref = $2 AND a.tool_name = $3 AND a.category = $4",
    )
    .bind(ctx.repo.id)
    .bind(git_ref)
    .bind(tool)
    .bind(category)
    .fetch_all(&state.db)
    .await?;
    recompute_alerts(state, ctx, &ids).await
}

/// An alert is open while the latest analysis of any set it appeared in still has it.
async fn recompute_alerts(state: &AppState, ctx: &RepoCtx, ids: &[i64]) -> anyhow::Result<()> {
    let open: Vec<i64> = sqlx::query_scalar(
        "WITH latest AS (
             SELECT DISTINCT ON (ref, tool_name, category) id FROM code_scanning_analyses WHERE repo_id = $1
              ORDER BY ref, tool_name, category, id DESC)
         SELECT DISTINCT x.alert_id FROM code_scanning_results x WHERE x.analysis_id IN (SELECT id FROM latest) AND x.alert_id = ANY($2)",
    )
    .bind(ctx.repo.id)
    .bind(ids)
    .fetch_all(&state.db)
    .await?;
    let before: Vec<(i64, String)> = sqlx::query_as("SELECT id, state FROM code_scanning_alerts WHERE id = ANY($1)").bind(ids).fetch_all(&state.db).await?;
    for (id, st) in before {
        if st == "dismissed" {
            continue;
        }
        let new = if open.contains(&id) { "open" } else { "fixed" };
        if new != st {
            sqlx::query("UPDATE code_scanning_alerts SET state = $2, fixed_at = CASE WHEN $2 = 'fixed' THEN now() END, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(new)
                .execute(&state.db)
                .await?;
            emit_webhook(state, ctx, id, if new == "fixed" { "fixed" } else { "reopened" }).await;
        }
    }
    // Brand-new alerts (created open) get a `created` event once.
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Alerts

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlertRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub tool_name: String,
    pub tool_version: Option<String>,
    pub tool_guid: Option<String>,
    pub rule_id: String,
    pub rule: Value,
    pub fingerprint: String,
    pub state: String,
    pub fixed_at: Option<DateTime<Utc>>,
    pub dismissed_at: Option<DateTime<Utc>>,
    pub dismissed_by: Option<i64>,
    pub dismissed_reason: Option<String>,
    pub dismissed_comment: Option<String>,
    pub dismissal_approved_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One instance (per analysis set) of an alert.
#[derive(Debug, Clone, sqlx::FromRow)]
struct InstanceRow {
    alert_id: i64,
    #[sqlx(rename = "ref")]
    git_ref: String,
    analysis_key: String,
    environment: String,
    category: String,
    commit_sha: String,
    open: bool,
    message: String,
    path: String,
    start_line: Option<i32>,
    end_line: Option<i32>,
    start_column: Option<i32>,
    end_column: Option<i32>,
    classifications: Vec<String>,
}

async fn instance_rows(s: &AppState, repo_ids: &[i64], alert_ids: &[i64]) -> ApiResult<Vec<InstanceRow>> {
    Ok(sqlx::query_as(
        "WITH sets AS (
             SELECT DISTINCT ON (repo_id, ref, tool_name, category) id, repo_id, ref, tool_name, category, commit_sha, analysis_key, environment
               FROM code_scanning_analyses WHERE repo_id = ANY($1) ORDER BY repo_id, ref, tool_name, category, id DESC),
         seen AS (
             SELECT DISTINCT ON (x.alert_id, a.ref, a.tool_name, a.category) x.*, a.ref, a.tool_name, a.category
               FROM code_scanning_results x JOIN code_scanning_analyses a ON a.id = x.analysis_id
              WHERE x.alert_id = ANY($2) ORDER BY x.alert_id, a.ref, a.tool_name, a.category, a.id DESC)
         SELECT seen.alert_id, seen.ref, st.analysis_key, st.environment, seen.category, st.commit_sha,
                (seen.analysis_id = st.id) AS open, seen.message, seen.path, seen.start_line, seen.end_line, seen.start_column, seen.end_column,
                seen.classifications, st.id AS analysis_id
           FROM seen JOIN sets st ON st.ref = seen.ref AND st.tool_name = seen.tool_name AND st.category = seen.category
          ORDER BY st.id DESC",
    )
    .bind(repo_ids)
    .bind(alert_ids)
    .fetch_all(&s.db)
    .await?)
}

fn instance_json(s: &AppState, full: &str, number: i64, i: &InstanceRow, dismissed: bool) -> Value {
    let state = if dismissed { "dismissed" } else if i.open { "open" } else { "fixed" };
    json!({
        "ref": i.git_ref,
        "analysis_key": i.analysis_key,
        "environment": i.environment,
        "category": i.category,
        "state": state,
        "commit_sha": i.commit_sha,
        "message": { "text": i.message },
        "location": { "path": i.path, "start_line": i.start_line, "end_line": i.end_line, "start_column": i.start_column, "end_column": i.end_column },
        "html_url": format!("{}/{full}/security/code-scanning/{number}?ref={}", s.base_url(), i.git_ref),
        "classifications": i.classifications,
    })
}

pub async fn render(s: &AppState, rows: &[AlertRow], with_repo: bool) -> ApiResult<Vec<Value>> {
    let repos = super::repos_by_id(&s.db, rows.iter().map(|r| r.repo_id)).await?;
    let repo_ids: Vec<i64> = repos.keys().copied().collect();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let inst = instance_rows(s, &repo_ids, &ids).await?;
    let assignees: Vec<(i64, i64)> = sqlx::query_as("SELECT alert_id, user_id FROM code_scanning_alert_assignees WHERE alert_id = ANY($1) ORDER BY user_id")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let users = super::users_by_id(&s.db, rows.iter().flat_map(|r| [r.dismissed_by, r.dismissal_approved_by]).flatten().chain(assignees.iter().map(|a| a.1))).await?;
    let mut out = Vec::new();
    for r in rows {
        let Some(ctx) = repos.get(&r.repo_id) else { continue };
        let full = ctx.full_name();
        let url = format!("{}/repos/{full}/code-scanning/alerts/{}", s.api_url(), r.number);
        let default_ref = format!("refs/heads/{}", ctx.repo.default_branch);
        let mine: Vec<&InstanceRow> = inst.iter().filter(|i| i.alert_id == r.id).collect();
        let recent = mine.iter().find(|i| i.git_ref == default_ref).or_else(|| mine.first()).copied();
        let mut rule = r.rule.clone();
        rule["id"] = json!(r.rule_id);
        let mut v = json!({
            "number": r.number,
            "created_at": crate::json::ts(r.created_at),
            "updated_at": crate::json::ts(r.updated_at),
            "url": url,
            "html_url": format!("{}/{full}/security/code-scanning/{}", s.base_url(), r.number),
            "instances_url": format!("{url}/instances"),
            "state": r.state,
            "fixed_at": crate::json::ts_opt(r.fixed_at),
            "dismissed_by": super::user_json(s, &users, r.dismissed_by),
            "dismissed_at": crate::json::ts_opt(r.dismissed_at),
            "dismissed_reason": r.dismissed_reason,
            "dismissed_comment": r.dismissed_comment,
            "rule": rule,
            "tool": { "name": r.tool_name, "version": r.tool_version, "guid": r.tool_guid },
            "most_recent_instance": recent.map(|i| instance_json(s, &full, r.number, i, r.state == "dismissed")).unwrap_or(Value::Null),
            "dismissal_approved_by": super::user_json(s, &users, r.dismissal_approved_by),
            "assignees": assignees.iter().filter(|a| a.0 == r.id).map(|a| super::user_json(s, &users, Some(a.1))).collect::<Vec<_>>(),
        });
        if with_repo {
            v["repository"] = super::repo_json(s, ctx);
        }
        out.push(v);
    }
    Ok(out)
}

#[derive(Deserialize, Default)]
struct ListQ {
    tool_name: Option<String>,
    tool_guid: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    #[serde(default, deserialize_with = "super::qs::opt_i64")]
    pr: Option<i64>,
    direction: Option<String>,
    sort: Option<String>,
    state: Option<String>,
    severity: Option<String>,
    assignees: Option<String>,
    #[serde(flatten)]
    cursor: CursorParams,
}

fn full_ref(r: &str) -> String {
    if r.starts_with("refs/") { r.to_string() } else { format!("refs/heads/{r}") }
}

async fn list(s: &AppState, uri: &OriginalUri, repo_scope: &str, scope_id: i64, q: ListQ, with_repo: bool) -> ApiResult<Response> {
    super::check_enum("CodeScanningAlert", "state", &q.state, &["open", "closed", "dismissed", "fixed"])?;
    super::check_enum("CodeScanningAlert", "sort", &q.sort, &["created", "updated"])?;
    super::check_enum("CodeScanningAlert", "severity", &q.severity, &["critical", "high", "medium", "low", "warning", "note", "error"])?;
    let states: Option<Vec<String>> = q.state.as_deref().map(|st| match st {
        "closed" => vec!["dismissed".into(), "fixed".into()],
        other => vec![other.to_string()],
    });
    let refs: Option<Vec<String>> = match (q.pr, &q.git_ref) {
        (Some(pr), _) => Some(vec![format!("refs/pull/{pr}/merge"), format!("refs/pull/{pr}/head")]),
        (None, Some(r)) => Some(vec![full_ref(r)]),
        _ => None,
    };
    let order = if q.sort.as_deref() == Some("updated") { "c.updated_at" } else { "c.created_at" };
    let d = super::dir(q.direction.as_deref());
    let rows: Vec<AlertRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT c.* FROM code_scanning_alerts c WHERE c.repo_id IN {repo_scope}
            AND ($2::text IS NULL OR c.tool_name = $2) AND ($3::text IS NULL OR c.tool_guid = $3)
            AND ($4::text[] IS NULL OR c.state = ANY($4))
            AND ($5::text IS NULL OR c.rule->>'security_severity_level' = $5 OR c.rule->>'severity' = $5)
            AND ($6::text[] IS NULL OR EXISTS(SELECT 1 FROM code_scanning_results x JOIN code_scanning_analyses a ON a.id = x.analysis_id
                                               WHERE x.alert_id = c.id AND a.ref = ANY($6)))
            AND ($7::text IS NULL OR ($7 = 'none' AND NOT EXISTS(SELECT 1 FROM code_scanning_alert_assignees x WHERE x.alert_id = c.id))
                 OR ($7 = '*' AND EXISTS(SELECT 1 FROM code_scanning_alert_assignees x WHERE x.alert_id = c.id))
                 OR EXISTS(SELECT 1 FROM code_scanning_alert_assignees x JOIN users u ON u.id = x.user_id
                            WHERE x.alert_id = c.id AND lower(u.login) = ANY(string_to_array(lower($7), ','))))
          ORDER BY {order} {d}, c.id {d}"
    )))
    .bind(scope_id)
    .bind(&q.tool_name)
    .bind(&q.tool_guid)
    .bind(&states)
    .bind(&q.severity)
    .bind(&refs)
    .bind(&q.assignees)
    .fetch_all(&s.db)
    .await?;
    let total = rows.len() as i64;
    let page: Vec<AlertRow> = rows.into_iter().skip(q.cursor.offset() as usize).take(q.cursor.per_page() as usize).collect();
    let items = render(s, &page, with_repo).await?;
    Ok(super::list_response(s, uri, &q.cursor, total, items))
}

async fn repo_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): Path<(String, String)>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, false).await?;
    list(&s, &uri, "(SELECT $1::bigint)", ctx.repo.id, q, false).await
}

async fn org_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let o = super::org_for(&s, &u, &org, Product::CodeScanning).await?;
    list(&s, &uri, "(SELECT id FROM repositories WHERE owner_id = $1)", o.id, q, true).await
}

async fn ent_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(e): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    list(&s, &uri, "(SELECT r.id FROM repositories r JOIN users o ON o.id = r.owner_id WHERE o.kind = 'Organization' AND $1 = 0)", 0, q, true).await
}

pub async fn alert_row(s: &AppState, repo_id: i64, n: i64) -> ApiResult<AlertRow> {
    sqlx::query_as("SELECT * FROM code_scanning_alerts WHERE repo_id = $1 AND number = $2")
        .bind(repo_id)
        .bind(n)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn repo_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, false).await?;
    let row = alert_row(&s, ctx.repo.id, n).await?;
    Ok(Json(render(&s, &[row], false).await?.remove(0)))
}

#[derive(Deserialize)]
struct UpdateBody {
    state: Option<String>,
    dismissed_reason: Option<String>,
    dismissed_comment: Option<String>,
    create_request: Option<bool>,
    assignees: Option<Vec<String>>,
}

pub async fn dismiss(s: &AppState, ctx: &RepoCtx, n: i64, reason: &str, comment: Option<&str>, by: &User, approved_by: Option<&User>) -> ApiResult<()> {
    sqlx::query(
        "UPDATE code_scanning_alerts SET state = 'dismissed', dismissed_at = now(), dismissed_by = $3, dismissed_reason = $4,
                dismissed_comment = $5, dismissal_approved_by = $6, updated_at = now() WHERE repo_id = $1 AND number = $2",
    )
    .bind(ctx.repo.id)
    .bind(n)
    .bind(by.id)
    .bind(reason)
    .bind(comment)
    .bind(approved_by.map(|u| u.id))
    .execute(&s.db)
    .await?;
    Ok(())
}

pub async fn apply_dismissal(s: &AppState, ctx: &RepoCtx, n: i64, requester: &User, reviewer: &User, req: &requests::RequestRow) -> ApiResult<()> {
    let reason = req.reason.clone().unwrap_or_else(|| "false positive".into());
    dismiss(s, ctx, n, &reason, req.requester_comment.as_deref(), requester, Some(reviewer)).await
}

async fn repo_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<UpdateBody>) -> ApiResult<Json<Value>> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, true).await?;
    let row = alert_row(&s, ctx.repo.id, n).await?;
    if b.state.is_none() && b.assignees.is_none() {
        return Err(ApiError::validation("CodeScanningAlert", "state", "missing_field"));
    }
    if let Some(list) = &b.assignees {
        let mut ids = Vec::new();
        for login in list {
            let target = crate::db::user_by_login(&s.db, login).await.map_err(|_| ApiError::validation("CodeScanningAlert", "assignees", "invalid"))?;
            if crate::db::permission(&s.db, Some(&target), &ctx.repo, &ctx.owner).await? < Perm::Write {
                return Err(ApiError::validation("CodeScanningAlert", "assignees", "invalid"));
            }
            ids.push(target.id);
        }
        let mut tx = s.db.begin().await?;
        sqlx::query("DELETE FROM code_scanning_alert_assignees WHERE alert_id = $1").bind(row.id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO code_scanning_alert_assignees (alert_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(row.id)
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE code_scanning_alerts SET updated_at = now() WHERE id = $1").bind(row.id).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    match b.state.as_deref() {
        Some("dismissed") => {
            let reason = b.dismissed_reason.clone().ok_or_else(|| ApiError::validation("CodeScanningAlert", "dismissed_reason", "missing_field"))?;
            if !DISMISS_REASONS.contains(&reason.as_str()) {
                return Err(ApiError::validation("CodeScanningAlert", "dismissed_reason", "invalid"));
            }
            if b.dismissed_comment.as_ref().is_some_and(|c| c.chars().count() > 280) {
                return Err(ApiError::validation("CodeScanningAlert", "dismissed_comment", "invalid"));
            }
            let delegated = requests::delegated(&s, ctx.repo.id, Kind::CodeDismissal).await;
            if b.create_request == Some(true) {
                if !delegated {
                    return Err(ApiError::unprocessable("Delegated alert dismissal is not enabled for this repository."));
                }
                let data = json!([{ "reason": reason, "alert_number": n.to_string(), "pr_review_thread_id": "" }]);
                requests::create(&s, &ctx, Kind::CodeDismissal, Some(n), &u, Some(&reason), b.dismissed_comment.as_deref(), data, &n.to_string()).await?;
            } else if delegated && !requests::can_review(&s, &ctx, &u, Kind::CodeDismissal).await {
                return Err(ApiError::forbidden("Delegated alert dismissal is enabled for this repository: set create_request to request a dismissal."));
            } else if row.state != "dismissed" {
                dismiss(&s, &ctx, n, &reason, b.dismissed_comment.as_deref(), &u, None).await?;
                emit_webhook(&s, &ctx, row.id, "closed_by_user").await;
            }
        }
        Some("open") => {
            if row.state == "dismissed" {
                sqlx::query(
                    "UPDATE code_scanning_alerts SET state = 'open', dismissed_at = NULL, dismissed_by = NULL, dismissed_reason = NULL,
                            dismissed_comment = NULL, dismissal_approved_by = NULL, updated_at = now() WHERE id = $1",
                )
                .bind(row.id)
                .execute(&s.db)
                .await?;
                recompute_alerts(&s, &ctx, &[row.id]).await?;
                emit_webhook(&s, &ctx, row.id, "reopened_by_user").await;
            }
        }
        Some(_) => return Err(ApiError::validation("CodeScanningAlert", "state", "invalid")),
        None => {}
    }
    let row = alert_row(&s, ctx.repo.id, n).await?;
    Ok(Json(render(&s, &[row], false).await?.remove(0)))
}

#[derive(Deserialize, Default)]
struct InstQ {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    pr: Option<i64>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn instances(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r, n)): Path<(String, String, i64)>, Query(q): Query<InstQ>) -> ApiResult<crate::api::Paged> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, false).await?;
    let row = alert_row(&s, ctx.repo.id, n).await?;
    let p = crate::api::PageParams { page: q.page, per_page: q.per_page };
    let refs: Option<Vec<String>> = match (q.pr, &q.git_ref) {
        (Some(pr), _) => Some(vec![format!("refs/pull/{pr}/merge"), format!("refs/pull/{pr}/head")]),
        (None, Some(x)) => Some(vec![full_ref(x)]),
        _ => None,
    };
    let all = instance_rows(&s, &[ctx.repo.id], &[row.id]).await?;
    let full = ctx.full_name();
    let list: Vec<Value> = all
        .iter()
        .filter(|i| refs.as_ref().is_none_or(|rs| rs.contains(&i.git_ref)))
        .map(|i| {
            let mut v = instance_json(&s, &full, row.number, i, false);
            v["message"] = json!({ "text": i.message });
            v
        })
        .collect();
    let total = list.len() as i64;
    let items = list.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    Ok(crate::api::Paged::new(&uri, &s, p, Some(total), items))
}

// ---------------------------------------------------------------------------------------------
// Analyses

#[derive(Debug, Clone, sqlx::FromRow)]
struct AnalysisRow {
    id: i64,
    sarif_id: String,
    run_index: i32,
    #[sqlx(rename = "ref")]
    git_ref: String,
    commit_sha: String,
    analysis_key: String,
    environment: String,
    category: String,
    tool_name: String,
    tool_version: Option<String>,
    tool_guid: Option<String>,
    results_count: i32,
    rules_count: i32,
    error: String,
    warning: String,
    created_at: DateTime<Utc>,
    deletable: bool,
}

const ANALYSIS_SQL: &str = "SELECT a.id, a.sarif_id, a.run_index, a.ref, a.commit_sha, a.analysis_key, a.environment, a.category, a.tool_name,
        a.tool_version, a.tool_guid, a.results_count, a.rules_count, a.error, a.warning, a.created_at,
        NOT EXISTS(SELECT 1 FROM code_scanning_analyses b WHERE b.repo_id = a.repo_id AND b.ref = a.ref AND b.tool_name = a.tool_name
                    AND b.category = a.category AND b.id > a.id) AS deletable
   FROM code_scanning_analyses a";

fn analysis_json(s: &AppState, full: &str, a: &AnalysisRow) -> Value {
    json!({
        "ref": a.git_ref,
        "commit_sha": a.commit_sha,
        "analysis_key": a.analysis_key,
        "environment": a.environment,
        "category": a.category,
        "error": a.error,
        "created_at": crate::json::ts(a.created_at),
        "results_count": a.results_count,
        "rules_count": a.rules_count,
        "id": a.id,
        "url": format!("{}/repos/{full}/code-scanning/analyses/{}", s.api_url(), a.id),
        "sarif_id": a.sarif_id,
        "tool": { "name": a.tool_name, "version": a.tool_version, "guid": a.tool_guid },
        "deletable": a.deletable,
        "warning": a.warning,
    })
}

#[derive(Deserialize, Default)]
struct AnalysesQ {
    tool_name: Option<String>,
    tool_guid: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    pr: Option<i64>,
    sarif_id: Option<String>,
    direction: Option<String>,
    sort: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn analyses(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): Path<(String, String)>, Query(q): Query<AnalysesQ>) -> ApiResult<crate::api::Paged> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, false).await?;
    super::check_enum("Analysis", "sort", &q.sort, &["created"])?;
    let p = crate::api::PageParams { page: q.page, per_page: q.per_page };
    let refs: Option<Vec<String>> = match (q.pr, &q.git_ref) {
        (Some(pr), _) => Some(vec![format!("refs/pull/{pr}/merge"), format!("refs/pull/{pr}/head")]),
        (None, Some(x)) => Some(vec![full_ref(x)]),
        _ => None,
    };
    let d = super::dir(q.direction.as_deref());
    let filter = "WHERE a.repo_id = $1 AND ($2::text IS NULL OR a.tool_name = $2) AND ($3::text IS NULL OR a.tool_guid = $3)
                  AND ($4::text[] IS NULL OR a.ref = ANY($4)) AND ($5::text IS NULL OR a.sarif_id = $5)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM code_scanning_analyses a {filter}")))
        .bind(ctx.repo.id)
        .bind(&q.tool_name)
        .bind(&q.tool_guid)
        .bind(&refs)
        .bind(&q.sarif_id)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<AnalysisRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ANALYSIS_SQL} {filter} ORDER BY a.id {d} LIMIT $6 OFFSET $7")))
        .bind(ctx.repo.id)
        .bind(&q.tool_name)
        .bind(&q.tool_guid)
        .bind(&refs)
        .bind(&q.sarif_id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let full = ctx.full_name();
    Ok(crate::api::Paged::new(&uri, &s, p, Some(total), rows.iter().map(|a| analysis_json(&s, &full, a)).collect()))
}

async fn analysis_row(s: &AppState, repo_id: i64, id: i64) -> ApiResult<AnalysisRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{ANALYSIS_SQL} WHERE a.repo_id = $1 AND a.id = $2")))
        .bind(repo_id)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn analysis_get(State(s): State<AppState>, AuthUser(u): AuthUser, headers: HeaderMap, Path((o, r, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, false).await?;
    let a = analysis_row(&s, ctx.repo.id, id).await?;
    let wants_sarif = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("application/sarif+json"));
    if wants_sarif {
        let key: String = sqlx::query_scalar("SELECT storage_key FROM code_scanning_sarifs WHERE id = $1").bind(&a.sarif_id).fetch_one(&s.db).await?;
        let gz = s.store.get(&ObjPath::from(key)).await?.bytes().await?;
        let doc: Value = serde_json::from_slice(&gunzip_bounded(&gz)?).map_err(ApiError::internal)?;
        let run = doc.pointer(&format!("/runs/{}", a.run_index)).cloned().unwrap_or(json!({}));
        let body = json!({
            "$schema": doc.get("$schema").cloned().unwrap_or(json!("https://json.schemastore.org/sarif-2.1.0.json")),
            "version": "2.1.0",
            "runs": [run],
        });
        let mut res = (StatusCode::OK, serde_json::to_vec(&body).unwrap_or_default()).into_response();
        res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/sarif+json"));
        return Ok(res);
    }
    Ok(Json(analysis_json(&s, &ctx.full_name(), &a)).into_response())
}

#[derive(Deserialize, Default)]
struct DeleteQ {
    confirm_delete: Option<String>,
}

async fn analysis_delete(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r, id)): Path<(String, String, i64)>, Query(q): Query<DeleteQ>) -> ApiResult<Json<Value>> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, true).await?;
    let a = analysis_row(&s, ctx.repo.id, id).await?;
    if !a.deletable {
        return Err(ApiError::bad_request("Analysis specified is not deletable."));
    }
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM code_scanning_analyses WHERE repo_id = $1 AND ref = $2 AND tool_name = $3 AND category = $4")
        .bind(ctx.repo.id)
        .bind(&a.git_ref)
        .bind(&a.tool_name)
        .bind(&a.category)
        .fetch_one(&s.db)
        .await?;
    let confirmed = q.confirm_delete.is_some() || uri.0.query().is_some_and(|x| x.split('&').any(|p| p == "confirm_delete" || p.starts_with("confirm_delete=")));
    if remaining <= 1 && !confirmed {
        return Err(ApiError::bad_request(
            "Analysis is last of its type and deletion may result in the loss of historical alert data. Please specify confirm_delete.",
        ));
    }
    let alerts: Vec<i64> = sqlx::query_scalar("SELECT alert_id FROM code_scanning_results WHERE analysis_id = $1").bind(id).fetch_all(&s.db).await?;
    sqlx::query("DELETE FROM code_scanning_analyses WHERE id = $1").bind(id).execute(&s.db).await?;
    // Alerts left without any result disappear with their last analysis.
    sqlx::query("DELETE FROM code_scanning_alerts c WHERE c.id = ANY($1) AND NOT EXISTS(SELECT 1 FROM code_scanning_results x WHERE x.alert_id = c.id)")
        .bind(&alerts)
        .execute(&s.db)
        .await?;
    recompute_set(&s, &ctx, &a.git_ref, &a.tool_name, &a.category).await?;
    let next: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM code_scanning_analyses WHERE repo_id = $1 AND ref = $2 AND tool_name = $3 AND category = $4 ORDER BY id DESC LIMIT 1",
    )
    .bind(ctx.repo.id)
    .bind(&a.git_ref)
    .bind(&a.tool_name)
    .bind(&a.category)
    .fetch_optional(&s.db)
    .await?;
    let base = format!("{}/repos/{}/code-scanning/analyses", s.api_url(), ctx.full_name());
    let (next_url, confirm_url) = match next {
        Some(n) if remaining - 1 > 1 => (Some(format!("{base}/{n}")), None),
        Some(n) => (None, Some(format!("{base}/{n}?confirm_delete"))),
        None => (None, None),
    };
    Ok(Json(json!({ "next_analysis_url": next_url, "confirm_delete_url": confirm_url })))
}

// ---------------------------------------------------------------------------------------------
// Default setup

const LANGS: &[&str] = &["actions", "c-cpp", "csharp", "go", "java-kotlin", "javascript-typescript", "python", "ruby", "swift"];

/// CodeQL languages detected from the repository's languages.
async fn detected_languages(s: &AppState, ctx: &RepoCtx) -> Vec<String> {
    let refname = format!("refs/heads/{}", ctx.repo.default_branch);
    let langs: Vec<String> = async {
        let (_, refs) = s.git.refs(ctx.repo.id).await.ok()?;
        let sha = refs.get(&refname)?.clone();
        let path = s.git.sync(ctx.repo.id).await.ok()?;
        let entries = crate::git::read::ls_tree(&s.git.git, &path, &sha, None, true).await.ok()?;
        let by = crate::git::stats::language_bytes(entries.iter().filter(|e| e.kind == "blob").map(|e| (e.path.as_str(), e.size.unwrap_or(0) as u64)));
        Some(by.into_keys().map(str::to_string).collect())
    }
    .await
    .unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for l in langs {
        let m = match l.as_str() {
            "C" | "C++" => "c-cpp",
            "C#" => "csharp",
            "Go" => "go",
            "Java" | "Kotlin" => "java-kotlin",
            "JavaScript" | "TypeScript" => "javascript-typescript",
            "Python" => "python",
            "Ruby" => "ruby",
            "Swift" => "swift",
            _ => continue,
        };
        if !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    }
    out.sort();
    out
}

#[derive(sqlx::FromRow)]
struct SetupRow {
    state: String,
    languages: Vec<String>,
    runner_type: Option<String>,
    runner_label: Option<String>,
    query_suite: String,
    threat_model: String,
    updated_at: Option<DateTime<Utc>>,
}

async fn default_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::CodeScanning, true).await?;
    let row: Option<SetupRow> = sqlx::query_as("SELECT state, languages, runner_type, runner_label, query_suite, threat_model, updated_at FROM code_scanning_default_setup WHERE repo_id = $1")
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?;
    let row = match row {
        Some(r) => r,
        None => SetupRow { state: "not-configured".into(), languages: detected_languages(&s, &ctx).await, runner_type: None, runner_label: None, query_suite: "default".into(), threat_model: "remote".into(), updated_at: None },
    };
    let configured = row.state == "configured";
    Ok(Json(json!({
        "state": row.state,
        "languages": row.languages,
        "runner_type": row.runner_type,
        "runner_label": row.runner_label,
        "query_suite": row.query_suite,
        "threat_model": row.threat_model,
        "updated_at": crate::json::ts_opt(row.updated_at),
        "schedule": if configured { json!("weekly") } else { Value::Null },
    })))
}

#[derive(Deserialize)]
struct SetupBody {
    state: Option<String>,
    runner_type: Option<String>,
    runner_label: Option<String>,
    query_suite: Option<String>,
    threat_model: Option<String>,
    languages: Option<Vec<String>>,
}

async fn default_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<SetupBody>) -> ApiResult<Response> {
    let ctx = crate::db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Admin)?;
    let check = |f: &str, v: &Option<String>, ok: &[&str]| -> ApiResult<()> {
        if v.as_ref().is_some_and(|x| !ok.contains(&x.as_str())) { Err(ApiError::validation("DefaultSetup", f, "invalid")) } else { Ok(()) }
    };
    check("state", &b.state, &["configured", "not-configured"])?;
    check("runner_type", &b.runner_type, &["standard", "labeled"])?;
    check("query_suite", &b.query_suite, &["default", "extended"])?;
    check("threat_model", &b.threat_model, &["remote", "remote_and_local"])?;
    if let Some(ls) = &b.languages {
        if ls.iter().any(|l| !LANGS.contains(&l.as_str())) {
            return Err(ApiError::validation("DefaultSetup", "languages", "invalid"));
        }
    }
    if b.runner_type.as_deref() == Some("labeled") && b.runner_label.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::validation("DefaultSetup", "runner_label", "missing_field"));
    }
    let languages = match &b.languages {
        Some(l) => l.clone(),
        None => detected_languages(&s, &ctx).await,
    };
    sqlx::query(
        "INSERT INTO code_scanning_default_setup (repo_id, state, languages, runner_type, runner_label, query_suite, threat_model, updated_at)
         VALUES ($1, COALESCE($2, 'not-configured'), $3, $4, $5, COALESCE($6, 'default'), COALESCE($7, 'remote'), now())
         ON CONFLICT (repo_id) DO UPDATE SET state = COALESCE($2, code_scanning_default_setup.state), languages = $3,
             runner_type = COALESCE($4, code_scanning_default_setup.runner_type),
             runner_label = CASE WHEN $4 = 'standard' THEN NULL ELSE COALESCE($5, code_scanning_default_setup.runner_label) END,
             query_suite = COALESCE($6, code_scanning_default_setup.query_suite), threat_model = COALESCE($7, code_scanning_default_setup.threat_model),
             updated_at = now()",
    )
    .bind(ctx.repo.id)
    .bind(&b.state)
    .bind(&languages)
    .bind(&b.runner_type)
    .bind(&b.runner_label)
    .bind(&b.query_suite)
    .bind(&b.threat_model)
    .execute(&s.db)
    .await?;
    if let Some(st) = &b.state {
        super::set_enabled(&s, ctx.repo.id, "code_scanning_default_setup", st == "configured", Some(json!({ "query_suite": b.query_suite.clone().unwrap_or_else(|| "default".into()) }))).await?;
    }
    // forge has no hosted CodeQL runners: the configuration is stored, no run is started.
    Ok(Json(json!({})).into_response())
}

// ---------------------------------------------------------------------------------------------
// Webhooks

pub async fn emit_webhook(s: &AppState, ctx: &RepoCtx, alert_id: i64, action: &str) {
    let row: Option<AlertRow> = sqlx::query_as("SELECT * FROM code_scanning_alerts WHERE id = $1").bind(alert_id).fetch_optional(&s.db).await.ok().flatten();
    let Some(row) = row else { return };
    let Ok(mut v) = render(s, &[row], false).await else { return };
    if v.is_empty() {
        return;
    }
    let payload = json!({ "action": action, "alert": v.remove(0), "ref": "", "commit_oid": "" });
    if let Err(e) = crate::webhooks::enqueue_event_ctx(s, ctx, "code_scanning_alert", payload).await {
        tracing::warn!(error = %e, "code_scanning_alert webhook");
    }
}

