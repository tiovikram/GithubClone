//! Self-hosted runner groups of organizations and of the enterprise.
//!
//! Every owner has a `Default` group (created on first use, cannot be deleted). A group serves
//! `all` repositories (org) / organizations (enterprise), only `private` repositories (org), or
//! a `selected` list; public repositories only when `allows_public_repositories`; and, when
//! `restricted_to_workflows`, only jobs from the listed workflow files. Enterprise groups
//! visible to an organization show up in its listing with `inherited: true` (read-only there).
//!
//! Runner membership is stored here (`actions_runner_group_runners`); a runner without a row
//! belongs to its owner's default group. The runners themselves (registration, status, labels)
//! belong to the runner area, reached through [`registered_runners`].
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{check_org_repos, check_orgs, enterprise_admin, org_admin, org_repos_json, orgs_json};
use crate::api::plat::total_envelope;
use crate::api::{no_content, PageParams};
use crate::auth::AuthUser;
use crate::db::User;
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

// ---------------------------------------------------------------------------------------------
// Runner registry (integration point with the runner area)

/// A self-hosted runner as rendered by the runner area (`runner` schema JSON).
pub struct RunnerInfo {
    pub id: i64,
    pub json: Value,
}

/// Self-hosted runners registered to an org (`"org"`, org id) or the enterprise
/// (`"enterprise"`, 0).
///
/// INTEGRATION: the runner area owns the runner registry. When it merges, this function should
/// return its runners for the owner (id + `runner` JSON); runner-group membership, listing and
/// validation here then work unchanged. Until then no runner is registered at org/enterprise
/// level, so adding a runner to a group answers 404.
pub async fn registered_runners(s: &AppState, owner_kind: &str, owner_id: i64) -> ApiResult<Vec<RunnerInfo>> {
    // runner area: the registry lives in `actions_runners`.
    Ok(crate::api::runner_runners::runners_for_owner(s, owner_kind, owner_id).await?.into_iter().map(|(id, json)| RunnerInfo { id, json }).collect())
}

// ---------------------------------------------------------------------------------------------
// Storage

#[derive(sqlx::FromRow, Clone)]
pub struct Group {
    pub id: i64,
    pub owner_kind: String,
    pub owner_id: i64,
    pub name: String,
    pub visibility: String,
    pub is_default: bool,
    pub allows_public_repositories: bool,
    pub restricted_to_workflows: bool,
    pub selected_workflows: Vec<String>,
}

const COLS: &str = "id, owner_kind, owner_id, name, visibility, is_default, allows_public_repositories, restricted_to_workflows, selected_workflows";

/// The owner's default group, created on first use.
pub async fn ensure_default(s: &AppState, kind: &str, id: i64) -> ApiResult<Group> {
    sqlx::query("INSERT INTO actions_runner_groups (owner_kind, owner_id, name, visibility, is_default, allows_public_repositories) VALUES ($1, $2, 'Default', 'all', true, true) ON CONFLICT DO NOTHING")
        .bind(kind)
        .bind(id)
        .execute(&s.db)
        .await?;
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM actions_runner_groups WHERE owner_kind = $1 AND owner_id = $2 AND is_default")))
        .bind(kind)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| ApiError::internal("default runner group missing"))
}

async fn groups_of(s: &AppState, kind: &str, id: i64) -> ApiResult<Vec<Group>> {
    ensure_default(s, kind, id).await?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM actions_runner_groups WHERE owner_kind = $1 AND owner_id = $2 ORDER BY is_default DESC, id"
    )))
    .bind(kind)
    .bind(id)
    .fetch_all(&s.db)
    .await?)
}

async fn group_by_id(s: &AppState, gid: i64) -> ApiResult<Option<Group>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM actions_runner_groups WHERE id = $1")))
        .bind(gid)
        .fetch_optional(&s.db)
        .await?)
}

