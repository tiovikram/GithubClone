//! REST packages API (`/user/packages`, `/users/{username}/packages`, `/orgs/{org}/packages`):
//! packages and versions published through forge's container and npm registries
//! (`crate::packages`). Deletes are soft and restorable for 30 days.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use super::{no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::packages::{self as pk, PkgPerm};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/docker/conflicts", get(conflicts_me))
        .route("/users/{username}/docker/conflicts", get(conflicts_user))
        .route("/orgs/{org}/docker/conflicts", get(conflicts_org))
        .route("/user/packages", get(list_me))
        .route("/users/{username}/packages", get(list_user))
        .route("/orgs/{org}/packages", get(list_org))
        .route("/user/packages/{package_type}/{package_name}", get(get_me).delete(delete_me))
        .route("/users/{username}/packages/{package_type}/{package_name}", get(get_user).delete(delete_user))
        .route("/orgs/{org}/packages/{package_type}/{package_name}", get(get_org).delete(delete_org))
        .route("/user/packages/{package_type}/{package_name}/restore", post(restore_me))
        .route("/users/{username}/packages/{package_type}/{package_name}/restore", post(restore_user))
        .route("/orgs/{org}/packages/{package_type}/{package_name}/restore", post(restore_org))
        .route("/user/packages/{package_type}/{package_name}/versions", get(versions_me))
        .route("/users/{username}/packages/{package_type}/{package_name}/versions", get(versions_user))
        .route("/orgs/{org}/packages/{package_type}/{package_name}/versions", get(versions_org))
        .route("/user/packages/{package_type}/{package_name}/versions/{version_id}", get(version_me).delete(delete_version_me))
        .route("/users/{username}/packages/{package_type}/{package_name}/versions/{version_id}", get(version_user).delete(delete_version_user))
        .route("/orgs/{org}/packages/{package_type}/{package_name}/versions/{version_id}", get(version_org).delete(delete_version_org))
        .route("/user/packages/{package_type}/{package_name}/versions/{version_id}/restore", post(restore_version_me))
        .route("/users/{username}/packages/{package_type}/{package_name}/versions/{version_id}/restore", post(restore_version_user))
        .route("/orgs/{org}/packages/{package_type}/{package_name}/versions/{version_id}/restore", post(restore_version_org))
}

/// Whose namespace a request addresses.
enum Ns {
    Me,
    User(String),
    Org(String),
}

async fn owner(s: &AppState, viewer: Option<&User>, ns: &Ns) -> ApiResult<User> {
    match ns {
        Ns::Me => viewer.cloned().ok_or_else(ApiError::unauthorized),
        Ns::User(login) => {
            let u = db::user_by_login(&s.db, login).await?;
            if u.kind == "Organization" {
                return Err(ApiError::not_found());
            }
            Ok(u)
        }
        Ns::Org(login) => crate::orgs::org_by_login(s, login).await,
    }
}

