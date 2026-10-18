//! Organization settings and administration: update/delete, member privileges and the base
//! repository permission, announcement banner, immutable-release policy, org-wide security
//! product enablement, outside collaborators, audit log, app installations, fine-grained PAT
//! policy endpoints, push-rule bypass requests and org ruleset history.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::orgs::{self, audit_quiet, org_by_login, org_role, require_capability, require_member, require_owner};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let o = "/orgs/{org}";
    Router::new()
        .route(o, axum::routing::patch(update_org).delete(delete_org))
        .route(&format!("{o}/announcement"), get(get_announcement).patch(set_announcement).delete(remove_announcement))
        .route(&format!("{o}/settings/immutable-releases"), get(get_immutable).put(set_immutable))
        .route(&format!("{o}/settings/immutable-releases/repositories"), get(immutable_repos).put(set_immutable_repos))
        .route(&format!("{o}/settings/immutable-releases/repositories/{{repository_id}}"), put(enable_immutable_repo).delete(disable_immutable_repo))
        .route(&format!("{o}/{{security_product}}/{{enablement}}"), post(security_product).fallback(|| async { ApiError::not_found() }))
        .route(&format!("{o}/outside_collaborators"), get(list_outside))
        .route(&format!("{o}/outside_collaborators/{{username}}"), put(convert_to_outside).delete(remove_outside))
        .route(&format!("{o}/audit-log"), get(audit_log))
        .route(&format!("{o}/installations"), get(installations))
        .route(&format!("{o}/personal-access-token-requests"), get(pat_requests).post(review_pat_requests))
        .route(&format!("{o}/personal-access-token-requests/{{id}}"), post(review_pat_request))
        .route(&format!("{o}/personal-access-token-requests/{{id}}/repositories"), get(pat_request_repos))
        .route(&format!("{o}/personal-access-tokens"), get(pat_grants).post(update_pat_accesses))
        .route(&format!("{o}/personal-access-tokens/{{id}}"), post(update_pat_access))
        .route(&format!("{o}/personal-access-tokens/{{id}}/repositories"), get(pat_grant_repos))
        .route(&format!("{o}/bypass-requests/push-rules"), get(push_bypass_requests))
        .route(&format!("{o}/rulesets/{{ruleset_id}}/history"), get(ruleset_history))
        .route(&format!("{o}/rulesets/{{ruleset_id}}/history/{{version_id}}"), get(ruleset_version))
}

// ---------------------------------------------------------------------------------------------
// Update / delete

#[derive(Deserialize, Default)]
pub struct OrgUpdate {
    pub billing_email: Option<String>,
    pub company: Option<String>,
    pub email: Option<String>,
    pub twitter_username: Option<String>,
    pub location: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub blog: Option<String>,
    pub has_organization_projects: Option<bool>,
    pub has_repository_projects: Option<bool>,
    pub default_repository_permission: Option<String>,
    pub members_can_create_repositories: Option<bool>,
    pub members_can_create_internal_repositories: Option<bool>,
    pub members_can_create_private_repositories: Option<bool>,
    pub members_can_create_public_repositories: Option<bool>,
    pub members_allowed_repository_creation_type: Option<String>,
    pub members_can_create_pages: Option<bool>,
    pub members_can_fork_private_repositories: Option<bool>,
    pub members_can_create_teams: Option<bool>,
    pub web_commit_signoff_required: Option<bool>,
    pub deploy_keys_enabled_for_repositories: Option<bool>,
    pub advanced_security_enabled_for_new_repositories: Option<bool>,
    pub dependabot_alerts_enabled_for_new_repositories: Option<bool>,
    pub dependabot_security_updates_enabled_for_new_repositories: Option<bool>,
    pub dependency_graph_enabled_for_new_repositories: Option<bool>,
    pub secret_scanning_enabled_for_new_repositories: Option<bool>,
    pub secret_scanning_push_protection_enabled_for_new_repositories: Option<bool>,
    pub secret_scanning_push_protection_custom_link_enabled: Option<bool>,
    pub secret_scanning_push_protection_custom_link: Option<String>,
}

fn invalid(field: &str) -> ApiError {
    ApiError::validation("Organization", field, "invalid")
}

