//! Secret scanning REST API: alerts (repository / organization / enterprise), locations,
//! push protection bypasses, scan history and pattern configurations.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::requests::{self, Kind};
use super::{patterns, CursorParams, Product};
use crate::auth::AuthUser;
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/secret-scanning";
    Router::new()
        .route(&format!("{r}/alerts"), get(repo_list))
        .route(&format!("{r}/alerts/{{alert_number}}"), get(repo_get).patch(repo_update))
        .route(&format!("{r}/alerts/{{alert_number}}/locations"), get(locations))
        .route(&format!("{r}/push-protection-bypasses"), post(create_bypass))
        .route(&format!("{r}/scan-history"), get(scan_history))
        .route("/orgs/{org}/secret-scanning/alerts", get(org_list))
        .route("/enterprises/{enterprise}/secret-scanning/alerts", get(ent_list))
        .route("/orgs/{org}/secret-scanning/pattern-configurations", get(org_patterns).patch(org_patterns_update))
        .route("/enterprises/{enterprise}/secret-scanning/pattern-configurations", get(ent_patterns).patch(ent_patterns_update))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlertRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub secret_type: String,
    pub secret: String,
    pub state: String,
    pub resolution: Option<String>,
    pub resolution_comment: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<i64>,
    pub validity: String,
    pub push_protection_bypassed: bool,
    pub push_protection_bypassed_by: Option<i64>,
    pub push_protection_bypassed_at: Option<DateTime<Utc>>,
    pub bypass_request_id: Option<i64>,
    pub assigned_to: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "a.id, a.repo_id, a.number, a.secret_type, a.secret, a.state, a.resolution, a.resolution_comment, a.resolved_at,
    a.resolved_by, a.validity, a.push_protection_bypassed, a.push_protection_bypassed_by, a.push_protection_bypassed_at, a.bypass_request_id,
    a.assigned_to, a.created_at, a.updated_at";

#[derive(Debug, Clone, sqlx::FromRow)]
struct LocRow {
    alert_id: i64,
    commit_sha: String,
    blob_sha: String,
    path: String,
    start_line: i32,
    end_line: i32,
    start_column: i32,
    end_column: i32,
}

fn location_details(s: &AppState, full: &str, l: &LocRow) -> Value {
    json!({
        "path": l.path,
        "start_line": l.start_line,
        "end_line": l.end_line,
        "start_column": l.start_column,
        "end_column": l.end_column,
        "blob_sha": l.blob_sha,
        "blob_url": format!("{}/repos/{full}/git/blobs/{}", s.api_url(), l.blob_sha),
        "commit_sha": l.commit_sha,
        "commit_url": format!("{}/repos/{full}/git/commits/{}", s.api_url(), l.commit_sha),
        "html_url": format!("{}/{full}/blob/{}/{}#L{}", s.base_url(), l.commit_sha, l.path, l.start_line),
    })
}

