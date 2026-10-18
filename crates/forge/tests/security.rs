//! Regression tests for security fixes (see docs/security.md; one test per finding).
mod common;

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn gql(s: &TestServer, u: &TestUser, query: &str, vars: Value) -> Value {
    let res = s
        .client()
        .post(format!("{}/api/graphql", s.base))
        .header("Authorization", format!("token {}", u.token))
        .json(&json!({"query": query, "variables": vars}))
        .send()
        .await
        .unwrap();
    res.json().await.unwrap()
}

/// Creates branch `feature` with one new file and opens a PR into `main`; returns the PR number.
async fn open_pr(s: &TestServer, u: &TestUser, rp: &str) -> i64 {
    let (st, r) = s.call(Some(u), "GET", &format!("{rp}/git/ref/heads/main"), None).await;
    assert_eq!(st, 200, "{r}");
    let sha = r["object"]["sha"].as_str().unwrap().to_string();
    let (st, r) = s.call(Some(u), "POST", &format!("{rp}/git/refs"), Some(json!({"ref": "refs/heads/feature", "sha": sha}))).await;
    assert_eq!(st, 201, "{r}");
    let (st, r) = s
        .call(Some(u), "PUT", &format!("{rp}/contents/f.txt"), Some(json!({"message": "f", "content": "aGVsbG8K", "branch": "feature"})))
        .await;
    assert_eq!(st, 201, "{r}");
    let (st, pr) = s.call(Some(u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "t", "head": "feature", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    pr["number"].as_i64().unwrap()
}

// ---- Critical: git option injection through GraphQL commit oids ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graphql_commit_node_id_cannot_inject_git_options() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let (_, r) = s.call(Some(&u), "GET", &format!("/repos/{}/{}", u.login, repo), None).await;
    let rid = r["id"].as_i64().unwrap();
    let target = s.dir.path().join("pwned.txt");
    let id = forge::ids::encode_str("Commit", &format!("{rid}:--output={}", target.display()));
    let v = gql(&s, &u, "query($id:ID!){node(id:$id){... on Commit{message oid}}}", json!({"id": id})).await;
    assert!(v["data"]["node"].is_null(), "{v}");
    assert!(!target.exists(), "git log --output wrote a file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_commit_id_must_be_a_sha() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let n = open_pr(&s, &u, &rp).await;
    let target = s.dir.path().join("pwned2.txt");
    let (st, e) = s
        .call(Some(&u), "POST", &format!("{rp}/pulls/{n}/reviews"), Some(json!({"body": "x", "event": "COMMENT", "commit_id": format!("--output={}", target.display())})))
        .await;
    assert_eq!(st, 422, "{e}");
    // Reading reviews' commits through GraphQL must not run git with it either.
    let v = gql(&s, &u, "query($o:String!,$n:String!,$p:Int!){repository(owner:$o,name:$n){pullRequest(number:$p){reviews(first:10){nodes{commit{message}}}}}}", json!({"o": u.login, "n": repo, "p": n})).await;
    assert!(v.get("errors").is_none(), "{v}");
    assert!(!target.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_review_not_readable_by_node_id() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let rp = format!("/repos/{}/{}", owner.login, repo);
    let n = open_pr(&s, &owner, &rp).await;
    // `other` starts a draft (PENDING) review on the public PR.
    let (st, r) = s.call(Some(&other), "POST", &format!("{rp}/pulls/{n}/reviews"), Some(json!({"body": "draft secret"}))).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["state"], "PENDING");
    let id = r["node_id"].as_str().unwrap().to_string();
    let q = "query($id:ID!){node(id:$id){... on PullRequestReview{body}}}";
    let v = gql(&s, &owner, q, json!({"id": id})).await;
    assert!(v["data"]["node"].is_null(), "pending review leaked: {v}");
    let v = gql(&s, &other, q, json!({"id": id})).await;
    assert_eq!(v["data"]["node"]["body"], "draft secret", "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn license_ref_is_not_an_option() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let (st, _) = s.call(Some(&u), "GET", &format!("/repos/{}/{}/license?ref=--abbrev=4", u.login, repo), None).await;
    assert_eq!(st, 404);
}

// ---- High: webhook SSRF ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webhooks_do_not_reach_internal_addresses() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    // An "internal service" on loopback.
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h2 = hits.clone();
    let app = axum::Router::new().fallback(move || {
        let h = h2.clone();
        async move {
            h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            "internal secret"
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    for url in [format!("http://127.0.0.1:{port}/"), format!("http://localhost:{port}/"), format!("http://[::ffff:127.0.0.1]:{port}/")] {
        let (st, hook) = s.call(Some(&u), "POST", &format!("{rp}/hooks"), Some(json!({"config": {"url": url, "content_type": "json"}}))).await;
        assert_eq!(st, 201, "{hook}");
        let hook_id = hook["id"].as_i64().unwrap();
        let d: i64 = sqlx::query_scalar("SELECT id FROM webhook_deliveries WHERE hook_id = $1").bind(hook_id).fetch_one(&s.state.db).await.unwrap();
        let _ = forge::webhooks::deliver(&s.state, d).await;
        let (_, full) = s.call(Some(&u), "GET", &format!("{rp}/hooks/{hook_id}/deliveries/{d}"), None).await;
        assert!(!full.to_string().contains("internal secret"), "{full}");
    }
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "internal service was reached");
}

#[test]
fn ssrf_address_classification() {
    for ip in ["127.0.0.1", "10.0.0.1", "169.254.169.254", "::1", "fd00::1", "::ffff:10.0.0.1"] {
        assert!(!forge::security::is_public_ip(ip.parse().unwrap()), "{ip}");
    }
    assert!(forge::security::is_public_ip("1.1.1.1".parse().unwrap()));
}

// ---- Medium: refs/pull is server-managed ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pushes_and_api_cannot_touch_pull_refs() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let n = open_pr(&s, &u, &rp).await;
    let before = s.state.git.refs(
        sqlx::query_scalar::<_, i64>("SELECT r.id FROM repositories r JOIN users o ON o.id = r.owner_id WHERE o.login = $1 AND r.name = $2")
            .bind(&u.login)
            .bind(&repo)
            .fetch_one(&s.state.db)
            .await
            .unwrap(),
    );
    let before = before.await.unwrap().1;
    let pr_head = before.get(&format!("refs/pull/{n}/head")).cloned().expect("pull ref");
    let main = before.get("refs/heads/main").cloned().unwrap();
    assert_ne!(pr_head, main);

    let w = s.work_dir(&unique("w"));
    let (ok, out) = s.git(&w, &["clone", "-q", &s.clone_url(&u, &repo), "c"]);
    assert!(ok, "{out}");
    let c = w.join("c");
    let (ok, _) = s.git(&c, &["push", "-q", "origin", &format!("+main:refs/pull/{n}/head")]);
    assert!(!ok, "push to refs/pull must be rejected");
    let (ok, _) = s.git(&c, &["push", "-q", "origin", "main:refs/pull/999/head"]);
    assert!(!ok, "creating refs/pull must be rejected");

    let (st, _) = s.call(Some(&u), "PATCH", &format!("{rp}/git/refs/pull/{n}/head"), Some(json!({"sha": main, "force": true}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/git/refs"), Some(json!({"ref": "refs/pull/77/head", "sha": main}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/git/refs/pull/{n}/head"), None).await;
    assert_eq!(st, 422);

    let (st, pr) = s.call(Some(&u), "GET", &format!("{rp}/git/ref/pull/{n}/head"), None).await;
    assert_eq!(st, 200);
    assert_eq!(pr["object"]["sha"].as_str().unwrap(), pr_head);
    // Normal pushes still work.
    let (ok, out) = s.git(&c, &["push", "-q", "origin", "main:refs/heads/copy"]);
    assert!(ok, "{out}");
}

// ---- Medium: login brute force and login CSRF ----

async fn login(s: &TestServer, login: &str, pw: &str, csrf: bool) -> u16 {
    let mut req = s.client().post(format!("{}/api/forge/session", s.base)).json(&json!({"login": login, "password": pw}));
    if csrf {
        req = req.header("X-Forge-CSRF", "1");
    }
    req.send().await.unwrap().status().as_u16()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_is_throttled_after_repeated_failures() {
    let s = TestServer::start().await;
    // The throttle lives in the (shared) database: start from a clean per-client counter so
    // earlier runs from 127.0.0.1 within the window don't count.
    sqlx::query("DELETE FROM login_throttle WHERE key LIKE 'c:%'").execute(&s.state.db).await.unwrap();
    let u = s.user().await;
    assert_eq!(login(&s, &u.login, "password123", true).await, 200);
    for _ in 0..10 {
        assert_eq!(login(&s, &u.login, "wrong-password", true).await, 401);
    }
    assert_eq!(login(&s, &u.login, "wrong-password", true).await, 429);
    // Even the right password is refused while throttled (no oracle for the attacker).
    assert_eq!(login(&s, &u.login, "password123", true).await, 429);
    // Basic auth with the password is throttled too.
    let res = s
        .client()
        .get(format!("{}/user", s.api))
        .basic_auth(&u.login, Some("password123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 429);
    // Tokens are unaffected.
    let (st, _) = s.call(Some(&u), "GET", "/user", None).await;
    assert_eq!(st, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forge_session_endpoints_require_csrf_header() {
    let s = TestServer::start().await;
    let u = s.user().await;
    // A cross-site form (text/plain body, no custom header) cannot log the victim in.
    let res = s
        .client()
        .post(format!("{}/api/forge/session", s.base))
        .header("Content-Type", "text/plain")
        .body(format!("{{\"login\":\"{}\",\"password\":\"password123\"}}", u.login))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 403);
    assert!(res.headers().get("set-cookie").is_none());
    let res = s.client().post(format!("{}/api/forge/signup", s.base)).json(&json!({"login": unique("x"), "password": "password123"})).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 403);
    assert_eq!(login(&s, &u.login, "password123", true).await, 200);
}

// ---- Medium: activity feeds ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn draft_releases_and_private_repos_stay_out_of_public_events() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v9", "name": "unannounced-draft", "draft": true}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v1", "name": "soon-private-release"}))).await;
    assert_eq!(st, 201);
    let feed = format!("/users/{}/events/public", u.login);
    let (_, ev) = s.call(None, "GET", &feed, None).await;
    let text = ev.to_string();
    assert!(!text.contains("unannounced-draft"), "draft leaked: {text}");
    assert!(text.contains("soon-private-release"), "{text}");
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"private": true}))).await;
    assert_eq!(st, 200);
    for path in [feed, format!("/users/{}/events", u.login), "/events?per_page=100".to_string()] {
        let (_, ev) = s.call(None, "GET", &path, None).await;
        assert!(!ev.to_string().contains("soon-private-release"), "{path}: {ev}");
    }
}

// ---- Medium: archived repositories are read-only ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archived_repositories_are_read_only_over_rest() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let stranger = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"archived": true}))).await;
    assert_eq!(st, 200);
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/issues"), Some(json!({"title": "x"}))).await;
    assert_eq!(st, 403, "{e}");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/labels"), Some(json!({"name": "x", "color": "ffffff"}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/issues"), None).await;
    assert_eq!(st, 200);
    // Unarchiving is allowed.
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"archived": false}))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/issues"), Some(json!({"title": "x"}))).await;
    assert_eq!(st, 201);
    // A private archived repo still answers 404 to strangers (no existence oracle).
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"archived": true, "private": true}))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&stranger), "POST", &format!("{rp}/issues"), Some(json!({"title": "x"}))).await;
    assert_eq!(st, 404);
}

