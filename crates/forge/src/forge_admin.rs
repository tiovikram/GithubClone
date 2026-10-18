//! Forge-only site administration endpoints for the web UI (`/api/forge/...`, not part of the
//! GitHub API). GHES's REST API has no way to list or search users, organizations and
//! repositories with their administrative state (suspension, reason, site admin, LDAP DN) — the
//! GHES web UI ("stafftools") reads that directly — so the site admin pages use these:
//!
//! - `GET /maintenance` (public, exempt from maintenance mode): the maintenance state as the
//!   viewer experiences it, so the SPA can show its maintenance page instead of failing requests;
//! - `GET /admin/info`: enterprise slug and versions;
//! - `GET /admin/users`, `GET /admin/users/{login}`: search users with their admin state, and a
//!   user's details (organizations, repositories, SSH keys, emails, tokens);
//! - `PUT /admin/users/{login}/password`: sets a user's password (accounts created by
//!   `POST /admin/users` have none);
//! - `GET /admin/orgs`, `GET /admin/repos`: search organizations / all repositories.
//!
//! Everything but `/maintenance` is for site administrators only (403 otherwise). State changes
//! go through the GHES endpoints (`/api/v3/admin/...`) except the password reset.
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::MaybeUser;
use crate::db::User;
use crate::enterprise::{self as ent, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/maintenance", get(maintenance))
        .route("/admin/info", get(info))
        .route("/admin/users", get(list_users))
        .route("/admin/users/{login}", get(user_detail))
        .route("/admin/users/{login}/password", put(set_password))
        .route("/admin/orgs", get(list_orgs))
        .route("/admin/repos", get(list_repos))
}

/// Paths of this module that stay reachable during maintenance (see `enterprise::maintenance_exempt`).
pub const MAINTENANCE_STATUS_PATH: &str = "/api/forge/maintenance";

async fn maintenance(State(s): State<AppState>, MaybeUser(u): MaybeUser) -> ApiResult<Json<Value>> {
    let m = ent::maintenance(&s).await;
    let bypass = u.as_ref().is_some_and(|u| u.site_admin);
    Ok(Json(json!({
        "status": m.status(),
        "active": m.active(),
        "message": if m.message.is_empty() { Value::Null } else { json!(m.message) },
        "scheduled_time": m.when.map(crate::json::ts),
        // Site admins (and the IP exception list, which the guard checks) keep access.
        "bypass": bypass,
    })))
}

