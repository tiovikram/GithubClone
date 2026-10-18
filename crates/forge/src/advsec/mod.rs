//! Advanced security: secret scanning (with push protection), Dependabot alerts against the
//! advisory database, code scanning (SARIF uploads), code security configurations, and the
//! delegated bypass / alert dismissal requests that go with them. See docs/advsec.md.
//!
//! Features act only when enabled for a repository. Enablement lives in the orgs area's
//! `repo_security_products` table (one row per product; set by `PATCH /repos/{o}/{r}`
//! `security_and_analysis`, by `POST /orgs/{org}/{product}/enable_all`, or by attaching a code
//! security configuration) and, for Dependabot alerts, in the repos area's
//! `repo_settings.vulnerability_alerts` (`PUT /repos/{o}/{r}/vulnerability-alerts`).
pub mod advisories;
pub mod code_scanning;
pub mod code_security;
pub mod dependabot;
pub mod dependabot_secrets;
pub mod patterns;
pub mod requests;
pub mod secret_scanning;
pub mod secret_scanning_api;
pub mod zip;

use std::collections::HashMap;

use axum::Router;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(advisories::routes())
        .merge(dependabot::routes())
        .merge(dependabot_secrets::routes())
        .merge(secret_scanning_api::routes())
        .merge(code_scanning::routes())
        .merge(code_security::routes())
        .merge(requests::routes())
}

/// forge-only endpoints, mounted under `/api/forge`.
pub fn forge_routes() -> Router<AppState> {
    advisories::forge_routes()
}

// ---------------------------------------------------------------------------------------------
// Background jobs (Postgres job queue, `jobs.rs`)

pub const Q_SECRET_SCAN: &str = "advsec.secret_scan";
pub const Q_DEPENDABOT: &str = "advsec.dependabot";
pub const Q_SARIF: &str = "advsec.sarif";
pub const Q_ADVISORY_RESCAN: &str = "advsec.advisory_rescan";

pub async fn run_job(state: &AppState, job: &crate::jobs::Job) -> anyhow::Result<()> {
    match job.queue.as_str() {
        Q_SECRET_SCAN => secret_scanning::run_scan_job(state, &job.payload).await,
        Q_DEPENDABOT => dependabot::run_job(state, &job.payload).await,
        Q_SARIF => code_scanning::run_job(state, &job.payload).await,
        Q_ADVISORY_RESCAN => dependabot::run_rescan_all(state).await,
        other => anyhow::bail!("unknown advsec queue {other}"),
    }
}

pub async fn enqueue(state: &AppState, queue: &str, payload: Value) {
    if let Err(e) = crate::jobs::enqueue(&state.db, queue, &payload).await {
        tracing::error!(error = %e, queue, "could not enqueue advsec job");
    }
}

// ---------------------------------------------------------------------------------------------
// Hooks called from other areas

/// After refs change (push or API write): incremental secret scan of the new commits, and a
/// Dependabot re-match when the default branch moved.
pub async fn on_refs_changed(state: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) {
    let updates: Vec<&(String, String, String)> = changed.iter().filter(|(_, _, new)| new != crate::webhooks::ZERO_SHA).collect();
    if updates.is_empty() {
        return;
    }
    if enabled(&state.db, ctx.repo.id, "secret_scanning").await {
        let changes: Vec<Value> = updates.iter().map(|(r, o, n)| json!([r, o, n])).collect();
        secret_scanning::schedule_scan(state, ctx.repo.id, "incremental", Some(changes), actor.map(|u| u.id)).await;
    }
    let default_ref = format!("refs/heads/{}", ctx.repo.default_branch);
    if updates.iter().any(|(r, _, _)| *r == default_ref) && enabled(&state.db, ctx.repo.id, "dependabot_alerts").await {
        enqueue(state, Q_DEPENDABOT, json!({ "repo_id": ctx.repo.id })).await;
    }
}

