//! Dependabot alerts: the dependency graph of the default branch (manifests + submitted
//! snapshots, `api::dependency_graph::graph`) matched against the advisory database.
//!
//! Matching runs in a job (`advsec.dependabot`) after pushes to the default branch, when
//! Dependabot alerts get enabled, and for every enabled repository after an advisory import.
//! A matching (advisory, package, manifest) opens an alert; when the manifest no longer has a
//! vulnerable version the alert becomes `fixed`; a fixed alert that matches again reopens.
//! Dismissed alerts keep their state. The preset auto-triage rule dismisses
//! (`auto_dismissed`) alerts for npm *development* dependencies whose advisory is only about
//! resource exhaustion / ReDoS (CWE-400, CWE-770, CWE-1333, CWE-409); they reopen if the
//! dependency becomes a runtime dependency.
use std::collections::{HashMap, HashSet};

use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::advisories::{self, AdvisoryRow, VulnRow};
use super::requests::{self, Kind};
use super::{CursorParams, Product};
use crate::auth::AuthUser;
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/dependabot/alerts", get(repo_list))
        .route("/repos/{owner}/{repo}/dependabot/alerts/{alert_number}", get(repo_get).patch(repo_update))
        .route("/orgs/{org}/dependabot/alerts", get(org_list))
        .route("/enterprises/{enterprise}/dependabot/alerts", get(ent_list))
}

pub const DISMISS_REASONS: &[&str] = &["fix_started", "inaccurate", "no_bandwidth", "not_used", "tolerable_risk"];
const AUTO_DISMISS_CWES: &[&str] = &["CWE-400", "CWE-770", "CWE-1333", "CWE-409"];

fn clean_version(v: &str) -> String {
    v.trim().trim_start_matches(|c: char| "^~=<>! v".contains(c)).split([',', ' ', '|']).next().unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------------------------
// Matching

pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let repo_id = payload["repo_id"].as_i64().unwrap_or(0);
    let Ok(ctx) = super::system_ctx(&state.db, repo_id).await else { return Ok(()) };
    if !super::enabled(&state.db, repo_id, "dependabot_alerts").await {
        return Ok(());
    }
    match_repo(state, &ctx).await.map(|_| ()).map_err(|e| anyhow::anyhow!(e.message))
}

/// Re-matches every repository with Dependabot alerts enabled (after an advisory import).
pub async fn run_rescan_all(state: &AppState) -> anyhow::Result<()> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT repo_id FROM repo_settings WHERE vulnerability_alerts").fetch_all(&state.db).await?;
    for id in ids {
        super::enqueue(state, super::Q_DEPENDABOT, json!({ "repo_id": id })).await;
    }
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlertRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub advisory_id: i64,
    pub vulnerability_id: i64,
    pub ecosystem: String,
    pub package: String,
    pub manifest_path: String,
    pub scope: Option<String>,
    pub version: String,
    pub state: String,
    pub dismissed_at: Option<DateTime<Utc>>,
    pub dismissed_by: Option<i64>,
    pub dismissed_reason: Option<String>,
    pub dismissed_comment: Option<String>,
    pub fixed_at: Option<DateTime<Utc>>,
    pub auto_dismissed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Matches the default branch's dependencies; returns the number of open alerts.