// ---- Low: user content served with hardening headers ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_content_is_not_rendered_by_browsers() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    // <script>alert(1)</script>
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/x.html"), Some(json!({"message": "x", "content": "PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pgo="}))).await;
    assert_eq!(st, 201);
    let res = s.client().get(format!("{}/{}/{}/raw/main/x.html", s.base, u.login, repo)).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(res.headers()["content-security-policy"].to_str().unwrap().contains("sandbox"));
    assert!(!res.headers()["content-type"].to_str().unwrap().contains("html"));
    let res = s
        .client()
        .get(format!("{}{rp}/contents/x.html", s.api))
        .header("Accept", "application/vnd.github.raw")
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(res.headers()["content-security-policy"].to_str().unwrap().contains("sandbox"));

    // Release asset uploaded as text/html is still an attachment with nosniff/CSP.
    let (st, rel) = s.call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v1"}))).await;
    assert_eq!(st, 201);
    let up = format!("{}/api/uploads{rp}/releases/{}/assets?name=a.html", s.base, rel["id"]);
    let res = s
        .client()
        .post(up)
        .header("Authorization", format!("token {}", u.token))
        .header("Content-Type", "text/html")
        .body("<script>alert(1)</script>")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 201);
    let a: Value = res.json().await.unwrap();
    let res = s.client().get(a["browser_download_url"].as_str().unwrap()).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert!(res.headers()["content-disposition"].to_str().unwrap().starts_with("attachment"));
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(res.headers()["content-security-policy"].to_str().unwrap().contains("sandbox"));

    // Gist raw.
    let (st, g) = s.call(Some(&u), "POST", "/gists", Some(json!({"public": true, "files": {"a.html": {"content": "<script>alert(1)</script>"}}}))).await;
    assert_eq!(st, 201, "{g}");
    let raw = g["files"]["a.html"]["raw_url"].as_str().unwrap();
    let res = s.client().get(raw).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(res.headers()["content-security-policy"].to_str().unwrap().contains("sandbox"));
    // List views report sizes without loading contents.
    let (_, list) = s.call(Some(&u), "GET", "/gists", None).await;
    let mine = list.as_array().unwrap().iter().find(|x| x["id"] == g["id"]).unwrap();
    assert_eq!(mine["files"]["a.html"]["size"], 25);
}

