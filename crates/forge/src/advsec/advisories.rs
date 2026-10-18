//! The advisory database: GitHub Advisory Database records imported from OSV files
//! (github/advisory-database `advisories/**/GHSA-*.json`, or osv.dev's per-ecosystem `all.zip`),
//! served by `GET /advisories[/{ghsa_id}]` and matched against dependency graphs by Dependabot.
//!
//! Import: `forge advisories sync --from <dir|file.json|file.zip|https://...>`, or the forge-only
//! `POST /api/forge/advisories` (site admins; body: one OSV record or an array). Every import
//! re-runs Dependabot matching for repositories with Dependabot alerts enabled.
use std::cmp::Ordering;
use std::path::Path;

use axum::extract::{OriginalUri, Path as AxPath, Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::CursorParams;
use crate::auth::{AuthUser, MaybeUser};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/advisories", get(list)).route("/advisories/{ghsa_id}", get(get_one))
}

pub fn forge_routes() -> Router<AppState> {
    Router::new().route("/advisories", post(import_endpoint))
}

// ---------------------------------------------------------------------------------------------
// Versions

/// One comparable version segment.
#[derive(Debug, PartialEq, Eq)]
enum Seg {
    Num(u64),
    Text(String),
}

fn segments(v: &str) -> (Vec<Seg>, Option<Vec<Seg>>) {
    let v = v.trim().trim_start_matches(['v', 'V', '=']);
    let v = v.split('+').next().unwrap_or(v);
    let (main, pre) = match v.find('-') {
        Some(i) => (&v[..i], Some(&v[i + 1..])),
        None => (v, None),
    };
    let parse = |s: &str| -> Vec<Seg> {
        let mut out = Vec::new();
        for part in s.split(['.', '_']) {
            // "1rc2" -> 1, "rc", 2
            let mut cur = String::new();
            let mut num: Option<bool> = None;
            for c in part.chars() {
                let is_num = c.is_ascii_digit();
                if num.is_some_and(|n| n != is_num) {
                    out.push(seg(&cur));
                    cur.clear();
                }
                num = Some(is_num);
                cur.push(c);
            }
            if !cur.is_empty() {
                out.push(seg(&cur));
            }
        }
        out
    };
    // pip-style pre-releases without a dash ("2.0rc1") are text segments in `main`.
    (parse(main), pre.map(parse))
}

fn seg(s: &str) -> Seg {
    s.parse::<u64>().map(Seg::Num).unwrap_or_else(|_| Seg::Text(s.to_ascii_lowercase()))
}

fn cmp_segs(a: &[Seg], b: &[Seg]) -> Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let o = match (a.get(i), b.get(i)) {
            (Some(Seg::Num(x)), Some(Seg::Num(y))) => x.cmp(y),
            (Some(Seg::Text(x)), Some(Seg::Text(y))) => x.cmp(y),
            (Some(Seg::Num(_)), Some(Seg::Text(_))) => Ordering::Greater,
            (Some(Seg::Text(_)), Some(Seg::Num(_))) => Ordering::Less,
            (Some(Seg::Num(x)), None) => if *x == 0 { Ordering::Equal } else { Ordering::Greater },
            (None, Some(Seg::Num(y))) => if *y == 0 { Ordering::Equal } else { Ordering::Less },
            // "1.0rc1" < "1.0"
            (Some(Seg::Text(_)), None) => Ordering::Less,
            (None, Some(Seg::Text(_))) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// Compares two versions (semver-like, tolerant of ecosystem quirks: `v` prefixes, pip
/// pre-releases, Maven-style segments). Pre-releases sort before their release.
pub fn cmp_versions(a: &str, b: &str) -> Ordering {
    let (am, ap) = segments(a);
    let (bm, bp) = segments(b);
    match cmp_segs(&am, &bm) {
        Ordering::Equal => match (ap, bp) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(x), Some(y)) => cmp_segs(&x, &y),
        },
        o => o,
    }
}

/// An affected interval of an OSV range: `introduced` (inclusive; "0" = all) up to `fixed`
/// (exclusive) or `last_affected` (inclusive).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Interval {
    pub introduced: Option<String>,
    pub fixed: Option<String>,
    pub last_affected: Option<String>,
}

/// `0`, `0.0.0`, `0.0.0-0`: "all versions".
fn is_zero(v: &str) -> bool {
    let main = v.split('-').next().unwrap_or(v);
    !main.is_empty() && main.split('.').all(|p| p == "0") && v.split_once('-').is_none_or(|(_, pre)| pre == "0")
}

impl Interval {
    pub fn contains(&self, v: &str) -> bool {
        if let Some(i) = self.introduced.as_deref().filter(|i| !is_zero(i)) {
            if cmp_versions(v, i) == Ordering::Less {
                return false;
            }
        }
        if let Some(f) = &self.fixed {
            return cmp_versions(v, f) == Ordering::Less;
        }
        if let Some(l) = &self.last_affected {
            return cmp_versions(v, l) != Ordering::Greater;
        }
        true
    }
    /// GitHub's `vulnerable_version_range` notation.
    pub fn github_range(&self) -> String {
        let mut parts = Vec::new();
        if let Some(i) = self.introduced.as_deref().filter(|i| !is_zero(i)) {
            parts.push(format!(">= {i}"));
        }
        if let Some(f) = &self.fixed {
            parts.push(format!("< {f}"));
        } else if let Some(l) = &self.last_affected {
            parts.push(format!("<= {l}"));
        }
        if parts.is_empty() { ">= 0".to_string() } else { parts.join(", ") }
    }
    pub fn to_json(&self) -> Value {
        json!({ "introduced": self.introduced, "fixed": self.fixed, "last_affected": self.last_affected })
    }
    pub fn from_json(v: &Value) -> Interval {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Interval { introduced: s("introduced"), fixed: s("fixed"), last_affected: s("last_affected") }
    }
}