/// The orgs area's `POST /orgs/{org}/{product}/{enable_all|disable_all}` changed `product` on
/// every repository of `org`: keep the repos-area flag in sync and start scans.
pub async fn on_products_changed(state: &AppState, org_id: i64, product: &str, enabled: bool) {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM repositories WHERE owner_id = $1")
        .bind(org_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for id in ids {
        after_toggle(state, id, product, enabled, true).await;
    }
}

/// The repos area toggled `PUT/DELETE /repos/{o}/{r}/vulnerability-alerts`.
pub async fn on_vulnerability_alerts_changed(state: &AppState, repo_id: i64, enabled: bool) {
    if enabled {
        enqueue(state, Q_DEPENDABOT, json!({ "repo_id": repo_id })).await;
    }
}

// ---------------------------------------------------------------------------------------------
// Enablement

/// Products whose flag lives in `repo_security_products`.
pub const PRODUCTS: &[&str] = &[
    "advanced_security",
    "dependency_graph",
    "dependabot_alerts",
    "dependabot_security_updates",
    "dependabot_delegated_alert_dismissal",
    "code_scanning_default_setup",
    "code_scanning_delegated_alert_dismissal",
    "secret_scanning",
    "secret_scanning_push_protection",
    "secret_scanning_non_provider_patterns",
    "secret_scanning_validity_checks",
    "secret_scanning_delegated_bypass",
    "secret_scanning_delegated_alert_dismissal",
    "private_vulnerability_reporting",
];

/// Whether `product` is on for a repository. Dependency graph defaults to on; everything else to
/// off (GHES defaults; push protection in particular is off unless enabled).
pub async fn enabled(db: &PgPool, repo_id: i64, product: &str) -> bool {
    if product == "dependabot_alerts" {
        let v: Option<bool> = sqlx::query_scalar("SELECT vulnerability_alerts FROM repo_settings WHERE repo_id = $1")
            .bind(repo_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
        return v.unwrap_or(false);
    }
    let row = crate::api::orgs_settings::security_product_enabled(db, repo_id, product).await.ok().flatten();
    row.unwrap_or(product == "dependency_graph")
}

/// Options stored with a product (e.g. delegated bypass reviewers).
pub async fn product_options(db: &PgPool, repo_id: i64, product: &str) -> Value {
    sqlx::query_scalar("SELECT options FROM repo_security_products WHERE repo_id = $1 AND product = $2")
        .bind(repo_id)
        .bind(product)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| json!({}))
}

/// Turns a product on or off for one repository and runs the side effects (backfill scan,
/// Dependabot match).
pub async fn set_enabled(state: &AppState, repo_id: i64, product: &str, on: bool, options: Option<Value>) -> ApiResult<()> {
    let was = enabled(&state.db, repo_id, product).await;
    sqlx::query(
        "INSERT INTO repo_security_products (repo_id, product, enabled, options) VALUES ($1, $2, $3, COALESCE($4, '{}'::jsonb))
         ON CONFLICT (repo_id, product) DO UPDATE SET enabled = $3, options = COALESCE($4, repo_security_products.options), updated_at = now()",
    )
    .bind(repo_id)
    .bind(product)
    .bind(on)
    .bind(options)
    .execute(&state.db)
    .await?;
    after_toggle(state, repo_id, product, on, !was).await;
    Ok(())
}

async fn after_toggle(state: &AppState, repo_id: i64, product: &str, on: bool, newly: bool) {
    if product == "dependabot_alerts" {
        let _ = sqlx::query(
            "INSERT INTO repo_settings (repo_id, vulnerability_alerts) VALUES ($1, $2)
             ON CONFLICT (repo_id) DO UPDATE SET vulnerability_alerts = $2, updated_at = now()",
        )
        .bind(repo_id)
        .bind(on)
        .execute(&state.db)
        .await;
        if on {
            enqueue(state, Q_DEPENDABOT, json!({ "repo_id": repo_id })).await;
        }
    }
    if product == "secret_scanning" && on && newly {
        secret_scanning::schedule_scan(state, repo_id, "backfill", None, None).await;
    }
}

/// `security_and_analysis` of a repository update (`PATCH /repos/{o}/{r}`):
/// `{"secret_scanning": {"status": "enabled"}, ...}`.
pub async fn apply_security_and_analysis(state: &AppState, ctx: &RepoCtx, v: &Value) -> ApiResult<()> {
    let Some(obj) = v.as_object() else { return Err(ApiError::validation("Repository", "security_and_analysis", "invalid")) };
    code_security::check_not_enforced(state, ctx.repo.id).await?;
    for (k, setting) in obj {
        let product = match k.as_str() {
            "advanced_security" | "code_security" | "secret_protection" => "advanced_security",
            "secret_scanning" | "secret_scanning_push_protection" | "secret_scanning_non_provider_patterns" | "secret_scanning_validity_checks"
            | "dependabot_security_updates" | "secret_scanning_ai_detection" => k.as_str(),
            _ => continue,
        };
        let on = match setting.get("status").and_then(Value::as_str) {
            Some("enabled") => true,
            Some("disabled") => false,
            _ => return Err(ApiError::validation("Repository", "security_and_analysis", "invalid")),
        };
        set_enabled(state, ctx.repo.id, product, on, None).await?;
    }
    Ok(())
}

/// `security_and_analysis` object of the repository JSON (admins only on GitHub).
pub async fn security_and_analysis_json(db: &PgPool, repo_id: i64) -> Value {
    let rows: Vec<(String, bool)> = sqlx::query_as("SELECT product, enabled FROM repo_security_products WHERE repo_id = $1")
        .bind(repo_id)
        .fetch_all(db)
        .await
        .unwrap_or_default();
    let get = |p: &str| rows.iter().find(|(x, _)| x == p).map(|(_, e)| *e).unwrap_or(false);
    let st = |b: bool| json!({ "status": if b { "enabled" } else { "disabled" } });
    let dependabot = enabled(db, repo_id, "dependabot_alerts").await;
    json!({
        "advanced_security": st(get("advanced_security")),
        "dependabot_security_updates": st(get("dependabot_security_updates") && dependabot),
        "secret_scanning": st(get("secret_scanning")),
        "secret_scanning_push_protection": st(get("secret_scanning_push_protection")),
        "secret_scanning_non_provider_patterns": st(get("secret_scanning_non_provider_patterns")),
        "secret_scanning_validity_checks": st(get("secret_scanning_validity_checks")),
        "secret_scanning_delegated_alert_dismissal": st(get("secret_scanning_delegated_alert_dismissal")),
        "secret_scanning_delegated_bypass": st(get("secret_scanning_delegated_bypass")),
    })
}

// ---------------------------------------------------------------------------------------------
// Access

/// Security product, for permission checks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Product {
    SecretScanning,
    Dependabot,
    CodeScanning,
}

