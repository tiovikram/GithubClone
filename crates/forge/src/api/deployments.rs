//! Deployments and deployment statuses.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/deployments", get(list).post(create))
        .route("/repos/{owner}/{repo}/deployments/{deployment_id}", get(get_one).delete(delete))
        .route("/repos/{owner}/{repo}/deployments/{deployment_id}/statuses", get(list_statuses).post(create_status))
        .route("/repos/{owner}/{repo}/deployments/{deployment_id}/statuses/{status_id}", get(get_status))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeploymentRow {
    pub id: i64,
    pub repo_id: i64,
    pub sha: String,
    #[sqlx(rename = "ref")]
    pub git_ref: String,
    pub task: String,
    pub payload: Value,
    pub environment: String,
    pub original_environment: String,
    pub description: Option<String>,
    pub creator_id: Option<i64>,
    pub transient_environment: bool,
    pub production_environment: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StatusRow {
    pub id: i64,
    pub deployment_id: i64,
    pub state: String,
    pub creator_id: Option<i64>,
    pub description: String,
    pub environment: String,
    pub target_url: String,
    pub log_url: String,
    pub environment_url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn dep_url(s: &AppState, ctx: &RepoCtx, id: i64) -> String {
    format!("{}/repos/{}/deployments/{id}", s.api_url(), ctx.full_name())
}

pub async fn deployments_json(s: &AppState, ctx: &RepoCtx, rows: &[DeploymentRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().filter_map(|d| d.creator_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let api = s.api_url();
    Ok(rows
        .iter()
        .map(|d| {
            let url = dep_url(s, ctx, d.id);
            json!({
                "url": url,
                "id": d.id,
                "node_id": crate::ids::encode("Deployment", d.id),
                "sha": d.sha,
                "ref": d.git_ref,
                "task": d.task,
                "payload": d.payload,
                "original_environment": d.original_environment,
                "environment": d.environment,
                "description": d.description,
                "creator": d.creator_id.and_then(|u| users.get(&u)).map(|u| crate::json::user::simple(&api, s.base_url(), u)).unwrap_or(Value::Null),
                "created_at": crate::json::ts(d.created_at),
                "updated_at": crate::json::ts(d.updated_at),
                "statuses_url": format!("{url}/statuses"),
                "repository_url": format!("{api}/repos/{}", ctx.full_name()),
                "transient_environment": d.transient_environment,
                "production_environment": d.production_environment,
                "performed_via_github_app": null,
            })
        })
        .collect())
}

pub async fn statuses_json(s: &AppState, ctx: &RepoCtx, rows: &[StatusRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().filter_map(|d| d.creator_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let api = s.api_url();
    Ok(rows
        .iter()
        .map(|st| {
            let durl = dep_url(s, ctx, st.deployment_id);
            let mut v = json!({
                "url": format!("{durl}/statuses/{}", st.id),
                "id": st.id,
                "node_id": crate::ids::encode("DeploymentStatus", st.id),
                "state": st.state,
                "creator": st.creator_id.and_then(|u| users.get(&u)).map(|u| crate::json::user::simple(&api, s.base_url(), u)).unwrap_or(Value::Null),
                "description": st.description,
                "environment": st.environment,
                // GitHub sends "" when unset, but the schema requires a URI: fall back to the
                // environment URL or the deployments page.
                "target_url": if !st.target_url.is_empty() {
                    st.target_url.clone()
                } else if !st.environment_url.is_empty() {
                    st.environment_url.clone()
                } else {
                    format!("{}/{}/deployments", s.base_url(), ctx.full_name())
                },
                "created_at": crate::json::ts(st.created_at),
                "updated_at": crate::json::ts(st.updated_at),
                "deployment_url": durl,
                "repository_url": format!("{api}/repos/{}", ctx.full_name()),
                "performed_via_github_app": null,
            });
            // `environment_url`/`log_url` are uri-formatted: omit when empty.
            if !st.environment_url.is_empty() {
                v["environment_url"] = json!(st.environment_url);
            }
            if !st.log_url.is_empty() {
                v["log_url"] = json!(st.log_url);
            }
            v
        })
        .collect())
}

#[derive(Deserialize)]
pub struct CreateDeployment {
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub task: Option<String>,
    pub auto_merge: Option<bool>,
    pub required_contexts: Option<Vec<String>>,
    pub payload: Option<Value>,
    pub environment: Option<String>,
    pub description: Option<String>,
    pub transient_environment: Option<bool>,
    pub production_environment: Option<bool>,
}

pub async fn create_deployment(s: &AppState, ctx: &RepoCtx, actor: &User, req: CreateDeployment) -> ApiResult<DeploymentRow> {
    ctx.require(Perm::Write)?;
    let git_ref = req.git_ref.filter(|r| !r.is_empty()).ok_or_else(|| ApiError::validation("Deployment", "ref", "missing_field"))?;
    let path = s.git.sync(ctx.repo.id).await?;
    let sha = crate::git::plumbing::rev_parse(&s.git.git, &path, &git_ref).await.ok_or_else(|| {
        let mut e = ApiError::unprocessable(format!("No ref found for: {git_ref}"));
        e.errors = None;
        e
    })?;
    let env = req.environment.filter(|e| !e.is_empty()).unwrap_or_else(|| "production".into());
    let payload = match req.payload {
        Some(Value::String(st)) if !st.is_empty() => serde_json::from_str(&st).unwrap_or(Value::String(st)),
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    };
    let _ = (req.auto_merge, req.required_contexts);
    // Deploying to a new environment name creates the environment (GitHub behaviour).
    super::repos_env::ensure_environment(s, ctx.repo.id, &env).await?;
    let d: DeploymentRow = sqlx::query_as(
        "INSERT INTO deployments (repo_id, sha, ref, task, payload, environment, original_environment, description,
            creator_id, transient_environment, production_environment)
         VALUES ($1,$2,$3,$4,$5,$6,$6,$7,$8,$9,$10) RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(&sha)
    .bind(&git_ref)
    .bind(req.task.unwrap_or_else(|| "deploy".into()))
    .bind(&payload)
    .bind(&env)
    .bind(req.description)
    .bind(actor.id)
    .bind(req.transient_environment.unwrap_or(false))
    .bind(req.production_environment.unwrap_or(env == "production"))
    .fetch_one(&s.db)
    .await?;
    if let Ok(mut v) = deployments_json(s, ctx, std::slice::from_ref(&d)).await {
        let p = json!({"action": "created", "deployment": v.remove(0), "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor)});
        let _ = crate::webhooks::enqueue_event_ctx(s, ctx, "deployment", p).await;
    }
    Ok(d)
}

#[derive(Deserialize)]
pub struct CreateStatus {
    pub state: Option<String>,
    pub target_url: Option<String>,
    pub log_url: Option<String>,
    pub description: Option<String>,
    pub environment: Option<String>,
    pub environment_url: Option<String>,
    pub auto_inactive: Option<bool>,
}

const STATES: [&str; 7] = ["error", "failure", "inactive", "in_progress", "queued", "pending", "success"];

pub async fn create_deployment_status(s: &AppState, ctx: &RepoCtx, actor: &User, d: &DeploymentRow, req: CreateStatus) -> ApiResult<StatusRow> {
    ctx.require(Perm::Write)?;
    let state = req.state.filter(|st| STATES.contains(&st.as_str())).ok_or_else(|| ApiError::validation("DeploymentStatus", "state", "invalid"))?;
    let env = req.environment.filter(|e| !e.is_empty()).unwrap_or_else(|| d.environment.clone());
    let target = req.target_url.or(req.log_url.clone()).unwrap_or_default();
    let st: StatusRow = sqlx::query_as(
        "INSERT INTO deployment_statuses (deployment_id, state, creator_id, description, environment, target_url, log_url, environment_url)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *",
    )
    .bind(d.id)
    .bind(&state)
    .bind(actor.id)
    .bind(req.description.unwrap_or_default())
    .bind(&env)
    .bind(&target)
    .bind(req.log_url.unwrap_or_else(|| target.clone()))
    .bind(req.environment_url.unwrap_or_default())
    .fetch_one(&s.db)
    .await?;
    sqlx::query("UPDATE deployments SET updated_at = now() WHERE id = $1").bind(d.id).execute(&s.db).await?;
    // auto_inactive (default true): earlier successful deployments to the same environment
    // become inactive once a new one succeeds.
    if state == "success" && req.auto_inactive.unwrap_or(true) && !d.transient_environment {
        sqlx::query(
            "INSERT INTO deployment_statuses (deployment_id, state, creator_id, description, environment)
             SELECT dd.id, 'inactive', $3, '', dd.environment FROM deployments dd
              WHERE dd.repo_id = $1 AND dd.environment = $2 AND dd.id <> $4
                AND (SELECT state FROM deployment_statuses x WHERE x.deployment_id = dd.id ORDER BY x.id DESC LIMIT 1) = 'success'",
        )
        .bind(ctx.repo.id)
        .bind(&d.environment)
        .bind(actor.id)
        .bind(d.id)
        .execute(&s.db)
        .await?;
    }
    if let (Ok(mut sv), Ok(mut dv)) = (
        statuses_json(s, ctx, std::slice::from_ref(&st)).await,
        deployments_json(s, ctx, std::slice::from_ref(d)).await,
    ) {
        let p = json!({"action": "created", "deployment_status": sv.remove(0), "deployment": dv.remove(0),
            "sender": crate::json::user::simple(&s.api_url(), s.base_url(), actor)});
        let _ = crate::webhooks::enqueue_event_ctx(s, ctx, "deployment_status", p).await;
    }
    Ok(st)
}

async fn load_deployment(s: &AppState, ctx: &RepoCtx, id: i64) -> ApiResult<DeploymentRow> {
    sqlx::query_as("SELECT * FROM deployments WHERE id = $1 AND repo_id = $2")
        .bind(id)
        .bind(ctx.repo.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
struct ListQuery {
    sha: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    task: Option<String>,
    environment: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let filter = "repo_id = $1 AND ($2::text IS NULL OR sha = $2) AND ($3::text IS NULL OR ref = $3)
                  AND ($4::text IS NULL OR task = $4) AND ($5::text IS NULL OR environment = $5)";
    let rows: Vec<DeploymentRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM deployments WHERE {filter} ORDER BY id DESC LIMIT $6 OFFSET $7")))
        .bind(ctx.repo.id)
        .bind(&q.sha)
        .bind(&q.git_ref)
        .bind(&q.task)
        .bind(&q.environment)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM deployments WHERE {filter}")))
        .bind(ctx.repo.id)
        .bind(&q.sha)
        .bind(&q.git_ref)
        .bind(&q.task)
        .bind(&q.environment)
        .fetch_one(&s.db)
        .await?;
    let items = deployments_json(&s, &ctx, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<CreateDeployment>,
) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let d = create_deployment(&s, &ctx, &u, req).await?;
    Ok(created(deployments_json(&s, &ctx, std::slice::from_ref(&d)).await?.remove(0)))
}

async fn get_one(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let d = load_deployment(&s, &ctx, id).await?;
    Ok(Json(deployments_json(&s, &ctx, std::slice::from_ref(&d)).await?.remove(0)))
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    ctx.require(Perm::Write)?;
    let d = load_deployment(&s, &ctx, id).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM deployments WHERE repo_id = $1").bind(ctx.repo.id).fetch_one(&s.db).await?;
    let last: Option<String> = sqlx::query_scalar("SELECT state FROM deployment_statuses WHERE deployment_id = $1 ORDER BY id DESC LIMIT 1")
        .bind(d.id)
        .fetch_optional(&s.db)
        .await?;
    // GitHub: with more than one deployment, only inactive ones can be deleted.
    if count > 1 && last.as_deref().is_some_and(|st| st != "inactive" && st != "error" && st != "failure") {
        let mut e = ApiError::unprocessable("We cannot delete an active deployment unless it is the only deployment in a given environment.");
        e.errors = None;
        return Err(e);
    }
    sqlx::query("DELETE FROM deployments WHERE id = $1").bind(d.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn list_statuses(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let d = load_deployment(&s, &ctx, id).await?;
    let rows: Vec<StatusRow> = sqlx::query_as("SELECT * FROM deployment_statuses WHERE deployment_id = $1 ORDER BY id DESC LIMIT $2 OFFSET $3")
        .bind(d.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM deployment_statuses WHERE deployment_id = $1").bind(d.id).fetch_one(&s.db).await?;
    let items = statuses_json(&s, &ctx, &rows).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn create_status(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, id)): Path<(String, String, i64)>,
    Json(req): Json<CreateStatus>,
) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let d = load_deployment(&s, &ctx, id).await?;
    let st = create_deployment_status(&s, &ctx, &u, &d, req).await?;
    Ok(created(statuses_json(&s, &ctx, std::slice::from_ref(&st)).await?.remove(0)))
}

async fn get_status(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, id, sid)): Path<(String, String, i64, i64)>,
) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let d = load_deployment(&s, &ctx, id).await?;
    let st: StatusRow = sqlx::query_as("SELECT * FROM deployment_statuses WHERE id = $1 AND deployment_id = $2")
        .bind(sid)
        .bind(d.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(statuses_json(&s, &ctx, std::slice::from_ref(&st)).await?.remove(0)))
}
