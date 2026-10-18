//! Regression tests for the second round of security fixes (docs/security.md, "Fixed" #18-#23).
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

/// Creates branch `branch` with file `name` (base64 `content`) and opens a PR into `main`.
async fn open_pr_with(s: &TestServer, u: &TestUser, rp: &str, branch: &str, name: &str, content: &str) -> Value {
    let (st, r) = s.call(Some(u), "GET", &format!("{rp}/git/ref/heads/main"), None).await;
    assert_eq!(st, 200, "{r}");
    let sha = r["object"]["sha"].as_str().unwrap().to_string();
    let (st, r) = s.call(Some(u), "POST", &format!("{rp}/git/refs"), Some(json!({"ref": format!("refs/heads/{branch}"), "sha": sha}))).await;
    assert_eq!(st, 201, "{r}");
    let (st, r) = s
        .call(Some(u), "PUT", &format!("{rp}/contents/{name}"), Some(json!({"message": "f", "content": content, "branch": branch})))
        .await;
    assert_eq!(st, 201, "{r}");
    let (st, pr) = s.call(Some(u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "t", "head": branch, "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    pr
}

async fn open_pr(s: &TestServer, u: &TestUser, rp: &str) -> Value {
    open_pr_with(s, u, rp, "feature", "f.txt", "aGVsbG8K").await
}

async fn site_admin(s: &TestServer) -> TestUser {
    let login = unique("adm");
    let u = forge::forge_api::create_user(&s.state, &login, Some(&format!("{login}@example.com")), "password123", true)
        .await
        .map_err(|e| e.message)
        .unwrap();
    let (_, token) = forge::forge_api::create_token_for(&s.state, u.id, "test").await.map_err(|e| e.message).unwrap();
    TestUser { login, token }
}

fn gql_error_type(v: &Value) -> String {
    v["errors"][0]["type"].as_str().unwrap_or_default().to_string()
}

// ---- #18 GraphQL mutations on archived repositories ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graphql_mutations_on_archived_repositories_are_rejected() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let (_, r) = s.call(Some(&u), "GET", &rp, None).await;
    let repo_id = r["node_id"].as_str().unwrap().to_string();
    let (st, issue) = s.call(Some(&u), "POST", &format!("{rp}/issues"), Some(json!({"title": "bug"}))).await;
    assert_eq!(st, 201);
    let issue_id = issue["node_id"].as_str().unwrap().to_string();
    let (st, label) = s.call(Some(&u), "POST", &format!("{rp}/labels"), Some(json!({"name": "l1", "color": "ffffff"}))).await;
    assert_eq!(st, 201, "{label}");
    let pr = open_pr(&s, &u, &rp).await;
    let pr_id = pr["node_id"].as_str().unwrap().to_string();

    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"archived": true}))).await;
    assert_eq!(st, 200);

    let add_comment = "mutation($id:ID!){addComment(input:{subjectId:$id, body:\"x\"}){clientMutationId}}";
    let cases: Vec<(&str, Value)> = vec![
        (add_comment, json!({"id": issue_id})),
        ("mutation($r:ID!){createIssue(input:{repositoryId:$r, title:\"t\"}){issue{number}}}", json!({"r": repo_id})),
        ("mutation($id:ID!){addReaction(input:{subjectId:$id, content:HEART}){clientMutationId}}", json!({"id": issue_id})),
        ("mutation($id:ID!,$l:[ID!]!){addLabelsToLabelable(input:{labelableId:$id, labelIds:$l}){clientMutationId}}", json!({"id": issue_id, "l": [label["node_id"]]})),
        ("mutation($id:ID!){addPullRequestReview(input:{pullRequestId:$id, event:COMMENT, body:\"r\"}){clientMutationId}}", json!({"id": pr_id})),
        ("mutation($id:ID!){closePullRequest(input:{pullRequestId:$id}){clientMutationId}}", json!({"id": pr_id})),
    ];
    for (q, vars) in &cases {
        let v = gql(&s, &u, q, vars.clone()).await;
        assert_eq!(gql_error_type(&v), "UNPROCESSABLE", "{q}: {v}");
        assert_eq!(v["errors"][0]["message"], "Repository was archived so is read-only.", "{v}");
        let field = v["errors"][0]["path"][0].as_str().unwrap().to_string();
        assert!(v["data"][&field].is_null(), "{v}");
    }
    // Inline literal ids are covered too (no variables).
    let v = gql(&s, &u, &format!("mutation{{addComment(input:{{subjectId:\"{issue_id}\", body:\"x\"}}){{clientMutationId}}}}"), json!({})).await;
    assert_eq!(gql_error_type(&v), "UNPROCESSABLE", "{v}");
    // Nothing was written.
    let (_, comments) = s.call(Some(&u), "GET", &format!("{rp}/issues/{}/comments", issue["number"]), None).await;
    assert_eq!(comments.as_array().unwrap().len(), 0);
    let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{}", pr["number"]), None).await;
    assert_eq!(p["state"], "open");

    // A private archived repository is not revealed to outsiders: they get the normal "not found".
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"private": true}))).await;
    assert_eq!(st, 200);
    let v = gql(&s, &other, add_comment, json!({"id": issue_id})).await;
    assert_ne!(gql_error_type(&v), "UNPROCESSABLE", "{v}");
    assert!(!v.to_string().contains("archived"), "{v}");
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"private": false}))).await;
    assert_eq!(st, 200);

    // Unarchiving is allowed, and then writes work again.
    let v = gql(&s, &u, "mutation($r:ID!){unarchiveRepository(input:{repositoryId:$r}){repository{isArchived}}}", json!({"r": repo_id})).await;
    assert_eq!(v["data"]["unarchiveRepository"]["repository"]["isArchived"], false, "{v}");
    let v = gql(&s, &u, add_comment, json!({"id": issue_id})).await;
    assert!(v.get("errors").is_none(), "{v}");
}