// ---- Low: tree paths ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tree_and_contents_paths_reject_dot_git_and_control_chars() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    for path in [".GIT/config", "a/.Git/hooks/post-checkout", "a\u{0}100644 blob 0000000000000000000000000000000000000000\t.git"] {
        let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/git/trees"), Some(json!({"tree": [{"path": path, "mode": "100644", "type": "blob", "content": "x"}]}))).await;
        assert_eq!(st, 422, "{path}: {e}");
    }
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/.GIT/config"), Some(json!({"message": "x", "content": "eA=="}))).await;
    assert_eq!(st, 422);
}

// ---- Low: resource bounds / misc ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn misc_bounds() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    // Huge page numbers don't overflow into a 500.
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/issues?page=9223372036854775807"), None).await;
    assert_eq!(st, 200);
    // nodes(ids:) is capped at 100.
    let ids: Vec<String> = (0..101).map(|i| forge::ids::encode("Repository", i)).collect();
    let v = gql(&s, &u, "query($ids:[ID!]!){nodes(ids:$ids){id}}", json!({"ids": ids})).await;
    assert!(v.get("errors").is_some(), "{v}");
    // At most 20 hooks per repository.
    for i in 0..20 {
        let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/hooks"), Some(json!({"config": {"url": format!("https://example.com/{i}")}}))).await;
        assert_eq!(st, 201);
    }
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/hooks"), Some(json!({"config": {"url": "https://example.com/x"}}))).await;
    assert_eq!(st, 422);
    // default_branch must be a branch name (it is written into HEAD).
    let (st, _) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": unique("r"), "default_branch": "a\nb"}))).await;
    assert_eq!(st, 422);
}

