//! GitHub Actions permission policies (`.../actions/permissions*`) for repositories,
//! organizations and the enterprise, the repository OIDC `sub` customization, and the
//! enforcement functions the runner calls.
//!
//! Inheritance follows GitHub: a level can only be as permissive as an *explicit* setting above
//! it (enterprise → organization → repository). Unset levels inherit; the base defaults are
//! Actions enabled, all actions allowed, a read-only `GITHUB_TOKEN`, no PR approvals by
//! workflows, 90 days retention (400 at most), approval for first-time contributors, and fork
//! PR workflows on private repositories disabled.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{check_org_repos, check_orgs, enterprise_admin, org_admin, org_repos_json, orgs_json, repo_admin};
use crate::api::plat::total_envelope;
use crate::api::{no_content, PageParams};
use crate::auth::AuthUser;
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub const DEFAULT_RETENTION_DAYS: i32 = 90;
pub const MAX_RETENTION_DAYS: i32 = 400;

// ---------------------------------------------------------------------------------------------
// Storage

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct Policy {
    pub enabled: bool,
    pub enabled_scope: String,
    pub allowed_actions: String,
    pub sha_pinning_required: bool,
    pub github_owned_allowed: bool,
    pub verified_allowed: bool,
    pub patterns_allowed: Vec<String>,
    pub default_workflow_permissions: Option<String>,
    pub can_approve_pull_request_reviews: Option<bool>,
    pub retention_days: Option<i32>,
    pub approval_policy: String,
    pub fork_pr_run: bool,
    pub fork_pr_write_tokens: bool,
    pub fork_pr_secrets: bool,
    pub fork_pr_require_approval: bool,
    pub access_level: String,
    pub self_hosted_scope: String,
    pub self_hosted_disabled: bool,
    pub oidc_use_default: bool,
    pub oidc_claim_keys: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            enabled: true,
            enabled_scope: "all".into(),
            allowed_actions: "all".into(),
            sha_pinning_required: false,
            github_owned_allowed: true,
            verified_allowed: false,
            patterns_allowed: vec![],
            default_workflow_permissions: None,
            can_approve_pull_request_reviews: None,
            retention_days: None,
            approval_policy: "first_time_contributors".into(),
            fork_pr_run: false,
            fork_pr_write_tokens: false,
            fork_pr_secrets: false,
            fork_pr_require_approval: true,
            access_level: "none".into(),
            self_hosted_scope: "all".into(),
            self_hosted_disabled: false,
            oidc_use_default: true,
            oidc_claim_keys: vec![],
        }
    }
}

const COLS: &str = "enabled, enabled_scope, allowed_actions, sha_pinning_required, github_owned_allowed, verified_allowed,
    patterns_allowed, default_workflow_permissions, can_approve_pull_request_reviews, retention_days, approval_policy,
    fork_pr_run, fork_pr_write_tokens, fork_pr_secrets, fork_pr_require_approval, access_level, self_hosted_scope,
    self_hosted_disabled, oidc_use_default, oidc_claim_keys";

/// The stored policy of an owner, if any setting was ever made there.
pub async fn load_opt(s: &AppState, kind: &str, id: i64) -> ApiResult<Option<Policy>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM actions_policies WHERE owner_kind = $1 AND owner_id = $2")))
        .bind(kind)
        .bind(id)
        .fetch_optional(&s.db)
        .await?)
}

pub async fn load(s: &AppState, kind: &str, id: i64) -> ApiResult<Policy> {
    Ok(load_opt(s, kind, id).await?.unwrap_or_default())
}

pub async fn save(s: &AppState, kind: &str, id: i64, p: &Policy) -> ApiResult<()> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO actions_policies (owner_kind, owner_id, {COLS})
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)
         ON CONFLICT (owner_kind, owner_id) DO UPDATE SET
            enabled = EXCLUDED.enabled, enabled_scope = EXCLUDED.enabled_scope, allowed_actions = EXCLUDED.allowed_actions,
            sha_pinning_required = EXCLUDED.sha_pinning_required, github_owned_allowed = EXCLUDED.github_owned_allowed,
            verified_allowed = EXCLUDED.verified_allowed, patterns_allowed = EXCLUDED.patterns_allowed,
            default_workflow_permissions = EXCLUDED.default_workflow_permissions,
            can_approve_pull_request_reviews = EXCLUDED.can_approve_pull_request_reviews,
            retention_days = EXCLUDED.retention_days, approval_policy = EXCLUDED.approval_policy,
            fork_pr_run = EXCLUDED.fork_pr_run, fork_pr_write_tokens = EXCLUDED.fork_pr_write_tokens,
            fork_pr_secrets = EXCLUDED.fork_pr_secrets, fork_pr_require_approval = EXCLUDED.fork_pr_require_approval,
            access_level = EXCLUDED.access_level, self_hosted_scope = EXCLUDED.self_hosted_scope,
            self_hosted_disabled = EXCLUDED.self_hosted_disabled, oidc_use_default = EXCLUDED.oidc_use_default,
            oidc_claim_keys = EXCLUDED.oidc_claim_keys, updated_at = now()"
    )))
    .bind(kind)
    .bind(id)
    .bind(p.enabled)
    .bind(&p.enabled_scope)
    .bind(&p.allowed_actions)
    .bind(p.sha_pinning_required)
    .bind(p.github_owned_allowed)
    .bind(p.verified_allowed)
    .bind(&p.patterns_allowed)
    .bind(&p.default_workflow_permissions)
    .bind(p.can_approve_pull_request_reviews)
    .bind(p.retention_days)
    .bind(&p.approval_policy)
    .bind(p.fork_pr_run)
    .bind(p.fork_pr_write_tokens)
    .bind(p.fork_pr_secrets)
    .bind(p.fork_pr_require_approval)
    .bind(&p.access_level)
    .bind(&p.self_hosted_scope)
    .bind(p.self_hosted_disabled)
    .bind(p.oidc_use_default)
    .bind(&p.oidc_claim_keys)
    .execute(&s.db)
    .await?;
    Ok(())
}

