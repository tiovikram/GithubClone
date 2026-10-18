//! Deployment environments for jobs (`jobs.<id>.environment`), using the repos area's
//! environments (`api::repos_env`): deployment branch policies, required reviewers (pending
//! deployments, approve/reject) and wait timers. A gated job waits with status `waiting`;
//! [`release_waiting`] (scheduler tick and after each review) queues it once its gates open.
use chrono::Utc;
use serde_json::{json, Value};

use super::engine::{self, JobRow};
use crate::api::actions::RunRow;
use crate::api::repos_env::{self, EnvRow};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// The environment name of a job definition (`environment: name` or `{name, url}`).
pub fn env_name(def: &Value) -> Option<String> {
    match def.get("environment") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Object(o)) => o.get("name").and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty()),
        _ => None,
    }
}

pub enum Gate {
    Open,
    Wait,
    Fail(String),
}

async fn review_state(s: &AppState, run_id: i64, env_id: i64) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT state FROM actions_run_approvals WHERE run_id = $1 AND environments @> jsonb_build_array(jsonb_build_object('id', $2::bigint))
          ORDER BY id DESC LIMIT 1",
    )
    .bind(run_id)
    .bind(env_id)
    .fetch_optional(&s.db)
    .await?)
}

fn has_reviewers(env: &EnvRow) -> bool {
    env.reviewers.as_array().is_some_and(|a| !a.is_empty())
}

/// Whether a job deploying to `name` may start now (`since`: when it became ready).
pub async fn gate(s: &AppState, ctx: &RepoCtx, run: &RunRow, name: &str, since: chrono::DateTime<Utc>) -> ApiResult<Gate> {
    let env = repos_env::ensure_environment(s, ctx.repo.id, name).await?;
    let git_ref = run.git_ref.clone().unwrap_or_default();
    if !repos_env::ref_may_deploy(s, ctx, &env, &git_ref).await? {
        return Ok(Gate::Fail(format!("Branch \"{git_ref}\" is not allowed to deploy to {name} due to environment protection rules.")));
    }
    if has_reviewers(&env) {
        match review_state(s, run.id, env.id).await?.as_deref() {
            Some("approved") => {}
            Some("rejected") => return Ok(Gate::Fail(format!("The deployment to {name} was rejected."))),
            _ => return Ok(Gate::Wait),
        }
    }
    if env.wait_timer > 0 && Utc::now() < since + chrono::Duration::minutes(env.wait_timer as i64) {
        return Ok(Gate::Wait);
    }
    Ok(Gate::Open)
}

/// Queues (or fails) waiting jobs whose environment gates opened. Returns how many changed.
pub async fn release_waiting(s: &AppState, run_id: Option<i64>) -> ApiResult<usize> {
    let jobs: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE status = 'waiting' AND ($1::bigint IS NULL OR run_id = $1) ORDER BY id")
        .bind(run_id)
        .fetch_all(&s.db)
        .await?;
    let mut n = 0;
    for j in jobs {
        let run: RunRow = sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(j.run_id).fetch_one(&s.db).await?;
        let ctx = repo_ctx(s, run.repo_id).await?;
        let Some(name) = env_name(&j.definition) else { continue };
        let since = j.waiting_since.unwrap_or(j.created_at);
        match gate(s, &ctx, &run, &name, since).await? {
            Gate::Wait => continue,
            Gate::Open => {
                sqlx::query("UPDATE actions_jobs SET status = 'queued' WHERE id = $1 AND status = 'waiting'").bind(j.id).execute(&s.db).await?;
            }
            Gate::Fail(msg) => {
                let _ = super::logs::append(s, j.id, 1, format!("{} ##[error]{msg}\n", super::logs::stamp()).as_bytes()).await;
                sqlx::query("UPDATE actions_jobs SET status = 'in_progress', started_at = now() WHERE id = $1 AND status = 'waiting'").bind(j.id).execute(&s.db).await?;
                engine::finish_job(s, j.id, "failure", None, None).await?;
            }
        }
        n += 1;
        engine::advance(s, run.id).await?;
    }
    Ok(n)
}

async fn repo_ctx(s: &AppState, repo_id: i64) -> ApiResult<RepoCtx> {
    let repo = db::repo_by_id(&s.db, repo_id).await?;
    let owner = db::user_by_id(&s.db, repo.owner_id).await?;
    Ok(RepoCtx { repo, owner, perm: Perm::Admin })
}

