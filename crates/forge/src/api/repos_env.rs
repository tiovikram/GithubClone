//! Deployment environments: `/repos/{o}/{r}/environments[/{name}]`, deployment branch policies
//! and deployment protection rules (wait timer, required reviewers, branch policy, custom
//! GitHub App gates).
//!
//! Environments are created explicitly (`PUT`) or implicitly by the first deployment to a new
//! environment name (`ensure_environment`, called by `api::deployments`). Reading requires read
//! access; changing requires admin, like GitHub. Deleting an environment also deletes its
//! Actions secrets and variables.
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::json::release::seg;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let e = "/repos/{owner}/{repo}/environments/{env}";
    Router::new()
        .route("/repos/{owner}/{repo}/environments", get(list))
        .route(e, get(get_one).put(put_env).delete(delete_env))
        .route(&format!("{e}/deployment-branch-policies"), get(list_policies).post(create_policy))
        .route(&format!("{e}/deployment-branch-policies/{{id}}"), get(get_policy).put(update_policy).delete(delete_policy))
        .route(&format!("{e}/deployment_protection_rules"), get(list_rules).post(create_rule))
        .route(&format!("{e}/deployment_protection_rules/apps"), get(list_rule_apps))
        .route(&format!("{e}/deployment_protection_rules/{{id}}"), get(get_rule).delete(delete_rule))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EnvRow {
    pub id: i64,
    pub repo_id: i64,
    pub name: String,
    pub wait_timer: i32,
    pub prevent_self_review: bool,
    pub reviewers: Value,
    pub protected_branches: Option<bool>,
    pub custom_branch_policies: Option<bool>,
    pub wait_timer_rule_id: Option<i64>,
    pub reviewers_rule_id: Option<i64>,
    pub branch_rule_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PolicyRow {
    pub id: i64,
    pub environment_id: i64,
    pub name: String,
    #[sqlx(rename = "type")]
    pub kind: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RuleRow {
    pub id: i64,
    pub environment_id: i64,
    pub integration_id: i64,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------------------------
// Service layer

/// The environment `name` of a repository (case-insensitive), if it exists.
pub async fn find_environment(s: &AppState, repo_id: i64, name: &str) -> ApiResult<Option<EnvRow>> {
    Ok(sqlx::query_as("SELECT * FROM environments WHERE repo_id = $1 AND lower(name) = lower($2)")
        .bind(repo_id)
        .bind(name)
        .fetch_optional(&s.db)
        .await?)
}

/// Creates the environment if it does not exist yet (deployments to a new environment name
/// create it, like GitHub).
pub async fn ensure_environment(s: &AppState, repo_id: i64, name: &str) -> ApiResult<EnvRow> {
    sqlx::query("INSERT INTO environments (repo_id, name) VALUES ($1, $2) ON CONFLICT (repo_id, lower(name)) DO NOTHING")
        .bind(repo_id)
        .bind(name)
        .execute(&s.db)
        .await?;
    find_environment(s, repo_id, name).await?.ok_or_else(ApiError::not_found)
}

async fn load_env(s: &AppState, ctx: &RepoCtx, name: &str) -> ApiResult<EnvRow> {
    find_environment(s, ctx.repo.id, name).await?.ok_or_else(ApiError::not_found)
}

fn invalid(msg: impl Into<String>) -> ApiError {
    let mut e = ApiError::unprocessable(msg);
    e.errors = None;
    e
}

#[derive(Debug, Deserialize, Default)]
pub struct ReviewerInput {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub id: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
pub struct EnvInput {
    pub wait_timer: Option<i64>,
    pub prevent_self_review: Option<bool>,
    pub reviewers: Option<Vec<ReviewerInput>>,
    /// `Some(None)` = explicit null (all branches).
    #[serde(default, deserialize_with = "double_option")]
    pub deployment_branch_policy: Option<Option<BranchPolicySettings>>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct BranchPolicySettings {
    pub protected_branches: bool,
    pub custom_branch_policies: bool,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(d)?))
}

async fn next_rule_id(s: &AppState) -> ApiResult<i64> {
    // Rule ids share a sequence that starts far above table identities so they never collide
    // with custom protection rule ids.
    let n: i64 = sqlx::query_scalar("SELECT nextval('environment_rule_ids')").fetch_one(&s.db).await?;
    Ok(1_000_000_000 + n)
}

/// `PUT /environments/{name}`: creates or updates an environment (admin).
pub async fn create_or_update_environment(s: &AppState, ctx: &RepoCtx, name: &str, input: EnvInput) -> ApiResult<EnvRow> {
    ctx.require(Perm::Admin)?;
    let name = name.trim();
    if name.is_empty() || name.len() > 255 {
        return Err(ApiError::validation("Environment", "name", "invalid"));
    }
    if let Some(w) = input.wait_timer {
        if !(0..=43200).contains(&w) {
            return Err(invalid("Wait timer must be between 0 and 43200 minutes"));
        }
    }
    let mut reviewers: Option<Vec<Value>> = None;
    if let Some(list) = &input.reviewers {
        if list.len() > 6 {
            return Err(invalid("A maximum of 6 reviewers can be specified"));
        }
        let mut out = Vec::new();
        for r in list {
            let kind = r.kind.as_deref().unwrap_or("User");
            let id = r.id.ok_or_else(|| invalid("Reviewer id is required"))?;
            match kind {
                "User" => {
                    let u = db::user_by_id(&s.db, id).await.map_err(|_| invalid(format!("User {id} not found")))?;
                    let p = db::permission(&s.db, Some(&u), &ctx.repo, &ctx.owner).await?;
                    if p < Perm::Read || u.kind != "User" {
                        return Err(invalid(format!("Reviewer {} must have read access to the repository", u.login)));
                    }
                }
                "Team" => {
                    let ok: Option<i64> = sqlx::query_scalar("SELECT id FROM teams WHERE id = $1 AND org_id = $2")
                        .bind(id)
                        .bind(ctx.owner.id)
                        .fetch_optional(&s.db)
                        .await?;
                    if ok.is_none() {
                        return Err(invalid(format!("Team {id} not found in {}", ctx.owner.login)));
                    }
                }
                other => return Err(invalid(format!("Invalid reviewer type: {other}"))),
            }
            if !out.iter().any(|v: &Value| v["type"] == kind && v["id"] == id) {
                out.push(json!({"type": kind, "id": id}));
            }
        }
        reviewers = Some(out);
    }
    if let Some(Some(p)) = &input.deployment_branch_policy {
        if p.protected_branches == p.custom_branch_policies {
            return Err(invalid(
                "Only one of `protected_branches` and `custom_branch_policies` can be set to true at a time",
            ));
        }
    }
    let env = ensure_environment(s, ctx.repo.id, name).await?;
    let wait = input.wait_timer.map(|w| w as i32).unwrap_or(env.wait_timer);
    let prevent = input.prevent_self_review.unwrap_or(env.prevent_self_review);
    let reviewers = reviewers.map(Value::Array).unwrap_or_else(|| env.reviewers.clone());
    let (pb, cb) = match input.deployment_branch_policy {
        Some(Some(p)) => (Some(p.protected_branches), Some(p.custom_branch_policies)),
        Some(None) => (None, None),
        None => (env.protected_branches, env.custom_branch_policies),
    };
    let wait_rule = match (wait > 0, env.wait_timer_rule_id) {
        (true, Some(id)) => Some(id),
        (true, None) => Some(next_rule_id(s).await?),
        (false, _) => None,
    };
    let has_reviewers = reviewers.as_array().is_some_and(|a| !a.is_empty());
    let rev_rule = match (has_reviewers, env.reviewers_rule_id) {
        (true, Some(id)) => Some(id),
        (true, None) => Some(next_rule_id(s).await?),
        (false, _) => None,
    };
    let branch_rule = match (pb.is_some(), env.branch_rule_id) {
        (true, Some(id)) => Some(id),
        (true, None) => Some(next_rule_id(s).await?),
        (false, _) => None,
    };
    // Switching away from custom policies drops them (GitHub does the same).
    if cb != Some(true) {
        sqlx::query("DELETE FROM environment_branch_policies WHERE environment_id = $1").bind(env.id).execute(&s.db).await?;
    }
    let row: EnvRow = sqlx::query_as(
        "UPDATE environments SET wait_timer = $2, prevent_self_review = $3, reviewers = $4, protected_branches = $5,
            custom_branch_policies = $6, wait_timer_rule_id = $7, reviewers_rule_id = $8, branch_rule_id = $9, updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(env.id)
    .bind(wait)
    .bind(prevent)
    .bind(&reviewers)
    .bind(pb)
    .bind(cb)
    .bind(wait_rule)
    .bind(rev_rule)
    .bind(branch_rule)
    .fetch_one(&s.db)
    .await?;
    Ok(row)
}

/// Deletes an environment with its policies, rules, secrets and variables (admin).
pub async fn delete_environment(s: &AppState, ctx: &RepoCtx, name: &str) -> ApiResult<()> {
    ctx.require(Perm::Admin)?;
    let env = load_env(s, ctx, name).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM actions_secrets WHERE repo_id = $1 AND environment = $2").bind(ctx.repo.id).bind(&env.name).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM actions_variables WHERE repo_id = $1 AND environment = $2").bind(ctx.repo.id).bind(&env.name).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM environments WHERE id = $1").bind(env.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Glob match for deployment branch policies (`*` = anything but `/`, `**` = anything,
/// `?` = one char), as documented by GitHub (Ruby `File.fnmatch` semantics).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    fn rec(p: &[u8], n: &[u8]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                (0..=n.len()).any(|i| rec(rest, &n[i..]))
            }
            Some(b'*') => {
                let rest = &p[1..];
                let mut i = 0;
                loop {
                    if rec(rest, &n[i..]) {
                        return true;
                    }
                    if i >= n.len() || n[i] == b'/' {
                        return false;
                    }
                    i += 1;
                }
            }
            Some(b'?') => !n.is_empty() && n[0] != b'/' && rec(&p[1..], &n[1..]),
            Some(c) => n.first() == Some(c) && rec(&p[1..], &n[1..]),
        }
    }
    rec(pattern.as_bytes(), name.as_bytes())
}