impl Product {
    fn caps(self, write: bool) -> &'static [&'static str] {
        match (self, write) {
            (Product::SecretScanning, false) => &["read_secret_scanning", "write_secret_scanning"],
            (Product::SecretScanning, true) => &["write_secret_scanning"],
            (Product::Dependabot, false) => &["read_dependabot_alerts", "write_dependabot_alerts"],
            (Product::Dependabot, true) => &["write_dependabot_alerts"],
            (Product::CodeScanning, false) => &["read_code_scanning", "write_code_scanning"],
            (Product::CodeScanning, true) => &["write_code_scanning"],
        }
    }
}

/// Whether `u` holds one of `caps` in the org owning `ctx` (security managers do), or owns it.
pub async fn org_cap(db: &PgPool, ctx: &RepoCtx, u: &User, caps: &[&str]) -> bool {
    if ctx.owner.kind != "Organization" {
        return false;
    }
    if crate::orgs::is_owner(db, &ctx.owner, u).await.unwrap_or(false) {
        return true;
    }
    let have = crate::orgs::org_permissions(db, ctx.owner.id, u.id).await.unwrap_or_default();
    caps.iter().any(|c| have.iter().any(|h| h == c))
}

/// Loads a repository for a security product. Secret scanning needs admin, Dependabot write,
/// code scanning write (read for public repositories when only reading); org security managers
/// (and roles holding the product's fine-grained permissions) always qualify.
pub async fn repo_for(s: &AppState, u: &User, owner: &str, repo: &str, product: Product, write: bool) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    check_repo(s, &ctx, u, product, write).await?;
    Ok(ctx)
}

pub async fn check_repo(s: &AppState, ctx: &RepoCtx, u: &User, product: Product, write: bool) -> ApiResult<()> {
    let need = match (product, write) {
        (Product::SecretScanning, _) => Perm::Admin,
        (Product::Dependabot, _) => Perm::Write,
        (Product::CodeScanning, false) if !ctx.repo.private => Perm::Read,
        (Product::CodeScanning, _) => Perm::Write,
    };
    if ctx.perm >= need || org_cap(&s.db, ctx, u, product.caps(write)).await {
        return Ok(());
    }
    Err(ApiError::forbidden("Resource not accessible by personal access token"))
}

/// An organization for a security product's org-level lists (owners and security managers).
pub async fn org_for(s: &AppState, u: &User, org: &str, product: Product) -> ApiResult<User> {
    let o = crate::orgs::org_by_login(s, org).await?;
    crate::orgs::require_capability(s, &o, u, product.caps(false)).await?;
    Ok(o)
}

/// Org owners and security managers (configurations, pattern settings).
pub async fn require_security_manager(s: &AppState, org: &User, u: &User) -> ApiResult<()> {
    if crate::orgs::is_owner(&s.db, org, u).await? {
        return Ok(());
    }
    if crate::orgs::org_role(&s.db, org.id, u.id).await?.is_none() {
        return Err(ApiError::not_found());
    }
    if is_security_manager(&s.db, org.id, u.id).await {
        return Ok(());
    }
    Err(ApiError::forbidden("You must be an organization owner or security manager to perform this action."))
}

