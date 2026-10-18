//! Enterprise-level management of GitHub App installations on organizations (GHES
//! `enterprise-apps`): enterprise owners (site administrators) install an app (by client id) on
//! any organization of the enterprise and manage its repository access.
//! forge models a single enterprise, `FORGE_ENTERPRISE_SLUG` (default `forge`).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::apps::{self, InstallTarget, InstallationRow};
use crate::auth::AuthUser;
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let e = "/enterprises/{enterprise}/apps";
    Router::new()
        .route(&format!("{e}/installable_organizations"), get(installable_orgs))
        .route(&format!("{e}/installable_organizations/{{org}}/accessible_repositories"), get(accessible_repos))
        .route(&format!("{e}/organizations/{{org}}/installations"), get(list).post(create))
        .route(&format!("{e}/organizations/{{org}}/installations/{{installation_id}}"), delete(remove))
        .route(&format!("{e}/organizations/{{org}}/installations/{{installation_id}}/repositories"), get(repos).patch(set_repos))
        .route(&format!("{e}/organizations/{{org}}/installations/{{installation_id}}/repositories/add"), patch(add_repos))
        .route(&format!("{e}/organizations/{{org}}/installations/{{installation_id}}/repositories/remove"), patch(remove_repos))
}

fn check(enterprise: &str, u: &User) -> ApiResult<()> {
    if !enterprise.eq_ignore_ascii_case(&apps::enterprise_slug()) {
        return Err(ApiError::not_found());
    }
    if !u.site_admin {
        return Err(ApiError::forbidden("You must be an enterprise owner to access this resource."));
    }
    Ok(())
}

fn accessible(ctx: &RepoCtx) -> Value {
    json!({ "id": ctx.repo.id, "name": ctx.repo.name, "full_name": ctx.full_name() })
}

