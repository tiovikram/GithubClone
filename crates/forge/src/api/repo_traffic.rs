//! Repository traffic and community profile (api.github.com endpoints GHES lacks, used by the
//! web UI's Insights tab), plus small forge-only helpers for the repository settings UI.
//!
//! - `GET /repos/{o}/{r}/traffic/views|clones` (`?per=day|week`), `traffic/popular/referrers`,
//!   `traffic/popular/paths`: last 14 days, push access (as on github.com).
//! - `GET /repos/{o}/{r}/community/profile`: README / LICENSE / CODE_OF_CONDUCT / CONTRIBUTING /
//!   issue and pull request templates on the default branch.
//! - `POST /api/forge/repos/{o}/{r}/traffic/view` (`{path, title, referrer}`): the web UI reports
//!   a page view (the SPA renders client-side, so the server cannot see page loads).
//!   Clones are recorded by the git transports ([`record_clone`]).
//! - `GET /api/forge/repos/{o}/{r}/lfs` (`{enabled}`): the LFS flag set by `PUT/DELETE /lfs`.
//! - `GET /api/forge/enterprise` (`{slug}`): the enterprise slug used in `/enterprises/{e}` URLs.
use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}";
    Router::new()
        .route(&format!("{r}/traffic/views"), get(views))
        .route(&format!("{r}/traffic/clones"), get(clones))
        .route(&format!("{r}/traffic/popular/referrers"), get(referrers))
        .route(&format!("{r}/traffic/popular/paths"), get(paths))
        .route(&format!("{r}/community/profile"), get(community))
}

/// Mounted under `/api/forge` (behind its CSRF check).
pub fn forge_routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/traffic/view", post(record_view_h))
        .route("/repos/{owner}/{repo}/lfs", get(lfs_state))
        .route("/enterprise", get(|| async { Json(json!({ "slug": crate::enterprise::slug() })) }))
}

// ---------------------------------------------------------------------------------------------
// Recording

const KEEP_DAYS: i64 = 15;

fn visitor(user: Option<&User>, ip: Option<std::net::IpAddr>) -> String {
    match (user, ip) {
        (Some(u), _) => format!("u:{}", u.id),
        (None, Some(ip)) => format!("ip:{ip}"),
        (None, None) => "anonymous".into(),
    }
}