/// A group owned by (kind, id).
async fn own_group(s: &AppState, kind: &str, id: i64, gid: i64) -> ApiResult<Group> {
    ensure_default(s, kind, id).await?;
    group_by_id(s, gid).await?.filter(|g| g.owner_kind == kind && g.owner_id == id).ok_or_else(ApiError::not_found)
}

async fn targets(s: &AppState, gid: i64) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT target_id FROM actions_runner_group_targets WHERE group_id = $1 ORDER BY target_id")
        .bind(gid)
        .fetch_all(&s.db)
        .await?)
}

async fn set_targets(s: &AppState, gid: i64, ids: &[i64]) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM actions_runner_group_targets WHERE group_id = $1").bind(gid).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO actions_runner_group_targets (group_id, target_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(gid)
        .bind(ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Enterprise groups an organization inherits.
async fn inherited_groups(s: &AppState, org_id: i64) -> ApiResult<Vec<Group>> {
    ensure_default(s, "enterprise", 0).await?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM actions_runner_groups g WHERE owner_kind = 'enterprise' AND owner_id = 0
           AND (visibility = 'all' OR EXISTS (SELECT 1 FROM actions_runner_group_targets t WHERE t.group_id = g.id AND t.target_id = $1))
         ORDER BY is_default DESC, id"
    )))
    .bind(org_id)
    .fetch_all(&s.db)
    .await?)
}

/// Which group a runner belongs to: its membership row, else its owner's default group.
pub async fn runner_group_of(s: &AppState, owner_kind: &str, owner_id: i64, runner_id: i64) -> ApiResult<i64> {
    let gid: Option<i64> = sqlx::query_scalar(
        "SELECT m.group_id FROM actions_runner_group_runners m JOIN actions_runner_groups g ON g.id = m.group_id
          WHERE m.runner_id = $1 AND g.owner_kind = $2 AND g.owner_id = $3",
    )
    .bind(runner_id)
    .bind(owner_kind)
    .bind(owner_id)
    .fetch_optional(&s.db)
    .await?;
    match gid {
        Some(g) => Ok(g),
        None => Ok(ensure_default(s, owner_kind, owner_id).await?.id),
    }
}

/// Whether runners of group `group_id` may pick up a job of `repo_id`. `workflow_ref` is the
/// job's workflow as `owner/repo/.github/workflows/file.yml@ref`, checked against
/// `selected_workflows` (globs) when the group is restricted to workflows.
pub async fn runner_group_allows(s: &AppState, group_id: i64, repo_id: i64, workflow_ref: Option<&str>) -> ApiResult<bool> {
    let Some(g) = group_by_id(s, group_id).await? else { return Ok(false) };
    let repo = crate::db::repo_by_id(&s.db, repo_id).await?;
    if !group_serves_repo(s, &g, &repo).await? {
        return Ok(false);
    }
    if g.restricted_to_workflows {
        let Some(wf) = workflow_ref else { return Ok(false) };
        return Ok(g.selected_workflows.iter().any(|p| super::policy::pattern_matches(p, wf) || p.eq_ignore_ascii_case(wf)));
    }
    Ok(true)
}

/// Group visibility for a repository (ignoring workflow restrictions).
async fn group_serves_repo(s: &AppState, g: &Group, repo: &crate::db::Repo) -> ApiResult<bool> {
    let public = !repo.private && repo.visibility == "public";
    if public && !g.allows_public_repositories {
        return Ok(false);
    }
    Ok(match (g.owner_kind.as_str(), g.visibility.as_str()) {
        ("org", _) if repo.owner_id != g.owner_id => false,
        (_, "all") => true,
        ("org", "private") => !public,
        ("org", _) => targets(s, g.id).await?.contains(&repo.id),
        (_, _) => targets(s, g.id).await?.contains(&repo.owner_id),
    })
}

// ---------------------------------------------------------------------------------------------
// JSON