/// Splits OSV range events into intervals.
pub fn intervals(events: &[Value]) -> Vec<Interval> {
    let mut out = Vec::new();
    let mut cur: Option<Interval> = None;
    for e in events {
        if let Some(i) = e.get("introduced").and_then(Value::as_str) {
            if let Some(c) = cur.take() {
                out.push(c);
            }
            cur = Some(Interval { introduced: Some(i.to_string()), ..Default::default() });
        } else if let Some(f) = e.get("fixed").and_then(Value::as_str) {
            let mut c = cur.take().unwrap_or_default();
            c.fixed = Some(f.to_string());
            out.push(c);
        } else if let Some(l) = e.get("last_affected").and_then(Value::as_str) {
            let mut c = cur.take().unwrap_or_default();
            c.last_affected = Some(l.to_string());
            out.push(c);
        }
    }
    if let Some(c) = cur {
        out.push(c);
    }
    out
}

/// Parses GitHub's range notation (`>= 1.0, < 1.2.3`, `<= 2.0`, `= 1.0.1`) into an interval.
pub fn parse_github_range(r: &str) -> Interval {
    let mut iv = Interval::default();
    for part in r.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(">=") {
            iv.introduced = Some(v.trim().to_string());
        } else if let Some(v) = part.strip_prefix("<=") {
            iv.last_affected = Some(v.trim().to_string());
        } else if let Some(v) = part.strip_prefix('<') {
            iv.fixed = Some(v.trim().to_string());
        } else if let Some(v) = part.strip_prefix('>') {
            // "> x": treat as >= x and exclude x itself through a tiny bump is not possible
            // generically; >= is the conservative reading.
            iv.introduced = Some(v.trim().to_string());
        } else if let Some(v) = part.strip_prefix('=') {
            iv.introduced = Some(v.trim().to_string());
            iv.last_affected = Some(v.trim().to_string());
        }
    }
    iv
}

// ---------------------------------------------------------------------------------------------
// Ecosystems

/// OSV ecosystem -> GitHub ecosystem name.
pub fn github_ecosystem(osv: &str) -> &'static str {
    match osv.split(':').next().unwrap_or(osv) {
        "npm" => "npm",
        "PyPI" => "pip",
        "crates.io" => "rust",
        "Go" => "go",
        "RubyGems" => "rubygems",
        "Maven" => "maven",
        "NuGet" => "nuget",
        "Packagist" => "composer",
        "Hex" => "erlang",
        "Pub" => "pub",
        "SwiftURL" => "swift",
        "GitHub Actions" => "actions",
        _ => "other",
    }
}

/// Dependency-graph ecosystem (`api::dependency_graph::Dep::ecosystem`) -> GitHub ecosystem.
pub fn dep_ecosystem(dep: &str) -> &str {
    match dep {
        "cargo" => "rust",
        "golang" => "go",
        "pypi" => "pip",
        "gem" => "rubygems",
        other => other,
    }
}

/// Package name as matched (pip names are case/underscore-insensitive).
pub fn normalize_package(ecosystem: &str, name: &str) -> String {
    match ecosystem {
        "pip" => name.to_lowercase().replace(['_', '.'], "-"),
        _ => name.to_lowercase(),
    }
}

pub(crate) fn cwe_name(id: &str) -> String {
    let known = [
        ("CWE-20", "Improper Input Validation"),
        ("CWE-22", "Improper Limitation of a Pathname to a Restricted Directory ('Path Traversal')"),
        ("CWE-78", "Improper Neutralization of Special Elements used in an OS Command ('OS Command Injection')"),
        ("CWE-79", "Improper Neutralization of Input During Web Page Generation ('Cross-site Scripting')"),
        ("CWE-89", "Improper Neutralization of Special Elements used in an SQL Command ('SQL Injection')"),
        ("CWE-94", "Improper Control of Generation of Code ('Code Injection')"),
        ("CWE-200", "Exposure of Sensitive Information to an Unauthorized Actor"),
        ("CWE-400", "Uncontrolled Resource Consumption"),
        ("CWE-502", "Deserialization of Untrusted Data"),
        ("CWE-770", "Allocation of Resources Without Limits or Throttling"),
        ("CWE-915", "Improperly Controlled Modification of Dynamically-Determined Object Attributes"),
        ("CWE-1321", "Improperly Controlled Modification of Object Prototype Attributes ('Prototype Pollution')"),
        ("CWE-1333", "Inefficient Regular Expression Complexity"),
        ("CWE-918", "Server-Side Request Forgery (SSRF)"),
        ("CWE-352", "Cross-Site Request Forgery (CSRF)"),
        ("CWE-287", "Improper Authentication"),
        ("CWE-601", "URL Redirection to Untrusted Site ('Open Redirect')"),
    ];
    known.iter().find(|(k, _)| *k == id).map(|(_, n)| n.to_string()).unwrap_or_else(|| id.to_string())
}