/// Applies an org settings update (owners only).
pub async fn update_org_settings(s: &AppState, org: &User, actor: &User, req: OrgUpdate) -> ApiResult<()> {
    require_owner(s, org, actor).await?;
    if let Some(p) = &req.default_repository_permission {
        if !["read", "write", "admin", "none"].contains(&p.as_str()) {
            return Err(invalid("default_repository_permission"));
        }
    }
    for (f, v) in [("email", &req.email), ("billing_email", &req.billing_email)] {
        if v.as_deref().is_some_and(|e| !e.is_empty() && !e.contains('@')) {
            return Err(invalid(f));
        }
    }
    let st = orgs::settings(&s.db, org.id).await?;
    // members_allowed_repository_creation_type is the legacy spelling of the three booleans
    let (mut create, mut public, mut private) =
        (st.members_can_create_repositories, st.members_can_create_public_repositories, st.members_can_create_private_repositories);
    match req.members_allowed_repository_creation_type.as_deref() {
        None => {}
        Some("all") => (create, public, private) = (true, true, true),
        Some("private") => (create, public, private) = (true, false, true),
        Some("none") => (create, public, private) = (false, false, false),
        Some(_) => return Err(invalid("members_allowed_repository_creation_type")),
    }
    if let Some(v) = req.members_can_create_repositories {
        create = v;
        if v && req.members_can_create_public_repositories.is_none() && req.members_can_create_private_repositories.is_none() && !public && !private {
            (public, private) = (true, true);
        }
    }
    if let Some(v) = req.members_can_create_public_repositories {
        public = v;
    }
    if let Some(v) = req.members_can_create_private_repositories {
        private = v;
    }
    if req.members_can_create_public_repositories.is_some() || req.members_can_create_private_repositories.is_some() {
        create = public || private;
    }
    let mut sec = st.security_defaults.as_object().cloned().unwrap_or_default();
    for (k, v) in [
        ("advanced_security_enabled_for_new_repositories", req.advanced_security_enabled_for_new_repositories),
        ("dependabot_alerts_enabled_for_new_repositories", req.dependabot_alerts_enabled_for_new_repositories),
        ("dependabot_security_updates_enabled_for_new_repositories", req.dependabot_security_updates_enabled_for_new_repositories),
        ("dependency_graph_enabled_for_new_repositories", req.dependency_graph_enabled_for_new_repositories),
        ("secret_scanning_enabled_for_new_repositories", req.secret_scanning_enabled_for_new_repositories),
        ("secret_scanning_push_protection_enabled_for_new_repositories", req.secret_scanning_push_protection_enabled_for_new_repositories),
        ("secret_scanning_push_protection_custom_link_enabled", req.secret_scanning_push_protection_custom_link_enabled),
    ] {
        if let Some(v) = v {
            sec.insert(k.into(), json!(v));
        }
    }
    if let Some(l) = &req.secret_scanning_push_protection_custom_link {
        sec.insert("secret_scanning_push_protection_custom_link".into(), json!(l));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query(
        "UPDATE users SET name = COALESCE($2, name), company = COALESCE($3, company), email = COALESCE($4, email),
            twitter_username = COALESCE($5, twitter_username), location = COALESCE($6, location), blog = COALESCE($7, blog),
            bio = COALESCE($8, bio), updated_at = now() WHERE id = $1",
    )
    .bind(org.id)
    .bind(&req.name)
    .bind(&req.company)
    .bind(&req.email)
    .bind(&req.twitter_username)
    .bind(&req.location)
    .bind(&req.blog)
    .bind(&req.description)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE org_settings SET billing_email = COALESCE($2, billing_email),
            default_repository_permission = COALESCE($3, default_repository_permission),
            has_organization_projects = COALESCE($4, has_organization_projects),
            has_repository_projects = COALESCE($5, has_repository_projects),
            members_can_create_repositories = $6, members_can_create_public_repositories = $7,
            members_can_create_private_repositories = $8,
            members_can_create_internal_repositories = COALESCE($9, members_can_create_internal_repositories),
            members_can_create_pages = COALESCE($10, members_can_create_pages),
            members_can_fork_private_repositories = COALESCE($11, members_can_fork_private_repositories),
            members_can_create_teams = COALESCE($12, members_can_create_teams),
            web_commit_signoff_required = COALESCE($13, web_commit_signoff_required),
            deploy_keys_enabled_for_repositories = COALESCE($14, deploy_keys_enabled_for_repositories),
            security_defaults = $15, updated_at = now()
          WHERE org_id = $1",
    )
    .bind(org.id)
    .bind(&req.billing_email)
    .bind(&req.default_repository_permission)
    .bind(req.has_organization_projects)
    .bind(req.has_repository_projects)
    .bind(create)
    .bind(public)
    .bind(private)
    .bind(req.members_can_create_internal_repositories)
    .bind(req.members_can_create_pages)
    .bind(req.members_can_fork_private_repositories)
    .bind(req.members_can_create_teams)
    .bind(req.web_commit_signoff_required)
    .bind(req.deploy_keys_enabled_for_repositories)
    .bind(Value::Object(sec))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if let Some(p) = &req.default_repository_permission {
        if *p != st.default_repository_permission {
            audit_quiet(&s.db, org, Some(actor), "org.update_default_repository_permission",
                json!({ "permission": p, "permission_was": st.default_repository_permission })).await;
        }
    }
    if (create, public, private) != (st.members_can_create_repositories, st.members_can_create_public_repositories, st.members_can_create_private_repositories) {
        audit_quiet(&s.db, org, Some(actor), "org.update_member_repository_creation_permission",
            json!({ "can_create_repositories": create, "public": public, "private": private })).await;
    }
    audit_quiet(&s.db, org, Some(actor), "org.update", json!({})).await;
    Ok(())
}

