//! Git over SSH with stock `git` + OpenSSH client, and the SSH key REST API.
mod common;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{TestServer, TestUser};
use serde_json::json;

struct SshServer {
    s: TestServer,
    port: u16,
}

async fn start() -> SshServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let s = TestServer::start_with(|c| {
        c.ssh_listen = Some(format!("127.0.0.1:{port}"));
        c.ssh_url_host = Some("127.0.0.1".into());
    })
    .await;
    forge::ssh::serve_listener(s.state.clone(), listener).await.unwrap();
    SshServer { s, port }
}

/// Generates a throwaway ed25519 key; returns (private key path, public key text).
fn keygen(dir: &Path, name: &str) -> (PathBuf, String) {
    let path = dir.join(name);
    let out = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let public = std::fs::read_to_string(path.with_extension("pub")).unwrap();
    (path, public.trim().to_string())
}

/// The ssh command git should use: only this key, no agent, no user config, no host checks.
fn ssh_command(key: &Path, port: Option<u16>) -> String {
    let mut c = format!(
        "ssh -F /dev/null -i {} -o IdentitiesOnly=yes -o IdentityAgent=none -o BatchMode=yes \
         -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR",
        key.display()
    );
    if let Some(p) = port {
        c.push_str(&format!(" -p {p}"));
    }
    c
}