/// Which API the group is shown through.
enum View<'a> {
    Org(&'a User),
    Enterprise(&'a str),
}

fn group_json(s: &AppState, view: &View, g: &Group) -> Value {
    let base = match view {
        View::Org(o) => format!("{}/orgs/{}/actions/runner-groups/{}", s.api_url(), o.login, g.id),
        View::Enterprise(e) => format!("{}/enterprises/{e}/actions/runner-groups/{}", s.api_url(), g.id),
    };
    let mut v = json!({
        "id": g.id,
        "name": g.name,
        "visibility": g.visibility,
        "default": g.is_default,
        "runners_url": format!("{base}/runners"),
        "allows_public_repositories": g.allows_public_repositories,
        "restricted_to_workflows": g.restricted_to_workflows,
        "selected_workflows": g.selected_workflows,
        "workflow_restrictions_read_only": false,
    });
    match view {
        View::Org(_) => {
            let inherited = g.owner_kind == "enterprise";
            v["inherited"] = json!(inherited);
            if inherited {
                v["inherited_allows_public_repositories"] = json!(g.allows_public_repositories);
                v["workflow_restrictions_read_only"] = json!(true);
            } else if g.visibility == "selected" {
                v["selected_repositories_url"] = json!(format!("{base}/repositories"));
            }
        }
        View::Enterprise(_) => {
            if g.visibility == "selected" {
                v["selected_organizations_url"] = json!(format!("{base}/organizations"));
            }
        }
    }
    v
}

fn page<T>(items: Vec<T>, p: PageParams) -> Vec<T> {
    items.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect()
}

fn paged(s: &AppState, uri: &OriginalUri, p: PageParams, key: &str, items: Vec<Value>, total: i64) -> Response {
    let links = crate::api::Paged::new(uri, s, p, Some(total), vec![]).into_response();
    let mut res = (StatusCode::OK, Json(total_envelope(key, items, total))).into_response();
    if let Some(l) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, l.clone());
    }
    res
}

// ---------------------------------------------------------------------------------------------
// Service layer

#[derive(Deserialize, Default)]
pub struct GroupInput {
    pub name: Option<String>,
    pub visibility: Option<String>,
    pub selected_repository_ids: Option<Vec<i64>>,
    pub selected_organization_ids: Option<Vec<i64>>,
    pub runners: Option<Vec<i64>>,
    pub allows_public_repositories: Option<bool>,
    pub restricted_to_workflows: Option<bool>,
    pub selected_workflows: Option<Vec<String>>,
}

fn valid_visibility(kind: &str, v: &str) -> ApiResult<String> {
    let ok = match kind {
        "org" => ["all", "selected", "private"].contains(&v),
        _ => ["all", "selected"].contains(&v),
    };
    if ok { Ok(v.to_string()) } else { Err(ApiError::validation("RunnerGroup", "visibility", "invalid")) }
}

/// Validates target ids for the owner kind (repositories of the org / organizations).
async fn check_targets(s: &AppState, kind: &str, org: Option<&User>, ids: &[i64]) -> ApiResult<Vec<i64>> {
    match (kind, org) {
        ("org", Some(o)) => check_org_repos(s, o, ids, "selected_repository_ids").await,
        _ => check_orgs(s, ids, "selected_organization_ids").await,
    }
}

