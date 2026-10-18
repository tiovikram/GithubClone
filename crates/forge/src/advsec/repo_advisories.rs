//! Repository security advisories and private vulnerability reporting (api.github.com's
//! repository `security-advisories` endpoints; GHES 3.22 has no REST surface for them, the web
//! UI uses these):
//!
//! - `GET/POST /repos/{o}/{r}/security-advisories`, `GET/PATCH .../{ghsa_id}`,
//!   `POST .../{ghsa_id}/cve` (forge is no CNA: the request is recorded), `POST .../reports`
//!   (private vulnerability report, state `triage`), `GET /orgs/{org}/security-advisories`;
//! - `GET/PUT/DELETE /repos/{o}/{r}/private-vulnerability-reporting` (the
//!   `private_vulnerability_reporting` security product).
//!
//! Who sees what: published advisories are public to everyone who can read the repository;
//! drafts, triage reports and closed advisories only to repository admins, organization owners
//! and security managers ("managers"), the author (reporter) and collaborating users. Managers
//! create and edit advisories and change their state; collaborators may edit the content.
//! States: `triage` → `draft` (accept) or `closed` (reject), `draft` → `published` or
//! `closed`, `closed` → `draft` (reopen). Publishing imports the advisory into the global
//! advisory database (`GET /advisories/{ghsa_id}`), so Dependabot matches it.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::CursorParams;
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/security-advisories";
    Router::new()
        .route(r, get(list).post(create))
        .route(&format!("{r}/reports"), post(report))
        .route(&format!("{r}/{{ghsa_id}}"), get(get_one).patch(update))
        .route(&format!("{r}/{{ghsa_id}}/cve"), post(request_cve))
        .route(&format!("{r}/{{ghsa_id}}/forks"), post(create_fork))
        .route("/orgs/{org}/security-advisories", get(org_list))
        .route("/repos/{owner}/{repo}/private-vulnerability-reporting", get(pvr_get).put(pvr_enable).delete(pvr_disable))
}

const STATES: &[&str] = &["triage", "draft", "published", "closed", "withdrawn"];
const SEVERITIES: &[&str] = &["critical", "high", "medium", "low"];
const ECOSYSTEMS: &[&str] = &["rubygems", "npm", "pip", "maven", "nuget", "composer", "go", "rust", "erlang", "actions", "pub", "other", "swift"];
const CREDIT_TYPES: &[&str] = &["analyst", "finder", "reporter", "coordinator", "remediation_developer", "remediation_reviewer", "remediation_verifier", "tool", "sponsor", "other"];

#[derive(Debug, Clone, sqlx::FromRow)]
struct Row {
    id: i64,
    repo_id: i64,
    ghsa_id: String,
    cve_id: Option<String>,
    summary: String,
    description: Option<String>,
    severity: Option<String>,
    cvss_vector: Option<String>,
    cvss_score: Option<f64>,
    cwe_ids: Vec<String>,
    vulnerabilities: Value,
    state: String,
    author_id: Option<i64>,
    publisher_id: Option<i64>,
    is_report: bool,
    report_accepted: bool,
    collaborating_users: Vec<i64>,
    collaborating_teams: Vec<i64>,
    cve_requested_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    published_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    withdrawn_at: Option<DateTime<Utc>>,
}

/// GHSA ids use GitHub's alphabet (no vowels or easily confused characters).
fn new_ghsa() -> String {
    use rand::RngExt;
    const A: &[u8] = b"23456789cfghjmpqrvwx";
    let mut rng = rand::rng();
    let mut g = || (0..4).map(|_| A[rng.random_range(0..A.len())] as char).collect::<String>();
    format!("GHSA-{}-{}-{}", g(), g(), g())
}

/// Repository admins, organization owners and security managers.
async fn is_manager(s: &AppState, ctx: &RepoCtx, u: &User) -> bool {
    if ctx.perm >= Perm::Admin || u.site_admin {
        return true;
    }
    if ctx.owner.kind == "Organization" {
        if crate::orgs::is_owner(&s.db, &ctx.owner, u).await.unwrap_or(false) {
            return true;
        }
        return super::is_security_manager(&s.db, ctx.owner.id, u.id).await;
    }
    false
}