/// Renders alerts. `with_repo` adds `repository` (org / enterprise lists).
pub async fn render(s: &AppState, rows: &[AlertRow], with_repo: bool, hide_secret: bool) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let repos = super::repos_by_id(&s.db, rows.iter().map(|r| r.repo_id)).await?;
    let first: Vec<LocRow> = sqlx::query_as(
        "SELECT DISTINCT ON (alert_id) alert_id, commit_sha, blob_sha, path, start_line, end_line, start_column, end_column
           FROM secret_scanning_locations WHERE alert_id = ANY($1) ORDER BY alert_id, id",
    )
    .bind(&ids)
    .fetch_all(&s.db)
    .await?;
    let counts: Vec<(i64, i64)> = sqlx::query_as("SELECT alert_id, count(*) FROM secret_scanning_locations WHERE alert_id = ANY($1) GROUP BY alert_id")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let counts: HashMap<i64, i64> = counts.into_iter().collect();
    let repo_ids: Vec<i64> = repos.keys().copied().collect();
    let dismissal = requests::for_alerts(s, &repo_ids, Kind::SecretDismissal).await?;
    let bypass_ids: Vec<i64> = rows.iter().filter_map(|r| r.bypass_request_id).collect();
    let bypass_reqs: Vec<requests::RequestRow> = sqlx::query_as("SELECT * FROM advsec_requests WHERE id = ANY($1)").bind(&bypass_ids).fetch_all(&s.db).await?;
    let all_req_ids: Vec<i64> = dismissal.iter().chain(bypass_reqs.iter()).map(|r| r.id).collect();
    let reviews: Vec<(i64, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT ON (request_id) request_id, reviewer_id, message FROM advsec_request_responses WHERE request_id = ANY($1) ORDER BY request_id, id DESC",
    )
    .bind(&all_req_ids)
    .fetch_all(&s.db)
    .await?;
    let users = super::users_by_id(
        &s.db,
        rows.iter()
            .flat_map(|r| [r.resolved_by, r.push_protection_bypassed_by, r.assigned_to])
            .flatten()
            .chain(reviews.iter().filter_map(|x| x.1)),
    )
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let Some(ctx) = repos.get(&r.repo_id) else { continue };
        let full = ctx.full_name();
        let url = format!("{}/repos/{full}/secret-scanning/alerts/{}", s.api_url(), r.number);
        let pat = patterns::by_slug(&r.secret_type);
        let dreq = dismissal.iter().find(|d| d.repo_id == r.repo_id && d.alert_number == Some(r.number));
        let breq = r.bypass_request_id.and_then(|id| bypass_reqs.iter().find(|b| b.id == id));
        let review_of = |req: Option<&requests::RequestRow>| req.and_then(|q| reviews.iter().find(|x| x.0 == q.id));
        let first_loc = first.iter().find(|l| l.alert_id == r.id).map(|l| location_details(s, &full, l));
        let mut v = json!({
            "number": r.number,
            "created_at": crate::json::ts(r.created_at),
            "updated_at": crate::json::ts(r.updated_at),
            "url": url,
            "html_url": format!("{}/{full}/security/secret-scanning/{}", s.base_url(), r.number),
            "locations_url": format!("{url}/locations"),
            "state": r.state,
            "resolution": r.resolution,
            "resolved_at": crate::json::ts_opt(r.resolved_at),
            "resolved_by": super::user_json(s, &users, r.resolved_by),
            "resolution_comment": r.resolution_comment,
            "secret_type": r.secret_type,
            "secret_type_display_name": pat.map(|p| p.display_name).unwrap_or(&r.secret_type),
            "provider": pat.and_then(|p| p.provider),
            "provider_slug": pat.and_then(|p| p.provider_slug),
            "push_protection_bypassed": r.push_protection_bypassed,
            "push_protection_bypassed_by": super::user_json(s, &users, r.push_protection_bypassed_by),
            "push_protection_bypassed_at": crate::json::ts_opt(r.push_protection_bypassed_at),
            "push_protection_bypass_request_reviewer": super::user_json(s, &users, review_of(breq).and_then(|x| x.1)),
            "push_protection_bypass_request_reviewer_comment": review_of(breq).and_then(|x| x.2.clone()),
            "push_protection_bypass_request_comment": breq.and_then(|b| b.requester_comment.clone()),
            "push_protection_bypass_request_html_url": breq.map(|b| format!("{}/{full}/security/secret-scanning/push-protection-bypasses/{}", s.base_url(), b.number)),
            "validity": r.validity,
            "multi_repo": false,
            "is_base64_encoded": false,
            "first_location_detected": first_loc,
            "has_more_locations": counts.get(&r.id).copied().unwrap_or(0) > 1,
            "assigned_to": super::user_json(s, &users, r.assigned_to),
            "closure_request_comment": dreq.and_then(|d| d.requester_comment.clone()),
            "closure_request_reviewer_comment": review_of(dreq).and_then(|x| x.2.clone()),
            "closure_request_reviewer": super::user_json(s, &users, review_of(dreq).and_then(|x| x.1)),
        });
        if !hide_secret {
            v["secret"] = json!(r.secret);
        }
        if with_repo {
            v["repository"] = super::repo_json(s, ctx);
        }
        out.push(v);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Listing

#[derive(Deserialize, Default)]
struct ListQ {
    state: Option<String>,
    secret_type: Option<String>,
    exclude_secret_types: Option<String>,
    exclude_providers: Option<String>,
    providers: Option<String>,
    resolution: Option<String>,
    assignee: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    validity: Option<String>,
    #[serde(default, deserialize_with = "super::qs::opt_bool")]
    is_publicly_leaked: Option<bool>,
    #[serde(default, deserialize_with = "super::qs::opt_bool")]
    is_multi_repo: Option<bool>,
    #[serde(default, deserialize_with = "super::qs::opt_bool")]
    hide_secret: Option<bool>,
    #[serde(default, deserialize_with = "super::qs::opt_bool")]
    is_bypassed: Option<bool>,
    #[serde(flatten)]
    cursor: CursorParams,
}