pub async fn match_repo(state: &AppState, ctx: &RepoCtx) -> ApiResult<i64> {
    let refname = format!("refs/heads/{}", ctx.repo.default_branch);
    let (_, refs) = state.git.refs(ctx.repo.id).await?;
    let deps = match refs.get(&refname) {
        Some(sha) => crate::api::dependency_graph::graph(state, ctx, sha, Some(&refname)).await?,
        None => vec![],
    };
    let ecos: Vec<String> = deps.iter().map(|d| advisories::dep_ecosystem(&d.ecosystem).to_string()).collect::<HashSet<_>>().into_iter().collect();
    let names: Vec<String> = deps.iter().map(|d| d.name.to_lowercase()).collect::<HashSet<_>>().into_iter().collect();
    let vulns: Vec<VulnRow> = sqlx::query_as(
        "SELECT v.* FROM advisory_vulnerabilities v JOIN advisories a ON a.id = v.advisory_id
          WHERE v.ecosystem = ANY($1) AND (lower(v.package) = ANY($2) OR lower(replace(replace(v.package, '_', '-'), '.', '-')) = ANY($2))
            AND a.withdrawn_at IS NULL",
    )
    .bind(&ecos)
    .bind(&names.iter().map(|n| n.replace(['_', '.'], "-")).chain(names.iter().cloned()).collect::<Vec<_>>())
    .fetch_all(&state.db)
    .await?;
    let adv_ids: Vec<i64> = vulns.iter().map(|v| v.advisory_id).collect::<HashSet<_>>().into_iter().collect();
    let advs: Vec<AdvisoryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {} FROM advisories a WHERE a.id = ANY($1)", advisories::ADV_COLS)))
        .bind(&adv_ids)
        .fetch_all(&state.db)
        .await?;
    // (advisory, ecosystem, package, manifest) -> (vulnerability, dependency)
    let mut matched: HashMap<(i64, String, String, String), (i64, String, String)> = HashMap::new();
    for d in &deps {
        let eco = advisories::dep_ecosystem(&d.ecosystem);
        let version = clean_version(&d.version);
        if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let norm = advisories::normalize_package(eco, &d.name);
        for v in vulns.iter().filter(|v| v.ecosystem == eco && advisories::normalize_package(eco, &v.package) == norm) {
            if v.affects(&version) {
                matched.entry((v.advisory_id, eco.to_string(), v.package.clone(), d.manifest.clone())).or_insert((v.id, d.scope.clone(), version.clone()));
            }
        }
    }
    let existing: Vec<AlertRow> = sqlx::query_as("SELECT * FROM dependabot_alerts WHERE repo_id = $1").bind(ctx.repo.id).fetch_all(&state.db).await?;
    let mut created = Vec::new();
    for ((adv_id, eco, pkg, manifest), (vid, scope, version)) in &matched {
        let adv = advs.iter().find(|a| a.id == *adv_id);
        let auto = scope == "development"
            && eco == "npm"
            && adv.is_some_and(|a| {
                let cwes: Vec<&str> = a.cwes.as_array().into_iter().flatten().filter_map(|c| c.get("cwe_id").and_then(Value::as_str)).collect();
                !cwes.is_empty() && cwes.iter().all(|c| AUTO_DISMISS_CWES.contains(c))
            });
        let cur = existing.iter().find(|a| a.advisory_id == *adv_id && &a.ecosystem == eco && &a.package == pkg && &a.manifest_path == manifest);
        match cur {
            None => {
                let mut tx = state.db.begin().await?;
                let number = super::next_number(&mut *tx, ctx.repo.id, "dependabot").await?;
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO dependabot_alerts (repo_id, number, advisory_id, vulnerability_id, ecosystem, package, manifest_path, scope, version,
                                                    state, auto_dismissed_at)
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10, CASE WHEN $10 = 'auto_dismissed' THEN now() END) RETURNING id",
                )
                .bind(ctx.repo.id)
                .bind(number)
                .bind(adv_id)
                .bind(vid)
                .bind(eco)
                .bind(pkg)
                .bind(manifest)
                .bind(scope)
                .bind(version)
                .bind(if auto { "auto_dismissed" } else { "open" })
                .fetch_one(&mut *tx)
                .await?;
                tx.commit().await?;
                created.push(id);
            }
            Some(a) => {
                let new_state = match a.state.as_str() {
                    "fixed" => Some(if auto { "auto_dismissed" } else { "open" }),
                    "auto_dismissed" if !auto => Some("open"),
                    "open" if auto => Some("auto_dismissed"),
                    _ => None,
                };
                sqlx::query(
                    "UPDATE dependabot_alerts SET vulnerability_id = $2, scope = $3, version = $4,
                            state = COALESCE($5, state),
                            fixed_at = CASE WHEN $5 IS NOT NULL THEN NULL ELSE fixed_at END,
                            auto_dismissed_at = CASE WHEN $5 = 'auto_dismissed' THEN now() WHEN $5 IS NOT NULL THEN NULL ELSE auto_dismissed_at END,
                            updated_at = CASE WHEN $5 IS NOT NULL OR version <> $4 THEN now() ELSE updated_at END
                      WHERE id = $1",
                )
                .bind(a.id)
                .bind(vid)
                .bind(scope)
                .bind(version)
                .bind(new_state)
                .execute(&state.db)
                .await?;
                if new_state == Some("open") {
                    emit_webhook(state, ctx, a.id, "reopened").await;
                }
            }
        }
    }
    for a in existing.iter().filter(|a| matches!(a.state.as_str(), "open" | "auto_dismissed")) {
        if !matched.contains_key(&(a.advisory_id, a.ecosystem.clone(), a.package.clone(), a.manifest_path.clone())) {
            sqlx::query("UPDATE dependabot_alerts SET state = 'fixed', fixed_at = now(), updated_at = now() WHERE id = $1").bind(a.id).execute(&state.db).await?;
            emit_webhook(state, ctx, a.id, "fixed").await;
        }
    }
    for id in created {
        emit_webhook(state, ctx, id, "created").await;
    }
    Ok(sqlx::query_scalar("SELECT count(*) FROM dependabot_alerts WHERE repo_id = $1 AND state = 'open'").bind(ctx.repo.id).fetch_one(&state.db).await?)
}

