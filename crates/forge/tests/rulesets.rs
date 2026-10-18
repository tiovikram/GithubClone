//! Ruleset enforcement (crate::rules): pushes over HTTP (GH013), enforcement modes, bypass
//! actors, push rulesets with delegated bypass requests, API writes, PR merges, signatures.
mod common;
use std::path::{Path, PathBuf};

use base64::Engine;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

fn git_ok(s: &TestServer, dir: &Path, args: &[&str]) -> String {
    let (ok, out) = s.git(dir, args);
    assert!(ok, "git {args:?} failed: {out}");
    out
}

fn commit(s: &TestServer, dir: &Path, file: &str, content: &[u8], msg: &str) -> String {
    let p = dir.join(file);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
    git_ok(s, dir, &["add", "-A"]);
    git_ok(s, dir, &["commit", "-q", "-m", msg]);
    git_ok(s, dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn url_for(s: &TestServer, who: &TestUser, owner: &str, repo: &str) -> String {
    format!("http://{}:{}@{}/{owner}/{repo}.git", who.login, who.token, s.base.trim_start_matches("http://"))
}

async fn setup(s: &TestServer) -> (TestUser, String, PathBuf) {
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let w = s.work_dir(&unique("w"));
    git_ok(s, &w, &["clone", "-q", &s.clone_url(&u, &repo), "c"]);
    (u, repo, w.join("c"))
}

async fn ruleset(s: &TestServer, u: &TestUser, rp: &str, body: Value) -> i64 {
    let (st, r) = s.call(Some(u), "POST", &format!("{rp}/rulesets"), Some(body)).await;
    assert_eq!(st, 201, "{r}");
    r["id"].as_i64().unwrap()
}

async fn suites(s: &TestServer, u: &TestUser, rp: &str) -> Vec<Value> {
    let (st, v) = s.call(Some(u), "GET", &format!("{rp}/rulesets/rule-suites"), None).await;
    assert_eq!(st, 200, "{v}");
    v.as_array().unwrap().clone()
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn branch_rules_block_pushes_with_gh013() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let id = ruleset(&s, &u, &rp, json!({
        "name": "main-rules", "target": "branch", "enforcement": "active",
        "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
        "rules": [{"type": "deletion"}, {"type": "non_fast_forward"},
                  {"type": "commit_message_pattern", "parameters": {"operator": "starts_with", "pattern": "feat", "negate": false}}]})).await;

    // Message pattern: rejected with GH013 and the rule's details.
    commit(&s, &c, "a.txt", b"a\n", "wip");
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:main"]);
    assert!(!ok, "{out}");
    assert!(out.contains("GH013: Repository rule violations found for refs/heads/main"), "{out}");
    assert!(out.contains("Commit message must start with a given pattern: feat"), "{out}");
    let all = suites(&s, &u, &rp).await;
    assert_eq!(all[0]["result"], "fail", "{all:?}");
    let (_, one) = s.call(Some(&u), "GET", &format!("{rp}/rulesets/rule-suites/{}", all[0]["id"]), None).await;
    assert_eq!(one["evaluation_result"], Value::Null, "{one}");
    let evals = one["rule_evaluations"].as_array().unwrap();
    assert!(evals.iter().any(|e| e["rule_type"] == "commit_message_pattern" && e["result"] == "fail"), "{one}");
    assert!(evals.iter().any(|e| e["rule_type"] == "deletion" && e["result"] == "pass"), "{one}");

    // A conforming commit goes through.
    git_ok(&s, &c, &["commit", "-q", "--amend", "-m", "feat: a"]);
    git_ok(&s, &c, &["push", "-q", "origin", "HEAD:main"]);
    assert_eq!(suites(&s, &u, &rp).await[0]["result"], "pass");

    // Force push and deletion are blocked (even for the repository admin: no bypass actors).
    git_ok(&s, &c, &["reset", "-q", "--hard", "HEAD~1"]);
    commit(&s, &c, "b.txt", b"b\n", "feat: b");
    let (ok, out) = s.git(&c, &["push", "-f", "origin", "HEAD:main"]);
    assert!(!ok && out.contains("Cannot force-push to this branch"), "{out}");
    git_ok(&s, &c, &["push", "-q", "origin", "HEAD:other"]);
    let (ok, out) = s.git(&c, &["push", "origin", ":main"]);
    assert!(!ok, "{out}");
    // (deleting the default branch is refused by git itself or by the rule)

    // Evaluate mode: pushes pass, the failure is recorded.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/rulesets/{id}"), Some(json!({"enforcement": "evaluate"}))).await;
    assert_eq!(st, 200);
    git_ok(&s, &c, &["fetch", "-q", "origin"]);
    git_ok(&s, &c, &["reset", "-q", "--hard", "origin/main"]);
    commit(&s, &c, "c.txt", b"c\n", "nope");
    git_ok(&s, &c, &["push", "-q", "origin", "HEAD:main"]);
    let latest = &suites(&s, &u, &rp).await[0];
    assert_eq!((latest["result"].as_str(), latest["evaluation_result"].as_str()), (Some("pass"), Some("fail")), "{latest}");

    // Disabled: nothing recorded.
    let before = suites(&s, &u, &rp).await.len();
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/rulesets/{id}"), Some(json!({"enforcement": "disabled"}))).await;
    assert_eq!(st, 200);
    commit(&s, &c, "d.txt", b"d\n", "nope again");
    git_ok(&s, &c, &["push", "-q", "origin", "HEAD:main"]);
    assert_eq!(suites(&s, &u, &rp).await.len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bypass_actors_and_api_writes() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let collab = s.user().await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/collaborators/{}", collab.login), Some(json!({"permission": "push"}))).await;
    assert!(st == 201 || st == 204, "{st}");
    let id = ruleset(&s, &u, &rp, json!({
        "name": "pr-only", "target": "branch", "enforcement": "active",
        "conditions": {"ref_name": {"include": ["refs/heads/main"], "exclude": []}},
        "bypass_actors": [{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}],
        "rules": [{"type": "pull_request", "parameters": {"required_approving_review_count": 0, "dismiss_stale_reviews_on_push": false,
            "require_code_owner_review": false, "require_last_push_approval": false, "required_review_thread_resolution": false}}]})).await;
    let (_, rs) = s.call(Some(&u), "GET", &format!("{rp}/rulesets/{id}"), None).await;
    assert_eq!(rs["current_user_can_bypass"], "always", "{rs}");
    let (_, rs) = s.call(Some(&collab), "GET", &format!("{rp}/rulesets/{id}"), None).await;
    assert_eq!(rs["current_user_can_bypass"], "never", "{rs}");

    // The admin bypasses (the push says so; the suite says bypass).
    commit(&s, &c, "a.txt", b"a\n", "direct");
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:main"]);
    assert!(ok, "{out}");
    assert!(out.contains("Bypassed rule violations for refs/heads/main"), "{out}");
    assert_eq!(suites(&s, &u, &rp).await[0]["result"], "bypass");

    // A writer is blocked over git and over the contents API (409).
    let w = s.work_dir(&unique("w"));
    git_ok(&s, &w, &["clone", "-q", &url_for(&s, &collab, &u.login, &repo), "c"]);
    let c2 = w.join("c");
    commit(&s, &c2, "b.txt", b"b\n", "direct too");
    let (ok, out) = s.git(&c2, &["push", "origin", "HEAD:main"]);
    assert!(!ok && out.contains("Changes must be made through a pull request."), "{out}");
    let (st, e) = s.call(Some(&collab), "PUT", &format!("{rp}/contents/x.txt"), Some(json!({"message": "x", "content": b64(b"x")}))).await;
    assert_eq!(st, 409, "{e}");
    assert!(e["message"].as_str().unwrap().starts_with("Repository rule violations found"), "{e}");

    // ... but can merge a pull request (the pull_request rule is satisfied by merging one).
    git_ok(&s, &c2, &["push", "-q", "origin", "HEAD:topic"]);
    let (st, pr) = s.call(Some(&collab), "POST", &format!("{rp}/pulls"), Some(json!({"title": "t", "head": "topic", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    let n = pr["number"].as_i64().unwrap();
    // Requiring an approval blocks the merge (405) and the PR shows as blocked.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/rulesets/{id}"), Some(json!({"rules": [{"type": "pull_request", "parameters": {
        "required_approving_review_count": 1, "dismiss_stale_reviews_on_push": false, "require_code_owner_review": false,
        "require_last_push_approval": false, "required_review_thread_resolution": false}}]}))).await;
    assert_eq!(st, 200);
    let (st, e) = s.call(Some(&collab), "PUT", &format!("{rp}/pulls/{n}/merge"), Some(json!({}))).await;
    assert_eq!(st, 405, "{e}");
    assert!(e["message"].as_str().unwrap().contains("approving review"), "{e}");
    let (st, rv) = s.call(Some(&u), "POST", &format!("{rp}/pulls/{n}/reviews"), Some(json!({"event": "APPROVE"}))).await;
    assert_eq!(st, 200, "{rv}");
    let (st, m) = s.call(Some(&collab), "PUT", &format!("{rp}/pulls/{n}/merge"), Some(json!({"merge_method": "squash"}))).await;
    assert_eq!(st, 200, "{m}");

    // Tag rules through the git refs API (422).
    let id2 = ruleset(&s, &u, &rp, json!({
        "name": "tags", "target": "tag", "enforcement": "active",
        "conditions": {"ref_name": {"include": ["~ALL"], "exclude": []}},
        "rules": [{"type": "tag_name_pattern", "parameters": {"operator": "regex", "pattern": "^v[0-9]+", "negate": false}}]})).await;
    let sha = m["sha"].as_str().unwrap();
    let (st, e) = s.call(Some(&u), "POST", &format!("{rp}/git/refs"), Some(json!({"ref": "refs/tags/bad", "sha": sha}))).await;
    assert_eq!(st, 422, "{e}");
    assert!(e["message"].as_str().unwrap().contains("Tag name must match a given pattern"), "{e}");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/git/refs"), Some(json!({"ref": "refs/tags/v1", "sha": sha}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/rulesets/{id2}"), None).await;
    assert_eq!(st, 204);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_rulesets_and_delegated_bypass() {
    let s = TestServer::start().await;
    let (u, repo, _c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let collab = s.user().await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/collaborators/{}", collab.login), Some(json!({"permission": "push"}))).await;
    assert!(st == 201 || st == 204);
    ruleset(&s, &u, &rp, json!({
        "name": "push-rules", "target": "push", "enforcement": "active",
        "bypass_actors": [{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}],
        "rules": [{"type": "file_path_restriction", "parameters": {"restricted_file_paths": ["secrets/**"]}},
                  {"type": "file_extension_restriction", "parameters": {"restricted_file_extensions": ["*.exe"]}},
                  {"type": "max_file_path_length", "parameters": {"max_file_path_length": 40}},
                  {"type": "max_file_size", "parameters": {"max_file_size": 1}}]})).await;
    let w = s.work_dir(&unique("w"));
    git_ok(&s, &w, &["clone", "-q", &url_for(&s, &collab, &u.login, &repo), "c"]);
    let c = w.join("c");

    // Each push rule rejects (on any branch).
    for (file, content) in [("secrets/key.txt", b"k".to_vec()), ("tool.exe", b"MZ".to_vec()), ("big.bin", vec![b'x'; 1_200_000])] {
        git_ok(&s, &c, &["checkout", "-q", "-B", "work", "origin/main"]);
        commit(&s, &c, file, &content, "add");
        let (ok, out) = s.git(&c, &["push", "origin", "HEAD:work"]);
        assert!(!ok, "{file}: {out}");
        assert!(out.contains("GH013"), "{out}");
        assert!(out.contains(file), "{file}: {out}");
    }
    let long = format!("{}/x.txt", "d".repeat(45));
    git_ok(&s, &c, &["checkout", "-q", "-B", "work", "origin/main"]);
    commit(&s, &c, &long, b"x", "long");
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:work"]);
    assert!(!ok && out.contains("File paths must be at most 40 characters long"), "{out}");

    // The last blocked push left a pending bypass request for that commit.
    let sha = git_ok(&s, &c, &["rev-parse", "HEAD"]).trim().to_string();
    let (st, list) = s.call(Some(&u), "GET", &format!("{rp}/bypass-requests/push-rules"), None).await;
    assert_eq!(st, 200, "{list}");
    let req = list.as_array().unwrap().iter().find(|r| r["resource_identifier"] == sha).cloned().expect("request for the pushed commit");
    assert_eq!(req["status"], "pending");
    assert_eq!(req["requester"]["actor_name"], collab.login);
    assert_eq!(req["data"][0]["rule_type"], "max_file_path_length");
    let n = req["number"].as_i64().unwrap();
    let (st, one) = s.call(Some(&collab), "GET", &format!("{rp}/bypass-requests/push-rules/{n}"), None).await;
    assert_eq!(st, 200, "{one}");
    // The requester can't approve their own request; the admin (a bypass actor) can.
    let (st, _) = s.call(Some(&collab), "PATCH", &format!("{rp}/bypass-requests/push-rules/{n}"), Some(json!({"status": "approve"}))).await;
    assert_eq!(st, 403);
    let (st, one) = s.call(Some(&u), "PATCH", &format!("{rp}/bypass-requests/push-rules/{n}"), Some(json!({"status": "approve"}))).await;
    assert_eq!(st, 200, "{one}");
    assert_eq!(one["status"], "approved");
    assert_eq!(one["responses"][0]["reviewer"]["actor_name"], u.login);
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:work"]);
    assert!(ok, "{out}");
    let (_, one) = s.call(Some(&u), "GET", &format!("{rp}/bypass-requests/push-rules/{n}"), None).await;
    assert_eq!(one["status"], "completed");

    // The admin always bypasses; contents API writes are push-checked too.
    let (st, e) = s.call(Some(&collab), "PUT", &format!("{rp}/contents/setup.exe"), Some(json!({"message": "x", "content": b64(b"MZ")}))).await;
    assert_eq!(st, 409, "{e}");
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/contents/setup.exe"), Some(json!({"message": "x", "content": b64(b"MZ")}))).await;
    assert_eq!(st, 201, "{e}");
    let (st, _) = s.call(Some(&u), "GET", "/orgs/nope/bypass-requests/push-rules", None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn required_signatures_verify_ssh_signing_keys() {
    let s = TestServer::start().await;
    let (u, repo, c) = setup(&s).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    ruleset(&s, &u, &rp, json!({
        "name": "signed", "target": "branch", "enforcement": "active",
        "conditions": {"ref_name": {"include": ["~ALL"], "exclude": []}},
        "rules": [{"type": "required_signatures"}]})).await;
    commit(&s, &c, "a.txt", b"a\n", "unsigned");
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:main"]);
    assert!(!ok && out.contains("Commits must have verified signatures."), "{out}");

    // Sign with an SSH key registered as the user's signing key, as the user's email.
    let keydir = s.work_dir(&unique("k"));
    let key = keydir.join("id");
    let st = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(&key).status().unwrap();
    assert!(st.success());
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let email = format!("{}@example.com", u.login);
    let sign = |args: &[&str]| -> (bool, String) {
        let out = std::process::Command::new("git")
            .current_dir(&c)
            .args(["-c", "gpg.format=ssh", "-c"])
            .arg(format!("user.signingkey={}", key.display()))
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", &email)
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", &email)
            .output()
            .unwrap();
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let (ok, out) = sign(&["commit", "-q", "--amend", "-S", "-m", "signed"]);
    assert!(ok, "{out}");
    // Not registered yet: still rejected.
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:main"]);
    assert!(!ok, "{out}");
    let (st, k) = s.call(Some(&u), "POST", "/user/ssh_signing_keys", Some(json!({"title": "sign", "key": public.trim()}))).await;
    assert_eq!(st, 201, "{k}");
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:main"]);
    assert!(ok, "{out}");
    // API-created commits count as signed (like GitHub's web-flow signature).
    let (st, e) = s.call(Some(&u), "PUT", &format!("{rp}/contents/api.txt"), Some(json!({"message": "api", "content": b64(b"x")}))).await;
    assert_eq!(st, 201, "{e}");
}

// ---------------------------------------------------------------------------------------------
// SSH pushes (user key blocked, deploy key bypass actor allowed)

fn keygen(dir: &Path, name: &str) -> (PathBuf, String) {
    let path = dir.join(name);
    let st = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"]).arg(&path).status().unwrap();
    assert!(st.success());
    (path.clone(), std::fs::read_to_string(path.with_extension("pub")).unwrap().trim().to_string())
}

fn git_ssh(cwd: &Path, key: &Path, args: &[&str]) -> (bool, String) {
    let ssh = format!(
        "ssh -F /dev/null -i {} -o IdentitiesOnly=yes -o IdentityAgent=none -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
        key.display()
    );
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", ssh)
        .env_remove("SSH_AUTH_SOCK")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_pushes_and_deploy_key_bypass() {
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
    let keys = s.work_dir(&unique("k"));
    let (ukey, upub) = keygen(&keys, "user");
    let (dkey, dpub) = keygen(&keys, "deploy");
    let (st, _) = s.call(Some(&u), "POST", "/user/keys", Some(json!({"title": "me", "key": upub}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/keys"), Some(json!({"title": "deployer", "key": dpub, "read_only": false}))).await;
    assert_eq!(st, 201);
    ruleset(&s, &u, &rp, json!({
        "name": "no-updates", "target": "branch", "enforcement": "active",
        "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
        "bypass_actors": [{"actor_id": null, "actor_type": "DeployKey", "bypass_mode": "always"}],
        "rules": [{"type": "update"}]})).await;
    let url = format!("ssh://git@127.0.0.1:{port}/{}/{repo}.git", u.login);
    commit(&s, &c, "a.txt", b"a\n", "over ssh");
    let (ok, out) = git_ssh(&c, &ukey, &["push", &url, "HEAD:main"]);
    assert!(!ok, "{out}");
    assert!(out.contains("GH013") && out.contains("Cannot update this protected ref."), "{out}");
    let (ok, out) = git_ssh(&c, &dkey, &["push", &url, "HEAD:main"]);
    assert!(ok, "{out}");
    let all = suites(&s, &u, &rp).await;
    assert_eq!(all[0]["result"], "bypass", "{all:?}");
    assert_eq!(all[1]["result"], "fail", "{all:?}");
}
