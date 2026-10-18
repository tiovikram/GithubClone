//! Push-ruleset delegated bypass requests (`push-rule-bypass-request`).
//!
//! A push blocked by an active push ruleset that has bypass actors creates (or reuses) a
//! `pending` request for the pushed commit (`resource_identifier`). A bypass actor of one of the
//! request's rulesets (or a repository admin) approves or denies it with
//! `PATCH /repos/{o}/{r}/bypass-requests/push-rules/{number}` (`{"status": "approve" | "deny"}`,
//! forge's counterpart of GitHub's web UI). Pushing the same commit again while the request is
//! approved and unexpired (7 days) succeeds and completes the request.
use serde_json::{json, Value};

use super::{Actor, RefUpdate, Violation};
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RequestRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub requester_id: Option<i64>,
    pub requester_name: String,
    #[sqlx(rename = "ref")]
    pub git_ref: String,
    pub resource_identifier: String,
    pub data: Value,
    pub status: String,
    pub requester_comment: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn data_of(violations: &[Violation]) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for v in violations {
        out.push(json!({"ruleset_id": v.ruleset_id, "ruleset_name": v.ruleset_name, "total_violations": 1, "rule_type": v.rule_type}));
    }
    Value::Array(out)
}

pub fn html_url(s: &AppState, full_name: &str, number: i64) -> String {
    format!("{}/{full_name}/exemptions/{number}", s.base_url())
}

