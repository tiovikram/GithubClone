//! gh-compatibility additions: GraphQL mutations behind gh repo/issue/pr commands, the unknown
//! mutation guard, Ref.compare, linked branches, the OAuth device flow and search grouping.
mod common;
use common::{TestServer, TestUser};
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
    assert_eq!(res.status().as_u16(), 200);
    res.json().await.unwrap()
}

async fn data(s: &TestServer, u: &TestUser, query: &str, vars: Value) -> Value {
    let v = gql(s, u, query, vars).await;
    assert!(v.get("errors").is_none(), "unexpected errors: {v}");
    v["data"].clone()
}

async fn node_ids(s: &TestServer, u: &TestUser, repo: &str, number: i64) -> (String, String) {
    let d = data(
        s,
        u,
        "query($o:String!,$n:String!,$i:Int!){repository(owner:$o,name:$n){id issueOrPullRequest(number:$i){... on Issue{id} ... on PullRequest{id}}}}",
        json!({"o": u.login, "n": repo, "i": number}),
    )
    .await;
    (d["repository"]["id"].as_str().unwrap().into(), d["repository"]["issueOrPullRequest"]["id"].as_str().unwrap().into())
}

async fn new_issue(s: &TestServer, u: &TestUser, repo: &str, title: &str) -> i64 {
    let (st, v) = s.call(Some(u), "POST", &format!("/repos/{}/{repo}/issues", u.login), Some(json!({"title": title, "labels": ["bug"]}))).await;
    assert_eq!(st, 201, "{v}");
    v["number"].as_i64().unwrap()
}

