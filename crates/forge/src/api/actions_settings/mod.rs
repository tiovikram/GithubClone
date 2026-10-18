//! Actions settings: permission policies at repository / organization / enterprise level,
//! self-hosted runner groups, the repository OIDC subject-claim template, and the services the
//! runner calls before and while executing a job (`actions_allowed`, `workflow_token_permissions`,
//! `action_allowed`, `secrets_for_job`, `variables_for_job`, ...).
//!
//! Secrets and variables CRUD lives in `api/actions_secrets.rs`; the encryption of secrets
//! (X25519 key pairs, master key) in `crypto`.
//!
//! forge (like GHES) has a single enterprise: every `{enterprise}` slug names it, and only site
//! administrators may manage it. forge has no GitHub-hosted runner fleet; jobs run on
//! self-hosted runners registered with `forge runner`.
use axum::Router;
use serde_json::Value;

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub mod crypto;
pub mod policy;
pub mod runner_groups;

pub use policy::{
    action_allowed, actions_allowed, fork_pr_policy, oidc_sub_claim_keys, retention_days, self_hosted_runners_allowed,
    workflow_access_allowed, workflow_token_permissions, ForkPrPolicy, WorkflowTokenPermissions,
};
pub use runner_groups::{runner_group_allows, runner_group_of};

/// Org- and repository-level routes (tag `actions`).
pub fn routes() -> Router<AppState> {
    policy::routes().merge(runner_groups::routes())
}

/// Enterprise-level routes. The `actions/*-enterprise` workflow-permission ops are tag `actions`;
/// the rest are tag `enterprise-admin` but share this module's data model, so they live here.
pub fn enterprise_routes() -> Router<AppState> {
    policy::enterprise_routes().merge(runner_groups::enterprise_routes())
}

// ---------------------------------------------------------------------------------------------
// Authorization helpers

/// Repository the caller administers (404 if invisible, 403 if not admin).
pub async fn repo_admin(s: &AppState, u: &User, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    ctx.require(Perm::Admin)?;
    Ok(ctx)
}