async fn info(SiteAdmin(_): SiteAdmin) -> Json<Value> {
    Json(json!({
        "enterprise_slug": ent::slug(),
        "forge_version": env!("CARGO_PKG_VERSION"),
        "ghes_api_version": "3.22",
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    /// users: all | admin | suspended | active
    filter: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

impl ListQuery {
    fn per(&self) -> i64 {
        self.per_page.unwrap_or(30).clamp(1, 100)
    }
    fn offset(&self) -> i64 {
        (self.page.unwrap_or(1).clamp(1, 100_000) - 1) * self.per()
    }
    fn pattern(&self) -> Option<String> {
        self.q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(|q| {
            let esc = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            format!("%{esc}%")
        })
    }
}

#[derive(sqlx::FromRow)]
struct AdminUserRow {
    id: i64,
    login: String,
    name: Option<String>,
    email: Option<String>,
    site_admin: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    suspended_at: Option<chrono::DateTime<chrono::Utc>>,
    suspension_reason: Option<String>,
    has_password: bool,
    repos: i64,
    ldap_dn: Option<String>,
}

const USER_SELECT: &str = "SELECT u.id, u.login, u.name, u.email, u.site_admin, u.created_at, u.suspended_at, u.suspension_reason,
        u.password_hash IS NOT NULL AS has_password,
        (SELECT count(*) FROM repositories r WHERE r.owner_id = u.id) AS repos,
        (SELECT ldap_dn FROM ent_ldap_mappings m WHERE m.kind = 'user' AND m.subject_id = u.id) AS ldap_dn
   FROM users u";

fn user_row_json(s: &AppState, r: &AdminUserRow) -> Value {
    json!({
        "id": r.id,
        "login": r.login,
        "name": r.name,
        "email": r.email,
        "avatar_url": format!("{}/avatars/{}", s.base_url(), r.login),
        "html_url": format!("{}/{}", s.base_url(), r.login),
        "site_admin": r.site_admin,
        "created_at": crate::json::ts(r.created_at),
        "suspended_at": r.suspended_at.map(crate::json::ts),
        "suspension_reason": r.suspension_reason,
        "has_password": r.has_password,
        "repos": r.repos,
        "ldap_dn": r.ldap_dn,
    })
}

async fn list_users(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let filter = match q.filter.as_deref().unwrap_or("all") {
        "all" | "" => "TRUE",
        "admin" => "u.site_admin",
        "suspended" => "u.suspended_at IS NOT NULL",
        "active" => "u.suspended_at IS NULL",
        _ => return Err(ApiError::validation("User", "filter", "invalid")),
    };
    let cond = format!(
        "u.kind = 'User' AND {filter} AND ($1::text IS NULL OR u.login ILIKE $1 OR u.email ILIKE $1 OR u.name ILIKE $1)"
    );
    let pat = q.pattern();
    let rows: Vec<AdminUserRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{USER_SELECT} WHERE {cond} ORDER BY lower(u.login) LIMIT $2 OFFSET $3")))
        .bind(&pat)
        .bind(q.per())
        .bind(q.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM users u WHERE {cond}"))).bind(&pat).fetch_one(&s.db).await?;
    Ok(Json(json!({ "total_count": total, "items": rows.iter().map(|r| user_row_json(&s, r)).collect::<Vec<_>>() })))
}

async fn account(s: &AppState, login: &str) -> ApiResult<User> {
    let u = crate::db::user_by_login(&s.db, login).await?;
    if u.kind != "User" {
        return Err(ApiError::not_found());
    }
    Ok(u)
}

async fn user_detail(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(login): Path<String>) -> ApiResult<Json<Value>> {
    let u = account(&s, &login).await?;
    let row: AdminUserRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{USER_SELECT} WHERE u.id = $1"))).bind(u.id).fetch_one(&s.db).await?;
    let mut v = user_row_json(&s, &row);
    let orgs: Vec<(String, String)> = sqlx::query_as(
        "SELECT o.login, m.role FROM org_members m JOIN users o ON o.id = m.org_id WHERE m.user_id = $1 ORDER BY lower(o.login)",
    )
    .bind(u.id)
    .fetch_all(&s.db)
    .await?;
    v["organizations"] = json!(orgs.iter().map(|(o, r)| json!({"login": o, "role": r})).collect::<Vec<_>>());
    let repos: Vec<(String, bool, bool, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT name, private, archived, pushed_at FROM repositories WHERE owner_id = $1 ORDER BY lower(name) LIMIT 100",
    )
    .bind(u.id)
    .fetch_all(&s.db)
    .await?;
    v["repositories"] = json!(repos
        .iter()
        .map(|(n, p, a, pushed)| json!({"name": n, "full_name": format!("{}/{n}", u.login), "private": p, "archived": a, "pushed_at": pushed.map(crate::json::ts)}))
        .collect::<Vec<_>>());
    let keys: Vec<(i64, String, String, String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT id, title, key, fingerprint, created_at FROM ssh_keys WHERE user_id = $1 ORDER BY id").bind(u.id).fetch_all(&s.db).await?;
    v["keys"] = json!(keys
        .iter()
        .map(|(id, t, k, f, c)| json!({"id": id, "title": t, "key": k, "fingerprint": f, "created_at": crate::json::ts(*c)}))
        .collect::<Vec<_>>());
    let mut emails: Vec<String> = u.email.iter().cloned().collect();
    let extra: Vec<String> = sqlx::query_scalar("SELECT email FROM user_emails WHERE user_id = $1 ORDER BY id").bind(u.id).fetch_all(&s.db).await?;
    for e in extra {
        if !emails.iter().any(|x| x.eq_ignore_ascii_case(&e)) {
            emails.push(e);
        }
    }
    v["emails"] = json!(emails);
    let tokens: Vec<ent::TokenRow> = sqlx::query_as(
        "SELECT id, user_id, name, token_hash, token_last_eight, scopes, created_at, last_used_at, expires_at, impersonation
           FROM tokens WHERE user_id = $1 ORDER BY id",
    )
    .bind(u.id)
    .fetch_all(&s.db)
    .await?;
    v["tokens"] = json!(tokens
        .iter()
        .map(|t| json!({
            "id": t.id,
            "name": t.name,
            "token_last_eight": t.token_last_eight,
            "scopes": t.scopes,
            "impersonation": t.impersonation,
            "created_at": crate::json::ts(t.created_at),
            "last_used_at": t.last_used_at.map(crate::json::ts),
            "expires_at": t.expires_at.map(crate::json::ts),
        }))
        .collect::<Vec<_>>());
    Ok(Json(v))
}

#[derive(Deserialize)]
struct PasswordBody {
    password: Option<String>,
}

async fn set_password(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(login): Path<String>, Json(b): Json<PasswordBody>) -> ApiResult<Response> {
    let u = account(&s, &login).await?;
    let pw = b.password.filter(|p| p.len() >= 8).ok_or_else(|| ApiError::validation("User", "password", "invalid"))?;
    let hash = tokio::task::spawn_blocking(move || crate::auth::hash_password(&pw)).await.map_err(ApiError::internal)??;
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1").bind(u.id).bind(hash).execute(&s.db).await?;
    // Existing web sessions of the user end, like a password change on github.com.
    if u.id != a.id {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(u.id).execute(&s.db).await?;
    }
    ent::audit(&s, "user.reset_password", Some(&a), json!({"user": u.login, "user_id": u.id})).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

#[derive(sqlx::FromRow)]
struct OrgRow {
    id: i64,
    login: String,
    name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    members: i64,
    owners: i64,
    repos: i64,
    teams: i64,
}

async fn list_orgs(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let cond = "o.kind = 'Organization' AND ($1::text IS NULL OR o.login ILIKE $1 OR o.name ILIKE $1)";
    let pat = q.pattern();
    let rows: Vec<OrgRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT o.id, o.login, o.name, o.created_at,
                (SELECT count(*) FROM org_members m WHERE m.org_id = o.id) AS members,
                (SELECT count(*) FROM org_members m WHERE m.org_id = o.id AND m.role = 'admin') AS owners,
                (SELECT count(*) FROM repositories r WHERE r.owner_id = o.id) AS repos,
                (SELECT count(*) FROM teams t WHERE t.org_id = o.id) AS teams
           FROM users o WHERE {cond} ORDER BY lower(o.login) LIMIT $2 OFFSET $3"
    )))
    .bind(&pat)
    .bind(q.per())
    .bind(q.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM users o WHERE {cond}"))).bind(&pat).fetch_one(&s.db).await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|o| {
            json!({
                "id": o.id,
                "login": o.login,
                "name": o.name,
                "avatar_url": format!("{}/avatars/{}", s.base_url(), o.login),
                "created_at": crate::json::ts(o.created_at),
                "members": o.members,
                "owners": o.owners,
                "repos": o.repos,
                "teams": o.teams,
            })
        })
        .collect();
    Ok(Json(json!({ "total_count": total, "items": items })))
}

#[derive(sqlx::FromRow)]
struct RepoRow {
    id: i64,
    owner: String,
    owner_kind: String,
    name: String,
    description: Option<String>,
    private: bool,
    visibility: String,
    archived: bool,
    fork: bool,
    size_kb: i64,
    created_at: chrono::DateTime<chrono::Utc>,
    pushed_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn list_repos(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    let filter = match q.filter.as_deref().unwrap_or("all") {
        "all" | "" => "TRUE",
        "private" => "r.private",
        "public" => "NOT r.private",
        "archived" => "r.archived",
        "fork" => "r.fork_parent_id IS NOT NULL",
        _ => return Err(ApiError::validation("Repository", "filter", "invalid")),
    };
    let cond = format!("{filter} AND ($1::text IS NULL OR (o.login || '/' || r.name) ILIKE $1 OR r.description ILIKE $1)");
    let pat = q.pattern();
    let rows: Vec<RepoRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.id, o.login AS owner, o.kind AS owner_kind, r.name, r.description, r.private, r.visibility, r.archived,
                r.fork_parent_id IS NOT NULL AS fork, r.size_kb, r.created_at, r.pushed_at
           FROM repositories r JOIN users o ON o.id = r.owner_id
          WHERE {cond} ORDER BY lower(o.login), lower(r.name) LIMIT $2 OFFSET $3"
    )))
    .bind(&pat)
    .bind(q.per())
    .bind(q.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM repositories r JOIN users o ON o.id = r.owner_id WHERE {cond}"
    )))
    .bind(&pat)
    .fetch_one(&s.db)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "full_name": format!("{}/{}", r.owner, r.name),
                "owner": {"login": r.owner, "type": r.owner_kind},
                "description": r.description,
                "private": r.private,
                "visibility": r.visibility,
                "archived": r.archived,
                "fork": r.fork,
                "size": r.size_kb,
                "created_at": crate::json::ts(r.created_at),
                "pushed_at": r.pushed_at.map(crate::json::ts),
            })
        })
        .collect();
    Ok(Json(json!({ "total_count": total, "items": items })))
}
