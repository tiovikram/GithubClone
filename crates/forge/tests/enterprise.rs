//! Enterprise administration (area "enterprise"): site-admin gating, user suspension across
//! every authentication path, impersonation tokens, global webhooks, pre-receive hook
//! enforcement during pushes (including downloaded environments), maintenance mode, enterprise
//! teams granting organization membership, SCIM provisioning and audit log events.
mod common;

use std::sync::LazyLock;

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};
use tokio::sync::RwLock;

/// Maintenance mode is instance-wide: the maintenance test takes the write lock, every other
/// test in this file the read lock, so no test sees an unexpected 503.
static MAINT: LazyLock<RwLock<()>> = LazyLock::new(|| RwLock::new(()));

async fn admin(s: &TestServer) -> TestUser {
    let login = unique("adm");
    let u = forge::forge_api::create_user(&s.state, &login, Some(&format!("{login}@example.com")), "password123", true).await.map_err(|e| e.message).unwrap();
    let (_, token) = forge::forge_api::create_token_for(&s.state, u.id, "test").await.map_err(|e| e.message).unwrap();
    TestUser { login, token }
}

fn basic(login: &str, secret: &str) -> String {
    use base64::Engine;
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{login}:{secret}")))
}

async fn raw(s: &TestServer, method: &str, path: &str, auth: Option<String>, body: Option<Value>) -> (u16, Value) {
    let mut req = s.client().request(method.parse().unwrap(), format!("{}{path}", s.api));
    if let Some(a) = auth {
        req = req.header("Authorization", a);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let st = res.status().as_u16();
    let text = res.text().await.unwrap();
    (st, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_endpoints_require_site_admin() {
    let _g = MAINT.read().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let u = s.user().await;
    for (m, p) in [
        ("GET", "/enterprise/stats/all"),
        ("GET", "/admin/hooks"),
        ("GET", "/admin/keys"),
        ("GET", "/enterprises/forge/teams"),
        ("GET", "/manage/v1/cluster/status"),
        ("GET", "/scim/v2/enterprises/forge/Users"),
        ("GET", "/admin/pre-receive-hooks"),
        ("POST", "/admin/users"),
    ] {
        let (st, _) = s.call(Some(&u), m, p, Some(json!({}))).await;
        assert_eq!(st, 403, "{m} {p} as non-admin");
    }
    let (st, _) = s.call(None, "GET", "/enterprise/stats/all", None).await;
    assert_eq!(st, 401);
    let (st, _) = s.call(Some(&a), "GET", "/enterprises/not-this-one/teams", None).await;
    assert_eq!(st, 404);
    let (st, v) = s.call(Some(&a), "GET", "/enterprise/stats/users", None).await;
    assert_eq!(st, 200, "{v}");
    assert!(v["total_users"].as_i64().unwrap() >= 2 && v["admin_users"].as_i64().unwrap() >= 1, "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspended_users_cannot_authenticate_anywhere() {
    let _g = MAINT.read().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let (st, _) = s.call(Some(&u), "PATCH", &format!("/repos/{}/{repo}", u.login), Some(json!({"private": true}))).await;
    assert_eq!(st, 200);
    // a web session
    let res = s
        .client()
        .post(format!("{}/api/forge/session", s.base))
        .header("X-Forge-CSRF", "1")
        .json(&json!({"login": u.login, "password": "password123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let work = s.work_dir("susp");
    let url = s.clone_url(&u, &repo);
    let (ok, out) = s.git(&work, &["clone", &url, "before"]);
    assert!(ok, "{out}");

    let (st, e) = s.call(Some(&a), "PUT", &format!("/users/{}/suspended", u.login), Some(json!({"reason": "test"}))).await;
    assert_eq!(st, 204, "{e}");
    let (st, _) = s.call(Some(&u), "GET", "/user", None).await;
    assert_eq!(st, 401, "token");
    let (st, _) = raw(&s, "GET", "/user", Some(basic(&u.login, "password123")), None).await;
    assert_eq!(st, 401, "basic password");
    let res = s.client().get(format!("{}/user", s.api)).header("Cookie", &cookie).send().await.unwrap();
    assert_eq!(res.status(), 401, "session (revoked)");
    let res = s
        .client()
        .post(format!("{}/api/forge/session", s.base))
        .header("X-Forge-CSRF", "1")
        .json(&json!({"login": u.login, "password": "password123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "login");
    let (ok, _) = s.git(&work, &["clone", &url, "during"]);
    assert!(!ok, "git clone as a suspended user");
    let (_, v) = s.call(Some(&a), "GET", "/enterprise/stats/users", None).await;
    assert!(v["suspended_users"].as_i64().unwrap() >= 1);
    // site admins cannot be suspended
    let (st, _) = s.call(Some(&a), "PUT", &format!("/users/{}/suspended", a.login), None).await;
    assert_eq!(st, 403);

    let (st, _) = s.call(Some(&a), "DELETE", &format!("/users/{}/suspended", u.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", "/user", None).await;
    assert_eq!(st, 200);
    let (ok, out) = s.git(&work, &["clone", &url, "after"]);
    assert!(ok, "{out}");
    // audit log records the suspension
    let (st, log) = s.call(Some(&a), "GET", &format!("/enterprises/forge/audit-log?phrase=action:user.suspend+user:{}", u.login), None).await;
    assert_eq!(st, 200, "{log}");
    assert!(log.as_array().unwrap().iter().any(|e| e["action"] == "user.suspend" && e["actor"] == a.login.as_str()), "{log}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_users_impersonation_rename_delete() {
    let _g = MAINT.read().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let login = unique("eu");
    let (st, u) = s.call(Some(&a), "POST", "/admin/users", Some(json!({"login": login, "email": format!("{login}@example.com")}))).await;
    assert_eq!(st, 201, "{u}");
    let (st, _) = s.call(Some(&a), "POST", "/admin/users", Some(json!({"login": login}))).await;
    assert_eq!(st, 422);
    let (st, t) = s.call(Some(&a), "POST", &format!("/admin/users/{login}/authorizations"), Some(json!({"scopes": ["repo"]}))).await;
    assert_eq!(st, 201, "{t}");
    let imp = TestUser { login: login.clone(), token: t["token"].as_str().unwrap().to_string() };
    let (st, me) = s.call(Some(&imp), "GET", "/user", None).await;
    assert_eq!(st, 200);
    assert_eq!(me["login"], login.as_str());
    // the impersonated user can act (create a repository)
    let repo = s.create_repo(&imp, false).await;
    let new_login = format!("{login}-x");
    let (st, r) = s.call(Some(&a), "PATCH", &format!("/admin/users/{login}"), Some(json!({"login": new_login}))).await;
    assert_eq!(st, 202, "{r}");
    let (st, r) = s.call(None, "GET", &format!("/repos/{new_login}/{repo}"), None).await;
    assert_eq!(st, 200, "repository follows the rename: {r}");
    // promote / demote
    let (st, _) = s.call(Some(&a), "PUT", &format!("/users/{new_login}/site_admin"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&imp), "GET", "/enterprise/stats/repos", None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/users/{new_login}"), None).await;
    assert_eq!(st, 403, "site admins must be demoted first");
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/users/{new_login}/site_admin"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&imp), "GET", "/enterprise/stats/repos", None).await;
    assert_eq!(st, 403);
    // revoke impersonation tokens
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/users/{new_login}/authorizations"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&imp), "GET", "/user", None).await;
    assert_eq!(st, 401);
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/users/{new_login}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(None, "GET", &format!("/users/{new_login}"), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_webhooks_receive_user_and_org_events() {
    let _g = MAINT.read().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let url = format!("https://hooks.example.com/{}", unique("g"));
    let (st, h) = s.call(Some(&a), "POST", "/admin/hooks", Some(json!({"name": "web", "events": ["user", "organization"], "config": {"url": url, "content_type": "json"}}))).await;
    assert_eq!(st, 201, "{h}");
    let hid = h["id"].as_i64().unwrap();
    let login = unique("gw");
    let (st, _) = s.call(Some(&a), "POST", "/admin/users", Some(json!({"login": login}))).await;
    assert_eq!(st, 201);
    let org = unique("gorg");
    let (st, _) = s.call(Some(&a), "POST", "/admin/organizations", Some(json!({"login": org, "admin": a.login}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&a), "PATCH", &format!("/admin/organizations/{org}"), Some(json!({"login": format!("{org}-r")}))).await;
    assert_eq!(st, 202);
    let (st, _) = s.call(Some(&a), "PUT", &format!("/users/{login}/suspended"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/users/{login}"), None).await;
    assert_eq!(st, 204);
    let rows: Vec<(String, Option<String>, Value)> =
        sqlx::query_as("SELECT event, action, payload FROM webhook_deliveries WHERE hook_id = $1 ORDER BY id").bind(hid).fetch_all(&s.state.db).await.unwrap();
    let got: Vec<(String, String)> = rows.iter().map(|(e, a, _)| (e.clone(), a.clone().unwrap_or_default())).collect();
    for want in [("user", "created"), ("organization", "created"), ("organization", "renamed"), ("user", "suspended"), ("user", "deleted")] {
        assert!(got.iter().any(|(e, a)| e == want.0 && a == want.1), "missing {want:?} in {got:?}");
    }
    let created = rows.iter().find(|r| r.0 == "user" && r.1.as_deref() == Some("created") && r.2["user"]["login"] == login.as_str()).unwrap();
    assert_eq!(created.2["user"]["login"], login.as_str());
    assert_eq!(created.2["enterprise"]["slug"], "forge");
    assert_eq!(created.2["sender"]["login"], a.login.as_str());
    // a repository event never reaches a global hook
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE hook_id = $1 AND event NOT IN ('user', 'organization')").bind(hid).fetch_one(&s.state.db).await.unwrap();
    assert_eq!(n, 0);
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/hooks/{hid}"), None).await;
    assert_eq!(st, 204);
}

/// Serves `body` once per request at http://127.0.0.1:{port}/env.tar.gz.
async fn serve_bytes(body: Vec<u8>) -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let app = axum::Router::new().route("/env.tar.gz", axum::routing::get(move || {
        let b = body.clone();
        async move { b }
    }));
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    port
}

async fn wait_download(s: &TestServer, a: &TestUser, env_id: i64) -> Value {
    for _ in 0..100 {
        forge::jobs::run_once(&s.state).await.unwrap();
        let (_, d) = s.call(Some(a), "GET", &format!("/admin/pre-receive-environments/{env_id}/downloads/latest"), None).await;
        if d["state"] != "in_progress" {
            return d;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("download did not finish");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pre_receive_hooks_reject_pushes() {
    let _g = MAINT.read().await;
    forge::enterprise::prereceive::set_exec_bin(env!("CARGO_BIN_EXE_forge"));
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let u = s.user().await;
    // Script repository: rejects pushes that add a file named "forbidden.txt" and prints the
    // GitHub environment it got; a second script uses a tool from a custom environment.
    let scripts = s.create_repo(&a, true).await;
    let script = "#!/bin/sh\nwhile read old new ref; do\n  if git diff --name-only \"$old\" \"$new\" 2>/dev/null | grep -q forbidden.txt; then\n    echo \"rejected for $GITHUB_USER_LOGIN on $GITHUB_REPO_NAME via $GITHUB_VIA\"\n    exit 1\n  fi\ndone\necho \"checked by forge pre-receive\"\nexit 0\n";
    let (st, r) = s
        .call(Some(&a), "PUT", &format!("/repos/{}/{}/contents/check.sh", a.login, scripts), Some(json!({"message": "hook", "content": base64(script)})))
        .await;
    assert_eq!(st, 201, "{r}");
    let (st, r) = s
        .call(Some(&a), "PUT", &format!("/repos/{}/{}/contents/env-check.sh", a.login, scripts), Some(json!({"message": "hook", "content": base64("#!/bin/sh\nexec forge-env-tool\n")})))
        .await;
    assert_eq!(st, 201, "{r}");
    // Globally disabled but configurable downstream, enabled for one repository only (other tests
    // share this database and push too).
    let name = unique("hook");
    let (st, h) = s
        .call(Some(&a), "POST", "/admin/pre-receive-hooks", Some(json!({
            "name": name, "script": "check.sh", "script_repository": {"full_name": format!("{}/{}", a.login, scripts)},
            "environment": {"id": 1}, "enforcement": "disabled", "allow_downstream_configuration": true,
        })))
        .await;
    assert_eq!(st, 201, "{h}");
    let hid = h["id"].as_i64().unwrap();
    assert_eq!(h["environment"]["default_environment"], true);
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let (st, r) = s.call(Some(&u), "PATCH", &format!("{rp}/pre-receive-hooks/{hid}"), Some(json!({"enforcement": "enabled"}))).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["enforcement"], "enabled");
    assert!(r["configuration_url"].as_str().unwrap().ends_with(&format!("{rp}/pre-receive-hooks/{hid}")), "{r}");

    let work = s.work_dir("prh");
    let (ok, out) = s.git(&work, &["clone", &s.clone_url(&u, &repo), "w"]);
    assert!(ok, "{out}");
    let w = work.join("w");
    std::fs::write(w.join("ok.txt"), "fine\n").unwrap();
    s.git(&w, &["add", "."]);
    s.git(&w, &["commit", "-m", "ok"]);
    let (ok, out) = s.git(&w, &["push", "origin", "HEAD:main"]);
    assert!(ok, "{out}");
    assert!(out.contains("checked by forge pre-receive"), "hook output is shown: {out}");
    std::fs::write(w.join("forbidden.txt"), "no\n").unwrap();
    s.git(&w, &["add", "."]);
    s.git(&w, &["commit", "-m", "bad"]);
    let (ok, out) = s.git(&w, &["push", "origin", "HEAD:main"]);
    assert!(!ok, "push must be rejected: {out}");
    assert!(out.contains(&format!("rejected for {} on {}/{} via git push", u.login, u.login, repo)), "{out}");
    assert!(out.contains("pre-receive hook declined") || out.contains("declined"), "{out}");
    // nothing was committed
    let (_, b) = s.call(Some(&u), "GET", &format!("{rp}/contents/forbidden.txt"), None).await;
    assert_eq!(b["message"], "Not Found");

    // testing: runs and reports, but does not reject
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{rp}/pre-receive-hooks/{hid}"), Some(json!({"enforcement": "testing"}))).await;
    assert_eq!(st, 200);
    let (ok, out) = s.git(&w, &["push", "origin", "HEAD:main"]);
    assert!(ok, "{out}");
    assert!(out.contains("would have declined"), "{out}");

    // A custom environment: a tarball with bin/forge-env-tool, downloaded into object storage.
    forge::security::set_webhook_allow_private(true);
    let envdir = s.work_dir("envsrc");
    std::fs::create_dir_all(envdir.join("bin")).unwrap();
    std::fs::write(envdir.join("bin/forge-env-tool"), "#!/bin/sh\necho from-custom-environment\nexit 3\n").unwrap();
    std::process::Command::new("chmod").args(["755", envdir.join("bin/forge-env-tool").to_str().unwrap()]).status().unwrap();
    let tarball = s.dir.path().join("env.tar.gz");
    let st = std::process::Command::new("tar").arg("-czf").arg(&tarball).arg("-C").arg(&envdir).arg("bin").status().unwrap();
    assert!(st.success());
    let port = serve_bytes(std::fs::read(&tarball).unwrap()).await;
    let (st, env) = s
        .call(Some(&a), "POST", "/admin/pre-receive-environments", Some(json!({"name": unique("env"), "image_url": format!("http://127.0.0.1:{port}/env.tar.gz")})))
        .await;
    assert_eq!(st, 201, "{env}");
    let env_id = env["id"].as_i64().unwrap();
    let d = wait_download(&s, &a, env_id).await;
    assert_eq!(d["state"], "success", "{d}");
    let (st, r) = s
        .call(Some(&a), "PATCH", &format!("/admin/pre-receive-hooks/{hid}"), Some(json!({"script": "env-check.sh", "environment": {"id": env_id}})))
        .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["environment"]["id"], env_id);
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{rp}/pre-receive-hooks/{hid}"), Some(json!({"enforcement": "enabled"}))).await;
    assert_eq!(st, 200);
    std::fs::write(w.join("again.txt"), "x\n").unwrap();
    s.git(&w, &["add", "."]);
    s.git(&w, &["commit", "-m", "again"]);
    let (ok, out) = s.git(&w, &["push", "origin", "HEAD:main"]);
    assert!(!ok, "{out}");
    assert!(out.contains("from-custom-environment"), "{out}");
    // an environment in use cannot be deleted
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/pre-receive-environments/{env_id}"), None).await;
    assert_eq!(st, 422);

    // repository override removed -> global "disabled" applies again
    let (st, r) = s.call(Some(&u), "DELETE", &format!("{rp}/pre-receive-hooks/{hid}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r["enforcement"], "disabled");
    let (ok, out) = s.git(&w, &["push", "origin", "HEAD:main"]);
    assert!(ok, "{out}");
    // a hook that does not allow downstream configuration cannot be overridden
    let (st, _) = s.call(Some(&a), "PATCH", &format!("/admin/pre-receive-hooks/{hid}"), Some(json!({"allow_downstream_configuration": false}))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{rp}/pre-receive-hooks/{hid}"), Some(json!({"enforcement": "enabled"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "GET", "/admin/pre-receive-hooks", None).await;
    assert_eq!(st, 403, "only site admins configure hooks");
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/pre-receive-hooks/{hid}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/admin/pre-receive-environments/{env_id}"), None).await;
    assert_eq!(st, 204);
}

fn base64(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maintenance_mode_blocks_non_admins() {
    let _g = MAINT.write().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let u = s.user().await;
    let (st, r) = s.call(Some(&a), "POST", "/manage/v1/maintenance", Some(json!({"enabled": true, "maintenance_mode_message": "back soon"}))).await;
    assert_eq!(st, 200, "{r}");
    let (st, r) = s.call(Some(&u), "GET", "/user", None).await;
    assert_eq!(st, 503, "{r}");
    assert_eq!(r["message"], "back soon");
    let (st, _) = s.call(None, "GET", "/meta", None).await;
    assert_eq!(st, 503);
    let res = s.client().get(format!("{}/{}", s.base, u.login)).send().await.unwrap();
    assert_eq!(res.status(), 503, "web UI too");
    let (st, _) = s.call(Some(&a), "GET", "/user", None).await;
    assert_eq!(st, 200, "site admins keep access");
    let (_, m) = s.call(Some(&a), "GET", "/manage/v1/maintenance", None).await;
    assert_eq!(m[0]["status"], "on", "{m}");
    let res = s.client().get(format!("{}/healthz", s.base)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    // scheduled in the future: not active yet
    let (st, _) = s.call(Some(&a), "POST", "/manage/v1/maintenance", Some(json!({"enabled": true, "when": "2099-01-01T00:00:00Z"}))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "GET", "/user", None).await;
    assert_eq!(st, 200);
    let (_, m) = s.call(Some(&a), "GET", "/manage/v1/maintenance", None).await;
    assert_eq!(m[0]["status"], "scheduled");
    let (st, _) = s.call(Some(&a), "POST", "/manage/v1/maintenance", Some(json!({"enabled": false}))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "GET", "/user", None).await;
    assert_eq!(st, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enterprise_team_assignment_grants_org_membership() {
    let _g = MAINT.read().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let u = s.user().await;
    let existing = s.user().await;
    let org = unique("eorg");
    let (st, _) = s.call(Some(&a), "POST", "/admin/organizations", Some(json!({"login": org, "admin": a.login}))).await;
    assert_eq!(st, 201);
    // `existing` was already a member: removing the team must not remove them
    let (st, r) = s.call(Some(&a), "PUT", &format!("/orgs/{org}/memberships/{}", existing.login), Some(json!({"role": "member"}))).await;
    assert!(st == 200 || st == 201, "{st} {r}");
    let accept = s.call(Some(&existing), "PATCH", &format!("/user/memberships/orgs/{org}"), Some(json!({"state": "active"}))).await;
    let _ = accept;
    let name = unique("Platform ");
    let (st, t) = s.call(Some(&a), "POST", "/enterprises/forge/teams", Some(json!({"name": name, "organization_selection_type": "selected"}))).await;
    assert_eq!(st, 201, "{t}");
    let slug = t["slug"].as_str().unwrap().to_string();
    let tp = format!("/enterprises/forge/teams/{slug}");
    let (st, _) = s.call(Some(&a), "POST", &format!("{tp}/memberships/add"), Some(json!({"usernames": [u.login, existing.login]}))).await;
    assert_eq!(st, 200);
    let is_member = |login: String| {
        let s = &s;
        let a = &a;
        let org = org.clone();
        async move { s.call(Some(a), "GET", &format!("/orgs/{org}/members/{login}"), None).await.0 == 204 }
    };
    assert!(!is_member(u.login.clone()).await);
    let (st, r) = s.call(Some(&a), "PUT", &format!("{tp}/organizations/{org}"), None).await;
    assert_eq!(st, 201, "{r}");
    assert!(is_member(u.login.clone()).await, "assigned team members join the organization");
    let (st, l) = s.call(Some(&a), "GET", &format!("{tp}/organizations"), None).await;
    assert_eq!(st, 200);
    assert_eq!(l.as_array().unwrap().len(), 1);
    // a member removed from the team leaves the organization
    let (st, _) = s.call(Some(&a), "DELETE", &format!("{tp}/memberships/{}", u.login), None).await;
    assert_eq!(st, 204);
    assert!(!is_member(u.login.clone()).await);
    let (st, _) = s.call(Some(&a), "PUT", &format!("{tp}/memberships/{}", u.login), None).await;
    assert_eq!(st, 201);
    assert!(is_member(u.login.clone()).await);
    // unassigning the organization removes granted memberships only
    let (st, _) = s.call(Some(&a), "DELETE", &format!("{tp}/organizations/{org}"), None).await;
    assert_eq!(st, 204);
    assert!(!is_member(u.login.clone()).await);
    let existing_still: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members m JOIN users u ON u.id = m.user_id JOIN users o ON o.id = m.org_id WHERE o.login = $1 AND u.login = $2)")
        .bind(&org)
        .bind(&existing.login)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    let invited_only: bool = sqlx::query_scalar("SELECT to_regclass('public.org_invitations') IS NOT NULL").fetch_one(&s.state.db).await.unwrap();
    assert!(existing_still || invited_only, "pre-existing membership kept");
    let (st, _) = s.call(Some(&a), "DELETE", &tp, None).await;
    assert_eq!(st, 204);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scim_provisioning_creates_and_suspends_users() {
    let _g = MAINT.read().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let uname = unique("scim");
    let body = json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"], "externalId": unique("ext"), "active": true, "userName": format!("{uname}@example.com"),
        "name": {"givenName": "Ada", "familyName": "Lovelace"}, "displayName": "Ada Lovelace",
        "emails": [{"value": format!("{uname}@example.com"), "type": "work", "primary": true}],
    });
    let (st, su) = s.call(Some(&a), "POST", "/scim/v2/enterprises/forge/Users", Some(body.clone())).await;
    assert_eq!(st, 201, "{su}");
    let sid = su["id"].as_str().unwrap().to_string();
    let (st, u) = s.call(None, "GET", &format!("/users/{uname}"), None).await;
    assert_eq!(st, 200, "a forge user was created: {u}");
    assert_eq!(u["name"], "Ada Lovelace");
    let (st, _) = s.call(Some(&a), "POST", "/scim/v2/enterprises/forge/Users", Some(body)).await;
    assert_eq!(st, 409);
    // deactivate -> suspended
    let (st, r) = s
        .call(Some(&a), "PATCH", &format!("/scim/v2/enterprises/forge/Users/{sid}"), Some(json!({"schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"], "Operations": [{"op": "replace", "path": "active", "value": "false"}]})))
        .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["active"], false);
    let susp: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT suspended_at FROM users WHERE login = $1").bind(&uname).fetch_one(&s.state.db).await.unwrap();
    assert!(susp.is_some());
    let (st, r) = s
        .call(Some(&a), "PATCH", &format!("/scim/v2/enterprises/forge/Users/{sid}"), Some(json!({"schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"], "Operations": [{"op": "replace", "value": {"active": true}}]})))
        .await;
    assert_eq!(st, 200, "{r}");
    let susp: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT suspended_at FROM users WHERE login = $1").bind(&uname).fetch_one(&s.state.db).await.unwrap();
    assert!(susp.is_none());
    // a group mirrors into external_groups for organization teams
    let gname = unique("grp");
    let (st, g) = s
        .call(Some(&a), "POST", "/scim/v2/enterprises/forge/Groups", Some(json!({"schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"], "externalId": unique("gx"), "displayName": gname, "members": [{"value": sid, "displayName": "Ada"}]})))
        .await;
    assert_eq!(st, 201, "{g}");
    assert_eq!(g["members"][0]["value"], sid.as_str());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM external_groups e JOIN external_group_members m ON m.group_id = e.id JOIN users u ON u.id = m.user_id WHERE e.name = $1 AND e.org_id IS NULL AND u.login = $2")
        .bind(&gname)
        .bind(&uname)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // an enterprise team connected to the group mirrors its members
    let (st, t) = s.call(Some(&a), "POST", "/enterprises/forge/teams", Some(json!({"name": unique("idp"), "group_id": g["id"]}))).await;
    assert_eq!(st, 201, "{t}");
    let slug = t["slug"].as_str().unwrap();
    let (_, m) = s.call(Some(&a), "GET", &format!("/enterprises/forge/teams/{slug}/memberships"), None).await;
    assert_eq!(m[0]["login"], uname.as_str(), "{m}");
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/enterprises/forge/teams/{slug}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/scim/v2/enterprises/forge/Groups/{}", g["id"].as_str().unwrap()), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/scim/v2/enterprises/forge/Users/{sid}"), None).await;
    assert_eq!(st, 204);
    let susp: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT suspended_at FROM users WHERE login = $1").bind(&uname).fetch_one(&s.state.db).await.unwrap();
    assert!(susp.is_some(), "deprovisioned users are suspended");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settings_signup_and_announcement() {
    let _g = MAINT.write().await; // changes instance-wide settings
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let (st, _) = s.call(Some(&a), "PUT", "/manage/v1/config/settings", Some(json!({"signup_enabled": false}))).await;
    assert_eq!(st, 204);
    let res = s
        .client()
        .post(format!("{}/api/forge/signup", s.base))
        .header("X-Forge-CSRF", "1")
        .json(&json!({"login": unique("su"), "password": "password123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let (st, _) = s.call(Some(&a), "PUT", "/manage/v1/config/settings", Some(json!({"signup_enabled": true}))).await;
    assert_eq!(st, 204);
    let (_, v) = s.call(Some(&a), "GET", "/manage/v1/config/settings", None).await;
    assert_eq!(v["signup_enabled"], true);
    let (st, v) = s.call(Some(&a), "PATCH", "/enterprise/announcement", Some(json!({"announcement": "Hello", "expires_at": "2000-01-01T00:00:00Z"}))).await;
    assert_eq!(st, 200, "{v}");
    let (_, v) = s.call(Some(&a), "GET", "/enterprise/announcement", None).await;
    assert!(v["announcement"].is_null(), "expired banners are hidden: {v}");
    // The web UI banner endpoint (public) renders the markdown.
    let (st, _) = s.call(Some(&a), "PATCH", "/enterprise/announcement", Some(json!({"announcement": "Maintenance **tonight**", "user_dismissible": true}))).await;
    assert_eq!(st, 200);
    let banner: Value = s.client().get(format!("{}/api/forge/announcement", s.base)).send().await.unwrap().json().await.unwrap();
    assert_eq!(banner["announcement"], "Maintenance **tonight**");
    assert!(banner["html"].as_str().unwrap().contains("<strong>tonight</strong>"), "{banner}");
    assert_eq!(banner["user_dismissible"], true);
    let (st, _) = s.call(Some(&a), "DELETE", "/enterprise/announcement", None).await;
    assert_eq!(st, 204);
    let banner: Value = s.client().get(format!("{}/api/forge/announcement", s.base)).send().await.unwrap().json().await.unwrap();
    assert!(banner["announcement"].is_null() && banner["html"].is_null(), "{banner}");
}

/// Repository invitations are a runtime site setting (`repository_invitations.enabled`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repository_invitations_site_setting() {
    let _g = MAINT.write().await; // changes instance-wide settings
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let owner = s.user().await;
    let invitee = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let full = format!("{}/{repo}", owner.login);
    let (st, _) = s.call(Some(&a), "PUT", "/manage/v1/config/settings", Some(json!({"repository_invitations": {"enabled": "yes"}}))).await;
    assert_eq!(st, 400);
    let (_, v) = s.call(Some(&a), "GET", "/manage/v1/config/settings", None).await;
    assert_eq!(v["repository_invitations"]["enabled"], false, "default (no FORGE_REPO_INVITATIONS): {v}");
    // Default: GHES adds the collaborator directly.
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/repos/{full}/collaborators/{}", invitee.login), Some(json!({"permission": "push"}))).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/repos/{full}/collaborators/{}", invitee.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&a), "PUT", "/manage/v1/config/settings", Some(json!({"repository_invitations": {"enabled": true}}))).await;
    assert_eq!(st, 204);
    let (st, inv) = s.call(Some(&owner), "PUT", &format!("/repos/{full}/collaborators/{}", invitee.login), Some(json!({"permission": "push"}))).await;
    // Restore before asserting so a failure can't leave the instance in invitation mode.
    let (st2, _) = s.call(Some(&a), "PUT", "/manage/v1/config/settings", Some(json!({"repository_invitations": {"enabled": false}}))).await;
    assert_eq!(st2, 204);
    assert_eq!(st, 201, "{inv}");
    assert_eq!(inv["invitee"]["login"], invitee.login.as_str());
    assert_eq!(inv["permissions"], "write");
    let (st, _) = s.call(Some(&invitee), "PATCH", &format!("/user/repository_invitations/{}", inv["id"]), None).await;
    assert_eq!(st, 204);
    let (_, perm) = s.call(Some(&owner), "GET", &format!("/repos/{full}/collaborators/{}/permission", invitee.login), None).await;
    assert_eq!(perm["permission"], "write");
    let (_, v) = s.call(Some(&a), "GET", "/manage/v1/config/settings", None).await;
    assert_eq!(v["repository_invitations"]["enabled"], false);
}

/// Request against the forge-only API (`/api/forge`), with the CSRF header a browser sends.
async fn forge_call(s: &TestServer, u: Option<&TestUser>, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = s.client().request(method.parse().unwrap(), format!("{}/api/forge{path}", s.base)).header("X-Forge-CSRF", "1");
    if let Some(u) = u {
        req = req.header("Authorization", format!("token {}", u.token));
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let st = res.status().as_u16();
    let text = res.text().await.unwrap();
    (st, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web_admin_listings_details_and_password_reset() {
    let _g = MAINT.read().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    for p in ["/admin/users".to_string(), format!("/admin/users/{}", u.login), "/admin/orgs".into(), "/admin/repos".into(), "/admin/info".into()] {
        let (st, _) = forge_call(&s, Some(&u), "GET", &p, None).await;
        assert_eq!(st, 403, "GET {p} as non-admin");
    }
    // search + filters
    let (st, v) = forge_call(&s, Some(&a), "GET", &format!("/admin/users?q={}", u.login), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["total_count"], 1, "{v}");
    assert_eq!(v["items"][0]["login"], u.login.as_str());
    assert_eq!(v["items"][0]["suspended_at"], Value::Null);
    let (st, _) = s.call(Some(&a), "PUT", &format!("/users/{}/suspended", u.login), Some(json!({"reason": "spam"}))).await;
    assert_eq!(st, 204);
    let (_, v) = forge_call(&s, Some(&a), "GET", &format!("/admin/users?filter=suspended&q={}", u.login), None).await;
    assert_eq!(v["total_count"], 1, "{v}");
    assert_eq!(v["items"][0]["suspension_reason"], "spam");
    let (_, v) = forge_call(&s, Some(&a), "GET", &format!("/admin/users?filter=active&q={}", u.login), None).await;
    assert_eq!(v["total_count"], 0, "{v}");
    let (st, _) = s.call(Some(&a), "DELETE", &format!("/users/{}/suspended", u.login), Some(json!({}))).await;
    assert_eq!(st, 204);
    let (st, _) = forge_call(&s, Some(&a), "GET", "/admin/users?filter=bogus", None).await;
    assert_eq!(st, 422);
    // details
    let (st, d) = forge_call(&s, Some(&a), "GET", &format!("/admin/users/{}", u.login), None).await;
    assert_eq!(st, 200, "{d}");
    assert!(d["repositories"].as_array().unwrap().iter().any(|r| r["name"] == repo.as_str()), "{d}");
    assert_eq!(d["tokens"].as_array().unwrap().len(), 1, "{d}");
    assert!(d["emails"].as_array().unwrap().iter().any(|e| e == &json!(format!("{}@example.com", u.login))), "{d}");
    // repos and orgs
    let (_, v) = forge_call(&s, Some(&a), "GET", &format!("/admin/repos?q={}/{repo}", u.login), None).await;
    assert_eq!(v["total_count"], 1, "{v}");
    assert_eq!(v["items"][0]["full_name"], format!("{}/{repo}", u.login));
    let org = unique("o");
    let (st, _) = s.call(Some(&a), "POST", "/admin/organizations", Some(json!({"login": org, "admin": u.login}))).await;
    assert_eq!(st, 201);
    let (_, v) = forge_call(&s, Some(&a), "GET", &format!("/admin/orgs?q={org}"), None).await;
    assert_eq!(v["items"][0]["owners"], 1, "{v}");
    // a password set by an admin lets an admin-created (passwordless) user sign in
    let newbie = unique("n");
    let (st, _) = s.call(Some(&a), "POST", "/admin/users", Some(json!({"login": newbie, "email": format!("{newbie}@example.com")}))).await;
    assert_eq!(st, 201);
    let (_, d) = forge_call(&s, Some(&a), "GET", &format!("/admin/users/{newbie}"), None).await;
    assert_eq!(d["has_password"], false, "{d}");
    let (st, _) = forge_call(&s, Some(&a), "PUT", &format!("/admin/users/{newbie}/password"), Some(json!({"password": "short"}))).await;
    assert_eq!(st, 422);
    let (st, _) = forge_call(&s, Some(&u), "PUT", &format!("/admin/users/{newbie}/password"), Some(json!({"password": "brand-new-pass"}))).await;
    assert_eq!(st, 403);
    let (st, _) = forge_call(&s, Some(&a), "PUT", &format!("/admin/users/{newbie}/password"), Some(json!({"password": "brand-new-pass"}))).await;
    assert_eq!(st, 204);
    let (st, v) = forge_call(&s, None, "POST", "/session", Some(json!({"login": newbie, "password": "brand-new-pass"}))).await;
    assert_eq!(st, 200, "{v}");
    let (_, log) = s.call(Some(&a), "GET", &format!("/enterprises/forge/audit-log?phrase=action:user.reset_password+user:{newbie}"), None).await;
    assert_eq!(log.as_array().map(Vec::len), Some(1), "{log}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maintenance_status_endpoint_and_spa_shell() {
    let _g = MAINT.write().await;
    let s = TestServer::start().await;
    let a = admin(&s).await;
    let u = s.user().await;
    let (st, v) = forge_call(&s, Some(&u), "GET", "/maintenance", None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["status"], "off");
    let (st, _) = s.call(Some(&a), "POST", "/manage/v1/maintenance", Some(json!({"enabled": true, "maintenance_mode_message": "upgrading"}))).await;
    assert_eq!(st, 200);
    // the status endpoint stays reachable and tells the SPA what to show
    let (st, v) = forge_call(&s, Some(&u), "GET", "/maintenance", None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["status"], "on");
    assert_eq!(v["active"], true);
    assert_eq!(v["message"], "upgrading");
    assert_eq!(v["bypass"], false);
    let (_, v) = forge_call(&s, Some(&a), "GET", "/maintenance", None).await;
    assert_eq!(v["bypass"], true);
    // browser navigations get the web UI shell (with 503); API and git clients the JSON 503
    let res = s.client().get(format!("{}/{}", s.base, u.login)).header("Accept", "text/html,application/xhtml+xml").send().await.unwrap();
    assert_eq!(res.status(), 503);
    let ct = res.headers().get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    assert!(!ct.starts_with("application/json"), "{ct}");
    let res = s.client().get(format!("{}/api/v3/user", s.base)).header("Accept", "text/html").header("Authorization", format!("token {}", u.token)).send().await.unwrap();
    assert_eq!(res.status(), 503);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["message"], "upgrading");
    let res = s.client().get(format!("{}/{}/x.git/info/refs?service=git-upload-pack", s.base, u.login)).header("Accept", "text/html").send().await.unwrap();
    assert_eq!(res.status(), 503);
    let ct = res.headers().get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    assert!(ct.starts_with("application/json"), "{ct}");
    let (st, _) = s.call(Some(&a), "POST", "/manage/v1/maintenance", Some(json!({"enabled": false}))).await;
    assert_eq!(st, 200);
    let (_, v) = forge_call(&s, Some(&u), "GET", "/maintenance", None).await;
    assert_eq!(v["status"], "off");
}
