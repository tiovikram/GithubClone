//! Gaps: commit `comment_count`, repo `subscribers_count`, template generation as fresh root
//! commits, and markdown raw-HTML sanitization.
mod common;
use std::path::Path;

use common::{unique, TestServer};
use serde_json::json;

fn git_ok(s: &TestServer, dir: &Path, args: &[&str]) -> String {
    let (ok, out) = s.git(dir, args);
    assert!(ok, "git {args:?} failed: {out}");
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_comment_count_and_subscribers_count() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let v = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);

    let (st, c) = s.call(Some(&u), "GET", &format!("{rp}/commits/main"), None).await;
    assert_eq!(st, 200, "{c}");
    let sha = c["sha"].as_str().unwrap().to_string();
    assert_eq!(c["commit"]["comment_count"], 0);
    for body in ["first", "second"] {
        let (st, cc) = s.call(Some(&u), "POST", &format!("{rp}/commits/{sha}/comments"), Some(json!({"body": body}))).await;
        assert_eq!(st, 201, "{cc}");
    }
    let (_, c) = s.call(Some(&u), "GET", &format!("{rp}/commits/{sha}"), None).await;
    assert_eq!(c["commit"]["comment_count"], 2, "{c}");
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/commits"), None).await;
    assert_eq!(list[0]["commit"]["comment_count"], 2, "{list}");

    let (_, r) = s.call(Some(&u), "GET", &rp, None).await;
    let before = r["subscribers_count"].as_i64().unwrap();
    let (st, _) = s.call(Some(&v), "PUT", &format!("{rp}/subscription"), Some(json!({"subscribed": true}))).await;
    assert_eq!(st, 200);
    let (_, r) = s.call(Some(&u), "GET", &rp, None).await;
    assert_eq!(r["subscribers_count"].as_i64().unwrap(), before + 1, "{r}");
    // An ignoring watcher is not a subscriber.
    let (st, _) = s.call(Some(&v), "PUT", &format!("{rp}/subscription"), Some(json!({"subscribed": false, "ignored": true}))).await;
    assert_eq!(st, 200);
    let (_, r) = s.call(Some(&u), "GET", &rp, None).await;
    assert_eq!(r["subscribers_count"].as_i64().unwrap(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn template_generation_creates_fresh_root_commits() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    let w = s.work_dir(&unique("w"));
    git_ok(&s, &w, &["clone", "-q", &s.clone_url(&u, &repo), "c"]);
    let c = w.join("c");
    std::fs::write(c.join("a.txt"), "a\n").unwrap();
    git_ok(&s, &c, &["add", "-A"]);
    git_ok(&s, &c, &["commit", "-q", "-m", "second"]);
    git_ok(&s, &c, &["checkout", "-q", "-b", "dev"]);
    std::fs::write(c.join("dev.txt"), "dev\n").unwrap();
    git_ok(&s, &c, &["add", "-A"]);
    git_ok(&s, &c, &["commit", "-q", "-m", "dev work"]);
    git_ok(&s, &c, &["push", "-q", "origin", "main", "dev"]);
    let main_tree = git_ok(&s, &c, &["rev-parse", "main^{tree}"]).trim().to_string();
    let dev_tree = git_ok(&s, &c, &["rev-parse", "dev^{tree}"]).trim().to_string();
    let old_main = git_ok(&s, &c, &["rev-parse", "main"]).trim().to_string();

    let (st, _) = s.call(Some(&u), "PATCH", &rp, Some(json!({"is_template": true}))).await;
    assert_eq!(st, 200);

    // Default branch only.
    let g1 = unique("g");
    let (st, g) = s.call(Some(&u), "POST", &format!("{rp}/generate"), Some(json!({"name": g1}))).await;
    assert_eq!(st, 201, "{g}");
    let gp = format!("/repos/{}/{}", u.login, g1);
    let (st, commits) = s.call(Some(&u), "GET", &format!("{gp}/commits"), None).await;
    assert_eq!(st, 200, "{commits}");
    assert_eq!(commits.as_array().unwrap().len(), 1, "{commits}");
    let root = &commits[0];
    assert_eq!(root["commit"]["message"], "Initial commit");
    assert_eq!(root["parents"], json!([]));
    assert_eq!(root["commit"]["tree"]["sha"], main_tree.as_str());
    assert_ne!(root["sha"], old_main.as_str());
    assert_eq!(root["author"]["login"], u.login.as_str());
    let (_, branches) = s.call(Some(&u), "GET", &format!("{gp}/branches"), None).await;
    assert_eq!(branches.as_array().unwrap().len(), 1, "{branches}");
    // A clone has exactly one commit and the template's files.
    let w2 = s.work_dir(&unique("g1"));
    git_ok(&s, &w2, &["clone", "-q", &s.clone_url(&u, &g1), "c"]);
    assert_eq!(git_ok(&s, &w2.join("c"), &["rev-list", "--count", "HEAD"]).trim(), "1");
    assert!(w2.join("c/a.txt").exists());

    // All branches: each branch tip becomes its own root commit.
    let g2 = unique("g");
    let (st, g) = s.call(Some(&u), "POST", &format!("{rp}/generate"), Some(json!({"name": g2, "include_all_branches": true}))).await;
    assert_eq!(st, 201, "{g}");
    let gp = format!("/repos/{}/{}", u.login, g2);
    let (_, dev) = s.call(Some(&u), "GET", &format!("{gp}/commits?sha=dev"), None).await;
    assert_eq!(dev.as_array().unwrap().len(), 1, "{dev}");
    assert_eq!(dev[0]["commit"]["tree"]["sha"], dev_tree.as_str());
    assert_eq!(dev[0]["parents"], json!([]));
    let (_, main) = s.call(Some(&u), "GET", &format!("{gp}/commits?sha=main"), None).await;
    assert_eq!(main.as_array().unwrap().len(), 1);
    assert_eq!(main[0]["commit"]["tree"]["sha"], main_tree.as_str());
    let (_, r) = s.call(Some(&u), "GET", &gp, None).await;
    assert_eq!(r["default_branch"], "main");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn markdown_allows_safe_html_subset() {
    let s = TestServer::start().await;
    let md = "<details open><summary>More</summary>\n\nbody\n\n</details>\n\n\
              Press <kbd>Ctrl</kbd>, H<sub>2</sub>O, x<sup>2</sup>\n\n\
              <img src=\"https://example.com/a.png\" alt=\"pic\" width=\"20\" onerror=\"alert(1)\">\n\n\
              <a href=\"javascript:alert(1)\" onclick=\"evil()\">bad</a> <a href=\"https://example.com\">good</a>\n\n\
              <script>alert(1)</script><style>body{}</style><iframe src=\"https://x\"></iframe>\n\n\
              <p style=\"color:red\">styled</p>\n\n- [x] done\n";
    for mode in ["gfm", "markdown"] {
        let res = s.client().post(format!("{}/markdown", s.api)).json(&json!({"text": md, "mode": mode})).send().await.unwrap();
        assert_eq!(res.status().as_u16(), 200);
        let html = res.text().await.unwrap();
        assert!(html.contains("<details open"), "{html}");
        assert!(html.contains("<summary>More</summary>"), "{html}");
        assert!(html.contains("<kbd>Ctrl</kbd>"), "{html}");
        assert!(html.contains("<sub>2</sub>") && html.contains("<sup>2</sup>"), "{html}");
        assert!(html.contains("src=\"https://example.com/a.png\"") && html.contains("alt=\"pic\"") && html.contains("width=\"20\""), "{html}");
        assert!(html.contains("href=\"https://example.com\""), "{html}");
        assert!(html.contains("checkbox"), "{html}");
        for bad in ["<script", "alert(1)", "onerror", "onclick", "javascript:", "<style", "<iframe", "style="] {
            assert!(!html.contains(bad), "{bad} survived: {html}");
        }
    }
    // raw mode uses the same sanitizer.
    let res = s
        .client()
        .post(format!("{}/markdown/raw", s.api))
        .header("content-type", "text/plain")
        .body("<b>bold</b><script>x()</script>")
        .send()
        .await
        .unwrap();
    let html = res.text().await.unwrap();
    assert!(html.contains("<b>bold</b>") && !html.contains("<script"), "{html}");
}