/// CVSS v3 base score from a vector string (`CVSS:3.x/AV:N/AC:L/...`).
pub fn cvss3_score(vector: &str) -> Option<f64> {
    let mut m = std::collections::HashMap::new();
    for part in vector.split('/').skip(1) {
        let (k, v) = part.split_once(':')?;
        m.insert(k, v);
    }
    let av = match *m.get("AV")? { "N" => 0.85, "A" => 0.62, "L" => 0.55, "P" => 0.2, _ => return None };
    let ac = match *m.get("AC")? { "L" => 0.77, "H" => 0.44, _ => return None };
    let changed = *m.get("S")? == "C";
    let pr = match (*m.get("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match *m.get("UI")? { "N" => 0.85, "R" => 0.62, _ => return None };
    let cia = |k: &str| -> Option<f64> { Some(match *m.get(k)? { "H" => 0.56, "L" => 0.22, "N" => 0.0, _ => return None }) };
    let (c, i, a) = (cia("C")?, cia("I")?, cia("A")?);
    let iss = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);
    let impact = if changed { 7.52 * (iss - 0.029) - 3.25 * (iss - 0.02).powi(15) } else { 6.42 * iss };
    let expl = 8.22 * av * ac * pr * ui;
    if impact <= 0.0 {
        return Some(0.0);
    }
    let raw = if changed { (1.08 * (impact + expl)).min(10.0) } else { (impact + expl).min(10.0) };
    Some((raw * 10.0).ceil() / 10.0)
}

pub(crate) fn severity_from_score(s: f64) -> &'static str {
    match s {
        x if x >= 9.0 => "critical",
        x if x >= 7.0 => "high",
        x if x >= 4.0 => "medium",
        x if x > 0.0 => "low",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------------------------
// Import

fn ts(v: Option<&Value>) -> Option<DateTime<Utc>> {
    v.and_then(Value::as_str).and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc))
}