async fn selected_ids(s: &AppState, kind: &str, id: i64, list: &str) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT target_id FROM actions_policy_selected WHERE owner_kind = $1 AND owner_id = $2 AND list = $3 ORDER BY target_id")
        .bind(kind)
        .bind(id)
        .bind(list)
        .fetch_all(&s.db)
        .await?)
}

async fn is_selected(s: &AppState, kind: &str, id: i64, list: &str, target: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM actions_policy_selected WHERE owner_kind = $1 AND owner_id = $2 AND list = $3 AND target_id = $4)",
    )
    .bind(kind)
    .bind(id)
    .bind(list)
    .bind(target)
    .fetch_one(&s.db)
    .await?)
}

pub async fn set_selected(s: &AppState, kind: &str, id: i64, list: &str, targets: &[i64]) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM actions_policy_selected WHERE owner_kind = $1 AND owner_id = $2 AND list = $3")
        .bind(kind)
        .bind(id)
        .bind(list)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO actions_policy_selected (owner_kind, owner_id, list, target_id) SELECT $1, $2, $3, unnest($4::bigint[]) ON CONFLICT DO NOTHING")
        .bind(kind)
        .bind(id)
        .bind(list)
        .bind(targets)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn toggle_selected(s: &AppState, kind: &str, id: i64, list: &str, target: i64, add: bool) -> ApiResult<()> {
    let q = if add {
        "INSERT INTO actions_policy_selected (owner_kind, owner_id, list, target_id) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING"
    } else {
        "DELETE FROM actions_policy_selected WHERE owner_kind = $1 AND owner_id = $2 AND list = $3 AND target_id = $4"
    };
    sqlx::query(q).bind(kind).bind(id).bind(list).bind(target).execute(&s.db).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Owners

/// The level a policy request is about.
pub enum Owner {
    Repo(Box<RepoCtx>),
    Org(User),
    Enterprise(String),
}

impl Owner {
    pub async fn repo(s: &AppState, u: &User, owner: &str, repo: &str) -> ApiResult<Owner> {
        Ok(Owner::Repo(Box::new(repo_admin(s, u, owner, repo).await?)))
    }
    pub async fn org(s: &AppState, u: &User, org: &str) -> ApiResult<Owner> {
        Ok(Owner::Org(org_admin(s, u, org).await?))
    }
    pub async fn enterprise(_s: &AppState, u: &User, slug: &str) -> ApiResult<Owner> {
        enterprise_admin(u, slug)?;
        Ok(Owner::Enterprise(slug.to_string()))
    }
    pub fn key(&self) -> (&'static str, i64) {
        match self {
            Owner::Repo(c) => ("repo", c.repo.id),
            Owner::Org(o) => ("org", o.id),
            Owner::Enterprise(_) => ("enterprise", 0),
        }
    }
    fn permissions_url(&self, s: &AppState) -> String {
        match self {
            Owner::Repo(c) => format!("{}/repos/{}/actions/permissions", s.api_url(), c.full_name()),
            Owner::Org(o) => format!("{}/orgs/{}/actions/permissions", s.api_url(), o.login),
            Owner::Enterprise(e) => format!("{}/enterprises/{e}/actions/permissions", s.api_url()),
        }
    }
    /// Explicit policies of the levels above, nearest first.
    async fn parents(&self, s: &AppState) -> ApiResult<Vec<Policy>> {
        let mut out = vec![];
        match self {
            Owner::Repo(c) => {
                if c.owner.kind == "Organization" {
                    out.extend(load_opt(s, "org", c.owner.id).await?);
                }
                out.extend(load_opt(s, "enterprise", 0).await?);
            }
            Owner::Org(_) => out.extend(load_opt(s, "enterprise", 0).await?),
            Owner::Enterprise(_) => {}
        }
        Ok(out)
    }
    async fn load(&self, s: &AppState) -> ApiResult<Policy> {
        let (k, id) = self.key();
        load(s, k, id).await
    }
    async fn save(&self, s: &AppState, p: &Policy) -> ApiResult<()> {
        let (k, id) = self.key();
        save(s, k, id, p).await
    }
}

/// The policy chain for a repository, top (enterprise) first: (kind, id, policy).
async fn chain(s: &AppState, repo_id: i64) -> ApiResult<(crate::db::Repo, User, Vec<(&'static str, i64, Policy)>)> {
    let repo = crate::db::repo_by_id(&s.db, repo_id).await?;
    let owner = crate::db::user_by_id(&s.db, repo.owner_id).await?;
    let mut levels = vec![("enterprise", 0, load(s, "enterprise", 0).await?)];
    if owner.kind == "Organization" {
        levels.push(("org", owner.id, load(s, "org", owner.id).await?));
    }
    levels.push(("repo", repo.id, load(s, "repo", repo.id).await?));
    Ok((repo, owner, levels))
}

fn invalid(resource: &str, field: &str) -> ApiError {
    ApiError::validation(resource, field, "invalid")
}

fn missing(resource: &str, field: &str) -> ApiError {
    ApiError::validation(resource, field, "missing_field")
}

fn one_of(v: &str, allowed: &[&str], field: &str) -> ApiResult<String> {
    if allowed.contains(&v) { Ok(v.to_string()) } else { Err(invalid("ActionsPermissions", field)) }
}

// ---------------------------------------------------------------------------------------------
// Sections shared by every level

#[derive(Deserialize, Default)]
pub struct SelectedActionsInput {
    pub github_owned_allowed: Option<bool>,
    pub verified_allowed: Option<bool>,
    pub patterns_allowed: Option<Vec<String>>,
}

async fn selected_actions_get(s: &AppState, o: &Owner) -> ApiResult<Response> {
    let p = o.load(s).await?;
    Ok(Json(json!({
        "github_owned_allowed": p.github_owned_allowed,
        "verified_allowed": p.verified_allowed,
        "patterns_allowed": p.patterns_allowed,
    }))
    .into_response())
}

async fn selected_actions_put(s: &AppState, o: &Owner, b: SelectedActionsInput) -> ApiResult<Response> {
    let mut p = o.load(s).await?;
    if let Some(v) = b.github_owned_allowed {
        p.github_owned_allowed = v;
    }
    if let Some(v) = b.verified_allowed {
        p.verified_allowed = v;
    }
    if let Some(v) = b.patterns_allowed {
        if v.iter().any(|x| x.trim().is_empty()) {
            return Err(invalid("ActionsPermissions", "patterns_allowed"));
        }
        p.patterns_allowed = v.into_iter().map(|x| x.trim().to_string()).collect();
    }
    o.save(s, &p).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
pub struct WorkflowInput {
    pub default_workflow_permissions: Option<String>,
    pub can_approve_pull_request_reviews: Option<bool>,
}

async fn workflow_get(s: &AppState, o: &Owner) -> ApiResult<Response> {
    let mine = o.load(s).await?;
    let parents = o.parents(s).await?;
    let perms = mine
        .default_workflow_permissions
        .clone()
        .or_else(|| parents.iter().find_map(|p| p.default_workflow_permissions.clone()))
        .unwrap_or_else(|| "read".into());
    let approve = mine
        .can_approve_pull_request_reviews
        .or_else(|| parents.iter().find_map(|p| p.can_approve_pull_request_reviews))
        .unwrap_or(false);
    Ok(Json(json!({ "default_workflow_permissions": perms, "can_approve_pull_request_reviews": approve })).into_response())
}

async fn workflow_put(s: &AppState, o: &Owner, b: WorkflowInput) -> ApiResult<Response> {
    let mut p = o.load(s).await?;
    let parents = o.parents(s).await?;
    if let Some(v) = b.default_workflow_permissions {
        let v = one_of(&v, &["read", "write"], "default_workflow_permissions")?;
        if v == "write" && parents.iter().any(|p| p.default_workflow_permissions.as_deref() == Some("read")) {
            return Err(ApiError::conflict("Write permissions for the GITHUB_TOKEN are restricted by a higher-level policy."));
        }
        p.default_workflow_permissions = Some(v);
    }
    if let Some(v) = b.can_approve_pull_request_reviews {
        if v && parents.iter().any(|p| p.can_approve_pull_request_reviews == Some(false)) {
            return Err(ApiError::conflict("Approving pull request reviews is restricted by a higher-level policy."));
        }
        p.can_approve_pull_request_reviews = Some(v);
    }
    o.save(s, &p).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
pub struct RetentionInput {
    pub days: Option<i64>,
}

/// (days, maximum_allowed_days) at this level.
async fn retention(s: &AppState, o: &Owner) -> ApiResult<(i32, i32)> {
    let mine = o.load(s).await?;
    let parents = o.parents(s).await?;
    let max = parents.iter().find_map(|p| p.retention_days).unwrap_or(MAX_RETENTION_DAYS);
    let days = mine.retention_days.unwrap_or(DEFAULT_RETENTION_DAYS.min(max)).min(max);
    Ok((days, max))
}

async fn retention_get(s: &AppState, o: &Owner) -> ApiResult<Response> {
    let (days, max) = retention(s, o).await?;
    Ok(Json(json!({ "days": days, "maximum_allowed_days": max })).into_response())
}

async fn retention_put(s: &AppState, o: &Owner, b: RetentionInput) -> ApiResult<Response> {
    let days = b.days.ok_or_else(|| missing("RetentionPolicy", "days"))?;
    let (_, max) = retention(s, o).await?;
    if days < 1 || days > max as i64 {
        let mut e = invalid("RetentionPolicy", "days");
        e.errors = Some(json!([{ "resource": "RetentionPolicy", "field": "days", "code": "invalid",
            "message": format!("days must be between 1 and {max}") }]));
        return Err(e);
    }
    let mut p = o.load(s).await?;
    p.retention_days = Some(days as i32);
    o.save(s, &p).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
pub struct ApprovalInput {
    pub approval_policy: Option<String>,
}

const APPROVAL_POLICIES: &[&str] = &["first_time_contributors_new_to_github", "first_time_contributors", "all_external_contributors"];

async fn approval_get(s: &AppState, o: &Owner) -> ApiResult<Response> {
    Ok(Json(json!({ "approval_policy": o.load(s).await?.approval_policy })).into_response())
}

async fn approval_put(s: &AppState, o: &Owner, b: ApprovalInput) -> ApiResult<Response> {
    let v = b.approval_policy.ok_or_else(|| missing("ActionsPermissions", "approval_policy"))?;
    let mut p = o.load(s).await?;
    p.approval_policy = one_of(&v, APPROVAL_POLICIES, "approval_policy")?;
    o.save(s, &p).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
pub struct ForkPrInput {
    pub run_workflows_from_fork_pull_requests: Option<bool>,
    pub send_write_tokens_to_workflows: Option<bool>,
    pub send_secrets_and_variables: Option<bool>,
    pub require_approval_for_fork_pr_workflows: Option<bool>,
}

async fn fork_pr_get(s: &AppState, o: &Owner) -> ApiResult<Response> {
    let p = o.load(s).await?;
    Ok(Json(json!({
        "run_workflows_from_fork_pull_requests": p.fork_pr_run,
        "send_write_tokens_to_workflows": p.fork_pr_write_tokens,
        "send_secrets_and_variables": p.fork_pr_secrets,
        "require_approval_for_fork_pr_workflows": p.fork_pr_require_approval,
    }))
    .into_response())
}

async fn fork_pr_put(s: &AppState, o: &Owner, b: ForkPrInput) -> ApiResult<Response> {
    let run = b.run_workflows_from_fork_pull_requests.ok_or_else(|| missing("ActionsPermissions", "run_workflows_from_fork_pull_requests"))?;
    let parents = o.parents(s).await?;
    // A level cannot enable what an explicit higher-level setting disables.
    let blocked = |f: fn(&Policy) -> bool| parents.iter().any(|p| !f(p));
    let want = |v: Option<bool>| v.unwrap_or(false);
    if (run && blocked(|p| p.fork_pr_run))
        || (want(b.send_write_tokens_to_workflows) && blocked(|p| p.fork_pr_write_tokens))
        || (want(b.send_secrets_and_variables) && blocked(|p| p.fork_pr_secrets))
    {
        return Err(ApiError::unprocessable("This setting is disabled by a higher-level policy."));
    }
    let mut p = o.load(s).await?;
    p.fork_pr_run = run;
    if let Some(v) = b.send_write_tokens_to_workflows {
        p.fork_pr_write_tokens = v;
    }
    if let Some(v) = b.send_secrets_and_variables {
        p.fork_pr_secrets = v;
    }
    if let Some(v) = b.require_approval_for_fork_pr_workflows {
        p.fork_pr_require_approval = v;
    }
    o.save(s, &p).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Level-specific sections

#[derive(Deserialize, Default)]
pub struct PermissionsInput {
    pub enabled: Option<bool>,
    pub enabled_repositories: Option<String>,
    pub enabled_organizations: Option<String>,
    pub allowed_actions: Option<String>,
    pub sha_pinning_required: Option<bool>,
}

async fn permissions_get(s: &AppState, o: &Owner) -> ApiResult<Response> {
    let p = o.load(s).await?;
    let url = o.permissions_url(s);
    let mut v = match o {
        Owner::Repo(_) => json!({ "enabled": p.enabled }),
        Owner::Org(_) => json!({ "enabled_repositories": p.enabled_scope }),
        Owner::Enterprise(_) => json!({ "enabled_organizations": p.enabled_scope }),
    };
    if p.enabled_scope == "selected" {
        match o {
            Owner::Org(_) => v["selected_repositories_url"] = json!(format!("{url}/repositories")),
            Owner::Enterprise(_) => v["selected_organizations_url"] = json!(format!("{url}/organizations")),
            Owner::Repo(_) => {}
        }
    }
    v["allowed_actions"] = json!(p.allowed_actions);
    if p.allowed_actions == "selected" {
        v["selected_actions_url"] = json!(format!("{url}/selected-actions"));
    }
    v["sha_pinning_required"] = json!(p.sha_pinning_required);
    Ok(Json(v).into_response())
}

async fn permissions_put(s: &AppState, o: &Owner, b: PermissionsInput) -> ApiResult<Response> {
    let mut p = o.load(s).await?;
    match o {
        Owner::Repo(_) => p.enabled = b.enabled.ok_or_else(|| missing("ActionsPermissions", "enabled"))?,
        Owner::Org(_) => {
            let v = b.enabled_repositories.ok_or_else(|| missing("ActionsPermissions", "enabled_repositories"))?;
            p.enabled_scope = one_of(&v, &["all", "none", "selected"], "enabled_repositories")?;
        }
        Owner::Enterprise(_) => {
            let v = b.enabled_organizations.ok_or_else(|| missing("ActionsPermissions", "enabled_organizations"))?;
            p.enabled_scope = one_of(&v, &["all", "none", "selected"], "enabled_organizations")?;
        }
    }
    if let Some(v) = b.allowed_actions {
        p.allowed_actions = one_of(&v, &["all", "local_only", "selected"], "allowed_actions")?;
    }
    if let Some(v) = b.sha_pinning_required {
        p.sha_pinning_required = v;
    }
    o.save(s, &p).await?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
pub struct AccessInput {
    pub access_level: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct OidcInput {
    pub use_default: Option<bool>,
    pub include_claim_keys: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
pub struct SelectedReposInput {
    pub selected_repository_ids: Option<Vec<i64>>,
}

#[derive(Deserialize, Default)]
pub struct SelectedOrgsInput {
    pub selected_organization_ids: Option<Vec<i64>>,
}

#[derive(Deserialize, Default)]
pub struct SelfHostedInput {
    pub enabled_repositories: Option<String>,
    pub disable_self_hosted_runners_for_all_orgs: Option<bool>,
}

// ---------------------------------------------------------------------------------------------
// Handlers

type RepoP = Path<(String, String)>;

macro_rules! level_handlers {
    ($get:ident, $put:ident, $body:ty, $rg:ident, $rp:ident, $og:ident, $op:ident, $eg:ident, $ep:ident) => {
        async fn $rg(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP) -> ApiResult<Response> {
            $get(&s, &Owner::repo(&s, &u, &o, &r).await?).await
        }
        async fn $rp(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP, Json(b): Json<$body>) -> ApiResult<Response> {
            $put(&s, &Owner::repo(&s, &u, &o, &r).await?, b).await
        }
        async fn $og(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Response> {
            $get(&s, &Owner::org(&s, &u, &org).await?).await
        }
        async fn $op(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<$body>) -> ApiResult<Response> {
            $put(&s, &Owner::org(&s, &u, &org).await?, b).await
        }
        async fn $eg(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>) -> ApiResult<Response> {
            $get(&s, &Owner::enterprise(&s, &u, &e).await?).await
        }
        async fn $ep(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>, Json(b): Json<$body>) -> ApiResult<Response> {
            $put(&s, &Owner::enterprise(&s, &u, &e).await?, b).await
        }
    };
}

level_handlers!(permissions_get, permissions_put, PermissionsInput, r_perm_get, r_perm_put, o_perm_get, o_perm_put, e_perm_get, e_perm_put);
level_handlers!(selected_actions_get, selected_actions_put, SelectedActionsInput, r_sa_get, r_sa_put, o_sa_get, o_sa_put, e_sa_get, e_sa_put);
level_handlers!(workflow_get, workflow_put, WorkflowInput, r_wf_get, r_wf_put, o_wf_get, o_wf_put, e_wf_get, e_wf_put);
level_handlers!(retention_get, retention_put, RetentionInput, r_ret_get, r_ret_put, o_ret_get, o_ret_put, e_ret_get, e_ret_put);
level_handlers!(approval_get, approval_put, ApprovalInput, r_ap_get, r_ap_put, o_ap_get, o_ap_put, e_ap_get, e_ap_put);
level_handlers!(fork_pr_get, fork_pr_put, ForkPrInput, r_fp_get, r_fp_put, o_fp_get, o_fp_put, e_fp_get, e_fp_put);

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/actions/permissions";
    let o = "/orgs/{org}/actions/permissions";
    Router::new()
        .route(r, get(r_perm_get).put(r_perm_put))
        .route(&format!("{r}/selected-actions"), get(r_sa_get).put(r_sa_put))
        .route(&format!("{r}/workflow"), get(r_wf_get).put(r_wf_put))
        .route(&format!("{r}/artifact-and-log-retention"), get(r_ret_get).put(r_ret_put))
        .route(&format!("{r}/fork-pr-contributor-approval"), get(r_ap_get).put(r_ap_put))
        .route(&format!("{r}/fork-pr-workflows-private-repos"), get(r_fp_get).put(r_fp_put))
        .route(&format!("{r}/access"), get(r_access_get).put(r_access_put))
        .route("/repos/{owner}/{repo}/actions/oidc/customization/sub", get(r_oidc_get).put(r_oidc_put))
        .route(o, get(o_perm_get).put(o_perm_put))
        .route(&format!("{o}/selected-actions"), get(o_sa_get).put(o_sa_put))
        .route(&format!("{o}/workflow"), get(o_wf_get).put(o_wf_put))
        .route(&format!("{o}/artifact-and-log-retention"), get(o_ret_get).put(o_ret_put))
        .route(&format!("{o}/fork-pr-contributor-approval"), get(o_ap_get).put(o_ap_put))
        .route(&format!("{o}/fork-pr-workflows-private-repos"), get(o_fp_get).put(o_fp_put))
        .route(&format!("{o}/repositories"), get(o_enabled_repos).put(o_set_enabled_repos))
        .route(&format!("{o}/repositories/{{repository_id}}"), put(o_enable_repo).delete(o_disable_repo))
        .route(&format!("{o}/self-hosted-runners"), get(o_sh_get).put(o_sh_put))
        .route(&format!("{o}/self-hosted-runners/repositories"), get(o_sh_repos).put(o_sh_set_repos))
        .route(&format!("{o}/self-hosted-runners/repositories/{{repository_id}}"), put(o_sh_add_repo).delete(o_sh_remove_repo))
}

pub fn enterprise_routes() -> Router<AppState> {
    let e = "/enterprises/{enterprise}/actions/permissions";
    Router::new()
        .route(e, get(e_perm_get).put(e_perm_put))
        .route(&format!("{e}/selected-actions"), get(e_sa_get).put(e_sa_put))
        .route(&format!("{e}/workflow"), get(e_wf_get).put(e_wf_put))
        .route(&format!("{e}/artifact-and-log-retention"), get(e_ret_get).put(e_ret_put))
        .route(&format!("{e}/fork-pr-contributor-approval"), get(e_ap_get).put(e_ap_put))
        .route(&format!("{e}/fork-pr-workflows-private-repos"), get(e_fp_get).put(e_fp_put))
        .route(&format!("{e}/organizations"), get(e_enabled_orgs).put(e_set_enabled_orgs))
        .route(&format!("{e}/organizations/{{org_id}}"), put(e_enable_org).delete(e_disable_org))
        .route(&format!("{e}/self-hosted-runners"), get(e_sh_get).put(e_sh_put))
}

// Repository-only: workflow access and OIDC.

async fn r_access_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP) -> ApiResult<Response> {
    let ow = Owner::repo(&s, &u, &o, &r).await?;
    Ok(Json(json!({ "access_level": ow.load(&s).await?.access_level })).into_response())
}
async fn r_access_put(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP, Json(b): Json<AccessInput>) -> ApiResult<Response> {
    let ow = Owner::repo(&s, &u, &o, &r).await?;
    let v = b.access_level.ok_or_else(|| missing("ActionsPermissions", "access_level"))?;
    let v = one_of(&v, &["none", "user", "organization", "enterprise"], "access_level")?;
    if let Owner::Repo(c) = &ow {
        let is_org = c.owner.kind == "Organization";
        if (v == "user" && is_org) || (v == "organization" && !is_org) {
            return Err(invalid("ActionsPermissions", "access_level"));
        }
    }
    let mut p = ow.load(&s).await?;
    p.access_level = v;
    ow.save(&s, &p).await?;
    Ok(no_content())
}

async fn r_oidc_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP) -> ApiResult<Response> {
    let p = Owner::repo(&s, &u, &o, &r).await?.load(&s).await?;
    let mut v = json!({ "use_default": p.oidc_use_default });
    if !p.oidc_use_default {
        v["include_claim_keys"] = json!(p.oidc_claim_keys);
    }
    Ok(Json(v).into_response())
}
async fn r_oidc_put(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RepoP, Json(b): Json<OidcInput>) -> ApiResult<Response> {
    let ow = Owner::repo(&s, &u, &o, &r).await?;
    let use_default = b.use_default.ok_or_else(|| missing("OidcCustomSub", "use_default"))?;
    let mut p = ow.load(&s).await?;
    p.oidc_use_default = use_default;
    if !use_default {
        let keys = b.include_claim_keys.unwrap_or_default();
        if keys.is_empty() || keys.iter().any(|k| k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            return Err(invalid("OidcCustomSub", "include_claim_keys"));
        }
        p.oidc_claim_keys = keys;
    } else if let Some(keys) = b.include_claim_keys {
        p.oidc_claim_keys = keys;
    }
    ow.save(&s, &p).await?;
    Ok((StatusCode::CREATED, Json(json!({}))).into_response())
}

// Organization-only: repositories enabled for Actions, self-hosted runner access.

fn org_of(o: &Owner) -> &User {
    match o {
        Owner::Org(u) => u,
        _ => unreachable!(),
    }
}

async fn repos_page(s: &AppState, org: &User, ids: &[i64], p: PageParams, uri: &OriginalUri) -> ApiResult<Response> {
    let all = org_repos_json(s, org, ids).await?;
    let total = all.len() as i64;
    let items: Vec<Value> = all.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    let links = crate::api::Paged::new(uri, s, p, Some(total), vec![]).into_response();
    let mut res = Json(total_envelope("repositories", items, total)).into_response();
    if let Some(l) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, l.clone());
    }
    Ok(res)
}

async fn o_enabled_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let ids = selected_ids(&s, "org", o.id, "enabled").await?;
    repos_page(&s, &o, &ids, p, &uri).await
}
async fn o_set_enabled_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<SelectedReposInput>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let ids = b.selected_repository_ids.ok_or_else(|| missing("ActionsPermissions", "selected_repository_ids"))?;
    let ids = check_org_repos(&s, &o, &ids, "selected_repository_ids").await?;
    set_selected(&s, "org", o.id, "enabled", &ids).await?;
    Ok(no_content())
}
async fn org_repo_toggle(s: &AppState, u: &User, org: &str, list: &str, repo_id: i64, add: bool, need_selected: bool) -> ApiResult<Response> {
    let ow = Owner::org(s, u, org).await?;
    let o = org_of(&ow);
    let p = ow.load(s).await?;
    let scope = if list == "enabled" { &p.enabled_scope } else { &p.self_hosted_scope };
    if need_selected && scope != "selected" {
        return Err(ApiError::conflict("The repository access policy must be set to 'selected' to manage selected repositories."));
    }
    check_org_repos(s, o, &[repo_id], "repository_id").await.map_err(|_| ApiError::not_found())?;
    toggle_selected(s, "org", o.id, list, repo_id, add).await?;
    Ok(no_content())
}
async fn o_enable_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    org_repo_toggle(&s, &u, &org, "enabled", id, true, false).await
}
async fn o_disable_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    org_repo_toggle(&s, &u, &org, "enabled", id, false, false).await
}

async fn o_sh_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Response> {
    let ow = Owner::org(&s, &u, &org).await?;
    let p = ow.load(&s).await?;
    let mut v = json!({ "enabled_repositories": p.self_hosted_scope });
    if p.self_hosted_scope == "selected" {
        v["selected_repositories_url"] = json!(format!("{}/self-hosted-runners/repositories", ow.permissions_url(&s)));
    }
    Ok(Json(v).into_response())
}
async fn o_sh_put(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<SelfHostedInput>) -> ApiResult<Response> {
    let ow = Owner::org(&s, &u, &org).await?;
    let v = b.enabled_repositories.ok_or_else(|| missing("ActionsPermissions", "enabled_repositories"))?;
    let v = one_of(&v, &["all", "selected", "none"], "enabled_repositories")?;
    if v != "none" && ow.parents(&s).await?.iter().any(|p| p.self_hosted_disabled) {
        return Err(ApiError::conflict("Self-hosted runners are disabled for all organizations by the enterprise."));
    }
    let mut p = ow.load(&s).await?;
    p.self_hosted_scope = v;
    ow.save(&s, &p).await?;
    Ok(no_content())
}
async fn o_sh_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let ids = selected_ids(&s, "org", o.id, "self_hosted").await?;
    repos_page(&s, &o, &ids, p, &uri).await
}
async fn o_sh_set_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<SelectedReposInput>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let ids = b.selected_repository_ids.ok_or_else(|| missing("ActionsPermissions", "selected_repository_ids"))?;
    let ids = check_org_repos(&s, &o, &ids, "selected_repository_ids").await?;
    set_selected(&s, "org", o.id, "self_hosted", &ids).await?;
    Ok(no_content())
}
async fn o_sh_add_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    org_repo_toggle(&s, &u, &org, "self_hosted", id, true, true).await
}
async fn o_sh_remove_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    org_repo_toggle(&s, &u, &org, "self_hosted", id, false, true).await
}

