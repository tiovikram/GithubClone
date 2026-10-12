//! Gaps: pull request `rebaseable` accuracy and review comments following the diff.
mod common;
use std::path::{Path, PathBuf};

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

fn write(dir: &Path, file: &str, content: &str) {
    std::fs::write(dir.join(file), content).unwrap();
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

async fn setup(s: &TestServer) -> (TestUser, String, PathBuf) {
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let w = s.work_dir(&unique("w"));
    git_ok(s, &w, &["clone", "-q", &s.clone_url(&u, &repo), "c"]);
    (u, repo, w.join("c"))
}

/// Polls the PR until mergeability is computed for `head`.
async fn computed(s: &TestServer, u: &TestUser, path: &str, head: &str) -> Value {
    for _ in 0..100 {
        let (st, pr) = s.call(Some(u), "GET", path, None).await;
        assert_eq!(st, 200, "{pr}");
        if !pr["mergeable"].is_null() && pr["head"]["sha"] == head {
            return pr;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("mergeable never computed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rebaseable_reflects_replay_conflicts() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    commit(&s, &c, "f.txt", "x\n", "base f");
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);

    // feature: x -> y -> x (net no change to f.txt) plus a new file.
    git_ok(&s, &c, &["checkout", "-q", "-b", "feature"]);
    commit(&s, &c, "f.txt", "y\n", "to y");
    commit(&s, &c, "f.txt", "x\n", "back to x");
    let head = commit(&s, &c, "g.txt", "g\n", "add g");
    git_ok(&s, &c, &["push", "-q", "origin", "feature"]);
    // A clean PR first: replay works.
    let (st, pr) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "t", "head": "feature", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    let n = pr["number"].as_i64().unwrap();
    let pr = computed(&s, &u, &format!("{rp}/pulls/{n}"), &head).await;
    assert_eq!(pr["mergeable"], true);
    assert_eq!(pr["rebaseable"], true);

    // main: x -> z. The three-way merge is clean (feature's net change to f.txt is nil), but
    // replaying "to y" onto z conflicts, so the PR is mergeable yet not rebaseable.
    git_ok(&s, &c, &["checkout", "-q", "main"]);
    commit(&s, &c, "f.txt", "z\n", "to z");
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);
    let mut pr = Value::Null;
    for _ in 0..100 {
        let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{n}"), None).await;
        if p["rebaseable"] == false {
            pr = p;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(pr["mergeable"], true, "{pr}");
    assert_eq!(pr["rebaseable"], false, "{pr}");
    // And a rebase merge is refused while a merge commit works.
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/{n}/merge"), Some(json!({"merge_method": "rebase"}))).await;
    assert_eq!(st, 405, "{e}");
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/pulls/{n}/merge"), Some(json!({"merge_method": "merge"}))).await;
    assert_eq!(st, 200, "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_comments_follow_head_and_outdate() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let lines: Vec<String> = (1..=12).map(|i| format!("l{i}")).collect();
    commit(&s, &c, "f.txt", &(lines.join("\n") + "\n"), "base f");
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);

    git_ok(&s, &c, &["checkout", "-q", "-b", "feature"]);
    let mut v1 = lines.clone();
    v1[2] = "L3".into();
    v1[9] = "L10".into();
    let head1 = commit(&s, &c, "f.txt", &(v1.join("\n") + "\n"), "edit 3 and 10");
    git_ok(&s, &c, &["push", "-q", "origin", "feature"]);
    let (st, pr) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "t", "head": "feature", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    let n = pr["number"].as_i64().unwrap();
    computed(&s, &u, &format!("{rp}/pulls/{n}"), &head1).await;

    let mut ids = vec![];
    for line in [3, 10] {
        let (st, cm) = s
            .call(Some(&u), "POST", &format!("{rp}/pulls/{n}/comments"), Some(json!({"body": format!("on {line}"), "commit_id": head1, "path": "f.txt", "line": line, "side": "RIGHT"})))
            .await;
        assert_eq!(st, 201, "{cm}");
        assert_eq!(cm["line"], line);
        assert_eq!(cm["original_line"], line);
        ids.push(cm["id"].as_i64().unwrap());
    }

    // Insert two lines at the top (moves line 3 to 5) and rewrite line 10 (outdates it).
    let mut v2 = vec!["n1".to_string(), "n2".to_string()];
    v2.extend(v1.iter().cloned());
    v2[11] = "TEN".into();
    let head2 = commit(&s, &c, "f.txt", &(v2.join("\n") + "\n"), "move and rewrite");
    git_ok(&s, &c, &["push", "-q", "origin", "feature"]);
    computed(&s, &u, &format!("{rp}/pulls/{n}"), &head2).await;

    let get = |id: i64| {
        let s = &s;
        let u = &u;
        let rp = rp.clone();
        async move {
            let (st, cm) = s.call(Some(u), "GET", &format!("{rp}/pulls/comments/{id}"), None).await;
            assert_eq!(st, 200, "{cm}");
            cm
        }
    };
    let moved = get(ids[0]).await;
    assert_eq!(moved["line"], 5, "{moved}");
    assert_eq!(moved["original_line"], 3);
    assert_eq!(moved["commit_id"], head2);
    assert_eq!(moved["original_commit_id"], head1);
    assert!(moved["position"].is_i64(), "{moved}");
    assert!(moved["original_position"].is_i64());

    let outdated = get(ids[1]).await;
    assert!(outdated.get("line").is_none_or(Value::is_null), "{outdated}");
    assert!(outdated.get("position").is_none_or(Value::is_null), "{outdated}");
    assert_eq!(outdated["original_line"], 10);
    assert_eq!(outdated["original_commit_id"], head1);
    assert_eq!(outdated["commit_id"], head1);

    // The list endpoint agrees.
    let (st, list) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{n}/comments"), None).await;
    assert_eq!(st, 200);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.iter().find(|c| c["id"] == ids[0]).unwrap()["line"], 5);
}
