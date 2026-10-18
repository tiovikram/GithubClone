//! Advanced security: secret scanning of pushes (alerts with locations, push protection and
//! bypasses, auto-revocation of forge tokens), Dependabot alerts from the advisory fixture,
//! code scanning SARIF processing, code security configurations and delegated requests.
mod common;

use std::io::Write;

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("fixtures/advisories/osv-sample.json");

/// A fake AWS key id (random, so nothing secret-looking is committed).
fn aws_key() -> String {
    let a = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let r = forge::auth::random_string(16);
    format!("AKIA{}", r.bytes().map(|b| a[(b as usize) % a.len()] as char).collect::<String>())
}

async fn site_admin(s: &TestServer) -> TestUser {
    let login = unique("adm");
    let u = forge::forge_api::create_user(&s.state, &login, Some(&format!("{login}@example.com")), "password123", true).await.map_err(|e| e.message).unwrap();
    let (_, token) = forge::forge_api::create_token_for(&s.state, u.id, "test").await.map_err(|e| e.message).unwrap();
    TestUser { login, token }
}

async fn create_org(s: &TestServer, u: &TestUser) -> String {
    let login = unique("org");
    let res = s.client().post(format!("{}/api/forge/orgs", s.base)).header("Authorization", format!("token {}", u.token)).json(&json!({"login": login})).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 201);
    login
}

async fn repo_id(s: &TestServer, u: &TestUser, rp: &str) -> i64 {
    s.call(Some(u), "GET", rp, None).await.1["id"].as_i64().unwrap()
}

/// Runs this repository's advsec jobs (secret scans, Dependabot matches, SARIF processing)
/// until none are left. Jobs are taken by repository, so parallel tests (each with its own
/// object storage) never run each other's jobs.
async fn drain(s: &TestServer, repo_id: i64) {
    for _ in 0..20 {
        let jobs: Vec<(i64, String, Value)> = sqlx::query_as(
            "DELETE FROM jobs WHERE queue LIKE 'advsec.%' AND (payload->>'repo_id')::bigint = $1 RETURNING id, queue, payload",
        )
        .bind(repo_id)
        .fetch_all(&s.state.db)
        .await
        .unwrap();
        if jobs.is_empty() {
            return;
        }
        for (id, queue, payload) in jobs {
            let job = forge::jobs::Job { id, queue, payload, attempts: 1, max_attempts: 1 };
            forge::advsec::run_job(&s.state, &job).await.unwrap();
        }
    }
}

async fn enable(s: &TestServer, u: &TestUser, rp: &str, products: &[&str]) {
    let mut sa = serde_json::Map::new();
    for p in products {
        sa.insert(p.to_string(), json!({"status": "enabled"}));
    }
    let (st, b) = s.call(Some(u), "PATCH", rp, Some(json!({"security_and_analysis": sa}))).await;
    assert_eq!(st, 200, "{b}");
}

fn commit_file(s: &TestServer, dir: &std::path::Path, path: &str, content: &str, msg: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::File::create(&full).unwrap().write_all(content.as_bytes()).unwrap();
    assert!(s.git(dir, &["add", "-A"]).0);
    assert!(s.git(dir, &["commit", "-q", "-m", msg]).0);
}