// ---------------------------------------------------------------------------------------------
// JSON

pub async fn render(s: &AppState, rows: &[AlertRow], with_repo: bool) -> ApiResult<Vec<Value>> {
    let adv_ids: Vec<i64> = rows.iter().map(|r| r.advisory_id).collect::<HashSet<_>>().into_iter().collect();
    let advs: Vec<AdvisoryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {} FROM advisories a WHERE a.id = ANY($1)", advisories::ADV_COLS)))
        .bind(&adv_ids)
        .fetch_all(&s.db)
        .await?;
    let vulns = advisories::vulns_for(&s.db, &adv_ids).await?;
    let repos = super::repos_by_id(&s.db, rows.iter().map(|r| r.repo_id)).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let assignees: Vec<(i64, i64)> = sqlx::query_as("SELECT alert_id, user_id FROM dependabot_alert_assignees WHERE alert_id = ANY($1) ORDER BY user_id")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let repo_ids: Vec<i64> = repos.keys().copied().collect();
    let reqs = requests::for_alerts(s, &repo_ids, Kind::DependabotDismissal).await?;
    let users = super::users_by_id(
        &s.db,
        rows.iter().filter_map(|r| r.dismissed_by).chain(assignees.iter().map(|a| a.1)).chain(reqs.iter().filter_map(|q| q.requester_id)),
    )
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let (Some(ctx), Some(adv)) = (repos.get(&r.repo_id), advs.iter().find(|a| a.id == r.advisory_id)) else { continue };
        let full = ctx.full_name();
        let vuln = vulns.iter().find(|v| v.id == r.vulnerability_id);
        let req = reqs.iter().find(|q| q.repo_id == r.repo_id && q.alert_number == Some(r.number));
        let dreq = req.map(|q| {
            let st = match q.effective_status() {
                "pending" => "pending",
                "approved" => "approved",
                "denied" => "rejected",
                _ => "cancelled",
            };
            let requester = q.requester_id.and_then(|i| users.get(&i));
            json!({
                "id": q.id,
                "status": st,
                "requester": { "id": requester.map(|u| u.id).unwrap_or(0), "login": requester.map(|u| u.login.clone()).unwrap_or_else(|| "ghost".into()) },
                "created_at": crate::json::ts(q.created_at),
                "url": format!("{}/repos/{full}/dismissal-requests/dependabot/{}", s.api_url(), r.number),
            })
        });
        let mut v = json!({
            "number": r.number,
            "state": r.state,
            "dependency": {
                "package": { "ecosystem": r.ecosystem, "name": r.package },
                "manifest_path": r.manifest_path,
                "scope": r.scope,
                "relationship": if r.manifest_path.ends_with(".lock") || r.manifest_path.ends_with("-lock.json") { "inconclusive" } else { "direct" },
            },
            "security_advisory": advisories::dependabot_advisory_json(adv, &vulns),
            "security_vulnerability": vuln.map(advisories::dependabot_vuln_json(adv)).unwrap_or(Value::Null),
            "url": format!("{}/repos/{full}/dependabot/alerts/{}", s.api_url(), r.number),
            "html_url": format!("{}/{full}/security/dependabot/{}", s.base_url(), r.number),
            "created_at": crate::json::ts(r.created_at),
            "updated_at": crate::json::ts(r.updated_at),
            "dismissed_at": crate::json::ts_opt(r.dismissed_at),
            "dismissed_by": super::user_json(s, &users, r.dismissed_by),
            "dismissed_reason": r.dismissed_reason,
            "dismissed_comment": r.dismissed_comment,
            "fixed_at": crate::json::ts_opt(r.fixed_at),
            "auto_dismissed_at": crate::json::ts_opt(r.auto_dismissed_at),
            "dismissal_request": dreq,
            "assignees": assignees.iter().filter(|a| a.0 == r.id).map(|a| super::user_json(s, &users, Some(a.1))).collect::<Vec<_>>(),
        });
        if with_repo {
            v["repository"] = super::repo_json(s, ctx);
        }
        out.push(v);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// REST

#[derive(Deserialize, Default)]
struct ListQ {
    classification: Option<String>,
    state: Option<String>,
    severity: Option<String>,
    ecosystem: Option<String>,
    package: Option<String>,
    manifest: Option<String>,
    epss_percentage: Option<String>,
    has: Option<String>,
    assignee: Option<String>,
    scope: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    #[serde(flatten)]
    cursor: CursorParams,
}

async fn list(s: &AppState, uri: &OriginalUri, repo_scope: &str, scope_id: i64, q: ListQ, with_repo: bool) -> ApiResult<Response> {
    super::check_enum("DependabotAlert", "state", &q.state, &["auto_dismissed", "dismissed", "fixed", "open"])?;
    super::check_enum("DependabotAlert", "severity", &q.severity, &["low", "medium", "high", "critical"])?;
    super::check_enum("DependabotAlert", "scope", &q.scope, &["development", "runtime"])?;
    super::check_enum("DependabotAlert", "sort", &q.sort, &["created", "updated", "epss_percentage"])?;
    super::check_enum("DependabotAlert", "classification", &q.classification, &["malware", "general"])?;
    super::check_enum("DependabotAlert", "has", &q.has, &["patch"])?;
    let _ = &q.epss_percentage;
    let order = if q.sort.as_deref() == Some("updated") { "d.updated_at" } else { "d.created_at" };
    let dir = super::dir(q.direction.as_deref());
    let rows: Vec<AlertRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT d.* FROM dependabot_alerts d JOIN advisories a ON a.id = d.advisory_id JOIN advisory_vulnerabilities v ON v.id = d.vulnerability_id
          WHERE d.repo_id IN {repo_scope}
            AND ($2::text[] IS NULL OR d.state = ANY($2))
            AND ($3::text[] IS NULL OR (CASE WHEN a.severity = 'unknown' THEN 'low' ELSE a.severity END) = ANY($3))
            AND ($4::text[] IS NULL OR d.ecosystem = ANY($4))
            AND ($5::text[] IS NULL OR d.package = ANY($5))
            AND ($6::text[] IS NULL OR d.manifest_path = ANY($6))
            AND ($7::text IS NULL OR d.scope = $7)
            AND ($8::text[] IS NULL OR (CASE WHEN a.kind = 'malware' THEN 'malware' ELSE 'general' END) = ANY($8))
            AND ($9::text IS NULL OR v.first_patched_version IS NOT NULL)
            AND ($10::text IS NULL OR ($10 = 'none' AND NOT EXISTS(SELECT 1 FROM dependabot_alert_assignees x WHERE x.alert_id = d.id))
                 OR ($10 = '*' AND EXISTS(SELECT 1 FROM dependabot_alert_assignees x WHERE x.alert_id = d.id))
                 OR EXISTS(SELECT 1 FROM dependabot_alert_assignees x JOIN users u ON u.id = x.user_id WHERE x.alert_id = d.id AND lower(u.login) = lower($10)))
          ORDER BY {order} {dir}, d.id {dir}"
    )))
    .bind(scope_id)
    .bind(super::csv(&q.state))
    .bind(super::csv(&q.severity))
    .bind(super::csv(&q.ecosystem))
    .bind(super::csv(&q.package))
    .bind(super::csv(&q.manifest))
    .bind(&q.scope)
    .bind(super::csv(&q.classification))
    .bind(&q.has)
    .bind(&q.assignee)
    .fetch_all(&s.db)
    .await?;
    let total = rows.len() as i64;
    let page: Vec<AlertRow> = rows.into_iter().skip(q.cursor.offset() as usize).take(q.cursor.per_page() as usize).collect();
    let items = render(s, &page, with_repo).await?;
    Ok(super::list_response(s, uri, &q.cursor, total, items))
}

