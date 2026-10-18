//! Delegated alert dismissal and push protection bypass requests.
//!
//! When a repository has `{secret_scanning,code_scanning,dependabot}_delegated_alert_dismissal`
//! enabled (through a code security configuration), closing an alert creates a *dismissal
//! request* that a reviewer approves (which applies the dismissal) or denies. With
//! `secret_scanning_delegated_bypass` enabled, bypassing push protection for a secret creates a
//! *bypass request*; approving it grants the bypass. Reviewers: org owners, security managers,
//! holders of the product's write permission, and the configuration's listed reviewers (teams /
//! organization roles); on user-owned repositories, the repository admins.
//!
//! Requests expire after 7 days (`expired` is computed when read).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{CursorParams, Product};
use crate::auth::AuthUser;
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/dismissal-requests/{kind}", get(repo_list))
        .route(
            "/repos/{owner}/{repo}/dismissal-requests/{kind}/{alert_number}",
            get(repo_get).patch(repo_review).post(repo_create).delete(repo_cancel),
        )
        .route("/orgs/{org}/dismissal-requests/{kind}", get(org_list))
        .route("/enterprises/{enterprise}/dismissal-requests/{kind}", get(ent_list))
        .route("/repos/{owner}/{repo}/bypass-requests/secret-scanning", get(bypass_repo_list))
        .route("/repos/{owner}/{repo}/bypass-requests/secret-scanning/{number}", get(bypass_get).patch(bypass_review))
        .route("/orgs/{org}/bypass-requests/secret-scanning", get(bypass_org_list))
        .route("/enterprises/{enterprise}/bypass-requests/secret-scanning", get(bypass_ent_list))
        .route("/repos/{owner}/{repo}/bypass-responses/secret-scanning/{id}", delete(dismiss_response))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    SecretDismissal,
    CodeDismissal,
    DependabotDismissal,
    SecretBypass,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::SecretDismissal => "secret_scanning_dismissal",
            Kind::CodeDismissal => "code_scanning_dismissal",
            Kind::DependabotDismissal => "dependabot_dismissal",
            Kind::SecretBypass => "secret_scanning_bypass",
        }
    }
    fn from_segment(seg: &str) -> Option<Kind> {
        match seg {
            "secret-scanning" => Some(Kind::SecretDismissal),
            "code-scanning" => Some(Kind::CodeDismissal),
            "dependabot" => Some(Kind::DependabotDismissal),
            _ => None,
        }
    }
    fn segment(self) -> &'static str {
        match self {
            Kind::SecretDismissal | Kind::SecretBypass => "secret-scanning",
            Kind::CodeDismissal => "code-scanning",
            Kind::DependabotDismissal => "dependabot",
        }
    }
    fn request_type(self) -> &'static str {
        match self {
            Kind::SecretDismissal => "secret_scanning_closure",
            Kind::CodeDismissal => "code_scanning_alert_dismissal",
            Kind::DependabotDismissal => "dependabot_alert_dismissal",
            Kind::SecretBypass => "push_protection_bypass",
        }
    }
    pub fn product(self) -> Product {
        match self {
            Kind::SecretDismissal | Kind::SecretBypass => Product::SecretScanning,
            Kind::CodeDismissal => Product::CodeScanning,
            Kind::DependabotDismissal => Product::Dependabot,
        }
    }
    /// The repository setting that turns the delegation on.
    pub fn setting(self) -> &'static str {
        match self {
            Kind::SecretDismissal => "secret_scanning_delegated_alert_dismissal",
            Kind::CodeDismissal => "code_scanning_delegated_alert_dismissal",
            Kind::DependabotDismissal => "dependabot_delegated_alert_dismissal",
            Kind::SecretBypass => "secret_scanning_delegated_bypass",
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RequestRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub kind: String,
    pub alert_number: Option<i64>,
    pub requester_id: Option<i64>,
    pub status: String,
    pub reason: Option<String>,
    pub requester_comment: Option<String>,
    pub data: Value,
    pub resource_identifier: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RequestRow {
    pub fn effective_status(&self) -> &str {
        if self.status == "pending" && self.expires_at < Utc::now() { "expired" } else { &self.status }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ResponseRow {
    id: i64,
    request_id: i64,
    reviewer_id: Option<i64>,
    status: String,
    message: Option<String>,
    created_at: DateTime<Utc>,
}

/// Whether delegation `kind` is on for the repository.
pub async fn delegated(s: &AppState, repo_id: i64, kind: Kind) -> bool {
    super::enabled(&s.db, repo_id, kind.setting()).await
}

/// Whether `u` may review requests of `kind` on `ctx`.
pub async fn can_review(s: &AppState, ctx: &RepoCtx, u: &User, kind: Kind) -> bool {
    if u.site_admin {
        return true;
    }
    if ctx.owner.kind != "Organization" {
        return ctx.perm >= Perm::Admin;
    }
    let write_caps: &[&str] = match kind.product() {
        Product::SecretScanning => &["write_secret_scanning"],
        Product::CodeScanning => &["write_code_scanning"],
        Product::Dependabot => &["write_dependabot_alerts"],
    };
    if super::org_cap(&s.db, ctx, u, write_caps).await || super::is_security_manager(&s.db, ctx.owner.id, u.id).await {
        return true;
    }
    is_listed_reviewer(s, ctx, u, kind).await
}

/// Member of a team / holder of a role listed in the delegation's reviewers.
pub async fn is_listed_reviewer(s: &AppState, ctx: &RepoCtx, u: &User, kind: Kind) -> bool {
    let opts = super::product_options(&s.db, ctx.repo.id, kind.setting()).await;
    for r in opts.get("reviewers").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = r.get("reviewer_id").and_then(Value::as_i64) else { continue };
        let hit: bool = match r.get("reviewer_type").and_then(Value::as_str) {
            Some("TEAM") => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM team_members WHERE team_id = $1 AND user_id = $2)")
                .bind(id)
                .bind(u.id)
                .fetch_one(&s.db)
                .await
                .unwrap_or(false),
            Some("ROLE") => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM org_role_users WHERE org_id = $1 AND role_id = $2 AND user_id = $3)
                     OR EXISTS(SELECT 1 FROM org_role_teams x JOIN team_members m ON m.team_id = x.team_id
                                WHERE x.org_id = $1 AND x.role_id = $2 AND m.user_id = $3)",
            )
            .bind(ctx.owner.id)
            .bind(id)
            .bind(u.id)
            .fetch_one(&s.db)
            .await
            .unwrap_or(false),
            _ => false,
        };
        if hit {
            return true;
        }
    }
    false
}