/// Holds the predefined `security_manager` organization role (directly or through a team).
pub async fn is_security_manager(db: &PgPool, org_id: i64, user_id: i64) -> bool {
    sqlx::query_scalar(
        "WITH RECURSIVE mt(team_id) AS (
             SELECT tm.team_id FROM team_members tm JOIN teams t ON t.id = tm.team_id WHERE t.org_id = $1 AND tm.user_id = $2
             UNION SELECT t.parent_id FROM mt JOIN teams t ON t.id = mt.team_id WHERE t.parent_id IS NOT NULL)
         SELECT EXISTS(SELECT 1 FROM org_role_users WHERE org_id = $1 AND user_id = $2 AND role_id = $3)
             OR EXISTS(SELECT 1 FROM org_role_teams WHERE org_id = $1 AND role_id = $3 AND team_id IN (SELECT team_id FROM mt))",
    )
    .bind(org_id)
    .bind(user_id)
    .bind(crate::api::orgs_roles::SECURITY_MANAGER_ROLE)
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

/// The enterprise (site administrators only).
pub fn enterprise_admin(u: &User, slug: &str) -> ApiResult<()> {
    crate::api::actions_settings::enterprise_admin(u, slug)
}

/// Repository ids an enterprise-level list covers: every org-owned repository.
pub const ENTERPRISE_REPOS: &str = "(SELECT r.id FROM repositories r JOIN users o ON o.id = r.owner_id WHERE o.kind = 'Organization')";

// ---------------------------------------------------------------------------------------------
// Shared helpers

/// Allocates the next per-repository number of an alert/request kind.
pub async fn next_number<'e, E: sqlx::PgExecutor<'e>>(db: E, repo_id: i64, kind: &str) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO advsec_counters (repo_id, kind, next) VALUES ($1, $2, 2)
         ON CONFLICT (repo_id, kind) DO UPDATE SET next = advsec_counters.next + 1 RETURNING next - 1",
    )
    .bind(repo_id)
    .bind(kind)
    .fetch_one(db)
    .await?)
}

/// Users by id (batch).
pub fn users_by_id(db: &PgPool, ids: impl IntoIterator<Item = i64>) -> impl std::future::Future<Output = ApiResult<HashMap<i64, User>>> + Send + '_ {
    // Collected eagerly so the returned future does not hold the (closure-bearing) iterator.
    users_by_id_vec(db, ids.into_iter().collect())
}

async fn users_by_id_vec(db: &PgPool, mut ids: Vec<i64>) -> ApiResult<HashMap<i64, User>> {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&ids).fetch_all(db).await?;
    Ok(rows.into_iter().map(|u| (u.id, u)).collect())
}

/// `simple-user` JSON of an optional user id.
pub fn user_json(s: &AppState, users: &HashMap<i64, User>, id: Option<i64>) -> Value {
    id.and_then(|i| users.get(&i)).map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).unwrap_or(Value::Null)
}

/// Repository contexts by id (batch; the owner is loaded too, permission is irrelevant).
pub fn repos_by_id(db: &PgPool, ids: impl IntoIterator<Item = i64>) -> impl std::future::Future<Output = ApiResult<HashMap<i64, RepoCtx>>> + Send + '_ {
    repos_by_id_vec(db, ids.into_iter().collect())
}

async fn repos_by_id_vec(db: &PgPool, mut ids: Vec<i64>) -> ApiResult<HashMap<i64, RepoCtx>> {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let repos: Vec<db::Repo> = sqlx::query_as("SELECT * FROM repositories WHERE id = ANY($1)").bind(&ids).fetch_all(db).await?;
    let owners = users_by_id(db, repos.iter().map(|r| r.owner_id)).await?;
    Ok(repos
        .into_iter()
        .filter_map(|r| {
            let owner = owners.get(&r.owner_id)?.clone();
            Some((r.id, RepoCtx { repo: r, owner, perm: Perm::Read }))
        })
        .collect())
}

/// `simple-repository` JSON.
pub fn repo_json(s: &AppState, ctx: &RepoCtx) -> Value {
    crate::json::repo::repo(&s.api_url(), s.base_url(), ctx)
}

/// RepoCtx for background work (admin view).
pub async fn system_ctx(db: &PgPool, repo_id: i64) -> ApiResult<RepoCtx> {
    let repo = db::repo_by_id(db, repo_id).await?;
    let owner = db::user_by_id(db, repo.owner_id).await?;
    Ok(RepoCtx { repo, owner, perm: Perm::Admin })
}

