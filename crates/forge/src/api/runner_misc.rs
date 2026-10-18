//! Actions odds and ends: organization OIDC subject customisation (the repository level lives in
//! actions_settings), cache usage and cache size policy,
//! run approvals, pending deployments and custom deployment protection rule reviews.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::plat::total_envelope;
use super::{no_content, PageParams};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/actions";
    Router::new()
        .route("/orgs/{org}/actions/oidc/customization/sub", get(org_oidc).put(put_org_oidc))
        .route(&format!("{r}/cache/usage"), get(repo_cache_usage))
        .route(&format!("{r}/cache/usage-policy"), get(repo_cache_policy).patch(set_repo_cache_policy))
        .route("/orgs/{org}/actions/cache/usage", get(org_cache_usage))
        .route("/orgs/{org}/actions/cache/usage-by-repository", get(org_cache_usage_by_repo))
        .route("/enterprises/{enterprise}/actions/cache/usage", get(enterprise_cache_usage))
        .route("/enterprises/{enterprise}/actions/cache/usage-policy", get(enterprise_cache_policy).patch(set_enterprise_cache_policy))
        .route(&format!("{r}/runs/{{run_id}}/approvals"), get(approvals))
        .route(&format!("{r}/runs/{{run_id}}/pending_deployments"), get(pending_deployments).post(review_pending_deployments))
        .route(&format!("{r}/runs/{{run_id}}/deployment_protection_rule"), post(review_custom_gates))
}

async fn org_admin(s: &AppState, u: &User, org: &str) -> ApiResult<User> {
    let o = db::user_by_login(&s.db, org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    if u.site_admin {
        return Ok(o);
    }
    match super::orgs_extra::org_role(&s.db, o.id, u.id).await?.as_deref() {
        Some("admin") => Ok(o),
        Some(_) => Err(ApiError::forbidden("You must be an organization owner.")),
        None => Err(ApiError::not_found()),
    }
}

fn site_admin(u: &User) -> ApiResult<()> {
    if u.site_admin {
        Ok(())
    } else {
        Err(ApiError::forbidden("You must be a site administrator."))
    }
}

// ---------------------------------------------------------------------------------------------
// OIDC subject claim

/// Claim keys GitHub accepts in a subject template.
const CLAIMS: &[&str] = &[
    "repo", "context", "ref", "ref_type", "repository", "repository_id", "repository_owner", "repository_owner_id",
    "repository_visibility", "actor", "actor_id", "environment", "event_name", "head_ref", "base_ref", "job_workflow_ref",
    "job_workflow_sha", "workflow", "workflow_ref", "workflow_sha", "run_id", "run_number", "run_attempt", "runner_environment",
    "enterprise", "enterprise_id", "check_run_id",
];

fn check_claims(keys: &[String]) -> ApiResult<()> {
    if let Some(bad) = keys.iter().find(|k| !CLAIMS.contains(&k.as_str())) {
        return Err(ApiError::unprocessable(format!("Invalid claim key: {bad}")));
    }
    Ok(())
}

async fn load_oidc(s: &AppState, kind: &str, id: i64) -> ApiResult<Option<(bool, Vec<String>)>> {
    Ok(sqlx::query_as("SELECT use_default, include_claim_keys FROM actions_oidc_subs WHERE scope_kind = $1 AND scope_id = $2")
        .bind(kind)
        .bind(id)
        .fetch_optional(&s.db)
        .await?)
}

async fn store_oidc(s: &AppState, kind: &str, id: i64, use_default: bool, keys: &[String]) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO actions_oidc_subs (scope_kind, scope_id, use_default, include_claim_keys) VALUES ($1, $2, $3, $4)
         ON CONFLICT (scope_kind, scope_id) DO UPDATE SET use_default = EXCLUDED.use_default, include_claim_keys = EXCLUDED.include_claim_keys",
    )
    .bind(kind)
    .bind(id)
    .bind(use_default)
    .bind(keys)
    .execute(&s.db)
    .await?;
    Ok(())
}