// ---- #19 REST/GraphQL permission deviations ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn triage_cannot_edit_other_users_pull_requests() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let t = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/collaborators/{}", t.login), Some(json!({"permission": "triage"}))).await;
    assert_eq!(st, 204);
    let pr = open_pr(&s, &u, &rp).await;
    let n = pr["number"].as_i64().unwrap();

    for body in [json!({"title": "hijacked"}), json!({"body": "hijacked"}), json!({"base": "feature"})] {
        let (st, e) = s.call(Some(&t), "PATCH", &format!("{rp}/pulls/{n}"), Some(body.clone())).await;
        assert_eq!(st, 403, "{body}: {e}");
    }
    // Through the issues endpoint too.
    let (st, _) = s.call(Some(&t), "PATCH", &format!("{rp}/issues/{n}"), Some(json!({"title": "hijacked"}))).await;
    assert_eq!(st, 403);
    // GraphQL updatePullRequest goes through the same rule.
    let v = gql(&s, &t, "mutation($id:ID!){updatePullRequest(input:{pullRequestId:$id, title:\"hijacked\"}){clientMutationId}}", json!({"id": pr["node_id"]})).await;
    assert!(v.get("errors").is_some(), "{v}");
    let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{n}"), None).await;
    assert_eq!(p["title"], "t");
    // Triage may still close and reopen, and resending unchanged fields is fine.
    let (st, _) = s.call(Some(&t), "PATCH", &format!("{rp}/pulls/{n}"), Some(json!({"state": "closed", "title": "t"}))).await;
    assert_eq!(st, 200);
    let (st, p) = s.call(Some(&t), "PATCH", &format!("{rp}/pulls/{n}"), Some(json!({"state": "open"}))).await;
    assert_eq!(st, 200);
    assert_eq!(p["state"], "open");
    // The author (here the owner) can edit.
    let (st, p) = s.call(Some(&u), "PATCH", &format!("{rp}/pulls/{n}"), Some(json!({"title": "renamed"}))).await;
    assert_eq!(st, 200);
    assert_eq!(p["title"], "renamed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_author_can_delete_a_reaction() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let fan = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let rp = format!("/repos/{}/{}", owner.login, repo);
    let (_, issue) = s.call(Some(&owner), "POST", &format!("{rp}/issues"), Some(json!({"title": "bug"}))).await;
    let n = issue["number"].as_i64().unwrap();
    let (st, r) = s.call(Some(&fan), "POST", &format!("{rp}/issues/{n}/reactions"), Some(json!({"content": "heart"}))).await;
    assert_eq!(st, 201, "{r}");
    let rid = r["id"].as_i64().unwrap();
    // The repository admin can't remove someone else's reaction.
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("{rp}/issues/{n}/reactions/{rid}"), None).await;
    assert_eq!(st, 403);
    let (st, c) = s.call(Some(&owner), "POST", &format!("{rp}/issues/{n}/comments"), Some(json!({"body": "c"}))).await;
    assert_eq!(st, 201);
    let cid = c["id"].as_i64().unwrap();
    let (_, cr) = s.call(Some(&fan), "POST", &format!("{rp}/issues/comments/{cid}/reactions"), Some(json!({"content": "+1"}))).await;
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("{rp}/issues/comments/{cid}/reactions/{}", cr["id"]), None).await;
    assert_eq!(st, 403);
    // The author can.
    let (st, _) = s.call(Some(&fan), "DELETE", &format!("{rp}/issues/{n}/reactions/{rid}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&fan), "DELETE", &format!("{rp}/issues/comments/{cid}/reactions/{}", cr["id"]), None).await;
    assert_eq!(st, 204);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issues_endpoint_applies_pull_request_rules() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let pr = open_pr(&s, &u, &rp).await;
    let n = pr["number"].as_i64().unwrap();
    // Closing a PR through the issues endpoint closes the pull request.
    let (st, i) = s.call(Some(&u), "PATCH", &format!("{rp}/issues/{n}"), Some(json!({"state": "closed"}))).await;
    assert_eq!(st, 200, "{i}");
    assert_eq!(i["state"], "closed");
    let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{n}"), None).await;
    assert_eq!(p["state"], "closed");
    let (st, i) = s.call(Some(&u), "PATCH", &format!("{rp}/issues/{n}"), Some(json!({"state": "open"}))).await;
    assert_eq!(st, 200, "{i}");
    assert_eq!(i["state"], "open");
    // A merged pull request can't be reopened through the issues endpoint.
    let (st, m) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/{n}/merge"), Some(json!({}))).await;
    assert_eq!(st, 200, "{m}");
    let (st, e) = s.call(Some(&u), "PATCH", &format!("{rp}/issues/{n}"), Some(json!({"state": "open"}))).await;
    assert_eq!(st, 422, "{e}");
    let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{n}"), None).await;
    assert_eq!(p["state"], "closed");
    assert_eq!(p["merged"], true);
    let v = gql(&s, &u, "mutation($id:ID!){reopenIssue(input:{issueId:$id}){clientMutationId}}", json!({"id": pr["node_id"]})).await;
    let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{n}"), None).await;
    assert_eq!(p["state"], "closed", "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locked_conversations_block_reactions_and_reviews() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let pr = open_pr(&s, &u, &rp).await;
    let n = pr["number"].as_i64().unwrap();
    let (st, c) = s.call(Some(&u), "POST", &format!("{rp}/issues/{n}/comments"), Some(json!({"body": "c"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/issues/{n}/lock"), Some(json!({"lock_reason": "resolved"}))).await;
    assert_eq!(st, 204);

    let v = gql(&s, &other, "mutation($id:ID!){addReaction(input:{subjectId:$id, content:HEART}){clientMutationId}}", json!({"id": pr["node_id"]})).await;
    assert_eq!(gql_error_type(&v), "FORBIDDEN", "{v}");
    let v = gql(&s, &other, "mutation($id:ID!){addReaction(input:{subjectId:$id, content:HEART}){clientMutationId}}", json!({"id": c["node_id"]})).await;
    assert_eq!(gql_error_type(&v), "FORBIDDEN", "{v}");
    let (st, _) = s.call(Some(&other), "POST", &format!("{rp}/issues/comments/{}/reactions", c["id"]), Some(json!({"content": "heart"}))).await;
    assert_eq!(st, 403);
    let (st, e) = s.call(Some(&other), "POST", &format!("{rp}/pulls/{n}/reviews"), Some(json!({"body": "x", "event": "COMMENT"}))).await;
    assert_eq!(st, 403, "{e}");
    let v = gql(&s, &other, "mutation($id:ID!){addPullRequestReview(input:{pullRequestId:$id, event:COMMENT, body:\"r\"}){clientMutationId}}", json!({"id": pr["node_id"]})).await;
    assert_eq!(gql_error_type(&v), "FORBIDDEN", "{v}");
    let (st, e) = s
        .call(
            Some(&other),
            "POST",
            &format!("{rp}/pulls/{n}/comments"),
            Some(json!({"body": "x", "commit_id": pr["head"]["sha"], "path": "f.txt", "line": 1, "side": "RIGHT"})),
        )
        .await;
    assert_eq!(st, 403, "{e}");
    // Users with Write still can.
    let v = gql(&s, &u, "mutation($id:ID!){addReaction(input:{subjectId:$id, content:HEART}){clientMutationId}}", json!({"id": pr["node_id"]})).await;
    assert!(v.get("errors").is_none(), "{v}");
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/pulls/{n}/reviews"), Some(json!({"body": "x", "event": "COMMENT"}))).await;
    assert_eq!(st, 200, "{e}");
}

// ---- #20 organization creation ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_organizations_is_for_site_admins() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let victim = s.user().await;
    let admin = site_admin(&s).await;
    let (st, _) = s.call(Some(&u), "POST", "/admin/organizations", Some(json!({"login": unique("org"), "admin": victim.login}))).await;
    assert_eq!(st, 403);
    let org = unique("org");
    let (st, o) = s.call(Some(&admin), "POST", "/admin/organizations", Some(json!({"login": org, "admin": u.login}))).await;
    assert_eq!(st, 201, "{o}");
    let (st, m) = s.call(Some(&u), "GET", &format!("/user/memberships/orgs/{org}"), None).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["role"], "admin");

    // Normal users create organizations through the forge-only endpoint, as their own admin.
    let mine = unique("org");
    let res = s
        .client()
        .post(format!("{}/api/forge/orgs", s.base))
        .header("Authorization", format!("token {}", u.token))
        .json(&json!({"login": mine, "admin": victim.login}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 201);
    let (st, m) = s.call(Some(&u), "GET", &format!("/user/memberships/orgs/{mine}"), None).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["role"], "admin");
    let (st, _) = s.call(Some(&victim), "GET", &format!("/user/memberships/orgs/{mine}"), None).await;
    assert_eq!(st, 404);
    // Without a session/token it is refused.
    let res = s.client().post(format!("{}/api/forge/orgs", s.base)).header("X-Forge-CSRF", "1").json(&json!({"login": unique("org")})).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 401);
}

// ---- #21 login throttle shared by replicas ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_throttle_is_shared_between_replicas() {
    // Two servers on the same database stand in for two pods.
    let a = TestServer::start().await;
    let b = TestServer::start().await;
    sqlx::query("DELETE FROM login_throttle WHERE key LIKE 'c:%'").execute(&a.state.db).await.unwrap();
    let u = a.user().await;
    let login = |s: &TestServer, pw: &'static str| {
        let req = s.client().post(format!("{}/api/forge/session", s.base)).header("X-Forge-CSRF", "1").json(&json!({"login": u.login, "password": pw}));
        async move { req.send().await.unwrap().status().as_u16() }
    };
    for i in 0..10 {
        let s = if i % 2 == 0 { &a } else { &b };
        assert_eq!(login(s, "wrong-password").await, 401);
    }
    assert_eq!(login(&a, "password123").await, 429);
    assert_eq!(login(&b, "password123").await, 429);
    // The counter is one row per key.
    let n: i32 = sqlx::query_scalar("SELECT count FROM login_throttle WHERE key LIKE $1")
        .bind(format!("l:{}|%", u.login.to_lowercase()))
        .fetch_one(&a.state.db)
        .await
        .unwrap();
    assert_eq!(n, 10);
    // An expired window starts over.
    sqlx::query("UPDATE login_throttle SET window_start = now() - interval '16 minutes' WHERE key LIKE $1")
        .bind(format!("l:{}|%", u.login.to_lowercase()))
        .execute(&a.state.db)
        .await
        .unwrap();
    assert_eq!(login(&b, "password123").await, 200);
}

// ---- #22 free-text issue search ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unscoped_issue_search_keeps_semantics() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let w = unique("zq");
    let (_, a) = s.call(Some(&u), "POST", &format!("{rp}/issues"), Some(json!({"title": format!("title {w}xx")}))).await;
    let (_, b) = s.call(Some(&u), "POST", &format!("{rp}/issues"), Some(json!({"title": "other", "body": format!("body has {w}")}))).await;
    let (_, c) = s.call(Some(&u), "POST", &format!("{rp}/issues"), Some(json!({"title": "third"}))).await;
    s.call(Some(&u), "POST", &format!("{rp}/issues/{}/comments", c["number"]), Some(json!({"body": format!("comment {w}")}))).await;
    let numbers = |v: &Value| -> Vec<i64> {
        let mut n: Vec<i64> = v["items"].as_array().unwrap().iter().map(|i| i["number"].as_i64().unwrap()).collect();
        n.sort();
        n
    };
    // Partial words in title and body, no repo qualifier.
    let (st, v) = s.call(Some(&u), "GET", &format!("/search/issues?q={w}"), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(numbers(&v), vec![a["number"].as_i64().unwrap(), b["number"].as_i64().unwrap()], "{v}");
    assert_eq!(v["total_count"], 2);
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/issues?q={w}+in:title"), None).await;
    assert_eq!(numbers(&v), vec![a["number"].as_i64().unwrap()]);
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/issues?q={w}+in:comments"), None).await;
    assert_eq!(numbers(&v), vec![c["number"].as_i64().unwrap()]);
    // Same results with a repo qualifier (the scoped query form).
    let (_, v) = s.call(Some(&u), "GET", &format!("/search/issues?q={w}+repo:{}/{repo}", u.login), None).await;
    assert_eq!(v["total_count"], 2);
    // Private repositories stay invisible to others.
    let other = s.user().await;
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"private": true}))).await;
    assert_eq!(st, 200);
    let (_, v) = s.call(Some(&other), "GET", &format!("/search/issues?q={w}"), None).await;
    assert_eq!(v["total_count"], 0, "{v}");
    // The plan uses the trigram index for the instance-wide arm.
    let mut tx = s.state.db.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off").execute(&mut *tx).await.unwrap();
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT id FROM issues WHERE title ILIKE $1")
        .bind(format!("%{w}%"))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(plan.join("\n").contains("issues_title_trgm"), "{plan:?}");
}