fn disabled() -> ApiError {
    ApiError::forbidden("Dependabot alerts are disabled for this repository.")
}

async fn repo_ctx(s: &AppState, u: &User, o: &str, r: &str, write: bool) -> ApiResult<RepoCtx> {
    let ctx = super::repo_for(s, u, o, r, Product::Dependabot, write).await?;
    if !super::enabled(&s.db, ctx.repo.id, "dependabot_alerts").await {
        return Err(disabled());
    }
    Ok(ctx)
}

async fn repo_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): Path<(String, String)>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let ctx = repo_ctx(&s, &u, &o, &r, false).await?;
    list(&s, &uri, "(SELECT $1::bigint)", ctx.repo.id, q, false).await
}

async fn org_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let o = super::org_for(&s, &u, &org, Product::Dependabot).await?;
    list(&s, &uri, "(SELECT id FROM repositories WHERE owner_id = $1)", o.id, q, true).await
}

async fn ent_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(e): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    list(&s, &uri, "(SELECT r.id FROM repositories r JOIN users o ON o.id = r.owner_id WHERE o.kind = 'Organization' AND $1 = 0)", 0, q, true).await
}

pub async fn alert_row(s: &AppState, repo_id: i64, n: i64) -> ApiResult<AlertRow> {
    sqlx::query_as("SELECT * FROM dependabot_alerts WHERE repo_id = $1 AND number = $2")
        .bind(repo_id)
        .bind(n)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn repo_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = repo_ctx(&s, &u, &o, &r, false).await?;
    let row = alert_row(&s, ctx.repo.id, n).await?;
    Ok(Json(render(&s, &[row], false).await?.remove(0)))
}