async fn insert(s: &AppState, repo_id: i64, kind: &str, who: &str, path: Option<&str>, title: Option<&str>, referrer: Option<&str>) -> ApiResult<()> {
    sqlx::query("INSERT INTO repo_traffic (repo_id, kind, visitor, path, title, referrer) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(repo_id)
        .bind(kind)
        .bind(who)
        .bind(path)
        .bind(title)
        .bind(referrer)
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM repo_traffic WHERE repo_id = $1 AND kind = $2 AND at < now() - make_interval(days => $3::int)")
        .bind(repo_id)
        .bind(kind)
        .bind(KEEP_DAYS as i32)
        .execute(&s.db)
        .await?;
    Ok(())
}

/// Records a git clone/fetch of `repo_id` (an upload-pack ref advertisement). Never fails.
pub async fn record_clone(s: &AppState, repo_id: i64, user: Option<&User>, ip: Option<std::net::IpAddr>) {
    if let Err(e) = insert(s, repo_id, "clone", &visitor(user, ip), None, None, None).await {
        tracing::warn!(error = ?e, "recording clone traffic failed");
    }
}

#[derive(Deserialize)]
struct ViewBody {
    path: Option<String>,
    title: Option<String>,
    referrer: Option<String>,
}

/// Host of a referrer URL ("https://www.google.com/x" -> "google.com"); None for this site.
fn referrer_host(s: &AppState, r: &str) -> Option<String> {
    let u = url::Url::parse(r).ok()?;
    let host = u.host_str()?.trim_start_matches("www.").to_ascii_lowercase();
    let own = url::Url::parse(s.base_url()).ok().and_then(|b| b.host_str().map(str::to_ascii_lowercase));
    (own.as_deref() != Some(host.as_str()) && !host.is_empty()).then_some(host)
}

async fn record_view_h(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    req_parts: axum::http::request::Parts,
    Json(b): Json<ViewBody>,
) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let base = format!("/{}/{}", ctx.owner.login, ctx.repo.name);
    let path = b.path.unwrap_or_else(|| base.clone());
    let lower = path.to_ascii_lowercase();
    let base_l = base.to_ascii_lowercase();
    if !(lower == base_l || lower.starts_with(&format!("{base_l}/"))) || path.len() > 1024 {
        return Err(ApiError::validation("Traffic", "path", "invalid"));
    }
    let title: Option<String> = b.title.map(|t| t.chars().take(200).collect());
    let referrer = b.referrer.as_deref().and_then(|r| referrer_host(&s, r));
    let who = visitor(u.as_ref(), crate::security::client_ip(&req_parts));
    insert(&s, ctx.repo.id, "view", &who, Some(&path), title.as_deref(), referrer.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn lfs_state(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let st = super::repos_settings::settings(&s, ctx.repo.id).await?;
    Ok(Json(json!({ "enabled": st.lfs_enabled })))
}

// ---------------------------------------------------------------------------------------------
// Traffic endpoints

async fn push_ctx(s: &AppState, u: &User, owner: &str, repo: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    if ctx.perm < Perm::Write {
        return Err(ApiError::forbidden("Must have push access to repository"));
    }
    Ok(ctx)
}

#[derive(Deserialize)]
struct PerQuery {
    per: Option<String>,
}

fn bucket(t: DateTime<Utc>, week: bool) -> DateTime<Utc> {
    let d = t.date_naive();
    let d = if week { d - Duration::days(d.weekday().num_days_from_monday() as i64) } else { d };
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
}

async fn counts(s: &AppState, ctx: &RepoCtx, kind: &str, per: Option<&str>) -> ApiResult<Value> {
    let week = match per.unwrap_or("day") {
        "day" => false,
        "week" => true,
        _ => return Err(ApiError::validation("Traffic", "per", "invalid")),
    };
    let rows: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT visitor, at FROM repo_traffic WHERE repo_id = $1 AND kind = $2 AND at >= date_trunc('day', now()) - interval '14 days'",
    )
    .bind(ctx.repo.id)
    .bind(kind)
    .fetch_all(&s.db)
    .await?;
    let now = Utc::now();
    let mut buckets: BTreeMap<DateTime<Utc>, (i64, HashSet<String>)> = BTreeMap::new();
    // Every bucket of the window, including empty ones (as github.com reports them).
    let mut d = bucket(now - Duration::days(14), week);
    while d <= now {
        buckets.entry(d).or_default();
        d += Duration::days(if week { 7 } else { 1 });
    }
    let mut all: HashSet<String> = HashSet::new();
    for (v, at) in &rows {
        let e = buckets.entry(bucket(*at, week)).or_default();
        e.0 += 1;
        e.1.insert(v.clone());
        all.insert(v.clone());
    }
    let key = if kind == "view" { "views" } else { "clones" };
    let list: Vec<Value> = buckets
        .into_iter()
        .map(|(t, (c, u))| json!({ "timestamp": crate::json::ts(t), "count": c, "uniques": u.len() }))
        .collect();
    Ok(json!({ "count": rows.len(), "uniques": all.len(), key: list }))
}

async fn views(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Query(q): Query<PerQuery>) -> ApiResult<Json<Value>> {
    let ctx = push_ctx(&s, &u, &o, &r).await?;
    Ok(Json(counts(&s, &ctx, "view", q.per.as_deref()).await?))
}

async fn clones(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Query(q): Query<PerQuery>) -> ApiResult<Json<Value>> {
    let ctx = push_ctx(&s, &u, &o, &r).await?;
    Ok(Json(counts(&s, &ctx, "clone", q.per.as_deref()).await?))
}

async fn referrers(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = push_ctx(&s, &u, &o, &r).await?;
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT referrer, count(*), count(DISTINCT visitor) FROM repo_traffic
          WHERE repo_id = $1 AND kind = 'view' AND referrer IS NOT NULL AND at >= now() - interval '14 days'
          GROUP BY referrer ORDER BY count(*) DESC, referrer LIMIT 10",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    Ok(Json(json!(rows.into_iter().map(|(r, c, u)| json!({"referrer": r, "count": c, "uniques": u})).collect::<Vec<_>>())))
}

async fn paths(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = push_ctx(&s, &u, &o, &r).await?;
    let rows: Vec<(String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT path, (array_agg(title ORDER BY at DESC) FILTER (WHERE title IS NOT NULL AND title <> ''))[1], count(*), count(DISTINCT visitor) FROM repo_traffic
          WHERE repo_id = $1 AND kind = 'view' AND path IS NOT NULL AND at >= now() - interval '14 days'
          GROUP BY path ORDER BY count(*) DESC, path LIMIT 10",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    let full = ctx.full_name();
    Ok(Json(json!(rows
        .into_iter()
        .map(|(p, t, c, u)| json!({"path": p, "title": t.filter(|t| !t.is_empty()).unwrap_or_else(|| full.clone()), "count": c, "uniques": u}))
        .collect::<Vec<_>>())))
}

// ---------------------------------------------------------------------------------------------
// Community profile

/// Paths on the default branch (root, `.github/`, `docs/`), lowercased -> real path.
async fn community_files(s: &AppState, ctx: &RepoCtx) -> ApiResult<HashMap<String, String>> {
    let br = ctx.repo.default_branch.clone();
    if !crate::git::read::safe_rev(&br) {
        return Ok(HashMap::new());
    }
    let mut out = HashMap::new();
    for args in [
        vec!["ls-tree", "--name-only", "--end-of-options", br.as_str()],
        vec!["ls-tree", "-r", "--name-only", "--end-of-options", br.as_str(), "--", ".github", "docs"],
    ] {
        if let Some(o) = super::plat::git_read(s, ctx.repo.id, &args).await? {
            for l in String::from_utf8_lossy(&o).lines() {
                out.insert(l.to_ascii_lowercase(), l.to_string());
            }
        }
    }
    // `.github/ISSUE_TEMPLATE` directory itself (ls-tree -r lists only its files).
    Ok(out)
}

fn find<'a>(files: &'a HashMap<String, String>, stems: &[&str]) -> Option<&'a String> {
    for dir in ["", ".github/", "docs/"] {
        for stem in stems {
            for ext in ["", ".md", ".markdown", ".txt", ".rst", ".adoc", ".org"] {
                if let Some(p) = files.get(&format!("{dir}{stem}{ext}")) {
                    return Some(p);
                }
            }
        }
    }
    None
}