// Enterprise-only: organizations enabled for Actions, self-hosted runners switch.

async fn e_enabled_orgs(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(e): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    enterprise_admin(&u, &e)?;
    let ids = selected_ids(&s, "enterprise", 0, "enabled").await?;
    let all = orgs_json(&s, &ids).await?;
    let total = all.len() as i64;
    let items: Vec<Value> = all.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    let links = crate::api::Paged::new(&uri, &s, p, Some(total), vec![]).into_response();
    let mut res = Json(total_envelope("organizations", items, total)).into_response();
    if let Some(l) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, l.clone());
    }
    Ok(res)
}
async fn e_set_enabled_orgs(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>, Json(b): Json<SelectedOrgsInput>) -> ApiResult<Response> {
    enterprise_admin(&u, &e)?;
    let ids = b.selected_organization_ids.ok_or_else(|| missing("ActionsPermissions", "selected_organization_ids"))?;
    let ids = check_orgs(&s, &ids, "selected_organization_ids").await?;
    set_selected(&s, "enterprise", 0, "enabled", &ids).await?;
    Ok(no_content())
}
async fn e_org_toggle(s: &AppState, u: &User, e: &str, org_id: i64, add: bool) -> ApiResult<Response> {
    enterprise_admin(u, e)?;
    check_orgs(s, &[org_id], "org_id").await.map_err(|_| ApiError::not_found())?;
    toggle_selected(s, "enterprise", 0, "enabled", org_id, add).await?;
    Ok(no_content())
}
async fn e_enable_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, id)): Path<(String, i64)>) -> ApiResult<Response> {
    e_org_toggle(&s, &u, &e, id, true).await
}
async fn e_disable_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, id)): Path<(String, i64)>) -> ApiResult<Response> {
    e_org_toggle(&s, &u, &e, id, false).await
}
async fn e_sh_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>) -> ApiResult<Response> {
    enterprise_admin(&u, &e)?;
    Ok(Json(json!({ "disable_self_hosted_runners_for_all_orgs": load(&s, "enterprise", 0).await?.self_hosted_disabled })).into_response())
}
async fn e_sh_put(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>, Json(b): Json<SelfHostedInput>) -> ApiResult<Response> {
    enterprise_admin(&u, &e)?;
    let v = b.disable_self_hosted_runners_for_all_orgs.ok_or_else(|| missing("ActionsPermissions", "disable_self_hosted_runners_for_all_orgs"))?;
    let mut p = load(&s, "enterprise", 0).await?;
    p.self_hosted_disabled = v;
    save(&s, "enterprise", 0, &p).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Enforcement (called by the runner)

/// Whether workflows of `repo_id` may run at all: the repository has Actions enabled, its org
/// enables it (`all`, or `selected` with the repository selected) and the enterprise enables
/// the org. Errors count as "not allowed".
pub async fn actions_allowed(s: &AppState, repo_id: i64) -> bool {
    match actions_allowed_inner(s, repo_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(repo_id, error = %e.message, "actions policy lookup failed");
            false
        }
    }
}

