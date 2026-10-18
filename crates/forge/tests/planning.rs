//! Server pieces behind the web UI's planning area: sub-issues, review thread resolution,
//! project views over GraphQL, issue search qualifiers (review-requested, review, archived),
//! review_requested timeline events, pinned comments in comment lists, discussion locking and
//! Markdown issue templates.
mod common;
use base64::Engine;
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

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

async fn put_file(s: &TestServer, u: &TestUser, repo: &str, path: &str, content: &str, branch: &str) {
    let base = format!("/repos/{}/{repo}/contents/{path}", u.login);
    let (st, cur) = s.call(Some(u), "GET", &format!("{base}?ref={branch}"), None).await;
    let sha = if st == 200 { cur["sha"].clone() } else { Value::Null };
    let (st, v) = s.call(Some(u), "PUT", &base, Some(json!({"message": "edit", "content": b64(content), "branch": branch, "sha": sha}))).await;
    assert!(st == 200 || st == 201, "{v}");
}

async fn issue(s: &TestServer, u: &TestUser, repo: &str, title: &str) -> Value {
    let (st, v) = s.call(Some(u), "POST", &format!("/repos/{}/{repo}/issues", u.login), Some(json!({"title": title}))).await;
    assert_eq!(st, 201, "{v}");
    v
}