async fn update_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(req): Json<OrgUpdate>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    update_org_settings(&s, &o, &u, req).await?;
    let o = db::user_by_id(&s.db, o.id).await?;
    Ok(Json(orgs::org_full_json(&s, &o, Some(&u)).await?))
}

/// Deletes the organization with all its repositories (owners only).
pub async fn delete_organization(s: &AppState, org: &User, actor: &User) -> ApiResult<()> {
    require_owner(s, org, actor).await?;
    let repos: Vec<i64> = sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1").bind(org.id).fetch_all(&s.db).await?;
    sqlx::query("DELETE FROM users WHERE id = $1 AND kind = 'Organization'").bind(org.id).execute(&s.db).await?;
    for id in repos {
        if let Err(e) = s.git.delete_repo(id).await {
            tracing::warn!(error = %e, repo_id = id, "deleting repository storage of a deleted org");
        }
    }
    Ok(())
}

async fn delete_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    delete_organization(&s, &o, &u).await?;
    Ok((StatusCode::ACCEPTED, axum::Json(json!({}))).into_response())
}

// ---------------------------------------------------------------------------------------------
// Announcement banner

fn announcement_json(st: &orgs::OrgSettings) -> Value {
    let live = st.announcement.is_some() && st.announcement_expires_at.is_none_or(|t| t > Utc::now());
    if !live {
        return json!({ "announcement": null, "expires_at": null, "user_dismissible": false });
    }
    json!({
        "announcement": st.announcement,
        "expires_at": crate::json::ts_opt(st.announcement_expires_at),
        "user_dismissible": st.announcement_user_dismissible,
    })
}

async fn get_announcement(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_member(&s, &o, &u).await?;
    Ok(Json(announcement_json(&orgs::settings(&s.db, o.id).await?)))
}

#[derive(Deserialize)]
struct AnnouncementBody {
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    announcement: Option<Option<String>>,
    expires_at: Option<String>,
    user_dismissible: Option<bool>,
}

async fn set_announcement(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<AnnouncementBody>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let Some(text) = b.announcement else { return Err(ApiError::validation("Announcement", "announcement", "missing_field")) };
    let expires: Option<DateTime<Utc>> = match b.expires_at.as_deref() {
        None | Some("") => None,
        Some(t) => Some(
            DateTime::parse_from_rfc3339(t).map_err(|_| ApiError::validation("Announcement", "expires_at", "invalid"))?.with_timezone(&Utc),
        ),
    };
    orgs::settings(&s.db, o.id).await?;
    let st: orgs::OrgSettings = sqlx::query_as(
        "UPDATE org_settings SET announcement = $2, announcement_expires_at = $3, announcement_user_dismissible = $4, updated_at = now()
          WHERE org_id = $1 RETURNING *",
    )
    .bind(o.id)
    .bind(text.filter(|t| !t.is_empty()))
    .bind(expires)
    .bind(b.user_dismissible.unwrap_or(false))
    .fetch_one(&s.db)
    .await?;
    audit_quiet(&s.db, &o, Some(&u), "org.set_announcement", json!({})).await;
    Ok(Json(announcement_json(&st)))
}