/// Imports one OSV record. Returns false when it is not a GitHub advisory (no GHSA id).
pub async fn import_osv(s: &AppState, rec: &Value) -> ApiResult<bool> {
    let id = rec.get("id").and_then(Value::as_str).unwrap_or("");
    let aliases: Vec<String> = rec.get("aliases").and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
    let ghsa = if id.starts_with("GHSA-") {
        id.to_string()
    } else if let Some(a) = aliases.iter().find(|a| a.starts_with("GHSA-")) {
        a.clone()
    } else {
        return Ok(false);
    };
    let cve = aliases.iter().chain(std::iter::once(&id.to_string())).find(|a| a.starts_with("CVE-")).cloned();
    let dbs = rec.get("database_specific").cloned().unwrap_or(json!({}));
    let mut v3: Option<(String, Option<f64>)> = None;
    let mut v4: Option<String> = None;
    for sev in rec.get("severity").and_then(Value::as_array).into_iter().flatten() {
        let score = sev.get("score").and_then(Value::as_str).unwrap_or("");
        match sev.get("type").and_then(Value::as_str) {
            Some("CVSS_V3") => v3 = Some((score.to_string(), cvss3_score(score))),
            Some("CVSS_V4") => v4 = Some(score.to_string()),
            _ => {}
        }
    }
    let severity = match dbs.get("severity").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        Some("critical") => "critical".to_string(),
        Some("high") => "high".to_string(),
        Some("moderate") | Some("medium") => "medium".to_string(),
        Some("low") => "low".to_string(),
        _ => v3.as_ref().and_then(|(_, s)| *s).map(severity_from_score).unwrap_or("unknown").to_string(),
    };
    let cwes: Vec<Value> = dbs
        .get("cwe_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|c| json!({ "cwe_id": c, "name": cwe_name(c) }))
        .collect();
    let mut identifiers = vec![json!({ "type": "GHSA", "value": ghsa })];
    if let Some(c) = &cve {
        identifiers.push(json!({ "type": "CVE", "value": c }));
    }
    let refs: Vec<Value> = rec
        .get("references")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("url").and_then(Value::as_str).map(|u| json!(u)))
        .collect();
    let source = rec
        .get("references")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|r| r.get("type").and_then(Value::as_str) == Some("PACKAGE"))
        .and_then(|r| r.get("url").and_then(Value::as_str))
        .map(str::to_string);
    let kind = if id.starts_with("MAL-") || dbs.get("malware").is_some() {
        "malware"
    } else if dbs.get("github_reviewed").and_then(Value::as_bool) == Some(false) {
        "unreviewed"
    } else {
        "reviewed"
    };
    let published = ts(rec.get("published")).unwrap_or_else(Utc::now);
    let modified = ts(rec.get("modified")).unwrap_or(published);
    let mut tx = s.db.begin().await?;
    let adv_id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO advisories (ghsa_id, cve_id, kind, summary, description, severity, cvss_v3_vector, cvss_v3_score, cvss_v4_vector,
                                 cwes, identifiers, refs, source_code_location, published_at, updated_at, github_reviewed_at,
                                 nvd_published_at, withdrawn_at, source_id, imported_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19, now())
         ON CONFLICT (ghsa_id) DO UPDATE SET cve_id = $2, kind = $3, summary = $4, description = $5, severity = $6,
             cvss_v3_vector = $7, cvss_v3_score = $8, cvss_v4_vector = $9, cwes = $10, identifiers = $11, refs = $12,
             source_code_location = $13, published_at = $14, updated_at = $15, github_reviewed_at = $16, nvd_published_at = $17,
             withdrawn_at = $18, source_id = $19, imported_at = now()
           -- GitHub's own record wins over an alias record (e.g. RUSTSEC) of the same advisory.
           WHERE advisories.source_id NOT LIKE 'GHSA-%' OR EXCLUDED.source_id LIKE 'GHSA-%'
         RETURNING id",
    )
    .bind(&ghsa)
    .bind(&cve)
    .bind(kind)
    .bind(rec.get("summary").and_then(Value::as_str).unwrap_or(""))
    .bind(rec.get("details").and_then(Value::as_str))
    .bind(&severity)
    .bind(v3.as_ref().map(|(v, _)| v.clone()))
    .bind(v3.as_ref().and_then(|(_, s)| *s))
    .bind(&v4)
    .bind(json!(cwes))
    .bind(json!(identifiers))
    .bind(json!(refs))
    .bind(&source)
    .bind(published)
    .bind(modified)
    .bind(ts(dbs.get("github_reviewed_at")))
    .bind(ts(dbs.get("nvd_published_at")))
    .bind(ts(rec.get("withdrawn")))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(adv_id) = adv_id else {
        return Ok(true);
    };
    // Vulnerabilities are replaced as a whole; alerts that referenced old rows are re-matched.
    let old: Vec<i64> = sqlx::query_scalar("SELECT id FROM advisory_vulnerabilities WHERE advisory_id = $1").bind(adv_id).fetch_all(&mut *tx).await?;
    let mut kept = Vec::new();
    for aff in rec.get("affected").and_then(Value::as_array).into_iter().flatten() {
        let eco = github_ecosystem(aff.pointer("/package/ecosystem").and_then(Value::as_str).unwrap_or(""));
        let Some(name) = aff.pointer("/package/name").and_then(Value::as_str) else { continue };
        let functions: Vec<String> = aff
            .pointer("/ecosystem_specific/affected_functions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str().map(str::to_string))
            .collect();
        let versions: Vec<String> = aff.get("versions").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let last_known = aff.pointer("/database_specific/last_known_affected_version_range").and_then(Value::as_str);
        let mut ivs: Vec<Interval> = Vec::new();
        for r in aff.get("ranges").and_then(Value::as_array).into_iter().flatten() {
            if r.get("type").and_then(Value::as_str) == Some("GIT") {
                continue;
            }
            ivs.extend(intervals(r.get("events").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])));
        }
        if ivs.is_empty() && versions.is_empty() {
            continue;
        }
        if ivs.is_empty() {
            ivs.push(Interval { introduced: versions.first().cloned(), last_affected: versions.last().cloned(), fixed: None });
        }
        for iv in ivs {
            let mut iv = iv;
            // An open-ended range with GitHub's "last known affected" hint becomes bounded.
            if iv.fixed.is_none() && iv.last_affected.is_none() {
                if let Some(lk) = last_known {
                    let p = parse_github_range(lk);
                    iv.last_affected = p.last_affected;
                    iv.fixed = p.fixed;
                }
            }
            let range = iv.github_range();
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM advisory_vulnerabilities WHERE advisory_id = $1 AND ecosystem = $2 AND package = $3 AND vulnerable_version_range = $4",
            )
            .bind(adv_id)
            .bind(eco)
            .bind(name)
            .bind(&range)
            .fetch_optional(&mut *tx)
            .await?;
            let vid: i64 = match existing {
                Some(v) => {
                    sqlx::query("UPDATE advisory_vulnerabilities SET first_patched_version = $2, vulnerable_functions = $3, ranges = $4, versions = $5 WHERE id = $1")
                        .bind(v)
                        .bind(&iv.fixed)
                        .bind(&functions)
                        .bind(json!([iv.to_json()]))
                        .bind(&versions)
                        .execute(&mut *tx)
                        .await?;
                    v
                }
                None => {
                    sqlx::query_scalar(
                        "INSERT INTO advisory_vulnerabilities (advisory_id, ecosystem, package, vulnerable_version_range, first_patched_version,
                                                               vulnerable_functions, ranges, versions)
                         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id",
                    )
                    .bind(adv_id)
                    .bind(eco)
                    .bind(name)
                    .bind(&range)
                    .bind(&iv.fixed)
                    .bind(&functions)
                    .bind(json!([iv.to_json()]))
                    .bind(&versions)
                    .fetch_one(&mut *tx)
                    .await?
                }
            };
            kept.push(vid);
        }
    }
    let stale: Vec<i64> = old.into_iter().filter(|o| !kept.contains(o)).collect();
    if !stale.is_empty() {
        // Alerts follow the advisory's new ranges for the same package.
        sqlx::query(
            "UPDATE dependabot_alerts d SET vulnerability_id = (
                 SELECT v.id FROM advisory_vulnerabilities v WHERE v.id = ANY($2) AND v.ecosystem = d.ecosystem AND v.package = d.package ORDER BY v.id LIMIT 1)
              WHERE d.vulnerability_id = ANY($1)",
        )
        .bind(&stale)
        .bind(&kept)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM advisory_vulnerabilities WHERE id = ANY($1)").bind(&stale).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Imports a JSON document: one OSV record, an array of them, or `{"advisories": [...]}`.