async fn is_collaborator(s: &AppState, a: &Row, u: &User) -> bool {
    if a.author_id == Some(u.id) || a.collaborating_users.contains(&u.id) {
        return true;
    }
    if a.collaborating_teams.is_empty() {
        return false;
    }
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM team_members WHERE team_id = ANY($1) AND user_id = $2)")
        .bind(&a.collaborating_teams)
        .bind(u.id)
        .fetch_one(&s.db)
        .await
        .unwrap_or(false)
}

async fn can_see(s: &AppState, ctx: &RepoCtx, a: &Row, u: Option<&User>) -> bool {
    if a.state == "published" || a.state == "withdrawn" {
        return ctx.perm >= Perm::Read || !ctx.repo.private;
    }
    match u {
        Some(u) => is_manager(s, ctx, u).await || is_collaborator(s, a, u).await,
        None => false,
    }
}

async fn load(s: &AppState, ctx: &RepoCtx, ghsa: &str) -> ApiResult<Row> {
    sqlx::query_as("SELECT * FROM repository_advisories WHERE repo_id = $1 AND lower(ghsa_id) = lower($2)")
        .bind(ctx.repo.id)
        .bind(ghsa)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn advisory_json(s: &AppState, ctx: &RepoCtx, a: &Row) -> ApiResult<Value> {
    let full = ctx.full_name();
    let mut ids: Vec<i64> = a.author_id.into_iter().chain(a.publisher_id).collect();
    ids.extend(&a.collaborating_users);
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    let simple = |id: Option<i64>| id.and_then(|i| users.get(&i)).map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).unwrap_or(Value::Null);
    let credits: Vec<(i64, String, String)> = sqlx::query_as("SELECT user_id, kind, state FROM repository_advisory_credits WHERE advisory_id = $1 ORDER BY user_id")
        .bind(a.id)
        .fetch_all(&s.db)
        .await?;
    let credit_users = crate::json::issue::users_by_ids(&s.db, &credits.iter().map(|c| c.0).collect::<Vec<_>>()).await?;
    let credits_detailed: Vec<Value> = credits
        .iter()
        .filter_map(|(id, kind, state)| {
            credit_users.get(id).map(|u| json!({ "user": crate::json::user::simple(&s.api_url(), s.base_url(), u), "type": kind, "state": state }))
        })
        .collect();
    let credits_simple: Vec<Value> = credits
        .iter()
        .filter_map(|(id, kind, _)| credit_users.get(id).map(|u| json!({ "login": u.login, "type": kind })))
        .collect();
    let teams: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, name, slug FROM teams WHERE id = ANY($1) ORDER BY id")
        .bind(&a.collaborating_teams)
        .fetch_all(&s.db)
        .await
        .unwrap_or_default();
    let mut identifiers = vec![json!({ "type": "GHSA", "value": a.ghsa_id })];
    if let Some(c) = &a.cve_id {
        identifiers.push(json!({ "type": "CVE", "value": c }));
    }
    let cwes: Vec<Value> = a.cwe_ids.iter().map(|c| json!({ "cwe_id": c, "name": super::advisories::cwe_name(c) })).collect();
    Ok(json!({
        "ghsa_id": a.ghsa_id,
        "cve_id": a.cve_id,
        "url": format!("{}/repos/{full}/security-advisories/{}", s.api_url(), a.ghsa_id),
        "html_url": format!("{}/{full}/security/advisories/{}", s.base_url(), a.ghsa_id),
        "summary": a.summary,
        "description": a.description,
        "severity": a.severity,
        "author": simple(a.author_id),
        "publisher": simple(a.publisher_id),
        "identifiers": identifiers,
        "state": a.state,
        "created_at": crate::json::ts(a.created_at),
        "updated_at": crate::json::ts(a.updated_at),
        "published_at": crate::json::ts_opt(a.published_at),
        "closed_at": crate::json::ts_opt(a.closed_at),
        "withdrawn_at": crate::json::ts_opt(a.withdrawn_at),
        "submission": if a.is_report { json!({ "accepted": a.report_accepted }) } else { Value::Null },
        "vulnerabilities": a.vulnerabilities,
        "cvss": if a.cvss_vector.is_some() { json!({ "vector_string": a.cvss_vector, "score": a.cvss_score }) } else { Value::Null },
        "cvss_severities": {
            "cvss_v3": { "vector_string": a.cvss_vector, "score": a.cvss_score },
            "cvss_v4": { "vector_string": null, "score": null },
        },
        "cwes": cwes,
        "cwe_ids": a.cwe_ids,
        "credits": credits_simple,
        "credits_detailed": credits_detailed,
        "collaborating_users": a.collaborating_users.iter().filter_map(|i| users.get(i)).map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).collect::<Vec<_>>(),
        "collaborating_teams": teams.iter().map(|(id, name, slug)| json!({
            "id": id, "name": name, "slug": slug,
            "html_url": format!("{}/orgs/{}/teams/{slug}", s.base_url(), ctx.owner.login),
        })).collect::<Vec<_>>(),
        "private_fork": null,
        // forge extension: a CVE was requested (forge is not a CNA, so nobody assigns one).
        "cve_requested_at": crate::json::ts_opt(a.cve_requested_at),
    }))
}