/// Whether `user` is a required reviewer of the environment (directly or through a team).
async fn can_review(s: &AppState, env: &EnvRow, run: &RunRow, user: &User) -> ApiResult<bool> {
    if env.prevent_self_review && run.triggering_actor_id == Some(user.id) {
        return Ok(false);
    }
    for rv in env.reviewers.as_array().into_iter().flatten() {
        let id = rv["id"].as_i64().unwrap_or(0);
        if rv["type"] == "Team" {
            let member: Option<i64> = sqlx::query_scalar("SELECT 1::bigint FROM team_members WHERE team_id = $1 AND user_id = $2")
                .bind(id)
                .bind(user.id)
                .fetch_optional(&s.db)
                .await
                .unwrap_or(None);
            if member.is_some() {
                return Ok(true);
            }
        } else if id == user.id {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Environments a run's jobs are waiting on for review.
async fn pending_envs(s: &AppState, run: &RunRow) -> ApiResult<Vec<(EnvRow, chrono::DateTime<Utc>)>> {
    let jobs: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2 AND status = 'waiting' ORDER BY id")
        .bind(run.id)
        .bind(run.run_attempt)
        .fetch_all(&s.db)
        .await?;
    let mut out: Vec<(EnvRow, chrono::DateTime<Utc>)> = Vec::new();
    for j in jobs {
        let Some(name) = env_name(&j.definition) else { continue };
        let Some(env) = repos_env::find_environment(s, run.repo_id, &name).await? else { continue };
        if !out.iter().any(|(e, _)| e.id == env.id) {
            out.push((env, j.waiting_since.unwrap_or(j.created_at)));
        }
    }
    Ok(out)
}

fn env_object(s: &AppState, ctx: &RepoCtx, e: &EnvRow, full: bool) -> Value {
    let seg = crate::json::release::seg(&e.name);
    let mut v = json!({
        "id": e.id,
        "node_id": crate::ids::encode("Environment", e.id),
        "name": e.name,
        "url": format!("{}/repos/{}/environments/{seg}", s.api_url(), ctx.full_name()),
        "html_url": format!("{}/{}/deployments/activity_log?environments_filter={seg}", s.base_url(), ctx.full_name()),
    });
    if full {
        v["created_at"] = json!(crate::json::ts(e.created_at));
        v["updated_at"] = json!(crate::json::ts(e.updated_at));
    }
    v
}

/// `GET /actions/runs/{id}/pending_deployments`.
pub async fn pending_json(s: &AppState, ctx: &RepoCtx, run: &RunRow, viewer: Option<&User>) -> ApiResult<Value> {
    let mut out = Vec::new();
    for (env, since) in pending_envs(s, run).await? {
        if !has_reviewers(&env) {
            continue;
        }
        let rendered = repos_env::environments_json(s, ctx, std::slice::from_ref(&env)).await?;
        let reviewers = rendered
            .first()
            .and_then(|e| e["protection_rules"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .find(|r| r["type"] == "required_reviewers")
            .and_then(|r| r["reviewers"].as_array().cloned())
            .unwrap_or_default();
        let can = match viewer {
            Some(u) => can_review(s, &env, run, u).await?,
            None => false,
        };
        out.push(json!({
            "environment": env_object(s, ctx, &env, false),
            "wait_timer": env.wait_timer,
            "wait_timer_started_at": if env.wait_timer > 0 { json!(crate::json::ts(since)) } else { Value::Null },
            "current_user_can_approve": can,
            "reviewers": reviewers,
        }));
    }
    Ok(Value::Array(out))
}

/// `POST /actions/runs/{id}/pending_deployments`: approves or rejects waiting environments.
/// Returns the deployments created for approved environments.
pub async fn review(s: &AppState, ctx: &RepoCtx, run: &RunRow, user: &User, env_ids: &[i64], state: &str, comment: &str) -> ApiResult<Vec<Value>> {
    let pending = pending_envs(s, run).await?;
    let mut chosen: Vec<&EnvRow> = Vec::new();
    for id in env_ids {
        let Some((env, _)) = pending.iter().find(|(e, _)| e.id == *id && has_reviewers(e)) else {
            return Err(ApiError::unprocessable(format!("No pending deployment requests to approve or reject for environment {id}")));
        };
        if !can_review(s, env, run, user).await? {
            return Err(ApiError::forbidden(format!("You are not a required reviewer of environment {}", env.name)));
        }
        chosen.push(env);
    }
    if chosen.is_empty() {
        return Err(ApiError::unprocessable("No pending deployment requests to approve or reject"));
    }
    let envs: Vec<Value> = chosen.iter().map(|e| env_object(s, ctx, e, true)).collect();
    sqlx::query("INSERT INTO actions_run_approvals (run_id, environments, state, user_id, comment) VALUES ($1, $2, $3, $4, $5)")
        .bind(run.id)
        .bind(Value::Array(envs))
        .bind(state)
        .bind(user.id)
        .bind(comment)
        .execute(&s.db)
        .await?;
    let mut deployments = Vec::new();
    if state == "approved" {
        let actor_ctx = RepoCtx { perm: Perm::Admin, ..ctx.clone() };
        for env in &chosen {
            let req = crate::api::deployments::CreateDeployment {
                git_ref: Some(run.head_sha.clone()),
                task: Some("deploy".into()),
                auto_merge: Some(false),
                required_contexts: Some(vec![]),
                payload: None,
                environment: Some(env.name.clone()),
                description: Some(format!("Deployment of workflow run {} approved", run.id)),
                transient_environment: None,
                production_environment: None,
            };
            if let Ok(d) = crate::api::deployments::create_deployment(s, &actor_ctx, user, req).await {
                deployments.extend(crate::api::deployments::deployments_json(s, ctx, std::slice::from_ref(&d)).await?);
            }
        }
    }
    release_waiting(s, Some(run.id)).await?;
    Ok(deployments)
}