async fn check_runners(s: &AppState, kind: &str, id: i64, ids: &[i64]) -> ApiResult<Vec<i64>> {
    let known: Vec<i64> = registered_runners(s, kind, id).await?.into_iter().map(|r| r.id).collect();
    if ids.iter().any(|r| !known.contains(r)) {
        return Err(ApiError::validation("RunnerGroup", "runners", "invalid"));
    }
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

async fn assign_runners(s: &AppState, gid: i64, runner_ids: &[i64]) -> ApiResult<()> {
    sqlx::query("INSERT INTO actions_runner_group_runners (runner_id, group_id) SELECT unnest($2::bigint[]), $1 ON CONFLICT (runner_id) DO UPDATE SET group_id = EXCLUDED.group_id")
        .bind(gid)
        .bind(runner_ids)
        .execute(&s.db)
        .await?;
    Ok(())
}

pub async fn create_group(s: &AppState, kind: &str, id: i64, org: Option<&User>, b: GroupInput) -> ApiResult<Group> {
    ensure_default(s, kind, id).await?;
    let name = b.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("RunnerGroup", "name", "missing_field"))?;
    let visibility = valid_visibility(kind, b.visibility.as_deref().unwrap_or("all"))?;
    let target_ids = match if kind == "org" { b.selected_repository_ids } else { b.selected_organization_ids } {
        Some(ids) => check_targets(s, kind, org, &ids).await?,
        None => vec![],
    };
    let runners = check_runners(s, kind, id, &b.runners.unwrap_or_default()).await?;
    let g: Option<Group> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO actions_runner_groups (owner_kind, owner_id, name, visibility, allows_public_repositories, restricted_to_workflows, selected_workflows)
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING RETURNING {COLS}"
    )))
    .bind(kind)
    .bind(id)
    .bind(&name)
    .bind(&visibility)
    .bind(b.allows_public_repositories.unwrap_or(false))
    .bind(b.restricted_to_workflows.unwrap_or(false))
    .bind(b.selected_workflows.unwrap_or_default())
    .fetch_optional(&s.db)
    .await?;
    let g = g.ok_or_else(|| {
        let mut e = ApiError::validation("RunnerGroup", "name", "already_exists");
        e.errors = Some(json!([{ "resource": "RunnerGroup", "field": "name", "code": "already_exists", "message": "A runner group with this name already exists" }]));
        e
    })?;
    set_targets(s, g.id, &target_ids).await?;
    assign_runners(s, g.id, &runners).await?;
    Ok(g)
}

pub async fn update_group(s: &AppState, g: &Group, b: GroupInput) -> ApiResult<Group> {
    let name = match b.name.map(|n| n.trim().to_string()) {
        Some(n) if n.is_empty() => return Err(ApiError::validation("RunnerGroup", "name", "invalid")),
        Some(n) => n,
        None => g.name.clone(),
    };
    let visibility = match b.visibility {
        Some(v) => valid_visibility(&g.owner_kind, &v)?,
        None => g.visibility.clone(),
    };
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE actions_runner_groups SET name = $2, visibility = $3, allows_public_repositories = $4, restricted_to_workflows = $5,
                selected_workflows = $6 WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(g.id)
    .bind(&name)
    .bind(&visibility)
    .bind(b.allows_public_repositories.unwrap_or(g.allows_public_repositories))
    .bind(b.restricted_to_workflows.unwrap_or(g.restricted_to_workflows))
    .bind(b.selected_workflows.unwrap_or_else(|| g.selected_workflows.clone()))
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("RunnerGroup", "name", "already_exists"),
        e => e.into(),
    })
}

pub async fn delete_group(s: &AppState, g: &Group) -> ApiResult<()> {
    if g.is_default {
        return Err(ApiError::unprocessable("The default runner group cannot be deleted."));
    }
    // Member runners fall back to the default group (their membership rows cascade away).
    sqlx::query("DELETE FROM actions_runner_groups WHERE id = $1").bind(g.id).execute(&s.db).await?;
    Ok(())
}

/// Runners in a group, with `runner_group_id` filled in.
pub async fn group_runners(s: &AppState, g: &Group) -> ApiResult<Vec<Value>> {
    let members: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT m.runner_id, m.group_id FROM actions_runner_group_runners m JOIN actions_runner_groups g ON g.id = m.group_id
          WHERE g.owner_kind = $1 AND g.owner_id = $2",
    )
    .bind(&g.owner_kind)
    .bind(g.owner_id)
    .fetch_all(&s.db)
    .await?;
    let mut out = vec![];
    for r in registered_runners(s, &g.owner_kind, g.owner_id).await? {
        let gid = members.iter().find(|(id, _)| *id == r.id).map(|(_, g)| *g);
        if gid == Some(g.id) || (gid.is_none() && g.is_default) {
            let mut v = r.json;
            v["runner_group_id"] = json!(g.id);
            out.push(v);
        }
    }
    Ok(out)
}