// ---------------------------------------------------------------------------------------------
// Body validation

/// Validated content fields of a create / update body.
#[derive(Default)]
struct Fields {
    summary: Option<String>,
    description: Option<Option<String>>,
    cve_id: Option<Option<String>>,
    severity: Option<Option<String>>,
    cvss: Option<Option<(String, Option<f64>)>>,
    cwe_ids: Option<Vec<String>>,
    vulnerabilities: Option<Value>,
    credits: Option<Vec<(i64, String)>>,
    collaborating_users: Option<Vec<i64>>,
    collaborating_teams: Option<Vec<i64>>,
}

fn bad(field: &str) -> ApiError {
    ApiError::validation("RepositoryAdvisory", field, "invalid")
}

async fn parse_fields(s: &AppState, ctx: &RepoCtx, b: &Map<String, Value>) -> ApiResult<Fields> {
    let mut f = Fields::default();
    let opt_str = |k: &str| -> ApiResult<Option<Option<String>>> {
        match b.get(k) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::String(v)) => Ok(Some(Some(v.trim().to_string()).filter(|v| !v.is_empty()))),
            Some(_) => Err(bad(k)),
        }
    };
    if let Some(v) = opt_str("summary")? {
        let v = v.unwrap_or_default();
        if v.chars().count() > 1024 {
            return Err(bad("summary"));
        }
        f.summary = Some(v);
    }
    if let Some(v) = opt_str("description")? {
        if v.as_ref().is_some_and(|d| d.len() > 65535) {
            return Err(bad("description"));
        }
        f.description = Some(v);
    }
    if let Some(v) = opt_str("cve_id")? {
        if v.as_ref().is_some_and(|c| !regex_lite_cve(c)) {
            return Err(bad("cve_id"));
        }
        f.cve_id = Some(v);
    }
    if let Some(v) = opt_str("severity")? {
        if v.as_ref().is_some_and(|x| !SEVERITIES.contains(&x.as_str())) {
            return Err(bad("severity"));
        }
        f.severity = Some(v);
    }
    if let Some(v) = opt_str("cvss_vector_string")? {
        match v {
            None => f.cvss = Some(None),
            Some(vec) => {
                let score = super::advisories::cvss3_score(&vec);
                if score.is_none() {
                    return Err(bad("cvss_vector_string"));
                }
                f.cvss = Some(Some((vec, score)));
            }
        }
    }
    if matches!((&f.severity, &f.cvss), (Some(Some(_)), Some(Some(_)))) {
        return Err(ApiError::unprocessable("You must choose between setting severity or cvss_vector_string."));
    }
    match b.get("cwe_ids") {
        None => {}
        Some(Value::Null) => f.cwe_ids = Some(vec![]),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for c in a {
                let c = c.as_str().ok_or_else(|| bad("cwe_ids"))?.trim().to_uppercase();
                if !c.strip_prefix("CWE-").is_some_and(|n| !n.is_empty() && n.chars().all(|d| d.is_ascii_digit())) {
                    return Err(bad("cwe_ids"));
                }
                out.push(c);
            }
            f.cwe_ids = Some(out);
        }
        Some(_) => return Err(bad("cwe_ids")),
    }
    match b.get("vulnerabilities") {
        None => {}
        Some(Value::Null) => f.vulnerabilities = Some(json!([])),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for v in a {
                let eco = v.pointer("/package/ecosystem").and_then(Value::as_str).ok_or_else(|| bad("vulnerabilities"))?;
                if !ECOSYSTEMS.contains(&eco) {
                    return Err(bad("vulnerabilities"));
                }
                let s_or_null = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).map(|x| json!(x)).unwrap_or(Value::Null);
                let funcs: Vec<Value> = v.get("vulnerable_functions").and_then(Value::as_array).into_iter().flatten().filter(|x| x.is_string()).cloned().collect();
                out.push(json!({
                    "package": { "ecosystem": eco, "name": v.pointer("/package/name").and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()) },
                    "vulnerable_version_range": s_or_null("vulnerable_version_range"),
                    "patched_versions": s_or_null("patched_versions"),
                    "vulnerable_functions": funcs,
                }));
            }
            f.vulnerabilities = Some(Value::Array(out));
        }
        Some(_) => return Err(bad("vulnerabilities")),
    }
    match b.get("credits") {
        None => {}
        Some(Value::Null) => f.credits = Some(vec![]),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for c in a {
                let login = c.get("login").and_then(Value::as_str).ok_or_else(|| bad("credits"))?;
                let kind = c.get("type").and_then(Value::as_str).ok_or_else(|| bad("credits"))?;
                if !CREDIT_TYPES.contains(&kind) {
                    return Err(bad("credits"));
                }
                let u = db::user_by_login(&s.db, login).await.map_err(|_| ApiError::validation("RepositoryAdvisory", "credits", "invalid"))?;
                out.retain(|(id, _)| *id != u.id);
                out.push((u.id, kind.to_string()));
            }
            f.credits = Some(out);
        }
        Some(_) => return Err(bad("credits")),
    }
    match b.get("collaborating_users") {
        None => {}
        Some(Value::Null) => f.collaborating_users = Some(vec![]),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for l in a {
                let l = l.as_str().ok_or_else(|| bad("collaborating_users"))?;
                let u = db::user_by_login(&s.db, l).await.map_err(|_| bad("collaborating_users"))?;
                if !out.contains(&u.id) {
                    out.push(u.id);
                }
            }
            f.collaborating_users = Some(out);
        }
        Some(_) => return Err(bad("collaborating_users")),
    }
    match b.get("collaborating_teams") {
        None => {}
        Some(Value::Null) => f.collaborating_teams = Some(vec![]),
        Some(Value::Array(a)) => {
            if ctx.owner.kind != "Organization" && !a.is_empty() {
                return Err(bad("collaborating_teams"));
            }
            let mut out = Vec::new();
            for t in a {
                let slug = t.as_str().ok_or_else(|| bad("collaborating_teams"))?;
                let id: Option<i64> = sqlx::query_scalar("SELECT id FROM teams WHERE org_id = $1 AND slug = $2")
                    .bind(ctx.owner.id)
                    .bind(slug)
                    .fetch_optional(&s.db)
                    .await?;
                out.push(id.ok_or_else(|| bad("collaborating_teams"))?);
            }
            f.collaborating_teams = Some(out);
        }
        Some(_) => return Err(bad("collaborating_teams")),
    }
    Ok(f)
}

