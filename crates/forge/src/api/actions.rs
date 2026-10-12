//! GitHub Actions read endpoints. forge does not run workflows: workflow files under
//! `.github/workflows` are listed, and every run/job/artifact list is validly empty
//! (`gh run list`, `gh workflow list`, `gh pr checks` read these).
use axum::extract::{Path, State};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::plat::total_envelope;
use crate::auth::MaybeUser;
use crate::db::{self, RepoCtx};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/actions";
    Router::new()
        .route(&format!("{r}/workflows"), get(list_workflows))
        .route(&format!("{r}/workflows/{{workflow_id}}"), get(get_workflow))
        .route(&format!("{r}/workflows/{{workflow_id}}/runs"), get(workflow_runs))
        .route(&format!("{r}/runs"), get(runs))
        .route(&format!("{r}/runs/{{run_id}}"), get(no_run).delete(no_run))
        .route(&format!("{r}/runs/{{run_id}}/jobs"), get(no_run))
        .route(&format!("{r}/runs/{{run_id}}/attempts/{{attempt}}"), get(no_run))
        .route(&format!("{r}/runs/{{run_id}}/attempts/{{attempt}}/jobs"), get(no_run))
        .route(&format!("{r}/runs/{{run_id}}/artifacts"), get(no_run))
        .route(&format!("{r}/runs/{{run_id}}/logs"), get(no_run))
        .route(&format!("{r}/jobs/{{job_id}}"), get(no_run))
        .route(&format!("{r}/jobs/{{job_id}}/logs"), get(no_run))
        .route(&format!("{r}/artifacts"), get(artifacts))
        .route(&format!("{r}/caches"), get(caches))
        .route(&format!("{r}/secrets"), get(secrets))
        .route(&format!("{r}/variables"), get(variables))
        .route(&format!("{r}/runners"), get(runners))
        .route(&format!("{r}/permissions"), get(permissions))
}

/// A workflow file discovered in the repository.
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub path: String,
}

fn workflow_id(repo_id: i64, path: &str) -> i64 {
    let h = Sha256::digest(format!("{repo_id}:{path}").as_bytes());
    // 47 bits: positive and safe for JavaScript clients.
    (u64::from_be_bytes(h[..8].try_into().unwrap()) >> 17) as i64
}

/// Lists `.github/workflows/*.yml|yaml` on the default branch.
pub async fn workflows(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<Workflow>> {
    let refname = format!("refs/heads/{}", ctx.repo.default_branch);
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    if !refs.contains_key(&refname) {
        return Ok(vec![]);
    }
    let tree = format!("{refname}:.github/workflows");
    let Some(out) = super::plat::git_read(s, ctx.repo.id, &["ls-tree", &tree]).await? else { return Ok(vec![]) };
    let mut list = Vec::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let Some((meta, name)) = line.split_once('\t') else { continue };
        let f: Vec<&str> = meta.split_whitespace().collect();
        if f.get(1) != Some(&"blob") || !(name.ends_with(".yml") || name.ends_with(".yaml")) {
            continue;
        }
        let path = format!(".github/workflows/{name}");
        let content = super::plat::git_read(s, ctx.repo.id, &["cat-file", "blob", f[2]]).await?.unwrap_or_default();
        let text = String::from_utf8_lossy(&content);
        let wf_name = text
            .lines()
            .find_map(|l| l.strip_prefix("name:").map(|v| v.trim().trim_matches(['"', '\'']).to_string()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| path.clone());
        list.push(Workflow { id: workflow_id(ctx.repo.id, &path), name: wf_name, path });
    }
    Ok(list)
}

fn workflow_json(s: &AppState, ctx: &RepoCtx, w: &Workflow) -> Value {
    let full = ctx.full_name();
    let base = s.base_url();
    json!({
        "id": w.id,
        "node_id": crate::ids::encode("Workflow", w.id),
        "name": w.name,
        "path": w.path,
        "state": "active",
        "created_at": crate::json::ts(ctx.repo.created_at),
        "updated_at": crate::json::ts(ctx.repo.pushed_at.unwrap_or(ctx.repo.updated_at)),
        "url": format!("{}/repos/{full}/actions/workflows/{}", s.api_url(), w.id),
        "html_url": format!("{base}/{full}/blob/{}/{}", ctx.repo.default_branch, w.path),
        "badge_url": format!("{base}/{full}/workflows/{}/badge.svg", w.name),
    })
}

async fn repo(s: &AppState, u: Option<&crate::db::User>, owner: &str, name: &str) -> ApiResult<RepoCtx> {
    db::load_repo(&s.db, u, owner, name).await
}

async fn list_workflows(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let wfs = workflows(&s, &ctx).await?;
    let n = wfs.len() as i64;
    Ok(Json(total_envelope("workflows", wfs.iter().map(|w| workflow_json(&s, &ctx, w)).collect(), n)))
}

async fn find_workflow(s: &AppState, ctx: &RepoCtx, id: &str) -> ApiResult<Workflow> {
    workflows(s, ctx)
        .await?
        .into_iter()
        .find(|w| w.id.to_string() == id || w.path.rsplit('/').next() == Some(id))
        .ok_or_else(ApiError::not_found)
}

async fn get_workflow(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    let w = find_workflow(&s, &ctx, &id).await?;
    Ok(Json(workflow_json(&s, &ctx, &w)))
}

async fn workflow_runs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name, id)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    find_workflow(&s, &ctx, &id).await?;
    Ok(Json(total_envelope("workflow_runs", vec![], 0)))
}

async fn runs(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    repo(&s, u.as_ref(), &owner, &name).await?;
    Ok(Json(total_envelope("workflow_runs", vec![], 0)))
}

async fn no_run(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path(p): Path<Vec<(String, String)>>) -> ApiResult<Json<Value>> {
    let get = |k: &str| p.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    repo(&s, u.as_ref(), &get("owner"), &get("repo")).await?;
    Err(ApiError::not_found())
}

async fn artifacts(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    repo(&s, u.as_ref(), &owner, &name).await?;
    Ok(Json(total_envelope("artifacts", vec![], 0)))
}

async fn caches(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    repo(&s, u.as_ref(), &owner, &name).await?;
    Ok(Json(total_envelope("actions_caches", vec![], 0)))
}

async fn secrets(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    ctx.require(crate::db::Perm::Admin)?;
    Ok(Json(total_envelope("secrets", vec![], 0)))
}

async fn variables(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    ctx.require(crate::db::Perm::Admin)?;
    Ok(Json(total_envelope("variables", vec![], 0)))
}

async fn runners(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    ctx.require(crate::db::Perm::Admin)?;
    Ok(Json(total_envelope("runners", vec![], 0)))
}

async fn permissions(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = repo(&s, u.as_ref(), &owner, &name).await?;
    ctx.require(crate::db::Perm::Admin)?;
    Ok(Json(json!({ "enabled": false, "allowed_actions": "all", "selected_actions_url": null })))
}
