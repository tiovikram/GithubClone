//! Apps area: GitHub Apps (registration, JWT, installations, installation tokens and their
//! confinement), user-to-server tokens, OAuth token endpoints, the legacy OAuth Authorizations
//! API and migrations (exports, repository locks).
mod common;

use base64::Engine;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn req(s: &TestServer, auth: Option<&str>, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let url = if path.starts_with("http") { path.to_string() } else { format!("{}{}", s.api, path) };
    let mut r = s.client().request(method.parse().unwrap(), url);
    if let Some(a) = auth {
        r = r.header("Authorization", a);
    }
    if let Some(b) = body {
        r = r.json(&b);
    }
    let res = r.send().await.unwrap();
    let st = res.status().as_u16();
    let text = res.text().await.unwrap();
    (st, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
}

fn tok(u: &TestUser) -> String {
    format!("token {}", u.token)
}

fn basic(a: &str, b: &str) -> String {
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{a}:{b}")))
}

struct App {
    id: i64,
    slug: String,
    client_id: String,
    client_secret: String,
    pem: String,
}

impl App {
    fn jwt(&self) -> String {
        let now = chrono::Utc::now().timestamp();
        let j = forge::apps::jwt::sign_jwt(&self.pem, &json!({"iss": self.id, "iat": now - 30, "exp": now + 540})).unwrap();
        format!("Bearer {j}")
    }
}

async fn create_app(s: &TestServer, owner: &TestUser, perms: Value, events: Value, public: bool) -> App {
    let name = unique("App ");
    let (st, v) = req(
        s,
        Some(&tok(owner)),
        "POST",
        &format!("{}/api/forge/apps", s.base),
        Some(json!({"name": name, "url": "https://example.com/app", "public": public,
            "hook_attributes": {"url": "https://example.com/hook"}, "default_permissions": perms, "default_events": events})),
    )
    .await;
    assert_eq!(st, 201, "{v}");
    assert!(v["pem"].as_str().unwrap().starts_with("-----BEGIN RSA PRIVATE KEY-----"));
    App {
        id: v["id"].as_i64().unwrap(),
        slug: v["slug"].as_str().unwrap().into(),
        client_id: v["client_id"].as_str().unwrap().into(),
        client_secret: v["client_secret"].as_str().unwrap().into(),
        pem: v["pem"].as_str().unwrap().into(),
    }
}

async fn install(s: &TestServer, u: &TestUser, app: &App, body: Value) -> i64 {
    let (st, v) = req(s, Some(&tok(u)), "POST", &format!("{}/api/forge/apps/{}/installations", s.base, app.slug), Some(body)).await;
    assert_eq!(st, 201, "{v}");
    v["id"].as_i64().unwrap()
}

async fn private_repo(s: &TestServer, u: &TestUser) -> (String, i64) {
    let name = unique("r");
    let (st, v) = req(s, Some(&tok(u)), "POST", "/user/repos", Some(json!({"name": name, "auto_init": true, "private": true}))).await;
    assert_eq!(st, 201, "{v}");
    (name, v["id"].as_i64().unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn app_jwt_installation_tokens_are_confined() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let (a_name, a_id) = private_repo(&s, &owner).await;
    let (b_name, _) = private_repo(&s, &owner).await;
    let app = create_app(&s, &owner, json!({"issues": "write", "contents": "read", "checks": "write"}), json!(["issues", "push"]), false).await;

    // JWT authentication
    let (st, v) = req(&s, Some(&app.jwt()), "GET", "/app", None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["id"], app.id);
    assert_eq!(v["permissions"]["metadata"], "read");
    let (st, _) = req(&s, Some(&tok(&owner)), "GET", "/app", None).await;
    assert_eq!(st, 401);
    let now = chrono::Utc::now().timestamp();
    let long = forge::apps::jwt::sign_jwt(&app.pem, &json!({"iss": app.id, "iat": now, "exp": now + 3600})).unwrap();
    let (st, v) = req(&s, Some(&format!("Bearer {long}")), "GET", "/app", None).await;
    assert_eq!(st, 401, "{v}");
    let (st, v) = req(&s, None, "GET", &format!("/apps/{}", app.slug), None).await;
    assert_eq!(st, 404, "private apps are hidden: {v}");
    let (st, _) = req(&s, Some(&tok(&owner)), "GET", &format!("/apps/{}", app.slug), None).await;
    assert_eq!(st, 200);

    // install on the owner's account, repository A only
    let inst = install(&s, &owner, &app, json!({"repository_selection": "selected", "repository_ids": [a_id]})).await;
    let (st, v) = req(&s, Some(&app.jwt()), "GET", &format!("/users/{}/installation", owner.login), None).await;
    assert_eq!((st, v["id"].as_i64()), (200, Some(inst)));
    let (st, _) = req(&s, Some(&app.jwt()), "GET", &format!("/repos/{}/{}/installation", owner.login, b_name), None).await;
    assert_eq!(st, 404);
    let (st, v) = req(&s, Some(&app.jwt()), "GET", "/app/hook/deliveries", None).await;
    assert_eq!(st, 200);
    assert!(v.as_array().unwrap().iter().any(|d| d["event"] == "installation" && d["installation_id"] == inst), "{v}");

    // permissions beyond the installation are refused
    let (st, _) = req(&s, Some(&app.jwt()), "POST", &format!("/app/installations/{inst}/access_tokens"), Some(json!({"permissions": {"contents": "write"}}))).await;
    assert_eq!(st, 422);
    let (st, _) = req(&s, Some(&app.jwt()), "POST", &format!("/app/installations/{inst}/access_tokens"), Some(json!({"repositories": [b_name]}))).await;
    assert_eq!(st, 422);
    let (st, v) = req(&s, Some(&app.jwt()), "POST", &format!("/app/installations/{inst}/access_tokens"), None).await;
    assert_eq!(st, 201, "{v}");
    let token = v["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("ghs_"));
    let t = format!("token {token}");

    let (st, v) = req(&s, Some(&t), "GET", "/installation/repositories", None).await;
    assert_eq!((st, v["total_count"].as_i64()), (200, Some(1)), "{v}");
    let (st, _) = req(&s, Some(&t), "GET", &format!("/repos/{}/{a_name}", owner.login), None).await;
    assert_eq!(st, 200);
    let (st, _) = req(&s, Some(&t), "GET", &format!("/repos/{}/{b_name}", owner.login), None).await;
    assert_eq!(st, 404, "repositories outside the installation are invisible");
    let (st, v) = req(&s, Some(&t), "POST", &format!("/repos/{}/{a_name}/issues", owner.login), Some(json!({"title": "from the app"}))).await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["user"]["type"], "Bot");
    let (st, v) = req(
        &s,
        Some(&t),
        "PUT",
        &format!("/repos/{}/{a_name}/contents/app.txt", owner.login),
        Some(json!({"message": "m", "content": "aGk="})),
    )
    .await;
    assert_eq!(st, 403, "{v}");
    assert_eq!(v["message"], "Resource not accessible by integration");
    let (st, _) = req(&s, Some(&t), "PATCH", &format!("/repos/{}/{a_name}", owner.login), Some(json!({"description": "x"}))).await;
    assert_eq!(st, 403);
    let (st, _) = req(&s, Some(&t), "GET", "/user", None).await;
    assert_eq!(st, 403);
    let (st, _) = req(&s, Some(&t), "POST", &format!("{}/api/forge/tokens", s.base), Some(json!({"name": "x"}))).await;
    assert_eq!(st, 403, "app tokens can't mint personal tokens");

    // check runs created with the token belong to the app
    let (_, branch) = req(&s, Some(&tok(&owner)), "GET", &format!("/repos/{}/{a_name}/branches/main", owner.login), None).await;
    let sha = branch["commit"]["sha"].as_str().unwrap().to_string();
    let (st, v) = req(&s, Some(&t), "POST", &format!("/repos/{}/{a_name}/check-runs", owner.login), Some(json!({"name": "app-check", "head_sha": sha}))).await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["app"]["id"], app.id);
    assert_eq!(v["app"]["slug"], app.slug.as_str());

    // git: clone works with x-access-token, push is refused (contents: read)
    let dir = s.work_dir(&unique("w"));
    let url = format!("http://x-access-token:{token}@{}/{}/{a_name}.git", s.base.trim_start_matches("http://"), owner.login);
    let (ok, out) = s.git(&dir, &["clone", &url, "c"]);
    assert!(ok, "{out}");
    let c = dir.join("c");
    std::fs::write(c.join("f.txt"), "x").unwrap();
    s.git(&c, &["add", "."]);
    s.git(&c, &["commit", "-m", "x"]);
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD"]);
    assert!(!ok, "push must be refused: {out}");

    // a token restricted to no repo of B; suspended installations lose their tokens
    let (st, _) = req(&s, Some(&app.jwt()), "PUT", &format!("/app/installations/{inst}/suspended"), None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&t), "GET", &format!("/repos/{}/{a_name}", owner.login), None).await;
    assert_eq!(st, 401);
    let (st, _) = req(&s, Some(&app.jwt()), "POST", &format!("/app/installations/{inst}/access_tokens"), None).await;
    assert_eq!(st, 403);
    let (st, _) = req(&s, Some(&app.jwt()), "DELETE", &format!("/app/installations/{inst}/suspended"), None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&t), "DELETE", "/installation/token", None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&t), "GET", "/installation/repositories", None).await;
    assert_eq!(st, 401);

    // the user's view of installations; adding repository B
    let (st, v) = req(&s, Some(&tok(&owner)), "GET", "/user/installations", None).await;
    assert_eq!(st, 200);
    assert!(v["installations"].as_array().unwrap().iter().any(|i| i["id"] == inst));
    let (_, b) = req(&s, Some(&tok(&owner)), "GET", &format!("/repos/{}/{b_name}", owner.login), None).await;
    let b_id = b["id"].as_i64().unwrap();
    let (st, _) = req(&s, Some(&tok(&owner)), "PUT", &format!("/user/installations/{inst}/repositories/{b_id}"), None).await;
    assert_eq!(st, 204);
    let (_, v) = req(&s, Some(&tok(&owner)), "GET", &format!("/user/installations/{inst}/repositories"), None).await;
    assert_eq!(v["total_count"], 2);
    let other = s.user().await;
    let (st, _) = req(&s, Some(&tok(&other)), "GET", &format!("/user/installations/{inst}/repositories"), None).await;
    assert_eq!(st, 404);
    let (st, _) = req(&s, Some(&app.jwt()), "DELETE", &format!("/app/installations/{inst}"), None).await;
    assert_eq!(st, 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn contents_write_tokens_can_push_and_scope_to_repositories() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let (a_name, _) = private_repo(&s, &owner).await;
    let (b_name, b_id) = private_repo(&s, &owner).await;
    let app = create_app(&s, &owner, json!({"contents": "write"}), json!([]), true).await;
    let inst = install(&s, &owner, &app, json!({"repository_selection": "all"})).await;
    let (st, v) = req(&s, Some(&app.jwt()), "POST", &format!("/app/installations/{inst}/access_tokens"), Some(json!({"repository_ids": [b_id]}))).await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["repository_selection"], "selected");
    let t = format!("Bearer {}", v["token"].as_str().unwrap());
    let (st, _) = req(&s, Some(&t), "GET", &format!("/repos/{}/{a_name}", owner.login), None).await;
    assert_eq!(st, 404, "token restricted to B");
    let (st, v) = req(&s, Some(&t), "PUT", &format!("/repos/{}/{b_name}/contents/app.txt", owner.login), Some(json!({"message": "by app", "content": "aGk="}))).await;
    assert_eq!(st, 201, "{v}");
    // issues: not granted
    let (st, _) = req(&s, Some(&t), "POST", &format!("/repos/{}/{b_name}/issues", owner.login), Some(json!({"title": "x"}))).await;
    assert_eq!(st, 403);
    // public app: anyone can see it
    let (st, _) = req(&s, None, "GET", &format!("/apps/{}", app.slug), None).await;
    assert_eq!(st, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_flow_and_user_to_server_tokens() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let name = unique("Manifest ");
    let (st, v) = req(
        &s,
        Some(&tok(&owner)),
        "POST",
        &format!("{}/api/forge/app-manifests", s.base),
        Some(json!({"manifest": {"name": name, "url": "https://example.com", "default_permissions": {"issues": "write"}, "public": true}})),
    )
    .await;
    assert_eq!(st, 201, "{v}");
    let code = v["code"].as_str().unwrap().to_string();
    let (st, v) = req(&s, None, "POST", &format!("/app-manifests/{code}/conversions"), None).await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["owner"]["login"], owner.login.as_str());
    let app = App {
        id: v["id"].as_i64().unwrap(),
        slug: v["slug"].as_str().unwrap().into(),
        client_id: v["client_id"].as_str().unwrap().into(),
        client_secret: v["client_secret"].as_str().unwrap().into(),
        pem: v["pem"].as_str().unwrap().into(),
    };
    let (st, _) = req(&s, None, "POST", &format!("/app-manifests/{code}/conversions"), None).await;
    assert_eq!(st, 404, "codes are single-use");
    // the webhook secret is stored sealed, never in plaintext
    let cfg: Value = sqlx::query_scalar("SELECT config FROM webhooks WHERE app_id = $1").bind(app.id).fetch_one(&s.state.db).await.unwrap();
    assert!(cfg.get("secret").is_none(), "{cfg}");
    assert_eq!(forge::apps::hooks::open_secret(&s.state, app.id, &cfg).await.as_deref(), v["webhook_secret"].as_str());
    let (_, hc) = req(&s, Some(&app.jwt()), "GET", "/app/hook/config", None).await;
    assert_eq!(hc["secret"], "********");
    let (st, _) = req(&s, Some(&app.jwt()), "GET", "/app", None).await;
    assert_eq!(st, 200);

    let (repo, _) = private_repo(&s, &owner).await;
    install(&s, &owner, &app, json!({"repository_selection": "all"})).await;

    // device flow -> ghu_ + ghr_
    let client = s.client();
    let res: Value = client
        .post(format!("{}/login/device/code", s.base))
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form(&[("client_id", app.client_id.as_str())]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let (st, _) = req(&s, Some(&tok(&owner)), "POST", &format!("{}/api/forge/device/approve", s.base), Some(json!({"user_code": res["user_code"]}))).await;
    assert_eq!(st, 204);
    let t: Value = client
        .post(format!("{}/login/oauth/access_token", s.base))
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form(&[("client_id", app.client_id.as_str()), ("device_code", res["device_code"].as_str().unwrap()), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code")]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ghu = t["access_token"].as_str().unwrap().to_string();
    assert!(ghu.starts_with("ghu_"), "{t}");
    assert!(t["refresh_token"].as_str().unwrap().starts_with("ghr_"));
    assert_eq!(t["expires_in"], 28800);
    let u2s = format!("token {ghu}");
    let (st, v) = req(&s, Some(&u2s), "GET", "/user", None).await;
    assert_eq!((st, v["login"].as_str()), (200, Some(owner.login.as_str())));
    let (st, _) = req(&s, Some(&u2s), "POST", &format!("/repos/{}/{repo}/issues", owner.login), Some(json!({"title": "as user via app"}))).await;
    assert_eq!(st, 201);
    let (st, _) = req(&s, Some(&u2s), "PATCH", &format!("/repos/{}/{repo}", owner.login), Some(json!({"description": "x"}))).await;
    assert_eq!(st, 403, "the app has no administration permission");

    // check / reset / scope / delete with client credentials
    let cc = basic(&app.client_id, &app.client_secret);
    let (st, v) = req(&s, Some(&cc), "POST", &format!("/applications/{}/token", app.client_id), Some(json!({"access_token": ghu}))).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["user"]["login"], owner.login.as_str());
    let (st, _) = req(&s, Some(&basic(&app.client_id, "wrong")), "POST", &format!("/applications/{}/token", app.client_id), Some(json!({"access_token": ghu}))).await;
    assert_eq!(st, 404);
    let (st, v) = req(&s, Some(&cc), "POST", &format!("/applications/{}/token/scoped", app.client_id), Some(json!({"access_token": ghu, "target": owner.login, "permissions": {"issues": "read"}}))).await;
    assert_eq!(st, 200, "{v}");
    let scoped = format!("token {}", v["token"].as_str().unwrap());
    let (st, _) = req(&s, Some(&scoped), "POST", &format!("/repos/{}/{repo}/issues", owner.login), Some(json!({"title": "x"}))).await;
    assert_eq!(st, 403, "scoped to issues: read");
    let (st, v) = req(&s, Some(&cc), "PATCH", &format!("/applications/{}/token", app.client_id), Some(json!({"access_token": ghu}))).await;
    assert_eq!(st, 200, "{v}");
    let reset = v["token"].as_str().unwrap().to_string();
    let (st, _) = req(&s, Some(&u2s), "GET", "/user", None).await;
    assert_eq!(st, 401, "the old token stops working");
    // refresh rotates tokens
    let r: Value = client
        .post(format!("{}/login/oauth/access_token", s.base))
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form(&[("client_id", app.client_id.as_str()), ("client_secret", app.client_secret.as_str()), ("grant_type", "refresh_token"), ("refresh_token", t["refresh_token"].as_str().unwrap())]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(r["access_token"].as_str().unwrap().starts_with("ghu_"), "{r}");
    let (st, _) = req(&s, Some(&format!("token {reset}")), "GET", "/user", None).await;
    assert_eq!(st, 401, "refreshing revokes the previous access token");
    let fresh = r["access_token"].as_str().unwrap();
    let (st, _) = req(&s, Some(&cc), "DELETE", &format!("/applications/{}/token", app.client_id), Some(json!({"access_token": fresh}))).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&format!("token {fresh}")), "GET", "/user", None).await;
    assert_eq!(st, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_oauth_authorizations() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let pw = basic(&u.login, "password123");
    let (st, _) = req(&s, Some(&tok(&u)), "GET", "/authorizations", None).await;
    assert_eq!(st, 401, "Basic auth only");
    let (st, _) = req(&s, Some(&pw), "POST", "/authorizations", Some(json!({"scopes": ["repo"]}))).await;
    assert_eq!(st, 422, "note required");
    let (st, _) = req(&s, Some(&pw), "POST", "/authorizations", Some(json!({"scopes": ["nope"], "note": "n"}))).await;
    assert_eq!(st, 422);
    let (st, v) = req(&s, Some(&pw), "POST", "/authorizations", Some(json!({"scopes": ["repo", "gist"], "note": "ci token"}))).await;
    assert_eq!(st, 201, "{v}");
    let id = v["id"].as_i64().unwrap();
    let token = v["token"].as_str().unwrap().to_string();
    assert_eq!(v["app"]["client_id"], "00000000000000000000");
    let (st, me) = req(&s, Some(&format!("token {token}")), "GET", "/user", None).await;
    assert_eq!((st, me["login"].as_str()), (200, Some(u.login.as_str())));
    let (st, _) = req(&s, Some(&pw), "POST", "/authorizations", Some(json!({"scopes": ["repo"], "note": "ci token"}))).await;
    assert_eq!(st, 422, "duplicate note");
    let (st, v) = req(&s, Some(&pw), "GET", &format!("/authorizations/{id}"), None).await;
    assert_eq!((st, v["token"].as_str()), (200, Some("")));
    let (st, v) = req(&s, Some(&pw), "PATCH", &format!("/authorizations/{id}"), Some(json!({"add_scopes": ["user"]}))).await;
    assert_eq!(st, 200);
    assert_eq!(v["scopes"], json!(["repo", "gist", "user"]));
    let (st, _) = req(&s, Some(&pw), "PATCH", &format!("/authorizations/{id}"), Some(json!({"scopes": ["repo"], "add_scopes": ["user"]}))).await;
    assert_eq!(st, 422);
    let (st, v) = req(&s, Some(&pw), "GET", "/authorizations", None).await;
    assert_eq!(st, 200);
    assert!(v.as_array().unwrap().iter().any(|a| a["id"] == id));
    // for an OAuth app, through get-or-create
    let (st, oa) = req(&s, Some(&tok(&u)), "POST", &format!("{}/api/forge/oauth-apps", s.base), Some(json!({"name": "Legacy", "url": "https://example.com"}))).await;
    assert_eq!(st, 201, "{oa}");
    let cid = oa["client_id"].as_str().unwrap();
    let (st, v) = req(&s, Some(&pw), "PUT", &format!("/authorizations/clients/{cid}/fp1"), Some(json!({"client_secret": oa["client_secret"], "scopes": ["repo"]}))).await;
    assert_eq!(st, 201, "{v}");
    let (st, again) = req(&s, Some(&pw), "PUT", &format!("/authorizations/clients/{cid}/fp1"), Some(json!({"client_secret": oa["client_secret"]}))).await;
    assert_eq!((st, again["id"].clone()), (200, v["id"].clone()));
    let (st, grants) = req(&s, Some(&pw), "GET", "/applications/grants", Some(json!(null))).await;
    assert_eq!(st, 200);
    let g = grants.as_array().unwrap().iter().find(|g| g["app"]["client_id"] == cid).unwrap()["id"].as_i64().unwrap();
    let (st, _) = req(&s, Some(&pw), "DELETE", &format!("/applications/grants/{g}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&pw), "DELETE", &format!("/authorizations/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&format!("token {token}")), "GET", "/user", None).await;
    assert_eq!(st, 401, "deleted authorizations revoke their token");
}

#[tokio::test(flavor = "multi_thread")]
async fn migrations_export_lock_and_download() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let name = s.create_repo(&u, true).await;
    let (st, _) = req(&s, Some(&tok(&u)), "POST", &format!("/repos/{}/{name}/issues", u.login), Some(json!({"title": "exported issue"}))).await;
    assert_eq!(st, 201);
    let (st, m) = req(&s, Some(&tok(&u)), "POST", "/user/migrations", Some(json!({"repositories": [name], "lock_repositories": true}))).await;
    assert_eq!(st, 201, "{m}");
    assert_eq!(m["state"], "pending");
    let id = m["id"].as_i64().unwrap();
    // locked: REST writes and pushes are refused
    let (st, v) = req(&s, Some(&tok(&u)), "POST", &format!("/repos/{}/{name}/issues", u.login), Some(json!({"title": "x"}))).await;
    assert_eq!(st, 403, "{v}");
    let other = s.user().await;
    let (st, _) = req(&s, Some(&tok(&other)), "POST", &format!("/repos/{}/{name}/issues", u.login), Some(json!({"title": "x"}))).await;
    assert_eq!(st, 403, "public repo: the lock is visible");
    let dir = s.work_dir(&unique("m"));
    let (ok, out) = s.git(&dir, &["clone", &s.clone_url(&u, &name), "c"]);
    assert!(ok, "{out}");
    let c = dir.join("c");
    std::fs::write(c.join("f.txt"), "x").unwrap();
    s.git(&c, &["add", "."]);
    s.git(&c, &["commit", "-m", "x"]);
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD"]);
    assert!(!ok, "{out}");
    // run the export job
    for _ in 0..50 {
        forge::jobs::run_once(&s.state).await.unwrap();
        let (_, v) = req(&s, Some(&tok(&u)), "GET", &format!("/user/migrations/{id}"), None).await;
        if v["state"] == "exported" || v["state"] == "failed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (_, v) = req(&s, Some(&tok(&u)), "GET", &format!("/user/migrations/{id}"), None).await;
    assert_eq!(v["state"], "exported", "{v}");
    let (st, list) = req(&s, Some(&tok(&u)), "GET", &format!("/user/migrations/{id}/repositories"), None).await;
    assert_eq!((st, list.as_array().map(Vec::len)), (200, Some(1)));
    let nofollow = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let res = nofollow.get(format!("{}/user/migrations/{id}/archive", s.api)).header("Authorization", tok(&u)).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 302);
    let loc = res.headers()["location"].to_str().unwrap().to_string();
    let bytes = reqwest::get(&loc).await.unwrap().bytes().await.unwrap();
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    let names: Vec<String> = ar.entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string()).collect();
    assert!(names.iter().any(|n| n.ends_with("schema.json")), "{names:?}");
    assert!(names.iter().any(|n| n.ends_with("issues_000001.json")), "{names:?}");
    assert!(names.iter().any(|n| n.ends_with(&format!("repositories/{}/{name}.bundle", u.login))), "{names:?}");
    let tampered = format!("{}{}", &loc[..loc.len() - 1], if loc.ends_with('0') { '1' } else { '0' });
    let (st, _) = req(&s, None, "GET", &tampered, None).await;
    assert_eq!(st, 404);
    // unlock, then writes work again
    let (st, _) = req(&s, Some(&tok(&u)), "DELETE", &format!("/user/migrations/{id}/repos/{name}/lock"), None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&tok(&u)), "POST", &format!("/repos/{}/{name}/issues", u.login), Some(json!({"title": "after unlock"}))).await;
    assert_eq!(st, 201);
    let (st, _) = req(&s, Some(&tok(&u)), "DELETE", &format!("/user/migrations/{id}/archive"), None).await;
    assert_eq!(st, 204);
    let res = nofollow.get(format!("{}/user/migrations/{id}/archive", s.api)).header("Authorization", tok(&u)).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 404);
    let (st, _) = req(&s, Some(&tok(&u)), "POST", "/user/migrations", Some(json!({"repositories": ["does-not-exist"]}))).await;
    assert_eq!(st, 422);
}