fn regex_lite_cve(c: &str) -> bool {
    let p: Vec<&str> = c.split('-').collect();
    p.len() == 3 && p[0] == "CVE" && p[1].len() == 4 && p[1].chars().all(|d| d.is_ascii_digit()) && p[2].len() >= 4 && p[2].chars().all(|d| d.is_ascii_digit())
}

async fn apply_fields(s: &AppState, id: i64, f: &Fields, author_id: Option<i64>) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    if let Some(v) = &f.summary {
        sqlx::query("UPDATE repository_advisories SET summary = $2 WHERE id = $1").bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(v) = &f.description {
        sqlx::query("UPDATE repository_advisories SET description = $2 WHERE id = $1").bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(v) = &f.cve_id {
        sqlx::query("UPDATE repository_advisories SET cve_id = $2 WHERE id = $1").bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(v) = &f.severity {
        // An explicit severity replaces the CVSS vector (GitHub: one or the other).
        sqlx::query("UPDATE repository_advisories SET severity = $2, cvss_vector = CASE WHEN $2 IS NULL THEN cvss_vector END, cvss_score = CASE WHEN $2 IS NULL THEN cvss_score END WHERE id = $1")
            .bind(id)
            .bind(v)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(v) = &f.cvss {
        let sev = v.as_ref().and_then(|(_, sc)| *sc).map(super::advisories::severity_from_score).filter(|x| *x != "unknown").map(|x| if x == "moderate" { "medium" } else { x });
        sqlx::query("UPDATE repository_advisories SET cvss_vector = $2, cvss_score = $3, severity = COALESCE($4, CASE WHEN $2 IS NULL THEN severity END) WHERE id = $1")
            .bind(id)
            .bind(v.as_ref().map(|x| x.0.clone()))
            .bind(v.as_ref().and_then(|x| x.1))
            .bind(sev)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(v) = &f.cwe_ids {
        sqlx::query("UPDATE repository_advisories SET cwe_ids = $2 WHERE id = $1").bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(v) = &f.vulnerabilities {
        sqlx::query("UPDATE repository_advisories SET vulnerabilities = $2 WHERE id = $1").bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(v) = &f.collaborating_users {
        sqlx::query("UPDATE repository_advisories SET collaborating_users = $2 WHERE id = $1").bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(v) = &f.collaborating_teams {
        sqlx::query("UPDATE repository_advisories SET collaborating_teams = $2 WHERE id = $1").bind(id).bind(v).execute(&mut *tx).await?;
    }
    if let Some(credits) = &f.credits {
        let ids: Vec<i64> = credits.iter().map(|c| c.0).collect();
        sqlx::query("DELETE FROM repository_advisory_credits WHERE advisory_id = $1 AND NOT (user_id = ANY($2))").bind(id).bind(&ids).execute(&mut *tx).await?;
        for (uid, kind) in credits {
            // Crediting yourself needs no acceptance.
            let state = if Some(*uid) == author_id { "accepted" } else { "pending" };
            sqlx::query(
                "INSERT INTO repository_advisory_credits (advisory_id, user_id, kind, state) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (advisory_id, user_id) DO UPDATE SET kind = $3",
            )
            .bind(id)
            .bind(uid)
            .bind(kind)
            .bind(state)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query("UPDATE repository_advisories SET updated_at = now() WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn insert(s: &AppState, ctx: &RepoCtx, u: &User, state: &str, is_report: bool) -> ApiResult<i64> {
    for _ in 0..5 {
        let ghsa = new_ghsa();
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO repository_advisories (repo_id, ghsa_id, state, author_id, is_report) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (ghsa_id) DO NOTHING RETURNING id",
        )
        .bind(ctx.repo.id)
        .bind(&ghsa)
        .bind(state)
        .bind(u.id)
        .bind(is_report)
        .fetch_optional(&s.db)
        .await?;
        if let Some(id) = id {
            return Ok(id);
        }
    }
    Err(ApiError::internal("could not allocate a GHSA id"))
}

async fn by_id(s: &AppState, id: i64) -> ApiResult<Row> {
    Ok(sqlx::query_as("SELECT * FROM repository_advisories WHERE id = $1").bind(id).fetch_one(&s.db).await?)
}

/// The OSV record of a published advisory, imported into the global advisory database.
fn osv_record(ctx: &RepoCtx, s: &AppState, a: &Row) -> Value {
    let osv_eco = |e: &str| match e {
        "npm" => "npm",
        "pip" => "PyPI",
        "rust" => "crates.io",
        "go" => "Go",
        "rubygems" => "RubyGems",
        "maven" => "Maven",
        "nuget" => "NuGet",
        "composer" => "Packagist",
        "erlang" => "Hex",
        "pub" => "Pub",
        "swift" => "SwiftURL",
        "actions" => "GitHub Actions",
        _ => "other",
    };
    let affected: Vec<Value> = a
        .vulnerabilities
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let name = v.pointer("/package/name").and_then(Value::as_str)?;
            let eco = v.pointer("/package/ecosystem").and_then(Value::as_str).unwrap_or("other");
            let range = v.get("vulnerable_version_range").and_then(Value::as_str).unwrap_or("");
            let mut iv = super::advisories::parse_github_range(range);
            if iv.fixed.is_none() && iv.last_affected.is_none() {
                iv.fixed = v.get("patched_versions").and_then(Value::as_str).map(|p| p.split(',').next().unwrap_or(p).trim().to_string()).filter(|p| !p.is_empty());
            }
            let mut events = vec![json!({ "introduced": iv.introduced.clone().unwrap_or_else(|| "0".into()) })];
            if let Some(f) = &iv.fixed {
                events.push(json!({ "fixed": f }));
            } else if let Some(l) = &iv.last_affected {
                events.push(json!({ "last_affected": l }));
            }
            let funcs = v.get("vulnerable_functions").cloned().unwrap_or(json!([]));
            Some(json!({
                "package": { "ecosystem": osv_eco(eco), "name": name },
                "ranges": [{ "type": "ECOSYSTEM", "events": events }],
                "ecosystem_specific": { "affected_functions": funcs },
            }))
        })
        .collect();
    let mut severity = vec![];
    if let Some(v) = &a.cvss_vector {
        severity.push(json!({ "type": "CVSS_V3", "score": v }));
    }
    let now = crate::json::ts(Utc::now());
    json!({
        "schema_version": "1.4.0",
        "id": a.ghsa_id,
        "modified": now,
        "published": crate::json::ts_opt(a.published_at),
        "aliases": a.cve_id.iter().collect::<Vec<_>>(),
        "summary": a.summary,
        "details": a.description,
        "severity": severity,
        "affected": affected,
        "references": [
            { "type": "WEB", "url": format!("{}/{}/security/advisories/{}", s.base_url(), ctx.full_name(), a.ghsa_id) },
            { "type": "PACKAGE", "url": format!("{}/{}", s.base_url(), ctx.full_name()) },
        ],
        "database_specific": {
            "cwe_ids": a.cwe_ids,
            "severity": a.severity.as_deref().map(|x| if x == "medium" { "MODERATE".to_string() } else { x.to_uppercase() }),
            "github_reviewed": true,
            "github_reviewed_at": now,
        },
    })
}

// ---------------------------------------------------------------------------------------------
// REST

#[derive(Deserialize, Default)]
struct ListQ {
    state: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    #[serde(flatten)]
    cursor: CursorParams,
}

fn order(q: &ListQ) -> ApiResult<String> {
    let col = match q.sort.as_deref().unwrap_or("created") {
        "created" => "created_at",
        "updated" => "updated_at",
        "published" => "published_at",
        _ => return Err(bad("sort")),
    };
    let dir = match q.direction.as_deref().unwrap_or("desc") {
        "asc" => "ASC",
        "desc" => "DESC",
        _ => return Err(bad("direction")),
    };
    Ok(format!("{col} {dir} NULLS LAST, id {dir}"))
}

async fn visible_page(s: &AppState, rows: Vec<Row>, ctxs: &[RepoCtx], u: Option<&User>, q: &ListQ, uri: &OriginalUri) -> ApiResult<Response> {
    let mut vis = Vec::new();
    for a in rows {
        let Some(ctx) = ctxs.iter().find(|c| c.repo.id == a.repo_id) else { continue };
        if can_see(s, ctx, &a, u).await {
            vis.push((ctx, a));
        }
    }
    let total = vis.len() as i64;
    let mut items = Vec::new();
    for (ctx, a) in vis.iter().skip(q.cursor.offset() as usize).take(q.cursor.per_page() as usize) {
        items.push(advisory_json(s, ctx, a).await?);
    }
    Ok(super::list_response(s, uri, &q.cursor, total, items))
}

async fn list(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>, uri: OriginalUri, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    if let Some(st) = &q.state {
        if !STATES.contains(&st.as_str()) {
            return Err(bad("state"));
        }
    }
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM repository_advisories WHERE repo_id = $1 AND ($2::text IS NULL OR state = $2) ORDER BY {}",
        order(&q)?
    )))
    .bind(ctx.repo.id)
    .bind(&q.state)
    .fetch_all(&s.db)
    .await?;
    visible_page(&s, rows, std::slice::from_ref(&ctx), u.as_ref(), &q, &uri).await
}

async fn org_list(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, uri: OriginalUri, Query(q): Query<ListQ>) -> ApiResult<Response> {
    let o = crate::orgs::org_by_login(&s, &org).await?;
    super::require_security_manager(&s, &o, &u).await?;
    if let Some(st) = &q.state {
        if !STATES.contains(&st.as_str()) {
            return Err(bad("state"));
        }
    }
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT a.* FROM repository_advisories a JOIN repositories r ON r.id = a.repo_id
          WHERE r.owner_id = $1 AND ($2::text IS NULL OR a.state = $2) ORDER BY a.{}",
        order(&q)?.replace(", id", ", a.id")
    )))
    .bind(o.id)
    .bind(&q.state)
    .fetch_all(&s.db)
    .await?;
    let mut ctxs: Vec<RepoCtx> = Vec::new();
    for id in rows.iter().map(|a| a.repo_id).collect::<std::collections::BTreeSet<_>>() {
        if let Ok(c) = db::load_repo_by_id(&s.db, Some(&u), id).await {
            ctxs.push(c);
        }
    }
    visible_page(&s, rows, &ctxs, Some(&u), &q, &uri).await
}

async fn get_one(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r, ghsa)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let a = load(&s, &ctx, &ghsa).await?;
    if !can_see(&s, &ctx, &a, u.as_ref()).await {
        return Err(ApiError::not_found());
    }
    Ok(Json(advisory_json(&s, &ctx, &a).await?))
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<Map<String, Value>>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    if !is_manager(&s, &ctx, &u).await {
        ctx.require(Perm::Admin)?;
    }
    let f = parse_fields(&s, &ctx, &b).await?;
    if f.summary.as_deref().unwrap_or("").is_empty() {
        return Err(ApiError::validation("RepositoryAdvisory", "summary", "missing_field"));
    }
    if f.description.as_ref().is_none_or(|d| d.is_none()) {
        return Err(ApiError::validation("RepositoryAdvisory", "description", "missing_field"));
    }
    if f.vulnerabilities.as_ref().and_then(Value::as_array).is_none_or(|v| v.is_empty()) {
        return Err(ApiError::validation("RepositoryAdvisory", "vulnerabilities", "missing_field"));
    }
    let id = insert(&s, &ctx, &u, "draft", false).await?;
    apply_fields(&s, id, &f, Some(u.id)).await?;
    let a = by_id(&s, id).await?;
    Ok((StatusCode::CREATED, Json(advisory_json(&s, &ctx, &a).await?)).into_response())
}