#[derive(Deserialize)]
struct UpdateBody {
    state: Option<String>,
    dismissed_reason: Option<String>,
    dismissed_comment: Option<String>,
    assignees: Option<Vec<String>>,
}

/// Dismisses (or reopens) an alert.
pub async fn set_state(s: &AppState, ctx: &RepoCtx, n: i64, state: &str, reason: Option<&str>, comment: Option<&str>, by: &User) -> ApiResult<()> {
    let res = sqlx::query(
        "UPDATE dependabot_alerts SET state = $3,
                dismissed_at = CASE WHEN $3 = 'dismissed' THEN now() END,
                dismissed_by = CASE WHEN $3 = 'dismissed' THEN $4 END,
                dismissed_reason = CASE WHEN $3 = 'dismissed' THEN $5 END,
                dismissed_comment = CASE WHEN $3 = 'dismissed' THEN $6 END,
                updated_at = now()
          WHERE repo_id = $1 AND number = $2",
    )
    .bind(ctx.repo.id)
    .bind(n)
    .bind(state)
    .bind(by.id)
    .bind(reason)
    .bind(comment)
    .execute(&s.db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn repo_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<UpdateBody>) -> ApiResult<Json<Value>> {
    let ctx = repo_ctx(&s, &u, &o, &r, true).await?;
    let row = alert_row(&s, ctx.repo.id, n).await?;
    if b.state.is_none() && b.assignees.is_none() {
        return Err(ApiError::validation("DependabotAlert", "state", "missing_field"));
    }
    if let Some(list) = &b.assignees {
        let mut ids = Vec::new();
        for login in list {
            let target = crate::db::user_by_login(&s.db, login).await.map_err(|_| ApiError::validation("DependabotAlert", "assignees", "invalid"))?;
            let perm = crate::db::permission(&s.db, Some(&target), &ctx.repo, &ctx.owner).await?;
            if perm < crate::db::Perm::Write {
                return Err(ApiError::validation("DependabotAlert", "assignees", "invalid"));
            }
            ids.push(target.id);
        }
        let mut tx = s.db.begin().await?;
        sqlx::query("DELETE FROM dependabot_alert_assignees WHERE alert_id = $1").bind(row.id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO dependabot_alert_assignees (alert_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
            .bind(row.id)
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE dependabot_alerts SET updated_at = now() WHERE id = $1").bind(row.id).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    match b.state.as_deref() {
        Some("dismissed") => {
            let reason = b.dismissed_reason.clone().ok_or_else(|| ApiError::validation("DependabotAlert", "dismissed_reason", "missing_field"))?;
            if !DISMISS_REASONS.contains(&reason.as_str()) {
                return Err(ApiError::validation("DependabotAlert", "dismissed_reason", "invalid"));
            }
            if b.dismissed_comment.as_ref().is_some_and(|c| c.chars().count() > 280) {
                return Err(ApiError::validation("DependabotAlert", "dismissed_comment", "invalid"));
            }
            if row.state == "fixed" {
                return Err(ApiError::validation("DependabotAlert", "state", "invalid"));
            }
            if requests::delegated(&s, ctx.repo.id, Kind::DependabotDismissal).await && !requests::can_review(&s, &ctx, &u, Kind::DependabotDismissal).await {
                return Err(ApiError::forbidden("Delegated alert dismissal is enabled for this repository: create a dismissal request instead."));
            }
            set_state(&s, &ctx, n, "dismissed", Some(&reason), b.dismissed_comment.as_deref(), &u).await?;
            emit_webhook(&s, &ctx, row.id, "dismissed").await;
        }
        Some("open") => {
            if row.state == "dismissed" || row.state == "auto_dismissed" {
                set_state(&s, &ctx, n, "open", None, None, &u).await?;
                emit_webhook(&s, &ctx, row.id, "reopened").await;
            } else if row.state == "fixed" {
                return Err(ApiError::validation("DependabotAlert", "state", "invalid"));
            }
        }
        Some(_) => return Err(ApiError::validation("DependabotAlert", "state", "invalid")),
        None => {}
    }
    let row = alert_row(&s, ctx.repo.id, n).await?;
    Ok(Json(render(&s, &[row], false).await?.remove(0)))
}

/// Opens a dismissal request for an open alert (delegated alert dismissal).
pub async fn request_dismissal(s: &AppState, ctx: &RepoCtx, u: &User, n: i64, reason: &str, comment: Option<&str>) -> ApiResult<requests::RequestRow> {
    if !super::enabled(&s.db, ctx.repo.id, "dependabot_alerts").await {
        return Err(disabled());
    }
    if !requests::delegated(s, ctx.repo.id, Kind::DependabotDismissal).await {
        return Err(ApiError::unprocessable("Delegated alert dismissal is not enabled for this repository."));
    }
    let row = alert_row(s, ctx.repo.id, n).await?;
    if row.state != "open" {
        return Err(ApiError::unprocessable("Only open alerts can be dismissed."));
    }
    let title: String = sqlx::query_scalar("SELECT summary FROM advisories WHERE id = $1").bind(row.advisory_id).fetch_one(&s.db).await?;
    let data = json!([{ "reason": reason, "alert_number": n.to_string(), "alert_title": title }]);
    requests::create(s, ctx, Kind::DependabotDismissal, Some(n), u, Some(reason), comment, data, &n.to_string()).await
}

/// An approved dismissal request dismisses the alert as the requester asked.
pub async fn apply_dismissal(s: &AppState, ctx: &RepoCtx, n: i64, requester: &User, req: &requests::RequestRow) -> ApiResult<()> {
    let reason = req.reason.clone().unwrap_or_else(|| "tolerable_risk".into());
    set_state(s, ctx, n, "dismissed", Some(&reason), req.requester_comment.as_deref(), requester).await
}

/// `dependabot_alert` webhook.
pub async fn emit_webhook(s: &AppState, ctx: &RepoCtx, alert_id: i64, action: &str) {
    let row: Option<AlertRow> = sqlx::query_as("SELECT * FROM dependabot_alerts WHERE id = $1").bind(alert_id).fetch_optional(&s.db).await.ok().flatten();
    let Some(row) = row else { return };
    let Ok(mut v) = render(s, &[row], false).await else { return };
    if v.is_empty() {
        return;
    }
    let payload = json!({ "action": action, "alert": v.remove(0) });
    if let Err(e) = crate::webhooks::enqueue_event_ctx(s, ctx, "dependabot_alert", payload).await {
        tracing::warn!(error = %e, "dependabot_alert webhook");
    }
}
