//! Packages: the OCI container registry (`/v2/`), the npm registry (`/npm/`), the REST packages
//! API on top of them, permissions and garbage collection.
mod common;
use base64::Engine;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// `forge gc` with zero grace periods would collect other tests' in-flight uploads: the GC test
/// takes this lock exclusively, every other test shares it.
fn gc_lock() -> &'static tokio::sync::RwLock<()> {
    static L: std::sync::OnceLock<tokio::sync::RwLock<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| tokio::sync::RwLock::new(()))
}

fn digest(b: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(b)))
}

struct Res {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Res {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn header(&self, k: &str) -> String {
        self.headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
    }
    fn code(&self) -> String {
        self.json()["errors"][0]["code"].as_str().unwrap_or("").to_string()
    }
}

/// A registry request. `auth`: `Some(user)` sends the PAT as a bearer token.
async fn reg(s: &TestServer, method: &str, path: &str, auth: Option<&TestUser>, body: Option<Vec<u8>>, headers: &[(&str, &str)]) -> Res {
    let url = if path.starts_with("http") { path.to_string() } else { format!("{}{path}", s.base) };
    let mut req = s.client().request(method.parse().unwrap(), url);
    if let Some(u) = auth {
        req = req.header("Authorization", format!("Bearer {}", u.token));
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some(b) = body {
        req = req.body(b);
    }
    let res = req.send().await.unwrap();
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    let body = res.bytes().await.unwrap().to_vec();
    Res { status, headers, body }
}

async fn push_blob(s: &TestServer, u: &TestUser, name: &str, data: &[u8]) -> String {
    let d = digest(data);
    let r = reg(s, "POST", &format!("/v2/{name}/blobs/uploads/?digest={d}"), Some(u), Some(data.to_vec()), &[]).await;
    assert_eq!(r.status, 201, "monolithic upload: {}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("docker-content-digest"), d);
    d
}

/// Pushes an image manifest (config + one layer) and returns (manifest digest, manifest bytes).
async fn push_image(s: &TestServer, u: &TestUser, name: &str, tag: &str, labels: Value) -> (String, Vec<u8>) {
    let layer = format!("layer {tag} {}", unique("x")).into_bytes();
    let config = serde_json::to_vec(&json!({"architecture": "amd64", "os": "linux", "config": {"Labels": labels}, "rootfs": {"type": "layers", "diff_ids": [digest(&layer)]}})).unwrap();
    let ld = push_blob(s, u, name, &layer).await;
    let cd = push_blob(s, u, name, &config).await;
    let manifest = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": cd, "size": config.len()},
        "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": ld, "size": layer.len()}],
    }))
    .unwrap();
    let r = reg(s, "PUT", &format!("/v2/{name}/manifests/{tag}"), Some(u), Some(manifest.clone()), &[("content-type", "application/vnd.oci.image.manifest.v1+json")]).await;
    assert_eq!(r.status, 201, "manifest push: {}", String::from_utf8_lossy(&r.body));
    let md = digest(&manifest);
    assert_eq!(r.header("docker-content-digest"), md);
    (md, manifest)
}

