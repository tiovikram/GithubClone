//! Dependency graph: `GET /dependency-graph/sbom` (SPDX 2.3 JSON), `GET
//! /dependency-graph/compare/{basehead}` and `POST /dependency-graph/snapshots`.
//!
//! The graph of a commit is built from the manifests in its tree (`package.json`,
//! `package-lock.json`, `Cargo.toml`, `Cargo.lock`, `go.mod`, `requirements.txt`) plus the
//! latest submitted snapshot per job correlator (for the commit, or for the ref when building
//! the default branch SBOM). `compare` reports each changed version's known vulnerabilities from
//! the advisory database (advsec area).
use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest;

use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let r = "/repos/{owner}/{repo}/dependency-graph";
    Router::new()
        .route(&format!("{r}/sbom"), get(sbom))
        .route(&format!("{r}/compare/{{*basehead}}"), get(compare))
        .route(&format!("{r}/snapshots"), post(create_snapshot))
}

/// One dependency of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dep {
    pub manifest: String,
    pub ecosystem: String,
    pub name: String,
    pub version: String,
    pub scope: String,
    pub purl: Option<String>,
}

const MANIFESTS: [&str; 6] = ["package.json", "package-lock.json", "Cargo.toml", "Cargo.lock", "go.mod", "requirements.txt"];

fn clean_version(v: &str) -> String {
    v.trim().trim_start_matches(|c: char| "^~=<>! v".contains(c)).split([',', ' ']).next().unwrap_or("").to_string()
}

fn purl(eco: &str, name: &str, version: &str) -> String {
    let (t, n) = match eco {
        "npm" => ("npm", name.replace('@', "%40")),
        "cargo" => ("cargo", name.to_string()),
        "golang" => ("golang", name.to_string()),
        "pip" => ("pypi", name.to_lowercase().replace('_', "-")),
        other => (other, name.to_string()),
    };
    let v = clean_version(version);
    if v.is_empty() { format!("pkg:{t}/{n}") } else { format!("pkg:{t}/{n}@{v}") }
}

fn dep(manifest: &str, eco: &str, name: &str, version: &str, scope: &str) -> Dep {
    Dep {
        manifest: manifest.into(),
        ecosystem: eco.into(),
        name: name.into(),
        version: version.trim().into(),
        scope: scope.into(),
        purl: Some(purl(eco, name, version)),
    }
}