async fn remove_announcement(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    sqlx::query("UPDATE org_settings SET announcement = NULL, announcement_expires_at = NULL, announcement_user_dismissible = false WHERE org_id = $1")
        .bind(o.id)
        .execute(&s.db)
        .await?;
    audit_quiet(&s.db, &o, Some(&u), "org.remove_announcement", json!({})).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Immutable releases policy

async fn get_immutable(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let st = orgs::settings(&s.db, o.id).await?;
    let mut v = json!({ "enforced_repositories": st.immutable_releases });
    if st.immutable_releases == "selected" {
        v["selected_repositories_url"] = json!(format!("{}/orgs/{}/settings/immutable-releases/repositories", s.api_url(), o.login));
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct ImmutableBody {
    enforced_repositories: Option<String>,
    selected_repository_ids: Option<Vec<i64>>,
}

/// Replaces the selected repositories (ids must be repositories of the org).
async fn replace_immutable_repos(s: &AppState, org: &User, ids: &[i64]) -> ApiResult<()> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE id = ANY($1) AND owner_id = $2").bind(ids).bind(org.id).fetch_one(&s.db).await?;
    let distinct = ids.iter().collect::<std::collections::HashSet<_>>().len() as i64;
    if n != distinct {
        return Err(ApiError::validation("ImmutableReleases", "selected_repository_ids", "invalid"));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM org_immutable_release_repos WHERE org_id = $1").bind(org.id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO org_immutable_release_repos (org_id, repo_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(org.id)
        .bind(ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn set_immutable(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<ImmutableBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let mode = b.enforced_repositories.ok_or_else(|| ApiError::validation("ImmutableReleases", "enforced_repositories", "missing_field"))?;
    if !["all", "none", "selected"].contains(&mode.as_str()) {
        return Err(ApiError::validation("ImmutableReleases", "enforced_repositories", "invalid"));
    }
    orgs::settings(&s.db, o.id).await?;
    if let Some(ids) = &b.selected_repository_ids {
        replace_immutable_repos(&s, &o, ids).await?;
    }
    sqlx::query("UPDATE org_settings SET immutable_releases = $2, updated_at = now() WHERE org_id = $1").bind(o.id).bind(&mode).execute(&s.db).await?;
    audit_quiet(&s.db, &o, Some(&u), "org.immutable_releases_policy_update", json!({ "enforced_repositories": mode })).await;
    Ok(no_content())
}

async fn immutable_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM org_immutable_release_repos WHERE org_id = $1").bind(o.id).fetch_one(&s.db).await?;
    let repos: Vec<db::Repo> = sqlx::query_as(
        "SELECT r.* FROM org_immutable_release_repos x JOIN repositories r ON r.id = x.repo_id WHERE x.org_id = $1 ORDER BY r.id LIMIT $2 OFFSET $3",
    )
    .bind(o.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items: Vec<Value> = repos
        .into_iter()
        .map(|r| crate::json::repo::repo(&s.api_url(), s.base_url(), &RepoCtx { repo: r, owner: o.clone(), perm: Perm::Admin }))
        .collect();
    Ok(Json(json!({ "total_count": total, "repositories": items })))
}

#[derive(Deserialize)]
struct SelectedIds {
    selected_repository_ids: Option<Vec<i64>>,
}

async fn set_immutable_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<SelectedIds>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let ids = b.selected_repository_ids.ok_or_else(|| ApiError::validation("ImmutableReleases", "selected_repository_ids", "missing_field"))?;
    replace_immutable_repos(&s, &o, &ids).await?;
    Ok(no_content())
}

async fn org_repo_by_id(s: &AppState, org: &User, id: i64) -> ApiResult<db::Repo> {
    let r = db::repo_by_id(&s.db, id).await?;
    if r.owner_id != org.id {
        return Err(ApiError::not_found());
    }
    Ok(r)
}

async fn enable_immutable_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    org_repo_by_id(&s, &o, id).await?;
    sqlx::query("INSERT INTO org_immutable_release_repos (org_id, repo_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(o.id).bind(id).execute(&s.db).await?;
    Ok(no_content())
}

async fn disable_immutable_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    org_repo_by_id(&s, &o, id).await?;
    sqlx::query("DELETE FROM org_immutable_release_repos WHERE org_id = $1 AND repo_id = $2").bind(o.id).bind(id).execute(&s.db).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Security products on all repositories

pub const SECURITY_PRODUCTS: &[&str] = &[
    "dependency_graph",
    "dependabot_alerts",
    "dependabot_security_updates",
    "advanced_security",
    "code_scanning_default_setup",
    "secret_scanning",
    "secret_scanning_push_protection",
    "secret_scanning_non_provider_patterns",
];

#[derive(Deserialize, Default)]
struct ProductBody {
    query_suite: Option<String>,
}

/// Whether `product` is enabled on `repo_id` (set by the org-wide enable/disable endpoint).
pub async fn security_product_enabled(db: &sqlx::PgPool, repo_id: i64, product: &str) -> ApiResult<Option<bool>> {
    Ok(sqlx::query_scalar("SELECT enabled FROM repo_security_products WHERE repo_id = $1 AND product = $2")
        .bind(repo_id)
        .bind(product)
        .fetch_optional(db)
        .await?)
}

async fn security_product(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, product, enablement)): Path<(String, String, String)>, Json(b): Json<ProductBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    if !SECURITY_PRODUCTS.contains(&product.as_str()) {
        return Err(ApiError::not_found());
    }
    require_capability(&s, &o, &u, &["write_organization_security_products", "write_security_events"]).await?;
    let enabled = match enablement.as_str() {
        "enable_all" => true,
        "disable_all" => false,
        _ => return Err(ApiError::unprocessable("enablement must be enable_all or disable_all")),
    };
    if let Some(q) = &b.query_suite {
        if product != "code_scanning_default_setup" || !["default", "extended"].contains(&q.as_str()) {
            return Err(ApiError::unprocessable("query_suite is only valid for code_scanning_default_setup (default or extended)"));
        }
    }
    let options = b.query_suite.map(|q| json!({ "query_suite": q })).unwrap_or_else(|| json!({}));
    sqlx::query(
        "INSERT INTO repo_security_products (repo_id, product, enabled, options)
         SELECT id, $2, $3, $4 FROM repositories WHERE owner_id = $1
         ON CONFLICT (repo_id, product) DO UPDATE SET enabled = $3, options = $4, updated_at = now()",
    )
    .bind(o.id)
    .bind(&product)
    .bind(enabled)
    .bind(options)
    .execute(&s.db)
    .await?;
    audit_quiet(&s.db, &o, Some(&u), &format!("org.{}_{product}", if enabled { "enable" } else { "disable" }), json!({})).await;
    crate::advsec::on_products_changed(&s, o.id, &product, enabled).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Outside collaborators

#[derive(Deserialize, Default)]
struct OutsideQuery {
    filter: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_outside(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<OutsideQuery>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_member(&s, &o, &u).await?;
    if !matches!(q.filter.as_deref(), None | Some("all") | Some("2fa_disabled") | Some("2fa_insecure")) {
        return Err(ApiError::validation("OutsideCollaborator", "filter", "invalid"));
    }
    let p = PageParams { page: q.page, per_page: q.per_page };
    // forge has no two-factor authentication, so every collaborator matches `2fa_disabled`.
    let users: Vec<User> = sqlx::query_as(
        "SELECT * FROM users WHERE id IN (
            SELECT c.user_id FROM repo_collaborators c JOIN repositories r ON r.id = c.repo_id WHERE r.owner_id = $1
              AND NOT EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = $1 AND m.user_id = c.user_id))
          ORDER BY id LIMIT $2 OFFSET $3",
    )
    .bind(o.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items = users.iter().map(|x| crate::json::user::simple(&s.api_url(), s.base_url(), x)).collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

/// Converts an org member to an outside collaborator: they keep (as direct collaborator grants)
/// the repository access their teams gave them, and lose org membership, team memberships and
/// org roles.
pub async fn convert_member_to_outside(s: &AppState, org: &User, actor: &User, target: &User) -> ApiResult<()> {
    require_owner(s, org, actor).await?;
    let role = org_role(&s.db, org.id, target.id).await?.ok_or_else(ApiError::not_found)?;
    if role == "admin" {
        return Err(ApiError::forbidden("Organization owners cannot be converted to outside collaborators. Demote the owner first."));
    }
    let grants: Vec<(i64, String)> = sqlx::query_as(
        "WITH RECURSIVE mt(team_id) AS (
             SELECT tm.team_id FROM team_members tm JOIN teams t ON t.id = tm.team_id WHERE t.org_id = $1 AND tm.user_id = $2
             UNION SELECT t.parent_id FROM mt JOIN teams t ON t.id = mt.team_id WHERE t.parent_id IS NOT NULL)
         SELECT repo_id, permission FROM team_repos WHERE team_id IN (SELECT team_id FROM mt)",
    )
    .bind(org.id)
    .bind(target.id)
    .fetch_all(&s.db)
    .await?;
    let custom = orgs::perm::custom_roles(&s.db, org.id).await?;
    let mut best: std::collections::HashMap<i64, (Perm, String)> = std::collections::HashMap::new();
    for (rid, g) in grants {
        let p = orgs::perm::resolve_grant(&g, &custom);
        if best.get(&rid).is_none_or(|b| p > b.0) {
            best.insert(rid, (p, g));
        }
    }
    let mut tx = s.db.begin().await?;
    for (rid, (_, g)) in &best {
        sqlx::query(
            "INSERT INTO repo_collaborators (repo_id, user_id, permission) VALUES ($1, $2, $3) ON CONFLICT (repo_id, user_id) DO NOTHING",
        )
        .bind(rid)
        .bind(target.id)
        .bind(g)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2").bind(org.id).bind(target.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM team_members WHERE user_id = $2 AND team_id IN (SELECT id FROM teams WHERE org_id = $1)")
        .bind(org.id)
        .bind(target.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM org_role_users WHERE org_id = $1 AND user_id = $2").bind(org.id).bind(target.id).execute(&mut *tx).await?;
    tx.commit().await?;
    audit_quiet(&s.db, org, Some(actor), "org.convert_member_to_outside_collaborator", json!({ "user": target.login })).await;
    Ok(())
}

#[derive(Deserialize, Default)]
struct AsyncBody {
    #[serde(rename = "async")]
    is_async: Option<bool>,
}

async fn convert_to_outside(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>, Json(b): Json<AsyncBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    convert_member_to_outside(&s, &o, &u, &target).await?;
    if b.is_async == Some(true) {
        return Ok((StatusCode::ACCEPTED, axum::Json(json!({}))).into_response());
    }
    Ok(no_content())
}

async fn remove_outside(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, login)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let target = db::user_by_login(&s.db, &login).await?;
    if org_role(&s.db, o.id, target.id).await?.is_some() {
        return Err(ApiError::unprocessable("You cannot specify an organization member to remove as an outside collaborator."));
    }
    sqlx::query("DELETE FROM repo_collaborators WHERE user_id = $2 AND repo_id IN (SELECT id FROM repositories WHERE owner_id = $1)")
        .bind(o.id)
        .bind(target.id)
        .execute(&s.db)
        .await?;
    audit_quiet(&s.db, &o, Some(&u), "org.remove_outside_collaborator", json!({ "user": target.login })).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Audit log

#[derive(Deserialize, Default)]
struct AuditQuery {
    phrase: Option<String>,
    include: Option<String>,
    after: Option<String>,
    before: Option<String>,
    order: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    id: i64,
    action: String,
    actor_id: Option<i64>,
    actor: Option<String>,
    user_login: Option<String>,
    repo: Option<String>,
    team: Option<String>,
    data: Value,
    created_at: DateTime<Utc>,
}

fn cursor(id: i64) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("audit:{id}"))
}

fn parse_cursor(c: &str) -> Option<i64> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(c).ok()?;
    String::from_utf8(raw).ok()?.strip_prefix("audit:")?.parse().ok()
}

/// One `key:value` qualifier of the audit-log search phrase.
fn push_qualifier(qb: &mut sqlx::QueryBuilder<sqlx::Postgres>, key: &str, value: &str, negate: bool) -> ApiResult<()> {
    let col = match key {
        "action" => "action",
        "actor" => "actor",
        "user" => "user_login",
        "repo" => "repo",
        "team" => "team",
        "created" => {
            let (op, date) = [">=", "<=", ">", "<"].iter().find_map(|op| value.strip_prefix(op).map(|d| (*op, d))).unwrap_or(("=", value));
            let d = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| ApiError::unprocessable("Invalid created qualifier"))?;
            let start = d.and_hms_opt(0, 0, 0).expect("midnight").and_utc();
            let end = start + chrono::Duration::days(1);
            qb.push(if negate { " AND NOT (" } else { " AND (" });
            match op {
                ">=" => qb.push("created_at >= ").push_bind(start),
                ">" => qb.push("created_at >= ").push_bind(end),
                "<=" => qb.push("created_at < ").push_bind(end),
                "<" => qb.push("created_at < ").push_bind(start),
                _ => qb.push("created_at >= ").push_bind(start).push(" AND created_at < ").push_bind(end),
            };
            qb.push(")");
            return Ok(());
        }
        _ => return Err(ApiError::unprocessable(format!("Unsupported audit log qualifier: {key}"))),
    };
    // `action:team` matches the whole `team.*` category; `*` is a wildcard.
    let pattern = if key == "action" && !value.contains('.') && !value.contains('*') {
        format!("{value}.%")
    } else {
        value.replace('%', "\\%").replace('_', "\\_").replace('*', "%")
    };
    qb.push(if negate { " AND NOT COALESCE(" } else { " AND COALESCE(" }).push(col).push(", '') ILIKE ").push_bind(pattern.clone());
    if key == "action" && !value.contains('.') && !value.contains('*') {
        qb.push(if negate { " AND action <> " } else { " OR action = " }).push_bind(value.to_string());
    }
    Ok(())
}