async fn community(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let files = community_files(&s, &ctx).await?;
    let api = s.api_url();
    let base = s.base_url();
    let full = ctx.full_name();
    let br = &ctx.repo.default_branch;
    let file = |p: &String| json!({ "url": format!("{api}/repos/{full}/contents/{p}"), "html_url": format!("{base}/{full}/blob/{br}/{p}") });
    let readme = find(&files, &["readme"]).map(file);
    let contributing = find(&files, &["contributing"]).map(file);
    let coc_path = find(&files, &["code_of_conduct", "code-of-conduct"]);
    let coc_file = coc_path.map(file);
    let coc = coc_path.map(|p| {
        json!({ "key": "other", "name": "Other", "url": format!("{api}/repos/{full}/community/code_of_conduct"), "html_url": format!("{base}/{full}/blob/{br}/{p}") })
    });
    let issue_tpl = find(&files, &["issue_template"])
        .or_else(|| files.iter().filter(|(k, _)| k.starts_with(".github/issue_template/") || k.starts_with("docs/issue_template/")).map(|(_, v)| v).min())
        .map(file);
    let pr_tpl = find(&files, &["pull_request_template"])
        .or_else(|| files.iter().filter(|(k, _)| k.starts_with(".github/pull_request_template/")).map(|(_, v)| v).min())
        .map(file);
    let license = match super::repo_extras::find_license(&s, &ctx, br).await? {
        Some((path, _, _, _, mut lic)) => {
            lic["html_url"] = json!(format!("{base}/{full}/blob/{br}/{path}"));
            Some(lic)
        }
        None => None,
    };
    let description = ctx.repo.description.clone().filter(|d| !d.trim().is_empty());
    let checks = [description.is_some(), readme.is_some(), coc.is_some(), contributing.is_some(), license.is_some(), issue_tpl.is_some(), pr_tpl.is_some()];
    let health = (checks.iter().filter(|c| **c).count() * 100 / checks.len()) as i64;
    Ok(Json(json!({
        "health_percentage": health,
        "description": description,
        "documentation": ctx.repo.homepage.clone().filter(|h| !h.is_empty()),
        "files": {
            "code_of_conduct": coc,
            "code_of_conduct_file": coc_file,
            "contributing": contributing,
            "issue_template": issue_tpl,
            "pull_request_template": pr_tpl,
            "license": license,
            "readme": readme,
        },
        "updated_at": crate::json::ts(ctx.repo.updated_at),
        "content_reports_enabled": false,
    })))
}