/// A repository with a PR (#n) changing `a.txt` on branch `feature`.
async fn repo_with_pr(s: &TestServer, u: &TestUser) -> (String, i64) {
    let repo = s.create_repo(u, true).await;
    put_file(s, u, &repo, "a.txt", "one\ntwo\nthree\n", "main").await;
    let (_, main) = s.call(Some(u), "GET", &format!("/repos/{}/{repo}/git/ref/heads/main", u.login), None).await;
    let (st, _) = s
        .call(Some(u), "POST", &format!("/repos/{}/{repo}/git/refs", u.login), Some(json!({"ref": "refs/heads/feature", "sha": main["object"]["sha"]})))
        .await;
    assert_eq!(st, 201);
    put_file(s, u, &repo, "a.txt", "one\nTWO\nthree\nfour\n", "feature").await;
    let (st, pr) = s.call(Some(u), "POST", &format!("/repos/{}/{repo}/pulls", u.login), Some(json!({"title": "PR", "head": "feature", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    (repo, pr["number"].as_i64().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sub_issues_add_list_parent_remove_reorder() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let base = format!("/repos/{}/{repo}/issues", u.login);
    let parent = issue(&s, &u, &repo, "parent").await;
    let a = issue(&s, &u, &repo, "a").await;
    let b = issue(&s, &u, &repo, "b").await;
    let pn = parent["number"].as_i64().unwrap();
    for c in [&a, &b] {
        let (st, v) = s.call(Some(&u), "POST", &format!("{base}/{pn}/sub_issues"), Some(json!({"sub_issue_id": c["id"]}))).await;
        assert_eq!(st, 201, "{v}");
        assert_eq!(v["number"], pn, "returns the parent");
    }
    let (st, list) = s.call(Some(&u), "GET", &format!("{base}/{pn}/sub_issues"), None).await;
    assert_eq!(st, 200);
    let titles: Vec<&str> = list.as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["a", "b"]);
    let (st, p) = s.call(Some(&u), "GET", &format!("{base}/{}/parent", a["number"]), None).await;
    assert_eq!(st, 200);
    assert_eq!(p["id"], parent["id"]);
    let (st, _) = s.call(Some(&u), "GET", &format!("{base}/{pn}/parent"), None).await;
    assert_eq!(st, 404);

    // Cycles, duplicates, a second parent and self-parenting are rejected.
    let (st, e) = s.call(Some(&u), "POST", &format!("{base}/{}/sub_issues", a["number"]), Some(json!({"sub_issue_id": parent["id"]}))).await;
    assert_eq!(st, 422, "{e}");
    assert!(e["errors"][0]["message"].as_str().unwrap().contains("cycle"));
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/{pn}/sub_issues"), Some(json!({"sub_issue_id": a["id"]}))).await;
    assert_eq!(st, 422);
    let other = issue(&s, &u, &repo, "other parent").await;
    let on = other["number"].as_i64().unwrap();
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/{on}/sub_issues"), Some(json!({"sub_issue_id": a["id"]}))).await;
    assert_eq!(st, 422, "already has a parent");
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/{on}/sub_issues"), Some(json!({"sub_issue_id": other["id"]}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/{on}/sub_issues"), Some(json!({"sub_issue_id": a["id"], "replace_parent": true}))).await;
    assert_eq!(st, 201);
    let (_, p) = s.call(Some(&u), "GET", &format!("{base}/{}/parent", a["number"]), None).await;
    assert_eq!(p["id"], other["id"]);
    let (_, list) = s.call(Some(&u), "GET", &format!("{base}/{pn}/sub_issues"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Reprioritize and remove.
    let c = issue(&s, &u, &repo, "c").await;
    s.call(Some(&u), "POST", &format!("{base}/{pn}/sub_issues"), Some(json!({"sub_issue_id": c["id"]}))).await;
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{base}/{pn}/sub_issues/priority"), Some(json!({"sub_issue_id": c["id"], "before_id": b["id"]}))).await;
    assert_eq!(st, 200);
    let (_, list) = s.call(Some(&u), "GET", &format!("{base}/{pn}/sub_issues"), None).await;
    let titles: Vec<&str> = list.as_array().unwrap().iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["c", "b"]);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/{pn}/sub_issue"), Some(json!({"sub_issue_id": b["id"]}))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{base}/{pn}/sub_issue"), Some(json!({"sub_issue_id": b["id"]}))).await;
    assert_eq!(st, 400);

    // Read-only users can list but not add.
    let stranger = s.user().await;
    let (st, _) = s.call(Some(&stranger), "GET", &format!("{base}/{pn}/sub_issues"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&stranger), "POST", &format!("{base}/{pn}/sub_issues"), Some(json!({"sub_issue_id": b["id"]}))).await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_threads_resolve_and_unresolve() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let (repo, n) = repo_with_pr(&s, &u).await;
    let base = format!("/repos/{}/{repo}/pulls/{n}", u.login);
    let (_, pr) = s.call(Some(&u), "GET", &base, None).await;
    let (st, c) = s
        .call(Some(&u), "POST", &format!("{base}/comments"), Some(json!({"body": "hm", "commit_id": pr["head"]["sha"], "path": "a.txt", "line": 2, "side": "RIGHT"})))
        .await;
    assert_eq!(st, 201, "{c}");
    let (st, _) = s.call(Some(&u), "POST", &format!("{base}/comments/{}/replies", c["id"]), Some(json!({"body": "reply"}))).await;
    assert_eq!(st, 201);
    let q = "query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){reviewThreads(first:10){totalCount nodes{id isResolved viewerCanResolve viewerCanUnresolve path line resolvedBy{login} comments(first:10){nodes{databaseId body}}}}}}}";
    let vars = json!({"o": u.login, "r": repo, "n": n});
    let d = data(&s, &u, q, vars.clone()).await;
    let t = &d["repository"]["pullRequest"]["reviewThreads"];
    assert_eq!(t["totalCount"], 1, "replies join their thread");
    let t = &t["nodes"][0];
    assert_eq!(t["isResolved"], false);
    assert_eq!(t["viewerCanResolve"], true);
    assert_eq!(t["path"], "a.txt");
    assert_eq!(t["line"], 2);
    assert_eq!(t["comments"]["nodes"].as_array().unwrap().len(), 2);
    let id = t["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("PRRT_"));

    // A stranger can't resolve.
    let stranger = s.user().await;
    let v = gql(&s, &stranger, "mutation($t:ID!){resolveReviewThread(input:{threadId:$t}){thread{id}}}", json!({"t": id})).await;
    assert_eq!(v["errors"][0]["type"], "FORBIDDEN", "{v}");

    let d = data(&s, &u, "mutation($t:ID!){resolveReviewThread(input:{threadId:$t}){thread{isResolved resolvedBy{login} viewerCanUnresolve}}}", json!({"t": id})).await;
    assert_eq!(d["resolveReviewThread"]["thread"]["isResolved"], true);
    assert_eq!(d["resolveReviewThread"]["thread"]["resolvedBy"]["login"], u.login.as_str());
    assert_eq!(d["resolveReviewThread"]["thread"]["viewerCanUnresolve"], true);
    let d = data(&s, &u, "mutation($t:ID!){unresolveReviewThread(input:{threadId:$t}){thread{isResolved}}}", json!({"t": id})).await;
    assert_eq!(d["unresolveReviewThread"]["thread"]["isResolved"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_review_requested_review_state_and_archived() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let reviewer = s.user().await;
    let (repo, n) = repo_with_pr(&s, &u).await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("/repos/{}/{repo}/collaborators/{}", u.login, reviewer.login), Some(json!({"permission": "push"}))).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "POST", &format!("/repos/{}/{repo}/pulls/{n}/requested_reviewers", u.login), Some(json!({"reviewers": [reviewer.login]}))).await;
    assert_eq!(st, 201);
    let search = |q: String| {
        let s = &s;
        let r = &reviewer;
        async move {
            let (st, v) = s.call(Some(r), "GET", &format!("/search/issues?q={}", urlencode(&q)), None).await;
            assert_eq!(st, 200, "{v}");
            v["total_count"].as_i64().unwrap()
        }
    };
    assert_eq!(search(format!("is:pr review-requested:{}", reviewer.login)).await, 1);
    assert_eq!(search("is:pr review-requested:@me archived:false".into()).await, 1);
    assert_eq!(search(format!("is:pr user-review-requested:{}", reviewer.login)).await, 1);
    assert_eq!(search(format!("is:pr review-requested:{}", u.login)).await, 0);
    assert_eq!(search(format!("repo:{}/{repo} is:pr review:none", u.login)).await, 1);
    assert_eq!(search(format!("repo:{}/{repo} is:pr archived:true", u.login)).await, 0);

    // The timeline event names the reviewer and requester.
    let (_, tl) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}/issues/{n}/timeline", u.login), None).await;
    let e = tl.as_array().unwrap().iter().find(|e| e["event"] == "review_requested").unwrap();
    assert_eq!(e["requested_reviewer"]["login"], reviewer.login.as_str());
    assert_eq!(e["review_requester"]["login"], u.login.as_str());

    // Approving clears the request and flips the review qualifiers.
    let (st, _) = s.call(Some(&reviewer), "POST", &format!("/repos/{}/{repo}/pulls/{n}/reviews", u.login), Some(json!({"event": "APPROVE"}))).await;
    assert_eq!(st, 200);
    assert_eq!(search(format!("is:pr review-requested:{}", reviewer.login)).await, 0);
    assert_eq!(search(format!("repo:{}/{repo} is:pr review:approved", u.login)).await, 1);
    assert_eq!(search(format!("repo:{}/{repo} is:pr review:none", u.login)).await, 0);
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_views_over_graphql() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let me = data(&s, &u, "query{viewer{id databaseId}}", json!({})).await;
    let d = data(&s, &u, "mutation($o:ID!){createProjectV2(input:{ownerId:$o,title:\"P\"}){projectV2{number}}}", json!({"o": me["viewer"]["id"]})).await;
    let num = d["createProjectV2"]["projectV2"]["number"].as_i64().unwrap();
    let q = "query($n:Int!){viewer{projectV2(number:$n){views(first:10){totalCount nodes{id number name layout filter verticalGroupByFields(first:1){nodes{... on ProjectV2SingleSelectField{name}}}}} view(number:2){name}}}}";
    let d = data(&s, &u, q, json!({"n": num})).await;
    let v = &d["viewer"]["projectV2"];
    assert_eq!(v["views"]["totalCount"], 1, "implicit View 1");
    assert_eq!(v["views"]["nodes"][0]["name"], "View 1");
    assert_eq!(v["views"]["nodes"][0]["layout"], "TABLE_LAYOUT");
    assert_eq!(v["view"], Value::Null);
    let fields = data(&s, &u, "query($n:Int!){viewer{projectV2(number:$n){field(name:\"Status\"){... on ProjectV2SingleSelectField{databaseId}}}}}", json!({"n": num})).await;
    let status = fields["viewer"]["projectV2"]["field"]["databaseId"].as_i64().unwrap();
    let (st, view) = s
        .call(
            Some(&u),
            "POST",
            &format!("/users/{}/projectsV2/{num}/views", me["viewer"]["databaseId"]),
            Some(json!({"name": "Board", "layout": "board", "vertical_group_by": [status], "filter": "is:issue"})),
        )
        .await;
    assert_eq!(st, 201, "{view}");
    let d = data(&s, &u, q, json!({"n": num})).await;
    let v = &d["viewer"]["projectV2"];
    assert_eq!(v["views"]["totalCount"], 2);
    let board = &v["views"]["nodes"][1];
    assert_eq!(board["number"], 2);
    assert_eq!(board["layout"], "BOARD_LAYOUT");
    assert_eq!(board["filter"], "is:issue");
    assert_eq!(board["verticalGroupByFields"]["nodes"][0]["name"], "Status");
    assert_eq!(v["view"]["name"], "Board");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pinned_comment_in_lists_and_issue_templates() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let i = issue(&s, &u, &repo, "x").await;
    let base = format!("/repos/{}/{repo}/issues", u.login);
    let (_, c) = s.call(Some(&u), "POST", &format!("{base}/{}/comments", i["number"]), Some(json!({"body": "hi"}))).await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("{base}/comments/{}/pin", c["id"]), None).await;
    assert_eq!(st, 200);
    let (_, list) = s.call(Some(&u), "GET", &format!("{base}/{}/comments", i["number"]), None).await;
    assert_eq!(list[0]["pin"]["pinned_by"]["login"], u.login.as_str());
    let (_, one) = s.call(Some(&u), "GET", &format!("{base}/comments/{}", c["id"]), None).await;
    assert!(one["pin"]["pinned_at"].is_string(), "{one}");

    put_file(&s, &u, &repo, ".github/ISSUE_TEMPLATE/bug.md", "---\nname: Bug report\nabout: Report a bug\ntitle: \"[Bug] \"\nlabels: bug\n---\n\nSteps to reproduce\n", "main").await;
    put_file(&s, &u, &repo, ".github/ISSUE_TEMPLATE/feature.md", "Plain body\n", "main").await;
    let d = data(&s, &u, "query($o:String!,$r:String!){repository(owner:$o,name:$r){issueTemplates{name about title body filename}}}", json!({"o": u.login, "r": repo})).await;
    let t = d["repository"]["issueTemplates"].as_array().unwrap();
    assert_eq!(t.len(), 2, "{t:?}");
    assert_eq!(t[0]["name"], "Bug report");
    assert_eq!(t[0]["about"], "Report a bug");
    assert_eq!(t[0]["title"], "[Bug] ");
    assert_eq!(t[0]["body"], "Steps to reproduce\n");
    assert_eq!(t[0]["filename"], "bug.md");
    assert_eq!(t[1]["name"], "feature");
    assert_eq!(t[1]["body"], "Plain body\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discussions_lock_and_unlock() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let (st, _) = s.call(Some(&u), "PATCH", &format!("/repos/{}/{repo}", u.login), Some(json!({"has_discussions": true}))).await;
    assert_eq!(st, 200);
    let d = data(&s, &u, "query($o:String!,$r:String!){repository(owner:$o,name:$r){id discussionCategories(first:10){nodes{id}}}}", json!({"o": u.login, "r": repo})).await;
    let rid = d["repository"]["id"].clone();
    let cat = d["repository"]["discussionCategories"]["nodes"][0]["id"].clone();
    let d = data(&s, &u, "mutation($r:ID!,$c:ID!){createDiscussion(input:{repositoryId:$r,categoryId:$c,title:\"t\",body:\"b\"}){discussion{id}}}", json!({"r": rid, "c": cat})).await;
    let id = d["createDiscussion"]["discussion"]["id"].clone();
    let d = data(&s, &u, "mutation($id:ID!){lockLockable(input:{lockableId:$id}){lockedRecord{__typename locked}}}", json!({"id": id})).await;
    assert_eq!(d["lockLockable"]["lockedRecord"]["__typename"], "Discussion");
    assert_eq!(d["lockLockable"]["lockedRecord"]["locked"], true);
    // Non-collaborators can neither lock nor comment on a locked discussion.
    let other = s.user().await;
    let v = gql(&s, &other, "mutation($id:ID!){unlockLockable(input:{lockableId:$id}){clientMutationId}}", json!({"id": id})).await;
    assert!(v["errors"].is_array(), "{v}");
    let v = gql(&s, &other, "mutation($d:ID!){addDiscussionComment(input:{discussionId:$d,body:\"x\"}){comment{id}}}", json!({"d": id})).await;
    assert!(v["errors"].is_array(), "{v}");
    let d = data(&s, &u, "mutation($id:ID!){unlockLockable(input:{lockableId:$id}){unlockedRecord{locked}}}", json!({"id": id})).await;
    assert_eq!(d["unlockLockable"]["unlockedRecord"]["locked"], false);
}