/// Creates an organization administered by `u` (forge-only endpoint for normal users).
async fn create_org(s: &TestServer, u: &TestUser, login: &str) -> u16 {
    let res = s
        .client()
        .post(format!("{}/api/forge/orgs", s.base))
        .header("Authorization", format!("token {}", u.token))
        .json(&json!({"login": login}))
        .send()
        .await
        .unwrap();
    res.status().as_u16()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_parent_from_other_org_is_not_exposed() {
    let s = TestServer::start().await;
    let alice = s.user().await;
    let mallory = s.user().await;
    let (o1, o2) = (unique("org"), unique("org"));
    assert_eq!(create_org(&s, &alice, &o1).await, 201);
    let (st, t) = s.call(Some(&alice), "POST", &format!("/orgs/{o1}/teams"), Some(json!({"name": "secret-team-name", "privacy": "secret"}))).await;
    assert_eq!(st, 201, "{t}");
    assert_eq!(create_org(&s, &mallory, &o2).await, 201);
    let (_, t2) = s.call(Some(&mallory), "POST", &format!("/orgs/{o2}/teams"), Some(json!({"name": "x", "parent_team_id": t["id"]}))).await;
    assert!(!t2.to_string().contains("secret-team-name"), "{t2}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clone_template_requires_a_template() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let (_, r) = s.call(Some(&owner), "GET", &format!("/repos/{}/{}", owner.login, repo), None).await;
    let (_, me) = s.call(Some(&other), "GET", "/user", None).await;
    let v = gql(
        &s,
        &other,
        "mutation($r:ID!,$o:ID!,$n:String!){cloneTemplateRepository(input:{repositoryId:$r,ownerId:$o,name:$n,visibility:PRIVATE}){repository{name}}}",
        json!({"r": r["node_id"], "o": me["node_id"], "n": unique("copy")}),
    )
    .await;
    assert!(v.get("errors").is_some(), "{v}");
}