/// Whether an approved, unexpired request of `actor` for `sha` covers every violated ruleset;
/// if so it is marked completed.
pub async fn approved_for(s: &AppState, ctx: &RepoCtx, actor: &Actor, sha: &str, violations: &[Violation]) -> ApiResult<bool> {
    let Some(u) = &actor.user else { return Ok(false) };
    let rows: Vec<RequestRow> = sqlx::query_as(
        "SELECT * FROM push_bypass_requests WHERE repo_id = $1 AND requester_id = $2 AND resource_identifier = $3
            AND status IN ('approved', 'completed') AND expires_at > now() ORDER BY id DESC",
    )
    .bind(ctx.repo.id)
    .bind(u.id)
    .bind(sha)
    .fetch_all(&s.db)
    .await?;
    for r in rows {
        let ids: Vec<i64> = r.data.as_array().into_iter().flatten().filter_map(|d| d.get("ruleset_id").and_then(Value::as_i64)).collect();
        if violations.iter().all(|v| ids.contains(&v.ruleset_id)) {
            sqlx::query("UPDATE push_bypass_requests SET status = 'completed', updated_at = now() WHERE id = $1").bind(r.id).execute(&s.db).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// The pending request of `actor` for this update (created when missing): (number, html url).
pub async fn request_for(s: &AppState, ctx: &RepoCtx, actor: &Actor, upd: &RefUpdate, violations: &[Violation]) -> ApiResult<(i64, String)> {
    let full = ctx.full_name();
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT number FROM push_bypass_requests WHERE repo_id = $1 AND requester_id IS NOT DISTINCT FROM $2 AND resource_identifier = $3
            AND status = 'pending' AND expires_at > now() ORDER BY id DESC LIMIT 1",
    )
    .bind(ctx.repo.id)
    .bind(actor.user.as_ref().map(|u| u.id))
    .bind(&upd.new)
    .fetch_optional(&s.db)
    .await?;
    if let Some(n) = existing {
        return Ok((n, html_url(s, &full, n)));
    }
    // Per-repository numbers; the unique index settles races (retry once).
    for _ in 0..3 {
        let res: Result<i64, sqlx::Error> = sqlx::query_scalar(
            "INSERT INTO push_bypass_requests (repo_id, number, requester_id, requester_name, ref, resource_identifier, data)
             SELECT $1, COALESCE(MAX(number), 0) + 1, $2, $3, $4, $5, $6 FROM push_bypass_requests WHERE repo_id = $1
             RETURNING number",
        )
        .bind(ctx.repo.id)
        .bind(actor.user.as_ref().map(|u| u.id))
        .bind(actor.user.as_ref().map(|u| u.login.clone()).unwrap_or_default())
        .bind(&upd.refname)
        .bind(&upd.new)
        .bind(data_of(violations))
        .fetch_one(&s.db)
        .await;
        match res {
            Ok(n) => return Ok((n, html_url(s, &full, n))),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::conflict("could not allocate a bypass request number"))
}

/// Expires old pending/approved requests (lazily, before reads).
async fn expire(s: &AppState) {
    let _ = sqlx::query("UPDATE push_bypass_requests SET status = 'expired', updated_at = now() WHERE status IN ('pending', 'approved') AND expires_at <= now()")
        .execute(&s.db)
        .await;
}

#[derive(sqlx::FromRow)]
struct Joined {
    #[sqlx(flatten)]
    req: RequestRow,
    repo_name: String,
    owner_login: String,
    owner_id: i64,
    owner_kind: String,
}

/// `push-rule-bypass-request` JSON for rows (responses batch-loaded).
async fn render(s: &AppState, rows: Vec<Joined>) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.req.id).collect();
    let responses: Vec<(i64, i64, Option<i64>, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, request_id, reviewer_id, reviewer_name, status, created_at FROM push_bypass_responses WHERE request_id = ANY($1) ORDER BY id",
    )
    .bind(&ids)
    .fetch_all(&s.db)
    .await?;
    let api = s.api_url();
    Ok(rows
        .into_iter()
        .map(|j| {
            let r = &j.req;
            let full = format!("{}/{}", j.owner_login, j.repo_name);
            let resp: Vec<Value> = responses
                .iter()
                .filter(|x| x.1 == r.id)
                .map(|x| json!({"id": x.0, "reviewer": {"actor_id": x.2, "actor_name": x.3}, "status": x.4, "created_at": crate::json::ts(x.5)}))
                .collect();
            let org = if j.owner_kind == "Organization" { json!({"id": j.owner_id, "name": j.owner_login}) } else { json!({"id": null, "name": null}) };
            json!({
                "id": r.id,
                "number": r.number,
                "repository": {"id": r.repo_id, "name": j.repo_name, "full_name": full},
                "organization": org,
                "requester": {"actor_id": r.requester_id, "actor_name": r.requester_name},
                "request_type": "push_ruleset_bypass",
                "data": r.data,
                "resource_identifier": r.resource_identifier,
                "status": r.status,
                "requester_comment": r.requester_comment,
                "expires_at": crate::json::ts(r.expires_at),
                "created_at": crate::json::ts(r.created_at),
                "responses": resp,
                "url": format!("{api}/repos/{full}/bypass-requests/push-rules/{}", r.number),
                "html_url": html_url(s, &full, r.number),
            })
        })
        .collect())
}

/// Filters of the list endpoints.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ListFilter {
    pub repository_name: Option<String>,
    pub organization_name: Option<String>,
    pub reviewer: Option<String>,
    pub requester: Option<String>,
    pub time_period: Option<String>,
    pub request_status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

const STATUSES: [&str; 8] = ["completed", "cancelled", "approved", "expired", "deleted", "denied", "open", "all"];

/// Validates the query (422 like GitHub).
pub fn validate(f: &ListFilter) -> ApiResult<&'static str> {
    let interval = match f.time_period.as_deref().unwrap_or("day") {
        "hour" => "1 hour",
        "day" => "1 day",
        "week" => "7 days",
        "month" => "1 month",
        _ => return Err(ApiError::validation("BypassRequest", "time_period", "invalid")),
    };
    if f.request_status.as_deref().is_some_and(|x| !STATUSES.contains(&x) && x != "pending") {
        return Err(ApiError::validation("BypassRequest", "request_status", "invalid"));
    }
    Ok(interval)
}