/// The subject claim template a job of `ctx` gets: the repository setting (actions-settings
/// area), else the organization template (this module), else GitHub's default.
pub async fn subject_claim_keys(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<String>> {
    if let Some(keys) = super::actions_settings::policy::oidc_sub_claim_keys(s, ctx.repo.id).await? {
        return Ok(keys);
    }
    if let Some((_, keys)) = load_oidc(s, "org", ctx.owner.id).await? {
        if !keys.is_empty() {
            return Ok(keys);
        }
    }
    Ok(vec!["repo".into(), "context".into()])
}

async fn org_oidc(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_admin(&s, &u, &org).await?;
    let (_, keys) = load_oidc(&s, "org", o.id).await?.unwrap_or((true, vec!["repo".into(), "context".into()]));
    Ok(Json(json!({ "include_claim_keys": keys })))
}

#[derive(Deserialize)]
struct OrgOidcBody {
    include_claim_keys: Option<Vec<String>>,
}

async fn put_org_oidc(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<OrgOidcBody>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let keys = b.include_claim_keys.ok_or_else(|| ApiError::validation("OidcSubjectClaim", "include_claim_keys", "missing_field"))?;
    check_claims(&keys)?;
    store_oidc(&s, "org", o.id, false, &keys).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

// ---------------------------------------------------------------------------------------------
// Cache usage and policy

async fn usage_for(s: &AppState, repo_ids: Option<&[i64]>) -> ApiResult<(i64, i64)> {
    let (n, size): (i64, Option<i64>) = sqlx::query_as("SELECT count(*), sum(size_in_bytes)::bigint FROM actions_caches WHERE ($1::bigint[] IS NULL OR repo_id = ANY($1))")
        .bind(repo_ids)
        .fetch_one(&s.db)
        .await?;
    Ok((n, size.unwrap_or(0)))
}

async fn repo_cache_usage(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    ctx.require(Perm::Read)?;
    let (n, size) = usage_for(&s, Some(&[ctx.repo.id])).await?;
    Ok(Json(json!({ "full_name": ctx.full_name(), "active_caches_size_in_bytes": size, "active_caches_count": n })))
}

async fn repo_cache_policy(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let gb = crate::runner::storage::cache_limit_gb(&s, ctx.repo.id).await?;
    Ok(Json(json!({ "repo_cache_size_limit_in_gb": gb })))
}

#[derive(Deserialize)]
struct RepoPolicyBody {
    repo_cache_size_limit_in_gb: Option<i64>,
}

async fn set_repo_cache_policy(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>, Json(b): Json<RepoPolicyBody>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let gb = b.repo_cache_size_limit_in_gb.ok_or_else(|| ApiError::validation("CacheUsagePolicy", "repo_cache_size_limit_in_gb", "missing_field"))?;
    let max: Option<i32> = sqlx::query_scalar("SELECT max_repo_cache_size_limit_in_gb FROM actions_cache_policies WHERE scope_kind = 'enterprise' AND scope_id = 0")
        .fetch_optional(&s.db)
        .await?
        .flatten();
    if gb < 0 || gb > max.unwrap_or(10000) as i64 {
        return Err(ApiError::unprocessable(format!("repo_cache_size_limit_in_gb must be between 0 and {}", max.unwrap_or(10000))));
    }
    sqlx::query(
        "INSERT INTO actions_cache_policies (scope_kind, scope_id, repo_cache_size_limit_in_gb) VALUES ('repo', $1, $2)
         ON CONFLICT (scope_kind, scope_id) DO UPDATE SET repo_cache_size_limit_in_gb = EXCLUDED.repo_cache_size_limit_in_gb",
    )
    .bind(ctx.repo.id)
    .bind(gb as i32)
    .execute(&s.db)
    .await?;
    crate::runner::storage::evict(&s, ctx.repo.id).await?;
    Ok(no_content())
}

async fn org_repo_ids(s: &AppState, org: &User) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1").bind(org.id).fetch_all(&s.db).await?)
}

async fn org_cache_usage(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_admin(&s, &u, &org).await?;
    let ids = org_repo_ids(&s, &o).await?;
    let (n, size) = usage_for(&s, Some(&ids)).await?;
    Ok(Json(json!({ "total_active_caches_count": n, "total_active_caches_size_in_bytes": size })))
}

#[derive(Deserialize, Default)]
struct PageQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn org_cache_usage_by_repo(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<PageQuery>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let total: i64 = sqlx::query_scalar("SELECT count(DISTINCT c.repo_id) FROM actions_caches c JOIN repositories r ON r.id = c.repo_id WHERE r.owner_id = $1")
        .bind(o.id)
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT r.name, count(*), sum(c.size_in_bytes)::bigint FROM actions_caches c JOIN repositories r ON r.id = c.repo_id
          WHERE r.owner_id = $1 GROUP BY r.id, r.name ORDER BY r.name LIMIT $2 OFFSET $3",
    )
    .bind(o.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(name, n, size)| json!({ "full_name": format!("{}/{name}", o.login), "active_caches_size_in_bytes": size, "active_caches_count": n }))
        .collect();
    let links = super::Paged::new(&uri, &s, p, Some(total), vec![]).into_response();
    let mut res = Json(total_envelope("repository_cache_usages", items, total)).into_response();
    if let Some(l) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, l.clone());
    }
    Ok(res)
}

async fn enterprise_cache_usage(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    crate::enterprise::check_enterprise(&e)?;
    site_admin(&u)?;
    let (n, size) = usage_for(&s, None).await?;
    Ok(Json(json!({ "total_active_caches_count": n, "total_active_caches_size_in_bytes": size })))
}

