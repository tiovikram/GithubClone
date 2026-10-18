//! Social web UI build-out: user blocks, custom watching, issue subscriptions, profile fields
//! (hireable, counters), email changes, account settings (password, rename, delete), and the
//! CreateEvent recorded when a repository is created.
mod common;
use std::time::Duration;

use common::{TestServer, TestUser};
use serde_json::{json, Value};

/// Request against /api/forge (token auth, so no CSRF header needed).
async fn forge(s: &TestServer, u: &TestUser, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = s.client().request(method.parse().unwrap(), format!("{}/api/forge{path}", s.base)).header("Authorization", format!("token {}", u.token));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let st = res.status().as_u16();
    let text = res.text().await.unwrap();
    (st, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn titles(s: &TestServer, u: &TestUser) -> Vec<String> {
    let (_, n) = s.call(Some(u), "GET", "/notifications?all=true", None).await;
    n.as_array().unwrap().iter().map(|t| t["subject"]["title"].as_str().unwrap().to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocks_prevent_follows_issues_and_comments() {
    let s = TestServer::start().await;
    let a = s.user().await;
    let b = s.user().await;
    let repo = s.create_repo(&a, true).await;
    let rp = format!("/repos/{}/{repo}", a.login);
    let (st, _) = s.call(Some(&b), "POST", &format!("{rp}/issues"), Some(json!({"title": "before block"}))).await;
    assert_eq!(st, 201);
    assert_eq!(s.call(Some(&b), "PUT", &format!("/user/following/{}", a.login), None).await.0, 204);
    assert_eq!(s.call(Some(&a), "PUT", &format!("/user/following/{}", b.login), None).await.0, 204);

    assert_eq!(s.call(Some(&a), "GET", &format!("/user/blocks/{}", b.login), None).await.0, 404);
    assert_eq!(s.call(Some(&a), "PUT", &format!("/user/blocks/{}", b.login), None).await.0, 204);
    assert_eq!(s.call(Some(&a), "GET", &format!("/user/blocks/{}", b.login), None).await.0, 204);
    let (st, list) = s.call(Some(&a), "GET", "/user/blocks", None).await;
    assert_eq!(st, 200);
    assert_eq!(list[0]["login"], b.login.as_str());
    // can't block yourself
    assert_eq!(s.call(Some(&a), "PUT", &format!("/user/blocks/{}", a.login), None).await.0, 422);

    // Follows in both directions are gone; the blocked user can't follow again.
    let (_, f) = s.call(None, "GET", &format!("/users/{}/followers", a.login), None).await;
    assert_eq!(f.as_array().unwrap().len(), 0, "{f}");
    let (_, f) = s.call(None, "GET", &format!("/users/{}/followers", b.login), None).await;
    assert_eq!(f.as_array().unwrap().len(), 0, "{f}");
    assert_eq!(s.call(Some(&b), "PUT", &format!("/user/following/{}", a.login), None).await.0, 403);
    // No new issues, comments or pull requests in the blocker's repositories.
    assert_eq!(s.call(Some(&b), "POST", &format!("{rp}/issues"), Some(json!({"title": "blocked"}))).await.0, 403);
    assert_eq!(s.call(Some(&b), "POST", &format!("{rp}/issues/1/comments"), Some(json!({"body": "blocked"}))).await.0, 403);
    // The owner still can.
    assert_eq!(s.call(Some(&a), "POST", &format!("{rp}/issues/1/comments"), Some(json!({"body": "owner"}))).await.0, 201);

    assert_eq!(s.call(Some(&a), "DELETE", &format!("/user/blocks/{}", b.login), None).await.0, 204);
    assert_eq!(s.call(Some(&b), "PUT", &format!("/user/following/{}", a.login), None).await.0, 204);
    assert_eq!(s.call(Some(&b), "POST", &format!("{rp}/issues/1/comments"), Some(json!({"body": "unblocked"}))).await.0, 201);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn custom_watching_filters_subject_kinds() {
    let s = TestServer::start().await;
    let a = s.user().await;
    let w = s.user().await;
    let repo = s.create_repo(&a, true).await;
    let full = format!("{}/{repo}", a.login);
    let (st, v) = forge(&s, &w, "GET", &format!("/repos/{full}/subscription"), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["state"], "not_watching");

    let (st, v) = forge(&s, &w, "PUT", &format!("/repos/{full}/subscription"), Some(json!({"state": "custom", "events": ["releases"]}))).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["state"], "custom");
    assert_eq!(v["events"], json!(["releases"]));
    assert_eq!(forge(&s, &w, "PUT", &format!("/repos/{full}/subscription"), Some(json!({"state": "custom", "events": ["wiki"]}))).await.0, 422);
    // REST view: subscribed
    let (_, rest) = s.call(Some(&w), "GET", &format!("/repos/{full}/subscription"), None).await;
    assert_eq!(rest["subscribed"], true);

    s.call(Some(&a), "POST", &format!("/repos/{full}/issues"), Some(json!({"title": "custom-issue"}))).await;
    s.call(Some(&a), "POST", &format!("/repos/{full}/releases"), Some(json!({"tag_name": "v1", "name": "custom-release"}))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t = titles(&s, &w).await;
    assert!(t.contains(&"custom-release".to_string()), "{t:?}");
    assert!(!t.contains(&"custom-issue".to_string()), "{t:?}");

    // "watching" (all activity) and REST PUT both clear the custom kinds.
    let (_, v) = forge(&s, &w, "PUT", &format!("/repos/{full}/subscription"), Some(json!({"state": "watching"}))).await;
    assert_eq!(v["state"], "watching");
    s.call(Some(&a), "POST", &format!("/repos/{full}/issues"), Some(json!({"title": "all-issue"}))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(titles(&s, &w).await.contains(&"all-issue".to_string()));
    let (_, v) = forge(&s, &w, "PUT", &format!("/repos/{full}/subscription"), Some(json!({"state": "ignoring"}))).await;
    assert_eq!(v["state"], "ignoring");
    let (_, v) = forge(&s, &w, "PUT", &format!("/repos/{full}/subscription"), Some(json!({"state": "not_watching"}))).await;
    assert_eq!(v["state"], "not_watching");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_subscription_endpoint() {
    let s = TestServer::start().await;
    let a = s.user().await;
    let b = s.user().await;
    let repo = s.create_repo(&a, true).await;
    let full = format!("{}/{repo}", a.login);
    s.call(Some(&b), "POST", &format!("/repos/{full}/issues"), Some(json!({"title": "sub-issue"}))).await;
    let path = format!("/repos/{full}/issues/1/subscription");
    let (st, v) = forge(&s, &b, "GET", &path, None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v, json!({"subscribed": true, "reason": "author"}));
    // The owner watches the repository (auto-watch on create).
    let (_, v) = forge(&s, &a, "GET", &path, None).await;
    assert_eq!(v["reason"], "subscribed");

    // Unsubscribe the owner: the author's comments no longer notify them.
    let (_, v) = forge(&s, &a, "DELETE", &path, None).await;
    assert_eq!(v, json!({"subscribed": false, "reason": "unsubscribed"}));
    let (_, n) = s.call(Some(&a), "GET", "/notifications", None).await;
    for t in n.as_array().unwrap() {
        let id = t["id"].as_str().unwrap();
        s.call(Some(&a), "DELETE", &format!("/notifications/threads/{id}"), None).await;
    }
    s.call(Some(&b), "POST", &format!("/repos/{full}/issues/1/comments"), Some(json!({"body": "quiet"}))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, n) = s.call(Some(&a), "GET", "/notifications", None).await;
    assert_eq!(n.as_array().unwrap().len(), 0, "{n}");

    let (_, v) = forge(&s, &a, "PUT", &path, None).await;
    assert_eq!(v, json!({"subscribed": true, "reason": "manual"}));
    s.call(Some(&b), "POST", &format!("/repos/{full}/issues/1/comments"), Some(json!({"body": "loud"}))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, n) = s.call(Some(&a), "GET", "/notifications", None).await;
    assert_eq!(n[0]["subject"]["title"], "sub-issue", "{n}");
    assert_eq!(forge(&s, &a, "GET", &format!("/repos/{full}/issues/99/subscription"), None).await.0, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_fields_counters_and_email_change() {
    let s = TestServer::start().await;
    let a = s.user().await;
    let b = s.user().await;
    let (st, me) = s.call(Some(&a), "PATCH", "/user", Some(json!({"hireable": true, "bio": "hi"}))).await;
    assert_eq!(st, 200, "{me}");
    assert_eq!(me["hireable"], true);
    let (_, pubu) = s.call(None, "GET", &format!("/users/{}", a.login), None).await;
    assert_eq!(pubu["hireable"], true);
    assert_eq!(pubu["bio"], "hi");

    s.call(Some(&b), "PUT", &format!("/user/following/{}", a.login), None).await;
    s.call(Some(&a), "POST", "/gists", Some(json!({"public": true, "files": {"a.txt": {"content": "x"}}}))).await;
    let (_, me) = s.call(Some(&a), "GET", "/user", None).await;
    assert_eq!(me["followers"], 1, "{me}");
    assert_eq!(me["public_gists"], 1, "{me}");
    let (_, pubu) = s.call(None, "GET", &format!("/users/{}", a.login), None).await;
    assert_eq!(pubu["public_gists"], 1);

    // Making an added address primary keeps the old primary as a secondary address.
    let old = format!("{}@example.com", a.login);
    let new = format!("{}+new@example.com", a.login);
    assert_eq!(s.call(Some(&a), "POST", "/user/emails", Some(json!({"emails": [new]}))).await.0, 201);
    let (_, me) = s.call(Some(&a), "PATCH", "/user", Some(json!({"email": new}))).await;
    assert_eq!(me["email"], new.as_str());
    let (_, emails) = s.call(Some(&a), "GET", "/user/emails", None).await;
    let list: Vec<(String, bool)> = emails.as_array().unwrap().iter().map(|e| (e["email"].as_str().unwrap().to_string(), e["primary"].as_bool().unwrap())).collect();
    assert_eq!(list, vec![(new.clone(), true), (old.clone(), false)], "{emails}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_password_rename_delete() {
    let s = TestServer::start().await;
    let a = s.user().await;
    // On a fresh test database the first user is a site admin (who can't delete themselves).
    sqlx::query("UPDATE users SET site_admin = FALSE WHERE lower(login) = lower($1)").bind(&a.login).execute(&s.state.db).await.unwrap();
    let (st, e) = forge(&s, &a, "PUT", "/user/password", Some(json!({"current_password": "nope-nope", "password": "newpass123"}))).await;
    assert_eq!(st, 422, "{e}");
    assert_eq!(forge(&s, &a, "PUT", "/user/password", Some(json!({"current_password": "password123", "password": "short"}))).await.0, 422);
    assert_eq!(forge(&s, &a, "PUT", "/user/password", Some(json!({"current_password": "password123", "password": "newpass123"}))).await.0, 204);
    let login = |pw: &str| {
        let body = json!({"login": a.login, "password": pw});
        let url = format!("{}/api/forge/session", s.base);
        let c = s.client();
        async move { c.post(url).header("X-Forge-CSRF", "1").json(&body).send().await.unwrap().status().as_u16() }
    };
    assert_eq!(login("password123").await, 401);
    assert_eq!(login("newpass123").await, 200);

    // Rename.
    let renamed = format!("{}-r", a.login);
    let (st, me) = forge(&s, &a, "PATCH", "/user/login", Some(json!({"login": renamed}))).await;
    assert_eq!(st, 200, "{me}");
    assert_eq!(me["login"], renamed.as_str());
    assert_eq!(forge(&s, &a, "PATCH", "/user/login", Some(json!({"login": "settings"}))).await.0, 422);
    let other = s.user().await;
    assert_eq!(forge(&s, &a, "PATCH", "/user/login", Some(json!({"login": other.login}))).await.0, 422);

    // Delete: needs the login and the password.
    let repo = s.create_repo(&a, true).await;
    assert_eq!(forge(&s, &a, "DELETE", "/user", Some(json!({"login": "wrong", "password": "newpass123"}))).await.0, 422);
    assert_eq!(forge(&s, &a, "DELETE", "/user", Some(json!({"login": renamed, "password": "bad-password"}))).await.0, 422);
    assert_eq!(forge(&s, &a, "DELETE", "/user", Some(json!({"login": renamed, "password": "newpass123"}))).await.0, 204);
    assert_eq!(s.call(None, "GET", &format!("/users/{renamed}"), None).await.0, 404);
    assert_eq!(s.call(None, "GET", &format!("/repos/{renamed}/{repo}"), None).await.0, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repository_creation_records_create_event() {
    let s = TestServer::start().await;
    let a = s.user().await;
    let repo = s.create_repo(&a, false).await;
    let (st, ev) = s.call(Some(&a), "GET", &format!("/users/{}/events", a.login), None).await;
    assert_eq!(st, 200);
    let e = ev.as_array().unwrap().iter().find(|e| e["type"] == "CreateEvent").unwrap_or_else(|| panic!("{ev}"));
    assert_eq!(e["payload"]["ref_type"], "repository");
    assert_eq!(e["repo"]["name"], format!("{}/{repo}", a.login));
}