/// Opens a request. Dismissal requests are one per alert (422 when one is pending).
#[allow(clippy::too_many_arguments)]
pub async fn create(
    s: &AppState,
    ctx: &RepoCtx,
    kind: Kind,
    alert_number: Option<i64>,
    requester: &User,
    reason: Option<&str>,
    comment: Option<&str>,
    data: Value,
    resource_identifier: &str,
) -> ApiResult<RequestRow> {
    if let Some(n) = alert_number {
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM advsec_requests WHERE repo_id = $1 AND kind = $2 AND alert_number = $3 AND status = 'pending' AND expires_at > now())",
        )
        .bind(ctx.repo.id)
        .bind(kind.key())
        .bind(n)
        .fetch_one(&s.db)
        .await?;
        if pending {
            return Err(ApiError::unprocessable("A pending dismissal request already exists for this alert."));
        }
    }
    let mut tx = s.db.begin().await?;
    let number = super::next_number(&mut *tx, ctx.repo.id, kind.key()).await?;
    let row: RequestRow = sqlx::query_as(
        "INSERT INTO advsec_requests (repo_id, number, kind, alert_number, requester_id, status, reason, requester_comment, data,
                                      resource_identifier, expires_at)
         VALUES ($1,$2,$3,$4,$5,'pending',$6,$7,$8,$9, now() + interval '7 days') RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(number)
    .bind(kind.key())
    .bind(alert_number)
    .bind(requester.id)
    .bind(reason)
    .bind(comment)
    .bind(&data)
    .bind(resource_identifier)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

/// The latest request for an alert.
pub async fn for_alert(s: &AppState, repo_id: i64, kind: Kind, alert_number: i64) -> ApiResult<Option<RequestRow>> {
    Ok(sqlx::query_as("SELECT * FROM advsec_requests WHERE repo_id = $1 AND kind = $2 AND alert_number = $3 ORDER BY id DESC LIMIT 1")
        .bind(repo_id)
        .bind(kind.key())
        .bind(alert_number)
        .fetch_optional(&s.db)
        .await?)
}