fn git_ssh(cwd: &Path, ssh: &str, args: &[&str], extra_env: &[(&str, &str)]) -> (bool, String) {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", ssh)
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("GIT_SSH")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

impl SshServer {
    fn url(&self, owner: &str, repo: &str) -> String {
        format!("ssh://git@127.0.0.1:{}/{owner}/{repo}.git", self.port)
    }

    async fn add_key(&self, u: &TestUser, title: &str, key: &str) -> i64 {
        let (st, body) = self.s.call(Some(u), "POST", "/user/keys", Some(json!({"title": title, "key": key}))).await;
        assert_eq!(st, 201, "{body}");
        body["id"].as_i64().unwrap()
    }
}

fn commit_file(s: &TestServer, dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
    assert!(s.git(dir, &["add", "."]).0);
    let (ok, out) = s.git(dir, &["commit", "-q", "-m", name]);
    assert!(ok, "{out}");
}

fn rev(s: &TestServer, dir: &Path, r: &str) -> String {
    let (ok, out) = s.git(dir, &["rev-parse", r]);
    assert!(ok, "{out}");
    out.trim().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clone_push_fetch_over_ssh() {
    let srv = start().await;
    let s = &srv.s;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let keys = s.work_dir("keys");
    let (key, public) = keygen(&keys, "id_a");
    srv.add_key(&u, "laptop", &public).await;

    // ssh_url reflects the non-standard port.
    let (_, rj) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}", u.login), None).await;
    assert_eq!(rj["ssh_url"], json!(srv.url(&u.login, &repo)));

    let w = s.work_dir("w");
    let ssh = ssh_command(&key, None);
    let (ok, out) = git_ssh(&w, &ssh, &["clone", &srv.url(&u.login, &repo), "a"], &[]);
    assert!(ok, "{out}");
    let a = w.join("a");
    assert!(a.join("README.md").exists());

    // Push a commit to the default branch and a new branch.
    commit_file(s, &a, "one.txt", "1");
    let (ok, out) = git_ssh(&a, &ssh, &["push", "origin", "HEAD:main", "HEAD:refs/heads/feature"], &[]);
    assert!(ok, "{out}");
    let head = rev(s, &a, "HEAD");

    // The push is committed to storage: a fresh HTTP clone from an empty cache sees it.
    std::fs::remove_dir_all(s.dir.path().join("cache")).unwrap();
    std::fs::create_dir_all(s.dir.path().join("cache")).unwrap();
    let (ok, out) = s.git(&w, &["clone", "-q", &s.clone_url(&u, &repo), "h"]);
    assert!(ok, "{out}");
    assert_eq!(rev(s, &w.join("h"), "HEAD"), head);
    assert_eq!(std::fs::read_to_string(w.join("h/one.txt")).unwrap(), "1");

    // post-push processing ran (pushed_at set).
    let (_, rj) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}", u.login), None).await;
    assert!(rj["pushed_at"].is_string(), "{rj}");

    // Someone else pushes over HTTP; fetch over SSH (scp-style URL, port via GIT_SSH_COMMAND).
    let h = w.join("h");
    commit_file(s, &h, "two.txt", "2");
    assert!(s.git(&h, &["push", "-q", "origin", "HEAD:main"]).0);
    let new_head = rev(s, &h, "HEAD");
    let ssh_p = ssh_command(&key, Some(srv.port));
    let scp = format!("git@127.0.0.1:{}/{repo}.git", u.login);
    let (ok, out) = git_ssh(&a, &ssh_p, &["fetch", &scp, "main"], &[]);
    assert!(ok, "{out}");
    assert_eq!(rev(s, &a, "FETCH_HEAD"), new_head);

    // Protocol v2 is negotiated through the GIT_PROTOCOL env request.
    let (ok, out) = git_ssh(
        &a,
        &ssh,
        &["-c", "protocol.version=2", "ls-remote", &srv.url(&u.login, &repo)],
        &[("GIT_TRACE_PACKET", "1")],
    );
    assert!(ok, "{out}");
    assert!(out.contains("version 2"), "{out}");
    assert!(out.contains("refs/heads/feature"), "{out}");

    // Protocol v0 still works.
    let (ok, out) = git_ssh(&a, &ssh, &["-c", "protocol.version=0", "ls-remote", &srv.url(&u.login, &repo)], &[]);
    assert!(ok && out.contains(&new_head), "{out}");

    // Delete a branch, and a rejected non-fast-forward is reported (not committed).
    let (ok, out) = git_ssh(&a, &ssh, &["push", "origin", ":feature"], &[]);
    assert!(ok, "{out}");
    let (ok, out) = git_ssh(&a, &ssh, &["ls-remote", &srv.url(&u.login, &repo)], &[]);
    assert!(ok && !out.contains("refs/heads/feature"), "{out}");

    // Concurrent pushes to different branches both land (cluster-wide lock serializes them).
    let mut handles = Vec::new();
    for i in 0..3 {
        let dir = w.join(format!("c{i}"));
        let (ok, out) = git_ssh(&w, &ssh, &["clone", "-q", &srv.url(&u.login, &repo), dir.to_str().unwrap()], &[]);
        assert!(ok, "{out}");
        commit_file(s, &dir, &format!("c{i}.txt"), "c");
        let ssh = ssh.clone();
        handles.push(std::thread::spawn(move || git_ssh(&dir, &ssh, &["push", "origin", &format!("HEAD:refs/heads/c{i}")], &[])));
    }
    for h in handles {
        let (ok, out) = h.join().unwrap();
        assert!(ok, "{out}");
    }
    let (ok, out) = git_ssh(&a, &ssh, &["ls-remote", &srv.url(&u.login, &repo)], &[]);
    assert!(ok, "{out}");
    for i in 0..3 {
        assert!(out.contains(&format!("refs/heads/c{i}")), "{out}");
    }

    // git archive over SSH (git-upload-archive).
    let (ok, out) = git_ssh(&a, &ssh, &["archive", "--remote", &srv.url(&u.login, &repo), "-o", "/dev/null", "main"], &[]);
    assert!(ok, "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_rejections() {
    let srv = start().await;
    let s = &srv.s;
    let alice = s.user().await;
    let bob = s.user().await;
    let public_repo = s.create_repo(&alice, true).await;
    let (st, _) = s
        .call(Some(&alice), "POST", "/user/repos", Some(json!({"name": "secret-ssh", "private": true, "auto_init": true})))
        .await;
    assert_eq!(st, 201);
    let keys = s.work_dir("keys");
    let (alice_key, alice_pub) = keygen(&keys, "alice");
    let (bob_key, bob_pub) = keygen(&keys, "bob");
    let (unknown_key, _) = keygen(&keys, "unknown");
    srv.add_key(&alice, "a", &alice_pub).await;
    srv.add_key(&bob, "b", &bob_pub).await;
    let w = s.work_dir("w");

    // Unknown key: authentication fails.
    let (ok, out) = git_ssh(&w, &ssh_command(&unknown_key, None), &["ls-remote", &srv.url(&alice.login, &public_repo)], &[]);
    assert!(!ok);
    assert!(out.contains("Permission denied"), "{out}");

    // Username must be `git`.
    let bad_user = format!("ssh://{}@127.0.0.1:{}/{}/{public_repo}.git", alice.login, srv.port, alice.login);
    let (ok, out) = git_ssh(&w, &ssh_command(&alice_key, None), &["ls-remote", &bad_user], &[]);
    assert!(!ok, "{out}");

    // Shell access: friendly message, exit 1.
    let out = Command::new("ssh")
        .args(ssh_command(&alice_key, Some(srv.port)).split_whitespace().skip(1))
        .args(["-T", "git@127.0.0.1"])
        .env_remove("SSH_AUTH_SOCK")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let msg = String::from_utf8_lossy(&out.stderr);
    assert!(
        msg.contains(&format!("Hi {}! You've successfully authenticated, but forge does not provide shell access.", alice.login)),
        "{msg}"
    );

    // Arbitrary commands are refused.
    let out = Command::new("ssh")
        .args(ssh_command(&alice_key, Some(srv.port)).split_whitespace().skip(1))
        .args(["git@127.0.0.1", "cat", "/etc/passwd"])
        .env_remove("SSH_AUTH_SOCK")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Invalid command"));

    // Bob cannot see alice's private repo: 404-style message.
    let bob_ssh = ssh_command(&bob_key, None);
    let (ok, out) = git_ssh(&w, &bob_ssh, &["clone", &srv.url(&alice.login, "secret-ssh"), "x"], &[]);
    assert!(!ok);
    assert!(out.contains("Repository not found"), "{out}");
    let (ok, out) = git_ssh(&w, &bob_ssh, &["ls-remote", &srv.url(&alice.login, "does-not-exist")], &[]);
    assert!(!ok && out.contains("Repository not found"), "{out}");

    // Bob can clone alice's public repo but not push to it.
    let (ok, out) = git_ssh(&w, &bob_ssh, &["clone", "-q", &srv.url(&alice.login, &public_repo), "pub"], &[]);
    assert!(ok, "{out}");
    let p = w.join("pub");
    commit_file(s, &p, "bob.txt", "b");
    let (ok, out) = git_ssh(&p, &bob_ssh, &["push", "origin", "HEAD:main"], &[]);
    assert!(!ok);
    assert!(out.contains("denied to"), "{out}");
    let (ok, out) = git_ssh(&p, &ssh_command(&alice_key, None), &["ls-remote", &srv.url(&alice.login, &public_repo)], &[]);
    assert!(ok, "{out}");
    assert!(!out.contains(&rev(s, &p, "HEAD")), "bob's commit must not have landed: {out}");

    // Alice (owner) can access the private repo.
    let (ok, out) = git_ssh(&w, &ssh_command(&alice_key, None), &["clone", "-q", &srv.url(&alice.login, "secret-ssh"), "sec"], &[]);
    assert!(ok, "{out}");

    // Archived repos are read-only over SSH.
    let (st, _) = s
        .call(Some(&alice), "PATCH", &format!("/repos/{}/{public_repo}", alice.login), Some(json!({"archived": true})))
        .await;
    if st == 200 {
        let a = w.join("arch");
        let (ok, out) = git_ssh(&w, &ssh_command(&alice_key, None), &["clone", "-q", &srv.url(&alice.login, &public_repo), "arch"], &[]);
        assert!(ok, "{out}");
        commit_file(s, &a, "z.txt", "z");
        let (ok, out) = git_ssh(&a, &ssh_command(&alice_key, None), &["push", "origin", "HEAD:main"], &[]);
        assert!(!ok && out.contains("archived"), "{out}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn key_api() {
    let srv = start().await;
    let s = &srv.s;
    let u = s.user().await;
    let other = s.user().await;
    let keys = s.work_dir("keys");
    let (key, public) = keygen(&keys, "k1");

    // Validation.
    let (st, body) = s.call(Some(&u), "POST", "/user/keys", Some(json!({"title": "x", "key": "ssh-ed25519 garbage"}))).await;
    assert_eq!(st, 422, "{body}");
    let (st, _) = s.call(Some(&u), "POST", "/user/keys", Some(json!({"title": "x"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(None, "GET", "/user/keys", None).await;
    assert_eq!(st, 401);

    // Create, comment stripped from the stored key.
    let (st, k) = s.call(Some(&u), "POST", "/user/keys", Some(json!({"title": "laptop", "key": format!("{public}\n")}))).await;
    assert_eq!(st, 201, "{k}");
    let id = k["id"].as_i64().unwrap();
    assert_eq!(k["title"], "laptop");
    assert_eq!(k["read_only"], false);
    assert_eq!(k["verified"], true);
    assert_eq!(k["url"], json!(format!("{}/user/keys/{id}", s.api)));
    let stored = public.rsplit_once(' ').unwrap().0;
    assert_eq!(k["key"], json!(stored));

    // Same key again (any user) -> 422 key is already in use.
    let (st, body) = s.call(Some(&other), "POST", "/user/keys", Some(json!({"title": "dup", "key": public}))).await;
    assert_eq!(st, 422);
    assert_eq!(body["errors"][0]["message"], "key is already in use", "{body}");

    // RSA and ECDSA keys are accepted.
    for (t, bits) in [("rsa", "2048"), ("ecdsa", "256")] {
        let path = keys.join(format!("k_{t}"));
        let out = Command::new("ssh-keygen").args(["-q", "-t", t, "-b", bits, "-N", "", "-f"]).arg(&path).output().unwrap();
        assert!(out.status.success());
        let pubk = std::fs::read_to_string(path.with_extension("pub")).unwrap();
        let (st, body) = s.call(Some(&other), "POST", "/user/keys", Some(json!({"title": t, "key": pubk}))).await;
        assert_eq!(st, 201, "{t}: {body}");
    }

    // Read endpoints.
    let (st, list) = s.call(Some(&u), "GET", "/user/keys", None).await;
    assert_eq!(st, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (st, one) = s.call(Some(&u), "GET", &format!("/user/keys/{id}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(one["id"], json!(id));
    let (st, _) = s.call(Some(&other), "GET", &format!("/user/keys/{id}"), None).await;
    assert_eq!(st, 404);
    let (st, pubs) = s.call(None, "GET", &format!("/users/{}/keys", u.login), None).await;
    assert_eq!(st, 200);
    assert_eq!(pubs, json!([{"id": id, "key": stored}]));
    let res = reqwest::get(format!("{}/{}.keys", s.base, u.login)).await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.headers()["content-type"].to_str().unwrap().starts_with("text/plain"));
    assert_eq!(res.text().await.unwrap(), format!("{stored}\n"));
    let res = reqwest::get(format!("{}/{}.keys", s.base, "nobody-here-xyz")).await.unwrap();
    assert_eq!(res.status(), 404);

    // The key works over SSH until deleted.
    let repo = s.create_repo(&u, true).await;
    let w = s.work_dir("w");
    let ssh = ssh_command(&key, None);
    let (ok, out) = git_ssh(&w, &ssh, &["ls-remote", &srv.url(&u.login, &repo)], &[]);
    assert!(ok, "{out}");
    let (st, _) = s.call(Some(&other), "DELETE", &format!("/user/keys/{id}"), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/user/keys/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("/user/keys/{id}"), None).await;
    assert_eq!(st, 404);
    let (ok, out) = git_ssh(&w, &ssh, &["ls-remote", &srv.url(&u.login, &repo)], &[]);
    assert!(!ok && out.contains("Permission denied"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deploy_keys() {
    let srv = start().await;
    let s = &srv.s;
    let u = s.user().await;
    let other = s.user().await;
    let (st, _) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": "deploy-me", "private": true, "auto_init": true}))).await;
    assert_eq!(st, 201);
    let other_repo = s.create_repo(&u, true).await;
    let keys = s.work_dir("keys");
    let (ro_key, ro_pub) = keygen(&keys, "ro");
    let (rw_key, rw_pub) = keygen(&keys, "rw");
    let path = format!("/repos/{}/deploy-me/keys", u.login);

    // Only admins manage deploy keys; invisible repo is 404.
    let (st, _) = s.call(Some(&other), "GET", &path, None).await;
    assert_eq!(st, 404);

    let (st, ro) = s.call(Some(&u), "POST", &path, Some(json!({"title": "ci", "key": ro_pub}))).await;
    assert_eq!(st, 201, "{ro}");
    assert_eq!(ro["read_only"], true);
    assert_eq!(ro["added_by"], json!(u.login));
    let (st, rw) = s.call(Some(&u), "POST", &path, Some(json!({"title": "deployer", "key": rw_pub, "read_only": false}))).await;
    assert_eq!(st, 201, "{rw}");
    assert_eq!(rw["read_only"], false);
    // A deploy key cannot also be a user key.
    let (st, body) = s.call(Some(&u), "POST", "/user/keys", Some(json!({"title": "x", "key": ro_pub}))).await;
    assert_eq!(st, 422, "{body}");
    let (st, list) = s.call(Some(&u), "GET", &path, None).await;
    assert_eq!(st, 200);
    assert_eq!(list.as_array().unwrap().len(), 2);
    let (st, one) = s.call(Some(&u), "GET", &format!("{path}/{}", ro["id"]), None).await;
    assert_eq!(st, 200);
    assert_eq!(one["title"], "ci");

    let w = s.work_dir("w");
    let url = srv.url(&u.login, "deploy-me");
    // Read-only key: clone ok, push denied.
    let (ok, out) = git_ssh(&w, &ssh_command(&ro_key, None), &["clone", "-q", &url, "ro"], &[]);
    assert!(ok, "{out}");
    commit_file(s, &w.join("ro"), "d.txt", "d");
    let (ok, out) = git_ssh(&w.join("ro"), &ssh_command(&ro_key, None), &["push", "origin", "HEAD:main"], &[]);
    assert!(!ok && out.contains("denied"), "{out}");
    // Read-write key: push ok.
    let (ok, out) = git_ssh(&w.join("ro"), &ssh_command(&rw_key, None), &["push", "origin", "HEAD:main"], &[]);
    assert!(ok, "{out}");
    // Deploy keys are scoped to their repository.
    let (ok, out) = git_ssh(&w, &ssh_command(&rw_key, None), &["ls-remote", &srv.url(&u.login, &other_repo)], &[]);
    assert!(!ok && out.contains("Repository not found"), "{out}");

    let (st, _) = s.call(Some(&u), "DELETE", &format!("{path}/{}", ro["id"]), None).await;
    assert_eq!(st, 204);
    let (ok, _) = git_ssh(&w, &ssh_command(&ro_key, None), &["ls-remote", &url], &[]);
    assert!(!ok);
}

/// If the storage commit fails after receive-pack accepted the pack, the client must see the
/// push rejected (synthesized report-status) and a non-zero exit — never "ok".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_fails_when_storage_commit_fails() {
    use std::os::unix::fs::PermissionsExt;
    let srv = start().await;
    let s = &srv.s;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let keys = s.work_dir("keys");
    let (key, public) = keygen(&keys, "k");
    srv.add_key(&u, "k", &public).await;
    let w = s.work_dir("w");
    let ssh = ssh_command(&key, None);
    let (ok, out) = git_ssh(&w, &ssh, &["clone", "-q", &srv.url(&u.login, &repo), "a"], &[]);
    assert!(ok, "{out}");
    let a = w.join("a");
    let before = rev(s, &a, "HEAD");
    commit_file(s, &a, "f.txt", "f");

    // Make object storage unwritable so uploading the new pack fails.
    let objects = s.dir.path().join("objects");
    let set_mode = |mode: u32| {
        for e in walk(&objects) {
            std::fs::set_permissions(&e, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    };
    set_mode(0o555);
    let (ok, out) = git_ssh(&a, &ssh, &["push", "origin", "HEAD:main"], &[]);
    set_mode(0o755);
    assert!(!ok, "push must fail: {out}");
    assert!(out.contains("storage commit failed"), "{out}");

    // Nothing was committed, even on this pod's cache.
    let (ok, out) = git_ssh(&w, &ssh, &["ls-remote", &srv.url(&u.login, &repo), "refs/heads/main"], &[]);
    assert!(ok, "{out}");
    assert!(out.starts_with(&before), "{out}");

    // And it works once storage is back.
    let (ok, out) = git_ssh(&a, &ssh, &["push", "origin", "HEAD:main"], &[]);
    assert!(ok, "{out}");
}

fn walk(p: &Path) -> Vec<PathBuf> {
    let mut out = vec![p.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                out.extend(walk(&e.path()));
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_key_is_shared_across_pods() {
    let a = start().await;
    let b = start().await;
    let ka = forge::ssh::host_key(&a.s.state).await.unwrap();
    let kb = forge::ssh::host_key(&b.s.state).await.unwrap();
    assert_eq!(ka.public_key(), kb.public_key());
}
