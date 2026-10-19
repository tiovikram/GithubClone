//! Forge-only endpoints behind the web UI's packages area: package lists/details/settings/access
//! (`/api/forge/packages/*`), app management (`/api/forge/apps/{slug}`), installation settings
//! (`/api/forge/installations/*`), authorized applications and the device-flow description.
mod common;

use base64::Engine;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};
use sha1::Digest;

async fn req(s: &TestServer, auth: Option<&str>, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let url = if path.starts_with("http") { path.to_string() } else { format!("{}/api/forge{}", s.base, path) };
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

fn tok(u: &TestUser) -> String {
    format!("token {}", u.token)
}

/// A gzipped npm tarball with `package/package.json` and `package/README.md`.
fn tgz(name: &str, readme: &str) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (path, data) in [("package/package.json", format!("{{\"name\":\"{name}\"}}")), ("package/README.md", readme.to_string())] {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, path, data.as_bytes()).unwrap();
    }
    let tar = b.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, &tar).unwrap();
    gz.finish().unwrap()
}

async fn npm_publish(s: &TestServer, u: &TestUser, name: &str, version: &str, readme: &str) {
    let full = format!("@{}/{name}", u.login.to_lowercase());
    let t = tgz(&full, readme);
    let file = format!("{name}-{version}.tgz");
    let doc = json!({
        "_id": full, "name": full, "dist-tags": {"latest": version},
        "versions": {version: {"name": full, "version": version, "license": "MIT",
            "dist": {"shasum": hex::encode(sha1::Sha1::digest(&t)), "tarball": format!("{}/npm/{full}/-/{file}", s.base)}}},
        "_attachments": {file: {"content_type": "application/octet-stream", "data": base64::engine::general_purpose::STANDARD.encode(&t), "length": t.len()}},
    });
    let res = s
        .client()
        .put(format!("{}/npm/{}", s.base, full.replace('/', "%2f")))
        .header("Authorization", format!("Bearer {}", u.token))
        .json(&doc)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 201, "{}", res.text().await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn package_details_settings_access_and_deleted_list() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let name = unique("lib");
    npm_publish(&s, &u, &name, "1.0.0", "# Hello\n\nfrom the tarball README\n").await;
    let auth = tok(&u);
    let pkg_path = format!("/packages/{}/npm/{name}", u.login);

    // All-type list and details with the README extracted from the tarball.
    let (st, list) = req(&s, Some(&auth), "GET", &format!("/packages/{}", u.login), None).await;
    assert_eq!(st, 200);
    assert!(list.as_array().unwrap().iter().any(|p| p["name"] == name.as_str()), "{list}");
    let (st, d) = req(&s, Some(&auth), "GET", &pkg_path, None).await;
    assert_eq!(st, 200, "{d}");
    assert!(d["readme"].as_str().unwrap().contains("from the tarball README"), "{d}");
    assert_eq!(d["viewer_permission"], "admin");
    assert_eq!(d["visibility"], "private");
    let vid = d["latest_version"]["id"].as_i64().unwrap();
    let (st, v) = req(&s, Some(&auth), "GET", &format!("{pkg_path}/versions/{vid}"), None).await;
    assert_eq!(st, 200);
    assert!(v["digest"].as_str().unwrap().starts_with("sha256:"));

    // Others can't see it, and can't change it.
    let (st, _) = req(&s, Some(&tok(&other)), "GET", &pkg_path, None).await;
    assert_eq!(st, 404);
    let (st, _) = req(&s, Some(&tok(&other)), "PATCH", &pkg_path, Some(json!({"visibility": "public"}))).await;
    assert_eq!(st, 404);

    // Granular access: read, then admin.
    let (st, _) = req(&s, Some(&auth), "PUT", &format!("{pkg_path}/access/{}", other.login), Some(json!({"role": "read"}))).await;
    assert_eq!(st, 204);
    let (st, d2) = req(&s, Some(&tok(&other)), "GET", &pkg_path, None).await;
    assert_eq!(st, 200);
    assert_eq!(d2["viewer_permission"], "read");
    let (st, _) = req(&s, Some(&tok(&other)), "PATCH", &pkg_path, Some(json!({"visibility": "public"}))).await;
    assert_eq!(st, 403);
    let (st, acc) = req(&s, Some(&auth), "GET", &format!("{pkg_path}/access"), None).await;
    assert_eq!(st, 200);
    assert_eq!(acc[0]["role"], "read");
    let (st, _) = req(&s, Some(&auth), "DELETE", &format!("{pkg_path}/access/{}", other.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&tok(&other)), "GET", &pkg_path, None).await;
    assert_eq!(st, 404);

    // Settings: link a repository of the same owner, make it public.
    let repo = s.create_repo(&u, true).await;
    let (st, r) = req(&s, Some(&auth), "PATCH", &pkg_path, Some(json!({"repository": repo, "visibility": "public", "inherit_access": false}))).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["package"]["repository"]["name"], repo.as_str());
    assert_eq!(r["inherit_access"], false);
    let (st, _) = s.call(None, "GET", &format!("/users/{}/packages/npm/{name}", u.login), None).await;
    assert_eq!(st, 200, "public package is readable anonymously");
    let (st, rp) = req(&s, None, "GET", &format!("/repos/{}/{repo}/packages", u.login), None).await;
    assert_eq!(st, 200);
    assert_eq!(rp.as_array().unwrap().len(), 1);
    let (st, _) = req(&s, Some(&auth), "PATCH", &pkg_path, Some(json!({"repository": format!("{}/x", other.login)}))).await;
    assert_eq!(st, 422);
    let (st, _) = req(&s, Some(&auth), "PATCH", &pkg_path, Some(json!({"visibility": "internal"}))).await;
    assert_eq!(st, 422, "internal is for organizations");

    // Deleted list (restorable), then restore.
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/users/{}/packages/npm/{name}", u.login), None).await;
    assert_eq!(st, 204);
    let (st, del) = req(&s, Some(&auth), "GET", &format!("/packages/{}/deleted", u.login), None).await;
    assert_eq!(st, 200);
    assert!(del.as_array().unwrap().iter().any(|p| p["name"] == name.as_str() && p["deleted_at"].is_string() && p["name_in_use"] == false), "{del}");
    let (_, del_other) = req(&s, Some(&tok(&other)), "GET", &format!("/packages/{}/deleted", u.login), None).await;
    assert_eq!(del_other.as_array().unwrap().len(), 0);
    let (st, _) = s.call(Some(&u), "POST", &format!("/users/{}/packages/npm/{name}/restore", u.login), None).await;
    assert_eq!(st, 204);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_management_installation_settings_and_authorizations() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let auth = tok(&u);
    let (st, app) = req(&s, Some(&auth), "POST", "/apps", Some(json!({"name": unique("App "), "url": "https://example.com", "default_permissions": {"issues": "write"}}))).await;
    assert_eq!(st, 201, "{app}");
    let slug = app["slug"].as_str().unwrap().to_string();
    let app_id = app["id"].as_i64().unwrap();
    let client_id = app["client_id"].as_str().unwrap().to_string();

    // Managed view, edit, visibility.
    let (st, m) = req(&s, Some(&auth), "GET", &format!("/apps/{slug}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(m["keys"].as_array().unwrap().len(), 1);
    assert_eq!(m["public"], false);
    let (st, _) = req(&s, Some(&tok(&other)), "GET", &format!("/apps/{slug}"), None).await;
    assert_eq!(st, 404);
    let (st, m) = req(&s, Some(&auth), "PATCH", &format!("/apps/{slug}"),
        Some(json!({"description": "edited", "hook_attributes": {"url": "https://example.com/h2"}, "webhook_secret": "s", "public": true}))).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["description"], "edited");
    assert_eq!(m["webhook"]["url"], "https://example.com/h2");
    assert_eq!(m["webhook"]["has_secret"], true);
    assert_eq!(m["public"], true);
    let (st, _) = req(&s, Some(&auth), "PATCH", &format!("/apps/{slug}"), Some(json!({"url": "not a url"}))).await;
    assert_eq!(st, 422);

    // Keys: the last one can't be deleted; a new one can replace it.
    let kid = m["keys"][0]["id"].as_i64().unwrap();
    let (st, _) = req(&s, Some(&auth), "DELETE", &format!("/apps/{slug}/keys/{kid}"), None).await;
    assert_eq!(st, 422);
    let (st, k) = req(&s, Some(&auth), "POST", &format!("/apps/{slug}/keys"), None).await;
    assert_eq!(st, 201);
    let pem = k["pem"].as_str().unwrap().to_string();
    let (st, _) = req(&s, Some(&auth), "DELETE", &format!("/apps/{slug}/keys/{kid}"), None).await;
    assert_eq!(st, 204);
    let (st, sec) = req(&s, Some(&auth), "POST", &format!("/apps/{slug}/client-secret"), None).await;
    assert_eq!(st, 201);
    assert_eq!(sec["client_secret"].as_str().unwrap().len(), 40);

    // Install on a selected repository, then manage it from the account side.
    let (_, r1) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": unique("r"), "auto_init": true}))).await;
    let (_, r2) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": unique("r"), "auto_init": true}))).await;
    let (st, inst) = req(&s, Some(&auth), "POST", &format!("/apps/{slug}/installations"), Some(json!({"repository_selection": "selected", "repository_ids": [r1["id"]]}))).await;
    assert_eq!(st, 201, "{inst}");
    let iid = inst["id"].as_i64().unwrap();
    let ipath = format!("/installations/{iid}");
    let (st, d) = req(&s, Some(&auth), "GET", &ipath, None).await;
    assert_eq!(st, 200);
    assert_eq!(d["repositories"].as_array().unwrap().len(), 1);
    assert_eq!(d["viewer_can_manage"], true);
    assert!(d["pending_update"].is_null());
    let (st, _) = req(&s, Some(&tok(&other)), "GET", &ipath, None).await;
    assert_eq!(st, 404);
    let (st, d) = req(&s, Some(&auth), "PATCH", &ipath, Some(json!({"repository_selection": "selected", "repository_ids": [r1["id"], r2["id"]]}))).await;
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["repositories"].as_array().unwrap().len(), 2);

    // Permission changes wait for the installation owner.
    let jwt = || {
        let now = chrono::Utc::now().timestamp();
        format!("Bearer {}", forge::apps::jwt::sign_jwt(&pem, &json!({"iss": app_id, "iat": now - 30, "exp": now + 540})).unwrap())
    };
    let mint = |j: String| {
        let s = &s;
        async move { req(s, Some(&j), "POST", &format!("{}/app/installations/{iid}/access_tokens", s.api), None).await }
    };
    let (st, _) = req(&s, Some(&auth), "PATCH", &format!("/apps/{slug}"), Some(json!({"default_permissions": {"issues": "write", "contents": "read"}}))).await;
    assert_eq!(st, 200);
    let (_, t1) = mint(jwt()).await;
    assert!(t1["permissions"]["contents"].is_null(), "{t1}");
    let (_, d) = req(&s, Some(&auth), "GET", &ipath, None).await;
    assert_eq!(d["pending_update"]["permissions"]["contents"], "read");
    let (st, d) = req(&s, Some(&auth), "POST", &format!("{ipath}/permissions"), None).await;
    assert_eq!(st, 200);
    assert!(d["pending_update"].is_null());
    let (_, t2) = mint(jwt()).await;
    assert_eq!(t2["permissions"]["contents"], "read");

    // Suspend blocks tokens; uninstall removes the installation.
    let (st, _) = req(&s, Some(&auth), "PUT", &format!("{ipath}/suspended"), None).await;
    assert_eq!(st, 204);
    let (st, _) = mint(jwt()).await;
    assert_eq!(st, 403);
    let (st, _) = req(&s, Some(&auth), "DELETE", &format!("{ipath}/suspended"), None).await;
    assert_eq!(st, 204);
    let (st, _) = mint(jwt()).await;
    assert_eq!(st, 201);
    let (st, _) = req(&s, Some(&auth), "DELETE", &ipath, None).await;
    assert_eq!(st, 204);
    let (st, _) = mint(jwt()).await;
    assert_eq!(st, 404);

    // Device flow: the description names the app; the grant shows up and can be revoked.
    let (code, user_code) = forge::oauth_device::start(&s.state, &client_id, "").await.map_err(|e| e.message).unwrap();
    let (st, desc) = req(&s, Some(&tok(&other)), "GET", &format!("/device/{user_code}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(desc["app"]["kind"], "app");
    assert_eq!(desc["app"]["owner"], u.login.as_str());
    let (st, _) = req(&s, Some(&tok(&other)), "POST", "/device/approve", Some(json!({"user_code": user_code}))).await;
    assert_eq!(st, 204);
    let res = s
        .client()
        .post(format!("{}/login/oauth/access_token", s.base))
        .header("Accept", "application/json")
        .json(&json!({"client_id": client_id, "device_code": code, "grant_type": "urn:ietf:params:oauth:grant-type:device_code"}))
        .send()
        .await
        .unwrap();
    let t: Value = res.json().await.unwrap();
    let ghu = t["access_token"].as_str().unwrap().to_string();
    let (st, grants) = req(&s, Some(&tok(&other)), "GET", "/authorizations?kind=app", None).await;
    assert_eq!(st, 200);
    assert_eq!(grants.as_array().unwrap().len(), 1, "{grants}");
    let gid = grants[0]["id"].as_i64().unwrap();
    let (_, none) = req(&s, Some(&tok(&other)), "GET", "/authorizations?kind=oauth", None).await;
    assert_eq!(none.as_array().unwrap().len(), 0);
    let (st, _) = req(&s, Some(&auth), "DELETE", &format!("/authorizations/{gid}"), None).await;
    assert_eq!(st, 404, "someone else's grant");
    let (st, _) = req(&s, Some(&tok(&other)), "DELETE", &format!("/authorizations/{gid}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = req(&s, Some(&format!("token {ghu}")), "GET", &format!("{}/user", s.api), None).await;
    assert_eq!(st, 401);

    // OAuth apps are listed with their owner and can be edited.
    let (st, o) = req(&s, Some(&auth), "POST", "/oauth-apps", Some(json!({"name": "My OAuth", "url": "https://example.com"}))).await;
    assert_eq!(st, 201);
    let oslug = o["slug"].as_str().unwrap();
    let (_, mine) = req(&s, Some(&auth), "GET", "/apps", None).await;
    let row = mine.as_array().unwrap().iter().find(|a| a["slug"] == oslug).unwrap().clone();
    assert_eq!(row["owner"]["login"], u.login.as_str());
    let (st, m) = req(&s, Some(&auth), "PATCH", &format!("/apps/{oslug}"), Some(json!({"name": "Renamed", "callback_urls": ["https://example.com/cb"]}))).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["name"], "Renamed");
    assert_eq!(m["callback_urls"][0], "https://example.com/cb");
}