async fn create_org(s: &TestServer, u: &TestUser) -> String {
    let login = unique("org");
    let res = s
        .client()
        .post(format!("{}/api/forge/orgs", s.base))
        .header("Authorization", format!("token {}", u.token))
        .json(&json!({"login": login}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    login
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_auth_challenge_and_token() {
    let _g = gc_lock().read().await;
    let s = TestServer::start().await;
    let u = s.user().await;
    let r = reg(&s, "GET", "/v2/", None, None, &[]).await;
    assert_eq!(r.status, 401);
    let host = s.base.trim_start_matches("http://");
    assert_eq!(r.header("www-authenticate"), format!("Bearer realm=\"{}/v2/token\",service=\"{host}\"", s.base));
    assert_eq!(r.header("docker-distribution-api-version"), "registry/2.0");
    assert_eq!(r.code(), "UNAUTHORIZED");

    // Basic login + PAT, and login + password, both yield a registry token.
    for secret in [u.token.as_str(), "password123"] {
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{}:{secret}", u.login));
        let r = reg(&s, "GET", &format!("/v2/token?service={host}&scope=repository:{}/x:pull,push", u.login), None, None, &[("authorization", &format!("Basic {basic}"))]).await;
        assert_eq!(r.status, 200);
        let tok = r.json()["token"].as_str().unwrap().to_string();
        assert_eq!(r.json()["access_token"], tok.as_str());
        let r = reg(&s, "GET", "/v2/", None, None, &[("authorization", &format!("Bearer {tok}"))]).await;
        assert_eq!(r.status, 200);
    }
    // docker's OAuth2 flavour.
    let form = format!("grant_type=password&username={}&password={}&service={host}&client_id=docker", u.login, u.token);
    let r = reg(&s, "POST", "/v2/token", None, Some(form.into_bytes()), &[("content-type", "application/x-www-form-urlencoded")]).await;
    assert_eq!(r.status, 200);
    // Bad credentials.
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{}:wrong", u.login));
    let r = reg(&s, "GET", "/v2/token", None, None, &[("authorization", &format!("Basic {basic}"))]).await;
    assert_eq!(r.status, 401);
    let r = reg(&s, "GET", "/v2/", None, None, &[("authorization", "Bearer fgr_1.99999999999.deadbeef")]).await;
    assert_eq!(r.status, 401);
    // Anonymous token: valid, but anonymous.
    let r = reg(&s, "GET", "/v2/token", None, None, &[]).await;
    let tok = r.json()["token"].as_str().unwrap().to_string();
    let r = reg(&s, "GET", "/v2/", None, None, &[("authorization", &format!("Bearer {tok}"))]).await;
    assert_eq!(r.status, 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_pull_chunked_tags_and_errors() {
    let _g = gc_lock().read().await;
    let s = TestServer::start().await;
    let u = s.user().await;
    let name = format!("{}/app/web", u.login);

    // Chunked upload with Content-Range, status, out-of-order chunk, finish.
    let blob: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let d = digest(&blob);
    let r = reg(&s, "POST", &format!("/v2/{name}/blobs/uploads/"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 202);
    let loc = r.header("location");
    assert_eq!(r.header("range"), "0-0");
    let r = reg(&s, "PATCH", &loc, Some(&u), Some(blob[..60_000].to_vec()), &[("content-range", "0-59999")]).await;
    assert_eq!(r.status, 202);
    assert_eq!(r.header("range"), "0-59999");
    let loc = r.header("location");
    let r = reg(&s, "PATCH", &loc, Some(&u), Some(blob[10..20].to_vec()), &[("content-range", "10-19")]).await;
    assert_eq!(r.status, 416);
    assert_eq!(r.header("range"), "0-59999");
    let r = reg(&s, "GET", &loc, Some(&u), None, &[]).await;
    assert_eq!(r.status, 204);
    assert_eq!(r.header("range"), "0-59999");
    let r = reg(&s, "PUT", &format!("{loc}?digest={d}"), Some(&u), Some(blob[60_000..].to_vec()), &[]).await;
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("location"), format!("/v2/{name}/blobs/{d}"));

    // Digest mismatch.
    let r = reg(&s, "POST", &format!("/v2/{name}/blobs/uploads/?digest={}", digest(b"other")), Some(&u), Some(b"content".to_vec()), &[]).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "DIGEST_INVALID");

    // Blob reads: HEAD, GET, Range.
    let r = reg(&s, "HEAD", &format!("/v2/{name}/blobs/{d}"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-length"), "100000");
    let r = reg(&s, "GET", &format!("/v2/{name}/blobs/{d}"), Some(&u), None, &[]).await;
    assert_eq!(r.body, blob);
    let r = reg(&s, "GET", &format!("/v2/{name}/blobs/{d}"), Some(&u), None, &[("range", "bytes=10-19")]).await;
    assert_eq!(r.status, 206);
    assert_eq!(r.body, blob[10..20].to_vec());
    assert_eq!(r.header("content-range"), "bytes 10-19/100000");

    // A manifest that references an unknown blob is rejected.
    let bad = serde_json::to_vec(&json!({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": digest(b"nope"), "size": 4}, "layers": []}))
    .unwrap();
    let r = reg(&s, "PUT", &format!("/v2/{name}/manifests/bad"), Some(&u), Some(bad), &[("content-type", "application/vnd.oci.image.manifest.v1+json")]).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "MANIFEST_BLOB_UNKNOWN");

    // Push two tags, a Docker v2 schema2 manifest, and an index over both.
    let (d1, m1) = push_image(&s, &u, &name, "v1", json!({})).await;
    let (d2, _) = push_image(&s, &u, &name, "v2", json!({})).await;
    let r = reg(&s, "GET", &format!("/v2/{name}/manifests/v1"), Some(&u), None, &[("accept", "application/vnd.oci.image.manifest.v1+json")]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, m1);
    assert_eq!(r.header("content-type"), "application/vnd.oci.image.manifest.v1+json");
    assert_eq!(r.header("docker-content-digest"), d1);
    let r = reg(&s, "HEAD", &format!("/v2/{name}/manifests/{d2}"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 200);
    let idx = serde_json::to_vec(&json!({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json", "manifests": [
        {"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": d1, "size": m1.len(), "platform": {"os": "linux", "architecture": "amd64"}},
        {"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": d2, "size": 10, "platform": {"os": "linux", "architecture": "arm64"}},
    ]}))
    .unwrap();
    let r = reg(&s, "PUT", &format!("/v2/{name}/manifests/multi"), Some(&u), Some(idx.clone()), &[("content-type", "application/vnd.oci.image.index.v1+json")]).await;
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    // By digest: the reference must match.
    let r = reg(&s, "PUT", &format!("/v2/{name}/manifests/{}", digest(b"x")), Some(&u), Some(idx.clone()), &[("content-type", "application/vnd.oci.image.index.v1+json")]).await;
    assert_eq!(r.code(), "DIGEST_INVALID");
    // Schema 1 is refused.
    let r = reg(&s, "PUT", &format!("/v2/{name}/manifests/old"), Some(&u), Some(br#"{"schemaVersion":1,"name":"x","tag":"old","fsLayers":[]}"#.to_vec()), &[("content-type", "application/vnd.docker.distribution.manifest.v1+prettyjws")]).await;
    assert_eq!(r.code(), "MANIFEST_INVALID");

    // Tags list with pagination.
    let r = reg(&s, "GET", &format!("/v2/{name}/tags/list"), Some(&u), None, &[]).await;
    assert_eq!(r.json(), json!({"name": name, "tags": ["multi", "v1", "v2"]}));
    let r = reg(&s, "GET", &format!("/v2/{name}/tags/list?n=2"), Some(&u), None, &[]).await;
    assert_eq!(r.json()["tags"], json!(["multi", "v1"]));
    assert!(r.header("link").contains("last=v1"), "{}", r.header("link"));
    let r = reg(&s, "GET", &format!("/v2/{name}/tags/list?n=2&last=v1"), Some(&u), None, &[]).await;
    assert_eq!(r.json()["tags"], json!(["v2"]));

    // Referrers: an artifact whose subject is v1.
    let empty = push_blob(&s, &u, &name, b"{}").await;
    let sig = serde_json::to_vec(&json!({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json", "artifactType": "application/vnd.example.sig",
        "config": {"mediaType": "application/vnd.oci.empty.v1+json", "digest": empty, "size": 2}, "layers": [],
        "subject": {"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": d1, "size": m1.len()}}))
    .unwrap();
    let r = reg(&s, "PUT", &format!("/v2/{name}/manifests/{}", digest(&sig)), Some(&u), Some(sig.clone()), &[("content-type", "application/vnd.oci.image.manifest.v1+json")]).await;
    assert_eq!(r.status, 201);
    assert_eq!(r.header("oci-subject"), d1);
    let r = reg(&s, "GET", &format!("/v2/{name}/referrers/{d1}"), Some(&u), None, &[]).await;
    let refs = r.json();
    assert_eq!(refs["manifests"].as_array().unwrap().len(), 1);
    assert_eq!(refs["manifests"][0]["artifactType"], "application/vnd.example.sig");

    // REST: versions with tags.
    let (st, vs) = s.call(Some(&u), "GET", "/user/packages/container/app%2Fweb/versions", None).await;
    assert_eq!(st, 200);
    let vs = vs.as_array().unwrap().clone();
    assert_eq!(vs.len(), 4);
    let v1 = vs.iter().find(|v| v["name"] == d1.as_str()).unwrap();
    assert_eq!(v1["metadata"], json!({"package_type": "container", "container": {"tags": ["v1"]}}));

    // Deleting a tag over the registry leaves the version; deleting the digest removes it.
    let r = reg(&s, "DELETE", &format!("/v2/{name}/manifests/v2"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 202);
    let r = reg(&s, "GET", &format!("/v2/{name}/manifests/v2"), Some(&u), None, &[]).await;
    assert_eq!(r.code(), "MANIFEST_UNKNOWN");
    let r = reg(&s, "GET", &format!("/v2/{name}/manifests/{d2}"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 200);
    let r = reg(&s, "DELETE", &format!("/v2/{name}/manifests/{d2}"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 202);
    let r = reg(&s, "GET", &format!("/v2/{name}/manifests/{d2}"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 404);
    let (_, vs) = s.call(Some(&u), "GET", "/user/packages/container/app%2Fweb/versions?state=deleted", None).await;
    assert_eq!(vs.as_array().unwrap().len(), 1);
    assert!(vs[0]["deleted_at"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn container_permissions_follow_github() {
    let _g = gc_lock().read().await;
    let s = TestServer::start().await;
    let owner = s.user().await;
    let other = s.user().await;
    let collab = s.user().await;
    let name = format!("{}/private-img", owner.login);
    let (d, _) = push_image(&s, &owner, &name, "latest", json!({})).await;

    // Private by default: anonymous gets a challenge, strangers a 404.
    let r = reg(&s, "GET", &format!("/v2/{name}/manifests/latest"), None, None, &[]).await;
    assert_eq!(r.status, 401);
    assert!(r.header("www-authenticate").contains(&format!("scope=\"repository:{name}:pull\"")));
    let r = reg(&s, "GET", &format!("/v2/{name}/manifests/latest"), Some(&other), None, &[]).await;
    assert_eq!(r.status, 404);
    assert_eq!(r.code(), "NAME_UNKNOWN");
    let (st, _) = s.call(Some(&other), "GET", &format!("/users/{}/packages/container/private-img", owner.login), None).await;
    assert_eq!(st, 404);
    // Nobody pushes into someone else's namespace.
    let r = reg(&s, "POST", &format!("/v2/{}/mine/blobs/uploads/", owner.login), Some(&other), None, &[]).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "DENIED");
    // Cross-repository mount from an unreadable package falls back to a fresh upload.
    let r = reg(&s, "POST", &format!("/v2/{}/stolen/blobs/uploads/?mount={d}&from={name}", other.login), Some(&other), None, &[]).await;
    assert_eq!(r.status, 202);

    // An image labelled with a public repository of the owner is linked to it, public, and
    // inherits its access: a collaborator with write access may push.
    let repo = s.create_repo(&owner, true).await;
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/repos/{}/{repo}/collaborators/{}", owner.login, collab.login), Some(json!({"permission": "push"}))).await;
    assert!(st == 201 || st == 204, "{st}");
    let linked = format!("{}/linked", owner.login);
    let source = format!("{}/{}/{repo}", s.base, owner.login);
    push_image(&s, &owner, &linked, "v1", json!({"org.opencontainers.image.source": source, "org.opencontainers.image.description": "hello"})).await;
    let (st, p) = s.call(None, "GET", &format!("/users/{}/packages/container/linked", owner.login), None).await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["visibility"], "public");
    assert_eq!(p["repository"]["name"], repo.as_str());
    let r = reg(&s, "GET", &format!("/v2/{linked}/manifests/v1"), None, None, &[]).await;
    assert_eq!(r.status, 200);
    // Mount from the readable public package works.
    let (_, vs) = s.call(Some(&owner), "GET", &format!("/users/{}/packages/container/linked/versions", owner.login), None).await;
    assert_eq!(vs[0]["description"], "hello");
    push_image(&s, &collab, &linked, "v2", json!({})).await;
    let r = reg(&s, "PUT", &format!("/v2/{linked}/manifests/v3"), Some(&other), Some(b"{}".to_vec()), &[]).await;
    assert_eq!(r.status, 403);
    // Write access is not admin: the collaborator can't delete versions.
    let vid = vs[0]["id"].as_i64().unwrap();
    let (st, _) = s.call(Some(&collab), "DELETE", &format!("/users/{}/packages/container/linked/versions/{vid}", owner.login), None).await;
    assert_eq!(st, 403);

    // Organizations: members publish, non-members can't.
    let org = create_org(&s, &owner).await;
    let r = reg(&s, "POST", &format!("/v2/{org}/svc/blobs/uploads/"), Some(&other), None, &[]).await;
    assert_eq!(r.status, 403);
    push_image(&s, &owner, &format!("{org}/svc"), "v1", json!({})).await;
    let (st, list) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/packages?package_type=container"), None).await;
    assert_eq!(st, 200);
    assert_eq!(list[0]["name"], "svc");
    assert_eq!(list[0]["version_count"], 1);
    let (st, list) = s.call(Some(&other), "GET", &format!("/orgs/{org}/packages?package_type=container"), None).await;
    assert_eq!(st, 200);
    assert_eq!(list, json!([]));
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/packages"), None).await;
    assert_eq!(st, 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_restore_and_namespace_reuse() {
    let _g = gc_lock().read().await;
    let s = TestServer::start().await;
    let u = s.user().await;
    let name = format!("{}/tool", u.login);
    let (d1, _) = push_image(&s, &u, &name, "v1", json!({})).await;
    push_image(&s, &u, &name, "v2", json!({})).await;

    let (st, p) = s.call(Some(&u), "GET", "/user/packages/container/tool", None).await;
    assert_eq!(st, 200);
    assert_eq!(p["version_count"], 2);
    assert_eq!(p["url"], format!("{}/users/{}/packages/container/tool", s.api, u.login));
    let (_, vs) = s.call(Some(&u), "GET", "/user/packages/container/tool/versions?per_page=1", None).await;
    assert_eq!(vs.as_array().unwrap().len(), 1);
    let (_, vs) = s.call(Some(&u), "GET", "/user/packages/container/tool/versions", None).await;
    let v1 = vs.as_array().unwrap().iter().find(|v| v["name"] == d1.as_str()).unwrap()["id"].as_i64().unwrap();

    // Version delete + restore.
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/user/packages/container/tool/versions/{v1}"), None).await;
    assert_eq!(st, 204);
    assert_eq!(reg(&s, "GET", &format!("/v2/{name}/manifests/v1"), Some(&u), None, &[]).await.status, 404);
    let (st, _) = s.call(Some(&u), "POST", &format!("/user/packages/container/tool/versions/{v1}/restore"), None).await;
    assert_eq!(st, 204);
    assert_eq!(reg(&s, "GET", &format!("/v2/{name}/manifests/v1"), Some(&u), None, &[]).await.status, 200);

    // Package delete: gone from the registry and the API, the name is free again.
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/users/{}/packages/container/tool", u.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", "/user/packages/container/tool", None).await;
    assert_eq!(st, 404);
    assert_eq!(reg(&s, "GET", &format!("/v2/{name}/tags/list"), Some(&u), None, &[]).await.code(), "NAME_UNKNOWN");
    let (st, _) = s.call(Some(&u), "POST", "/user/packages/container/tool/restore", None).await;
    assert_eq!(st, 204);
    let (_, p) = s.call(Some(&u), "GET", "/user/packages/container/tool", None).await;
    assert_eq!(p["version_count"], 2);

    // Restoring is refused while a new package uses the name.
    s.call(Some(&u), "DELETE", "/user/packages/container/tool", None).await;
    push_image(&s, &u, &name, "fresh", json!({})).await;
    let (st, _) = s.call(Some(&u), "POST", "/user/packages/container/tool/restore", None).await;
    assert_eq!(st, 403);
    let (_, p) = s.call(Some(&u), "GET", "/user/packages/container/tool", None).await;
    assert_eq!(p["version_count"], 1);
    let (st, conflicts) = s.call(Some(&u), "GET", "/user/docker/conflicts", None).await;
    assert_eq!((st, conflicts), (200, json!([])));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn garbage_collection_keeps_referenced_blobs() {
    let _g = gc_lock().write().await;
    let s = TestServer::start().await;
    let u = s.user().await;
    let name = format!("{}/gc", u.login);
    let orphan = push_blob(&s, &u, &name, format!("orphan {}", unique("o")).as_bytes()).await;
    let (d, _) = push_image(&s, &u, &name, "v1", json!({})).await;
    // Abandoned upload session.
    let r = reg(&s, "POST", &format!("/v2/{name}/blobs/uploads/"), Some(&u), None, &[]).await;
    let loc = r.header("location");
    reg(&s, "PATCH", &loc, Some(&u), Some(b"partial".to_vec()), &[]).await;

    let zero = chrono::Duration::zero();
    // Links not yet expired: nothing referenced is collected.
    forge::packages::gc::run_with(&s.state, chrono::Duration::days(30), zero, chrono::Duration::hours(24)).await.unwrap();
    assert_eq!(reg(&s, "HEAD", &format!("/v2/{name}/blobs/{orphan}"), Some(&u), None, &[]).await.status, 200);
    // Expire repository links and uploads: the orphan goes, the image stays.
    let st = forge::packages::gc::run_with(&s.state, chrono::Duration::days(30), zero, zero).await.unwrap();
    assert!(st.uploads_dropped >= 1);
    assert_eq!(reg(&s, "HEAD", &format!("/v2/{name}/blobs/{orphan}"), Some(&u), None, &[]).await.status, 404);
    assert_eq!(reg(&s, "GET", &loc, Some(&u), None, &[]).await.status, 404);
    let r = reg(&s, "GET", &format!("/v2/{name}/manifests/v1"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 200);
    let m: Value = r.json();
    let layer = m["layers"][0]["digest"].as_str().unwrap().to_string();
    assert_eq!(reg(&s, "GET", &format!("/v2/{name}/blobs/{layer}"), Some(&u), None, &[]).await.status, 200);

    // Purging the deleted package frees its blobs.
    s.call(Some(&u), "DELETE", "/user/packages/container/gc", None).await;
    forge::packages::gc::run_with(&s.state, zero, zero, zero).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM package_blobs WHERE digest = ANY($1)")
        .bind(vec![d.clone(), layer.clone()])
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let (st, _) = s.call(Some(&u), "POST", "/user/packages/container/gc/restore", None).await;
    assert_eq!(st, 404);
}

fn npm_doc(base: &str, scope: &str, name: &str, version: &str, tgz: &[u8], repo: Option<&str>) -> Value {
    let full = format!("@{scope}/{name}");
    let file = format!("{name}-{version}.tgz");
    let mut v = json!({
        "name": full, "version": version, "description": "test package", "license": "MIT",
        "dist": {"shasum": hex::encode(sha1::Sha1::digest(tgz)), "tarball": format!("{base}/npm/{full}/-/{file}")},
    });
    if let Some(r) = repo {
        v["repository"] = json!({"type": "git", "url": format!("git+{r}.git")});
    }
    json!({
        "_id": full, "name": full, "dist-tags": {"latest": version},
        "versions": {version: v},
        "_attachments": {file: {"content_type": "application/octet-stream", "data": base64::engine::general_purpose::STANDARD.encode(tgz), "length": tgz.len()}},
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npm_publish_install_dist_tags_unpublish() {
    let _g = gc_lock().read().await;
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let pkg = format!("@{}%2flib", u.login);
    let tgz = b"fake tarball 1".to_vec();
    let doc = npm_doc(&s.base, &u.login, "lib", "1.0.0", &tgz, None);
    // Anonymous and foreign publishes are refused.
    let r = reg(&s, "PUT", &format!("/npm/{pkg}"), None, Some(serde_json::to_vec(&doc).unwrap()), &[("content-type", "application/json")]).await;
    assert_eq!(r.status, 401);
    let r = reg(&s, "PUT", &format!("/npm/{pkg}"), Some(&other), Some(serde_json::to_vec(&doc).unwrap()), &[("content-type", "application/json")]).await;
    assert_eq!(r.status, 403);
    let r = reg(&s, "PUT", &format!("/npm/{pkg}"), Some(&u), Some(serde_json::to_vec(&doc).unwrap()), &[("content-type", "application/json")]).await;
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    // Same version again: refused.
    let r = reg(&s, "PUT", &format!("/npm/{pkg}"), Some(&u), Some(serde_json::to_vec(&doc).unwrap()), &[("content-type", "application/json")]).await;
    assert_eq!(r.status, 409);
    // Corrupt shasum.
    let mut bad = npm_doc(&s.base, &u.login, "lib", "1.0.1", b"x", None);
    bad["versions"]["1.0.1"]["dist"]["shasum"] = json!("0000");
    let r = reg(&s, "PUT", &format!("/npm/{pkg}"), Some(&u), Some(serde_json::to_vec(&bad).unwrap()), &[]).await;
    assert_eq!(r.status, 400);
    let tgz2 = b"fake tarball 2".to_vec();
    let mut doc2 = npm_doc(&s.base, &u.login, "lib", "2.0.0", &tgz2, None);
    doc2["dist-tags"] = json!({"next": "2.0.0"});
    let r = reg(&s, "PUT", &format!("/npm/{pkg}"), Some(&u), Some(serde_json::to_vec(&doc2).unwrap()), &[]).await;
    assert_eq!(r.status, 201);

    // Packument (both URL spellings), version document, tarball.
    let r = reg(&s, "GET", &format!("/npm/@{}/lib", u.login), Some(&u), None, &[]).await;
    assert_eq!(r.status, 200);
    let p = r.json();
    assert_eq!(p["name"], format!("@{}/lib", u.login));
    assert_eq!(p["dist-tags"], json!({"latest": "1.0.0", "next": "2.0.0"}));
    let tarball = p["versions"]["1.0.0"]["dist"]["tarball"].as_str().unwrap().to_string();
    assert_eq!(tarball, format!("{}/npm/@{}/lib/-/lib-1.0.0.tgz", s.base, u.login));
    assert!(p["versions"]["1.0.0"]["dist"]["integrity"].as_str().unwrap().starts_with("sha512-"));
    let r = reg(&s, "GET", &tarball, Some(&u), None, &[]).await;
    assert_eq!(r.body, tgz);
    let r = reg(&s, "GET", &format!("/npm/{pkg}/next"), Some(&u), None, &[]).await;
    assert_eq!(r.json()["version"], "2.0.0");
    // Private: anonymous 401, strangers 404.
    assert_eq!(reg(&s, "GET", &format!("/npm/{pkg}"), None, None, &[]).await.status, 401);
    assert_eq!(reg(&s, "GET", &format!("/npm/{pkg}"), Some(&other), None, &[]).await.status, 404);
    // whoami
    assert_eq!(reg(&s, "GET", "/npm/-/whoami", Some(&u), None, &[]).await.json()["username"], u.login.as_str());

    // dist-tags
    let r = reg(&s, "PUT", &format!("/npm/-/package/{pkg}/dist-tags/stable"), Some(&u), Some(br#""1.0.0""#.to_vec()), &[("content-type", "application/json")]).await;
    assert_eq!(r.status, 201);
    let r = reg(&s, "GET", &format!("/npm/-/package/{pkg}/dist-tags"), Some(&u), None, &[]).await;
    assert_eq!(r.json()["stable"], "1.0.0");
    let r = reg(&s, "DELETE", &format!("/npm/-/package/{pkg}/dist-tags/stable"), Some(&u), None, &[]).await;
    assert_eq!(r.status, 200);

    // REST view.
    let (st, v) = s.call(Some(&u), "GET", "/user/packages/npm/lib/versions", None).await;
    assert_eq!(st, 200);
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert_eq!(v[0]["metadata"], json!({"package_type": "npm"}));
    assert_eq!(v[0]["license"], "MIT");

    // Unpublish one version (packument PUT without it), then the whole package.
    let mut p2 = p.clone();
    p2["versions"].as_object_mut().unwrap().remove("1.0.0");
    let rev = p["_rev"].as_str().unwrap();
    let r = reg(&s, "PUT", &format!("/npm/{pkg}/-rev/{rev}"), Some(&u), Some(serde_json::to_vec(&p2).unwrap()), &[]).await;
    assert_eq!(r.status, 200);
    let p = reg(&s, "GET", &format!("/npm/{pkg}"), Some(&u), None, &[]).await.json();
    assert_eq!(p["versions"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["2.0.0"]);
    assert_eq!(reg(&s, "GET", &tarball, Some(&u), None, &[]).await.status, 404);
    let r = reg(&s, "DELETE", &format!("/npm/{pkg}/-rev/{}", p["_rev"].as_str().unwrap()), Some(&u), None, &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(reg(&s, "GET", &format!("/npm/{pkg}"), Some(&u), None, &[]).await.status, 404);
    let (st, _) = s.call(Some(&u), "POST", "/user/packages/npm/lib/restore", None).await;
    assert_eq!(st, 204);
    assert_eq!(reg(&s, "GET", &format!("/npm/{pkg}"), Some(&u), None, &[]).await.status, 200);

    // Unscoped names are not served.
    assert_eq!(reg(&s, "GET", "/npm/lodash", Some(&u), None, &[]).await.status, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn npm_repository_link_and_login() {
    let _g = gc_lock().read().await;
    let s = TestServer::start().await;
    let u = s.user().await;
    let reader = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let pkg = format!("@{}%2flinked", u.login);
    let doc = npm_doc(&s.base, &u.login, "linked", "0.1.0", b"tgz", Some(&format!("{}/{}/{repo}", s.base, u.login)));
    let r = reg(&s, "PUT", &format!("/npm/{pkg}"), Some(&u), Some(serde_json::to_vec(&doc).unwrap()), &[]).await;
    assert_eq!(r.status, 201);
    // Linked to a public repository: public.
    assert_eq!(reg(&s, "GET", &format!("/npm/{pkg}"), Some(&reader), None, &[]).await.status, 200);
    let (_, p) = s.call(None, "GET", &format!("/users/{}/packages/npm/linked", u.login), None).await;
    assert_eq!(p["repository"]["full_name"], format!("{}/{repo}", u.login));
    // Readers can't publish.
    let doc = npm_doc(&s.base, &u.login, "linked", "0.2.0", b"tgz2", None);
    assert_eq!(reg(&s, "PUT", &format!("/npm/{pkg}"), Some(&reader), Some(serde_json::to_vec(&doc).unwrap()), &[]).await.status, 403);

    // npm login (legacy): login + password -> a working token.
    let r = reg(&s, "PUT", &format!("/npm/-/user/org.couchdb.user:{}", u.login), None, Some(serde_json::to_vec(&json!({"name": u.login, "password": "password123"})).unwrap()), &[("content-type", "application/json")]).await;
    assert_eq!(r.status, 201);
    let token = r.json()["token"].as_str().unwrap().to_string();
    let r = reg(&s, "GET", "/npm/-/whoami", None, None, &[("authorization", &format!("Bearer {token}"))]).await;
    assert_eq!(r.json()["username"], u.login.as_str());
    let r = reg(&s, "PUT", &format!("/npm/-/user/org.couchdb.user:{}", u.login), None, Some(serde_json::to_vec(&json!({"name": u.login, "password": "nope"})).unwrap()), &[]).await;
    assert_eq!(r.status, 401);
}

/// Two pods sharing Postgres and object storage: a chunked upload started on one finishes on the
/// other, and either serves the result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uploads_hop_between_pods() {
    let _g = gc_lock().read().await;
    let a = TestServer::start().await;
    let storage = format!("file://{}", a.dir.path().join("objects").display());
    let b = TestServer::start_with(|c| c.storage = storage.clone()).await;
    let u = a.user().await;
    let name = format!("{}/hop", u.login);
    let blob: Vec<u8> = (0..50_000u32).map(|i| (i % 13) as u8).collect();
    let d = digest(&blob);
    let r = reg(&a, "POST", &format!("/v2/{name}/blobs/uploads/"), Some(&u), None, &[]).await;
    let loc = r.header("location");
    let r = reg(&b, "PATCH", &loc, Some(&u), Some(blob[..20_000].to_vec()), &[("content-range", "0-19999")]).await;
    assert_eq!(r.status, 202);
    let r = reg(&a, "PATCH", &r.header("location"), Some(&u), Some(blob[20_000..].to_vec()), &[]).await;
    assert_eq!(r.status, 202);
    assert_eq!(r.header("range"), "0-49999");
    let r = reg(&b, "PUT", &format!("{}?digest={d}", r.header("location")), Some(&u), None, &[]).await;
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(reg(&a, "GET", &format!("/v2/{name}/blobs/{d}"), Some(&u), None, &[]).await.body, blob);
    let (md, _) = push_image(&b, &u, &name, "v1", json!({})).await;
    let r = reg(&a, "GET", &format!("/v2/{name}/manifests/v1"), Some(&u), None, &[]).await;
    assert_eq!(r.header("docker-content-digest"), md);
}
