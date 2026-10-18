//! Checks API (check runs, annotations, suites, suite preferences), review comment reactions,
//! and check runs in GraphQL's statusCheckRollup.
mod common;

use base64::Engine;
use common::{TestServer, TestUser};
use serde_json::{json, Value};

async fn head(s: &TestServer, u: &TestUser, repo: &str, branch: &str) -> String {
    let (st, v) = s.call(Some(u), "GET", &format!("/repos/{}/{repo}/git/ref/heads/{branch}", u.login), None).await;
    assert_eq!(st, 200, "{v}");
    v["object"]["sha"].as_str().unwrap().to_string()
}

/// Creates `branch` from main with one new file; returns the new head sha.
async fn branch_with_file(s: &TestServer, u: &TestUser, repo: &str, branch: &str, path: &str) -> String {
    let main = head(s, u, repo, "main").await;
    let (st, v) = s.call(Some(u), "POST", &format!("/repos/{}/{repo}/git/refs", u.login), Some(json!({"ref": format!("refs/heads/{branch}"), "sha": main}))).await;
    assert_eq!(st, 201, "{v}");
    let content = base64::engine::general_purpose::STANDARD.encode(b"line one\nline two\n");
    let (st, v) = s
        .call(Some(u), "PUT", &format!("/repos/{}/{repo}/contents/{path}", u.login), Some(json!({"message": "add", "content": content, "branch": branch})))
        .await;
    assert_eq!(st, 201, "{v}");
    v["commit"]["sha"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn check_run_lifecycle_annotations_and_suites() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let r = format!("/repos/{}/{repo}", u.login);
    let sha = head(&s, &u, &repo, "main").await;

    // nothing yet
    let (st, v) = s.call(Some(&u), "GET", &format!("{r}/commits/main/check-runs"), None).await;
    assert_eq!((st, v["total_count"].as_i64()), (200, Some(0)));

    let ann = |line: i64, level: &str| json!({"path": "README.md", "start_line": line, "end_line": line, "start_column": 1, "end_column": 4, "annotation_level": level, "message": format!("{level} {line}")});
    let (st, run) = s
        .call(
            Some(&u),
            "POST",
            &format!("{r}/check-runs"),
            Some(json!({"name": "ci/build", "head_sha": sha, "status": "in_progress", "external_id": "ext-1",
                "output": {"title": "Build", "summary": "running", "annotations": [ann(1, "warning")]}})),
        )
        .await;
    assert_eq!(st, 201, "{run}");
    let id = run["id"].as_i64().unwrap();
    assert_eq!(run["status"], "in_progress");
    assert_eq!(run["conclusion"], Value::Null);
    assert!(run["started_at"].is_string());
    assert_eq!(run["completed_at"], Value::Null);
    assert_eq!(run["external_id"], "ext-1");
    assert_eq!(run["output"]["annotations_count"], 1);
    assert_eq!(run["app"]["slug"], "forge");
    assert_eq!(run["app"]["id"], 1);
    assert_eq!(run["head_sha"], sha.as_str());
    let suite_id = run["check_suite"]["id"].as_i64().unwrap();

    // update: complete + appended annotation
    let (st, upd) = s
        .call(Some(&u), "PATCH", &format!("{r}/check-runs/{id}"), Some(json!({"conclusion": "failure", "output": {"summary": "broken", "annotations": [ann(2, "failure")]}})))
        .await;
    assert_eq!(st, 200, "{upd}");
    assert_eq!(upd["status"], "completed");
    assert_eq!(upd["conclusion"], "failure");
    assert!(upd["completed_at"].is_string());
    assert_eq!(upd["output"]["title"], "Build", "title kept");
    assert_eq!(upd["output"]["summary"], "broken");
    assert_eq!(upd["output"]["annotations_count"], 2);
    assert_eq!(upd["name"], "ci/build");

    let (st, anns) = s.call(Some(&u), "GET", &format!("{r}/check-runs/{id}/annotations"), None).await;
    assert_eq!(st, 200);
    let anns = anns.as_array().unwrap();
    assert_eq!(anns.len(), 2);
    assert_eq!(anns[0]["annotation_level"], "warning");
    assert_eq!(anns[1]["message"], "failure 2");
    assert_eq!(anns[0]["start_column"], 1);
    assert!(anns[0]["blob_href"].as_str().unwrap().ends_with(&format!("/blob/{sha}/README.md")));

    let (st, got) = s.call(None, "GET", &format!("{r}/check-runs/{id}"), None).await;
    assert_eq!((st, got["id"].as_i64()), (200, Some(id)));

    // suite: auto-created, derived state
    let (st, suite) = s.call(Some(&u), "GET", &format!("{r}/check-suites/{suite_id}"), None).await;
    assert_eq!(st, 200, "{suite}");
    assert_eq!(suite["status"], "completed");
    assert_eq!(suite["conclusion"], "failure");
    assert_eq!(suite["head_branch"], "main");
    assert_eq!(suite["latest_check_runs_count"], 1);
    assert_eq!(suite["head_commit"]["id"], sha.as_str());
    assert_eq!(suite["app"]["slug"], "forge");

    // a second run with the same name: latest vs all; a queued run makes the suite in progress
    let (st, run2) = s.call(Some(&u), "POST", &format!("{r}/check-runs"), Some(json!({"name": "ci/build", "head_sha": "main"}))).await;
    assert_eq!(st, 201, "{run2}");
    assert_eq!(run2["status"], "queued");
    assert_eq!(run2["check_suite"]["id"].as_i64(), Some(suite_id), "same suite per commit");
    let (_, latest) = s.call(Some(&u), "GET", &format!("{r}/commits/{sha}/check-runs"), None).await;
    assert_eq!(latest["total_count"], 1);
    assert_eq!(latest["check_runs"][0]["id"], run2["id"]);
    let (_, all) = s.call(Some(&u), "GET", &format!("{r}/commits/main/check-runs?filter=all"), None).await;
    assert_eq!(all["total_count"], 2);
    let (_, by_status) = s.call(Some(&u), "GET", &format!("{r}/commits/main/check-runs?filter=all&status=completed"), None).await;
    assert_eq!(by_status["total_count"], 1);
    let (_, by_name) = s.call(Some(&u), "GET", &format!("{r}/commits/main/check-runs?check_name=nope"), None).await;
    assert_eq!(by_name["total_count"], 0);
    let (_, other_app) = s.call(Some(&u), "GET", &format!("{r}/commits/main/check-runs?app_id=42"), None).await;
    assert_eq!(other_app["total_count"], 0);
    let (_, in_suite) = s.call(Some(&u), "GET", &format!("{r}/check-suites/{suite_id}/check-runs?filter=all"), None).await;
    assert_eq!(in_suite["total_count"], 2);
    let (_, suite) = s.call(Some(&u), "GET", &format!("{r}/check-suites/{suite_id}"), None).await;
    assert_eq!((suite["status"].as_str(), suite["conclusion"].as_str()), (Some("queued"), None));

    let (_, suites) = s.call(Some(&u), "GET", &format!("{r}/commits/main/check-suites"), None).await;
    assert_eq!(suites["total_count"], 1);
    assert_eq!(suites["check_suites"][0]["id"].as_i64(), Some(suite_id));

    // create-suite: existing -> 200, new commit -> 201
    let (st, _) = s.call(Some(&u), "POST", &format!("{r}/check-suites"), Some(json!({"head_sha": sha}))).await;
    assert_eq!(st, 200);
    let other = branch_with_file(&s, &u, &repo, "other", "other.txt").await;
    let (st, ns) = s.call(Some(&u), "POST", &format!("{r}/check-suites"), Some(json!({"head_sha": other}))).await;
    assert_eq!(st, 201, "{ns}");
    assert_eq!(ns["status"], "queued");
    assert_eq!(ns["latest_check_runs_count"], 0);
    assert_eq!(ns["head_branch"], "other");
    let (st, _) = s.call(Some(&u), "POST", &format!("{r}/check-suites/{suite_id}/rerequest"), None).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{r}/check-runs/{id}/rerequest"), None).await;
    assert_eq!(st, 201);

    // 404s
    let (st, _) = s.call(Some(&u), "GET", &format!("{r}/check-runs/999999999"), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "GET", &format!("{r}/check-suites/999999999"), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn check_run_validation_and_permissions() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let r = format!("/repos/{}/{repo}", u.login);
    let sha = head(&s, &u, &repo, "main").await;
    let post = |b: Value| {
        let s = &s;
        let u = &u;
        let r = r.clone();
        async move { s.call(Some(u), "POST", &format!("{r}/check-runs"), Some(b)).await }
    };
    for (body, what) in [
        (json!({"head_sha": sha}), "missing name"),
        (json!({"name": "x"}), "missing head_sha"),
        (json!({"name": "x", "head_sha": "0000000000000000000000000000000000000000"}), "unknown sha"),
        (json!({"name": "x", "head_sha": sha, "status": "completed"}), "completed without conclusion"),
        (json!({"name": "x", "head_sha": sha, "status": "bogus"}), "bad status"),
        (json!({"name": "x", "head_sha": sha, "conclusion": "bogus"}), "bad conclusion"),
        (json!({"name": "x", "head_sha": sha, "output": {"summary": "no title"}}), "output without title"),
        (json!({"name": "x", "head_sha": sha, "output": {"title": "t", "summary": "s", "annotations": [{"path": "a", "start_line": 1, "end_line": 1, "annotation_level": "fatal", "message": "m"}]}}), "bad level"),
        (json!({"name": "x", "head_sha": sha, "output": {"title": "t", "summary": "s", "annotations": [{"path": "a", "start_line": 1, "end_line": 1, "annotation_level": "notice"}]}}), "annotation without message"),
    ] {
        let (st, v) = post(body).await;
        assert_eq!(st, 422, "{what}: {v}");
    }
    let many: Vec<Value> = (1..=51).map(|i| json!({"path": "a", "start_line": i, "end_line": i, "annotation_level": "notice", "message": "m"})).collect();
    let (st, _) = post(json!({"name": "x", "head_sha": sha, "output": {"title": "t", "summary": "s", "annotations": many}})).await;
    assert_eq!(st, 422, "more than 50 annotations");

    // a conclusion completes the run
    let (st, run) = post(json!({"name": "done", "head_sha": sha, "conclusion": "success"})).await;
    assert_eq!(st, 201);
    assert_eq!(run["status"], "completed");
    assert!(run["completed_at"].is_string());
    let id = run["id"].as_i64().unwrap();

    // read-only user and anonymous
    let (st, _) = s.call(Some(&other), "POST", &format!("{r}/check-runs"), Some(json!({"name": "x", "head_sha": sha}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&other), "PATCH", &format!("{r}/check-runs/{id}"), Some(json!({"status": "in_progress"}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(None, "POST", &format!("{r}/check-runs"), Some(json!({"name": "x", "head_sha": sha}))).await;
    assert_eq!(st, 401);
    let (st, _) = s.call(Some(&other), "POST", &format!("{r}/check-suites"), Some(json!({"head_sha": sha}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&other), "GET", &format!("{r}/check-runs/{id}"), None).await;
    assert_eq!(st, 200, "public repo is readable");

    // private repo: invisible to others
    let (st, _) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": format!("{repo}-p"), "private": true, "auto_init": true}))).await;
    assert_eq!(st, 201);
    let psha = head(&s, &u, &format!("{repo}-p"), "main").await;
    let pr = format!("/repos/{}/{repo}-p", u.login);
    let (st, prun) = s.call(Some(&u), "POST", &format!("{pr}/check-runs"), Some(json!({"name": "x", "head_sha": psha}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&other), "GET", &format!("{pr}/check-runs/{}", prun["id"]), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(None, "GET", &format!("{pr}/check-suites/{}", prun["check_suite"]["id"]), None).await;
    assert_eq!(st, 404);
    // run ids are per repository
    let (st, _) = s.call(Some(&u), "GET", &format!("{r}/check-runs/{}", prun["id"]), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn suite_preferences_auto_trigger_on_push() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let r = format!("/repos/{}/{repo}", u.login);

    // default: no suite on push
    let sha1 = branch_with_file(&s, &u, &repo, "b1", "one.txt").await;
    let (_, v) = s.call(Some(&u), "GET", &format!("{r}/commits/{sha1}/check-suites"), None).await;
    assert_eq!(v["total_count"], 0);

    let (st, v) = s.call(Some(&u), "PATCH", &format!("{r}/check-suites/preferences"), Some(json!({"auto_trigger_checks": [{"app_id": 1, "setting": true}]}))).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["preferences"]["auto_trigger_checks"], json!([{"app_id": 1, "setting": true}]));
    assert_eq!(v["repository"]["name"], repo.as_str());
    let sha2 = branch_with_file(&s, &u, &repo, "b2", "two.txt").await;
    let (_, v) = s.call(Some(&u), "GET", &format!("{r}/commits/{sha2}/check-suites"), None).await;
    assert_eq!(v["total_count"], 1, "{v}");
    assert_eq!(v["check_suites"][0]["status"], "queued");
    assert_eq!(v["check_suites"][0]["head_branch"], "b2");

    let (st, _) = s.call(Some(&u), "PATCH", &format!("{r}/check-suites/preferences"), Some(json!({"auto_trigger_checks": [{"app_id": 77, "setting": true}]}))).await;
    assert_eq!(st, 422, "unknown app");
    let (st, _) = s.call(Some(&other), "PATCH", &format!("{r}/check-suites/preferences"), Some(json!({"auto_trigger_checks": []}))).await;
    assert_eq!(st, 403, "admin only");
}

#[tokio::test(flavor = "multi_thread")]
async fn pr_check_runs_rollup_and_review_comment_reactions() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let r = format!("/repos/{}/{repo}", u.login);
    let head_sha = branch_with_file(&s, &u, &repo, "feat", "feat.txt").await;
    let (st, pr) = s.call(Some(&u), "POST", &format!("{r}/pulls"), Some(json!({"title": "Feat", "head": "feat", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    let number = pr["number"].as_i64().unwrap();

    // a commit status plus check runs on the head
    let (st, _) = s.call(Some(&u), "POST", &format!("{r}/statuses/{head_sha}"), Some(json!({"state": "success", "context": "ci/status"}))).await;
    assert_eq!(st, 201);
    let (st, run) = s.call(Some(&u), "POST", &format!("{r}/check-runs"), Some(json!({"name": "ci/check", "head_sha": head_sha, "status": "in_progress"}))).await;
    assert_eq!(st, 201);
    assert_eq!(run["pull_requests"][0]["number"].as_i64(), Some(number));
    assert_eq!(run["pull_requests"][0]["head"]["ref"], "feat");
    assert_eq!(run["pull_requests"][0]["base"]["ref"], "main");

    let q = r#"query($o:String!,$n:String!,$num:Int!){repository(owner:$o,name:$n){pullRequest(number:$num){
        commits(last:1){nodes{commit{statusCheckRollup{state contexts(first:100){nodes{__typename
          ... on StatusContext{context state}
          ... on CheckRun{name status conclusion startedAt completedAt detailsUrl checkSuite{workflowRun{workflow{name}}}}}}}}}}}}}"#;
    let gql = |q: &'static str| {
        let s = &s;
        let u = &u;
        let vars = json!({"o": u.login, "n": repo, "num": number});
        async move {
            let res = s
                .client()
                .post(format!("{}/api/graphql", s.base))
                .header("Authorization", format!("token {}", u.token))
                .json(&json!({"query": q, "variables": vars}))
                .send()
                .await
                .unwrap();
            let v: Value = res.json().await.unwrap();
            assert!(v.get("errors").is_none(), "{v}");
            v["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"].clone()
        }
    };
    let roll = gql(q).await;
    assert_eq!(roll["state"], "PENDING", "{roll}");
    let nodes = roll["contexts"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    let cr = nodes.iter().find(|n| n["__typename"] == "CheckRun").unwrap();
    assert_eq!((cr["name"].as_str(), cr["status"].as_str(), &cr["conclusion"]), (Some("ci/check"), Some("IN_PROGRESS"), &Value::Null));
    assert!(nodes.iter().any(|n| n["__typename"] == "StatusContext" && n["context"] == "ci/status"));

    let (st, _) = s.call(Some(&u), "PATCH", &format!("{r}/check-runs/{}", run["id"]), Some(json!({"conclusion": "success"}))).await;
    assert_eq!(st, 200);
    let roll = gql(q).await;
    assert_eq!(roll["state"], "SUCCESS", "{roll}");
    let (st, _) = s.call(Some(&u), "POST", &format!("{r}/check-runs"), Some(json!({"name": "ci/lint", "head_sha": head_sha, "conclusion": "failure"}))).await;
    assert_eq!(st, 201);
    let roll = gql(q).await;
    assert_eq!(roll["state"], "FAILURE", "{roll}");

    // review comment reactions
    let (st, c) = s
        .call(Some(&u), "POST", &format!("{r}/pulls/{number}/comments"), Some(json!({"body": "nit", "commit_id": head_sha, "path": "feat.txt", "line": 1, "side": "RIGHT"})))
        .await;
    assert_eq!(st, 201, "{c}");
    let cid = c["id"].as_i64().unwrap();
    let (st, re) = s.call(Some(&other), "POST", &format!("{r}/pulls/comments/{cid}/reactions"), Some(json!({"content": "eyes"}))).await;
    assert_eq!(st, 201, "{re}");
    let (st, _) = s.call(Some(&other), "POST", &format!("{r}/pulls/comments/{cid}/reactions"), Some(json!({"content": "eyes"}))).await;
    assert_eq!(st, 200, "same reaction again");
    let (st, _) = s.call(Some(&u), "POST", &format!("{r}/pulls/comments/{cid}/reactions"), Some(json!({"content": "nope"}))).await;
    assert_eq!(st, 422);
    let (st, list) = s.call(None, "GET", &format!("{r}/pulls/comments/{cid}/reactions"), None).await;
    assert_eq!(st, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["user"]["login"], other.login.as_str());
    let (_, got) = s.call(None, "GET", &format!("{r}/pulls/comments/{cid}"), None).await;
    assert_eq!(got["reactions"]["eyes"], 1);
    assert_eq!(got["reactions"]["total_count"], 1);
    // only the reactor (or an admin) may delete
    let rid = re["id"].as_i64().unwrap();
    let third = s.user().await;
    let (st, _) = s.call(Some(&third), "DELETE", &format!("{r}/pulls/comments/{cid}/reactions/{rid}"), None).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&other), "DELETE", &format!("{r}/pulls/comments/{cid}/reactions/{rid}"), None).await;
    assert_eq!(st, 204);
    let (_, list) = s.call(None, "GET", &format!("{r}/pulls/comments/{cid}/reactions"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
    let (st, _) = s.call(None, "GET", &format!("{r}/pulls/comments/999999999/reactions"), None).await;
    assert_eq!(st, 404);
}