/// Latest request per alert number for a set of alerts (batch).
pub async fn for_alerts(s: &AppState, repo_ids: &[i64], kind: Kind) -> ApiResult<Vec<RequestRow>> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT ON (repo_id, alert_number) * FROM advsec_requests WHERE repo_id = ANY($1) AND kind = $2 AND alert_number IS NOT NULL
          ORDER BY repo_id, alert_number, id DESC",
    )
    .bind(repo_ids)
    .bind(kind.key())
    .fetch_all(&s.db)
    .await?)
}

async fn responses(s: &AppState, ids: &[i64]) -> ApiResult<Vec<ResponseRow>> {
    Ok(sqlx::query_as("SELECT * FROM advsec_request_responses WHERE request_id = ANY($1) ORDER BY id").bind(ids).fetch_all(&s.db).await?)
}

fn actor(users: &std::collections::HashMap<i64, User>, id: Option<i64>) -> Value {
    match id.and_then(|i| users.get(&i)) {
        Some(u) => json!({ "actor_id": u.id, "actor_name": u.login }),
        None => json!({ "actor_id": id.unwrap_or(0), "actor_name": "ghost" }),
    }
}

fn request_url(s: &AppState, ctx: &RepoCtx, kind: Kind, r: &RequestRow) -> (String, String) {
    let full = ctx.full_name();
    match kind {
        Kind::SecretBypass => (
            format!("{}/repos/{full}/bypass-requests/secret-scanning/{}", s.api_url(), r.number),
            format!("{}/{full}/security/secret-scanning/push-protection-bypasses/{}", s.base_url(), r.number),
        ),
        _ => {
            let n = r.alert_number.unwrap_or(r.number);
            let html = match kind {
                Kind::SecretDismissal => format!("{}/{full}/security/secret-scanning/{n}", s.base_url()),
                Kind::CodeDismissal => format!("{}/{full}/security/code-scanning/{n}", s.base_url()),
                _ => format!("{}/{full}/security/dependabot/{n}", s.base_url()),
            };
            (format!("{}/repos/{full}/dismissal-requests/{}/{n}", s.api_url(), kind.segment()), html)
        }
    }
}