fn clone(s: &TestServer, u: &TestUser, repo: &str) -> std::path::PathBuf {
    let dir = s.work_dir(&unique("w"));
    let (ok, out) = s.git(&dir, &["clone", "-q", &s.clone_url(u, repo), "."]);
    assert!(ok, "{out}");
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pushed_secrets_become_alerts_with_locations() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let id = repo_id(&s, &u, &rp).await;

    // Disabled by default: 404 and nothing is scanned.
    assert_eq!(s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/alerts"), None).await.0, 404);
    let dir = clone(&s, &u, &repo);
    let key = aws_key();
    commit_file(&s, &dir, "early.txt", &format!("key {key}\n"), "early secret");
    assert!(s.git(&dir, &["push", "-q", "origin", "HEAD"]).0);
    drain(&s, id).await;

    // Enabling runs a backfill over the history.
    enable(&s, &u, &rp, &["secret_scanning"]).await;
    drain(&s, id).await;
    let (st, alerts) = s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/alerts"), None).await;
    assert_eq!(st, 200, "{alerts}");
    assert_eq!(alerts.as_array().unwrap().len(), 1, "{alerts}");
    assert_eq!(alerts[0]["secret_type"], "aws_access_key_id");
    assert_eq!(alerts[0]["secret"], key);

    // An incremental scan of a new push: one alert per secret, a location per file.
    let pat = format!("ghp_{}", forge::auth::random_string(36));
    commit_file(&s, &dir, "src/config.py", &format!("# settings\nTOKEN = \"{pat}\"\nAWS = \"{key}\"\n"), "more secrets");
    assert!(s.git(&dir, &["push", "-q", "origin", "HEAD"]).0);
    let (_, head) = s.git(&dir, &["rev-parse", "HEAD"]);
    drain(&s, id).await;
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/alerts?sort=created&direction=asc"), None).await;
    let alerts = alerts.as_array().unwrap().clone();
    assert_eq!(alerts.len(), 2, "{alerts:?}");
    let gh = alerts.iter().find(|a| a["secret_type"] == "github_personal_access_token").expect("PAT alert");
    assert_eq!(gh["provider"], "GitHub");
    assert_eq!(gh["first_location_detected"]["path"], "src/config.py");
    assert_eq!(gh["first_location_detected"]["start_line"], 2);
    assert_eq!(gh["first_location_detected"]["commit_sha"], head.trim());
    let aws = alerts.iter().find(|a| a["secret_type"] == "aws_access_key_id").unwrap();
    let n = aws["number"].as_i64().unwrap();
    let (_, locs) = s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/alerts/{n}/locations"), None).await;
    let paths: Vec<&str> = locs.as_array().unwrap().iter().map(|l| l["details"]["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"early.txt") && paths.contains(&"src/config.py"), "{locs}");
    assert_eq!(aws["has_more_locations"], true);

    // Resolution requires a reason; reopening clears it.
    let ap = format!("{rp}/secret-scanning/alerts/{n}");
    assert_eq!(s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "resolved"}))).await.0, 422);
    let (st, a) = s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "resolved", "resolution": "revoked", "resolution_comment": "rotated"}))).await;
    assert_eq!((st, a["state"].as_str(), a["resolution"].as_str()), (200, Some("resolved"), Some("revoked")));
    assert_eq!(a["resolved_by"]["login"], u.login);
    let (_, a) = s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "open"}))).await;
    assert_eq!(a["resolution"], Value::Null);
    let (_, open) = s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/alerts?state=resolved"), None).await;
    assert_eq!(open.as_array().unwrap().len(), 0);
    let (_, hidden) = s.call(Some(&u), "GET", &format!("{ap}?hide_secret=true"), None).await;
    assert!(hidden.get("secret").is_none());

    // Scan history records the backfill and the incremental scans.
    let (_, h) = s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/scan-history"), None).await;
    assert_eq!(h["backfill_scans"][0]["status"], "completed", "{h}");
    assert!(!h["incremental_scans"].as_array().unwrap().is_empty());

    // Others: write collaborators cannot see secret scanning alerts (admin only); strangers get 404.
    let other = s.user().await;
    assert_eq!(s.call(Some(&other), "GET", &format!("{rp}/secret-scanning/alerts"), None).await.0, 403, "public repo: readable but not alerts");
    assert_eq!(s.call(None, "GET", &format!("{rp}/secret-scanning/alerts"), None).await.0, 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_protection_blocks_secrets_until_bypassed() {
    // The pre-receive hook runs the forge binary.
    unsafe { std::env::set_var("FORGE_HOOK_BIN", env!("CARGO_BIN_EXE_forge")) };
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let id = repo_id(&s, &u, &rp).await;
    enable(&s, &u, &rp, &["secret_scanning", "secret_scanning_push_protection"]).await;
    drain(&s, id).await;

    let dir = clone(&s, &u, &repo);
    let key = aws_key();
    commit_file(&s, &dir, "deploy/creds.env", &format!("AWS_ACCESS_KEY_ID={key}\n"), "creds");
    let (ok, out) = s.git(&dir, &["push", "origin", "HEAD"]);
    assert!(!ok, "push with a secret must be rejected: {out}");
    assert!(out.contains("GH013") && out.contains("Amazon AWS Access Key ID") && out.contains("deploy/creds.env:1"), "{out}");
    let placeholder = out.split("unblock-secret/").nth(1).map(|x| x.split_whitespace().next().unwrap().to_string()).expect("unblock URL");
    // Nothing reached the repository.
    let (_, refs) = s.state.git.refs(id).await.unwrap();
    let (_, local) = s.git(&dir, &["rev-parse", "HEAD"]);
    assert!(!refs.values().any(|v| v == local.trim()));

    // Unknown placeholders are 404; a bypass allows exactly this secret.
    let bp = format!("{rp}/secret-scanning/push-protection-bypasses");
    assert_eq!(s.call(Some(&u), "POST", &bp, Some(json!({"reason": "used_in_tests", "placeholder_id": "nope"}))).await.0, 404);
    assert_eq!(s.call(Some(&u), "POST", &bp, Some(json!({"reason": "because", "placeholder_id": placeholder}))).await.0, 422);
    let (st, b) = s.call(Some(&u), "POST", &bp, Some(json!({"reason": "used_in_tests", "placeholder_id": placeholder}))).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["token_type"], "aws_access_key_id");
    assert!(b["expire_at"].is_string());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (ok, out) = s.git(&dir, &["push", "origin", "HEAD"]);
    assert!(ok, "bypassed secret must be accepted: {out}");
    drain(&s, id).await;
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/alerts?is_bypassed=true"), None).await;
    assert_eq!(alerts.as_array().unwrap().len(), 1, "{alerts}");
    assert_eq!(alerts[0]["push_protection_bypassed"], true);
    assert_eq!(alerts[0]["push_protection_bypassed_by"]["login"], u.login);

    // A different secret is still blocked; files without secrets push fine.
    commit_file(&s, &dir, "other.txt", &format!("{}\n", aws_key()), "another");
    assert!(!s.git(&dir, &["push", "origin", "HEAD"]).0);
    assert!(s.git(&dir, &["reset", "-q", "--hard", "HEAD~1"]).0);
    commit_file(&s, &dir, "readme2.md", "no secrets here\n", "clean");
    let (ok, out) = s.git(&dir, &["push", "origin", "HEAD"]);
    assert!(ok, "{out}");

    // The contents API is push protected too: 409 with the placeholder.
    let content = base64_encode(&format!("token: {}\n", aws_key()));
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/contents/api.txt"), Some(json!({"message": "api", "content": content}))).await;
    assert_eq!(st, 409, "{e}");
    let ph = e["errors"][0]["placeholder_id"].as_str().unwrap().to_string();
    let (st, _) = s.call(Some(&u), "POST", &bp, Some(json!({"reason": "false_positive", "placeholder_id": ph}))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/api.txt"), Some(json!({"message": "api", "content": content}))).await;
    assert_eq!(st, 201);
}

fn base64_encode(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forge_tokens_in_public_repos_are_revoked() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let victim = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let id = repo_id(&s, &u, &rp).await;
    enable(&s, &u, &rp, &["secret_scanning"]).await;
    drain(&s, id).await;
    assert_eq!(s.call(Some(&victim), "GET", "/user", None).await.0, 200);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/leak.sh"), Some(json!({"message": "oops", "content": base64_encode(&format!("export GH_TOKEN={}\n", victim.token))}))).await;
    assert_eq!(st, 201);
    drain(&s, id).await;
    // The token no longer authenticates; the alert says why.
    assert_eq!(s.call(Some(&victim), "GET", "/user", None).await.0, 401);
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/secret-scanning/alerts"), None).await;
    let a = &alerts[0];
    assert_eq!((a["secret_type"].as_str(), a["state"].as_str(), a["resolution"].as_str(), a["validity"].as_str()), (Some("github_oauth_access_token"), Some("resolved"), Some("revoked"), Some("inactive")), "{a}");
}

async fn import_fixture(s: &TestServer, admin: &TestUser) {
    let res = s
        .client()
        .post(format!("{}/api/forge/advisories", s.base))
        .header("Authorization", format!("token {}", admin.token))
        .body(FIXTURE)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    // The import queues a re-match of every enabled repository in the (shared) test database;
    // tests match their own repositories through `drain`.
    sqlx::query("DELETE FROM jobs WHERE queue = $1").bind(forge::advsec::Q_ADVISORY_RESCAN).execute(&s.state.db).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advisories_are_imported_and_queryable() {
    let s = TestServer::start().await;
    let admin = site_admin(&s).await;
    let u = s.user().await;
    // Only site admins import.
    let res = s.client().post(format!("{}/api/forge/advisories", s.base)).header("Authorization", format!("token {}", u.token)).body(FIXTURE).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 403);
    import_fixture(&s, &admin).await;
    let (st, a) = s.call(None, "GET", "/advisories/GHSA-xvch-5gv4-984h", None).await;
    assert_eq!(st, 200);
    assert_eq!(a["cve_id"], "CVE-2021-44906");
    assert_eq!(a["severity"], "critical");
    assert_eq!(a["cvss"]["score"], 9.8);
    assert_eq!(a["vulnerabilities"].as_array().unwrap().len(), 2);
    assert_eq!(a["vulnerabilities"][1]["vulnerable_version_range"], ">= 1.0.0, < 1.2.6");
    let (_, l) = s.call(None, "GET", "/advisories?affects=lodash@4.17.15", None).await;
    assert!(l.as_array().unwrap().iter().any(|x| x["ghsa_id"] == "GHSA-p6mc-m468-83gw"));
    let (_, l) = s.call(None, "GET", "/advisories?affects=lodash@4.17.21", None).await;
    assert!(!l.as_array().unwrap().iter().any(|x| x["ghsa_id"] == "GHSA-p6mc-m468-83gw"));
    let (_, l) = s.call(None, "GET", "/advisories?ecosystem=rust&cwes=476", None).await;
    assert!(l.as_array().unwrap().iter().any(|x| x["ghsa_id"] == "GHSA-wcg3-cvx6-7396"));
    // Records without a GHSA id are imported under their GHSA alias, unreviewed.
    let (_, l) = s.call(None, "GET", "/advisories?type=unreviewed&ghsa_id=GHSA-fake-forg-e000", None).await;
    assert_eq!(l.as_array().unwrap().len(), 1);
    assert_eq!(s.call(None, "GET", "/advisories?severity=bogus", None).await.0, 422);
    assert_eq!(s.call(None, "GET", "/advisories/GHSA-0000-0000-0000", None).await.0, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dependabot_alerts_follow_the_manifests() {
    let s = TestServer::start().await;
    let admin = site_admin(&s).await;
    import_fixture(&s, &admin).await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let id = repo_id(&s, &u, &rp).await;
    assert_eq!(s.call(Some(&u), "GET", &format!("{rp}/dependabot/alerts"), None).await.0, 403, "disabled until enabled");
    assert_eq!(s.call(Some(&u), "PUT", &format!("{rp}/vulnerability-alerts"), None).await.0, 204);
    let pkg = json!({"dependencies": {"lodash": "^4.17.15", "minimist": "1.2.5"}, "devDependencies": {"semver": "7.3.0"}});
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/package.json"), Some(json!({"message": "deps", "content": base64_encode(&pkg.to_string())}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/requirements.txt"), Some(json!({"message": "py", "content": base64_encode("requests==2.28.0\n")}))).await;
    assert_eq!(st, 201);
    drain(&s, id).await;
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/dependabot/alerts?sort=created&direction=asc"), None).await;
    let alerts = alerts.as_array().unwrap().clone();
    let by = |pkg: &str| alerts.iter().find(|a| a["dependency"]["package"]["name"] == pkg).cloned().unwrap_or_else(|| panic!("{pkg}: {alerts:?}"));
    let lodash = by("lodash");
    assert_eq!(lodash["state"], "open");
    assert_eq!(lodash["security_advisory"]["ghsa_id"], "GHSA-p6mc-m468-83gw");
    assert_eq!(lodash["security_vulnerability"]["first_patched_version"]["identifier"], "4.17.19");
    assert_eq!(lodash["dependency"]["manifest_path"], "package.json");
    assert_eq!(lodash["dependency"]["scope"], "runtime");
    assert_eq!(by("minimist")["security_advisory"]["severity"], "critical");
    assert_eq!(by("requests")["dependency"]["package"]["ecosystem"], "pip");
    // ReDoS in a development dependency is auto-dismissed by the preset rule.
    let semver = by("semver");
    assert_eq!(semver["state"], "auto_dismissed");
    assert!(semver["auto_dismissed_at"].is_string());

    // Filters.
    let (_, l) = s.call(Some(&u), "GET", &format!("{rp}/dependabot/alerts?ecosystem=pip"), None).await;
    assert_eq!(l.as_array().unwrap().len(), 1);
    let (_, l) = s.call(Some(&u), "GET", &format!("{rp}/dependabot/alerts?state=open&severity=critical"), None).await;
    assert_eq!(l.as_array().unwrap().len(), 1);

    // Dismissal needs a reason, then reopening works.
    let n = lodash["number"].as_i64().unwrap();
    let ap = format!("{rp}/dependabot/alerts/{n}");
    assert_eq!(s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "dismissed"}))).await.0, 422);
    let (st, a) = s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "dismissed", "dismissed_reason": "tolerable_risk", "assignees": [u.login]}))).await;
    assert_eq!((st, a["state"].as_str()), (200, Some("dismissed")), "{a}");
    assert_eq!(a["dismissed_by"]["login"], u.login);
    assert_eq!(a["assignees"][0]["login"], u.login);
    let (_, a) = s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "open"}))).await;
    assert_eq!(a["state"], "open");

    // Upgrading the dependency fixes the alert; the pip alert stays open.
    let (_, cur) = s.call(Some(&u), "GET", &format!("{rp}/contents/package.json"), None).await;
    let pkg = json!({"dependencies": {"lodash": "4.17.21", "minimist": "1.2.5"}, "devDependencies": {"semver": "7.3.0"}});
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/package.json"), Some(json!({"message": "bump", "content": base64_encode(&pkg.to_string()), "sha": cur["sha"]}))).await;
    assert_eq!(st, 200);
    drain(&s, id).await;
    let (_, a) = s.call(Some(&u), "GET", &ap, None).await;
    assert_eq!(a["state"], "fixed", "{a}");
    assert!(a["fixed_at"].is_string());
    let (_, l) = s.call(Some(&u), "GET", &format!("{rp}/dependabot/alerts?state=open"), None).await;
    assert_eq!(l.as_array().unwrap().len(), 2, "minimist + requests");

    // Readers without write access are refused.
    let other = s.user().await;
    assert_eq!(s.call(Some(&other), "GET", &format!("{rp}/dependabot/alerts"), None).await.0, 403);
}