async fn actions_allowed_inner(s: &AppState, repo_id: i64) -> ApiResult<bool> {
    let (repo, owner, levels) = chain(s, repo_id).await?;
    for (kind, id, p) in &levels {
        let ok = match *kind {
            "repo" => p.enabled,
            "org" => match p.enabled_scope.as_str() {
                "none" => false,
                "selected" => is_selected(s, kind, *id, "enabled", repo.id).await?,
                _ => true,
            },
            _ => match p.enabled_scope.as_str() {
                "none" => false,
                "selected" if owner.kind == "Organization" => is_selected(s, kind, *id, "enabled", owner.id).await?,
                _ => true,
            },
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The default `GITHUB_TOKEN` permissions for jobs of a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTokenPermissions {
    /// `write`: read/write on all scopes; otherwise `contents: read` and `packages: read` only.
    pub write: bool,
    /// Whether the token may create or approve pull request reviews.
    pub can_approve_pull_request_reviews: bool,
}

pub async fn workflow_token_permissions(s: &AppState, repo_id: i64) -> ApiResult<WorkflowTokenPermissions> {
    let (_, _, levels) = chain(s, repo_id).await?;
    // Nearest explicit setting wins, but an explicit restrictive parent caps it.
    let perms = levels.iter().rev().find_map(|(_, _, p)| p.default_workflow_permissions.clone()).unwrap_or_else(|| "read".into());
    let capped = levels.iter().any(|(_, _, p)| p.default_workflow_permissions.as_deref() == Some("read"));
    let approve = levels.iter().rev().find_map(|(_, _, p)| p.can_approve_pull_request_reviews).unwrap_or(false);
    let approve_capped = levels.iter().any(|(_, _, p)| p.can_approve_pull_request_reviews == Some(false));
    Ok(WorkflowTokenPermissions { write: perms == "write" && !capped, can_approve_pull_request_reviews: approve && !approve_capped })
}

/// Whether a workflow in `repo_id` may use the action/reusable workflow `uses` (as written in
/// the workflow: `./path`, `owner/repo@ref`, `owner/repo/path@ref`, `docker://image`). Every
/// level must allow it.
pub async fn action_allowed(s: &AppState, repo_id: i64, uses: &str) -> ApiResult<bool> {
    let (_, owner, levels) = chain(s, repo_id).await?;
    let uses = uses.trim();
    let local = uses.starts_with("./");
    let (path, git_ref) = uses.split_once('@').unwrap_or((uses, ""));
    let action_owner = path.split('/').next().unwrap_or("");
    let same_owner = !uses.starts_with("docker://") && action_owner.eq_ignore_ascii_case(&owner.login);
    let github_owned = ["actions", "github"].iter().any(|o| action_owner.eq_ignore_ascii_case(o));
    let pinned = git_ref.len() == 40 && git_ref.chars().all(|c| c.is_ascii_hexdigit());
    for (_, _, p) in &levels {
        if local {
            continue;
        }
        if p.sha_pinning_required && !uses.starts_with("docker://") && !pinned {
            return Ok(false);
        }
        let ok = match p.allowed_actions.as_str() {
            "local_only" => same_owner,
            "selected" => same_owner || (p.github_owned_allowed && github_owned) || p.patterns_allowed.iter().any(|pat| pattern_matches(pat, uses)),
            _ => true,
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

/// GitHub's allow-list patterns: `*` matches any run of characters; a pattern without `@`
/// matches every ref.
pub fn pattern_matches(pattern: &str, uses: &str) -> bool {
    let pattern = pattern.trim();
    let target = if pattern.contains('@') { uses.to_string() } else { uses.split('@').next().unwrap_or(uses).to_string() };
    glob(&pattern.to_ascii_lowercase(), &target.to_ascii_lowercase())
}

fn glob(p: &str, t: &str) -> bool {
    match p.split_once('*') {
        None => p == t,
        Some((head, rest)) => {
            let Some(t) = t.strip_prefix(head) else { return false };
            (0..=t.len()).filter(|i| t.is_char_boundary(*i)).any(|i| glob(rest, &t[i..]))
        }
    }
}

/// Effective policy for pull requests from forks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkPrPolicy {
    /// Which outside contributors need approval before their fork PR workflows run.
    pub approval_policy: String,
    /// Private repositories: run workflows from fork pull requests at all.
    pub run_workflows_from_fork_pull_requests: bool,
    pub send_write_tokens_to_workflows: bool,
    pub send_secrets_and_variables: bool,
    pub require_approval_for_fork_pr_workflows: bool,
}

pub async fn fork_pr_policy(s: &AppState, repo_id: i64) -> ApiResult<ForkPrPolicy> {
    let (repo, _, levels) = chain(s, repo_id).await?;
    let all = |f: fn(&Policy) -> bool| levels.iter().all(|(_, _, p)| f(p));
    let repo_p = &levels.last().expect("repo level").2;
    if !repo.private {
        // Public repositories always run fork PR workflows, with a read-only token and no secrets.
        return Ok(ForkPrPolicy {
            approval_policy: repo_p.approval_policy.clone(),
            run_workflows_from_fork_pull_requests: true,
            send_write_tokens_to_workflows: false,
            send_secrets_and_variables: false,
            require_approval_for_fork_pr_workflows: repo_p.approval_policy != "first_time_contributors_new_to_github",
        });
    }
    Ok(ForkPrPolicy {
        approval_policy: repo_p.approval_policy.clone(),
        run_workflows_from_fork_pull_requests: all(|p| p.fork_pr_run),
        send_write_tokens_to_workflows: all(|p| p.fork_pr_write_tokens),
        send_secrets_and_variables: all(|p| p.fork_pr_secrets),
        require_approval_for_fork_pr_workflows: levels.iter().any(|(_, _, p)| p.fork_pr_require_approval),
    })
}

/// Artifact and log retention in days for a repository.
pub async fn retention_days(s: &AppState, repo_id: i64) -> ApiResult<i32> {
    let (_, _, levels) = chain(s, repo_id).await?;
    let mut max = MAX_RETENTION_DAYS;
    let mut days = DEFAULT_RETENTION_DAYS;
    for (_, _, p) in &levels {
        if let Some(d) = p.retention_days {
            days = d;
        }
        days = days.min(max);
        max = days;
    }
    Ok(days)
}

/// Whether jobs of the repository may run on self-hosted runners (org policy, enterprise switch).
pub async fn self_hosted_runners_allowed(s: &AppState, repo_id: i64) -> ApiResult<bool> {
    let (repo, owner, levels) = chain(s, repo_id).await?;
    if owner.kind != "Organization" {
        return Ok(true);
    }
    for (kind, id, p) in &levels {
        match *kind {
            "enterprise" if p.self_hosted_disabled => return Ok(false),
            "org" => match p.self_hosted_scope.as_str() {
                "none" => return Ok(false),
                "selected" if !is_selected(s, kind, *id, "self_hosted", repo.id).await? => return Ok(false),
                _ => {}
            },
            _ => {}
        }
    }
    Ok(true)
}

/// Whether a workflow in `caller_repo_id` may use actions / reusable workflows stored in
/// `workflow_repo_id`: always for the same or a public repository, otherwise per the target's
/// access level (`user`: same user owner, `organization`: same org, `enterprise`: any).
pub async fn workflow_access_allowed(s: &AppState, workflow_repo_id: i64, caller_repo_id: i64) -> ApiResult<bool> {
    if workflow_repo_id == caller_repo_id {
        return Ok(true);
    }
    let target = crate::db::repo_by_id(&s.db, workflow_repo_id).await?;
    if !target.private && target.visibility == "public" {
        return Ok(true);
    }
    let caller = crate::db::repo_by_id(&s.db, caller_repo_id).await?;
    Ok(match load(s, "repo", target.id).await?.access_level.as_str() {
        "user" | "organization" => caller.owner_id == target.owner_id,
        "enterprise" => true,
        _ => false,
    })
}

/// Custom OIDC `sub` claim keys for a repository, or None for the default template.
pub async fn oidc_sub_claim_keys(s: &AppState, repo_id: i64) -> ApiResult<Option<Vec<String>>> {
    let p = load(s, "repo", repo_id).await?;
    Ok((!p.oidc_use_default && !p.oidc_claim_keys.is_empty()).then_some(p.oidc_claim_keys))
}

#[cfg(test)]
mod tests {
    use super::pattern_matches;

    #[test]
    fn patterns() {
        assert!(pattern_matches("octo/*", "octo/setup@v1"));
        assert!(pattern_matches("octo/setup@*", "octo/setup@v2"));
        assert!(!pattern_matches("octo/setup@v1", "octo/setup@v2"));
        assert!(pattern_matches("octo/setup", "Octo/Setup@main"));
        assert!(!pattern_matches("octo/*", "other/x@v1"));
    }
}
