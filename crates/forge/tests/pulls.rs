mod common;
use std::path::{Path, PathBuf};

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

fn write(dir: &Path, file: &str, content: &str) {
    let p = dir.join(file);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

fn git_ok(s: &TestServer, dir: &Path, args: &[&str]) -> String {
    let (ok, out) = s.git(dir, args);
    assert!(ok, "git {args:?} failed: {out}");
    out
}

fn commit(s: &TestServer, dir: &Path, file: &str, content: &str, msg: &str) -> String {
    write(dir, file, content);
    git_ok(s, dir, &["add", "-A"]);
    git_ok(s, dir, &["commit", "-q", "-m", msg]);
    git_ok(s, dir, &["rev-parse", "HEAD"]).trim().to_string()
}

/// A user, an auto-initialized repo and a clone of it.
async fn setup(s: &TestServer) -> (TestUser, String, PathBuf) {
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new("forge=warn")).with_test_writer().try_init();
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let w = s.work_dir(&unique("w"));
    git_ok(s, &w, &["clone", "-q", &s.clone_url(&u, &repo), "c"]);
    (u, repo, w.join("c"))
}

async fn wait_merged_state(s: &TestServer, u: &TestUser, path: &str) -> Value {
    for _ in 0..50 {
        let (st, pr) = s.call(Some(u), "GET", path, None).await;
        assert_eq!(st, 200, "{pr}");
        if !pr["mergeable"].is_null() {
            return pr;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("mergeable never computed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_list_get_files_commits_merge() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);

    git_ok(&s, &c, &["checkout", "-q", "-b", "feature"]);
    commit(&s, &c, "a.txt", "one\ntwo\nthree\n", "add a");
    let head = commit(&s, &c, "b.txt", "bee\n", "add b");
    git_ok(&s, &c, &["push", "-q", "origin", "feature"]);
    git_ok(&s, &c, &["push", "-q", "origin", "main:same"]);

    // Validation errors.
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "x", "head": "nope", "base": "main"}))).await;
    assert_eq!(st, 422, "{e}");
    assert_eq!(e["errors"][0]["field"], "head");
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "x", "head": "feature", "base": "nope"}))).await;
    assert_eq!(st, 422);
    assert_eq!(e["errors"][0]["field"], "base");
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "x", "head": "same", "base": "main"}))).await;
    assert_eq!(st, 422);
    assert_eq!(e["errors"][0]["message"], "No commits between main and same");

    let (st, pr) = s
        .call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "Add files", "body": "please", "head": "feature", "base": "main"})))
        .await;
    assert_eq!(st, 201, "{pr}");
    assert_eq!(pr["number"], 1);
    assert_eq!(pr["head"]["sha"], head);
    assert_eq!(pr["head"]["ref"], "feature");
    assert_eq!(pr["head"]["label"], format!("{}:feature", u.login));
    assert_eq!(pr["base"]["repo"]["full_name"], format!("{}/{}", u.login, repo));
    assert_eq!(pr["commits"], 2);
    assert_eq!(pr["additions"], 4);
    assert_eq!(pr["changed_files"], 2);
    assert_eq!(pr["merged"], false);
    assert_eq!(pr["mergeable"], true);
    assert_eq!(pr["mergeable_state"], "clean");
    assert!(pr["merge_commit_sha"].is_string());
    assert_eq!(pr["auto_merge"], Value::Null);

    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "dup", "head": "feature", "base": "main"}))).await;
    assert_eq!(st, 422);
    assert_eq!(e["errors"][0]["message"], format!("A pull request already exists for {}:feature.", u.login));

    let (_, r) = s.call(Some(&u), "GET", &rp, None).await;
    assert_eq!(r["open_issues_count"], 1);

    let (st, list) = s.call(Some(&u), "GET", &format!("{rp}/pulls"), None).await;
    assert_eq!(st, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/pulls?head={}:feature", u.login), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/pulls?head={}:other", u.login), None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/pulls?state=closed"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);

    let (st, files) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/files"), None).await;
    assert_eq!(st, 200);
    let files = files.as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["filename"], "a.txt");
    assert_eq!(files[0]["status"], "added");
    assert_eq!(files[0]["additions"], 3);
    assert_eq!(files[0]["patch"], "@@ -0,0 +1,3 @@\n+one\n+two\n+three");
    let (st, commits) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/commits"), None).await;
    assert_eq!(st, 200);
    let commits = commits.as_array().unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[1]["sha"], head);
    assert_eq!(commits[0]["commit"]["message"], "add a");

    // diff / patch media types
    let res = s
        .client()
        .get(format!("{}{rp}/pulls/1", s.api))
        .header("Authorization", format!("token {}", u.token))
        .header("Accept", "application/vnd.github.v3.diff")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let diff = res.text().await.unwrap();
    assert!(diff.starts_with("diff --git a/a.txt b/a.txt"), "{diff}");
    let res = s
        .client()
        .get(format!("{}{rp}/pulls/1", s.api))
        .header("Authorization", format!("token {}", u.token))
        .header("Accept", "application/vnd.github.patch")
        .send()
        .await
        .unwrap();
    assert!(res.text().await.unwrap().contains("Subject: [PATCH 1/2] add a"));

    // refs/pull/1/head is fetchable.
    git_ok(&s, &c, &["fetch", "-q", "origin", "pull/1/head:pr1"]);
    assert_eq!(git_ok(&s, &c, &["rev-parse", "pr1"]).trim(), head);

    // Pushing to the head branch updates the PR.
    let head2 = commit(&s, &c, "a.txt", "one\ntwo\nthree\nfour\n", "more a");
    git_ok(&s, &c, &["push", "-q", "origin", "feature"]);
    let (_, pr) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1"), None).await;
    assert_eq!(pr["head"]["sha"], head2);
    assert_eq!(pr["commits"], 3);
    git_ok(&s, &c, &["fetch", "-q", "origin", "+pull/1/head:pr1"]);
    assert_eq!(git_ok(&s, &c, &["rev-parse", "pr1"]).trim(), head2);
    git_ok(&s, &c, &["fetch", "-q", "origin", "+pull/1/merge:pr1m"]);

    // Merge: sha guard, then success.
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/merge"), None).await;
    assert_eq!(st, 404);
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/1/merge"), Some(json!({"sha": head}))).await;
    assert_eq!(st, 409, "{e}");
    assert_eq!(e["message"], "Head branch was modified. Review and try the merge again.");
    let (st, m) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/1/merge"), Some(json!({"sha": head2, "merge_method": "merge"}))).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["merged"], true);
    let merge_sha = m["sha"].as_str().unwrap().to_string();
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/merge"), None).await;
    assert_eq!(st, 204);
    let (_, pr) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1"), None).await;
    assert_eq!(pr["state"], "closed");
    assert_eq!(pr["merged"], true);
    assert_eq!(pr["merged_by"]["login"], u.login.as_str());
    assert_eq!(pr["merge_commit_sha"], merge_sha);
    let (_, r) = s.call(Some(&u), "GET", &rp, None).await;
    assert_eq!(r["open_issues_count"], 0);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/1/merge"), None).await;
    assert_eq!(st, 405);

    // History: merge commit with two parents on main.
    git_ok(&s, &c, &["fetch", "-q", "origin"]);
    assert_eq!(git_ok(&s, &c, &["rev-parse", "origin/main"]).trim(), merge_sha);
    let parents = git_ok(&s, &c, &["log", "-1", "--format=%P", "origin/main"]);
    assert_eq!(parents.split_whitespace().count(), 2);
    assert!(parents.contains(&head2));
    let subject = git_ok(&s, &c, &["log", "-1", "--format=%s%n%b", "origin/main"]);
    assert!(subject.starts_with(&format!("Merge pull request #1 from {}/feature", u.login)), "{subject}");
    assert!(subject.contains("Add files"));

    // Events recorded on the issue row.
    let events: Vec<String> = sqlx::query_scalar("SELECT event FROM issue_events e JOIN issues i ON i.id = e.issue_id WHERE i.repo_id = $1 ORDER BY e.id")
        .bind(r["id"].as_i64().unwrap())
        .fetch_all(&s.state.db)
        .await
        .unwrap();
    assert!(events.contains(&"merged".to_string()) && events.contains(&"closed".to_string()) && events.contains(&"referenced".to_string()), "{events:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn squash_and_rebase() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    git_ok(&s, &c, &["checkout", "-q", "-b", "sq"]);
    commit(&s, &c, "s1.txt", "1\n", "s1");
    commit(&s, &c, "s2.txt", "2\n", "s2");
    git_ok(&s, &c, &["push", "-q", "origin", "sq"]);
    git_ok(&s, &c, &["checkout", "-q", "main"]);
    git_ok(&s, &c, &["checkout", "-q", "-b", "rb"]);
    commit(&s, &c, "r1.txt", "1\n", "r1");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let r2 = commit(&s, &c, "r2.txt", "2\n", "r2");
    git_ok(&s, &c, &["push", "-q", "origin", "rb"]);

    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "Squash me", "head": "sq", "base": "main"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "Rebase me", "head": "rb", "base": "main"}))).await;
    assert_eq!(st, 201);

    let (st, m) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/1/merge"), Some(json!({"merge_method": "squash"}))).await;
    assert_eq!(st, 200, "{m}");
    git_ok(&s, &c, &["fetch", "-q", "origin"]);
    let sq = m["sha"].as_str().unwrap();
    assert_eq!(git_ok(&s, &c, &["log", "-1", "--format=%P", sq]).split_whitespace().count(), 1);
    let msg = git_ok(&s, &c, &["log", "-1", "--format=%B", sq]);
    assert!(msg.starts_with("Squash me (#1)"), "{msg}");
    assert!(msg.contains("* s1") && msg.contains("* s2"));
    git_ok(&s, &c, &["cat-file", "-e", &format!("{sq}:s2.txt")]);

    // PR 2 is still mergeable after main moved; rebase it.
    let pr2 = wait_merged_state(&s, &u, &format!("{rp}/pulls/2")).await;
    assert_eq!(pr2["mergeable"], true);
    assert_eq!(pr2["base"]["sha"], sq);
    let (st, m) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/2/merge"), Some(json!({"merge_method": "rebase"}))).await;
    assert_eq!(st, 200, "{m}");
    git_ok(&s, &c, &["fetch", "-q", "origin"]);
    let tip = m["sha"].as_str().unwrap();
    let log = git_ok(&s, &c, &["log", "--format=%s|%an|%at", &format!("{sq}..{tip}")]);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(lines[0].starts_with("r2|Test|"));
    assert!(lines[1].starts_with("r1|Test|"));
    let orig_time = git_ok(&s, &c, &["log", "-1", "--format=%at", &r2]);
    assert_eq!(lines[0].split('|').nth(2).unwrap(), orig_time.trim());
    assert_eq!(git_ok(&s, &c, &["log", "-1", "--format=%P", tip]).split_whitespace().count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conflicts_update_branch_and_push_merge() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let readme = std::fs::read_to_string(c.join("README.md")).unwrap();
    git_ok(&s, &c, &["checkout", "-q", "-b", "conflict"]);
    commit(&s, &c, "README.md", "branch version\n", "branch edit");
    git_ok(&s, &c, &["push", "-q", "origin", "conflict"]);
    git_ok(&s, &c, &["checkout", "-q", "main"]);
    git_ok(&s, &c, &["checkout", "-q", "-b", "behind"]);
    commit(&s, &c, "behind.txt", "x\n", "behind");
    git_ok(&s, &c, &["push", "-q", "origin", "behind"]);
    git_ok(&s, &c, &["checkout", "-q", "main"]);
    git_ok(&s, &c, &["checkout", "-q", "-b", "direct"]);
    let direct = commit(&s, &c, "direct.txt", "d\n", "direct");
    git_ok(&s, &c, &["push", "-q", "origin", "direct"]);
    git_ok(&s, &c, &["checkout", "-q", "main"]);

    for (title, head) in [("Conflicting", "conflict"), ("Behind", "behind"), ("Direct", "direct")] {
        let (st, pr) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": title, "head": head, "base": "main"}))).await;
        assert_eq!(st, 201, "{pr}");
    }
    // main moves: README conflicts with PR 1, PR 2 falls behind.
    commit(&s, &c, "README.md", &format!("{readme}main version\n"), "main edit");
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);

    let pr = wait_merged_state(&s, &u, &format!("{rp}/pulls/1")).await;
    assert_eq!(pr["mergeable"], false);
    assert_eq!(pr["mergeable_state"], "dirty");
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/1/merge"), None).await;
    assert_eq!(st, 405, "{e}");
    assert_eq!(e["message"], "Pull Request is not mergeable");

    // update-branch merges main into `behind`.
    let (_, pr2) = s.call(Some(&u), "GET", &format!("{rp}/pulls/2"), None).await;
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/2/update-branch"), Some(json!({"expected_head_sha": "0000000"}))).await;
    assert_eq!(st, 422, "{e}");
    let (st, r) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/2/update-branch"), Some(json!({"expected_head_sha": pr2["head"]["sha"]}))).await;
    assert_eq!(st, 202, "{r}");
    let (_, pr2b) = s.call(Some(&u), "GET", &format!("{rp}/pulls/2"), None).await;
    assert_ne!(pr2b["head"]["sha"], pr2["head"]["sha"]);
    git_ok(&s, &c, &["fetch", "-q", "origin"]);
    let parents = git_ok(&s, &c, &["log", "-1", "--format=%P", "origin/behind"]);
    assert_eq!(parents.split_whitespace().count(), 2);
    assert_eq!(pr2b["head"]["sha"].as_str().unwrap(), git_ok(&s, &c, &["rev-parse", "origin/behind"]).trim());

    // Pushing the head commits into main marks PR 3 merged.
    git_ok(&s, &c, &["merge", "-q", "--no-edit", "origin/direct"]);
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);
    let (_, pr3) = s.call(Some(&u), "GET", &format!("{rp}/pulls/3"), None).await;
    assert_eq!(pr3["merged"], true, "{pr3}");
    assert_eq!(pr3["state"], "closed");
    assert_eq!(pr3["head"]["sha"], direct);

    // Deleting a PR's head branch closes it.
    git_ok(&s, &c, &["push", "-q", "origin", ":conflict"]);
    let (_, pr1) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1"), None).await;
    assert_eq!(pr1["state"], "closed");
    assert_eq!(pr1["merged"], false);
    let (_, r) = s.call(Some(&u), "GET", &rp, None).await;
    assert_eq!(r["open_issues_count"], 1);

    // Close / reopen via PATCH, title and base edit.
    let (st, p) = s.call(Some(&u), "PATCH", &format!("{rp}/pulls/2"), Some(json!({"state": "closed", "title": "Behind (renamed)"}))).await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["state"], "closed");
    assert_eq!(p["title"], "Behind (renamed)");
    let (st, p) = s.call(Some(&u), "PATCH", &format!("{rp}/pulls/2"), Some(json!({"state": "open"}))).await;
    assert_eq!(st, 200);
    assert_eq!(p["state"], "open");
    let (st, p) = s.call(Some(&u), "PATCH", &format!("{rp}/pulls/2"), Some(json!({"base": "direct"}))).await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["base"]["ref"], "direct");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reviews_and_comments() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let rev = s.user().await;
    let repo_id: i64 = s.call(Some(&u), "GET", &rp, None).await.1["id"].as_i64().unwrap();
    let rev_id: i64 = s.call(Some(&rev), "GET", "/user", None).await.1["id"].as_i64().unwrap();
    sqlx::query("INSERT INTO repo_collaborators (repo_id, user_id, permission) VALUES ($1, $2, 'push')")
        .bind(repo_id)
        .bind(rev_id)
        .execute(&s.state.db)
        .await
        .unwrap();

    write(&c, "code.txt", "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n");
    git_ok(&s, &c, &["add", "-A"]);
    git_ok(&s, &c, &["commit", "-q", "-m", "base code"]);
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);
    git_ok(&s, &c, &["checkout", "-q", "-b", "rv"]);
    let head = commit(&s, &c, "code.txt", "l1\nl2\nl3\nL4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\n", "edit code");
    git_ok(&s, &c, &["push", "-q", "origin", "rv"]);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "Review me", "head": "rv", "base": "main"}))).await;
    assert_eq!(st, 201);

    // Requested reviewers.
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/pulls/1/requested_reviewers"), Some(json!({"reviewers": [u.login]}))).await;
    assert_eq!(st, 422, "{e}");
    let (st, p) = s.call(Some(&u), "POST", &format!("{rp}/pulls/1/requested_reviewers"), Some(json!({"reviewers": [rev.login]}))).await;
    assert_eq!(st, 201, "{p}");
    assert_eq!(p["requested_reviewers"][0]["login"], rev.login.as_str());
    let (_, rr) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/requested_reviewers"), None).await;
    assert_eq!(rr["users"].as_array().unwrap().len(), 1);
    let ctx = forge::db::load_repo(&s.state.db, None, &u.login, &repo).await.unwrap();
    let pull = forge::pulls::pull_by_number(&s.state.db, ctx.repo.id, 1).await.unwrap();
    assert_eq!(forge::pulls::review_decision(&s.state.db, &pull).await.unwrap(), Some("REVIEW_REQUIRED"));

    // Single line comment: line 4 on the right side is "+L4".
    let (st, rc) = s
        .call(Some(&u), "POST", &format!("{rp}/pulls/1/comments"), Some(json!({"body": "why caps?", "commit_id": head, "path": "code.txt", "line": 4, "side": "RIGHT"})))
        .await;
    assert_eq!(st, 201, "{rc}");
    assert_eq!(rc["line"], 4);
    assert_eq!(rc["position"], 5);
    assert!(rc["diff_hunk"].as_str().unwrap().starts_with("@@ "));
    assert!(rc["diff_hunk"].as_str().unwrap().ends_with("+L4"));
    assert!(rc["pull_request_review_id"].is_i64());
    let id = rc["id"].as_i64().unwrap();
    let (st, e) = s
        .call(Some(&u), "POST", &format!("{rp}/pulls/1/comments"), Some(json!({"body": "x", "commit_id": head, "path": "code.txt", "line": 40})))
        .await;
    assert_eq!(st, 422, "{e}");
    let (st, reply) = s.call(Some(&rev), "POST", &format!("{rp}/pulls/1/comments/{id}/replies"), Some(json!({"body": "style"}))).await;
    assert_eq!(st, 201, "{reply}");
    assert_eq!(reply["in_reply_to_id"], id);
    assert_eq!(reply["line"], 4);
    let (st, edited) = s.call(Some(&u), "PATCH", &format!("{rp}/pulls/comments/{id}"), Some(json!({"body": "why CAPS?"}))).await;
    assert_eq!(st, 200);
    assert_eq!(edited["body"], "why CAPS?");
    let (st, _) = s.call(Some(&rev), "PATCH", &format!("{rp}/pulls/comments/{id}"), Some(json!({"body": "hijack"}))).await;
    assert_eq!(st, 200, "collaborator with push may edit");
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/comments"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/pulls/comments"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let rid = reply["id"].as_i64().unwrap();
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/pulls/comments/{rid}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/pulls/comments/{rid}"), None).await;
    assert_eq!(st, 404);

    // Author can't approve their own PR.
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/pulls/1/reviews"), Some(json!({"event": "APPROVE"}))).await;
    assert_eq!(st, 422, "{e}");
    // Review with inline comments (LEFT side: old line 4 "l4").
    let (st, r) = s
        .call(
            Some(&u),
            "POST",
            &format!("{rp}/pulls/1/reviews"),
            Some(json!({"body": "self review", "event": "COMMENT", "comments": [{"path": "code.txt", "line": 4, "side": "LEFT", "body": "old"}, {"path": "code.txt", "position": 1, "body": "pos"}]})),
        )
        .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["state"], "COMMENTED");
    let (_, rcs) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/reviews/{}/comments", r["id"]), None).await;
    let rcs = rcs.as_array().unwrap();
    assert_eq!(rcs.len(), 2);
    assert_eq!(rcs[0]["side"], "LEFT");
    assert_eq!(rcs[0]["position"], 4);
    assert_eq!(rcs[1]["line"], 1);

    // Pending review by the reviewer is hidden from others, then submitted as APPROVE.
    let (st, pending) = s
        .call(Some(&rev), "POST", &format!("{rp}/pulls/1/reviews"), Some(json!({"body": "looking", "comments": [{"path": "code.txt", "line": 11, "body": "new line"}]})))
        .await;
    assert_eq!(st, 200, "{pending}");
    assert_eq!(pending["state"], "PENDING");
    assert!(pending.get("submitted_at").is_none());
    let pid = pending["id"].as_i64().unwrap();
    let (_, others) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/reviews"), None).await;
    assert!(others.as_array().unwrap().iter().all(|r| r["id"] != pid));
    let (_, cms) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/comments"), None).await;
    assert_eq!(cms.as_array().unwrap().len(), 3);
    let (st, e) = s.call(Some(&rev), "POST", &format!("{rp}/pulls/1/reviews"), Some(json!({"body": "again"}))).await;
    assert_eq!(st, 422, "{e}");
    let (st, sub) = s.call(Some(&rev), "POST", &format!("{rp}/pulls/1/reviews/{pid}/events"), Some(json!({"event": "APPROVE", "body": "LGTM"}))).await;
    assert_eq!(st, 200, "{sub}");
    assert_eq!(sub["state"], "APPROVED");
    assert!(sub["submitted_at"].is_string());
    let (_, cms) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/comments"), None).await;
    assert_eq!(cms.as_array().unwrap().len(), 4);
    let pull = forge::pulls::pull_by_number(&s.state.db, ctx.repo.id, 1).await.unwrap();
    assert_eq!(forge::pulls::review_decision(&s.state.db, &pull).await.unwrap(), Some("APPROVED"));
    let (_, rr) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/requested_reviewers"), None).await;
    assert_eq!(rr["users"].as_array().unwrap().len(), 0);

    // Update body, dismiss, delete pending.
    let (st, upd) = s.call(Some(&rev), "PUT", &format!("{rp}/pulls/1/reviews/{pid}"), Some(json!({"body": "LGTM!"}))).await;
    assert_eq!(st, 200);
    assert_eq!(upd["body"], "LGTM!");
    let (st, d) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/1/reviews/{pid}/dismissals"), Some(json!({"message": "stale", "event": "DISMISS"}))).await;
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["state"], "DISMISSED");
    let pull = forge::pulls::pull_by_number(&s.state.db, ctx.repo.id, 1).await.unwrap();
    assert_eq!(forge::pulls::review_decision(&s.state.db, &pull).await.unwrap(), None);
    let (st, ch) = s.call(Some(&rev), "POST", &format!("{rp}/pulls/1/reviews"), Some(json!({"event": "REQUEST_CHANGES", "body": "nope"}))).await;
    assert_eq!(st, 200, "{ch}");
    let status = forge::pulls::pull_status(&s.state, &forge::pulls::pull_by_number(&s.state.db, ctx.repo.id, 1).await.unwrap()).await.unwrap();
    assert_eq!(status.review_decision, Some("CHANGES_REQUESTED"));
    assert_eq!(status.mergeable_enum, "MERGEABLE");
    let (st, p2) = s.call(Some(&u), "POST", &format!("{rp}/pulls/1/reviews"), Some(json!({"body": "draft"}))).await;
    assert_eq!(st, 200);
    let (st, del) = s.call(Some(&u), "DELETE", &format!("{rp}/pulls/1/reviews/{}", p2["id"]), None).await;
    assert_eq!(st, 200, "{del}");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/pulls/1/reviews/{}", r["id"]), None).await;
    assert_eq!(st, 422);
    let (_, reviews) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1/reviews"), None).await;
    // single comment (1) + reply review (1) + COMMENT review + approved/dismissed + changes requested
    assert_eq!(reviews.as_array().unwrap().len(), 5, "{reviews}");

    // Drafts: GraphQL-facing service functions.
    let actor = forge::db::user_by_login(&s.state.db, &u.login).await.unwrap();
    let p = forge::pulls::convert_to_draft(&s.state, &ctx, &actor, 1).await.unwrap();
    assert!(p.pr.draft);
    let (_, pr) = s.call(Some(&u), "GET", &format!("{rp}/pulls/1"), None).await;
    assert_eq!(pr["draft"], true);
    assert_eq!(pr["mergeable_state"], "draft");
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/1/merge"), None).await;
    assert_eq!(st, 405);
    let p = forge::pulls::mark_ready(&s.state, &ctx, &actor, 1).await.unwrap();
    assert!(!p.pr.draft);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_cross_repo_pull_request() {
    let s = TestServer::start().await;
    let (owner, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", owner.login, repo);
    let contrib = s.user().await;
    // Make a fork: a repo of `contrib` with fork_parent_id, holding the base's history.
    let fork = s.create_repo(&contrib, false).await;
    let base_id = s.call(Some(&owner), "GET", &rp, None).await.1["id"].as_i64().unwrap();
    sqlx::query("UPDATE repositories SET fork_parent_id = $2 WHERE id = (SELECT r.id FROM repositories r JOIN users u ON u.id = r.owner_id WHERE u.login = $1 AND r.name = $3)")
        .bind(&contrib.login)
        .bind(base_id)
        .bind(&fork)
        .execute(&s.state.db)
        .await
        .unwrap();
    git_ok(&s, &c, &["remote", "add", "fork", &s.clone_url(&contrib, &fork)]);
    git_ok(&s, &c, &["push", "-q", "fork", "main"]);
    git_ok(&s, &c, &["checkout", "-q", "-b", "contrib"]);
    let head = commit(&s, &c, "contrib.txt", "from a fork\n", "fork change");
    git_ok(&s, &c, &["push", "-q", "fork", "contrib"]);

    let head_spec = format!("{}:contrib", contrib.login);
    let (st, pr) = s
        .call(Some(&contrib), "POST", &format!("{rp}/pulls"), Some(json!({"title": "From fork", "head": head_spec, "base": "main", "maintainer_can_modify": true})))
        .await;
    assert_eq!(st, 201, "{pr}");
    assert_eq!(pr["head"]["repo"]["full_name"], format!("{}/{}", contrib.login, fork));
    assert_eq!(pr["head"]["label"], head_spec);
    assert_eq!(pr["head"]["sha"], head);
    assert_eq!(pr["maintainer_can_modify"], true);
    assert_eq!(pr["mergeable"], true);
    let (_, files) = s.call(Some(&owner), "GET", &format!("{rp}/pulls/1/files"), None).await;
    assert_eq!(files[0]["filename"], "contrib.txt");

    // The base repo serves the head commit under refs/pull/1/head.
    git_ok(&s, &c, &["fetch", "-q", "origin", "pull/1/head:fromfork"]);
    assert_eq!(git_ok(&s, &c, &["rev-parse", "fromfork"]).trim(), head);

    // A push to the fork updates the PR in the base repo.
    let head2 = commit(&s, &c, "contrib.txt", "from a fork, v2\n", "fork change 2");
    git_ok(&s, &c, &["push", "-q", "fork", "contrib"]);
    let (_, pr) = s.call(Some(&owner), "GET", &format!("{rp}/pulls/1"), None).await;
    assert_eq!(pr["head"]["sha"], head2);
    assert_eq!(pr["commits"], 2);

    // Simulate another pod (no local cache) and merge.
    std::fs::remove_dir_all(s.dir.path().join("cache")).unwrap();
    std::fs::create_dir_all(s.dir.path().join("cache")).unwrap();
    let (st, e) = s.call(Some(&contrib), "PUT", &format!("{rp}/pulls/1/merge"), None).await;
    assert_eq!(st, 403, "{e}");
    let (st, m) = s.call(Some(&owner), "PUT", &format!("{rp}/pulls/1/merge"), Some(json!({"merge_method": "merge"}))).await;
    assert_eq!(st, 200, "{m}");
    let w = s.work_dir("verify");
    git_ok(&s, &w, &["clone", "-q", &s.clone_url(&owner, &repo), "v"]);
    assert_eq!(std::fs::read_to_string(w.join("v/contrib.txt")).unwrap(), "from a fork, v2\n");
    let (_, list) = s.call(Some(&owner), "GET", &format!("{rp}/pulls?state=closed&head={}", contrib.login), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}