async fn audit_log(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<AuditQuery>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, &["read_audit_logs"]).await?;
    if !matches!(q.include.as_deref(), None | Some("web") | Some("git") | Some("all")) {
        return Err(ApiError::validation("AuditLog", "include", "invalid"));
    }
    let desc = match q.order.as_deref() {
        None | Some("desc") => true,
        Some("asc") => false,
        Some(_) => return Err(ApiError::validation("AuditLog", "order", "invalid")),
    };
    let p = PageParams { page: q.page, per_page: q.per_page };
    let mut qb = sqlx::QueryBuilder::new("SELECT id, action, actor_id, actor, user_login, repo, team, data, created_at FROM org_audit_log WHERE org_id = ");
    qb.push_bind(o.id);
    // forge records web/API events only; `include=git` has none.
    if q.include.as_deref() == Some("git") {
        qb.push(" AND false");
    }
    for tok in q.phrase.as_deref().unwrap_or("").split_whitespace() {
        let (neg, tok) = tok.strip_prefix('-').map(|t| (true, t)).unwrap_or((false, tok));
        match tok.split_once(':') {
            Some((k, v)) => {
                qb.push(" AND (TRUE");
                push_qualifier(&mut qb, k, v, neg)?;
                qb.push(")");
            }
            None => {
                qb.push(if neg { " AND NOT " } else { " AND " }).push("(action ILIKE ").push_bind(format!("%{tok}%"));
                qb.push(" OR COALESCE(actor, '') ILIKE ").push_bind(format!("%{tok}%")).push(")");
            }
        }
    }
    let after = q.after.as_deref().map(|c| parse_cursor(c).ok_or_else(|| ApiError::validation("AuditLog", "after", "invalid"))).transpose()?;
    let before = q.before.as_deref().map(|c| parse_cursor(c).ok_or_else(|| ApiError::validation("AuditLog", "before", "invalid"))).transpose()?;
    if let Some(a) = after {
        qb.push(if desc { " AND id < " } else { " AND id > " }).push_bind(a);
    }
    if let Some(b) = before {
        qb.push(if desc { " AND id > " } else { " AND id < " }).push_bind(b);
    }
    qb.push(if desc { " ORDER BY id DESC" } else { " ORDER BY id ASC" });
    let cursor_mode = after.is_some() || before.is_some();
    qb.push(" LIMIT ").push_bind(p.limit() + 1);
    if !cursor_mode {
        qb.push(" OFFSET ").push_bind(p.offset());
    }
    let mut rows: Vec<AuditRow> = qb.build_query_as().fetch_all(&s.db).await?;
    let more = rows.len() as i64 > p.limit();
    rows.truncate(p.limit() as usize);
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let ms = r.created_at.timestamp_millis();
            let mut v = json!({
                "@timestamp": ms,
                "created_at": ms,
                "_document_id": cursor(r.id),
                "action": r.action,
                "actor": r.actor,
                "actor_id": r.actor_id,
                "org": o.login,
                "org_id": o.id,
                "operation_type": r.action.rsplit('.').next().unwrap_or(""),
                "data": r.data,
            });
            for (k, val) in [("user", &r.user_login), ("repo", &r.repo), ("team", &r.team)] {
                if let Some(x) = val {
                    v[k] = json!(x);
                }
            }
            v
        })
        .collect();
    let mut res = axum::Json(Value::Array(items)).into_response();
    if more {
        if let Some(last) = rows.last() {
            let mut pairs: Vec<(String, String)> = uri
                .0
                .query()
                .map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect())
                .unwrap_or_default();
            pairs.retain(|(k, _)| k != "after" && k != "before" && k != "page");
            pairs.push(("after".into(), cursor(last.id)));
            let qs = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
            if let Ok(v) = format!("<{}{}?{qs}>; rel=\"next\"", s.base_url(), uri.0.path()).parse() {
                res.headers_mut().insert(axum::http::header::LINK, v);
            }
        }
    }
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// App installations (forge has no GitHub Apps yet: the list is empty)

