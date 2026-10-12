//! Repository content & git object API: contents, git data (refs/blobs/trees/commits/tags),
//! commits/compare, branches/merges, forks, statuses, archives.
mod common;
use base64::Engine;
use common::{TestServer, TestUser};
use serde_json::{json, Value};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn unb64(v: &Value) -> String {
    let s: String = v.as_str().unwrap().chars().filter(|c| !c.is_whitespace()).collect();
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(s).unwrap()).unwrap()
}

async fn head_sha(s: &TestServer, u: &TestUser, repo: &str, branch: &str) -> String {
    let (st, r) = s.call(Some(u), "GET", &format!("/repos/{}/{repo}/git/ref/heads/{branch}", u.login), None).await;
    assert_eq!(st, 200, "{r}");
    r["object"]["sha"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contents_create_read_update_delete() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let p = format!("/repos/{}/{repo}", u.login);

    // create
    let (st, r) = s.call(Some(&u), "PUT", &format!("{p}/contents/docs/hello.md"), Some(json!({"message": "add hello", "content": b64("# hello\n")}))).await;
    assert_eq!(st, 201, "{r}");
    let sha1 = r["content"]["sha"].as_str().unwrap().to_string();
    assert_eq!(r["content"]["path"], "docs/hello.md");
    assert_eq!(r["commit"]["message"], "add hello");
    assert_eq!(r["commit"]["parents"].as_array().unwrap().len(), 1);

    // read
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/contents/docs/hello.md"), None).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["type"], "file");
    assert_eq!(r["encoding"], "base64");
    assert_eq!(unb64(&r["content"]), "# hello\n");
    assert!(r["_links"]["self"].as_str().unwrap().contains("/contents/docs/hello.md"));

    // raw media type
    let res = s
        .client()
        .get(format!("{}{p}/contents/docs/hello.md", s.api))
        .header("Authorization", format!("token {}", u.token))
        .header("Accept", "application/vnd.github.raw")
        .send()
        .await
        .unwrap();
    assert_eq!(res.text().await.unwrap(), "# hello\n");

    // update without sha -> 422; wrong sha -> 409; right sha -> 200
    let (st, _) = s.call(Some(&u), "PUT", &format!("{p}/contents/docs/hello.md"), Some(json!({"message": "x", "content": b64("x")}))).await;
    assert_eq!(st, 422);
    let bad = "0123456789012345678901234567890123456789";
    let (st, _) = s.call(Some(&u), "PUT", &format!("{p}/contents/docs/hello.md"), Some(json!({"message": "x", "content": b64("x"), "sha": bad}))).await;
    assert_eq!(st, 409);
    let (st, r) = s
        .call(Some(&u), "PUT", &format!("{p}/contents/docs/hello.md"), Some(json!({"message": "update", "content": b64("# hello again\n"), "sha": sha1})))
        .await;
    assert_eq!(st, 200, "{r}");
    let sha2 = r["content"]["sha"].as_str().unwrap().to_string();
    assert_ne!(sha1, sha2);

    // directory listing, root listing, ?ref=
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/contents/docs"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r.as_array().unwrap().len(), 1);
    assert_eq!(r[0]["name"], "hello.md");
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/contents"), None).await;
    assert_eq!(st, 200);
    assert!(r.as_array().unwrap().iter().any(|e| e["type"] == "dir" && e["name"] == "docs"));
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/contents/docs/hello.md?ref=main"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/contents/docs/hello.md?ref=nope"), None).await;
    assert_eq!(st, 404);
    // path traversal is never resolved outside the tree
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/contents/docs/../../etc/passwd"), None).await;
    assert_eq!(st, 404);

    // readme (case-insensitive) in root
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/readme"), None).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["name"], "README.md");

    // delete: missing sha -> 422, then ok, then file is gone
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{p}/contents/docs/hello.md"), Some(json!({"message": "rm"}))).await;
    assert_eq!(st, 422);
    let (st, r) = s.call(Some(&u), "DELETE", &format!("{p}/contents/docs/hello.md"), Some(json!({"message": "rm", "sha": sha2}))).await;
    assert_eq!(st, 200, "{r}");
    assert!(r["content"].is_null());
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/contents/docs/hello.md"), None).await;
    assert_eq!(st, 404);

    // commits list shows the history with Link pagination
    let res = s
        .client()
        .get(format!("{}{p}/commits?per_page=1", s.api))
        .header("Authorization", format!("token {}", u.token))
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("link").unwrap().to_str().unwrap().contains("rel=\"next\""));
    let list: Value = res.json().await.unwrap();
    assert_eq!(list[0]["commit"]["message"], "rm");
    // the API-created commits are attributed to the forge user (matched by email)
    assert_eq!(list[0]["author"]["login"], u.login.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_large_and_empty_repo() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let empty = s.create_repo(&u, false).await;
    let p = format!("/repos/{}/{empty}", u.login);
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/commits"), None).await;
    assert_eq!(st, 409);
    assert_eq!(r["message"], "Git Repository is empty.");
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/contents/"), None).await;
    assert_eq!(st, 404);
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/branches"), None).await;
    assert_eq!((st, r), (200, json!([])));

    // First file into an empty repository creates the default branch.
    let big = vec![b'a'; 1024 * 1024 + 10];
    let content = base64::engine::general_purpose::STANDARD.encode(&big);
    let (st, r) = s.call(Some(&u), "PUT", &format!("{p}/contents/big.txt"), Some(json!({"message": "big", "content": content}))).await;
    assert_eq!(st, 201, "{r}");
    assert_eq!(r["commit"]["parents"].as_array().unwrap().len(), 0);
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/contents/big.txt"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r["encoding"], "none");
    assert_eq!(r["content"], "");
    assert_eq!(r["size"], 1024 * 1024 + 10);

    let bin: Vec<u8> = (0..=255u8).collect();
    let content = base64::engine::general_purpose::STANDARD.encode(&bin);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{p}/contents/bin.dat"), Some(json!({"message": "bin", "content": content}))).await;
    assert_eq!(st, 201);
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/contents/bin.dat"), None).await;
    assert_eq!(st, 200);
    let got = base64::engine::general_purpose::STANDARD
        .decode(r["content"].as_str().unwrap().replace('\n', ""))
        .unwrap();
    assert_eq!(got, bin);
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/commits/main"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r["files"][0]["filename"], "bin.dat");
    assert!(r["files"][0].get("patch").is_none(), "binary files have no patch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refs_crud_and_fast_forward() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let p = format!("/repos/{}/{repo}", u.login);
    let base = head_sha(&s, &u, &repo, "main").await;

    let (st, r) = s.call(Some(&u), "POST", &format!("{p}/git/refs"), Some(json!({"ref": "refs/heads/feature/a", "sha": base}))).await;
    assert_eq!(st, 201, "{r}");
    assert_eq!(r["ref"], "refs/heads/feature/a");
    assert_eq!(r["object"]["type"], "commit");
    let (st, r) = s.call(Some(&u), "POST", &format!("{p}/git/refs"), Some(json!({"ref": "refs/heads/feature/a", "sha": base}))).await;
    assert_eq!(st, 422);
    assert_eq!(r["message"], "Reference already exists");
    let (st, _) = s.call(Some(&u), "POST", &format!("{p}/git/refs"), Some(json!({"ref": "heads/x", "sha": base}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{p}/git/refs"), Some(json!({"ref": "refs/heads/y", "sha": "1111111111111111111111111111111111111111"}))).await;
    assert_eq!(st, 422);

    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/git/ref/heads/feature/a"), None).await;
    assert_eq!(st, 200, "{r}");
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/git/matching-refs/heads/feat"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r.as_array().unwrap().len(), 1);

    // Advance main with a new commit, then move feature/a: ff ok; moving back requires force.
    let (st, r) = s.call(Some(&u), "PUT", &format!("{p}/contents/a.txt"), Some(json!({"message": "a", "content": b64("a")}))).await;
    assert_eq!(st, 201);
    let next = r["commit"]["sha"].as_str().unwrap().to_string();
    let (st, r) = s.call(Some(&u), "PATCH", &format!("{p}/git/refs/heads/feature/a"), Some(json!({"sha": next}))).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["object"]["sha"], next.as_str());
    let (st, r) = s.call(Some(&u), "PATCH", &format!("{p}/git/refs/heads/feature/a"), Some(json!({"sha": base}))).await;
    assert_eq!(st, 422);
    assert_eq!(r["message"], "Update is not a fast forward");
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{p}/git/refs/heads/feature/a"), Some(json!({"sha": base, "force": true}))).await;
    assert_eq!(st, 200);
    let (st, r) = s.call(Some(&u), "PATCH", &format!("{p}/git/refs/heads/nope"), Some(json!({"sha": base}))).await;
    assert_eq!(st, 422);
    assert_eq!(r["message"], "Reference does not exist");

    // branches: list/get (url-encoded slash), rename
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/branches"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r.as_array().unwrap().len(), 2);
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/branches/feature%2Fa"), None).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["commit"]["sha"], base.as_str());
    let (st, r) = s.call(Some(&u), "POST", &format!("{p}/branches/feature%2Fa/rename"), Some(json!({"new_name": "renamed"}))).await;
    assert_eq!(st, 201, "{r}");
    assert_eq!(r["name"], "renamed");

    let (st, _) = s.call(Some(&u), "DELETE", &format!("{p}/git/refs/heads/renamed"), None).await;
    assert_eq!(st, 204);
    let (st, r) = s.call(Some(&u), "DELETE", &format!("{p}/git/refs/heads/renamed"), None).await;
    assert_eq!(st, 422);
    assert_eq!(r["message"], "Reference does not exist");
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/git/ref/heads/renamed"), None).await;
    assert_eq!(st, 404);

    // Permissions: another user can read a public repo but not write.
    let other = s.user().await;
    let (st, _) = s.call(Some(&other), "GET", &format!("{p}/git/ref/heads/main"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&other), "POST", &format!("{p}/git/refs"), Some(json!({"ref": "refs/heads/z", "sha": base}))).await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_data_create_commit_flow_and_compare() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let p = format!("/repos/{}/{repo}", u.login);
    let base = head_sha(&s, &u, &repo, "main").await;

    // The octokit "create a commit via the git data API" flow.
    let (st, c) = s.call(Some(&u), "GET", &format!("{p}/git/commits/{base}"), None).await;
    assert_eq!(st, 200);
    let base_tree = c["tree"]["sha"].as_str().unwrap().to_string();
    let (st, blob) = s.call(Some(&u), "POST", &format!("{p}/git/blobs"), Some(json!({"content": b64("binary\0data"), "encoding": "base64"}))).await;
    assert_eq!(st, 201, "{blob}");
    let blob_sha = blob["sha"].as_str().unwrap().to_string();
    let (st, b) = s.call(Some(&u), "GET", &format!("{p}/git/blobs/{blob_sha}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(b["size"], 11);

    let (st, tree) = s
        .call(
            Some(&u),
            "POST",
            &format!("{p}/git/trees"),
            Some(json!({"base_tree": base_tree, "tree": [
                {"path": "lib/deep/data.bin", "mode": "100644", "type": "blob", "sha": blob_sha},
                {"path": "src/main.rs", "mode": "100644", "type": "blob", "content": "fn main() {}\n"},
                {"path": "run.sh", "mode": "100755", "type": "blob", "content": "#!/bin/sh\n"},
                {"path": "link", "mode": "120000", "type": "blob", "content": "src/main.rs"}
            ]})),
        )
        .await;
    assert_eq!(st, 201, "{tree}");
    let names: Vec<&str> = tree["tree"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(names.contains(&"README.md") && names.contains(&"lib") && names.contains(&"src"), "{names:?}");
    let tree_sha = tree["sha"].as_str().unwrap().to_string();

    // invalid entries are 422
    let (st, _) = s
        .call(Some(&u), "POST", &format!("{p}/git/trees"), Some(json!({"tree": [{"path": "x", "mode": "100644", "type": "blob", "sha": "1111111111111111111111111111111111111111"}]})))
        .await;
    assert_eq!(st, 422);

    let (st, commit) = s
        .call(
            Some(&u),
            "POST",
            &format!("{p}/git/commits"),
            Some(json!({"message": "via git data", "tree": tree_sha, "parents": [base],
                "author": {"name": "Bot", "email": "bot@example.com", "date": "2024-01-01T00:00:00Z"}})),
        )
        .await;
    assert_eq!(st, 201, "{commit}");
    assert_eq!(commit["author"]["date"], "2024-01-01T00:00:00Z");
    assert_eq!(commit["verification"]["verified"], false);
    let commit_sha = commit["sha"].as_str().unwrap().to_string();
    let (st, _) = s.call(Some(&u), "POST", &format!("{p}/git/commits"), Some(json!({"message": "x", "tree": tree_sha, "parents": [blob_sha]}))).await;
    assert_eq!(st, 422);

    // Objects created by the git data API survive losing this pod's cache (they were uploaded).
    std::fs::remove_dir_all(s.dir.path().join("cache")).unwrap();
    std::fs::create_dir_all(s.dir.path().join("cache")).unwrap();
    let (st, _) = s.call(Some(&u), "GET", &format!("{p}/git/commits/{commit_sha}"), None).await;
    assert_eq!(st, 200);

    let (st, r) = s.call(Some(&u), "POST", &format!("{p}/git/refs"), Some(json!({"ref": "refs/heads/feature", "sha": commit_sha}))).await;
    assert_eq!(st, 201, "{r}");

    // recursive tree, symlink & exec bit visible in contents
    let (st, t) = s.call(Some(&u), "GET", &format!("{p}/git/trees/feature?recursive=1"), None).await;
    assert_eq!(st, 200);
    let paths: Vec<&str> = t["tree"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"lib/deep/data.bin") && paths.contains(&"lib/deep"), "{paths:?}");
    let (st, l) = s.call(Some(&u), "GET", &format!("{p}/contents/?ref=feature"), None).await;
    assert_eq!(st, 200);
    let link = l.as_array().unwrap().iter().find(|e| e["name"] == "link").unwrap();
    assert_eq!(link["type"], "symlink");
    // A symlink to a file serves the target's content.
    let (st, f) = s.call(Some(&u), "GET", &format!("{p}/contents/link?ref=feature"), None).await;
    assert_eq!(st, 200);
    assert_eq!(unb64(&f["content"]), "fn main() {}\n");

    // annotated tag
    let (st, tag) = s
        .call(Some(&u), "POST", &format!("{p}/git/tags"), Some(json!({"tag": "v1", "message": "release", "object": commit_sha, "type": "commit"})))
        .await;
    assert_eq!(st, 201, "{tag}");
    let tag_sha = tag["sha"].as_str().unwrap();
    let (st, _) = s.call(Some(&u), "POST", &format!("{p}/git/refs"), Some(json!({"ref": "refs/tags/v1", "sha": tag_sha}))).await;
    assert_eq!(st, 201);
    let (st, tags) = s.call(Some(&u), "GET", &format!("{p}/tags"), None).await;
    assert_eq!(st, 200);
    assert_eq!(tags[0]["name"], "v1");
    assert_eq!(tags[0]["commit"]["sha"], commit_sha.as_str());
    let (st, r) = s.call(Some(&u), "GET", &format!("{p}/git/ref/tags/v1"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r["object"]["type"], "tag");

    // compare main...feature
    let (st, cmp) = s.call(Some(&u), "GET", &format!("{p}/compare/main...feature"), None).await;
    assert_eq!(st, 200, "{cmp}");
    assert_eq!(cmp["status"], "ahead");
    assert_eq!(cmp["ahead_by"], 1);
    assert_eq!(cmp["behind_by"], 0);
    assert_eq!(cmp["merge_base_commit"]["sha"], base.as_str());
    assert_eq!(cmp["files"].as_array().unwrap().len(), 4);
    let (st, cmp) = s.call(Some(&u), "GET", &format!("{p}/compare/feature...main"), None).await;
    assert_eq!(st, 200);
    assert_eq!(cmp["status"], "behind");

    // single commit with stats/files and patch
    let (st, c) = s.call(Some(&u), "GET", &format!("{p}/commits/{commit_sha}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(c["stats"]["additions"], 3);
    let main_rs = c["files"].as_array().unwrap().iter().find(|f| f["filename"] == "src/main.rs").unwrap();
    assert_eq!(main_rs["status"], "added");
    assert!(main_rs["patch"].as_str().unwrap().starts_with("@@"));
    assert!(c["author"].is_null(), "unknown email -> null user");

    // merges API: merge feature into main (201), again (204)
    let (st, m) = s.call(Some(&u), "POST", &format!("{p}/merges"), Some(json!({"base": "main", "head": "feature"}))).await;
    assert_eq!(st, 201, "{m}");
    assert_eq!(m["parents"].as_array().unwrap().len(), 2);
    let (st, _) = s.call(Some(&u), "POST", &format!("{p}/merges"), Some(json!({"base": "main", "head": "feature"}))).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "POST", &format!("{p}/merges"), Some(json!({"base": "nope", "head": "feature"}))).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_then_clone_and_statuses() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let p = format!("/repos/{}/{repo}", owner.login);
    let (st, _) = s.call(Some(&owner), "PUT", &format!("{p}/contents/x.rs"), Some(json!({"message": "x", "content": b64("fn x() {}\n")}))).await;
    assert_eq!(st, 201);

    let forker = s.user().await;
    let (st, f) = s.call(Some(&forker), "POST", &format!("{p}/forks"), Some(json!({}))).await;
    assert_eq!(st, 202, "{f}");
    assert_eq!(f["fork"], true);
    assert_eq!(f["owner"]["login"], forker.login.as_str());
    assert_eq!(f["parent"]["full_name"], format!("{}/{repo}", owner.login));
    let fork_name = f["name"].as_str().unwrap().to_string();

    // Immediately cloneable, with the parent's content.
    let w = s.work_dir("fork");
    let (ok, out) = s.git(&w, &["clone", &s.clone_url(&forker, &fork_name), "c"]);
    assert!(ok, "{out}");
    assert_eq!(std::fs::read_to_string(w.join("c/x.rs")).unwrap(), "fn x() {}\n");

    let (st, r) = s.call(Some(&owner), "GET", &p, None).await;
    assert_eq!(st, 200);
    assert_eq!(r["forks_count"], 1);
    let (st, list) = s.call(Some(&owner), "GET", &format!("{p}/forks"), None).await;
    assert_eq!(st, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Upstream moves on; merge-upstream fast-forwards the fork.
    let (st, _) = s.call(Some(&owner), "PUT", &format!("{p}/contents/y.rs"), Some(json!({"message": "y", "content": b64("y")}))).await;
    assert_eq!(st, 201);
    let fp = format!("/repos/{}/{fork_name}", forker.login);
    let (st, r) = s.call(Some(&forker), "POST", &format!("{fp}/merge-upstream"), Some(json!({"branch": "main"}))).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["merge_type"], "fast-forward");
    let (st, _) = s.call(Some(&forker), "GET", &format!("{fp}/contents/y.rs"), None).await;
    assert_eq!(st, 200);

    // statuses
    let sha = head_sha(&s, &owner, &repo, "main").await;
    let (st, r) = s
        .call(Some(&owner), "POST", &format!("{p}/statuses/{sha}"), Some(json!({"state": "pending", "context": "ci"})))
        .await;
    assert_eq!(st, 201, "{r}");
    let (st, _) = s.call(Some(&owner), "POST", &format!("{p}/statuses/{sha}"), Some(json!({"state": "bogus"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&owner), "POST", &format!("{p}/statuses/{sha}"), Some(json!({"state": "success", "context": "ci"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&owner), "POST", &format!("{p}/statuses/{sha}"), Some(json!({"state": "success", "context": "lint"}))).await;
    assert_eq!(st, 201);
    let (st, c) = s.call(Some(&owner), "GET", &format!("{p}/commits/main/status"), None).await;
    assert_eq!(st, 200);
    assert_eq!(c["state"], "success");
    assert_eq!(c["total_count"], 2);
    let (st, l) = s.call(Some(&owner), "GET", &format!("{p}/commits/main/statuses"), None).await;
    assert_eq!(st, 200);
    assert_eq!(l.as_array().unwrap().len(), 3);
    let (st, cr) = s.call(Some(&owner), "GET", &format!("{p}/commits/main/check-runs"), None).await;
    assert_eq!((st, cr["total_count"].clone()), (200, json!(0)));
    // Non-collaborators can't post statuses.
    let (st, _) = s.call(Some(&forker), "POST", &format!("{p}/statuses/{sha}"), Some(json!({"state": "success"}))).await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_repo_visibility_and_archives() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let name = common::unique("p");
    let (st, _) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": name, "auto_init": true, "private": true}))).await;
    assert_eq!(st, 201);
    let p = format!("/repos/{}/{name}", u.login);
    let other = s.user().await;
    for path in ["/contents/README.md", "/commits", "/branches", "/git/ref/heads/main", "/readme", "/tarball/main"] {
        let (st, _) = s.call(Some(&other), "GET", &format!("{p}{path}"), None).await;
        assert_eq!(st, 404, "{path}");
        let (st, _) = s.call(None, "GET", &format!("{p}{path}"), None).await;
        assert_eq!(st, 404, "{path}");
    }
    let (st, _) = s.call(Some(&other), "PUT", &format!("{p}/contents/x"), Some(json!({"message": "x", "content": b64("x")}))).await;
    assert_eq!(st, 404);

    // tarball redirects to a forge URL that streams the archive (auth carried on same host).
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let res = client
        .get(format!("{}{p}/tarball/main", s.api))
        .header("Authorization", format!("token {}", u.token))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 302);
    let loc = res.headers().get("location").unwrap().to_str().unwrap().to_string();
    assert!(loc.ends_with("/archive/main.tar.gz"), "{loc}");
    let res = client.get(&loc).header("Authorization", format!("token {}", u.token)).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let bytes = res.bytes().await.unwrap();
    assert_eq!(&bytes[..2], &[0x1f, 0x8b], "gzip magic");
    let res = client.get(&loc).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 404, "anonymous can't download a private archive");
}