/// Whether `git_ref` (`refs/heads/x`, `refs/tags/v1` or a bare branch name) may deploy to the
/// environment, following its deployment branch policy. For the Actions runner and deployments.
pub async fn ref_may_deploy(s: &AppState, ctx: &RepoCtx, env: &EnvRow, git_ref: &str) -> ApiResult<bool> {
    let (kind, name) = if let Some(b) = git_ref.strip_prefix("refs/heads/") {
        ("branch", b)
    } else if let Some(t) = git_ref.strip_prefix("refs/tags/") {
        ("tag", t)
    } else {
        ("branch", git_ref)
    };
    match (env.protected_branches, env.custom_branch_policies) {
        (Some(true), _) => Ok(kind == "branch" && crate::protection::protected_branches(&s.db, ctx.repo.id).await.contains(name)),
        (_, Some(true)) => {
            let pols: Vec<PolicyRow> = sqlx::query_as("SELECT * FROM environment_branch_policies WHERE environment_id = $1")
                .bind(env.id)
                .fetch_all(&s.db)
                .await?;
            Ok(pols.iter().any(|p| p.kind == kind && glob_match(&p.name, name)))
        }
        _ => Ok(true),
    }
}

// ---------------------------------------------------------------------------------------------
// JSON

fn env_url(s: &AppState, ctx: &RepoCtx, name: &str) -> String {
    format!("{}/repos/{}/environments/{}", s.api_url(), ctx.full_name(), seg(name))
}