async fn list(s: &AppState, uri: &OriginalUri, repo_scope: &str, scope_id: i64, q: ListQ, with_repo: bool) -> ApiResult<Response> {
    super::check_enum("SecretScanningAlert", "state", &q.state, &["open", "resolved"])?;
    super::check_enum("SecretScanningAlert", "sort", &q.sort, &["created", "updated"])?;
    super::check_enum("SecretScanningAlert", "resolution", &q.resolution, &["false_positive", "wont_fix", "revoked", "used_in_tests", "pattern_edited", "pattern_deleted"])?;
    super::check_enum("SecretScanningAlert", "validity", &q.validity, &["active", "inactive", "unknown"])?;
    let provider_of = |slug: &str| patterns::by_slug(slug).and_then(|p| p.provider_slug).unwrap_or("");
    let order = if q.sort.as_deref() == Some("updated") { "a.updated_at" } else { "a.created_at" };
    let d = super::dir(q.direction.as_deref());
    let rows: Vec<AlertRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM secret_scanning_alerts a WHERE a.repo_id IN {repo_scope}
            AND ($2::text IS NULL OR a.state = $2)
            AND ($3::text[] IS NULL OR a.secret_type = ANY($3))
            AND ($4::text[] IS NULL OR NOT a.secret_type = ANY($4))
            AND ($5::text[] IS NULL OR a.resolution = ANY($5))
            AND ($6::text[] IS NULL OR a.validity = ANY($6))
            AND ($7::bool IS NULL OR a.push_protection_bypassed = $7)
            AND ($8::text IS NULL OR ($8 = 'none' AND a.assigned_to IS NULL) OR ($8 = '*' AND a.assigned_to IS NOT NULL)
                 OR a.assigned_to = (SELECT id FROM users WHERE lower(login) = lower($8)))
          ORDER BY {order} {d}, a.id {d}"
    )))
    .bind(scope_id)
    .bind(&q.state)
    .bind(super::csv(&q.secret_type))
    .bind(super::csv(&q.exclude_secret_types))
    .bind(super::csv(&q.resolution))
    .bind(super::csv(&q.validity))
    .bind(q.is_bypassed)
    .bind(&q.assignee)
    .fetch_all(&s.db)
    .await?;
    let providers = super::csv(&q.providers);
    let excl = super::csv(&q.exclude_providers);
    let rows: Vec<AlertRow> = rows
        .into_iter()
        .filter(|r| providers.as_ref().is_none_or(|p| p.iter().any(|x| x == provider_of(&r.secret_type))))
        .filter(|r| excl.as_ref().is_none_or(|p| !p.iter().any(|x| x == provider_of(&r.secret_type))))
        // forge never sees secrets outside its own repositories, nor across repositories.
        .filter(|_| q.is_publicly_leaked != Some(true) && q.is_multi_repo != Some(true))
        .collect();
    let total = rows.len() as i64;
    let page: Vec<AlertRow> = rows.into_iter().skip(q.cursor.offset() as usize).take(q.cursor.per_page() as usize).collect();
    let items = render(s, &page, with_repo, q.hide_secret.unwrap_or(false)).await?;
    Ok(super::list_response(s, uri, &q.cursor, total, items))
}

fn disabled() -> ApiError {
    ApiError::not_found()
}

async fn repo_ctx(s: &AppState, u: &User, o: &str, r: &str) -> ApiResult<RepoCtx> {
    let ctx = super::repo_for(s, u, o, r, Product::SecretScanning, false).await?;
    if !super::enabled(&s.db, ctx.repo.id, "secret_scanning").await {
        return Err(disabled());
    }
    Ok(ctx)
}

async fn repo_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): Path<(String, String)>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let ctx = repo_ctx(&s, &u, &o, &r).await?;
    list(&s, &uri, "(SELECT $1::bigint)", ctx.repo.id, q, false).await
}

async fn org_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let o = super::org_for(&s, &u, &org, Product::SecretScanning).await?;
    list(&s, &uri, "(SELECT id FROM repositories WHERE owner_id = $1)", o.id, q, true).await
}

async fn ent_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(e): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    let scope = format!("(SELECT r.id FROM repositories r JOIN users o ON o.id = r.owner_id WHERE o.kind = 'Organization' AND $1 = 0)");
    list(&s, &uri, &scope, 0, q, true).await
}

