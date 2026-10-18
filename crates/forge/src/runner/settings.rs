//! The single call site between the runner and Actions settings (owned by the
//! "actions-settings" area): secrets and variables injected into jobs, whether Actions is
//! allowed for a repository, and the default GITHUB_TOKEN permission.
//!
//! Until `api::actions_settings` lands these are interim implementations: no secrets (only
//! GITHUB_TOKEN is injected), variables read from `actions_variables`, Actions always allowed,
//! and write as the default token permission. Swap each body for the actions-settings call
//! (`secrets_for_job`, the variables equivalent, `actions_allowed`, default workflow
//! permissions) when merging.
use serde_json::{Map, Value};

use crate::db::{Perm, RepoCtx};
use crate::error::ApiResult;
use crate::state::AppState;

/// Decrypted secrets for a job of `ctx` in `environment` (name, value).
pub async fn secrets_for_job(_s: &AppState, _ctx: &RepoCtx, _environment: Option<&str>) -> Vec<(String, String)> {
    Vec::new()
}

/// Variables for a job (`vars` context): organization, repository, then environment values.
pub async fn vars_for_job(s: &AppState, ctx: &RepoCtx, environment: Option<&str>) -> ApiResult<Value> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, value FROM actions_variables
          WHERE (repo_id = $1 AND (environment = '' OR environment = $3))
             OR (org_id = $2 AND environment = '' AND (visibility IS NULL OR visibility = 'all' OR (visibility = 'private' AND $4)))
          ORDER BY (repo_id IS NOT NULL), (environment <> '')",
    )
    .bind(ctx.repo.id)
    .bind(ctx.owner.id)
    .bind(environment.unwrap_or(""))
    .bind(ctx.repo.private)
    .fetch_all(&s.db)
    .await
    .unwrap_or_default();
    let mut m = Map::new();
    for (name, value) in rows {
        m.insert(name, Value::String(value));
    }
    Ok(Value::Object(m))
}

/// Whether workflows may run in the repository.
pub async fn actions_allowed(_s: &AppState, _repo_id: i64) -> bool {
    true
}

/// Default `contents` permission of GITHUB_TOKEN when the workflow sets no `permissions:`.
pub async fn default_token_perm(_s: &AppState, _repo_id: i64) -> Perm {
    Perm::Write
}