async fn installations(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(_p): Query<PageParams>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let rows: Vec<crate::apps::InstallationRow> = sqlx::query_as("SELECT * FROM app_installations WHERE target_id = $1 AND app_id NOT IN (SELECT id FROM apps WHERE slug = 'github-actions') ORDER BY id")
        .bind(o.id)
        .fetch_all(&s.db)
        .await?;
    let items = crate::apps::installations_json(&s, &rows).await?;
    Ok(Json(json!({ "total_count": items.len(), "installations": items })))
}

// ---------------------------------------------------------------------------------------------
// Fine-grained personal access tokens. forge issues classic tokens only, so there are never
// pending requests or fine-grained grants: lists are empty and ids are unknown (404).

#[derive(Deserialize, Default)]
struct PatQuery {
    sort: Option<String>,
    direction: Option<String>,
    #[allow(dead_code)]
    page: Option<i64>,
    #[allow(dead_code)]
    per_page: Option<i64>,
}

fn check_pat_query(q: &PatQuery) -> ApiResult<()> {
    if !matches!(q.sort.as_deref(), None | Some("created_at")) {
        return Err(ApiError::validation("PersonalAccessToken", "sort", "invalid"));
    }
    if !matches!(q.direction.as_deref(), None | Some("asc") | Some("desc")) {
        return Err(ApiError::validation("PersonalAccessToken", "direction", "invalid"));
    }
    Ok(())
}