async fn alert_row(s: &AppState, repo_id: i64, n: i64) -> ApiResult<AlertRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM secret_scanning_alerts a WHERE a.repo_id = $1 AND a.number = $2")))
        .bind(repo_id)
        .bind(n)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize, Default)]
struct GetQ {
    hide_secret: Option<bool>,
}

async fn repo_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Query(q): Query<GetQ>) -> ApiResult<Json<Value>> {
    let ctx = repo_ctx(&s, &u, &o, &r).await?;
    let row = alert_row(&s, ctx.repo.id, n).await?;
    Ok(Json(render(&s, &[row], false, q.hide_secret.unwrap_or(false)).await?.remove(0)))
}

#[derive(Deserialize)]
struct UpdateBody {
    state: Option<String>,
    resolution: Option<String>,
    resolution_comment: Option<String>,
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    assignee: Option<Option<String>>,
    validity: Option<String>,
}

const RESOLUTIONS: &[&str] = &["false_positive", "wont_fix", "revoked", "used_in_tests"];

/// Resolves or reopens an alert directly.
pub async fn set_state(s: &AppState, ctx: &RepoCtx, n: i64, state: &str, resolution: Option<&str>, comment: Option<&str>, by: &User) -> ApiResult<()> {
    let res = sqlx::query(
        "UPDATE secret_scanning_alerts SET state = $3,
                resolution = CASE WHEN $3 = 'resolved' THEN $4 ELSE NULL END,
                resolution_comment = CASE WHEN $3 = 'resolved' THEN $5 ELSE NULL END,
                resolved_at = CASE WHEN $3 = 'resolved' THEN now() ELSE NULL END,
                resolved_by = CASE WHEN $3 = 'resolved' THEN $6 ELSE NULL END, updated_at = now()
          WHERE repo_id = $1 AND number = $2",
    )
    .bind(ctx.repo.id)
    .bind(n)
    .bind(state)
    .bind(resolution)
    .bind(comment)
    .bind(by.id)
    .execute(&s.db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

/// An approved dismissal request resolves the alert with the requested reason.
pub async fn apply_dismissal(s: &AppState, ctx: &RepoCtx, n: i64, requester: &User, _reviewer: &User, req: &requests::RequestRow) -> ApiResult<()> {
    let resolution = match req.reason.as_deref() {
        Some("tests") => "used_in_tests",
        Some("revoked") => "revoked",
        Some("fixed_later") => "wont_fix",
        _ => "false_positive",
    };
    set_state(s, ctx, n, "resolved", Some(resolution), req.requester_comment.as_deref(), requester).await?;
    emit_webhook(s, ctx, 0, "resolved").await;
    Ok(())
}

async fn repo_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<UpdateBody>) -> ApiResult<Json<Value>> {
    let ctx = crate::db::load_repo(&s.db, Some(&u), &o, &r).await?;
    super::check_repo(&s, &ctx, &u, Product::SecretScanning, true).await?;
    if !super::enabled(&s.db, ctx.repo.id, "secret_scanning").await {
        return Err(disabled());
    }
    if b.state.is_none() && b.assignee.is_none() && b.validity.is_none() {
        return Err(ApiError::validation("SecretScanningAlert", "state", "missing_field"));
    }
    let row = alert_row(&s, ctx.repo.id, n).await?;
    if let Some(v) = &b.validity {
        if !["active", "inactive"].contains(&v.as_str()) {
            return Err(ApiError::validation("SecretScanningAlert", "validity", "invalid"));
        }
        sqlx::query("UPDATE secret_scanning_alerts SET validity = $2, updated_at = now() WHERE id = $1").bind(row.id).bind(v).execute(&s.db).await?;
    }
    if let Some(a) = &b.assignee {
        let uid = match a {
            Some(login) => {
                let target = crate::db::user_by_login(&s.db, login).await.map_err(|_| ApiError::validation("SecretScanningAlert", "assignee", "invalid"))?;
                let perm = crate::db::permission(&s.db, Some(&target), &ctx.repo, &ctx.owner).await?;
                if perm < crate::db::Perm::Write {
                    return Err(ApiError::validation("SecretScanningAlert", "assignee", "invalid"));
                }
                Some(target.id)
            }
            None => None,
        };
        sqlx::query("UPDATE secret_scanning_alerts SET assigned_to = $2, updated_at = now() WHERE id = $1").bind(row.id).bind(uid).execute(&s.db).await?;
    }
    if let Some(st) = &b.state {
        match st.as_str() {
            "resolved" => {
                let res = b.resolution.clone().ok_or_else(|| ApiError::validation("SecretScanningAlert", "resolution", "missing_field"))?;
                if !RESOLUTIONS.contains(&res.as_str()) {
                    return Err(ApiError::validation("SecretScanningAlert", "resolution", "invalid"));
                }
                if row.state != "resolved" {
                    let delegated = requests::delegated(&s, ctx.repo.id, Kind::SecretDismissal).await;
                    if delegated && !requests::can_review(&s, &ctx, &u, Kind::SecretDismissal).await {
                        let reason = match res.as_str() {
                            "used_in_tests" => "tests",
                            "revoked" => "revoked",
                            "wont_fix" => "fixed_later",
                            _ => "false_positive",
                        };
                        let data = json!([{ "secret_type": row.secret_type, "alert_number": row.number.to_string(), "reason": reason }]);
                        requests::create(&s, &ctx, Kind::SecretDismissal, Some(n), &u, Some(reason), b.resolution_comment.as_deref(), data, &n.to_string()).await?;
                    } else {
                        set_state(&s, &ctx, n, "resolved", Some(&res), b.resolution_comment.as_deref(), &u).await?;
                        emit_webhook(&s, &ctx, row.id, "resolved").await;
                    }
                } else {
                    set_state(&s, &ctx, n, "resolved", Some(&res), b.resolution_comment.as_deref(), &u).await?;
                }
            }
            "open" => {
                if row.state != "open" {
                    set_state(&s, &ctx, n, "open", None, None, &u).await?;
                    emit_webhook(&s, &ctx, row.id, "reopened").await;
                }
            }
            _ => return Err(ApiError::validation("SecretScanningAlert", "state", "invalid")),
        }
    }
    let row = alert_row(&s, ctx.repo.id, n).await?;
    Ok(Json(render(&s, &[row], false, false).await?.remove(0)))
}

async fn locations(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r, n)): Path<(String, String, i64)>, Query(p): Query<crate::api::PageParams>) -> ApiResult<crate::api::Paged> {
    let ctx = repo_ctx(&s, &u, &o, &r).await?;
    let row = alert_row(&s, ctx.repo.id, n).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM secret_scanning_locations WHERE alert_id = $1").bind(row.id).fetch_one(&s.db).await?;
    let locs: Vec<LocRow> = sqlx::query_as(
        "SELECT alert_id, commit_sha, blob_sha, path, start_line, end_line, start_column, end_column FROM secret_scanning_locations
          WHERE alert_id = $1 ORDER BY id LIMIT $2 OFFSET $3",
    )
    .bind(row.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let full = ctx.full_name();
    let items = locs.iter().map(|l| json!({ "type": "commit", "details": location_details(&s, &full, l) })).collect();
    Ok(crate::api::Paged::new(&uri, &s, p, Some(total), items))
}

// ---------------------------------------------------------------------------------------------
// Push protection bypasses

#[derive(Deserialize)]
struct BypassBody {
    reason: Option<String>,
    placeholder_id: Option<String>,
}

async fn create_bypass(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<BypassBody>) -> ApiResult<Response> {
    let ctx = crate::db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(crate::db::Perm::Write)?;
    let reason = b.reason.ok_or_else(|| ApiError::validation("PushProtectionBypass", "reason", "missing_field"))?;
    if !["false_positive", "used_in_tests", "will_fix_later"].contains(&reason.as_str()) {
        return Err(ApiError::validation("PushProtectionBypass", "reason", "invalid"));
    }
    let placeholder = b.placeholder_id.filter(|p| !p.is_empty()).ok_or_else(|| ApiError::validation("PushProtectionBypass", "placeholder_id", "missing_field"))?;
    let Some((token_type, path, branch)) = super::secret_scanning::blocked_token_type(&s, ctx.repo.id, &placeholder).await? else {
        return Err(ApiError::new(axum::http::StatusCode::NOT_FOUND, "Placeholder ID not found"));
    };
    if requests::delegated(&s, ctx.repo.id, Kind::SecretBypass).await && !requests::can_review(&s, &ctx, &u, Kind::SecretBypass).await {
        let bypass_reason = if reason == "will_fix_later" { "fix_later" } else { reason.as_str() };
        let data = json!([{ "secret_type": token_type, "bypass_reason": bypass_reason, "path": path, "branch": branch }]);
        let req = requests::create(&s, &ctx, Kind::SecretBypass, None, &u, Some(&reason), None, data, &placeholder).await?;
        return Err(ApiError::forbidden(format!(
            "Delegated bypass is enabled for this repository: bypass request {} must be approved before the secret can be pushed. {}/{}/security/secret-scanning/push-protection-bypasses/{}",
            req.number, s.base_url(), ctx.full_name(), req.number
        )));
    }
    let exp = super::secret_scanning::grant_bypass(&s, ctx.repo.id, &placeholder, &token_type, &reason, Some(u.id), None).await?;
    Ok(Json(json!({ "reason": reason, "expire_at": crate::json::ts(exp), "token_type": token_type })).into_response())
}

async fn scan_history(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo_ctx(&s, &u, &o, &r).await?;
    let mut scans = super::secret_scanning::scans(&s, ctx.repo.id).await?;
    let mut take = |k: &str| json!(scans.remove(k).unwrap_or_default());
    Ok(Json(json!({
        "incremental_scans": take("incremental"),
        "pattern_update_scans": take("pattern_update"),
        "backfill_scans": take("backfill"),
        "custom_pattern_backfill_scans": [],
        "generic_secrets_backfill_scans": take("generic_backfill"),
    })))
}

// ---------------------------------------------------------------------------------------------
// Pattern configurations

async fn pattern_config_json(s: &AppState, kind: &str, owner_id: i64) -> ApiResult<Value> {
    let settings: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT owner_kind, token_type, setting FROM secret_scanning_pattern_settings
          WHERE (owner_kind = $1 AND owner_id = $2) OR (owner_kind = 'enterprise' AND owner_id = 0)",
    )
    .bind(kind)
    .bind(owner_id)
    .fetch_all(&s.db)
    .await?;
    let version: Option<String> = sqlx::query_scalar("SELECT version FROM secret_scanning_pattern_versions WHERE owner_kind = $1 AND owner_id = $2")
        .bind(kind)
        .bind(owner_id)
        .fetch_optional(&s.db)
        .await?;
    // Alert statistics per token type over the owner's repositories.
    let scope = if kind == "org" { "r.owner_id = $1" } else { "o.kind = 'Organization' AND $1 = 0" };
    let stats: Vec<(String, i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT a.secret_type, count(*), count(*) FILTER (WHERE a.resolution = 'false_positive'), count(*) FILTER (WHERE a.push_protection_bypassed)
           FROM secret_scanning_alerts a JOIN repositories r ON r.id = a.repo_id JOIN users o ON o.id = r.owner_id WHERE {scope} GROUP BY a.secret_type"
    )))
    .bind(owner_id)
    .fetch_all(&s.db)
    .await?;
    let total: i64 = stats.iter().map(|x| x.1).sum();
    let get = |k: &str, t: &str| settings.iter().find(|(ok, tt, _)| ok == k && tt == t).map(|x| x.2.clone());
    let overrides: Vec<Value> = patterns::token_types()
        .iter()
        .map(|p| {
            let st = stats.iter().find(|x| x.0 == p.slug);
            let (n, fp, bp) = st.map(|x| (x.1, x.2, x.3)).unwrap_or((0, 0, 0));
            let pct = |a: i64, b: i64| if b > 0 { a * 100 / b } else { 0 };
            let ent = get("enterprise", p.slug);
            let own = if kind == "enterprise" { ent.clone() } else { get(kind, p.slug) };
            json!({
                "token_type": p.slug,
                "custom_pattern_version": null,
                "slug": p.slug,
                "display_name": p.display_name,
                "alert_total": n,
                "alert_total_percentage": pct(n, total),
                "false_positives": fp,
                "false_positive_rate": pct(fp, n),
                "bypass_rate": pct(bp, n),
                "default_setting": if p.push_protected { "enabled" } else { "disabled" },
                "enterprise_setting": if kind == "org" { json!(ent.unwrap_or_else(|| "not-set".into())) } else { Value::Null },
                "setting": own.unwrap_or_else(|| "not-set".into()),
            })
        })
        .collect();
    Ok(json!({ "pattern_config_version": version, "provider_pattern_overrides": overrides, "custom_pattern_overrides": [] }))
}