async fn installable_orgs(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(e): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    check(&e, &u)?;
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, login FROM users WHERE kind = 'Organization' ORDER BY id LIMIT $1 OFFSET $2")
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'Organization'").fetch_one(&s.db).await?;
    let api = s.api_url();
    let items = rows
        .iter()
        .map(|(id, login)| json!({ "id": id, "login": login, "accessible_repositories_url": format!("{api}/enterprises/{e}/apps/installable_organizations/{login}/accessible_repositories") }))
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn org_repo_page(s: &AppState, org: &User, ids: Option<&[i64]>, p: PageParams) -> ApiResult<(i64, Vec<Value>)> {
    let all: Vec<i64> = match ids {
        Some(ids) => ids.to_vec(),
        None => sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1 ORDER BY id").bind(org.id).fetch_all(&s.db).await?,
    };
    let page: Vec<i64> = all.iter().skip(p.offset() as usize).take(p.limit() as usize).copied().collect();
    let repos = apps::repos_by_ids(&s.db, &page, crate::db::Perm::Admin).await?;
    Ok((all.len() as i64, repos.iter().map(accessible).collect()))
}

async fn accessible_repos(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((e, org)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    check(&e, &u)?;
    let o = crate::orgs::org_by_login(&s, &org).await?;
    let (total, items) = org_repo_page(&s, &o, None, p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

fn ent_installation(s: &AppState, inst: &InstallationRow, app: &apps::AppRow, org: &str) -> Value {
    json!({
        "id": inst.id,
        "app_slug": app.slug,
        "client_id": app.client_id,
        "repository_selection": inst.repository_selection,
        "repositories_url": format!("{}/enterprises/{}/apps/organizations/{org}/installations/{}/repositories", s.api_url(), apps::enterprise_slug(), inst.id),
        "permissions": inst.permissions,
        "events": inst.events,
        "created_at": crate::json::ts(inst.created_at),
        "updated_at": crate::json::ts(inst.updated_at),
    })
}

async fn list(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((e, org)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    check(&e, &u)?;
    let o = crate::orgs::org_by_login(&s, &org).await?;
    let rows: Vec<InstallationRow> = sqlx::query_as("SELECT * FROM app_installations WHERE target_id = $1 AND app_id NOT IN (SELECT id FROM apps WHERE slug = 'github-actions') ORDER BY id LIMIT $2 OFFSET $3")
        .bind(o.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM app_installations WHERE target_id = $1 AND app_id NOT IN (SELECT id FROM apps WHERE slug = 'github-actions')").bind(o.id).fetch_one(&s.db).await?;
    let mut items = Vec::new();
    for r in &rows {
        let app = apps::app_by_id(&s.db, r.app_id).await?;
        items.push(ent_installation(&s, r, &app, &o.login));
    }
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize, Default)]
struct CreateBody {
    client_id: Option<String>,
    repository_selection: Option<String>,
    repositories: Option<Vec<String>>,
}

async fn names_to_ids(s: &AppState, org: &User, names: &[String]) -> ApiResult<Vec<i64>> {
    match apps::resolve_repo_names(&s.db, org.id, names).await? {
        Ok(ids) => Ok(ids),
        Err(n) => {
            let mut e = ApiError::validation("Installation", "repositories", "invalid");
            e.errors = Some(json!([{"resource": "Installation", "field": "repositories", "code": "invalid", "message": format!("Repository {n} was not found in the organization.")}]));
            Err(e)
        }
    }
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, org)): Path<(String, String)>, Json(b): Json<CreateBody>) -> ApiResult<Response> {
    check(&e, &u)?;
    let o = crate::orgs::org_by_login(&s, &org).await?;
    let cid = b.client_id.filter(|c| !c.is_empty()).ok_or_else(|| ApiError::validation("Installation", "client_id", "missing_field"))?;
    let app = apps::app_by_client_id(&s.db, &cid).await.map_err(|_| ApiError::validation("Installation", "client_id", "invalid"))?;
    let sel = b.repository_selection.ok_or_else(|| ApiError::validation("Installation", "repository_selection", "missing_field"))?;
    let (selection, ids) = match sel.as_str() {
        "all" => ("all", vec![]),
        "none" => ("selected", vec![]),
        "selected" => ("selected", names_to_ids(&s, &o, &b.repositories.unwrap_or_default()).await?),
        _ => return Err(ApiError::validation("Installation", "repository_selection", "invalid")),
    };
    let (inst, created) = apps::install(&s, &u, &app, &InstallTarget::Account(o), selection, &ids, true).await?;
    let v = apps::installation_json_with(&s, &inst, &app).await?;
    Ok((if created { StatusCode::CREATED } else { StatusCode::OK }, axum::Json(v)).into_response())
}

async fn org_installation(s: &AppState, u: &User, e: &str, org: &str, id: i64) -> ApiResult<(User, InstallationRow, apps::AppRow)> {
    check(e, u)?;
    let o = crate::orgs::org_by_login(s, org).await?;
    let inst = apps::installation_by_id(&s.db, id).await?;
    if inst.target_id != Some(o.id) {
        return Err(ApiError::not_found());
    }
    let app = apps::app_by_id(&s.db, inst.app_id).await?;
    Ok((o, inst, app))
}

async fn remove(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, org, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let (_, inst, app) = org_installation(&s, &u, &e, &org, id).await?;
    apps::uninstall(&s, &u, &app, &inst).await?;
    Ok(no_content())
}

async fn repos(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((e, org, id)): Path<(String, String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let (o, inst, _) = org_installation(&s, &u, &e, &org, id).await?;
    let ids = apps::installation_repo_ids(&s.db, &inst).await?;
    let (total, items) = org_repo_page(&s, &o, Some(&ids), p).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

#[derive(Deserialize, Default)]
struct SelectionBody {
    repository_selection: Option<String>,
    repositories: Option<Vec<String>>,
}

async fn set_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, org, id)): Path<(String, String, i64)>, Json(b): Json<SelectionBody>) -> ApiResult<Json<Value>> {
    let (o, inst, app) = org_installation(&s, &u, &e, &org, id).await?;
    let sel = b.repository_selection.ok_or_else(|| ApiError::validation("Installation", "repository_selection", "missing_field"))?;
    let ids = match sel.as_str() {
        "all" => vec![],
        "selected" => names_to_ids(&s, &o, &b.repositories.unwrap_or_default()).await?,
        _ => return Err(ApiError::validation("Installation", "repository_selection", "invalid")),
    };
    let inst = apps::set_repositories(&s, &u, &app, &inst, &sel, &ids).await?;
    Ok(Json(apps::installation_json_with(&s, &inst, &app).await?))
}

#[derive(Deserialize, Default)]
struct ReposBody {
    repositories: Option<Vec<String>>,
}

async fn change(s: &AppState, u: &User, e: &str, org: &str, id: i64, b: ReposBody, add: bool) -> ApiResult<Json<Value>> {
    let (o, inst, app) = org_installation(s, u, e, org, id).await?;
    let names = b.repositories.filter(|r| !r.is_empty()).ok_or_else(|| ApiError::validation("Installation", "repositories", "missing_field"))?;
    if inst.repository_selection == "all" {
        return Err(ApiError::unprocessable("The installation has access to all repositories; change the repository selection first."));
    }
    let ids = names_to_ids(s, &o, &names).await?;
    let inst = if add {
        apps::change_repositories(s, u, &app, &inst, &ids, &[]).await?
    } else {
        apps::change_repositories(s, u, &app, &inst, &[], &ids).await?
    };
    let ids = apps::installation_repo_ids(&s.db, &inst).await?;
    let repos = apps::repos_by_ids(&s.db, &ids, crate::db::Perm::Admin).await?;
    Ok(Json(Value::Array(repos.iter().map(accessible).collect())))
}

async fn add_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, org, id)): Path<(String, String, i64)>, Json(b): Json<ReposBody>) -> ApiResult<Json<Value>> {
    change(&s, &u, &e, &org, id, b, true).await
}

async fn remove_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((e, org, id)): Path<(String, String, i64)>, Json(b): Json<ReposBody>) -> ApiResult<Json<Value>> {
    change(&s, &u, &e, &org, id, b, false).await
}