/// Parses one manifest file into dependencies.
pub fn parse_manifest(path: &str, text: &str) -> Vec<Dep> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let mut out = Vec::new();
    match file {
        "package.json" => {
            let Ok(v) = serde_json::from_str::<Value>(text) else { return out };
            for (key, scope) in [("dependencies", "runtime"), ("optionalDependencies", "runtime"), ("peerDependencies", "runtime"), ("devDependencies", "development")] {
                for (n, ver) in v.get(key).and_then(Value::as_object).into_iter().flatten() {
                    out.push(dep(path, "npm", n, ver.as_str().unwrap_or(""), scope));
                }
            }
        }
        "package-lock.json" => {
            let Ok(v) = serde_json::from_str::<Value>(text) else { return out };
            if let Some(pkgs) = v.get("packages").and_then(Value::as_object) {
                for (k, p) in pkgs {
                    let Some(idx) = k.rfind("node_modules/") else { continue };
                    let name = &k[idx + "node_modules/".len()..];
                    let scope = if p.get("dev").and_then(Value::as_bool) == Some(true) { "development" } else { "runtime" };
                    out.push(dep(path, "npm", name, p.get("version").and_then(Value::as_str).unwrap_or(""), scope));
                }
            } else if let Some(deps) = v.get("dependencies").and_then(Value::as_object) {
                for (n, p) in deps {
                    let scope = if p.get("dev").and_then(Value::as_bool) == Some(true) { "development" } else { "runtime" };
                    out.push(dep(path, "npm", n, p.get("version").and_then(Value::as_str).unwrap_or(""), scope));
                }
            }
        }
        "Cargo.toml" => {
            let mut scope: Option<&str> = None;
            let mut table_dep: Option<(String, &str)> = None;
            for raw in text.lines() {
                let line = raw.split(" #").next().unwrap_or("").trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.starts_with('[') {
                    if let Some((n, sc)) = table_dep.take() {
                        out.push(dep(path, "cargo", &n, "", sc));
                    }
                    let sect = line.trim_matches(|c| c == '[' || c == ']').trim();
                    let last = sect.rsplit('.').next().unwrap_or(sect);
                    let kind = |k: &str| match k {
                        "dependencies" => Some("runtime"),
                        "build-dependencies" => Some("runtime"),
                        "dev-dependencies" => Some("development"),
                        _ => None,
                    };
                    scope = kind(last);
                    // [dependencies.foo]
                    if scope.is_none() {
                        let parts: Vec<&str> = sect.split('.').collect();
                        if parts.len() >= 2 {
                            if let Some(sc) = kind(parts[parts.len() - 2]) {
                                table_dep = Some((parts[parts.len() - 1].trim_matches('"').to_string(), sc));
                            }
                        }
                    }
                    continue;
                }
                let Some((k, v)) = line.split_once('=') else { continue };
                let (k, v) = (k.trim(), v.trim());
                if let Some((name, sc)) = &table_dep {
                    if k == "version" {
                        out.push(dep(path, "cargo", name, v.trim_matches('"'), sc));
                        table_dep = None;
                    }
                    continue;
                }
                let Some(sc) = scope else { continue };
                let name = k.split('.').next().unwrap_or(k).trim_matches('"');
                let version = if v.starts_with('"') {
                    v.trim_matches('"').to_string()
                } else if v.starts_with('{') {
                    v.split("version").nth(1).and_then(|r| r.split('"').nth(1)).unwrap_or("").to_string()
                } else {
                    String::new()
                };
                out.push(dep(path, "cargo", name, &version, sc));
            }
            if let Some((n, sc)) = table_dep {
                out.push(dep(path, "cargo", &n, "", sc));
            }
        }
        "Cargo.lock" => {
            let (mut name, mut version, mut source) = (None::<String>, None::<String>, false);
            let flush = |out: &mut Vec<Dep>, n: &mut Option<String>, v: &mut Option<String>, src: &mut bool| {
                if let (Some(nn), Some(vv), true) = (n.take(), v.take(), *src) {
                    out.push(dep(path, "cargo", &nn, &vv, "runtime"));
                }
                *src = false;
            };
            for line in text.lines() {
                let l = line.trim();
                if l == "[[package]]" {
                    flush(&mut out, &mut name, &mut version, &mut source);
                } else if let Some(v) = l.strip_prefix("name = ") {
                    name = Some(v.trim_matches('"').to_string());
                } else if let Some(v) = l.strip_prefix("version = ") {
                    version = Some(v.trim_matches('"').to_string());
                } else if l.starts_with("source = ") {
                    source = true;
                }
            }
            flush(&mut out, &mut name, &mut version, &mut source);
        }
        "go.mod" => {
            let mut block = false;
            for raw in text.lines() {
                let line = raw.split("//").next().unwrap_or("").trim();
                if line.starts_with("require (") || line == "require(" {
                    block = true;
                    continue;
                }
                if block && line == ")" {
                    block = false;
                    continue;
                }
                let spec = if block { Some(line) } else { line.strip_prefix("require ") };
                if let Some(spec) = spec {
                    let mut f = spec.split_whitespace();
                    if let (Some(m), Some(v)) = (f.next(), f.next()) {
                        out.push(dep(path, "golang", m, v, "runtime"));
                    }
                }
            }
        }
        "requirements.txt" => {
            for raw in text.lines() {
                let line = raw.split('#').next().unwrap_or("").trim();
                if line.is_empty() || line.starts_with('-') {
                    continue;
                }
                let line = line.split(';').next().unwrap_or(line).trim();
                let idx = line.find(|c: char| "=<>!~ [".contains(c)).unwrap_or(line.len());
                let name = line[..idx].trim();
                if name.is_empty() {
                    continue;
                }
                let rest = line[idx..].trim_start_matches(|c: char| c == '[' || c.is_alphanumeric() || c == ',' || c == ']');
                out.push(dep(path, "pip", name, rest.trim(), "runtime"));
            }
        }
        _ => {}
    }
    out
}

/// Dependencies declared by the manifests in the tree of `sha`.
async fn manifest_deps(s: &AppState, ctx: &RepoCtx, sha: &str) -> ApiResult<Vec<Dep>> {
    let path = s.git.sync(ctx.repo.id).await?;
    let git = &s.git.git;
    let entries = crate::git::read::ls_tree(git, &path, sha, None, true).await?;
    let files: Vec<_> = entries
        .into_iter()
        .filter(|e| e.kind == "blob" && MANIFESTS.contains(&e.name()) && !e.path.split('/').any(|seg| seg == "node_modules" || seg == "vendor") && e.size.unwrap_or(0) < 20 * 1024 * 1024)
        .collect();
    let revs: Vec<String> = files.iter().map(|e| e.sha.clone()).collect();
    let objs = crate::git::read::cat_objects(git, &path, &revs).await?;
    let mut out = Vec::new();
    for (e, o) in files.iter().zip(objs) {
        if let Some(o) = o {
            out.extend(parse_manifest(&e.path, &String::from_utf8_lossy(&o.data)));
        }
    }
    Ok(out)
}