/// Sort direction from a query string value (default desc).
pub fn dir(d: Option<&str>) -> &'static str {
    if d == Some("asc") { "ASC" } else { "DESC" }
}

/// Splits a comma-separated filter.
pub fn csv(v: &Option<String>) -> Option<Vec<String>> {
    v.as_deref().filter(|x| !x.trim().is_empty()).map(|x| x.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
}

/// Validates an enum query parameter (422 like GitHub).
pub fn check_enum(resource: &str, field: &str, v: &Option<String>, allowed: &[&str]) -> ApiResult<()> {
    if let Some(list) = csv(v) {
        if list.iter().any(|x| !allowed.contains(&x.as_str())) {
            return Err(ApiError::validation(resource, field, "invalid"));
        }
    }
    Ok(())
}

/// Query-string helpers: with `#[serde(flatten)]` every query value arrives as a string, so
/// numeric and boolean fields of flattened query structs parse through these.
pub mod qs {
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        S(String),
        I(i64),
        B(bool),
    }

    pub fn opt_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        Ok(match Option::<V>::deserialize(d)? {
            Some(V::S(s)) => s.trim().parse().ok(),
            Some(V::I(i)) => Some(i),
            _ => None,
        })
    }

    pub fn opt_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
        Ok(match Option::<V>::deserialize(d)? {
            Some(V::S(s)) => match s.as_str() {
                "true" | "1" => Some(true),
                "false" | "0" => Some(false),
                _ => None,
            },
            Some(V::B(b)) => Some(b),
            _ => None,
        })
    }
}

/// Cursor/page parameters used by the alert lists: `page`/`per_page`, or `before`/`after`
/// cursors (opaque strings that are row offsets here).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CursorParams {
    #[serde(default, deserialize_with = "qs::opt_i64")]
    pub page: Option<i64>,
    #[serde(default, deserialize_with = "qs::opt_i64")]
    pub per_page: Option<i64>,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl CursorParams {
    pub fn per_page(&self) -> i64 {
        self.per_page.unwrap_or(30).clamp(1, 100)
    }
    pub fn offset(&self) -> i64 {
        if let Some(a) = self.after.as_deref().and_then(decode_cursor) {
            return a;
        }
        if let Some(b) = self.before.as_deref().and_then(decode_cursor) {
            return (b - self.per_page()).max(0);
        }
        (self.page.unwrap_or(1).clamp(1, 1_000_000) - 1) * self.per_page()
    }
    pub fn pages(&self) -> crate::api::PageParams {
        crate::api::PageParams { page: Some(self.offset() / self.per_page() + 1), per_page: Some(self.per_page()) }
    }
}

pub fn encode_cursor(offset: i64) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("o:{offset}"))
}

fn decode_cursor(c: &str) -> Option<i64> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(c).ok()?;
    String::from_utf8(raw).ok()?.strip_prefix("o:")?.parse().ok()
}

/// A JSON list with GitHub `Link` headers (page-based plus cursor `after`/`before` links).
pub fn list_response(s: &AppState, uri: &axum::extract::OriginalUri, cp: &CursorParams, total: i64, items: Vec<Value>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let offset = cp.offset();
    let per = cp.per_page();
    let mut res = crate::api::Paged::new(uri, s, cp.pages(), Some(total), items).into_response();
    if cp.after.is_some() || cp.before.is_some() {
        // Cursor pagination: replace the page links with cursor links.
        let base = format!("{}{}", s.base_url(), uri.0.path());
        let mut pairs: Vec<(String, String)> = uri
            .0
            .query()
            .map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect())
            .unwrap_or_default();
        pairs.retain(|(k, _)| k != "after" && k != "before" && k != "page");
        let link = |k: &str, v: String| {
            let mut p = pairs.clone();
            p.push((k.to_string(), v));
            format!("{base}?{}", url::form_urlencoded::Serializer::new(String::new()).extend_pairs(p).finish())
        };
        let mut links = Vec::new();
        if offset + per < total {
            links.push(format!("<{}>; rel=\"next\"", link("after", encode_cursor(offset + per))));
        }
        if offset > 0 {
            links.push(format!("<{}>; rel=\"prev\"", link("before", encode_cursor(offset))));
        }
        res.headers_mut().remove(axum::http::header::LINK);
        if let Ok(v) = axum::http::HeaderValue::from_str(&links.join(", ")) {
            if !links.is_empty() {
                res.headers_mut().insert(axum::http::header::LINK, v);
            }
        }
    }
    res
}
