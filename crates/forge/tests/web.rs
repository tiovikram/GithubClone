//! Web UI routing: SPA fallback for deep links, and no SPA shell for server-owned paths.
mod common;

use common::TestServer;

async fn get(s: &TestServer, path: &str) -> (u16, String, String) {
    let res = s.client().get(format!("{}{}", s.base, path)).send().await.unwrap();
    let status = res.status().as_u16();
    let ct = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    (status, ct, res.text().await.unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn spa_fallback_and_reserved_paths() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;

    // Deep links into the UI get the SPA shell (or the placeholder when web/dist is not built).
    for p in [
        format!("/{}/{repo}", u.login),
        format!("/{}/{repo}/tree/main/a/b", u.login),
        format!("/{}/{repo}/blob/feature/x/README.md", u.login),
        format!("/{}/{repo}/releases/tag/v1.0", u.login),
        "/search?q=x".to_string(),
        "/settings/keys".to_string(),
    ] {
        let (st, _, _) = get(&s, &p).await;
        assert_eq!(st, 200, "{p}");
    }

    // Unknown API paths are JSON 404s, never the SPA.
    for p in ["/api/forge/nope", "/api/nope", "/api/uploads/nope", "/api/v3/nope"] {
        let (st, ct, body) = get(&s, p).await;
        assert_eq!(st, 404, "{p}");
        assert!(ct.starts_with("application/json"), "{p}: {ct}");
        assert!(body.contains("Not Found"), "{p}: {body}");
    }

    // A missing hashed asset is a 404, not index.html.
    let (st, _, _) = get(&s, "/assets/index-doesnotexist.js").await;
    assert_eq!(st, 404);

    // Server-owned routes under /{owner}/{repo} are not shadowed by the SPA.
    let (st, ct, body) = get(&s, &format!("/{}/{repo}/raw/main/README.md", u.login)).await;
    assert_eq!(st, 200);
    assert!(!ct.contains("text/html"), "{ct}");
    assert!(body.contains(&repo));
    let (st, ct, _) = get(&s, &format!("/{}/{repo}/archive/refs/heads/main.zip", u.login)).await;
    assert_eq!(st, 200);
    assert!(!ct.contains("text/html"), "{ct}");
    let (st, ct, _) = get(&s, &format!("/{}/{repo}/info/refs?service=git-upload-pack", u.login)).await;
    assert_eq!(st, 200);
    assert!(ct.contains("git-upload-pack"), "{ct}");
    let (st, ct, _) = get(&s, &format!("/{}.keys", u.login)).await;
    assert_eq!(st, 200);
    assert!(ct.starts_with("text/plain"), "{ct}");
    let (st, ct, _) = get(&s, &format!("/avatars/{}", u.login)).await;
    assert_eq!(st, 200);
    assert!(ct.starts_with("image/svg"), "{ct}");
}