fn check_type(t: &str) -> ApiResult<()> {
    if pk::TYPES.contains(&t) {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

#[derive(Deserialize)]
struct ListQuery {
    package_type: Option<String>,
    visibility: Option<String>,
}

async fn list(s: AppState, uri: OriginalUri, viewer: Option<User>, ns: Ns, q: ListQuery, page: PageParams) -> ApiResult<Response> {
    let o = owner(&s, viewer.as_ref(), &ns).await?;
    let ty = q.package_type.as_deref().unwrap_or("");
    if !pk::TYPES.contains(&ty) {
        return Err(ApiError::bad_request(format!(
            "package_type must be one of: {}",
            pk::TYPES.join(", ")
        )));
    }
    let vis = match q.visibility.as_deref() {
        None | Some("") => None,
        Some(v @ ("public" | "private" | "internal")) => Some(v),
        Some(_) => return Err(ApiError::bad_request("visibility must be one of: public, private, internal")),
    };
    // `docker` packages lived in GitHub's legacy Docker registry; forge's images are `container`.
    let rows = pk::list_for_owner(&s, viewer.as_ref(), &o, ty, vis).await?;
    let total = rows.len() as i64;
    let slice: Vec<_> = rows.into_iter().skip(page.offset() as usize).take(page.limit() as usize).collect();
    let items = pk::packages_json(&s, viewer.as_ref(), &o, &slice).await?;
    Ok(Paged::new(&uri, &s, page, Some(total), items).into_response())
}

async fn list_me(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<ListQuery>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    list(s, uri, Some(u), Ns::Me, q, p).await
}
async fn list_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    viewer: MaybeUser,
    Path(username): Path<String>,
    Query(q): Query<ListQuery>,
    Query(p): Query<PageParams>,
) -> ApiResult<Response> {
    list(s, uri, viewer.0, Ns::User(username), q, p).await
}
async fn list_org(State(s): State<AppState>, uri: OriginalUri, viewer: MaybeUser, Path(org): Path<String>, Query(q): Query<ListQuery>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    list(s, uri, viewer.0, Ns::Org(org), q, p).await
}

async fn get_pkg(s: AppState, viewer: Option<User>, ns: Ns, ty: String, name: String) -> ApiResult<Json<Value>> {
    check_type(&ty)?;
    let o = owner(&s, viewer.as_ref(), &ns).await?;
    let (p, _) = pk::get_readable(&s, viewer.as_ref(), &o, &ty, &name).await?;
    let mut v = pk::packages_json(&s, viewer.as_ref(), &o, std::slice::from_ref(&p)).await?;
    Ok(Json(v.remove(0)))
}

async fn get_me(State(s): State<AppState>, AuthUser(u): AuthUser, Path((ty, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    get_pkg(s, Some(u), Ns::Me, ty, name).await
}
async fn get_user(State(s): State<AppState>, viewer: MaybeUser, Path((login, ty, name)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    get_pkg(s, viewer.0, Ns::User(login), ty, name).await
}
async fn get_org(State(s): State<AppState>, viewer: MaybeUser, Path((org, ty, name)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    get_pkg(s, viewer.0, Ns::Org(org), ty, name).await
}

async fn delete_pkg(s: AppState, actor: User, ns: Ns, ty: String, name: String) -> ApiResult<Response> {
    check_type(&ty)?;
    let o = owner(&s, Some(&actor), &ns).await?;
    pk::delete_package(&s, &actor, &o, &ty, &name).await?;
    Ok(no_content())
}

async fn delete_me(State(s): State<AppState>, AuthUser(u): AuthUser, Path((ty, name)): Path<(String, String)>) -> ApiResult<Response> {
    delete_pkg(s, u, Ns::Me, ty, name).await
}
async fn delete_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((login, ty, name)): Path<(String, String, String)>) -> ApiResult<Response> {
    delete_pkg(s, u, Ns::User(login), ty, name).await
}
async fn delete_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, ty, name)): Path<(String, String, String)>) -> ApiResult<Response> {
    delete_pkg(s, u, Ns::Org(org), ty, name).await
}

async fn restore_pkg(s: AppState, actor: User, ns: Ns, ty: String, name: String) -> ApiResult<Response> {
    check_type(&ty)?;
    let o = owner(&s, Some(&actor), &ns).await?;
    pk::restore_package(&s, &actor, &o, &ty, &name).await?;
    Ok(no_content())
}

async fn restore_me(State(s): State<AppState>, AuthUser(u): AuthUser, Path((ty, name)): Path<(String, String)>) -> ApiResult<Response> {
    restore_pkg(s, u, Ns::Me, ty, name).await
}
async fn restore_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((login, ty, name)): Path<(String, String, String)>) -> ApiResult<Response> {
    restore_pkg(s, u, Ns::User(login), ty, name).await
}
async fn restore_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, ty, name)): Path<(String, String, String)>) -> ApiResult<Response> {
    restore_pkg(s, u, Ns::Org(org), ty, name).await
}

#[derive(Deserialize)]
struct VersionsQuery {
    state: Option<String>,
}

async fn list_versions(s: AppState, uri: OriginalUri, viewer: Option<User>, ns: Ns, ty: String, name: String, q: VersionsQuery, page: PageParams) -> ApiResult<Response> {
    check_type(&ty)?;
    let o = owner(&s, viewer.as_ref(), &ns).await?;
    let (p, perm) = pk::get_readable(&s, viewer.as_ref(), &o, &ty, &name).await?;
    let deleted = match q.state.as_deref() {
        None | Some("") | Some("active") => false,
        Some("deleted") => true,
        Some(_) => return Err(ApiError::validation("PackageVersion", "state", "invalid")),
    };
    if deleted {
        // Deleted versions are an administrator's business.
        pk::require(perm, PkgPerm::Admin)?;
    }
    let rows = pk::versions(&s, p.id, deleted).await?;
    let total = rows.len() as i64;
    let slice: Vec<_> = rows.into_iter().skip(page.offset() as usize).take(page.limit() as usize).collect();
    let tags = pk::tags_by_version(&s, &slice.iter().map(|v| v.id).collect::<Vec<_>>()).await?;
    let items = slice.iter().map(|v| pk::version_json(&s, &o, &p, v, tags.get(&v.id).map(|t| t.as_slice()).unwrap_or(&[]))).collect();
    Ok(Paged::new(&uri, &s, page, Some(total), items).into_response())
}