// ---- #23 diff output cap ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diff_output_is_capped() {
    use base64::Engine;
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let big: String = (0..400).map(|i| format!("line {i} of a long file\n")).collect();
    let content = base64::engine::general_purpose::STANDARD.encode(big.as_bytes());
    let pr = open_pr_with(&s, &u, &rp, "big", "big.txt", &content).await;
    let n = pr["number"].as_i64().unwrap();
    let head = pr["head"]["sha"].as_str().unwrap().to_string();
    let get = |path: String, accept: &'static str| {
        let req = s.client().get(format!("{}{path}", s.api)).header("Authorization", format!("token {}", u.token)).header("Accept", accept);
        async move {
            let r = req.send().await.unwrap();
            let st = r.status().as_u16();
            (st, r.bytes().await.unwrap().to_vec())
        }
    };
    // Under the default cap, all forms render.
    let (st, d) = get(format!("{rp}/pulls/{n}"), "application/vnd.github.diff").await;
    assert_eq!(st, 200);
    assert!(d.len() > 8000);

    forge::security::set_max_diff_bytes(2000);
    for (path, accept) in [
        (format!("{rp}/pulls/{n}"), "application/vnd.github.diff"),
        (format!("{rp}/pulls/{n}"), "application/vnd.github.patch"),
        (format!("{rp}/commits/{head}"), "application/vnd.github.diff"),
        (format!("{rp}/commits/{head}"), "application/vnd.github.patch"),
        (format!("{rp}/compare/main...big"), "application/vnd.github.diff"),
        (format!("{rp}/compare/main...big.patch"), "application/json"),
    ] {
        let (st, body) = get(path.clone(), accept).await;
        assert_eq!(st, 406, "{path} {accept}: {}", String::from_utf8_lossy(&body));
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["errors"][0]["code"], "too_large", "{v}");
    }
    // JSON views stay available; the oversized patch is omitted.
    let (st, body) = get(format!("{rp}/commits/{head}"), "application/vnd.github+json").await;
    assert_eq!(st, 200);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["files"][0]["filename"], "big.txt", "{v}");
    assert!(v["files"][0].get("patch").is_none_or(|p| p.is_null()), "{v}");
    let (st, _) = get(format!("{rp}/pulls/{n}/files"), "application/vnd.github+json").await;
    assert_eq!(st, 200);
    forge::security::set_max_diff_bytes(0);
}