pub async fn environments_json(s: &AppState, ctx: &RepoCtx, rows: &[EnvRow]) -> ApiResult<Vec<Value>> {
    let mut user_ids = Vec::new();
    let mut team_ids = Vec::new();
    for r in rows {
        for rv in r.reviewers.as_array().into_iter().flatten() {
            let id = rv["id"].as_i64().unwrap_or(0);
            if rv["type"] == "Team" {
                team_ids.push(id)
            } else {
                user_ids.push(id)
            }
        }
    }
    let users = crate::json::issue::users_by_ids(&s.db, &user_ids).await?;
    let teams: Vec<super::orgs_extra::TeamRow> =
        sqlx::query_as("SELECT * FROM teams WHERE id = ANY($1)").bind(&team_ids).fetch_all(&s.db).await?;
    let api = s.api_url();
    let base = s.base_url();
    let full = ctx.full_name();
    Ok(rows
        .iter()
        .map(|r| {
            let mut rules = Vec::new();
            if let Some(id) = r.wait_timer_rule_id.filter(|_| r.wait_timer > 0) {
                rules.push(json!({"id": id, "node_id": crate::ids::encode("ProtectionRule", id), "type": "wait_timer", "wait_timer": r.wait_timer}));
            }
            if let Some(id) = r.reviewers_rule_id {
                let reviewers: Vec<Value> = r
                    .reviewers
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|rv| {
                        let id = rv["id"].as_i64()?;
                        if rv["type"] == "Team" {
                            let t = teams.iter().find(|t| t.id == id)?;
                            Some(json!({"type": "Team", "reviewer": super::orgs_extra::team_json(s, &ctx.owner, t, None)}))
                        } else {
                            let u = users.get(&id)?;
                            Some(json!({"type": "User", "reviewer": crate::json::user::simple(&api, base, u)}))
                        }
                    })
                    .collect();
                rules.push(json!({"id": id, "node_id": crate::ids::encode("ProtectionRule", id), "type": "required_reviewers",
                    "prevent_self_review": r.prevent_self_review, "reviewers": reviewers}));
            }
            if let Some(id) = r.branch_rule_id.filter(|_| r.protected_branches.is_some()) {
                rules.push(json!({"id": id, "node_id": crate::ids::encode("ProtectionRule", id), "type": "branch_policy"}));
            }
            let policy = match (r.protected_branches, r.custom_branch_policies) {
                (Some(pb), cb) => json!({"protected_branches": pb, "custom_branch_policies": cb.unwrap_or(false)}),
                _ => Value::Null,
            };
            json!({
                "id": r.id,
                "node_id": crate::ids::encode("Environment", r.id),
                "name": r.name,
                "url": env_url(s, ctx, &r.name),
                "html_url": format!("{base}/{full}/deployments/activity_log?environments_filter={}", seg(&r.name)),
                "created_at": crate::json::ts(r.created_at),
                "updated_at": crate::json::ts(r.updated_at),
                "protection_rules": rules,
                "deployment_branch_policy": policy,
            })
        })
        .collect())
}