async fn report(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<Map<String, Value>>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    if !super::enabled(&s.db, ctx.repo.id, "private_vulnerability_reporting").await {
        return Err(ApiError::forbidden("Private vulnerability reporting is not enabled for this repository."));
    }
    // Reports only carry content; credits and collaborators are the maintainers' call.
    let mut body = b.clone();
    for k in ["credits", "collaborating_users", "collaborating_teams", "cve_id", "state"] {
        body.remove(k);
    }
    let f = parse_fields(&s, &ctx, &body).await?;
    if f.summary.as_deref().unwrap_or("").is_empty() {
        return Err(ApiError::validation("RepositoryAdvisory", "summary", "missing_field"));
    }
    if f.description.as_ref().is_none_or(|d| d.is_none()) {
        return Err(ApiError::validation("RepositoryAdvisory", "description", "missing_field"));
    }
    let id = insert(&s, &ctx, &u, "triage", true).await?;
    apply_fields(&s, id, &f, Some(u.id)).await?;
    // The reporter is credited as such.
    sqlx::query("INSERT INTO repository_advisory_credits (advisory_id, user_id, kind, state) VALUES ($1, $2, 'reporter', 'accepted') ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(u.id)
        .execute(&s.db)
        .await?;
    let a = by_id(&s, id).await?;
    Ok((StatusCode::CREATED, Json(advisory_json(&s, &ctx, &a).await?)).into_response())
}

async fn update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, ghsa)): Path<(String, String, String)>, Json(b): Json<Map<String, Value>>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let a = load(&s, &ctx, &ghsa).await?;
    let manager = is_manager(&s, &ctx, &u).await;
    if !manager {
        if !can_see(&s, &ctx, &a, Some(&u)).await {
            return Err(ApiError::not_found());
        }
        if !is_collaborator(&s, &a, &u).await {
            return Err(ApiError::forbidden("You do not have permission to update this advisory."));
        }
    }
    let f = parse_fields(&s, &ctx, &b).await?;
    if !manager && (f.credits.is_some() || f.collaborating_users.is_some() || f.collaborating_teams.is_some() || b.contains_key("state")) {
        return Err(ApiError::forbidden("Only repository administrators and security managers can change credits, collaborators or the state."));
    }
    if a.state == "published" || a.state == "withdrawn" {
        if f.cve_id.is_some() && a.cve_id.is_some() {
            return Err(ApiError::unprocessable("The CVE ID of a published advisory cannot be changed."));
        }
    }
    let new_state = match b.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(st)) if st == &a.state => None,
        Some(Value::String(st)) => {
            let ok = matches!(
                (a.state.as_str(), st.as_str()),
                ("triage", "draft") | ("triage", "closed") | ("draft", "published") | ("draft", "closed") | ("closed", "draft")
            );
            if !ok {
                return Err(ApiError::unprocessable(format!("Cannot change the state of a {} advisory to {st}.", a.state)));
            }
            Some(st.clone())
        }
        Some(_) => return Err(bad("state")),
    };
    apply_fields(&s, a.id, &f, a.author_id).await?;
    if let Some(st) = new_state {
        let cur = by_id(&s, a.id).await?;
        if st == "published" {
            if cur.summary.is_empty() || cur.description.as_deref().unwrap_or("").is_empty() {
                return Err(ApiError::unprocessable("A summary and a description are required to publish an advisory."));
            }
            if cur.severity.is_none() {
                return Err(ApiError::unprocessable("A severity or CVSS vector is required to publish an advisory."));
            }
        }
        sqlx::query(
            "UPDATE repository_advisories SET state = $2, updated_at = now(),
                    report_accepted = report_accepted OR (is_report AND $2 = 'draft'),
                    published_at = CASE WHEN $2 = 'published' THEN now() ELSE published_at END,
                    publisher_id = CASE WHEN $2 = 'published' THEN $3 ELSE publisher_id END,
                    closed_at = CASE WHEN $2 = 'closed' THEN now() WHEN $2 = 'draft' THEN NULL ELSE closed_at END
              WHERE id = $1",
        )
        .bind(a.id)
        .bind(&st)
        .bind(u.id)
        .execute(&s.db)
        .await?;
        if st == "published" {
            let row = by_id(&s, a.id).await?;
            super::advisories::import_osv(&s, &osv_record(&ctx, &s, &row)).await?;
            super::enqueue(&s, super::Q_ADVISORY_RESCAN, json!({})).await;
        }
    } else if a.state == "published" {
        // Edits to a published advisory reach the global database too.
        let row = by_id(&s, a.id).await?;
        super::advisories::import_osv(&s, &osv_record(&ctx, &s, &row)).await?;
    }
    let row = by_id(&s, a.id).await?;
    Ok(Json(advisory_json(&s, &ctx, &row).await?))
}

