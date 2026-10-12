//! Gaps: slash-containing branch names in paths, signed archive redirects for private repos,
//! branch protection enforcement (push + API + PR merge), cross-fork compare.
mod common;
use std::path::{Path, PathBuf};

use base64::Engine;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn git_ok(s: &TestServer, dir: &Path, args: &[&str]) -> String {
    let (ok, out) = s.git(dir, args);
    assert!(ok, "git {args:?} failed: {out}");
    out
}

fn commit(s: &TestServer, dir: &Path, file: &str, content: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), content).unwrap();
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

fn protection(extra: Value) -> Value {
    let mut v = json!({"required_status_checks": null, "enforce_admins": false, "required_pull_request_reviews": null, "restrictions": null});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_branch_names_in_paths() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    git_ok(&s, &c, &["checkout", "-q", "-b", "feature/x"]);
    let sha = commit(&s, &c, "f.txt", "f\n", "feature");
    git_ok(&s, &c, &["push", "-q", "origin", "feature/x"]);
    git_ok(&s, &c, &["tag", "rel/1.0"]);
    git_ok(&s, &c, &["push", "-q", "origin", "rel/1.0"]);

    for path in ["branches/feature/x", "branches/feature%2Fx", "branches/feature/x/"] {
        let (st, b) = s.call(Some(&u), "GET", &format!("{rp}/{path}"), None).await;
        assert_eq!(st, 200, "{path}: {b}");
        assert_eq!(b["name"], "feature/x");
        assert_eq!(b["commit"]["sha"], sha);
        assert_eq!(b["protected"], false);
    }
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/branches/feature/nope"), None).await;
    assert_eq!(st, 404);

    // Commits and their sub-resources.
    for r in ["feature/x", "feature%2Fx", "rel/1.0"] {
        let (st, c) = s.call(Some(&u), "GET", &format!("{rp}/commits/{r}"), None).await;
        assert_eq!(st, 200, "{r}: {c}");
        assert_eq!(c["sha"], sha);
    }
    let (st, cs) = s.call(Some(&u), "POST", &format!("{rp}/statuses/{sha}"), Some(json!({"state": "success", "context": "ci"}))).await;
    assert_eq!(st, 201, "{cs}");
    let (st, cs) = s.call(Some(&u), "GET", &format!("{rp}/commits/feature/x/status"), None).await;
    assert_eq!(st, 200, "{cs}");
    assert_eq!(cs["sha"], sha);
    assert_eq!(cs["state"], "success");
    let (st, l) = s.call(Some(&u), "GET", &format!("{rp}/commits/feature/x/statuses"), None).await;
    assert_eq!(st, 200);
    assert_eq!(l.as_array().unwrap().len(), 1);
    for sub in ["check-runs", "check-suites", "comments", "pulls", "branches-where-head"] {
        let (st, v) = s.call(Some(&u), "GET", &format!("{rp}/commits/feature/x/{sub}"), None).await;
        assert_eq!(st, 200, "{sub}: {v}");
    }
    let (st, v) = s.call(Some(&u), "GET", &format!("{rp}/commits/{sha}/branches-where-head"), None).await;
    assert_eq!(st, 200);
    assert_eq!(v[0]["name"], "feature/x");

    // Protection and its sub-resources.
    let (st, p) = s.call(Some(&u), "PUT", &format!("{rp}/branches/feature/x/protection"), Some(protection(json!({"enforce_admins": true})))).await;
    assert_eq!(st, 200, "{p}");
    let (st, p) = s.call(Some(&u), "GET", &format!("{rp}/branches/feature/x/protection/enforce_admins"), None).await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["enabled"], true);
    let (_, b) = s.call(Some(&u), "GET", &format!("{rp}/branches/feature/x"), None).await;
    assert_eq!(b["protected"], true);
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/branches?protected=true"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/branches/feature/x/protection"), None).await;
    assert_eq!(st, 204);

    // Rename and compare.
    let (st, b) = s.call(Some(&u), "POST", &format!("{rp}/branches/feature/x/rename"), Some(json!({"new_name": "feature/y"}))).await;
    assert_eq!(st, 201, "{b}");
    assert_eq!(b["name"], "feature/y");
    let (st, cmp) = s.call(Some(&u), "GET", &format!("{rp}/compare/main...feature/y"), None).await;
    assert_eq!(st, 200, "{cmp}");
    assert_eq!(cmp["ahead_by"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_archive_redirect_is_signed() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = unique("r");
    let (st, _) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": repo, "auto_init": true, "private": true}))).await;
    assert_eq!(st, 201);
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    for (kind, ext) in [("tarball", "tar.gz"), ("zipball", "zip")] {
        let res = client
            .get(format!("{}/repos/{}/{repo}/{kind}/main", s.api, u.login))
            .header("Authorization", format!("token {}", u.token))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 302);
        let loc = res.headers()["location"].to_str().unwrap().to_string();
        assert!(loc.contains(&format!("/archive/main.{ext}?token=")), "{loc}");
        // No credentials: the signed URL works, a tampered or missing token does not.
        let res = reqwest::get(&loc).await.unwrap();
        assert_eq!(res.status(), 200);
        assert!(!res.bytes().await.unwrap().is_empty());
        let bad = format!("{}0", &loc[..loc.len() - 1]);
        assert_eq!(reqwest::get(&bad).await.unwrap().status(), 404);
        let bare = loc.split('?').next().unwrap();
        assert_eq!(reqwest::get(bare).await.unwrap().status(), 404);
        // The token is bound to the ref.
        let other = loc.replace("/archive/main.", "/archive/HEAD.");
        assert_eq!(reqwest::get(&other).await.unwrap().status(), 404);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protection_enforced_on_push_and_api() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let base = git_ok(&s, &c, &["rev-parse", "HEAD"]).trim().to_string();

    // Default protection: no force pushes, no deletions.
    let (st, p) = s.call(Some(&u), "PUT", &format!("{rp}/branches/main/protection"), Some(protection(json!({})))).await;
    assert_eq!(st, 200, "{p}");
    let (_, b) = s.call(Some(&u), "GET", &format!("{rp}/branches/main"), None).await;
    assert_eq!(b["protected"], true);
    assert_eq!(b["protection"]["enabled"], true);

    let ff = commit(&s, &c, "a.txt", "a\n", "a");
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);
    git_ok(&s, &c, &["reset", "-q", "--hard", &base]);
    commit(&s, &c, "b.txt", "b\n", "b");
    let (ok, out) = s.git(&c, &["push", "-f", "origin", "main"]);
    assert!(!ok);
    assert!(out.contains("GH006: Protected branch update failed for refs/heads/main."), "{out}");
    assert!(out.contains("Cannot force-push to this branch"), "{out}");
    let (ok, out) = s.git(&c, &["push", "origin", ":main"]);
    assert!(!ok);
    assert!(out.contains("Cannot delete this branch"), "{out}");
    let (_, b) = s.call(Some(&u), "GET", &format!("{rp}/branches/main"), None).await;
    assert_eq!(b["commit"]["sha"], ff);

    // API: force update and delete rejected.
    let (st, e) = s.call(Some(&u), "PATCH", &format!("{rp}/git/refs/heads/main"), Some(json!({"sha": base, "force": true}))).await;
    assert_eq!(st, 422, "{e}");
    assert_eq!(e["message"], "Cannot force-push to this branch");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/git/refs/heads/main"), None).await;
    assert_eq!(st, 422);

    // Pull requests required, enforced for admins: direct pushes and contents writes fail.
    let (st, _) = s
        .call(
            Some(&u),
            "PUT",
            &format!("{rp}/branches/main/protection"),
            Some(protection(json!({"enforce_admins": true, "required_pull_request_reviews": {"required_approving_review_count": 1}}))),
        )
        .await;
    assert_eq!(st, 200);
    git_ok(&s, &c, &["reset", "-q", "--hard", &ff]);
    commit(&s, &c, "c.txt", "c\n", "c");
    let (ok, out) = s.git(&c, &["push", "origin", "main"]);
    assert!(!ok);
    assert!(out.contains("Changes must be made through a pull request."), "{out}");
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/contents/d.txt"), Some(json!({"message": "d", "content": b64("d")}))).await;
    assert_eq!(st, 409, "{e}");
    // Other branches are unaffected.
    git_ok(&s, &c, &["push", "-q", "origin", "HEAD:other"]);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/d.txt"), Some(json!({"message": "d", "content": b64("d"), "branch": "other"}))).await;
    assert_eq!(st, 201);

    // Admins bypass review requirements when enforce_admins is off.
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/branches/main/protection/enforce_admins"), None).await;
    assert_eq!(st, 204);
    git_ok(&s, &c, &["push", "-q", "origin", "main"]);

    // Linear history.
    let (st, _) = s
        .call(Some(&u), "PUT", &format!("{rp}/branches/main/protection"), Some(protection(json!({"required_linear_history": true}))))
        .await;
    assert_eq!(st, 200);
    git_ok(&s, &c, &["checkout", "-q", "-b", "side", "HEAD~1"]);
    commit(&s, &c, "s.txt", "s\n", "side");
    git_ok(&s, &c, &["checkout", "-q", "main"]);
    git_ok(&s, &c, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let (ok, out) = s.git(&c, &["push", "origin", "main"]);
    assert!(!ok);
    assert!(out.contains("This branch must not contain merge commits."), "{out}");

    // GraphQL exposes the rule.
    let q = format!(
        "{{ repository(owner: \"{}\", name: \"{repo}\") {{ ref(qualifiedName: \"refs/heads/main\") {{ branchProtectionRule {{ pattern requiresLinearHistory allowsForcePushes }} }} }} }}",
        u.login
    );
    let res = s
        .client()
        .post(format!("{}/api/graphql", s.base))
        .header("Authorization", format!("token {}", u.token))
        .json(&json!({"query": q}))
        .send()
        .await
        .unwrap();
    let v: Value = res.json().await.unwrap();
    let rule = &v["data"]["repository"]["ref"]["branchProtectionRule"];
    assert_eq!(rule["pattern"], "main", "{v}");
    assert_eq!(rule["requiresLinearHistory"], true);
    assert_eq!(rule["allowsForcePushes"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protection_gates_pull_request_merges() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let reviewer = s.user().await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/collaborators/{}", reviewer.login), Some(json!({"permission": "push"}))).await;
    assert!(st == 201 || st == 204, "{st}");
    git_ok(&s, &c, &["checkout", "-q", "-b", "topic"]);
    let head = commit(&s, &c, "t.txt", "t\n", "topic");
    git_ok(&s, &c, &["push", "-q", "origin", "topic"]);
    let (st, pr) = s.call(Some(&u), "POST", &format!("{rp}/pulls"), Some(json!({"title": "t", "head": "topic", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    let n = pr["number"].as_i64().unwrap();
    let (st, _) = s
        .call(
            Some(&u),
            "PUT",
            &format!("{rp}/branches/main/protection"),
            Some(protection(json!({
                "enforce_admins": true,
                "required_pull_request_reviews": {"required_approving_review_count": 1},
                "required_status_checks": {"strict": true, "contexts": ["ci"]},
                "required_linear_history": true,
            }))),
        )
        .await;
    assert_eq!(st, 200);

    let merge = |method: &'static str| {
        let s = &s;
        let u = &u;
        let rp = rp.clone();
        async move { s.call(Some(u), "PUT", &format!("{rp}/pulls/{n}/merge"), Some(json!({"merge_method": method}))).await }
    };
    let (st, e) = merge("merge").await;
    assert_eq!(st, 405, "{e}");
    assert_eq!(e["message"], "Merge commits are not allowed on this protected branch.");
    let (st, e) = merge("squash").await;
    assert_eq!(st, 405, "{e}");
    assert_eq!(e["message"], "At least 1 approving review is required by reviewers with write access.");
    for _ in 0..50 {
        let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/pulls/{n}"), None).await;
        if !p["mergeable"].is_null() {
            assert_eq!(p["mergeable_state"], "blocked", "{p}");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let (st, r) = s.call(Some(&reviewer), "POST", &format!("{rp}/pulls/{n}/reviews"), Some(json!({"event": "APPROVE"}))).await;
    assert_eq!(st, 200, "{r}");
    let (st, e) = merge("squash").await;
    assert_eq!(st, 405);
    assert_eq!(e["message"], "Required status check \"ci\" is expected.");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/statuses/{head}"), Some(json!({"state": "success", "context": "ci"}))).await;
    assert_eq!(st, 201);
    let (st, m) = merge("squash").await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["merged"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compare_across_forks() {
    let s = TestServer::start().await;
    let (owner, repo, _) = setup(&s).await;
    let p = format!("/repos/{}/{repo}", owner.login);
    let forker = s.user().await;
    let (st, f) = s.call(Some(&forker), "POST", &format!("{p}/forks"), Some(json!({}))).await;
    assert_eq!(st, 202, "{f}");
    let fork_name = f["name"].as_str().unwrap().to_string();
    let w = s.work_dir("fork");
    git_ok(&s, &w, &["clone", "-q", &s.clone_url(&forker, &fork_name), "c"]);
    let c = w.join("c");
    git_ok(&s, &c, &["checkout", "-q", "-b", "feat/fork"]);
    let sha = commit(&s, &c, "fork.txt", "fork\n", "fork change");
    git_ok(&s, &c, &["push", "-q", "origin", "feat/fork"]);

    for spec in [format!("main...{}:feat/fork", forker.login), format!("main...{}:{fork_name}:feat/fork", forker.login)] {
        let (st, cmp) = s.call(Some(&owner), "GET", &format!("{p}/compare/{spec}"), None).await;
        assert_eq!(st, 200, "{spec}: {cmp}");
        assert_eq!(cmp["ahead_by"], 1);
        assert_eq!(cmp["behind_by"], 0);
        assert_eq!(cmp["status"], "ahead");
        assert_eq!(cmp["commits"][0]["sha"], sha);
        assert_eq!(cmp["files"][0]["filename"], "fork.txt");
    }
    // Reverse direction from the fork's point of view.
    let (st, cmp) = s.call(Some(&forker), "GET", &format!("/repos/{}/{fork_name}/compare/{}:main...feat/fork", forker.login, owner.login), None).await;
    assert_eq!(st, 200, "{cmp}");
    assert_eq!(cmp["ahead_by"], 1);
    // Diff media type.
    let res = s
        .client()
        .get(format!("{}{p}/compare/main...{}:feat/fork", s.api, forker.login))
        .header("Authorization", format!("token {}", owner.token))
        .header("Accept", "application/vnd.github.diff")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.text().await.unwrap().contains("+fork"));
    // Unrelated owners are not part of the network.
    let stranger = s.user().await;
    let (st, _) = s.call(Some(&owner), "GET", &format!("{p}/compare/main...{}:feat/fork", stranger.login), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protection_enforced_on_ssh_push() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let s = TestServer::start_with(|c| {
        c.ssh_listen = Some(format!("127.0.0.1:{port}"));
        c.ssh_url_host = Some("127.0.0.1".into());
    })
    .await;
    forge::ssh::serve_listener(s.state.clone(), listener).await.unwrap();
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let keys = s.work_dir("keys");
    let key = keys.join("id");
    let out = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "k", "-f"]).arg(&key).output().unwrap();
    assert!(out.status.success());
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let (st, _) = s.call(Some(&u), "POST", "/user/keys", Some(json!({"title": "k", "key": public.trim()}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/branches/main/protection"), Some(protection(json!({})))).await;
    assert_eq!(st, 200);

    let ssh = format!(
        "ssh -F /dev/null -i {} -o IdentitiesOnly=yes -o IdentityAgent=none -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
        key.display()
    );
    let url = format!("ssh://git@127.0.0.1:{port}/{}/{repo}.git", u.login);
    let push = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&c)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_SSH_COMMAND", &ssh)
            .env_remove("SSH_AUTH_SOCK")
            .output()
            .unwrap();
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let base = git_ok(&s, &c, &["rev-parse", "HEAD"]).trim().to_string();
    commit(&s, &c, "a.txt", "a\n", "a");
    let (ok, out) = push(&["push", &url, "HEAD:main"]);
    assert!(ok, "{out}");
    git_ok(&s, &c, &["reset", "-q", "--hard", &base]);
    commit(&s, &c, "b.txt", "b\n", "b");
    let (ok, out) = push(&["push", "-f", &url, "HEAD:main"]);
    assert!(!ok, "{out}");
    assert!(out.contains("GH006: Protected branch update failed for refs/heads/main."), "{out}");
    let (ok, out) = push(&["push", &url, ":main"]);
    assert!(!ok);
    assert!(out.contains("Cannot delete this branch"), "{out}");
}