pub async fn set_group_runners(s: &AppState, g: &Group, ids: &[i64]) -> ApiResult<()> {
    let ids = check_runners(s, &g.owner_kind, g.owner_id, ids).await?;
    sqlx::query("DELETE FROM actions_runner_group_runners WHERE group_id = $1").bind(g.id).execute(&s.db).await?;
    assign_runners(s, g.id, &ids).await
}

pub async fn add_group_runner(s: &AppState, g: &Group, runner_id: i64) -> ApiResult<()> {
    check_runners(s, &g.owner_kind, g.owner_id, &[runner_id]).await.map_err(|_| ApiError::not_found())?;
    assign_runners(s, g.id, &[runner_id]).await
}

/// Removes a runner from a group; it goes back to the default group.
pub async fn remove_group_runner(s: &AppState, g: &Group, runner_id: i64) -> ApiResult<()> {
    check_runners(s, &g.owner_kind, g.owner_id, &[runner_id]).await.map_err(|_| ApiError::not_found())?;
    sqlx::query("DELETE FROM actions_runner_group_runners WHERE group_id = $1 AND runner_id = $2").bind(g.id).bind(runner_id).execute(&s.db).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Handlers: organizations

pub fn routes() -> Router<AppState> {
    let o = "/orgs/{org}/actions/runner-groups";
    Router::new()
        .route(o, get(o_list).post(o_create))
        .route(&format!("{o}/{{group_id}}"), get(o_get).patch(o_update).delete(o_delete))
        .route(&format!("{o}/{{group_id}}/repositories"), get(o_repos).put(o_set_repos))
        .route(&format!("{o}/{{group_id}}/repositories/{{repository_id}}"), put(o_add_repo).delete(o_remove_repo))
        .route(&format!("{o}/{{group_id}}/runners"), get(o_runners).put(o_set_runners))
        .route(&format!("{o}/{{group_id}}/runners/{{runner_id}}"), put(o_add_runner).delete(o_remove_runner))
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    visible_to_repository: Option<String>,
}

async fn o_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let mut groups = groups_of(&s, "org", o.id).await?;
    groups.extend(inherited_groups(&s, o.id).await?);
    if let Some(name) = q.visible_to_repository.filter(|n| !n.is_empty()) {
        let repo = crate::db::load_repo(&s.db, Some(&u), &o.login, &name).await?;
        let mut keep = vec![];
        for g in groups {
            if group_serves_repo(&s, &g, &repo.repo).await? {
                keep.push(g);
            }
        }
        groups = keep;
    }
    let total = groups.len() as i64;
    let items = page(groups, p).iter().map(|g| group_json(&s, &View::Org(&o), g)).collect();
    Ok(paged(&s, &uri, p, "runner_groups", items, total))
}

async fn o_create(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<GroupInput>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let g = create_group(&s, "org", o.id, Some(&o), b).await?;
    Ok((StatusCode::CREATED, Json(group_json(&s, &View::Org(&o), &g))).into_response())
}

/// An org group or a group the org inherits (read-only).
async fn o_visible_group(s: &AppState, o: &User, gid: i64) -> ApiResult<Group> {
    if let Ok(g) = own_group(s, "org", o.id, gid).await {
        return Ok(g);
    }
    inherited_groups(s, o.id).await?.into_iter().find(|g| g.id == gid).ok_or_else(ApiError::not_found)
}

async fn o_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let g = o_visible_group(&s, &o, gid).await?;
    Ok(Json(group_json(&s, &View::Org(&o), &g)).into_response())
}

async fn o_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid)): Path<(String, i64)>, Json(b): Json<GroupInput>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let g = own_group(&s, "org", o.id, gid).await?;
    let g = update_group(&s, &g, b).await?;
    Ok(Json(group_json(&s, &View::Org(&o), &g)).into_response())
}

async fn o_delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    delete_group(&s, &own_group(&s, "org", o.id, gid).await?).await?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct ReposBody {
    selected_repository_ids: Option<Vec<i64>>,
}
#[derive(Deserialize)]
struct OrgsBody {
    selected_organization_ids: Option<Vec<i64>>,
}
#[derive(Deserialize)]
struct RunnersBody {
    runners: Option<Vec<i64>>,
}

