//! The single call site between the runner and Actions settings (`api::actions_settings`,
//! owned by the actions-settings area): secrets and variables injected into jobs, whether
//! Actions and a given action are allowed, the default GITHUB_TOKEN permission, the fork pull
//! request policy, retention, self-hosted runner and runner group policy.
use serde_json::{Map, Value};

use crate::api::actions_settings::{self as aset, policy, runner_groups};
use crate::db::{Perm, RepoCtx};
use crate::error::ApiResult;
use crate::runner::token::RunnerRow;
use crate::state::AppState;

pub use crate::api::actions_settings::policy::ForkPrPolicy;

fn warn<T: Default>(what: &str, r: ApiResult<T>) -> T {
    r.unwrap_or_else(|e| {
        tracing::warn!(error = %e.message, "actions settings: {what} failed");
        T::default()
    })
}

/// Decrypted secrets for a job of `ctx` in `environment` (name, value).
pub async fn secrets_for_job(s: &AppState, ctx: &RepoCtx, environment: Option<&str>) -> Vec<(String, String)> {
    warn("secrets_for_job", aset::secrets_for_job(s, ctx.repo.id, environment).await)
}

/// Variables for a job (`vars` context).
pub async fn vars_for_job(s: &AppState, ctx: &RepoCtx, environment: Option<&str>) -> ApiResult<Value> {
    let rows = warn("variables_for_job", aset::variables_for_job(s, ctx.repo.id, environment).await);
    let mut m = Map::new();
    for (name, value) in rows {
        m.insert(name, Value::String(value));
    }
    Ok(Value::Object(m))
}

/// Whether workflows may run in the repository.
pub async fn actions_allowed(s: &AppState, repo_id: i64) -> bool {
    policy::actions_allowed(s, repo_id).await
}

/// Default `contents` permission of GITHUB_TOKEN when the workflow sets no `permissions:`.
pub async fn default_token_perm(s: &AppState, repo_id: i64) -> Perm {
    match policy::workflow_token_permissions(s, repo_id).await {
        Ok(p) if p.write => Perm::Write,
        Ok(_) => Perm::Read,
        Err(e) => {
            tracing::warn!(error = %e.message, "workflow_token_permissions failed");
            Perm::Read
        }
    }
}

/// Whether a step may use the action `uses` (as written in the workflow).
pub async fn action_allowed(s: &AppState, repo_id: i64, uses: &str) -> bool {
    policy::action_allowed(s, repo_id, uses).await.unwrap_or(false)
}

/// Policy for runs from fork pull requests.
pub async fn fork_pr_policy(s: &AppState, repo_id: i64) -> Option<ForkPrPolicy> {
    policy::fork_pr_policy(s, repo_id).await.ok()
}

/// Artifact (and log) retention in days.
pub async fn retention_days(s: &AppState, repo_id: i64) -> i64 {
    policy::retention_days(s, repo_id).await.map(|d| d as i64).unwrap_or(90)
}

/// Whether `runner` may take a job of `repo_id` (workflow `workflow_ref`): self-hosted runners
/// must be allowed for the repository, and organization/enterprise runners only take jobs
/// their runner group serves.
pub async fn runner_may_take(s: &AppState, runner: &RunnerRow, repo_id: i64, workflow_ref: Option<&str>) -> bool {
    if !policy::self_hosted_runners_allowed(s, repo_id).await.unwrap_or(false) {
        return false;
    }
    let (kind, owner) = match (runner.repo_id, runner.org_id) {
        (Some(_), _) => return true,
        (None, Some(o)) => ("org", o),
        (None, None) => ("enterprise", 0),
    };
    match runner_groups::runner_group_of(s, kind, owner, runner.id).await {
        Ok(g) => runner_groups::runner_group_allows(s, g, repo_id, workflow_ref).await.unwrap_or(false),
        Err(_) => false,
    }
}

/// The runner group id shown for a runner (`runner_group_id` in the runner JSON).
pub async fn runner_group_id(s: &AppState, runner: &RunnerRow) -> i64 {
    let (kind, owner) = match (runner.repo_id, runner.org_id) {
        (Some(_), _) => return runner.runner_group_id,
        (None, Some(o)) => ("org", o),
        (None, None) => ("enterprise", 0),
    };
    runner_groups::runner_group_of(s, kind, owner, runner.id).await.unwrap_or(runner.runner_group_id)
}