async fn enterprise_cache_policy(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    crate::enterprise::check_enterprise(&e)?;
    site_admin(&u)?;
    let row: Option<(Option<i32>, Option<i32>)> =
        sqlx::query_as("SELECT repo_cache_size_limit_in_gb, max_repo_cache_size_limit_in_gb FROM actions_cache_policies WHERE scope_kind = 'enterprise' AND scope_id = 0")
            .fetch_optional(&s.db)
            .await?;
    let (d, m) = row.unwrap_or((None, None));
    Ok(Json(json!({
        "repo_cache_size_limit_in_gb": d.unwrap_or(crate::runner::storage::DEFAULT_CACHE_LIMIT_GB),
        "max_repo_cache_size_limit_in_gb": m.unwrap_or(10000),
    })))
}

#[derive(Deserialize)]
struct EnterprisePolicyBody {
    repo_cache_size_limit_in_gb: Option<i64>,
    max_repo_cache_size_limit_in_gb: Option<i64>,
}

async fn set_enterprise_cache_policy(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>, Json(b): Json<EnterprisePolicyBody>) -> ApiResult<Response> {
    crate::enterprise::check_enterprise(&e)?;
    site_admin(&u)?;
    for v in [b.repo_cache_size_limit_in_gb, b.max_repo_cache_size_limit_in_gb].into_iter().flatten() {
        if !(0..=10000).contains(&v) {
            return Err(ApiError::unprocessable("Cache size limits must be between 0 and 10000 GB"));
        }
    }
    sqlx::query(
        "INSERT INTO actions_cache_policies (scope_kind, scope_id, repo_cache_size_limit_in_gb, max_repo_cache_size_limit_in_gb)
         VALUES ('enterprise', 0, $1, $2)
         ON CONFLICT (scope_kind, scope_id) DO UPDATE SET
           repo_cache_size_limit_in_gb = COALESCE(EXCLUDED.repo_cache_size_limit_in_gb, actions_cache_policies.repo_cache_size_limit_in_gb),
           max_repo_cache_size_limit_in_gb = COALESCE(EXCLUDED.max_repo_cache_size_limit_in_gb, actions_cache_policies.max_repo_cache_size_limit_in_gb)",
    )
    .bind(b.repo_cache_size_limit_in_gb.map(|v| v as i32))
    .bind(b.max_repo_cache_size_limit_in_gb.map(|v| v as i32))
    .execute(&s.db)
    .await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Approvals and deployments

async fn approvals(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let run = super::actions::run_by_id(&s, &ctx, &id).await?;
    let rows: Vec<(Value, String, Option<i64>, String)> =
        sqlx::query_as("SELECT environments, state, user_id, comment FROM actions_run_approvals WHERE run_id = $1 AND kind = 'review' ORDER BY id").bind(run.id).fetch_all(&s.db).await?;
    let mut out = Vec::new();
    for (envs, state, user_id, comment) in rows {
        let Some(uid) = user_id else { continue };
        let Ok(user) = db::user_by_id(&s.db, uid).await else { continue };
        out.push(json!({ "environments": envs, "state": state, "user": crate::json::user::simple(&s.api_url(), s.base_url(), &user), "comment": comment }));
    }
    Ok(Json(Value::Array(out)))
}

/// Environments the run's jobs wait on for review (required reviewers).
async fn pending_deployments(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let run = super::actions::run_by_id(&s, &ctx, &id).await?;
    Ok(Json(crate::runner::environments::pending_json(&s, &ctx, &run, u.as_ref()).await?))
}

#[derive(Deserialize)]
struct ReviewBody {
    environment_ids: Option<Vec<i64>>,
    state: Option<String>,
    comment: Option<String>,
}

async fn review_pending_deployments(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, String)>, Json(b): Json<ReviewBody>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let run = super::actions::run_by_id(&s, &ctx, &id).await?;
    let ids = b.environment_ids.ok_or_else(|| ApiError::validation("PendingDeployment", "environment_ids", "missing_field"))?;
    let state = b.state.clone().unwrap_or_default();
    if !matches!(state.as_str(), "approved" | "rejected") {
        return Err(ApiError::validation("PendingDeployment", "state", "invalid"));
    }
    let comment = b.comment.unwrap_or_default();
    let deployments = crate::runner::environments::review(&s, &ctx, &run, &u, &ids, &state, &comment).await?;
    Ok(Json(Value::Array(deployments)).into_response())
}

#[derive(Deserialize)]
struct GateBody {
    environment_name: Option<String>,
    state: Option<String>,
    comment: Option<String>,
}

/// Review by a custom deployment protection rule (GitHub: the rule's app; here any writer).
async fn review_custom_gates(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, String)>, Json(b): Json<GateBody>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let run = super::actions::run_by_id(&s, &ctx, &id).await?;
    ctx.require(Perm::Write)?;
    let env = b.environment_name.filter(|e| !e.is_empty()).ok_or_else(|| ApiError::validation("DeploymentProtectionRule", "environment_name", "missing_field"))?;
    crate::runner::environments::review_gate(&s, &ctx, &run, &u, &env, b.state.as_deref(), &b.comment.unwrap_or_default()).await?;
    Ok(no_content())
}