async fn versions_me(
    State(s): State<AppState>,
    uri: OriginalUri,
    AuthUser(u): AuthUser,
    Path((ty, name)): Path<(String, String)>,
    Query(q): Query<VersionsQuery>,
    Query(p): Query<PageParams>,
) -> ApiResult<Response> {
    list_versions(s, uri, Some(u), Ns::Me, ty, name, q, p).await
}
async fn versions_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    viewer: MaybeUser,
    Path((login, ty, name)): Path<(String, String, String)>,
    Query(q): Query<VersionsQuery>,
    Query(p): Query<PageParams>,
) -> ApiResult<Response> {
    list_versions(s, uri, viewer.0, Ns::User(login), ty, name, q, p).await
}
async fn versions_org(
    State(s): State<AppState>,
    uri: OriginalUri,
    viewer: MaybeUser,
    Path((org, ty, name)): Path<(String, String, String)>,
    Query(q): Query<VersionsQuery>,
    Query(p): Query<PageParams>,
) -> ApiResult<Response> {
    list_versions(s, uri, viewer.0, Ns::Org(org), ty, name, q, p).await
}

async fn get_version(s: AppState, viewer: Option<User>, ns: Ns, ty: String, name: String, id: i64) -> ApiResult<Json<Value>> {
    check_type(&ty)?;
    let o = owner(&s, viewer.as_ref(), &ns).await?;
    let (p, perm) = pk::get_readable(&s, viewer.as_ref(), &o, &ty, &name).await?;
    let v = pk::version_by_id(&s, p.id, id).await?;
    if v.deleted_at.is_some() && perm < PkgPerm::Admin {
        return Err(ApiError::not_found());
    }
    let tags = pk::tags_by_version(&s, &[v.id]).await?;
    Ok(Json(pk::version_json(&s, &o, &p, &v, tags.get(&v.id).map(|t| t.as_slice()).unwrap_or(&[]))))
}

async fn version_me(State(s): State<AppState>, AuthUser(u): AuthUser, Path((ty, name, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    get_version(s, Some(u), Ns::Me, ty, name, id).await
}
async fn version_user(State(s): State<AppState>, viewer: MaybeUser, Path((login, ty, name, id)): Path<(String, String, String, i64)>) -> ApiResult<Json<Value>> {
    get_version(s, viewer.0, Ns::User(login), ty, name, id).await
}
async fn version_org(State(s): State<AppState>, viewer: MaybeUser, Path((org, ty, name, id)): Path<(String, String, String, i64)>) -> ApiResult<Json<Value>> {
    get_version(s, viewer.0, Ns::Org(org), ty, name, id).await
}

async fn version_op(s: AppState, actor: User, ns: Ns, ty: String, name: String, id: i64, restore: bool) -> ApiResult<Response> {
    check_type(&ty)?;
    let o = owner(&s, Some(&actor), &ns).await?;
    let (p, perm) = pk::get_readable(&s, Some(&actor), &o, &ty, &name).await?;
    if restore {
        pk::restore_version(&s, &p, perm, id).await?;
    } else {
        pk::delete_version(&s, &actor, &p, perm, id).await?;
    }
    Ok(no_content())
}

async fn delete_version_me(State(s): State<AppState>, AuthUser(u): AuthUser, Path((ty, name, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    version_op(s, u, Ns::Me, ty, name, id, false).await
}
async fn delete_version_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((login, ty, name, id)): Path<(String, String, String, i64)>) -> ApiResult<Response> {
    version_op(s, u, Ns::User(login), ty, name, id, false).await
}
async fn delete_version_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, ty, name, id)): Path<(String, String, String, i64)>) -> ApiResult<Response> {
    version_op(s, u, Ns::Org(org), ty, name, id, false).await
}
async fn restore_version_me(State(s): State<AppState>, AuthUser(u): AuthUser, Path((ty, name, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    version_op(s, u, Ns::Me, ty, name, id, true).await
}
async fn restore_version_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((login, ty, name, id)): Path<(String, String, String, i64)>) -> ApiResult<Response> {
    version_op(s, u, Ns::User(login), ty, name, id, true).await
}
async fn restore_version_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, ty, name, id)): Path<(String, String, String, i64)>) -> ApiResult<Response> {
    version_op(s, u, Ns::Org(org), ty, name, id, true).await
}

/// Packages whose Docker-registry migration conflicts with an existing package. forge never ran
/// GitHub's legacy Docker registry (`docker.pkg.github.com`), so there are none to migrate.
async fn conflicts(s: AppState, viewer: Option<User>, ns: Ns) -> ApiResult<Json<Value>> {
    owner(&s, viewer.as_ref(), &ns).await?;
    Ok(Json(Value::Array(vec![])))
}

async fn conflicts_me(State(s): State<AppState>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    conflicts(s, Some(u), Ns::Me).await
}
async fn conflicts_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path(login): Path<String>) -> ApiResult<Json<Value>> {
    conflicts(s, Some(u), Ns::User(login)).await
}
async fn conflicts_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    conflicts(s, Some(u), Ns::Org(org)).await
}