fn policy_json(p: &PolicyRow) -> Value {
    json!({"id": p.id, "node_id": crate::ids::encode("DeploymentBranchPolicy", p.id), "name": p.name, "type": p.kind})
}

/// `custom-deployment-rule-app` for a GitHub App id, if the App exists. Apps are owned by the
/// apps area; until it provides a table this resolves nothing.
async fn rule_app(s: &AppState, integration_id: i64) -> ApiResult<Option<Value>> {
    let table: Option<String> = sqlx::query_scalar(
        "SELECT t FROM unnest(ARRAY['github_apps', 'apps']) t WHERE to_regclass('public.' || t) IS NOT NULL LIMIT 1",
    )
    .fetch_optional(&s.db)
    .await?;
    let Some(table) = table else { return Ok(None) };
    let row: Option<(i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT id, slug FROM {table} WHERE id = $1")))
        .bind(integration_id)
        .fetch_optional(&s.db)
        .await
        .unwrap_or(None);
    Ok(row.map(|(id, slug)| {
        json!({"id": id, "slug": slug, "node_id": crate::ids::encode("Integration", id),
            "integration_url": format!("{}/apps/{slug}", s.api_url())})
    }))
}

async fn rule_json(s: &AppState, r: &RuleRow) -> ApiResult<Value> {
    let app = rule_app(s, r.integration_id).await?.unwrap_or_else(|| {
        json!({"id": r.integration_id, "slug": format!("app-{}", r.integration_id),
            "node_id": crate::ids::encode("Integration", r.integration_id),
            "integration_url": format!("{}/app/installations/{}", s.api_url(), r.integration_id)})
    });
    Ok(json!({"id": r.id, "node_id": crate::ids::encode("DeploymentProtectionRule", r.id), "enabled": r.enabled, "app": app}))
}

// ---------------------------------------------------------------------------------------------
// Handlers

async fn read_ctx(s: &AppState, u: Option<&User>, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    db::load_repo(&s.db, u, owner, repo).await
}

async fn list(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM environments WHERE repo_id = $1").bind(ctx.repo.id).fetch_one(&s.db).await?;
    let rows: Vec<EnvRow> = sqlx::query_as("SELECT * FROM environments WHERE repo_id = $1 ORDER BY lower(name) LIMIT $2 OFFSET $3")
        .bind(ctx.repo.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let envs = environments_json(&s, &ctx, &rows).await?;
    Ok(Json(json!({"total_count": total, "environments": envs})))
}

async fn get_one(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, env)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let e = load_env(&s, &ctx, &env).await?;
    Ok(Json(environments_json(&s, &ctx, &[e]).await?.remove(0)))
}

async fn put_env(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, env)): Path<(String, String, String)>,
    Json(input): Json<EnvInput>,
) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    let e = create_or_update_environment(&s, &ctx, &env, input).await?;
    Ok(Json(environments_json(&s, &ctx, &[e]).await?.remove(0)))
}