/// Renders requests (batch-loading users, responses and repositories).
pub async fn render(s: &AppState, kind: Kind, rows: &[RequestRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let resp = responses(s, &ids).await?;
    let users = super::users_by_id(&s.db, rows.iter().filter_map(|r| r.requester_id).chain(resp.iter().filter_map(|x| x.reviewer_id))).await?;
    let repos = super::repos_by_id(&s.db, rows.iter().map(|r| r.repo_id)).await?;
    let mut out = Vec::new();
    for r in rows {
        let Some(ctx) = repos.get(&r.repo_id) else { continue };
        let (url, html) = request_url(s, ctx, kind, r);
        let rs: Vec<Value> = resp
            .iter()
            .filter(|x| x.request_id == r.id)
            .map(|x| json!({ "id": x.id, "reviewer": actor(&users, x.reviewer_id), "message": x.message, "status": x.status, "created_at": crate::json::ts(x.created_at) }))
            .collect();
        let mut v = json!({
            "id": r.id,
            "number": r.number,
            "repository": { "id": ctx.repo.id, "name": ctx.repo.name, "full_name": ctx.full_name() },
            "requester": actor(&users, r.requester_id),
            "request_type": kind.request_type(),
            "data": r.data,
            "resource_identifier": r.resource_identifier,
            "status": r.effective_status(),
            "requester_comment": r.requester_comment,
            "expires_at": crate::json::ts(r.expires_at),
            "created_at": crate::json::ts(r.created_at),
            "responses": rs,
            "url": url,
            "html_url": html,
        });
        if ctx.owner.kind == "Organization" {
            v["organization"] = json!({ "id": ctx.owner.id, "name": ctx.owner.login });
        }
        out.push(v);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Listing

#[derive(Deserialize, Default)]
struct ListQ {
    repository_name: Option<String>,
    organization_name: Option<String>,
    reviewer: Option<String>,
    requester: Option<String>,
    time_period: Option<String>,
    request_status: Option<String>,
    #[serde(flatten)]
    cursor: CursorParams,
}

enum Scope {
    Repo(i64),
    Org(i64),
    Enterprise,
}

async fn list_resp(s: &AppState, uri: &OriginalUri, kind: Kind, scope: Scope, q: ListQ, default_period: &str) -> ApiResult<Response> {
    super::check_enum("Request", "time_period", &q.time_period, &["hour", "day", "week", "month"])?;
    super::check_enum("Request", "request_status", &q.request_status, &["completed", "cancelled", "approved", "expired", "deleted", "denied", "open", "all", "pending"])?;
    let period = match q.time_period.as_deref().unwrap_or(default_period) {
        "hour" => "1 hour",
        "week" => "7 days",
        "month" => "30 days",
        _ => "1 day",
    };
    let (scope_sql, scope_id) = match scope {
        Scope::Repo(id) => ("q.repo_id = $2", id),
        Scope::Org(id) => ("r.owner_id = $2", id),
        Scope::Enterprise => ("o.kind = 'Organization' AND $2 = 0", 0),
    };
    let rows: Vec<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT q.* FROM advsec_requests q JOIN repositories r ON r.id = q.repo_id JOIN users o ON o.id = r.owner_id
          WHERE q.kind = $1 AND {scope_sql} AND q.created_at > now() - interval '{period}'
            AND ($3::text IS NULL OR lower(r.name) = lower($3))
            AND ($4::text IS NULL OR lower(o.login) = lower($4))
            AND ($5::text IS NULL OR q.requester_id = (SELECT id FROM users WHERE lower(login) = lower($5)))
            AND ($6::text IS NULL OR EXISTS(SELECT 1 FROM advsec_request_responses x JOIN users ru ON ru.id = x.reviewer_id
                                             WHERE x.request_id = q.id AND lower(ru.login) = lower($6)))
          ORDER BY q.id DESC"
    )))
    .bind(kind.key())
    .bind(scope_id)
    .bind(&q.repository_name)
    .bind(&q.organization_name)
    .bind(&q.requester)
    .bind(&q.reviewer)
    .fetch_all(&s.db)
    .await?;
    let want = q.request_status.clone().unwrap_or_else(|| "all".into());
    let rows: Vec<RequestRow> = rows
        .into_iter()
        .filter(|r| match want.as_str() {
            "all" => true,
            "open" | "pending" => r.effective_status() == "pending",
            other => r.effective_status() == other,
        })
        .collect();
    let total = rows.len() as i64;
    let page: Vec<RequestRow> = rows.into_iter().skip(q.cursor.offset() as usize).take(q.cursor.per_page() as usize).collect();
    let items = render(s, kind, &page).await?;
    Ok(super::list_response(s, uri, &q.cursor, total, items))
}

fn kind_of(seg: &str) -> ApiResult<Kind> {
    Kind::from_segment(seg).ok_or_else(ApiError::not_found)
}

async fn repo_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r, k)): Path<(String, String, String)>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let kind = kind_of(&k)?;
    let ctx = super::repo_for(&s, &u, &o, &r, kind.product(), false).await?;
    list_resp(&s, &uri, kind, Scope::Repo(ctx.repo.id), q, if kind == Kind::SecretDismissal { "day" } else { "month" }).await
}

async fn org_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((org, k)): Path<(String, String)>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let kind = kind_of(&k)?;
    let o = super::org_for(&s, &u, &org, kind.product()).await?;
    list_resp(&s, &uri, kind, Scope::Org(o.id), q, if kind == Kind::SecretDismissal { "day" } else { "month" }).await
}

async fn ent_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((e, k)): Path<(String, String)>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let kind = kind_of(&k)?;
    if kind != Kind::SecretDismissal {
        return Err(ApiError::not_found());
    }
    super::enterprise_admin(&u, &e)?;
    list_resp(&s, &uri, kind, Scope::Enterprise, q, "day").await
}

async fn bypass_repo_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): Path<(String, String)>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let ctx = super::repo_for(&s, &u, &o, &r, Product::SecretScanning, false).await?;
    list_resp(&s, &uri, Kind::SecretBypass, Scope::Repo(ctx.repo.id), q, "day").await
}

async fn bypass_org_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let o = super::org_for(&s, &u, &org, Product::SecretScanning).await?;
    list_resp(&s, &uri, Kind::SecretBypass, Scope::Org(o.id), q, "day").await
}

async fn bypass_ent_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(e): Path<String>, Query(q): Query<ListQ>) -> ApiResult<Response> {
    super::enterprise_admin(&u, &e)?;
    list_resp(&s, &uri, Kind::SecretBypass, Scope::Enterprise, q, "day").await
}