async fn request_cve(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, ghsa)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let a = load(&s, &ctx, &ghsa).await?;
    if !is_manager(&s, &ctx, &u).await {
        return Err(if can_see(&s, &ctx, &a, Some(&u)).await { ApiError::forbidden("Must have admin rights to Repository.") } else { ApiError::not_found() });
    }
    if a.cve_id.is_some() {
        return Err(ApiError::unprocessable("This advisory already has a CVE ID."));
    }
    if a.state != "draft" {
        return Err(ApiError::unprocessable("A CVE can only be requested for a draft advisory."));
    }
    if a.cve_requested_at.is_some() {
        return Err(ApiError::unprocessable("A CVE has already been requested for this advisory."));
    }
    sqlx::query("UPDATE repository_advisories SET cve_requested_at = now(), updated_at = now() WHERE id = $1").bind(a.id).execute(&s.db).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({}))).into_response())
}

async fn create_fork(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, ghsa)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    let a = load(&s, &ctx, &ghsa).await?;
    if !can_see(&s, &ctx, &a, Some(&u)).await {
        return Err(ApiError::not_found());
    }
    Err(ApiError::unprocessable("Temporary private forks are not supported by this server."))
}

async fn pvr_get(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    Ok(Json(json!({ "enabled": super::enabled(&s.db, ctx.repo.id, "private_vulnerability_reporting").await })))
}

async fn pvr_set(s: &AppState, u: &User, o: &str, r: &str, on: bool) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(u), o, r).await?;
    if !is_manager(s, &ctx, u).await {
        ctx.require(Perm::Admin)?;
    }
    super::set_enabled(s, ctx.repo.id, "private_vulnerability_reporting", on, None).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn pvr_enable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    pvr_set(&s, &u, &o, &r, true).await
}

async fn pvr_disable(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Response> {
    pvr_set(&s, &u, &o, &r, false).await
}