/// Requests of one repository, of an organization's repositories, or of every repository.
pub async fn list(s: &AppState, repo_id: Option<i64>, org_id: Option<i64>, f: &ListFilter) -> ApiResult<Vec<Value>> {
    let interval = validate(f)?;
    expire(s).await;
    let per_page = f.per_page.unwrap_or(30).clamp(1, 100);
    let offset = (f.page.unwrap_or(1).max(1) - 1) * per_page;
    // `open` = pending or approved (not yet used); `all` / none = everything.
    let status = f.request_status.clone().filter(|x| x != "all");
    let sql = format!(
        "SELECT b.*, r.name AS repo_name, o.login AS owner_login, o.id AS owner_id, o.kind AS owner_kind
           FROM push_bypass_requests b JOIN repositories r ON r.id = b.repo_id JOIN users o ON o.id = r.owner_id
          WHERE ($1::bigint IS NULL OR b.repo_id = $1) AND ($2::bigint IS NULL OR r.owner_id = $2)
            AND ($3::text IS NULL OR lower(r.name) = lower($3)) AND ($4::text IS NULL OR lower(o.login) = lower($4))
            AND ($5::text IS NULL OR lower(b.requester_name) = lower($5))
            AND ($6::text IS NULL OR EXISTS (SELECT 1 FROM push_bypass_responses x WHERE x.request_id = b.id AND lower(x.reviewer_name) = lower($6)))
            AND ($7::text IS NULL OR b.status = $7 OR ($7 = 'open' AND b.status IN ('pending', 'approved')))
            AND b.created_at > now() - interval '{interval}'
          ORDER BY b.id DESC LIMIT $8 OFFSET $9"
    );
    let rows: Vec<Joined> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(repo_id)
        .bind(org_id)
        .bind(&f.repository_name)
        .bind(&f.organization_name)
        .bind(&f.requester)
        .bind(&f.reviewer)
        .bind(&status)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&s.db)
        .await?;
    render(s, rows).await
}

pub async fn get(s: &AppState, repo_id: i64, number: i64) -> ApiResult<Value> {
    expire(s).await;
    let rows: Vec<Joined> = sqlx::query_as(
        "SELECT b.*, r.name AS repo_name, o.login AS owner_login, o.id AS owner_id, o.kind AS owner_kind
           FROM push_bypass_requests b JOIN repositories r ON r.id = b.repo_id JOIN users o ON o.id = r.owner_id
          WHERE b.repo_id = $1 AND b.number = $2",
    )
    .bind(repo_id)
    .bind(number)
    .fetch_all(&s.db)
    .await?;
    render(s, rows).await?.into_iter().next().ok_or_else(ApiError::not_found)
}

/// Approves or denies a pending request. Reviewers: bypass actors (`always`) of a ruleset the
/// request names, or repository admins.
pub async fn review(s: &AppState, ctx: &RepoCtx, reviewer: &User, number: i64, status: &str) -> ApiResult<Value> {
    let new_status = match status {
        "approve" | "approved" => "approved",
        "deny" | "denied" => "denied",
        _ => return Err(ApiError::validation("BypassRequest", "status", "invalid")),
    };
    expire(s).await;
    let req: RequestRow = sqlx::query_as("SELECT * FROM push_bypass_requests WHERE repo_id = $1 AND number = $2")
        .bind(ctx.repo.id)
        .bind(number)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if req.requester_id == Some(reviewer.id) {
        return Err(ApiError::forbidden("You cannot review your own bypass request."));
    }
    let mut allowed = ctx.perm >= Perm::Admin;
    if !allowed {
        let actor = Actor::new(ctx, reviewer);
        let ids: Vec<i64> = req.data.as_array().into_iter().flatten().filter_map(|d| d.get("ruleset_id").and_then(Value::as_i64)).collect();
        for r in super::repo_rulesets(s, ctx).await? {
            if ids.contains(&r.id) && super::bypass_mode(s, ctx, &actor, &r).await >= super::BypassMode::Always {
                allowed = true;
            }
        }
    }
    if !allowed {
        return Err(ApiError::forbidden("You must be a bypass actor of the ruleset to review this request."));
    }
    if req.status != "pending" && req.status != "approved" && req.status != "denied" {
        return Err(ApiError::unprocessable(format!("Bypass request is {}", req.status)));
    }
    sqlx::query("UPDATE push_bypass_requests SET status = $2, updated_at = now() WHERE id = $1").bind(req.id).bind(new_status).execute(&s.db).await?;
    sqlx::query("INSERT INTO push_bypass_responses (request_id, reviewer_id, reviewer_name, status) VALUES ($1, $2, $3, $4)")
        .bind(req.id)
        .bind(reviewer.id)
        .bind(&reviewer.login)
        .bind(new_status)
        .execute(&s.db)
        .await?;
    get(s, ctx.repo.id, number).await
}

/// Cancels the requester's own pending request.
pub async fn cancel(s: &AppState, ctx: &RepoCtx, user: &User, number: i64) -> ApiResult<()> {
    let n = sqlx::query("UPDATE push_bypass_requests SET status = 'cancelled', updated_at = now() WHERE repo_id = $1 AND number = $2 AND requester_id = $3 AND status IN ('pending', 'approved')")
        .bind(ctx.repo.id)
        .bind(number)
        .bind(user.id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}