async fn pat_requests(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<PatQuery>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    check_pat_query(&q)?;
    Ok(Json(json!([])))
}

async fn pat_grants(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<PatQuery>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    check_pat_query(&q)?;
    Ok(Json(json!([])))
}

#[derive(Deserialize, Default)]
struct ReviewBody {
    action: Option<String>,
    pat_request_ids: Option<Vec<i64>>,
    pat_ids: Option<Vec<i64>>,
    #[allow(dead_code)]
    reason: Option<String>,
}

fn check_action(b: &ReviewBody, allowed: &[&str]) -> ApiResult<()> {
    match b.action.as_deref() {
        None => Err(ApiError::validation("PersonalAccessToken", "action", "missing_field")),
        Some(a) if !allowed.contains(&a) => Err(ApiError::validation("PersonalAccessToken", "action", "invalid")),
        _ => Ok(()),
    }
}

async fn review_pat_requests(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<ReviewBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    check_action(&b, &["approve", "deny"])?;
    if b.pat_request_ids.as_ref().is_some_and(|ids| !ids.is_empty()) {
        return Err(ApiError::not_found());
    }
    // no ids: review every pending request (there are none)
    Ok((StatusCode::ACCEPTED, axum::Json(json!({}))).into_response())
}

async fn review_pat_request(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, _id)): Path<(String, i64)>, Json(b): Json<ReviewBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    check_action(&b, &["approve", "deny"])?;
    Err(ApiError::not_found())
}