pub async fn import_json(s: &AppState, doc: &Value) -> ApiResult<usize> {
    let list: Vec<&Value> = match doc {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) if o.contains_key("advisories") => o["advisories"].as_array().map(|a| a.iter().collect()).unwrap_or_default(),
        v => vec![v],
    };
    let mut n = 0;
    for rec in list {
        if import_osv(s, rec).await? {
            n += 1;
        }
    }
    Ok(n)
}

/// `forge advisories sync --from <path|url>`: a directory (walked recursively for `*.json`,
/// e.g. a checkout of github/advisory-database), a `.json` file, a `.zip` of OSV files
/// (osv.dev `gs://osv-vulnerabilities/<ecosystem>/all.zip`), or an http(s) URL of either.
pub async fn sync_from(s: &AppState, from: &str) -> anyhow::Result<usize> {
    let n = if from.starts_with("http://") || from.starts_with("https://") {
        let bytes = reqwest::get(from).await?.error_for_status()?.bytes().await?;
        import_bytes(s, from, &bytes).await?
    } else {
        let p = Path::new(from);
        if p.is_dir() {
            let mut n = 0;
            let mut stack = vec![p.to_path_buf()];
            while let Some(d) = stack.pop() {
                let mut rd = tokio::fs::read_dir(&d).await?;
                while let Some(e) = rd.next_entry().await? {
                    let path = e.path();
                    if e.file_type().await?.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|x| x == "json") {
                        let bytes = tokio::fs::read(&path).await?;
                        n += import_bytes(s, &path.to_string_lossy(), &bytes).await?;
                    }
                }
            }
            n
        } else {
            let bytes = tokio::fs::read(p).await?;
            import_bytes(s, from, &bytes).await?
        }
    };
    super::enqueue(s, super::Q_ADVISORY_RESCAN, json!({})).await;
    Ok(n)
}

async fn import_bytes(s: &AppState, name: &str, bytes: &[u8]) -> anyhow::Result<usize> {
    if bytes.starts_with(b"PK\x03\x04") || name.ends_with(".zip") {
        let mut n = 0;
        for (fname, data) in super::zip::read_all(bytes)? {
            if !fname.ends_with(".json") {
                continue;
            }
            let Ok(doc) = serde_json::from_slice::<Value>(&data) else { continue };
            n += import_json(s, &doc).await.map_err(|e| anyhow::anyhow!(e.message))?;
        }
        return Ok(n);
    }
    let doc: Value = serde_json::from_slice(bytes)?;
    import_json(s, &doc).await.map_err(|e| anyhow::anyhow!(e.message))
}

async fn import_endpoint(State(s): State<AppState>, AuthUser(u): AuthUser, Json(doc): Json<Value>) -> ApiResult<Json<Value>> {
    if !u.site_admin {
        return Err(ApiError::forbidden("Site administrators only."));
    }
    let n = import_json(&s, &doc).await?;
    super::enqueue(&s, super::Q_ADVISORY_RESCAN, json!({})).await;
    Ok(Json(json!({ "imported": n })))
}

// ---------------------------------------------------------------------------------------------
// Rows and JSON

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdvisoryRow {
    pub id: i64,
    pub ghsa_id: String,
    pub cve_id: Option<String>,
    pub kind: String,
    pub summary: String,
    pub description: Option<String>,
    pub severity: String,
    pub cvss_v3_vector: Option<String>,
    pub cvss_v3_score: Option<f64>,
    pub cvss_v4_vector: Option<String>,
    pub cvss_v4_score: Option<f64>,
    pub cwes: Value,
    pub identifiers: Value,
    pub refs: Value,
    pub source_code_location: Option<String>,
    pub epss: Option<Value>,
    pub published_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub github_reviewed_at: Option<DateTime<Utc>>,
    pub nvd_published_at: Option<DateTime<Utc>>,
    pub withdrawn_at: Option<DateTime<Utc>>,
}

pub const ADV_COLS: &str = "a.id, a.ghsa_id, a.cve_id, a.kind, a.summary, a.description, a.severity, a.cvss_v3_vector, a.cvss_v3_score,
    a.cvss_v4_vector, a.cvss_v4_score, a.cwes, a.identifiers, a.refs, a.source_code_location, a.epss, a.published_at, a.updated_at,
    a.github_reviewed_at, a.nvd_published_at, a.withdrawn_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct VulnRow {
    pub id: i64,
    pub advisory_id: i64,
    pub ecosystem: String,
    pub package: String,
    pub vulnerable_version_range: String,
    pub first_patched_version: Option<String>,
    pub vulnerable_functions: Vec<String>,
    pub ranges: Value,
    pub versions: Vec<String>,
}

