//! Platform area: releases + assets, webhooks (queue delivery), gists, gpg keys, follows,
//! commit comments, branch protection, template generation, markdown, search, misc.
mod common;
use std::time::Duration;

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn push_branch(s: &TestServer, u: &TestUser, repo: &str, branch: &str, file: &str, msg: &str) -> String {
    let w = s.work_dir(&unique("w"));
    let (ok, out) = s.git(&w, &["clone", "-q", &s.clone_url(u, repo), "c"]);
    assert!(ok, "{out}");
    let c = w.join("c");
    if branch != "main" {
        assert!(s.git(&c, &["checkout", "-q", "-b", branch]).0);
    }
    std::fs::write(c.join(file), format!("{msg}\n")).unwrap();
    assert!(s.git(&c, &["add", "."]).0);
    assert!(s.git(&c, &["commit", "-q", "-m", msg]).0);
    let (ok, out) = s.git(&c, &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")]);
    assert!(ok, "{out}");
    s.git(&c, &["rev-parse", "HEAD"]).1.trim().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_asset_roundtrip() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);

    // Draft without an existing tag: no tag yet, invisible to anonymous readers.
    let (st, draft) = s
        .call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v1.0.0", "name": "One", "draft": true})))
        .await;
    assert_eq!(st, 201, "{draft}");
    let id = draft["id"].as_i64().unwrap();
    assert!(draft["published_at"].is_null());
    let upload_url = draft["upload_url"].as_str().unwrap().to_string();
    assert_eq!(upload_url, format!("{}/api/uploads/repos/{}/{}/releases/{id}/assets{{?name,label}}", s.base, u.login, repo));
    let (st, _) = s.call(None, "GET", &format!("{rp}/releases/{id}"), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/git/refs/tags/v1.0.0"), None).await;
    assert_ne!(st, 200);

    // Upload an asset (streamed) to the upload_url, like gh does.
    let content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let up = s
        .client()
        .post(format!("{}?name=bin.dat&label=Binary", upload_url.split('{').next().unwrap()))
        .header("Authorization", format!("token {}", u.token))
        .header("Content-Type", "application/octet-stream")
        .body(content.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(up.status().as_u16(), 201);
    let asset: Value = up.json().await.unwrap();
    assert_eq!(asset["size"], json!(content.len()));
    assert_eq!(asset["label"], "Binary");
    assert_eq!(asset["state"], "uploaded");
    assert!(asset["digest"].as_str().unwrap().starts_with("sha256:"));
    // Duplicate name is a 422.
    let dup = s
        .client()
        .post(format!("{}/api/uploads/repos/{}/{}/releases/{id}/assets?name=bin.dat", s.base, u.login, repo))
        .header("Authorization", format!("token {}", u.token))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status().as_u16(), 422);

    // Publish: creates the tag at the default branch and becomes latest.
    let (st, rel) = s.call(Some(&u), "PATCH", &format!("{rp}/releases/{id}"), Some(json!({"draft": false}))).await;
    assert_eq!(st, 200, "{rel}");
    assert!(rel["published_at"].is_string());
    let (_, refs) = s.state.git.refs(repo_id(&s, &u, &repo).await).await.unwrap();
    assert!(refs.contains_key("refs/tags/v1.0.0"));
    let (st, latest) = s.call(None, "GET", &format!("{rp}/releases/latest"), None).await;
    assert_eq!(st, 200);
    assert_eq!(latest["id"].as_i64(), Some(id));
    let (st, by_tag) = s.call(None, "GET", &format!("{rp}/releases/tags/v1.0.0"), None).await;
    assert_eq!(st, 200);
    assert_eq!(by_tag["assets"].as_array().unwrap().len(), 1);

    // Download via browser_download_url and via the API with Accept: application/octet-stream.
    let browser = asset["browser_download_url"].as_str().unwrap();
    let got = s.client().get(browser).send().await.unwrap();
    assert_eq!(got.status().as_u16(), 200);
    assert_eq!(got.bytes().await.unwrap().to_vec(), content);
    let got = s
        .client()
        .get(asset["url"].as_str().unwrap())
        .header("Accept", "application/octet-stream")
        .header("Authorization", format!("token {}", u.token))
        .send()
        .await
        .unwrap();
    assert_eq!(got.status().as_u16(), 200);
    assert_eq!(got.bytes().await.unwrap().to_vec(), content);
    let (_, a) = s.call(Some(&u), "GET", &format!("{rp}/releases/assets/{}", asset["id"]), None).await;
    assert_eq!(a["download_count"], 2);

    // Second release with generated notes; prerelease does not become latest.
    push_branch(&s, &u, &repo, "main", "two.txt", "second change").await;
    let (st, notes) = s
        .call(Some(&u), "POST", &format!("{rp}/releases/generate-notes"), Some(json!({"tag_name": "v2.0.0"})))
        .await;
    assert_eq!(st, 200);
    assert_eq!(notes["name"], "v2.0.0");
    let body = notes["body"].as_str().unwrap();
    assert!(body.contains("second change"), "{body}");
    assert!(body.contains("compare/v1.0.0...v2.0.0"), "{body}");
    let (st, pre) = s
        .call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v2.0.0", "prerelease": true, "generate_release_notes": true})))
        .await;
    assert_eq!(st, 201, "{pre}");
    assert!(pre["body"].as_str().unwrap().contains("second change"));
    let (_, latest) = s.call(None, "GET", &format!("{rp}/releases/latest"), None).await;
    assert_eq!(latest["id"].as_i64(), Some(id));
    // A newer full release becomes latest; editing an older one does not steal it back, and
    // make_latest=false opts out.
    let (st, v3) = s.call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v3.0.0"}))).await;
    assert_eq!(st, 201);
    let (_, latest) = s.call(None, "GET", &format!("{rp}/releases/latest"), None).await;
    assert_eq!(latest["id"], v3["id"]);
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{rp}/releases/{id}"), Some(json!({"body": "edited"}))).await;
    assert_eq!(st, 200);
    let (_, latest) = s.call(None, "GET", &format!("{rp}/releases/latest"), None).await;
    assert_eq!(latest["id"], v3["id"]);
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{rp}/releases/{}", v3["id"]), Some(json!({"make_latest": "false"}))).await;
    assert_eq!(st, 200);
    let (_, latest) = s.call(None, "GET", &format!("{rp}/releases/latest"), None).await;
    assert_eq!(latest["id"].as_i64(), Some(id));
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/releases/{}", v3["id"]), None).await;
    assert_eq!(st, 204);
    let (_, list) = s.call(None, "GET", &format!("{rp}/releases"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    // Duplicate tag.
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v2.0.0"}))).await;
    assert_eq!(st, 422);

    // Delete asset and release.
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/releases/assets/{}", asset["id"]), None).await;
    assert_eq!(st, 204);
    let got = s.client().get(browser).send().await.unwrap();
    assert_eq!(got.status().as_u16(), 404);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/releases/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/releases/{id}"), None).await;
    assert_eq!(st, 404);
}

#[derive(Debug)]
struct Received {
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

async fn hook_listener() -> (String, tokio::sync::mpsc::UnboundedReceiver<Received>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Received>();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(Received { headers, body: body.to_vec() });
                "ok"
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, rx)
}

async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Received>, event: &str) -> Received {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let r = tokio::time::timeout_at(deadline, rx.recv()).await.expect("webhook not delivered in time").unwrap();
        if r.headers.get("x-github-event").and_then(|v| v.to_str().ok()) == Some(event) {
            return r;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webhook_delivery_via_queue() {
    let s = TestServer::start().await;
    forge::jobs::spawn_worker(s.state.clone());
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let (url, mut rx) = hook_listener().await;

    let (st, hook) = s
        .call(
            Some(&u),
            "POST",
            &format!("{rp}/hooks"),
            Some(json!({"name": "web", "events": ["push", "issues", "release"], "config": {"url": url, "content_type": "json", "secret": "s3cret"}})),
        )
        .await;
    assert_eq!(st, 201, "{hook}");
    assert_eq!(hook["config"]["secret"], "********");
    let hook_id = hook["id"].as_i64().unwrap();

    // A ping is sent on creation.
    let ping = next_event(&mut rx, "ping").await;
    let p: Value = serde_json::from_slice(&ping.body).unwrap();
    assert_eq!(p["hook_id"].as_i64(), Some(hook_id));
    assert_eq!(
        ping.headers.get("x-hub-signature-256").unwrap().to_str().unwrap(),
        forge::webhooks::signature_256("s3cret", &ping.body)
    );
    assert!(ping.headers.get("x-github-delivery").is_some());

    // Push via git -> push event.
    let sha = push_branch(&s, &u, &repo, "topic", "t.txt", "topic commit").await;
    let push = next_event(&mut rx, "push").await;
    let p: Value = serde_json::from_slice(&push.body).unwrap();
    assert_eq!(p["ref"], "refs/heads/topic");
    assert_eq!(p["after"], sha);
    assert_eq!(p["created"], true);
    assert_eq!(p["commits"][0]["message"], "topic commit");
    assert_eq!(p["repository"]["name"], repo.as_str());
    assert_eq!(p["sender"]["login"], u.login.as_str());
    assert_eq!(
        push.headers.get("x-hub-signature-256").unwrap().to_str().unwrap(),
        forge::webhooks::signature_256("s3cret", &push.body)
    );

    // Other areas emit through enqueue_event.
    let rid = repo_id(&s, &u, &repo).await;
    forge::webhooks::enqueue_event(&s.state, rid, "issues", json!({"action": "opened", "issue": {"number": 1}})).await.unwrap();
    let iss = next_event(&mut rx, "issues").await;
    let p: Value = serde_json::from_slice(&iss.body).unwrap();
    assert_eq!(p["action"], "opened");
    assert_eq!(p["repository"]["full_name"], format!("{}/{}", u.login, repo));

    // Release creation emits release events.
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/releases"), Some(json!({"tag_name": "v1"}))).await;
    assert_eq!(st, 201);
    let rel = next_event(&mut rx, "release").await;
    let p: Value = serde_json::from_slice(&rel.body).unwrap();
    assert_eq!(p["release"]["tag_name"], "v1");

    // Deliveries are recorded and can be redelivered.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (st, list) = s.call(Some(&u), "GET", &format!("{rp}/hooks/{hook_id}/deliveries"), None).await;
    assert_eq!(st, 200);
    let list = list.as_array().unwrap().clone();
    assert!(list.len() >= 3, "{list:?}");
    let ping_d = list.iter().find(|d| d["event"] == "ping").unwrap();
    assert_eq!(ping_d["status_code"], 200);
    assert_eq!(ping_d["status"], "OK");
    let (st, full) = s.call(Some(&u), "GET", &format!("{rp}/hooks/{hook_id}/deliveries/{}", ping_d["id"]), None).await;
    assert_eq!(st, 200);
    assert_eq!(full["request"]["payload"]["hook_id"].as_i64(), Some(hook_id));
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/hooks/{hook_id}/deliveries/{}/attempts", ping_d["id"]), None).await;
    assert_eq!(st, 202);
    let again = next_event(&mut rx, "ping").await;
    assert_ne!(again.headers.get("x-github-delivery"), ping.headers.get("x-github-delivery"));

    // Inactive hooks get nothing; the hook records its last response.
    let (st, h) = s.call(Some(&u), "PATCH", &format!("{rp}/hooks/{hook_id}"), Some(json!({"active": false}))).await;
    assert_eq!(st, 200);
    assert_eq!(h["last_response"]["code"], 200);

    // Activity events were recorded.
    let (st, events) = s.call(None, "GET", &format!("{rp}/events"), None).await;
    assert_eq!(st, 200);
    let types: Vec<&str> = events.as_array().unwrap().iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"PushEvent"), "{types:?}");
    assert!(types.contains(&"CreateEvent"), "{types:?}");
    assert!(types.contains(&"ReleaseEvent"), "{types:?}");
}

async fn repo_id(s: &TestServer, u: &TestUser, repo: &str) -> i64 {
    forge::db::load_repo(&s.state.db, None, &u.login, repo).await.map_err(|e| e.message).unwrap().repo.id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webhook_retries_failed_delivery() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    // Port 9 (discard) is closed: delivery fails and the job is rescheduled with backoff.
    let (st, hook) = s
        .call(Some(&u), "POST", &format!("{rp}/hooks"), Some(json!({"config": {"url": "http://127.0.0.1:9/x"}})))
        .await;
    assert_eq!(st, 201);
    let hook_id = hook["id"].as_i64().unwrap();
    let delivery_id: i64 = sqlx::query_scalar("SELECT id FROM webhook_deliveries WHERE hook_id = $1").bind(hook_id).fetch_one(&s.state.db).await.unwrap();
    // Run the job directly (no background worker in this test).
    let job_id: i64 = sqlx::query_scalar("SELECT id FROM jobs WHERE queue = 'webhook' AND (payload->>'delivery_id')::bigint = $1")
        .bind(delivery_id)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert!(forge::webhooks::deliver(&s.state, delivery_id).await.is_err());
    let (_, d) = s.call(Some(&u), "GET", &format!("{rp}/hooks/{hook_id}/deliveries/{delivery_id}"), None).await;
    assert_eq!(d["status_code"], 0);
    assert_eq!(d["status"], "failed to connect");
    // Claim-and-fail through the queue: attempts recorded, run_at pushed into the future.
    sqlx::query("UPDATE jobs SET run_at = now() - interval '1 second' WHERE id = $1").bind(job_id).execute(&s.state.db).await.unwrap();
    for _ in 0..20 {
        forge::jobs::run_once(&s.state).await.unwrap();
        let attempts: Option<i32> = sqlx::query_scalar("SELECT attempts FROM jobs WHERE id = $1").bind(job_id).fetch_optional(&s.state.db).await.unwrap();
        if attempts.unwrap_or(0) >= 1 {
            break;
        }
    }
    let (attempts, later): (i32, bool) = sqlx::query_as("SELECT attempts, run_at > now() FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert!(attempts >= 1);
    assert!(later);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gists_gpg_follow_markdown() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let v = s.user().await;

    // Gists
    let (st, g) = s
        .call(Some(&u), "POST", "/gists", Some(json!({"description": "d", "public": true, "files": {"a.rs": {"content": "fn main() {}\n"}}})))
        .await;
    assert_eq!(st, 201, "{g}");
    let gid = g["id"].as_str().unwrap().to_string();
    assert_eq!(g["files"]["a.rs"]["language"], "Rust");
    let (st, g2) = s
        .call(Some(&u), "PATCH", &format!("/gists/{gid}"), Some(json!({"files": {"a.rs": {"content": "fn main() { x() }\n"}, "b.txt": {"content": "b"}}})))
        .await;
    assert_eq!(st, 200);
    assert_eq!(g2["history"].as_array().unwrap().len(), 2);
    let raw = s.client().get(g2["files"]["b.txt"]["raw_url"].as_str().unwrap()).send().await.unwrap();
    assert_eq!(raw.text().await.unwrap(), "b");
    let (st, _) = s.call(Some(&v), "PATCH", &format!("/gists/{gid}"), Some(json!({"description": "x"}))).await;
    assert_eq!(st, 404);
    let (st, f) = s.call(Some(&v), "POST", &format!("/gists/{gid}/forks"), None).await;
    assert_eq!(st, 201, "{f}");
    let (_, mine) = s.call(Some(&u), "GET", "/gists", None).await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/gists/{gid}"), None).await;
    assert_eq!(st, 204);

    // GPG keys: an (unsigned) v4 Ed25519 key with a user id.
    let armored = "-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nxjMEZVPxABYJKwYBBAHaRw8BAQdAIO/8mtwnD9lGui5GDA3YDlX0C+apjv1pZoJ/\nbAHgCdTNEUEgPGFAZXhhbXBsZS5jb20+\n=AAAA\n-----END PGP PUBLIC KEY BLOCK-----\n";
    let (st, k) = s.call(Some(&u), "POST", "/user/gpg_keys", Some(json!({"name": "k", "armored_public_key": armored}))).await;
    assert_eq!(st, 201, "{k}");
    assert_eq!(k["key_id"], "A2608713E7D89137");
    assert_eq!(k["emails"][0]["email"], "a@example.com");
    let (st, _) = s.call(Some(&u), "POST", "/user/gpg_keys", Some(json!({"armored_public_key": armored}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", "/user/gpg_keys", Some(json!({"armored_public_key": "garbage"}))).await;
    assert_eq!(st, 422);
    let (_, list) = s.call(None, "GET", &format!("/users/{}/gpg_keys", u.login), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/user/gpg_keys/{}", k["id"]), None).await;
    assert_eq!(st, 204);

    // Follows
    let (st, _) = s.call(Some(&u), "PUT", &format!("/user/following/{}", v.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("/user/following/{}", v.login), None).await;
    assert_eq!(st, 204);
    let (_, followers) = s.call(None, "GET", &format!("/users/{}/followers", v.login), None).await;
    assert_eq!(followers[0]["login"], u.login.as_str());
    let (_, prof) = s.call(None, "GET", &format!("/users/{}", v.login), None).await;
    assert_eq!(prof["followers"], 1);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/user/following/{}", v.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(None, "GET", &format!("/users/{}/following/{}", u.login, v.login), None).await;
    assert_eq!(st, 404);

    // Markdown: GFM table, task list, sanitized HTML, mention linkified.
    let md = format!("| a |\n|---|\n| b |\n\n- [x] done\n\n<script>alert(1)</script>\n\nhi @{} and @nobody-here-xyz", v.login);
    let res = s
        .client()
        .post(format!("{}/markdown", s.api))
        .json(&json!({"text": md, "mode": "gfm"}))
        .send()
        .await
        .unwrap();
    assert!(res.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    let html = res.text().await.unwrap();
    assert!(html.contains("<table>"), "{html}");
    assert!(html.contains("checkbox"), "{html}");
    assert!(!html.contains("<script>"), "{html}");
    assert!(html.contains(&format!("href=\"{}/{}\"", s.base, v.login)), "{html}");
    assert!(!html.contains("/nobody-here-xyz\""), "only existing users are linked: {html}");

    // Misc lists
    let (_, em) = s.call(None, "GET", "/emojis", None).await;
    assert!(em["+1"].as_str().unwrap().starts_with("https://"));
    let (_, gi) = s.call(None, "GET", "/gitignore/templates/rust", None).await;
    assert_eq!(gi["name"], "Rust");
    let (_, lic) = s.call(None, "GET", "/licenses/apache-2.0", None).await;
    assert_eq!(lic["spdx_id"], "Apache-2.0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repo_extras() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let name = unique("r");
    let (st, _) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": name, "auto_init": true, "license_template": "mit"}))).await;
    assert_eq!(st, 201);
    let rp = format!("/repos/{}/{}", u.login, name);
    let sha = push_branch(&s, &u, &name, "feature", "needle.txt", "find the needle").await;

    // License detection, contributors.
    let (st, lic) = s.call(None, "GET", &format!("{rp}/license"), None).await;
    assert_eq!(st, 200, "{lic}");
    assert_eq!(lic["license"]["key"], "mit");
    let (st, contrib) = s.call(None, "GET", &format!("{rp}/contributors"), None).await;
    assert_eq!(st, 200);
    assert_eq!(contrib[0]["login"], u.login.as_str());

    // Commit comments + reactions.
    let (st, cc) = s
        .call(Some(&u), "POST", &format!("{rp}/commits/{sha}/comments"), Some(json!({"body": "nice", "path": "needle.txt", "position": 1})))
        .await;
    assert_eq!(st, 201, "{cc}");
    assert_eq!(cc["commit_id"], sha.as_str());
    assert_eq!(cc["author_association"], "OWNER");
    let cid = cc["id"].as_i64().unwrap();
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/comments/{cid}/reactions"), Some(json!({"content": "heart"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/comments/{cid}/reactions"), Some(json!({"content": "heart"}))).await;
    assert_eq!(st, 200);
    let (_, c) = s.call(None, "GET", &format!("{rp}/comments/{cid}"), None).await;
    assert_eq!(c["reactions"]["heart"], 1);
    let (_, list) = s.call(None, "GET", &format!("{rp}/commits/{sha}/comments"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/commits/{}/comments", "0".repeat(40)), Some(json!({"body": "x"}))).await;
    assert_eq!(st, 422);

    // Branch protection (stored).
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/branches/feature/protection"), None).await;
    assert_eq!(st, 404);
    let (st, p) = s
        .call(
            Some(&u),
            "PUT",
            &format!("{rp}/branches/feature/protection"),
            Some(json!({"required_status_checks": {"strict": true, "contexts": ["ci"]}, "enforce_admins": true,
                "required_pull_request_reviews": {"required_approving_review_count": 2}, "restrictions": null})),
        )
        .await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["enforce_admins"]["enabled"], true);
    assert_eq!(p["required_pull_request_reviews"]["required_approving_review_count"], 2);
    assert_eq!(p["required_status_checks"]["contexts"][0], "ci");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/branches/feature/protection/enforce_admins"), None).await;
    assert_eq!(st, 204);
    let (_, p) = s.call(Some(&u), "GET", &format!("{rp}/branches/feature/protection"), None).await;
    assert_eq!(p["enforce_admins"]["enabled"], false);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/branches/nope/protection"), Some(json!({}))).await;
    assert_eq!(st, 404);

    // Search code / commits.
    let (st, code) = s.call(Some(&u), "GET", &format!("/search/code?q=needle+repo:{}/{}", u.login, name), None).await;
    assert_eq!(st, 200, "{code}");
    // Only the default branch is indexed: the feature branch file is not found.
    assert_eq!(code["total_count"], 0);
    let (st, code) = s.call(Some(&u), "GET", &format!("/search/code?q=permission+repo:{}/{}", u.login, name), None).await;
    assert_eq!(st, 200);
    assert_eq!(code["items"][0]["path"], "LICENSE");
    let (st, commits) = s.call(Some(&u), "GET", &format!("/search/commits?q=initial+repo:{}/{}", u.login, name), None).await;
    assert_eq!(st, 200);
    assert_eq!(commits["total_count"], 1);
    assert_eq!(commits["items"][0]["author"]["login"], u.login.as_str());

    // Deployments.
    let (st, d) = s.call(Some(&u), "POST", &format!("{rp}/deployments"), Some(json!({"ref": "feature", "environment": "qa"}))).await;
    assert_eq!(st, 201, "{d}");
    assert_eq!(d["sha"], sha.as_str());
    let (st, ds) = s
        .call(Some(&u), "POST", &format!("{rp}/deployments/{}/statuses", d["id"]), Some(json!({"state": "success", "environment_url": "https://qa.example.com"})))
        .await;
    assert_eq!(st, 201, "{ds}");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/deployments/{}", d["id"]), None).await;
    assert_eq!(st, 204);

    // Template generation copies content without fork linkage.
    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"is_template": true}))).await;
    assert_eq!(st, 200);
    let gen_name = unique("g");
    let (st, g) = s
        .call(Some(&u), "POST", &format!("{rp}/generate"), Some(json!({"name": gen_name, "include_all_branches": false})))
        .await;
    assert_eq!(st, 201, "{g}");
    assert_eq!(g["fork"], false);
    assert_eq!(g["template_repository"]["name"], name.as_str());
    let w = s.work_dir(&unique("gen"));
    let (ok, out) = s.git(&w, &["clone", "-q", &s.clone_url(&u, &gen_name), "c"]);
    assert!(ok, "{out}");
    assert!(w.join("c/LICENSE").exists());
    let (_, branches) = s.git(&w.join("c"), &["branch", "-r"]);
    assert!(!branches.contains("feature"), "{branches}");

    // Actions reads are validly empty.
    let (st, runs) = s.call(None, "GET", &format!("{rp}/actions/runs"), None).await;
    assert_eq!(st, 200);
    assert_eq!(runs["total_count"], 0);
    let (_, wfs) = s.call(None, "GET", &format!("{rp}/actions/workflows"), None).await;
    assert_eq!(wfs["total_count"], 0);
    let (st, n) = s.call(Some(&u), "GET", "/notifications", None).await;
    assert_eq!((st, n), (200, json!([])));
}