async fn pat_request_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, _id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    Err(ApiError::not_found())
}

async fn update_pat_accesses(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<ReviewBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    check_action(&b, &["revoke"])?;
    match &b.pat_ids {
        None => Err(ApiError::validation("PersonalAccessToken", "pat_ids", "missing_field")),
        Some(ids) if ids.is_empty() => Err(ApiError::validation("PersonalAccessToken", "pat_ids", "invalid")),
        Some(_) => Err(ApiError::not_found()),
    }
}

async fn update_pat_access(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, _id)): Path<(String, i64)>, Json(b): Json<ReviewBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    check_action(&b, &["revoke"])?;
    Err(ApiError::not_found())
}

async fn pat_grant_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, _id)): Path<(String, i64)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    Err(ApiError::not_found())
}

// ---------------------------------------------------------------------------------------------
// Push-rule bypass requests (forge does not enforce push rulesets, so none are ever raised)

#[derive(Deserialize, Default)]
struct BypassQuery {
    time_period: Option<String>,
    request_status: Option<String>,
}

async fn push_bypass_requests(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<BypassQuery>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, &["manage_organization_ref_rules"]).await?;
    if !matches!(q.time_period.as_deref(), None | Some("hour") | Some("day") | Some("week") | Some("month")) {
        return Err(ApiError::validation("BypassRequest", "time_period", "invalid"));
    }
    let statuses = ["completed", "cancelled", "approved", "expired", "deleted", "denied", "open", "all"];
    if q.request_status.as_deref().is_some_and(|x| !statuses.contains(&x)) {
        return Err(ApiError::validation("BypassRequest", "request_status", "invalid"));
    }
    Ok(Json(json!([])))
}

// ---------------------------------------------------------------------------------------------
// Ruleset history

/// Records a ruleset version (called by the rulesets service on create/update).
pub async fn record_ruleset_version(db: &sqlx::PgPool, ruleset_id: i64, actor: &User, state: Value) {
    if let Err(e) = sqlx::query("INSERT INTO ruleset_versions (ruleset_id, actor_id, state) VALUES ($1, $2, $3)")
        .bind(ruleset_id)
        .bind(actor.id)
        .bind(state)
        .execute(db)
        .await
    {
        tracing::warn!(error = %e, ruleset_id, "recording ruleset version");
    }
}

async fn org_ruleset(s: &AppState, u: &User, org: &str, id: i64) -> ApiResult<(User, super::rulesets::RulesetRow)> {
    let o = org_by_login(s, org).await?;
    require_capability(s, &o, u, &["manage_organization_ref_rules"]).await?;
    let level = super::rulesets::Level::Org(o.clone());
    let row = super::rulesets::get_ruleset(s, &level, id).await?;
    if row.org_id != Some(o.id) {
        return Err(ApiError::not_found());
    }
    // rulesets created before history was recorded get their current state as version 1
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM ruleset_versions WHERE ruleset_id = $1").bind(id).fetch_one(&s.db).await?;
    if n == 0 {
        let state = super::rulesets::ruleset_json(s, &row, &level, true);
        sqlx::query("INSERT INTO ruleset_versions (ruleset_id, actor_id, state, created_at) VALUES ($1, NULL, $2, $3)")
            .bind(id)
            .bind(state)
            .bind(row.updated_at)
            .execute(&s.db)
            .await?;
    }
    Ok((o, row))
}

#[derive(sqlx::FromRow)]
struct VersionRow {
    id: i64,
    actor_id: Option<i64>,
    state: Value,
    created_at: DateTime<Utc>,
}

fn version_json(v: &VersionRow) -> Value {
    json!({
        "version_id": v.id,
        "actor": match v.actor_id { Some(id) => json!({ "id": id, "type": "User" }), None => json!({ "type": "unknown" }) },
        "updated_at": crate::json::ts(v.created_at),
    })
}

async fn ruleset_history(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    org_ruleset(&s, &u, &org, id).await?;
    let rows: Vec<VersionRow> = sqlx::query_as("SELECT id, actor_id, state, created_at FROM ruleset_versions WHERE ruleset_id = $1 ORDER BY id DESC LIMIT $2 OFFSET $3")
        .bind(id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ruleset_versions WHERE ruleset_id = $1").bind(id).fetch_one(&s.db).await?;
    Ok(Paged::new(&uri, &s, p, Some(total), rows.iter().map(version_json).collect()))
}

async fn ruleset_version(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id, vid)): Path<(String, i64, i64)>) -> ApiResult<Json<Value>> {
    org_ruleset(&s, &u, &org, id).await?;
    let row: VersionRow = sqlx::query_as("SELECT id, actor_id, state, created_at FROM ruleset_versions WHERE ruleset_id = $1 AND id = $2")
        .bind(id)
        .bind(vid)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut v = version_json(&row);
    v["state"] = row.state;
    Ok(Json(v))
}