impl VulnRow {
    /// Whether dependency version `v` is affected.
    pub fn affects(&self, v: &str) -> bool {
        if self.versions.iter().any(|x| x == v) {
            return true;
        }
        let ivs: Vec<Interval> = self.ranges.as_array().into_iter().flatten().map(Interval::from_json).collect();
        if ivs.is_empty() {
            return parse_github_range(&self.vulnerable_version_range).contains(v);
        }
        ivs.iter().any(|i| i.contains(v))
    }
}

pub async fn vulns_for(db: &sqlx::PgPool, advisory_ids: &[i64]) -> ApiResult<Vec<VulnRow>> {
    Ok(sqlx::query_as("SELECT * FROM advisory_vulnerabilities WHERE advisory_id = ANY($1) ORDER BY id").bind(advisory_ids).fetch_all(db).await?)
}

fn cvss_obj(vector: &Option<String>, score: Option<f64>) -> Value {
    json!({ "vector_string": vector, "score": score })
}

fn cvss_severities(a: &AdvisoryRow) -> Value {
    json!({
        "cvss_v3": cvss_obj(&a.cvss_v3_vector, a.cvss_v3_score),
        "cvss_v4": cvss_obj(&a.cvss_v4_vector, a.cvss_v4_score),
    })
}

/// `global-advisory` JSON.
pub fn global_json(s: &AppState, a: &AdvisoryRow, vulns: &[VulnRow]) -> Value {
    let vs: Vec<Value> = vulns
        .iter()
        .filter(|v| v.advisory_id == a.id)
        .map(|v| {
            json!({
                "package": { "ecosystem": v.ecosystem, "name": v.package },
                "vulnerable_version_range": v.vulnerable_version_range,
                "first_patched_version": v.first_patched_version,
                "vulnerable_functions": v.vulnerable_functions,
            })
        })
        .collect();
    json!({
        "ghsa_id": a.ghsa_id,
        "cve_id": a.cve_id,
        "url": format!("{}/advisories/{}", s.api_url(), a.ghsa_id),
        "html_url": format!("{}/advisories/{}", s.base_url(), a.ghsa_id),
        "repository_advisory_url": null,
        "summary": a.summary,
        "description": a.description,
        "type": a.kind,
        "severity": a.severity,
        "source_code_location": a.source_code_location,
        "identifiers": a.identifiers,
        "references": a.refs,
        "published_at": crate::json::ts(a.published_at),
        "updated_at": crate::json::ts(a.updated_at),
        "github_reviewed_at": crate::json::ts_opt(a.github_reviewed_at),
        "nvd_published_at": crate::json::ts_opt(a.nvd_published_at),
        "withdrawn_at": crate::json::ts_opt(a.withdrawn_at),
        "vulnerabilities": vs,
        "cvss": cvss_obj(&a.cvss_v3_vector, a.cvss_v3_score),
        "cvss_severities": cvss_severities(a),
        "epss": a.epss.clone().unwrap_or(Value::Null),
        "cwes": a.cwes,
        "credits": [],
    })
}

/// Known vulnerabilities of dependency versions (`(dependency-graph ecosystem, name, version)`),
/// as dependency review's `vulnerabilities` (`severity`, `advisory_ghsa_id`, `advisory_summary`,
/// `advisory_url`), one list per input.
pub async fn vulnerabilities_of(s: &AppState, deps: &[(&str, &str, &str)]) -> ApiResult<Vec<Vec<Value>>> {
    let ecos: Vec<String> = deps.iter().map(|d| dep_ecosystem(d.0).to_string()).collect();
    let names: Vec<String> = deps.iter().map(|d| normalize_package(dep_ecosystem(d.0), d.1)).collect();
    let rows: Vec<VulnRow> = sqlx::query_as(
        "SELECT v.* FROM advisory_vulnerabilities v JOIN advisories a ON a.id = v.advisory_id
          WHERE a.withdrawn_at IS NULL AND v.ecosystem = ANY($1)
            AND (lower(v.package) = ANY($2) OR lower(replace(replace(v.package, '_', '-'), '.', '-')) = ANY($2))",
    )
    .bind(&ecos)
    .bind(&names)
    .fetch_all(&s.db)
    .await?;
    let ids: Vec<i64> = rows.iter().map(|v| v.advisory_id).collect();
    let advs: Vec<AdvisoryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {ADV_COLS} FROM advisories a WHERE a.id = ANY($1)")))
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    Ok(deps
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let version = d.2.trim().trim_start_matches(|c: char| "^~=<>! v".contains(c)).split([',', ' ']).next().unwrap_or("");
            let mut seen = std::collections::HashSet::new();
            rows.iter()
                .filter(|v| v.ecosystem == ecos[i] && normalize_package(&ecos[i], &v.package) == names[i])
                .filter(|v| !version.is_empty() && v.affects(version))
                .filter_map(|v| advs.iter().find(|a| a.id == v.advisory_id))
                .filter(|a| seen.insert(a.id))
                .map(|a| {
                    json!({
                        "severity": alert_severity(&a.severity),
                        "advisory_ghsa_id": a.ghsa_id,
                        "advisory_summary": a.summary,
                        "advisory_url": format!("{}/advisories/{}", s.base_url(), a.ghsa_id),
                    })
                })
                .collect()
        })
        .collect())
}

/// Dependabot alert severity (no `unknown`).
pub fn alert_severity(sev: &str) -> &str {
    match sev {
        "critical" | "high" | "medium" | "low" => sev,
        _ => "low",
    }
}

