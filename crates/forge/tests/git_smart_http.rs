mod common;
use common::TestServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clone_push_fetch_and_rehydrate() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let w = s.work_dir("a");
    let (ok, out) = s.git(&w, &["clone", &s.clone_url(&u, &repo), "c"]);
    assert!(ok, "{out}");
    let c = w.join("c");
    std::fs::write(c.join("x.txt"), "x").unwrap();
    assert!(s.git(&c, &["add", "."]).0);
    assert!(s.git(&c, &["commit", "-m", "x"]).0);
    let (ok, out) = s.git(&c, &["push", "origin", "HEAD:refs/heads/topic"]);
    assert!(ok, "{out}");

    // Simulate a different pod: drop the local cache; content must come back from storage.
    std::fs::remove_dir_all(s.dir.path().join("cache")).unwrap();
    std::fs::create_dir_all(s.dir.path().join("cache")).unwrap();
    let (ok, out) = s.git(&w, &["clone", "-b", "topic", &s.clone_url(&u, &repo), "d"]);
    assert!(ok, "{out}");
    assert_eq!(std::fs::read_to_string(w.join("d/x.txt")).unwrap(), "x");

    // Anonymous push is challenged.
    let anon = format!("{}/{}/{}.git", s.base, u.login, repo);
    let (ok, _) = s.git(&c, &["push", &anon, "HEAD:refs/heads/anon"]);
    assert!(!ok);
}