fn sarif(results: Value) -> String {
    use base64::Engine;
    let doc = json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "lint", "semanticVersion": "2.0.0", "rules": [
            {"id": "r/eval", "name": "NoEval", "shortDescription": {"text": "No eval"}, "defaultConfiguration": {"level": "error"}, "properties": {"security-severity": "9.1", "tags": ["security"]}},
            {"id": "r/style", "shortDescription": {"text": "Style"}}
        ]}},
        "automationDetails": {"id": "lint/1"},
        "results": results,
    }]});
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(doc.to_string().as_bytes()).unwrap();
    base64::engine::general_purpose::STANDARD.encode(gz.finish().unwrap())
}

fn result(rule: &str, line: i64) -> Value {
    json!({"ruleId": rule, "message": {"text": format!("{rule} at {line}")},
        "locations": [{"physicalLocation": {"artifactLocation": {"uri": "src/a.js"}, "region": {"startLine": line}}}]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sarif_uploads_become_analyses_and_alerts() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let id = repo_id(&s, &u, &rp).await;
    let (st, put) = s.call(Some(&u), "PUT", &format!("{rp}/contents/src/a.js"), Some(json!({"message": "code", "content": base64_encode("eval(x)\nvar  y=1\nvar z\n")}))).await;
    assert_eq!(st, 201);
    let sha = put["commit"]["sha"].as_str().unwrap().to_string();
    let up = |results: Value| {
        let body = json!({"commit_sha": sha, "ref": "refs/heads/main", "sarif": sarif(results)});
        let rp = rp.clone();
        let s = &s;
        let u = &u;
        async move { s.call(Some(u), "POST", &format!("{rp}/code-scanning/sarifs"), Some(body)).await }
    };
    // Validation.
    assert_eq!(s.call(Some(&u), "POST", &format!("{rp}/code-scanning/sarifs"), Some(json!({"commit_sha": sha, "ref": "refs/heads/main", "sarif": "not-base64!"}))).await.0, 400);
    assert_eq!(s.call(Some(&u), "POST", &format!("{rp}/code-scanning/sarifs"), Some(json!({"commit_sha": "abc", "ref": "refs/heads/main", "sarif": sarif(json!([]))}))).await.0, 422);

    let (st, r1) = up(json!([result("r/eval", 1), result("r/style", 2)])).await;
    assert_eq!(st, 202, "{r1}");
    let sarif_id = r1["id"].as_str().unwrap().to_string();
    let (_, st1) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/sarifs/{sarif_id}"), None).await;
    assert_eq!(st1["processing_status"], "pending");
    drain(&s, id).await;
    let (_, st1) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/sarifs/{sarif_id}"), None).await;
    assert_eq!(st1["processing_status"], "complete", "{st1}");
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/alerts?sort=created&direction=asc"), None).await;
    let alerts = alerts.as_array().unwrap().clone();
    assert_eq!(alerts.len(), 2, "{alerts:?}");
    let ev = alerts.iter().find(|a| a["rule"]["id"] == "r/eval").unwrap().clone();
    assert_eq!(ev["rule"]["security_severity_level"], "critical");
    assert_eq!(ev["rule"]["severity"], "error");
    assert_eq!(ev["tool"]["name"], "lint");
    assert_eq!(ev["most_recent_instance"]["category"], "lint/");
    assert_eq!(ev["most_recent_instance"]["location"]["path"], "src/a.js");
    let (_, l) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/alerts?severity=critical"), None).await;
    assert_eq!(l.as_array().unwrap().len(), 1);

    // The same results (shifted by a line, same source text) dedupe; the style finding is
    // gone, so that alert is fixed.
    let (st, _) = up(json!([result("r/eval", 1)])).await;
    assert_eq!(st, 202);
    drain(&s, id).await;
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/alerts"), None).await;
    assert_eq!(alerts.as_array().unwrap().len(), 2);
    let style = alerts.as_array().unwrap().iter().find(|a| a["rule"]["id"] == "r/style").unwrap().clone();
    assert_eq!(style["state"], "fixed", "{style}");
    let (_, inst) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/alerts/{}/instances", style["number"]), None).await;
    assert_eq!(inst[0]["state"], "fixed");

    // Analyses: only the latest of a set is deletable; deleting it brings the style alert back.
    let (_, an) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/analyses"), None).await;
    let an = an.as_array().unwrap().clone();
    assert_eq!(an.len(), 2);
    assert_eq!((an[0]["deletable"].as_bool(), an[1]["deletable"].as_bool()), (Some(true), Some(false)));
    assert_eq!(s.call(Some(&u), "DELETE", &format!("{rp}/code-scanning/analyses/{}", an[1]["id"]), None).await.0, 400);
    let (st, d) = s.call(Some(&u), "DELETE", &format!("{rp}/code-scanning/analyses/{}", an[0]["id"]), None).await;
    assert_eq!(st, 200, "{d}");
    assert!(d["confirm_delete_url"].as_str().unwrap().ends_with("?confirm_delete"), "{d}");
    let (_, st2) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/alerts/{}", style["number"]), None).await;
    assert_eq!(st2["state"], "open");
    // The last analysis of a set needs confirm_delete.
    assert_eq!(s.call(Some(&u), "DELETE", &format!("{rp}/code-scanning/analyses/{}", an[1]["id"]), None).await.0, 400);
    assert_eq!(s.call(Some(&u), "DELETE", &format!("{rp}/code-scanning/analyses/{}?confirm_delete", an[1]["id"]), None).await.0, 200);
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/alerts"), None).await;
    assert_eq!(alerts.as_array().unwrap().len(), 0, "alerts go with their last analysis");

    // Dismissal.
    up(json!([result("r/eval", 1)])).await;
    drain(&s, id).await;
    let (_, alerts) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/alerts"), None).await;
    let n = alerts[0]["number"].as_i64().unwrap();
    let ap = format!("{rp}/code-scanning/alerts/{n}");
    assert_eq!(s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "dismissed", "dismissed_reason": "nope"}))).await.0, 422);
    let (_, a) = s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "dismissed", "dismissed_reason": "false positive"}))).await;
    assert_eq!(a["state"], "dismissed");
    // Requests need delegated dismissal.
    assert_eq!(s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "dismissed", "dismissed_reason": "won't fix", "create_request": true}))).await.0, 422);
    let (_, a) = s.call(Some(&u), "PATCH", &ap, Some(json!({"state": "open"}))).await;
    assert_eq!(a["state"], "open");

    // Default setup is stored.
    let (st, d) = s.call(Some(&u), "PATCH", &format!("{rp}/code-scanning/default-setup"), Some(json!({"state": "configured", "query_suite": "extended"}))).await;
    assert_eq!(st, 200, "{d}");
    let (_, d) = s.call(Some(&u), "GET", &format!("{rp}/code-scanning/default-setup"), None).await;
    assert_eq!((d["state"].as_str(), d["query_suite"].as_str()), (Some("configured"), Some("extended")));
    assert_eq!(d["languages"], json!(["javascript-typescript"]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configurations_enable_features_and_delegate_dismissal() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = create_org(&s, &owner).await;
    let name = unique("r");
    let (st, r) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": name, "private": true, "auto_init": true}))).await;
    assert_eq!(st, 201, "{r}");
    let rid = r["id"].as_i64().unwrap();
    let rp = format!("/repos/{org}/{name}");
    // Members are not security managers.
    assert_eq!(s.call(Some(&owner), "PUT", &format!("/orgs/{org}/memberships/{}", member.login), Some(json!({"role": "member"}))).await.0, 200);
    assert_eq!(s.call(Some(&member), "GET", &format!("/orgs/{org}/code-security/configurations"), None).await.0, 403);

    let base = format!("/orgs/{org}/code-security/configurations");
    let (st, c) = s.call(Some(&owner), "POST", &base, Some(json!({"name": "strict", "secret_scanning": "enabled", "dependabot_alerts": "enabled",
        "secret_scanning_delegated_alert_dismissal": "enabled", "dependabot_delegated_alert_dismissal": "enabled", "enforcement": "enforced"}))).await;
    assert_eq!(st, 201, "{c}");
    assert_eq!(c["dependency_graph"], "enabled", "GitHub's defaults fill unset fields");
    assert_eq!(c["secret_scanning_push_protection"], "disabled");
    assert_eq!(s.call(Some(&owner), "POST", &base, Some(json!({"name": "strict"}))).await.0, 422, "names are unique");
    assert_eq!(s.call(Some(&owner), "POST", &base, Some(json!({"name": "bad", "secret_scanning": "on"}))).await.0, 422);
    let cid = c["id"].as_i64().unwrap();
    let (st, _) = s.call(Some(&owner), "POST", &format!("{base}/{cid}/attach"), Some(json!({"scope": "selected", "selected_repository_ids": [rid]}))).await;
    assert_eq!(st, 202);
    assert!(forge::advsec::enabled(&s.state.db, rid, "secret_scanning").await);
    assert!(forge::advsec::enabled(&s.state.db, rid, "dependabot_alerts").await, "vulnerability alerts follow the configuration");
    assert_eq!(s.call(Some(&owner), "GET", &format!("{rp}/vulnerability-alerts"), None).await.0, 204);
    let (_, rc) = s.call(Some(&owner), "GET", &format!("{rp}/code-security-configuration"), None).await;
    assert_eq!((rc["status"].as_str(), rc["configuration"]["id"].as_i64()), (Some("enforced"), Some(cid)));
    // Enforced: the repository cannot change its own settings.
    assert_eq!(s.call(Some(&owner), "PATCH", &rp, Some(json!({"security_and_analysis": {"secret_scanning": {"status": "disabled"}}}))).await.0, 422);
    let (_, repos) = s.call(Some(&owner), "GET", &format!("{base}/{cid}/repositories"), None).await;
    assert_eq!(repos[0]["repository"]["id"], rid);

    // Defaults for new repositories.
    let (st, d) = s.call(Some(&owner), "PUT", &format!("{base}/{cid}/defaults"), Some(json!({"default_for_new_repos": "private_and_internal"}))).await;
    assert_eq!(st, 200, "{d}");
    let (_, r2) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": unique("r"), "private": true}))).await;
    assert!(forge::advsec::enabled(&s.state.db, r2["id"].as_i64().unwrap(), "secret_scanning").await, "new private repos get the default");
    let (_, r3) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": unique("r"), "private": false}))).await;
    assert!(!forge::advsec::enabled(&s.state.db, r3["id"].as_i64().unwrap(), "secret_scanning").await);

    // Delegated secret scanning dismissal: a repository admin who is not a reviewer files a
    // request; the owner approves it, which resolves the alert.
    let content = base64_encode(&format!("k={}\n", aws_key()));
    assert_eq!(s.call(Some(&owner), "PUT", &format!("{rp}/contents/k.txt"), Some(json!({"message": "k", "content": content}))).await.0, 201);
    drain(&s, rid).await;
    assert_eq!(s.call(Some(&owner), "PUT", &format!("{rp}/collaborators/{}", member.login), Some(json!({"permission": "admin"}))).await.0 / 100, 2);
    let (_, alerts) = s.call(Some(&member), "GET", &format!("{rp}/secret-scanning/alerts"), None).await;
    let n = alerts[0]["number"].as_i64().expect("alert");
    let (st, a) = s.call(Some(&member), "PATCH", &format!("{rp}/secret-scanning/alerts/{n}"), Some(json!({"state": "resolved", "resolution": "false_positive", "resolution_comment": "fake"}))).await;
    assert_eq!((st, a["state"].as_str()), (200, Some("open")), "{a}");
    assert_eq!(a["closure_request_comment"], "fake");
    let (_, req) = s.call(Some(&member), "GET", &format!("{rp}/dismissal-requests/secret-scanning/{n}"), None).await;
    assert_eq!((req["status"].as_str(), req["requester"]["actor_name"].as_str()), (Some("pending"), Some(member.login.as_str())), "{req}");
    assert_eq!(s.call(Some(&member), "PATCH", &format!("{rp}/dismissal-requests/secret-scanning/{n}"), Some(json!({"status": "approve", "message": "self"}))).await.0, 403);
    let (st, rv) = s.call(Some(&owner), "PATCH", &format!("{rp}/dismissal-requests/secret-scanning/{n}"), Some(json!({"status": "approve", "message": "ok"}))).await;
    assert_eq!(st, 200, "{rv}");
    assert!(rv["dismissal_review_id"].is_i64());
    let (_, a) = s.call(Some(&owner), "GET", &format!("{rp}/secret-scanning/alerts/{n}"), None).await;
    assert_eq!((a["state"].as_str(), a["resolution"].as_str()), (Some("resolved"), Some("false_positive")));
    assert_eq!(a["resolved_by"]["login"], member.login);
    assert_eq!(a["closure_request_reviewer"]["login"], owner.login);
    let (_, l) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/dismissal-requests/secret-scanning?request_status=approved"), None).await;
    assert_eq!(l.as_array().unwrap().len(), 1);

    // Detach keeps the settings; deleting removes the configuration.
    assert_eq!(s.call(Some(&owner), "DELETE", &format!("{base}/detach"), Some(json!({"selected_repository_ids": [rid]}))).await.0, 204);
    assert_eq!(s.call(Some(&owner), "GET", &format!("{rp}/code-security-configuration"), None).await.0, 204);
    assert!(forge::advsec::enabled(&s.state.db, rid, "secret_scanning").await);
    assert_eq!(s.call(Some(&owner), "DELETE", &format!("{base}/{cid}"), None).await.0, 204);
    assert_eq!(s.call(Some(&owner), "GET", &format!("{base}/{cid}"), None).await.0, 404);
    // The global configuration is listed but not deletable.
    let (_, g) = s.call(Some(&owner), "GET", &format!("{base}?target_type=global"), None).await;
    assert_eq!(g[0]["target_type"], "global");
    assert_eq!(s.call(Some(&owner), "DELETE", &format!("{base}/{}", g[0]["id"]), None).await.0, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dependabot_secrets_are_sealed_and_decryptable() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let id = repo_id(&s, &u, &rp).await;
    let (_, pk) = s.call(Some(&u), "GET", &format!("{rp}/dependabot/secrets/public-key"), None).await;
    let (_, apk) = s.call(Some(&u), "GET", &format!("{rp}/actions/secrets/public-key"), None).await;
    assert_ne!(pk["key"], apk["key"], "Dependabot has its own key pair");
    let sealed = forge::api::actions_settings::crypto::seal_for(pk["key"].as_str().unwrap(), "npm-token-value").unwrap();
    let put = |v: &str| json!({"encrypted_value": v, "key_id": pk["key_id"]});
    assert_eq!(s.call(Some(&u), "PUT", &format!("{rp}/dependabot/secrets/NPM_TOKEN"), Some(put("Zm9v"))).await.0, 422, "must open with the key");
    assert_eq!(s.call(Some(&u), "PUT", &format!("{rp}/dependabot/secrets/NPM_TOKEN"), Some(put(&sealed))).await.0, 201);
    assert_eq!(s.call(Some(&u), "PUT", &format!("{rp}/dependabot/secrets/NPM_TOKEN"), Some(put(&sealed))).await.0, 204);
    let got = forge::advsec::dependabot_secrets::secrets_for_update(&s.state, id).await.map_err(|e| e.message).unwrap();
    assert_eq!(got, vec![("NPM_TOKEN".to_string(), "npm-token-value".to_string())]);
    let (_, l) = s.call(Some(&u), "GET", &format!("{rp}/dependabot/secrets"), None).await;
    assert_eq!(l["total_count"], 1);
    assert!(l["secrets"][0].get("visibility").is_none());
    let other = s.user().await;
    assert_eq!(s.call(Some(&other), "GET", &format!("{rp}/dependabot/secrets"), None).await.0, 403);
    assert_eq!(s.call(Some(&u), "DELETE", &format!("{rp}/dependabot/secrets/NPM_TOKEN"), None).await.0, 204);
    assert_eq!(s.call(Some(&u), "DELETE", &format!("{rp}/dependabot/secrets/NPM_TOKEN"), None).await.0, 404);
}