/// `dependabot-alert-security-advisory` JSON.
pub fn dependabot_advisory_json(a: &AdvisoryRow, vulns: &[VulnRow]) -> Value {
    let vs: Vec<Value> = vulns.iter().filter(|v| v.advisory_id == a.id).map(dependabot_vuln_json(a)).collect();
    json!({
        "ghsa_id": a.ghsa_id,
        "cve_id": a.cve_id,
        "summary": a.summary,
        "description": a.description.clone().unwrap_or_default(),
        "vulnerabilities": vs,
        "severity": alert_severity(&a.severity),
        "classification": if a.kind == "malware" { "malware" } else { "general" },
        "cvss": { "score": a.cvss_v3_score.unwrap_or(0.0), "vector_string": a.cvss_v3_vector },
        "cvss_severities": cvss_severities(a),
        "epss": a.epss.clone().map(|e| json!([e])).unwrap_or(Value::Null),
        "cwes": a.cwes,
        "identifiers": a.identifiers,
        "references": a.refs.as_array().into_iter().flatten().map(|u| json!({ "url": u })).collect::<Vec<_>>(),
        "published_at": crate::json::ts(a.published_at),
        "updated_at": crate::json::ts(a.updated_at),
        "withdrawn_at": crate::json::ts_opt(a.withdrawn_at),
    })
}