// ---------------------------------------------------------------------------------------------
// Single requests

async fn load_repo_for_request(s: &AppState, u: &User, o: &str, r: &str, kind: Kind) -> ApiResult<RepoCtx> {
    let ctx = crate::db::load_repo(&s.db, Some(u), o, r).await?;
    if super::check_repo(s, &ctx, u, kind.product(), false).await.is_err() && ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Resource not accessible by personal access token"));
    }
    Ok(ctx)
}

async fn repo_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, k, n)): Path<(String, String, String, i64)>) -> ApiResult<Json<Value>> {
    let kind = kind_of(&k)?;
    let ctx = load_repo_for_request(&s, &u, &o, &r, kind).await?;
    let row = for_alert(&s, ctx.repo.id, kind, n).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(render(&s, kind, &[row]).await?.remove(0)))
}

#[derive(Deserialize)]
struct ReviewBody {
    status: Option<String>,
    message: Option<String>,
}

fn review_body(b: &ReviewBody, approve: &str, deny: &[&str]) -> ApiResult<(bool, String)> {
    let message = b.message.clone().ok_or_else(|| ApiError::validation("Review", "message", "missing_field"))?;
    match b.status.as_deref() {
        Some(x) if x == approve => Ok((true, message)),
        Some(x) if deny.contains(&x) => Ok((false, message)),
        Some(_) => Err(ApiError::validation("Review", "status", "invalid")),
        None => Err(ApiError::validation("Review", "status", "missing_field")),
    }
}