async fn delete_env(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, env)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    delete_environment(&s, &ctx, &env).await?;
    Ok(no_content())
}

/// Environment with custom branch policies enabled (404 otherwise, as documented).
async fn custom_policy_env(s: &AppState, ctx: &RepoCtx, env: &str) -> ApiResult<EnvRow> {
    let e = load_env(s, ctx, env).await?;
    if e.custom_branch_policies != Some(true) {
        return Err(ApiError::not_found());
    }
    Ok(e)
}

async fn list_policies(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, env)): Path<(String, String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let e = load_env(&s, &ctx, &env).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM environment_branch_policies WHERE environment_id = $1").bind(e.id).fetch_one(&s.db).await?;
    let rows: Vec<PolicyRow> = sqlx::query_as("SELECT * FROM environment_branch_policies WHERE environment_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(e.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    Ok(Json(json!({"total_count": total, "branch_policies": rows.iter().map(policy_json).collect::<Vec<_>>()})))
}

#[derive(Deserialize)]
struct PolicyInput {
    name: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// Adds a branch/tag name pattern to an environment with custom branch policies (admin).
pub async fn create_branch_policy(s: &AppState, ctx: &RepoCtx, env: &str, name: &str, kind: &str) -> ApiResult<(bool, PolicyRow)> {
    ctx.require(Perm::Admin)?;
    let e = custom_policy_env(s, ctx, env).await?;
    if name.is_empty() || name.len() > 255 {
        return Err(ApiError::validation("DeploymentBranchPolicy", "name", "invalid"));
    }
    if kind != "branch" && kind != "tag" {
        return Err(ApiError::validation("DeploymentBranchPolicy", "type", "invalid"));
    }
    if let Some(existing) = sqlx::query_as::<_, PolicyRow>(
        "SELECT * FROM environment_branch_policies WHERE environment_id = $1 AND type = $2 AND name = $3",
    )
    .bind(e.id)
    .bind(kind)
    .bind(name)
    .fetch_optional(&s.db)
    .await?
    {
        return Ok((false, existing));
    }
    let row: PolicyRow = sqlx::query_as("INSERT INTO environment_branch_policies (environment_id, name, type) VALUES ($1,$2,$3) RETURNING *")
        .bind(e.id)
        .bind(name)
        .bind(kind)
        .fetch_one(&s.db)
        .await?;
    Ok((true, row))
}

async fn create_policy(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, env)): Path<(String, String, String)>,
    Json(input): Json<PolicyInput>,
) -> ApiResult<Response> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    let name = input.name.unwrap_or_default();
    let (created, row) = create_branch_policy(&s, &ctx, &env, &name, input.kind.as_deref().unwrap_or("branch")).await?;
    if !created {
        // GitHub answers 303 See Other pointing at the existing policy.
        let loc = format!("{}/deployment-branch-policies/{}", env_url(&s, &ctx, &env), row.id);
        return Ok((StatusCode::SEE_OTHER, [(header::LOCATION, loc)]).into_response());
    }
    Ok(axum::Json(policy_json(&row)).into_response())
}