pub fn dependabot_vuln_json(a: &AdvisoryRow) -> impl Fn(&VulnRow) -> Value + '_ {
    move |v: &VulnRow| {
        json!({
            "package": { "ecosystem": v.ecosystem, "name": v.package },
            "severity": alert_severity(&a.severity),
            "vulnerable_version_range": v.vulnerable_version_range,
            "first_patched_version": v.first_patched_version.as_ref().map(|f| json!({ "identifier": f })),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// REST

#[derive(Deserialize, Default)]
struct ListQ {
    ghsa_id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    cve_id: Option<String>,
    ecosystem: Option<String>,
    severity: Option<String>,
    cwes: Option<String>,
    #[serde(default, deserialize_with = "super::qs::opt_bool")]
    is_withdrawn: Option<bool>,
    affects: Option<String>,
    published: Option<String>,
    updated: Option<String>,
    modified: Option<String>,
    epss_percentage: Option<String>,
    epss_percentile: Option<String>,
    direction: Option<String>,
    sort: Option<String>,
    #[serde(flatten)]
    cursor: CursorParams,
}

/// `published=2022-01-01..2023-01-01`, `>=2022-01-01`, `2022-01-01` -> SQL condition on `col`.
fn date_filter(col: &str, v: &str, binds: &mut Vec<String>) -> ApiResult<String> {
    let parse = |d: &str| -> ApiResult<String> {
        let d = d.trim();
        if NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok() || DateTime::parse_from_rfc3339(d).is_ok() {
            Ok(d.to_string())
        } else {
            Err(ApiError::validation("Advisory", col, "invalid"))
        }
    };
    let mut push = |b: String| {
        binds.push(b);
        format!("${}", binds.len())
    };
    if let Some((a, b)) = v.split_once("..") {
        let (a, b) = (parse(a)?, parse(b)?);
        return Ok(format!("({col} >= {}::date AND {col} < ({}::date + 1))", push(a), push(b)));
    }
    for (op, sql) in [(">=", ">="), ("<=", "<="), (">", ">"), ("<", "<")] {
        if let Some(d) = v.strip_prefix(op) {
            let d = parse(d)?;
            return Ok(if sql == "<=" || sql == ">" {
                format!("{col} {} ({}::date + 1)", if sql == "<=" { "<" } else { ">=" }, push(d))
            } else {
                format!("{col} {sql} {}::date", push(d))
            });
        }
    }
    let d = parse(v)?;
    let p = push(d);
    Ok(format!("({col} >= {p}::date AND {col} < ({p}::date + 1))"))
}

async fn list(State(s): State<AppState>, MaybeUser(_u): MaybeUser, uri: OriginalUri, Query(q): Query<ListQ>) -> ApiResult<Response> {
    super::check_enum("Advisory", "type", &q.kind, &["reviewed", "malware", "unreviewed"])?;
    super::check_enum("Advisory", "severity", &q.severity, &["unknown", "low", "medium", "high", "critical"])?;
    super::check_enum("Advisory", "sort", &q.sort, &["updated", "published", "epss_percentage", "epss_percentile"])?;
    let _ = (&q.epss_percentage, &q.epss_percentile);
    let mut binds: Vec<String> = Vec::new();
    let mut conds: Vec<String> = Vec::new();
    let bind = |v: String, binds: &mut Vec<String>| {
        binds.push(v);
        format!("${}", binds.len())
    };
    let kind = q.kind.clone().unwrap_or_else(|| "reviewed".into());
    conds.push(format!("a.kind = {}", bind(kind, &mut binds)));
    if let Some(g) = &q.ghsa_id {
        conds.push(format!("a.ghsa_id = {}", bind(g.clone(), &mut binds)));
    }
    if let Some(c) = &q.cve_id {
        conds.push(format!("a.cve_id = {}", bind(c.clone(), &mut binds)));
    }
    if let Some(sev) = super::csv(&q.severity) {
        conds.push(format!("a.severity = ANY(string_to_array({}, ','))", bind(sev.join(","), &mut binds)));
    }
    if let Some(e) = &q.ecosystem {
        conds.push(format!("EXISTS(SELECT 1 FROM advisory_vulnerabilities v WHERE v.advisory_id = a.id AND v.ecosystem = {})", bind(e.clone(), &mut binds)));
    }
    if let Some(cw) = super::csv(&q.cwes) {
        let ids: Vec<String> = cw.iter().map(|c| if c.starts_with("CWE-") { c.clone() } else { format!("CWE-{c}") }).collect();
        conds.push(format!(
            "EXISTS(SELECT 1 FROM jsonb_array_elements(a.cwes) c WHERE c->>'cwe_id' = ANY(string_to_array({}, ',')))",
            bind(ids.join(","), &mut binds)
        ));
    }
    if let Some(w) = q.is_withdrawn {
        conds.push(if w { "a.withdrawn_at IS NOT NULL".into() } else { "a.withdrawn_at IS NULL".into() });
    }
    for (col, v) in [("a.published_at", &q.published), ("a.updated_at", &q.updated), ("GREATEST(a.updated_at, a.withdrawn_at)", &q.modified)] {
        if let Some(v) = v {
            conds.push(date_filter(col, v, &mut binds)?);
        }
    }
    let mut affects_filter: Option<Vec<(String, Option<String>)>> = None;
    if let Some(list) = super::csv(&q.affects) {
        let pkgs: Vec<(String, Option<String>)> = list
            .iter()
            .map(|p| match p.rsplit_once('@').filter(|(n, _)| !n.is_empty()) {
                Some((n, v)) => (n.to_lowercase(), Some(v.to_string())),
                None => (p.to_lowercase(), None),
            })
            .collect();
        let names: Vec<String> = pkgs.iter().map(|(n, _)| n.clone()).collect();
        conds.push(format!(
            "EXISTS(SELECT 1 FROM advisory_vulnerabilities v WHERE v.advisory_id = a.id AND lower(v.package) = ANY(string_to_array({}, ',')))",
            bind(names.join(","), &mut binds)
        ));
        affects_filter = Some(pkgs);
    }
    let order_col = match q.sort.as_deref() {
        Some("updated") => "a.updated_at",
        _ => "a.published_at",
    };
    let d = super::dir(q.direction.as_deref());
    let sql = format!("SELECT {ADV_COLS} FROM advisories a WHERE {} ORDER BY {order_col} {d}, a.id {d}", conds.join(" AND "));
    let mut query = sqlx::query_as::<_, AdvisoryRow>(sqlx::AssertSqlSafe(sql));
    for b in &binds {
        query = query.bind(b);
    }
    let mut rows = query.fetch_all(&s.db).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let vulns = vulns_for(&s.db, &ids).await?;
    // Version-qualified `affects` filters need range evaluation.
    if let Some(pkgs) = &affects_filter {
        rows.retain(|a| {
            vulns.iter().filter(|v| v.advisory_id == a.id).any(|v| {
                pkgs.iter().any(|(n, ver)| v.package.to_lowercase() == *n && ver.as_deref().is_none_or(|x| v.affects(x)))
            })
        });
    }
    let total = rows.len() as i64;
    let cp = &q.cursor;
    let items: Vec<Value> = rows
        .iter()
        .skip(cp.offset() as usize)
        .take(cp.per_page() as usize)
        .map(|a| global_json(&s, a, &vulns))
        .collect();
    Ok(super::list_response(&s, &uri, cp, total, items))
}

async fn get_one(State(s): State<AppState>, MaybeUser(_u): MaybeUser, AxPath(ghsa): AxPath<String>) -> ApiResult<Json<Value>> {
    let a: AdvisoryRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {ADV_COLS} FROM advisories a WHERE lower(a.ghsa_id) = lower($1)")))
        .bind(&ghsa)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let vulns = vulns_for(&s.db, &[a.id]).await?;
    Ok(Json(global_json(&s, &a, &vulns)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_ranges() {
        assert_eq!(cmp_versions("4.17.15", "4.17.19"), Ordering::Less);
        assert_eq!(cmp_versions("v1.2.0", "1.2"), Ordering::Equal);
        assert_eq!(cmp_versions("2.0.0-beta.1", "2.0.0"), Ordering::Less);
        assert_eq!(cmp_versions("2.0rc1", "2.0"), Ordering::Less);
        assert_eq!(cmp_versions("1.10.0", "1.9.9"), Ordering::Greater);
        let ivs = intervals(&serde_json::from_str::<Vec<Value>>(r#"[{"introduced":"0"},{"fixed":"4.17.19"}]"#).unwrap());
        assert_eq!(ivs.len(), 1);
        assert!(ivs[0].contains("4.17.15"));
        assert!(!ivs[0].contains("4.17.19"));
        assert_eq!(ivs[0].github_range(), "< 4.17.19");
        let iv = parse_github_range(">= 1.0.0, < 1.2.6");
        assert!(iv.contains("1.2.5") && !iv.contains("0.9") && !iv.contains("1.2.6"));
        assert_eq!(cvss3_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), Some(9.8));
        assert_eq!(cvss3_score("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:H/I:H/A:H"), Some(8.1));
    }
}