/// Records a review; returns the response id.
async fn record_review(s: &AppState, req: &RequestRow, reviewer: &User, approve: bool, message: &str) -> ApiResult<i64> {
    let status = if approve { "approved" } else { "denied" };
    let mut tx = s.db.begin().await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO advsec_request_responses (request_id, reviewer_id, status, message) VALUES ($1,$2,$3,$4) RETURNING id")
        .bind(req.id)
        .bind(reviewer.id)
        .bind(status)
        .bind(message)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE advsec_requests SET status = $2, updated_at = now() WHERE id = $1").bind(req.id).bind(status).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

async fn repo_review(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, k, n)): Path<(String, String, String, i64)>, Json(b): Json<ReviewBody>) -> ApiResult<Response> {
    let kind = kind_of(&k)?;
    let ctx = crate::db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let (approve, message) = review_body(&b, "approve", &["deny", "reject"])?;
    let req = for_alert(&s, ctx.repo.id, kind, n).await?.ok_or_else(ApiError::not_found)?;
    if !can_review(&s, &ctx, &u, kind).await {
        return Err(ApiError::forbidden("You are not allowed to review dismissal requests for this repository."));
    }
    if req.effective_status() != "pending" {
        return Err(ApiError::unprocessable(format!("The dismissal request is {} and cannot be reviewed.", req.effective_status())));
    }
    let id = record_review(&s, &req, &u, approve, &message).await?;
    if approve {
        let requester = match req.requester_id {
            Some(id) => crate::db::user_by_id(&s.db, id).await.ok(),
            None => None,
        };
        let by = requester.as_ref().unwrap_or(&u);
        match kind {
            Kind::SecretDismissal => super::secret_scanning_api::apply_dismissal(&s, &ctx, n, by, &u, &req).await?,
            Kind::CodeDismissal => super::code_scanning::apply_dismissal(&s, &ctx, n, by, &u, &req).await?,
            Kind::DependabotDismissal => super::dependabot::apply_dismissal(&s, &ctx, n, by, &req).await?,
            Kind::SecretBypass => {}
        }
    }
    match kind {
        Kind::CodeDismissal => Ok(StatusCode::NO_CONTENT.into_response()),
        _ => Ok(Json(json!({ "dismissal_review_id": id })).into_response()),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    dismissed_reason: Option<String>,
    dismissed_comment: Option<String>,
}

/// `POST /repos/{o}/{r}/dismissal-requests/dependabot/{alert_number}`.
async fn repo_create(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, k, n)): Path<(String, String, String, i64)>, Json(b): Json<CreateBody>) -> ApiResult<Response> {
    let kind = kind_of(&k)?;
    if kind != Kind::DependabotDismissal {
        return Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed"));
    }
    let ctx = super::repo_for(&s, &u, &o, &r, Product::Dependabot, false).await?;
    let reason = b.dismissed_reason.ok_or_else(|| ApiError::validation("DismissalRequest", "dismissed_reason", "missing_field"))?;
    if !super::dependabot::DISMISS_REASONS.contains(&reason.as_str()) {
        return Err(ApiError::validation("DismissalRequest", "dismissed_reason", "invalid"));
    }
    let row = super::dependabot::request_dismissal(&s, &ctx, &u, n, &reason, b.dismissed_comment.as_deref()).await?;
    let v = render(&s, kind, &[row]).await?.remove(0);
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

/// `DELETE /repos/{o}/{r}/dismissal-requests/dependabot/{alert_number}`: the requester (or a
/// reviewer) withdraws a pending request.
async fn repo_cancel(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, k, n)): Path<(String, String, String, i64)>) -> ApiResult<Response> {
    let kind = kind_of(&k)?;
    if kind != Kind::DependabotDismissal {
        return Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed"));
    }
    let ctx = super::repo_for(&s, &u, &o, &r, Product::Dependabot, false).await?;
    let req = for_alert(&s, ctx.repo.id, kind, n).await?.ok_or_else(ApiError::not_found)?;
    if req.effective_status() != "pending" {
        return Err(ApiError::unprocessable("Only pending dismissal requests can be cancelled."));
    }
    if req.requester_id != Some(u.id) && !can_review(&s, &ctx, &u, kind).await {
        return Err(ApiError::forbidden("Only the requester can cancel this dismissal request."));
    }
    sqlx::query("DELETE FROM advsec_requests WHERE id = $1").bind(req.id).execute(&s.db).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------------------------
// Bypass requests

async fn bypass_row(s: &AppState, repo_id: i64, number: i64) -> ApiResult<RequestRow> {
    sqlx::query_as("SELECT * FROM advsec_requests WHERE repo_id = $1 AND kind = $2 AND number = $3")
        .bind(repo_id)
        .bind(Kind::SecretBypass.key())
        .bind(number)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn bypass_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = load_repo_for_request(&s, &u, &o, &r, Kind::SecretBypass).await?;
    let row = bypass_row(&s, ctx.repo.id, n).await?;
    Ok(Json(render(&s, Kind::SecretBypass, &[row]).await?.remove(0)))
}

async fn bypass_review(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): Path<(String, String, i64)>, Json(b): Json<ReviewBody>) -> ApiResult<Json<Value>> {
    let ctx = crate::db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let (approve, message) = review_body(&b, "approve", &["reject", "deny"])?;
    let req = bypass_row(&s, ctx.repo.id, n).await?;
    if !can_review(&s, &ctx, &u, Kind::SecretBypass).await {
        return Err(ApiError::forbidden("You are not allowed to review bypass requests for this repository."));
    }
    if req.effective_status() != "pending" {
        return Err(ApiError::unprocessable(format!("The bypass request is {} and cannot be reviewed.", req.effective_status())));
    }
    let id = record_review(&s, &req, &u, approve, &message).await?;
    if approve {
        super::secret_scanning::grant_bypass_from_request(&s, &ctx, &req).await?;
    }
    Ok(Json(json!({ "bypass_review_id": id })))
}

/// Dismisses a review on a bypass request: the request is pending again and an approval's
/// bypass is withdrawn.
async fn dismiss_response(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = crate::db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let row: Option<(i64, String, i64)> = sqlx::query_as(
        "SELECT x.request_id, x.status, q.number FROM advsec_request_responses x JOIN advsec_requests q ON q.id = x.request_id
          WHERE x.id = $1 AND q.repo_id = $2 AND q.kind = $3",
    )
    .bind(id)
    .bind(ctx.repo.id)
    .bind(Kind::SecretBypass.key())
    .fetch_optional(&s.db)
    .await?;
    let (request_id, status, _) = row.ok_or_else(ApiError::not_found)?;
    if !can_review(&s, &ctx, &u, Kind::SecretBypass).await {
        return Err(ApiError::forbidden("You are not allowed to dismiss bypass responses for this repository."));
    }
    if status == "dismissed" {
        return Err(ApiError::unprocessable("This response has already been dismissed."));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE advsec_request_responses SET status = 'dismissed' WHERE id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE advsec_requests SET status = 'pending', updated_at = now() WHERE id = $1 AND status IN ('approved', 'denied')")
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
    if status == "approved" {
        sqlx::query("DELETE FROM secret_scanning_push_bypasses WHERE request_id = $1").bind(request_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