async fn load_policy(s: &AppState, e: &EnvRow, id: i64) -> ApiResult<PolicyRow> {
    sqlx::query_as("SELECT * FROM environment_branch_policies WHERE id = $1 AND environment_id = $2")
        .bind(id)
        .bind(e.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_policy(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, env, id)): Path<(String, String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let e = load_env(&s, &ctx, &env).await?;
    Ok(Json(policy_json(&load_policy(&s, &e, id).await?)))
}

async fn update_policy(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, env, id)): Path<(String, String, String, i64)>,
    Json(input): Json<PolicyInput>,
) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let e = load_env(&s, &ctx, &env).await?;
    let p = load_policy(&s, &e, id).await?;
    let name = input.name.filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("DeploymentBranchPolicy", "name", "missing_field"))?;
    let row: PolicyRow = sqlx::query_as("UPDATE environment_branch_policies SET name = $2 WHERE id = $1 RETURNING *")
        .bind(p.id)
        .bind(&name)
        .fetch_one(&s.db)
        .await
        .map_err(|_| ApiError::validation("DeploymentBranchPolicy", "name", "already_exists"))?;
    Ok(Json(policy_json(&row)))
}

async fn delete_policy(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, env, id)): Path<(String, String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let e = load_env(&s, &ctx, &env).await?;
    let p = load_policy(&s, &e, id).await?;
    sqlx::query("DELETE FROM environment_branch_policies WHERE id = $1").bind(p.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn list_rules(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, env)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let e = load_env(&s, &ctx, &env).await?;
    let rows: Vec<RuleRow> = sqlx::query_as("SELECT * FROM environment_protection_rules WHERE environment_id = $1 ORDER BY id").bind(e.id).fetch_all(&s.db).await?;
    let mut items = Vec::new();
    for r in &rows {
        items.push(rule_json(&s, r).await?);
    }
    Ok(Json(json!({"total_count": items.len(), "custom_deployment_protection_rules": items})))
}

#[derive(Deserialize)]
struct RuleInput {
    integration_id: Option<i64>,
}

async fn create_rule(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, env)): Path<(String, String, String)>,
    Json(input): Json<RuleInput>,
) -> ApiResult<Response> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let e = load_env(&s, &ctx, &env).await?;
    let id = input.integration_id.ok_or_else(|| ApiError::validation("DeploymentProtectionRule", "integration_id", "missing_field"))?;
    if rule_app(&s, id).await?.is_none() {
        return Err(invalid(format!("Integration {id} is not installed on this repository or does not exist")));
    }
    let row: RuleRow = sqlx::query_as(
        "INSERT INTO environment_protection_rules (environment_id, integration_id) VALUES ($1,$2)
         ON CONFLICT (environment_id, integration_id) DO UPDATE SET enabled = TRUE RETURNING *",
    )
    .bind(e.id)
    .bind(id)
    .fetch_one(&s.db)
    .await?;
    Ok((StatusCode::CREATED, axum::Json(rule_json(&s, &row).await?)).into_response())
}

async fn list_rule_apps(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, env)): Path<(String, String, String)>,
    Query(_p): Query<PageParams>,
) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    load_env(&s, &ctx, &env).await?;
    // Apps with the deployment-protection-rule capability installed on the repository: forge
    // has no GitHub Apps yet, so none are available.
    Ok(Json(json!({"total_count": 0, "available_custom_deployment_protection_rule_integrations": []})))
}

async fn load_rule(s: &AppState, e: &EnvRow, id: i64) -> ApiResult<RuleRow> {
    sqlx::query_as("SELECT * FROM environment_protection_rules WHERE id = $1 AND environment_id = $2")
        .bind(id)
        .bind(e.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_rule(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, env, id)): Path<(String, String, String, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = read_ctx(&s, u.as_ref(), &owner, &repo).await?;
    let e = load_env(&s, &ctx, &env).await?;
    let r = load_rule(&s, &e, id).await?;
    Ok(Json(rule_json(&s, &r).await?))
}

async fn delete_rule(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, env, id)): Path<(String, String, String, i64)>,
) -> ApiResult<Response> {
    let ctx = read_ctx(&s, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let e = load_env(&s, &ctx, &env).await?;
    let r = load_rule(&s, &e, id).await?;
    sqlx::query("DELETE FROM environment_protection_rules WHERE id = $1").bind(r.id).execute(&s.db).await?;
    Ok(no_content())
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn globs() {
        assert!(glob_match("release/*", "release/1.0"));
        assert!(!glob_match("release/*", "release/1.0/x"));
        assert!(glob_match("release/**", "release/1.0/x"));
        assert!(glob_match("main", "main"));
        assert!(glob_match("v?", "v1"));
        assert!(!glob_match("v?", "v10"));
    }
}