/// Dependencies from the latest snapshot per job correlator (for `sha`, or for `git_ref`).
async fn snapshot_deps(s: &AppState, repo_id: i64, sha: Option<&str>, git_ref: Option<&str>) -> ApiResult<Vec<Dep>> {
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT DISTINCT ON (job_correlator) manifests FROM dependency_snapshots
          WHERE repo_id = $1 AND ($2::text IS NULL OR sha = $2) AND ($3::text IS NULL OR ref = $3)
          ORDER BY job_correlator, id DESC",
    )
    .bind(repo_id)
    .bind(sha)
    .bind(git_ref)
    .fetch_all(&s.db)
    .await?;
    let mut out = Vec::new();
    for manifests in rows {
        for (mname, m) in manifests.as_object().into_iter().flatten() {
            let mpath = m.pointer("/file/source_location").and_then(Value::as_str).unwrap_or(mname);
            for (pname, p) in m.get("resolved").and_then(Value::as_object).into_iter().flatten() {
                let purl = p.get("package_url").and_then(Value::as_str).map(str::to_string);
                // pkg:type/namespace/name@version
                let (eco, name, version) = match purl.as_deref().and_then(|u| u.strip_prefix("pkg:")) {
                    Some(rest) => {
                        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
                        let (t, nv) = rest.split_once('/').unwrap_or((rest, ""));
                        let (n, v) = nv.rsplit_once('@').unwrap_or((nv, ""));
                        let n = percent_encoding::percent_decode_str(n).decode_utf8_lossy().into_owned();
                        (if t == "pypi" { "pip".to_string() } else { t.to_string() }, n, v.to_string())
                    }
                    None => ("unknown".into(), pname.clone(), String::new()),
                };
                out.push(Dep {
                    manifest: mpath.to_string(),
                    ecosystem: eco,
                    name,
                    version,
                    scope: p.get("scope").and_then(Value::as_str).unwrap_or("runtime").to_string(),
                    purl,
                });
            }
        }
    }
    Ok(out)
}

pub async fn graph(s: &AppState, ctx: &RepoCtx, sha: &str, git_ref: Option<&str>) -> ApiResult<Vec<Dep>> {
    let mut deps = manifest_deps(s, ctx, sha).await?;
    deps.extend(match git_ref {
        Some(r) => snapshot_deps(s, ctx.repo.id, None, Some(r)).await?,
        None => snapshot_deps(s, ctx.repo.id, Some(sha), None).await?,
    });
    deps.sort();
    deps.dedup();
    Ok(deps)
}

fn spdx_id(d: &Dep) -> String {
    let raw = format!("{}-{}-{}", d.ecosystem, d.name, clean_version(&d.version));
    let id: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' }).collect();
    format!("SPDXRef-{id}")
}