async fn o_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((org, gid)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let g = own_group(&s, "org", o.id, gid).await?;
    let all = org_repos_json(&s, &o, &targets(&s, g.id).await?).await?;
    let total = all.len() as i64;
    Ok(paged(&s, &uri, p, "repositories", page(all, p), total))
}
async fn o_set_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid)): Path<(String, i64)>, Json(b): Json<ReposBody>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let g = own_group(&s, "org", o.id, gid).await?;
    let ids = b.selected_repository_ids.ok_or_else(|| ApiError::validation("RunnerGroup", "selected_repository_ids", "missing_field"))?;
    set_targets(&s, g.id, &check_org_repos(&s, &o, &ids, "selected_repository_ids").await?).await?;
    Ok(no_content())
}
async fn o_toggle_repo(s: &AppState, u: &User, org: &str, gid: i64, repo_id: i64, add: bool) -> ApiResult<Response> {
    let o = org_admin(s, u, org).await?;
    let g = own_group(s, "org", o.id, gid).await?;
    check_org_repos(s, &o, &[repo_id], "repository_id").await.map_err(|_| ApiError::not_found())?;
    let q = if add {
        "INSERT INTO actions_runner_group_targets (group_id, target_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    } else {
        "DELETE FROM actions_runner_group_targets WHERE group_id = $1 AND target_id = $2"
    };
    sqlx::query(q).bind(g.id).bind(repo_id).execute(&s.db).await?;
    Ok(no_content())
}
async fn o_add_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid, id)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    o_toggle_repo(&s, &u, &org, gid, id, true).await
}
async fn o_remove_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid, id)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    o_toggle_repo(&s, &u, &org, gid, id, false).await
}
async fn o_runners(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((org, gid)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let g = own_group(&s, "org", o.id, gid).await?;
    let all = group_runners(&s, &g).await?;
    let total = all.len() as i64;
    Ok(paged(&s, &uri, p, "runners", page(all, p), total))
}
async fn o_set_runners(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid)): Path<(String, i64)>, Json(b): Json<RunnersBody>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    let g = own_group(&s, "org", o.id, gid).await?;
    let ids = b.runners.ok_or_else(|| ApiError::validation("RunnerGroup", "runners", "missing_field"))?;
    set_group_runners(&s, &g, &ids).await?;
    Ok(no_content())
}
async fn o_add_runner(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid, rid)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    add_group_runner(&s, &own_group(&s, "org", o.id, gid).await?, rid).await?;
    Ok(no_content())
}
async fn o_remove_runner(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, gid, rid)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    let o = org_admin(&s, &u, &org).await?;
    remove_group_runner(&s, &own_group(&s, "org", o.id, gid).await?, rid).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Handlers: enterprise

pub fn enterprise_routes() -> Router<AppState> {
    let e = "/enterprises/{enterprise}/actions/runner-groups";
    Router::new()
        .route(e, get(e_list).post(e_create))
        .route(&format!("{e}/{{group_id}}"), get(e_get).patch(e_update).delete(e_delete))
        .route(&format!("{e}/{{group_id}}/organizations"), get(e_orgs).put(e_set_orgs))
        .route(&format!("{e}/{{group_id}}/organizations/{{org_id}}"), put(e_add_org).delete(e_remove_org))
        .route(&format!("{e}/{{group_id}}/runners"), get(e_runners).put(e_set_runners))
        .route(&format!("{e}/{{group_id}}/runners/{{runner_id}}"), put(e_add_runner).delete(e_remove_runner))
}

async fn e_group(s: &AppState, u: &User, e: &str, gid: i64) -> ApiResult<Group> {
    enterprise_admin(u, e)?;
    own_group(s, "enterprise", 0, gid).await
}