#[derive(Deserialize)]
struct PatternUpdate {
    pattern_config_version: Option<String>,
    provider_pattern_settings: Option<Vec<ProviderSetting>>,
    custom_pattern_settings: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct ProviderSetting {
    token_type: Option<String>,
    push_protection_setting: Option<String>,
}

async fn update_patterns(s: &AppState, kind: &str, owner_id: i64, b: PatternUpdate) -> ApiResult<Json<Value>> {
    let current: Option<String> = sqlx::query_scalar("SELECT version FROM secret_scanning_pattern_versions WHERE owner_kind = $1 AND owner_id = $2")
        .bind(kind)
        .bind(owner_id)
        .fetch_optional(&s.db)
        .await?;
    // Optimistic concurrency: the version read must still be current.
    if b.pattern_config_version.is_some() && b.pattern_config_version != current {
        return Err(ApiError::conflict("The pattern configuration has been updated since it was read (pattern_config_version mismatch)."));
    }
    if b.custom_pattern_settings.as_ref().is_some_and(|c| !c.is_empty()) {
        return Err(ApiError::unprocessable("Custom patterns are not supported."));
    }
    let mut tx = s.db.begin().await?;
    for ps in b.provider_pattern_settings.unwrap_or_default() {
        let t = ps.token_type.ok_or_else(|| ApiError::validation("PatternConfiguration", "token_type", "missing_field"))?;
        if patterns::by_slug(&t).is_none() {
            return Err(ApiError::validation("PatternConfiguration", "token_type", "invalid"));
        }
        let setting = ps.push_protection_setting.ok_or_else(|| ApiError::validation("PatternConfiguration", "push_protection_setting", "missing_field"))?;
        if !["not-set", "disabled", "enabled"].contains(&setting.as_str()) {
            return Err(ApiError::validation("PatternConfiguration", "push_protection_setting", "invalid"));
        }
        sqlx::query(
            "INSERT INTO secret_scanning_pattern_settings (owner_kind, owner_id, token_type, setting) VALUES ($1,$2,$3,$4)
             ON CONFLICT (owner_kind, owner_id, token_type) DO UPDATE SET setting = $4",
        )
        .bind(kind)
        .bind(owner_id)
        .bind(&t)
        .bind(&setting)
        .execute(&mut *tx)
        .await?;
    }
    let version = uuid::Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO secret_scanning_pattern_versions (owner_kind, owner_id, version) VALUES ($1,$2,$3)
         ON CONFLICT (owner_kind, owner_id) DO UPDATE SET version = $3",
    )
    .bind(kind)
    .bind(owner_id)
    .bind(&version)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "pattern_config_version": version })))
}