async fn sbom(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let branch = ctx.repo.default_branch.clone();
    let refname = format!("refs/heads/{branch}");
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    let deps = match refs.get(&refname) {
        Some(sha) => graph(&s, &ctx, sha, Some(&refname)).await?,
        None => vec![],
    };
    let full = ctx.full_name();
    let html = format!("{}/{full}", s.base_url());
    let mut packages = vec![json!({
        "SPDXID": "SPDXRef-Repository",
        "name": format!("com.github.{full}"),
        "versionInfo": branch,
        "downloadLocation": format!("git+{html}"),
        "filesAnalyzed": false,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "NOASSERTION",
        "copyrightText": "NOASSERTION",
        "supplier": "NOASSERTION",
        "externalRefs": [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl", "referenceLocator": format!("pkg:github/{full}@{branch}")}],
    })];
    let mut relationships = vec![json!({"relationshipType": "DESCRIBES", "spdxElementId": "SPDXRef-DOCUMENT", "relatedSpdxElement": "SPDXRef-Repository"})];
    let mut seen = BTreeSet::new();
    for d in &deps {
        let id = spdx_id(d);
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut p = json!({
            "SPDXID": id, "name": format!("{}:{}", d.ecosystem, d.name), "versionInfo": clean_version(&d.version),
            "downloadLocation": "NOASSERTION", "filesAnalyzed": false,
            "licenseConcluded": "NOASSERTION", "licenseDeclared": "NOASSERTION", "copyrightText": "NOASSERTION", "supplier": "NOASSERTION",
            "externalRefs": [],
        });
        if let Some(purl) = &d.purl {
            p["externalRefs"] = json!([{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl", "referenceLocator": purl}]);
        }
        packages.push(p);
        relationships.push(json!({"relationshipType": "DEPENDS_ON", "spdxElementId": "SPDXRef-Repository", "relatedSpdxElement": id}));
    }
    let digest = hex::encode(sha2::Sha256::digest(serde_json::to_vec(&packages).unwrap_or_default()));
    Ok(Json(json!({"sbom": {
        "SPDXID": "SPDXRef-DOCUMENT",
        "spdxVersion": "SPDX-2.3",
        "creationInfo": {"created": crate::json::ts(chrono::Utc::now()), "creators": ["Tool: forge-dependency-graph"]},
        "name": format!("com.github.{full}"),
        "dataLicense": "CC0-1.0",
        "documentDescribes": ["SPDXRef-Repository"],
        "documentNamespace": format!("{html}/dependency_graph/sbom-{}", &digest[..16]),
        "comment": "Generated by forge from the manifests of the default branch and submitted dependency snapshots.",
        "packages": packages,
        "relationships": relationships,
    }})))
}

#[derive(Deserialize)]
struct CompareQ {
    name: Option<String>,
}

async fn compare(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((o, r, basehead)): Path<(String, String, String)>, Query(q): Query<CompareQ>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &o, &r).await?;
    let Some((base, head)) = basehead.split_once("...") else {
        return Ok(ApiError::bad_request("The basehead must be in the format BASE...HEAD").into_response());
    };
    let path = s.git.sync(ctx.repo.id).await?;
    let git = &s.git.git;
    let resolve = |rev: &str| {
        let rev = rev.to_string();
        let path = path.clone();
        async move {
            if !crate::git::read::safe_rev(&rev) {
                return Ok::<Option<String>, ApiError>(None);
            }
            Ok(crate::git::read::resolve_commit(git, &path, &rev).await?)
        }
    };
    let (Some(b), Some(h)) = (resolve(base).await?, resolve(head).await?) else { return Err(ApiError::not_found()) };
    // GitHub compares the head against the merge base of the two commits.
    let b = crate::git::read::merge_base(git, &path, &b, &h).await?.unwrap_or(b);
    let old: BTreeSet<Dep> = graph(&s, &ctx, &b, None).await?.into_iter().collect();
    let new: BTreeSet<Dep> = graph(&s, &ctx, &h, None).await?.into_iter().collect();
    let mut out = Vec::new();
    let key = |d: &Dep| (d.manifest.clone(), d.ecosystem.clone(), d.name.clone(), d.version.clone());
    let old_keys: BTreeMap<_, &Dep> = old.iter().map(|d| (key(d), d)).collect();
    let new_keys: BTreeMap<_, &Dep> = new.iter().map(|d| (key(d), d)).collect();
    // Known vulnerabilities of each version (advisory database), keyed like `key`.
    let deps: Vec<&Dep> = old_keys.values().chain(new_keys.values()).copied().collect();
    let vulns = crate::advsec::advisories::vulnerabilities_of(&s, &deps.iter().map(|d| (d.ecosystem.as_str(), d.name.as_str(), d.version.as_str())).collect::<Vec<_>>()).await?;
    let vuln_of: BTreeMap<_, Value> = deps.iter().zip(vulns).map(|(d, v)| (key(d), Value::Array(v))).collect();
    let render = |kind: &str, d: &Dep| {
        json!({
            "change_type": kind, "manifest": d.manifest, "ecosystem": d.ecosystem, "name": d.name, "version": d.version,
            "package_url": d.purl, "license": null, "source_repository_url": null,
            "vulnerabilities": vuln_of.get(&key(d)).cloned().unwrap_or(json!([])), "scope": d.scope,
        })
    };
    for (k, d) in &old_keys {
        if !new_keys.contains_key(k) && q.name.as_deref().is_none_or(|n| n == d.manifest) {
            out.push(render("removed", d));
        }
    }
    for (k, d) in &new_keys {
        if !old_keys.contains_key(k) && q.name.as_deref().is_none_or(|n| n == d.manifest) {
            out.push(render("added", d));
        }
    }
    Ok(axum::Json(Value::Array(out)).into_response())
}

#[derive(Deserialize)]
struct SnapshotReq {
    version: Option<i64>,
    job: Option<Value>,
    sha: Option<String>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    detector: Option<Value>,
    metadata: Option<Value>,
    manifests: Option<Value>,
    scanned: Option<String>,
}

async fn create_snapshot(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Json(b): Json<SnapshotReq>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Write)?;
    let missing = |f: &str| ApiError::validation("Snapshot", f, "missing_field");
    b.version.ok_or_else(|| missing("version"))?;
    let job = b.job.filter(Value::is_object).ok_or_else(|| missing("job"))?;
    let correlator = job.get("correlator").and_then(Value::as_str).filter(|c| !c.is_empty()).ok_or_else(|| missing("job.correlator"))?.to_string();
    job.get("id").and_then(Value::as_str).ok_or_else(|| missing("job.id"))?;
    let sha = b.sha.filter(|x| x.len() == 40 && x.chars().all(|c| c.is_ascii_hexdigit())).ok_or_else(|| ApiError::validation("Snapshot", "sha", "invalid"))?;
    let git_ref = b.git_ref.filter(|x| x.starts_with("refs/")).ok_or_else(|| ApiError::validation("Snapshot", "ref", "invalid"))?;
    let detector = b.detector.filter(|d| ["name", "version", "url"].iter().all(|k| d.get(k).is_some_and(Value::is_string))).ok_or_else(|| missing("detector"))?;
    let scanned = b
        .scanned
        .as_deref()
        .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .ok_or_else(|| ApiError::validation("Snapshot", "scanned", "invalid"))?;
    let manifests = b.manifests.unwrap_or_else(|| json!({}));
    if !manifests.is_object() {
        return Err(ApiError::validation("Snapshot", "manifests", "invalid"));
    }
    let row: (i64, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "INSERT INTO dependency_snapshots (repo_id, sha, ref, job_correlator, detector, job, metadata, manifests, scanned)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id, created_at",
    )
    .bind(ctx.repo.id)
    .bind(sha.to_lowercase())
    .bind(&git_ref)
    .bind(&correlator)
    .bind(&detector)
    .bind(&job)
    .bind(b.metadata.unwrap_or_else(|| json!({})))
    .bind(&manifests)
    .bind(scanned)
    .fetch_one(&s.db)
    .await?;
    Ok((
        StatusCode::CREATED,
        axum::Json(json!({"id": row.0, "created_at": crate::json::ts(row.1), "result": "SUCCESS",
            "message": "Dependency results for the repo have been successfully updated."})),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::parse_manifest;

    #[test]
    fn manifests() {
        let pj = parse_manifest("package.json", r#"{"dependencies":{"left-pad":"^1.3.0","@scope/x":"2.0.0"},"devDependencies":{"jest":"~29.1.0"}}"#);
        assert_eq!(pj.len(), 3);
        assert!(pj.iter().any(|d| d.name == "@scope/x" && d.purl.as_deref() == Some("pkg:npm/%40scope/x@2.0.0")));
        assert!(pj.iter().any(|d| d.name == "jest" && d.scope == "development"));
        let ct = parse_manifest("Cargo.toml", "[package]\nname = \"a\"\n[dependencies]\nserde = \"1.0\"\ntokio = { version = \"1.2\", features = [\"full\"] }\n[dev-dependencies]\ntempfile = \"3\"\n[dependencies.foo]\nversion = \"0.1\"\n");
        let names: Vec<_> = ct.iter().map(|d| (d.name.as_str(), d.version.as_str(), d.scope.as_str())).collect();
        assert!(names.contains(&("serde", "1.0", "runtime")));
        assert!(names.contains(&("tokio", "1.2", "runtime")));
        assert!(names.contains(&("tempfile", "3", "development")));
        assert!(names.contains(&("foo", "0.1", "runtime")));
        let gm = parse_manifest("go.mod", "module x\n\nrequire github.com/a/b v1.2.3\nrequire (\n\tgolang.org/x/net v0.1.0 // indirect\n)\n");
        assert_eq!(gm.len(), 2);
        let rq = parse_manifest("requirements.txt", "# c\nrequests==2.31.0\nflask>=2.0\n-r other.txt\n");
        assert_eq!(rq.len(), 2);
        assert_eq!(rq[0].purl.as_deref(), Some("pkg:pypi/requests@2.31.0"));
    }
}