async fn e_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(e): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Response> {
    enterprise_admin(&u, &e)?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let groups = groups_of(&s, "enterprise", 0).await?;
    let total = groups.len() as i64;
    let items = page(groups, p).iter().map(|g| group_json(&s, &View::Enterprise(&e), g)).collect();
    Ok(paged(&s, &uri, p, "runner_groups", items, total))
}
async fn e_create(State(s): State<AppState>, AuthUser(u): AuthUser, Path(e): Path<String>, Json(b): Json<GroupInput>) -> ApiResult<Response> {
    enterprise_admin(&u, &e)?;
    let g = create_group(&s, "enterprise", 0, None, b).await?;
    Ok((StatusCode::CREATED, Json(group_json(&s, &View::Enterprise(&e), &g))).into_response())
}
async fn e_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid)): Path<(String, i64)>) -> ApiResult<Response> {
    let g = e_group(&s, &u, &e, gid).await?;
    Ok(Json(group_json(&s, &View::Enterprise(&e), &g)).into_response())
}
async fn e_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid)): Path<(String, i64)>, Json(b): Json<GroupInput>) -> ApiResult<Response> {
    let g = update_group(&s, &e_group(&s, &u, &e, gid).await?, b).await?;
    Ok(Json(group_json(&s, &View::Enterprise(&e), &g)).into_response())
}
async fn e_delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid)): Path<(String, i64)>) -> ApiResult<Response> {
    delete_group(&s, &e_group(&s, &u, &e, gid).await?).await?;
    Ok(no_content())
}
async fn e_orgs(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((e, gid)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let g = e_group(&s, &u, &e, gid).await?;
    let all = orgs_json(&s, &targets(&s, g.id).await?).await?;
    let total = all.len() as i64;
    Ok(paged(&s, &uri, p, "organizations", page(all, p), total))
}
async fn e_set_orgs(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid)): Path<(String, i64)>, Json(b): Json<OrgsBody>) -> ApiResult<Response> {
    let g = e_group(&s, &u, &e, gid).await?;
    let ids = b.selected_organization_ids.ok_or_else(|| ApiError::validation("RunnerGroup", "selected_organization_ids", "missing_field"))?;
    set_targets(&s, g.id, &check_orgs(&s, &ids, "selected_organization_ids").await?).await?;
    Ok(no_content())
}
async fn e_toggle_org(s: &AppState, u: &User, e: &str, gid: i64, org_id: i64, add: bool) -> ApiResult<Response> {
    let g = e_group(s, u, e, gid).await?;
    check_orgs(s, &[org_id], "org_id").await.map_err(|_| ApiError::not_found())?;
    let q = if add {
        "INSERT INTO actions_runner_group_targets (group_id, target_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
    } else {
        "DELETE FROM actions_runner_group_targets WHERE group_id = $1 AND target_id = $2"
    };
    sqlx::query(q).bind(g.id).bind(org_id).execute(&s.db).await?;
    Ok(no_content())
}
async fn e_add_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid, id)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    e_toggle_org(&s, &u, &e, gid, id, true).await
}
async fn e_remove_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid, id)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    e_toggle_org(&s, &u, &e, gid, id, false).await
}
async fn e_runners(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((e, gid)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let g = e_group(&s, &u, &e, gid).await?;
    let all = group_runners(&s, &g).await?;
    let total = all.len() as i64;
    Ok(paged(&s, &uri, p, "runners", page(all, p), total))
}
async fn e_set_runners(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid)): Path<(String, i64)>, Json(b): Json<RunnersBody>) -> ApiResult<Response> {
    let g = e_group(&s, &u, &e, gid).await?;
    let ids = b.runners.ok_or_else(|| ApiError::validation("RunnerGroup", "runners", "missing_field"))?;
    set_group_runners(&s, &g, &ids).await?;
    Ok(no_content())
}
async fn e_add_runner(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid, rid)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    add_group_runner(&s, &e_group(&s, &u, &e, gid).await?, rid).await?;
    Ok(no_content())
}
async fn e_remove_runner(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, gid, rid)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    remove_group_runner(&s, &e_group(&s, &u, &e, gid).await?, rid).await?;
    Ok(no_content())
}
