//! gh compatibility: autolinks, rulesets (REST + GraphQL), SSH signing keys, Repository.object
//! trees (`gh repo read-dir`), gists via GraphQL (`gh gist list`), POST-as-PATCH gist edits and
//! read-only git clone of gists.
mod common;

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn gql(s: &TestServer, u: &TestUser, query: &str, vars: Value) -> Value {
    let res = s
        .client()
        .post(format!("{}/api/graphql", s.base))
        .header("Authorization", format!("token {}", u.token))
        .json(&json!({ "query": query, "variables": vars }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    res.json().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn autolinks_crud_and_permissions() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let p = format!("/repos/{}/{}/autolinks", u.login, repo);

    let (st, a) = s.call(Some(&u), "POST", &p, Some(json!({"key_prefix": "TICKET-", "url_template": "https://t.example/<num>"}))).await;
    assert_eq!(st, 201, "{a}");
    assert_eq!(a["is_alphanumeric"], true);
    let id = a["id"].as_i64().unwrap();
    let (st, e) = s.call(Some(&u), "POST", &p, Some(json!({"key_prefix": "ticket-", "url_template": "https://t.example/<num>"}))).await;
    assert_eq!(st, 422, "{e}");
    let (st, _) = s.call(Some(&u), "POST", &p, Some(json!({"key_prefix": "X-", "url_template": "https://t.example/"}))).await;
    assert_eq!(st, 422);
    let (st, list) = s.call(Some(&u), "GET", &p, None).await;
    assert_eq!(st, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (st, one) = s.call(Some(&u), "GET", &format!("{p}/{id}"), None).await;
    assert_eq!((st, one["key_prefix"].as_str()), (200, Some("TICKET-")));
    // Not an admin: 404 like GitHub.
    let (st, _) = s.call(Some(&other), "GET", &p, None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{p}/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/{id}"), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rulesets_rest_graphql_and_branch_rules() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let (st, r) = s
        .call(
            Some(&u),
            "POST",
            &format!("{rp}/rulesets"),
            Some(json!({"name": "protect", "target": "branch", "enforcement": "active",
                "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH", "refs/heads/release/*"], "exclude": []}},
                "rules": [{"type": "deletion"}, {"type": "non_fast_forward"}]})),
        )
        .await;
    assert_eq!(st, 201, "{r}");
    let id = r["id"].as_i64().unwrap();
    assert_eq!(r["source_type"], "Repository");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/rulesets"), Some(json!({"name": "bad", "enforcement": "sometimes"}))).await;
    assert_eq!(st, 422);

    let (st, rules) = s.call(Some(&u), "GET", &format!("{rp}/rules/branches/main"), None).await;
    assert_eq!(st, 200);
    assert_eq!(rules.as_array().unwrap().len(), 2, "{rules}");
    assert_eq!(rules[0]["ruleset_id"], id);
    let (_, rules) = s.call(Some(&u), "GET", &format!("{rp}/rules/branches/release/1.0"), None).await;
    assert_eq!(rules.as_array().unwrap().len(), 2);
    let (_, rules) = s.call(Some(&u), "GET", &format!("{rp}/rules/branches/feature"), None).await;
    assert_eq!(rules.as_array().unwrap().len(), 0);

    let (st, upd) = s.call(Some(&u), "PUT", &format!("{rp}/rulesets/{id}"), Some(json!({"enforcement": "disabled"}))).await;
    assert_eq!((st, upd["enforcement"].as_str(), upd["name"].as_str()), (200, Some("disabled"), Some("protect")));
    let (_, rules) = s.call(Some(&u), "GET", &format!("{rp}/rules/branches/main"), None).await;
    assert_eq!(rules.as_array().unwrap().len(), 0);

    let v = gql(
        &s,
        &u,
        "query($o: String!, $n: String!) { level: repository(owner: $o, name: $n) { rulesets(first: 10, includeParents: true) {
            totalCount nodes { databaseId name target enforcement source { __typename ... on Repository { owner: nameWithOwner } } rules { totalCount } } } } }",
        json!({"o": u.login, "n": repo}),
    )
    .await;
    let node = &v["data"]["level"]["rulesets"]["nodes"][0];
    assert_eq!(node["databaseId"], id, "{v}");
    assert_eq!(node["target"], "BRANCH");
    assert_eq!(node["enforcement"], "DISABLED");
    assert_eq!(node["source"]["__typename"], "Repository");
    assert_eq!(node["rules"]["totalCount"], 2);

    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/rulesets/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, list) = s.call(Some(&u), "GET", &format!("{rp}/rulesets"), None).await;
    assert_eq!((st, list.as_array().unwrap().len()), (200, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_signing_keys() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let w = s.work_dir(&unique("k"));
    let out = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"]).arg(w.join("id")).output().unwrap();
    assert!(out.status.success());
    let full = std::fs::read_to_string(w.join("id.pub")).unwrap();
    let key = full.trim();
    let bare = key.rsplit_once(' ').unwrap().0.to_string();
    // The same key may be an authentication key and a signing key.
    let (st, _) = s.call(Some(&u), "POST", "/user/keys", Some(json!({"title": "auth", "key": key}))).await;
    assert_eq!(st, 201);
    let (st, k) = s.call(Some(&u), "POST", "/user/ssh_signing_keys", Some(json!({"title": "sign", "key": key}))).await;
    assert_eq!(st, 201, "{k}");
    assert_eq!(k["key"], bare.as_str());
    let (st, _) = s.call(Some(&u), "POST", "/user/ssh_signing_keys", Some(json!({"title": "again", "key": key}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", "/user/ssh_signing_keys", Some(json!({"key": "nonsense"}))).await;
    assert_eq!(st, 422);
    let (_, list) = s.call(None, "GET", &format!("/users/{}/ssh_signing_keys", u.login), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let id = k["id"].as_i64().unwrap();
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/user/ssh_signing_keys/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("/user/ssh_signing_keys/{id}"), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repository_object_tree_and_blob() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let (st, _) = s
        .call(Some(&u), "PUT", &format!("/repos/{}/{}/contents/docs/guide.md", u.login, repo), Some(json!({"message": "doc", "content": "IyBHdWlkZQo="})))
        .await;
    assert_eq!(st, 201);
    let q = "query($o: String!, $n: String!, $e: String!) { repository(owner: $o, name: $n) { object(expression: $e) {
        __typename ... on Tree { oid entries { name path type mode size } } ... on Blob { text byteSize } } } }";
    let v = gql(&s, &u, q, json!({"o": u.login, "n": repo, "e": "HEAD:"})).await;
    let obj = &v["data"]["repository"]["object"];
    assert_eq!(obj["__typename"], "Tree", "{v}");
    let names: Vec<&str> = obj["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"docs") && names.contains(&"README.md"), "{names:?}");
    let v = gql(&s, &u, q, json!({"o": u.login, "n": repo, "e": "HEAD:docs"})).await;
    let e = &v["data"]["repository"]["object"]["entries"][0];
    assert_eq!((e["path"].as_str(), e["type"].as_str(), e["mode"].as_i64()), (Some("docs/guide.md"), Some("blob"), Some(0o100644)));
    let v = gql(&s, &u, q, json!({"o": u.login, "n": repo, "e": "main:docs/guide.md"})).await;
    assert_eq!(v["data"]["repository"]["object"]["text"], "# Guide\n", "{v}");
    let v = gql(&s, &u, q, json!({"o": u.login, "n": repo, "e": "HEAD:missing"})).await;
    assert!(v["data"]["repository"]["object"].is_null(), "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gists_graphql_post_edit_and_git_clone() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let (st, g) = s
        .call(Some(&u), "POST", "/gists", Some(json!({"description": "secret one", "public": false, "files": {"a.txt": {"content": "one\n"}}})))
        .await;
    assert_eq!(st, 201, "{g}");
    let id = g["id"].as_str().unwrap().to_string();
    let (st, _) = s.call(Some(&u), "POST", "/gists", Some(json!({"public": true, "files": {"b.txt": {"content": "two\n"}}}))).await;
    assert_eq!(st, 201);

    // gh gist edit/rename send POST.
    let (st, e) = s.call(Some(&u), "POST", &format!("/gists/{id}"), Some(json!({"files": {"a.txt": {"filename": "renamed.txt", "content": "one more\n"}}}))).await;
    assert_eq!(st, 200, "{e}");
    assert!(e["files"]["renamed.txt"].is_object(), "{e}");

    let q = "query($p: GistPrivacy) { viewer { gists(first: 10, privacy: $p, orderBy: {field: CREATED_AT, direction: DESC}) {
        nodes { name description isPublic files { name text } } pageInfo { hasNextPage } } } }";
    let v = gql(&s, &u, q, json!({"p": "ALL"})).await;
    let nodes = v["data"]["viewer"]["gists"]["nodes"].as_array().unwrap().clone();
    assert_eq!(nodes.len(), 2, "{v}");
    assert_eq!(nodes[1]["name"], id.as_str());
    assert_eq!(nodes[1]["files"][0]["text"], "one more\n");
    let v = gql(&s, &u, q, json!({"p": "SECRET"})).await;
    assert_eq!(v["data"]["viewer"]["gists"]["nodes"].as_array().unwrap().len(), 1);
    // Someone else's secret gists stay hidden.
    let v = gql(&s, &other, &format!("{{ user(login: \"{}\") {{ gists(first: 10, privacy: ALL) {{ totalCount }} }} }}", u.login), json!({})).await;
    assert_eq!(v["data"]["user"]["gists"]["totalCount"], 1, "{v}");

    // git clone of the gist: two commits (create, edit), current files.
    let w = s.work_dir(&unique("g"));
    let url = format!("{}/gist/{id}.git", s.base);
    let (ok, out) = s.git(&w, &["clone", "-q", &url, "c"]);
    assert!(ok, "{out}");
    assert_eq!(std::fs::read_to_string(w.join("c/renamed.txt")).unwrap(), "one more\n");
    assert_eq!(s.git(&w.join("c"), &["rev-list", "--count", "HEAD"]).1.trim(), "2");
    // Deterministic: a rebuild yields the same head.
    let head = s.git(&w.join("c"), &["rev-parse", "HEAD"]).1;
    std::fs::remove_dir_all(s.state.cfg.cache_dir.join("gists")).unwrap();
    let (ok, _) = s.git(&w, &["clone", "-q", &url, "d"]);
    assert!(ok);
    assert_eq!(s.git(&w.join("d"), &["rev-parse", "HEAD"]).1, head);
    let (ok, _) = s.git(&w, &["clone", "-q", &format!("{}/gist/nope.git", s.base), "e"]);
    assert!(!ok);
}