/// Organization the caller owns (site admins too): 404 for non-members, 403 for members.
pub async fn org_admin(s: &AppState, u: &User, org: &str) -> ApiResult<User> {
    let o = db::user_by_login(&s.db, org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    if u.site_admin {
        return Ok(o);
    }
    match super::orgs_extra::org_role(&s.db, o.id, u.id).await?.as_deref() {
        Some("admin") => Ok(o),
        Some(_) => Err(ApiError::forbidden("You must be an organization owner to manage GitHub Actions settings.")),
        None => Err(ApiError::not_found()),
    }
}

/// The enterprise: site administrators only (others get 404, as for an unknown enterprise).
pub fn enterprise_admin(u: &User, slug: &str) -> ApiResult<()> {
    if u.site_admin && !slug.is_empty() {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// `minimal-repository` objects for repositories owned by `org`, in `ids` order of id.
pub async fn org_repos_json(s: &AppState, org: &User, ids: &[i64]) -> ApiResult<Vec<Value>> {
    let repos: Vec<db::Repo> = sqlx::query_as("SELECT * FROM repositories WHERE owner_id = $1 AND id = ANY($2) ORDER BY id")
        .bind(org.id)
        .bind(ids)
        .fetch_all(&s.db)
        .await?;
    let (api, base) = (s.api_url(), s.base_url().to_string());
    Ok(repos
        .into_iter()
        .map(|repo| crate::json::repo::repo(&api, &base, &RepoCtx { repo, owner: org.clone(), perm: Perm::Admin }))
        .collect())
}

/// `organization-simple` objects for the given org ids.
pub async fn orgs_json(s: &AppState, ids: &[i64]) -> ApiResult<Vec<Value>> {
    let orgs: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE kind = 'Organization' AND id = ANY($1) ORDER BY id")
        .bind(ids)
        .fetch_all(&s.db)
        .await?;
    let (api, base) = (s.api_url(), s.base_url().to_string());
    Ok(orgs.iter().map(|o| crate::json::user::org_simple(&api, &base, o)).collect())
}

/// Repository ids that must all be owned by `org` (422 otherwise); deduplicated.
pub async fn check_org_repos(s: &AppState, org: &User, ids: &[i64], field: &str) -> ApiResult<Vec<i64>> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE owner_id = $1 AND id = ANY($2)")
        .bind(org.id)
        .bind(&ids)
        .fetch_one(&s.db)
        .await?;
    if n != ids.len() as i64 {
        return Err(ApiError::validation("Repository", field, "invalid"));
    }
    Ok(ids)
}

/// Organization ids that must all exist (422 otherwise); deduplicated.
pub async fn check_orgs(s: &AppState, ids: &[i64], field: &str) -> ApiResult<Vec<i64>> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'Organization' AND id = ANY($1)")
        .bind(&ids)
        .fetch_one(&s.db)
        .await?;
    if n != ids.len() as i64 {
        return Err(ApiError::validation("Organization", field, "invalid"));
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------------------------
// Secrets and variables for a job

/// The secrets a job in `repo_id` (optionally deploying to `environment`) receives, decrypted,
/// as (NAME, value) sorted by name. GitHub precedence: environment > repository > organization
/// (org secrets only when visible to the repository: `all`, `private` for non-public
/// repositories, or `selected` with the repository selected). Secrets whose ciphertext no
/// longer opens (e.g. sealed for a rotated key) are skipped with a warning.
pub async fn secrets_for_job(s: &AppState, repo_id: i64, environment: Option<&str>) -> ApiResult<Vec<(String, String)>> {
    let cond = super::actions_secrets::visible_to_repo(super::actions_secrets::Selected::Secret);
    // level: 0 org, 1 repo, 2 environment (higher wins)
    let rows: Vec<(String, String, i32, String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT i.name, i.encrypted_value, 0, 'org', i.org_id FROM actions_secrets i JOIN repositories r ON r.id = $1
           WHERE i.org_id = r.owner_id AND {cond}
         UNION ALL
         SELECT i.name, i.encrypted_value, CASE WHEN i.environment = '' THEN 1 ELSE 2 END, 'repo', i.repo_id FROM actions_secrets i
           WHERE i.repo_id = $1 AND (i.environment = '' OR ($2::text IS NOT NULL AND lower(i.environment) = lower($2)))"
    )))
    .bind(repo_id)
    .bind(environment)
    .fetch_all(&s.db)
    .await?;
    let mut keys: std::collections::HashMap<(String, i64), Option<crypto_box::SecretKey>> = Default::default();
    let mut best: std::collections::BTreeMap<String, (i32, String)> = Default::default();
    for (name, ct, level, kind, owner) in rows {
        let k = (kind.clone(), owner);
        if !keys.contains_key(&k) {
            keys.insert(k.clone(), crypto::private_key(s, &kind, owner).await?.map(|(_, sk)| sk));
        }
        let Some(value) = keys[&k].as_ref().and_then(|sk| crypto::open_sealed(sk, &ct)) else {
            tracing::warn!(repo_id, secret = %name, "actions secret cannot be decrypted; skipped");
            continue;
        };
        let name = name.to_ascii_uppercase();
        if best.get(&name).is_none_or(|(l, _)| level > *l) {
            best.insert(name, (level, value));
        }
    }
    Ok(best.into_iter().map(|(n, (_, v))| (n, v)).collect())
}

/// The configuration variables (`vars.*`) a job receives, same precedence as `secrets_for_job`.
pub async fn variables_for_job(s: &AppState, repo_id: i64, environment: Option<&str>) -> ApiResult<Vec<(String, String)>> {
    let cond = super::actions_secrets::visible_to_repo(super::actions_secrets::Selected::Variable);
    let rows: Vec<(String, String, i32)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT i.name, i.value, 0 FROM actions_variables i JOIN repositories r ON r.id = $1
           WHERE i.org_id = r.owner_id AND {cond}
         UNION ALL
         SELECT i.name, i.value, CASE WHEN i.environment = '' THEN 1 ELSE 2 END FROM actions_variables i
           WHERE i.repo_id = $1 AND (i.environment = '' OR ($2::text IS NOT NULL AND lower(i.environment) = lower($2)))"
    )))
    .bind(repo_id)
    .bind(environment)
    .fetch_all(&s.db)
    .await?;
    let mut best: std::collections::BTreeMap<String, (i32, String)> = Default::default();
    for (name, value, level) in rows {
        let name = name.to_ascii_uppercase();
        if best.get(&name).is_none_or(|(l, _)| level > *l) {
            best.insert(name, (level, value));
        }
    }
    Ok(best.into_iter().map(|(n, (_, v))| (n, v)).collect())
}