/// Pushes `branch` with one new file on top of `from` (via a clone in a temp dir).
fn push_branch(s: &TestServer, u: &TestUser, repo: &str, from: &str, branch: &str, file: &str) {
    let w = s.work_dir(&format!("{repo}-{branch}"));
    assert!(s.git(&w, &["clone", "-q", &s.clone_url(u, repo), "c"]).0);
    let c = w.join("c");
    assert!(s.git(&c, &["checkout", "-q", "-B", branch, &format!("origin/{from}")]).0);
    std::fs::write(c.join(file), format!("{file}\n")).unwrap();
    assert!(s.git(&c, &["add", "."]).0);
    assert!(s.git(&c, &["commit", "-qm", &format!("Add {file}")]).0);
    let (ok, out) = s.git(&c, &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")]);
    assert!(ok, "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_mutation_is_an_error() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let v = gql(&s, &u, "mutation { bogusMutation(input: {}) { x } }", json!({})).await;
    assert!(v.get("data").is_none(), "{v}");
    assert_eq!(v["errors"][0]["message"], "Field 'bogusMutation' doesn't exist on type 'Mutation'");
    assert_eq!(v["errors"][0]["extensions"]["code"], "undefinedField");
    // Known mutations still run.
    let repo = s.create_repo(&u, true).await;
    let (rid, _) = (node_ids(&s, &u, &repo, new_issue(&s, &u, &repo, "x").await).await.0, ());
    let d = data(&s, &u, "mutation($r:ID!){createLabel(input:{repositoryId:$r,name:\"zz\",color:\"ffffff\"}){label{name}}}", json!({"r": rid})).await;
    assert_eq!(d["createLabel"]["label"]["name"], "zz");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archive_pin_lock_transfer() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let dest = s.create_repo(&u, true).await;
    let n = new_issue(&s, &u, &repo, "Move me").await;
    let (rid, iid) = node_ids(&s, &u, &repo, n).await;

    // archive / unarchive
    let d = data(&s, &u, "mutation($r:ID!){archiveRepository(input:{repositoryId:$r}){repository{isArchived}}}", json!({"r": rid})).await;
    assert_eq!(d["archiveRepository"]["repository"]["isArchived"], true);
    let d = data(&s, &u, "mutation($r:ID!){unarchiveRepository(input:{repositoryId:$r}){repository{isArchived}}}", json!({"r": rid})).await;
    assert_eq!(d["unarchiveRepository"]["repository"]["isArchived"], false);
    // non-admins can't
    let v = gql(&s, &other, "mutation($r:ID!){archiveRepository(input:{repositoryId:$r}){repository{id}}}", json!({"r": rid})).await;
    assert!(v["errors"].is_array(), "{v}");

    // pin / unpin
    let d = data(&s, &u, "mutation($i:ID!){pinIssue(input:{issueId:$i}){issue{isPinned}}}", json!({"i": iid})).await;
    assert_eq!(d["pinIssue"]["issue"]["isPinned"], true);
    let d = data(&s, &u, "mutation($i:ID!){unpinIssue(input:{issueId:$i}){issue{isPinned}}}", json!({"i": iid})).await;
    assert_eq!(d["unpinIssue"]["issue"]["isPinned"], false);

    // lock / unlock through the Lockable interface
    let d = data(&s, &u, "mutation($i:ID!){lockLockable(input:{lockableId:$i,lockReason:RESOLVED}){lockedRecord{locked activeLockReason}}}", json!({"i": iid})).await;
    assert_eq!(d["lockLockable"]["lockedRecord"], json!({"locked": true, "activeLockReason": "RESOLVED"}));
    let (_, v) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}/issues/{n}", u.login), None).await;
    assert_eq!(v["active_lock_reason"], "resolved");
    let d = data(&s, &u, "mutation($i:ID!){unlockLockable(input:{lockableId:$i}){unlockedRecord{locked}}}", json!({"i": iid})).await;
    assert_eq!(d["unlockLockable"]["unlockedRecord"]["locked"], false);

    // transfer: new number in the destination, labels kept by name, gone from the source
    let (dest_id, _) = node_ids(&s, &u, &dest, new_issue(&s, &u, &dest, "existing").await).await;
    let d = data(
        &s,
        &u,
        "mutation($i:ID!,$r:ID!){transferIssue(input:{issueId:$i,repositoryId:$r}){issue{number url title labels(first:5){nodes{name}}}}}",
        json!({"i": iid, "r": dest_id}),
    )
    .await;
    let moved = &d["transferIssue"]["issue"];
    assert_eq!(moved["number"], 2);
    assert_eq!(moved["title"], "Move me");
    assert!(moved["url"].as_str().unwrap().ends_with(&format!("/{}/{dest}/issues/2", u.login)));
    assert_eq!(moved["labels"]["nodes"][0]["name"], "bug");
    let (st, _) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}/issues/{n}", u.login), None).await;
    assert_eq!(st, 404);
    // only between repositories of the same owner
    let foreign = s.create_repo(&other, true).await;
    let (foreign_id, _) = node_ids(&s, &other, &foreign, new_issue(&s, &other, &foreign, "f").await).await;
    let v = gql(&s, &u, "mutation($i:ID!,$r:ID!){transferIssue(input:{issueId:$i,repositoryId:$r}){issue{id}}}", json!({"i": iid, "r": foreign_id})).await;
    assert!(v["errors"].is_array(), "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_branches_compare_update_and_revert() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let n = new_issue(&s, &u, &repo, "Fix the thing!").await;
    let (rid, iid) = node_ids(&s, &u, &repo, n).await;

    // gh issue develop: feature detection, branch creation, listing
    let d = data(&s, &u, "{LinkedBranch: __type(name: \"LinkedBranch\"){fields{name}}}", json!({})).await;
    assert!(!d["LinkedBranch"]["fields"].as_array().unwrap().is_empty());
    let d = data(&s, &u, "query($o:String!,$n:String!){repository(owner:$o,name:$n){defaultBranchRef{target{oid}}}}", json!({"o": u.login, "n": repo})).await;
    let oid = d["repository"]["defaultBranchRef"]["target"]["oid"].as_str().unwrap().to_string();
    let d = data(
        &s,
        &u,
        "mutation($i:ID!,$o:GitObjectID!,$r:ID){createLinkedBranch(input:{issueId:$i,oid:$o,repositoryId:$r}){linkedBranch{id ref{name}}}}",
        json!({"i": iid, "o": oid, "r": rid}),
    )
    .await;
    assert_eq!(d["createLinkedBranch"]["linkedBranch"]["ref"]["name"], format!("{n}-fix-the-thing"));
    let d = data(
        &s,
        &u,
        "query($o:String!,$n:String!,$i:Int!){repository(owner:$o,name:$n){issue(number:$i){linkedBranches(first:30){nodes{ref{name repository{url}}}}}}}",
        json!({"o": u.login, "n": repo, "i": n}),
    )
    .await;
    assert_eq!(d["repository"]["issue"]["linkedBranches"]["nodes"][0]["ref"]["name"], format!("{n}-fix-the-thing"));

    // A PR whose base moves on: compare says BEHIND by 1, update-branch (merge) fixes it.
    push_branch(&s, &u, &repo, "main", "topic", "topic.txt");
    let (st, pr) = s.call(Some(&u), "POST", &format!("/repos/{}/{repo}/pulls", u.login), Some(json!({"title": "Topic", "head": "topic", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    let pn = pr["number"].as_i64().unwrap();
    push_branch(&s, &u, &repo, "main", "main", "later.txt");
    let cmp = "query($o:String!,$n:String!,$p:Int!,$h:String!){repository(owner:$o,name:$n){pullRequest(number:$p){id baseRef{compare(headRef:$h){aheadBy behindBy status}}}}}";
    let d = data(&s, &u, cmp, json!({"o": u.login, "n": repo, "p": pn, "h": "topic"})).await;
    assert_eq!(d["repository"]["pullRequest"]["baseRef"]["compare"], json!({"aheadBy": 1, "behindBy": 1, "status": "DIVERGED"}));
    let pid = d["repository"]["pullRequest"]["id"].as_str().unwrap().to_string();
    data(&s, &u, "mutation($p:ID!){updatePullRequestBranch(input:{pullRequestId:$p,updateMethod:REBASE}){pullRequest{id}}}", json!({"p": pid})).await;
    let d = data(&s, &u, cmp, json!({"o": u.login, "n": repo, "p": pn, "h": "topic"})).await;
    assert_eq!(d["repository"]["pullRequest"]["baseRef"]["compare"]["behindBy"], 0);
    let (_, c) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}/commits?sha=topic&per_page=2", u.login), None).await;
    assert_eq!(c[0]["commit"]["message"], "Add topic.txt");
    assert_eq!(c[1]["commit"]["message"], "Add later.txt", "rebased onto the new base: {c}");

    // revert after merging: new PR off main undoing topic.txt
    let (st, m) = s.call(Some(&u), "PUT", &format!("/repos/{}/{repo}/pulls/{pn}/merge", u.login), Some(json!({}))).await;
    assert_eq!(st, 200, "{m}");
    let d = data(
        &s,
        &u,
        "mutation($p:ID!){revertPullRequest(input:{pullRequestId:$p}){pullRequest{number} revertPullRequest{number title body headRefName baseRefName url}}}",
        json!({"p": pid}),
    )
    .await;
    let r = &d["revertPullRequest"]["revertPullRequest"];
    assert_eq!(r["title"], "Revert \"Topic\"");
    assert_eq!(r["headRefName"], format!("revert-{pn}-topic"));
    assert_eq!(r["baseRefName"], "main");
    let (_, files) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}/pulls/{}/files", u.login, r["number"]), None).await;
    assert_eq!(files[0]["filename"], "topic.txt");
    assert_eq!(files[0]["status"], "removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oauth_device_flow() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let c = s.client();
    let form = |pairs: &[(&str, &str)]| url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
    let post = |path: &str, body: String| {
        c.post(format!("{}{path}", s.base)).header("Content-Type", "application/x-www-form-urlencoded").body(body).send()
    };
    // Form-encoded like github.com (what gh's oauth library parses).
    let res = post("/login/device/code", form(&[("client_id", "abc"), ("scope", "repo read:org")])).await.unwrap();
    assert!(res.headers()["content-type"].to_str().unwrap().starts_with("application/x-www-form-urlencoded"));
    let p: std::collections::HashMap<String, String> = url::form_urlencoded::parse(res.text().await.unwrap().as_bytes()).into_owned().collect();
    assert_eq!(p["verification_uri"], format!("{}/login/device", s.base));
    let (device, user_code) = (p["device_code"].clone(), p["user_code"].clone());
    let token_req = form(&[("client_id", "abc"), ("device_code", &device), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code")]);
    let poll = || async { url::form_urlencoded::parse(post("/login/oauth/access_token", token_req.clone()).await.unwrap().text().await.unwrap().as_bytes()).into_owned().collect::<std::collections::HashMap<String, String>>() };
    assert_eq!(poll().await["error"], "authorization_pending");
    // Approval needs a signed-in user and a valid code.
    let res = c.post(format!("{}/api/forge/device/approve", s.base)).json(&json!({"user_code": user_code})).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 401);
    let approve = |code: String| {
        c.post(format!("{}/api/forge/device/approve", s.base)).header("Authorization", format!("token {}", u.token)).json(&json!({"user_code": code})).send()
    };
    assert_eq!(approve("BBBB-CCCC".into()).await.unwrap().status().as_u16(), 404);
    assert_eq!(approve(user_code.to_lowercase()).await.unwrap().status().as_u16(), 204);
    let got = poll().await;
    let token = got.get("access_token").expect("token").clone();
    assert_eq!(got["token_type"], "bearer");
    // The token works and is issued once.
    let res = c.get(format!("{}/api/v3/user", s.base)).header("Authorization", format!("token {token}")).send().await.unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["login"], u.login);
    assert_eq!(poll().await["error"], "incorrect_device_code");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_ignores_grouping_parentheses() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    new_issue(&s, &u, &repo, "Parenthesized needle").await;
    let q = format!("( needle ) repo:{}/{repo} type:issue", u.login);
    let (st, v) = s.call(Some(&u), "GET", &format!("/search/issues?q={}", url::form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()), None).await;
    assert_eq!(st, 200);
    assert_eq!(v["total_count"], 1, "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discussions_lifecycle() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let q_repo = "query($o:String!,$n:String!){repository(owner:$o,name:$n){id hasDiscussionsEnabled}}";
    let d = data(&s, &u, q_repo, json!({"o": u.login, "n": repo})).await;
    assert_eq!(d["repository"]["hasDiscussionsEnabled"], false);
    let rid = d["repository"]["id"].as_str().unwrap().to_string();
    let (st, _) = s.call(Some(&u), "PATCH", &format!("/repos/{}/{repo}", u.login), Some(json!({"has_discussions": true}))).await;
    assert_eq!(st, 200);
    let d = data(
        &s,
        &u,
        "query($o:String!,$n:String!){repository(owner:$o,name:$n){discussionCategories(first:100){nodes{id slug isAnswerable}}}}",
        json!({"o": u.login, "n": repo}),
    )
    .await;
    let cats = d["repository"]["discussionCategories"]["nodes"].as_array().unwrap().clone();
    let general = cats.iter().find(|c| c["slug"] == "general").unwrap()["id"].as_str().unwrap().to_string();
    let qa = cats.iter().find(|c| c["slug"] == "q-a").unwrap();
    assert_eq!(qa["isAnswerable"], true);
    let qa = qa["id"].as_str().unwrap().to_string();

    let d = data(
        &s,
        &u,
        "mutation($r:ID!,$c:ID!){createDiscussion(input:{repositoryId:$r,categoryId:$c,title:\"Q\",body:\"B\"}){discussion{id number category{name}}}}",
        json!({"r": rid, "c": qa}),
    )
    .await;
    let disc = &d["createDiscussion"]["discussion"];
    assert_eq!(disc["category"]["name"], "Q&A");
    let did = disc["id"].as_str().unwrap().to_string();
    let n = disc["number"].as_i64().unwrap();
    let d = data(&s, &u, "mutation($d:ID!){addDiscussionComment(input:{discussionId:$d,body:\"answer\"}){comment{id url}}}", json!({"d": did})).await;
    let cid = d["addDiscussionComment"]["comment"]["id"].as_str().unwrap().to_string();
    assert!(d["addDiscussionComment"]["comment"]["url"].as_str().unwrap().contains(&format!("/discussions/{n}#discussioncomment-")));
    data(&s, &u, "mutation($d:ID!,$c:ID!){addDiscussionComment(input:{discussionId:$d,body:\"reply\",replyToId:$c}){comment{id}}}", json!({"d": did, "c": cid})).await;
    let d = data(&s, &u, "mutation($c:ID!){markDiscussionCommentAsAnswer(input:{id:$c}){discussion{isAnswered}}}", json!({"c": cid})).await;
    assert_eq!(d["markDiscussionCommentAsAnswer"]["discussion"]["isAnswered"], true);
    let d = data(
        &s,
        &u,
        "mutation($d:ID!,$c:ID!){updateDiscussion(input:{discussionId:$d,title:\"Q2\",categoryId:$c}){discussion{title category{slug}}}}",
        json!({"d": did, "c": general}),
    )
    .await;
    assert_eq!(d["updateDiscussion"]["discussion"], json!({"title": "Q2", "category": {"slug": "general"}}));
    let d = data(
        &s,
        &u,
        "query($o:String!,$n:String!,$i:Int!){repository(owner:$o,name:$n){discussion(number:$i){comments(first:10){totalCount nodes{isAnswer replies(last:4){nodes{body}}}}} discussions(first:10,states:[OPEN]){totalCount}}}",
        json!({"o": u.login, "n": repo, "i": n}),
    )
    .await;
    let r = &d["repository"];
    assert_eq!(r["discussions"]["totalCount"], 1);
    assert_eq!(r["discussion"]["comments"]["totalCount"], 1);
    assert_eq!(r["discussion"]["comments"]["nodes"][0]["isAnswer"], true);
    assert_eq!(r["discussion"]["comments"]["nodes"][0]["replies"]["nodes"][0]["body"], "reply");
    let d = data(&s, &u, "query($c:ID!){node(id:$c){... on DiscussionComment{discussion{id}}}}", json!({"c": cid})).await;
    assert_eq!(d["node"]["discussion"]["id"], did);
    // Others may read but not edit.
    let other = s.user().await;
    let v = gql(&s, &other, "mutation($d:ID!){updateDiscussion(input:{discussionId:$d,title:\"x\"}){discussion{id}}}", json!({"d": did})).await;
    assert!(v["errors"].is_array(), "{v}");
}