async fn org_patterns(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = crate::orgs::org_by_login(&s, &org).await?;
    super::require_security_manager(&s, &o, &u).await?;
    Ok(Json(pattern_config_json(&s, "org", o.id).await?))
}

async fn org_patterns_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<PatternUpdate>) -> ApiResult<Json<Value>> {
    let o = crate::orgs::org_by_login(&s, &org).await?;
    super::require_security_manager(&s, &o, &u).await?;
    update_patterns(&s, "org", o.id, b).await
}

async fn ent_patterns(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    super::enterprise_admin(&u, &e)?;
    Ok(Json(pattern_config_json(&s, "enterprise", 0).await?))
}

async fn ent_patterns_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>, Json(b): Json<PatternUpdate>) -> ApiResult<Json<Value>> {
    super::enterprise_admin(&u, &e)?;
    update_patterns(&s, "enterprise", 0, b).await
}

// ---------------------------------------------------------------------------------------------
// Webhooks

/// `secret_scanning_alert` webhook (`created`, `resolved`, `reopened`). `alert_id` 0 = skip.
pub async fn emit_webhook(s: &AppState, ctx: &RepoCtx, alert_id: i64, action: &str) {
    if alert_id == 0 {
        return;
    }
    let row: Option<AlertRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM secret_scanning_alerts a WHERE a.id = $1")))
        .bind(alert_id)
        .fetch_optional(&s.db)
        .await
        .ok()
        .flatten();
    let Some(row) = row else { return };
    let Ok(mut v) = render(s, &[row], false, true).await else { return };
    let payload = json!({ "action": action, "alert": v.remove(0) });
    if let Err(e) = crate::webhooks::enqueue_event_ctx(s, ctx, "secret_scanning_alert", payload).await {
        tracing::warn!(error = %e, "secret_scanning_alert webhook");
    }
}
