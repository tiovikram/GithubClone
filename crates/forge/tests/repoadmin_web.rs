//! Server pieces the repoadmin web UI needs: repository traffic (views beacon, clones, popular
//! content and referrers), the community profile, custom repository roles for collaborators and
//! invitations, and the forge-only LFS / enterprise helpers.
mod common;

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn forge_call(s: &TestServer, u: Option<&TestUser>, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = s.client().request(method.parse().unwrap(), format!("{}/api/forge{path}", s.base)).header("X-Forge-CSRF", "1");
    if let Some(u) = u {
        req = req.header("Authorization", format!("token {}", u.token));
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let st = res.status().as_u16();
    let text = res.text().await.unwrap();
    (st, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn traffic_views_clones_paths_referrers() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let reader = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let full = format!("{}/{repo}", owner.login);

    // Views reported by the web UI (signed in or anonymous); paths must be inside the repository.
    for (u, path, refr) in [
        (Some(&owner), format!("/{full}"), "https://www.google.com/search?q=x"),
        (Some(&owner), format!("/{full}/issues"), ""),
        (None, format!("/{full}"), "https://news.example.org/item"),
    ] {
        let (st, v) = forge_call(&s, u, "POST", &format!("/repos/{full}/traffic/view"), Some(json!({"path": path, "title": "T", "referrer": refr}))).await;
        assert_eq!(st, 204, "{v}");
    }
    let (st, _) = forge_call(&s, Some(&owner), "POST", &format!("/repos/{full}/traffic/view"), Some(json!({"path": "/someone/else"}))).await;
    assert_eq!(st, 422, "paths outside the repository are rejected");
    // A self-referral (this site) is not a referrer.
    forge_call(&s, Some(&owner), "POST", &format!("/repos/{full}/traffic/view"), Some(json!({"path": format!("/{full}"), "referrer": format!("{}/x", s.base)}))).await;

    // A clone over smart HTTP counts.
    let dir = s.work_dir("c");
    let (ok, out) = s.git(&dir, &["clone", "-q", &s.clone_url(&owner, &repo), "w"]);
    assert!(ok, "{out}");

    let (st, v) = s.call(Some(&owner), "GET", &format!("/repos/{full}/traffic/views"), None).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["count"], 4, "{v}");
    assert_eq!(v["uniques"], 2, "{v}");
    let days = v["views"].as_array().unwrap();
    assert!(days.len() >= 14, "every day of the window: {v}");
    assert_eq!(days.iter().map(|d| d["count"].as_i64().unwrap()).sum::<i64>(), 4);
    let (_, w) = s.call(Some(&owner), "GET", &format!("/repos/{full}/traffic/views?per=week"), None).await;
    assert_eq!(w["count"], 4);
    assert!(w["views"].as_array().unwrap().len() <= 4, "{w}");
    let (st, _) = s.call(Some(&owner), "GET", &format!("/repos/{full}/traffic/views?per=year"), None).await;
    assert_eq!(st, 422);

    let (_, c) = s.call(Some(&owner), "GET", &format!("/repos/{full}/traffic/clones"), None).await;
    assert!(c["count"].as_i64().unwrap() >= 1, "{c}");
    assert_eq!(c["uniques"], 1, "{c}");
    assert!(c["clones"].is_array());

    let (_, refs) = s.call(Some(&owner), "GET", &format!("/repos/{full}/traffic/popular/referrers"), None).await;
    let names: Vec<&str> = refs.as_array().unwrap().iter().map(|r| r["referrer"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 2, "{refs}");
    assert!(names.contains(&"google.com") && names.contains(&"news.example.org"), "{refs}");
    let (_, paths) = s.call(Some(&owner), "GET", &format!("/repos/{full}/traffic/popular/paths"), None).await;
    assert_eq!(paths[0]["path"], format!("/{full}"), "{paths}");
    assert_eq!(paths[0]["count"], 3);
    assert_eq!(paths[0]["title"], "T");

    // Traffic needs push access.
    let (st, _) = s.call(Some(&reader), "GET", &format!("/repos/{full}/traffic/views"), None).await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn community_profile_and_helpers() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let full = format!("{}/{repo}", owner.login);
    let (st, p) = s.call(None, "GET", &format!("/repos/{full}/community/profile"), None).await;
    assert_eq!(st, 200, "{p}");
    assert!(p["files"]["readme"]["html_url"].as_str().unwrap().ends_with("/README.md"), "{p}");
    assert!(p["files"]["license"].is_null() && p["files"]["contributing"].is_null());
    let before = p["health_percentage"].as_i64().unwrap();

    for (path, text) in [
        ("CONTRIBUTING.md", "How to contribute"),
        (".github/CODE_OF_CONDUCT.md", "Be nice"),
        ("LICENSE", "MIT License\n\nPermission is hereby granted, free of charge, to any person obtaining a copy"),
        (".github/ISSUE_TEMPLATE/bug.md", "Bug"),
        (".github/pull_request_template.md", "PR"),
    ] {
        use base64::Engine;
        let (st, v) = s.call(Some(&owner), "PUT", &format!("/repos/{full}/contents/{path}"),
            Some(json!({"message": format!("add {path}"), "content": base64::engine::general_purpose::STANDARD.encode(text)}))).await;
        assert_eq!(st, 201, "{v}");
    }
    s.call(Some(&owner), "PATCH", &format!("/repos/{full}"), Some(json!({"description": "A repo"}))).await;
    let (_, p) = s.call(None, "GET", &format!("/repos/{full}/community/profile"), None).await;
    assert_eq!(p["health_percentage"], 100, "{p}");
    assert!(p["health_percentage"].as_i64().unwrap() > before);
    assert_eq!(p["description"], "A repo");
    assert!(p["files"]["code_of_conduct"]["html_url"].as_str().unwrap().contains(".github/CODE_OF_CONDUCT.md"), "{p}");
    assert!(p["files"]["code_of_conduct_file"]["url"].is_string());
    assert!(p["files"]["contributing"]["url"].as_str().unwrap().contains("CONTRIBUTING.md"));
    assert!(p["files"]["issue_template"]["html_url"].as_str().unwrap().contains("ISSUE_TEMPLATE/bug.md"), "{p}");
    assert!(p["files"]["pull_request_template"].is_object());
    assert!(p["files"]["license"]["html_url"].as_str().unwrap().ends_with("/LICENSE"), "{p}");

    // LFS flag (PUT/DELETE /lfs have no GET in the REST API).
    let (st, v) = forge_call(&s, Some(&owner), "GET", &format!("/repos/{full}/lfs"), None).await;
    assert_eq!((st, v["enabled"].clone()), (200, json!(true)), "{v}");
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/repos/{full}/lfs"), None).await;
    assert_eq!(st, 204);
    let (_, v) = forge_call(&s, Some(&owner), "GET", &format!("/repos/{full}/lfs"), None).await;
    assert_eq!(v["enabled"], false);
    let other = s.user().await;
    let (st, _) = forge_call(&s, Some(&other), "GET", &format!("/repos/{full}/lfs"), None).await;
    assert_eq!(st, 403);

    let (st, v) = forge_call(&s, None, "GET", "/enterprise", None).await;
    assert_eq!(st, 200);
    assert!(v["slug"].as_str().is_some_and(|x| !x.is_empty()), "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn collaborators_and_invitations_with_custom_roles() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let alice = s.user().await;
    let bob = s.user().await;
    let org = unique("org");
    let (st, v) = forge_call(&s, Some(&owner), "POST", "/orgs", Some(json!({"login": org}))).await;
    assert_eq!(st, 201, "{v}");
    s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({"default_repository_permission": "none"}))).await;
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": "app", "private": true, "auto_init": true}))).await;
    assert_eq!(st, 201);
    let (st, role) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/custom-repository-roles"),
        Some(json!({"name": "Labeler", "base_role": "triage", "permissions": ["add_label"]}))).await;
    assert_eq!(st, 201, "{role}");
    let full = format!("{org}/app");

    // Direct add with a custom role (case-insensitive), reported by name.
    let (st, v) = s.call(Some(&owner), "PUT", &format!("/repos/{full}/collaborators/{}", alice.login), Some(json!({"permission": "labeler"}))).await;
    assert!(st == 201 || st == 204, "{st} {v}");
    if st == 201 {
        // instance runs with repository invitations on: accept it
        let (st, _) = s.call(Some(&alice), "PATCH", &format!("/user/repository_invitations/{}", v["id"]), None).await;
        assert_eq!(st, 204);
    }
    let (_, perm) = s.call(Some(&owner), "GET", &format!("/repos/{full}/collaborators/{}/permission", alice.login), None).await;
    assert_eq!(perm["role_name"], "Labeler", "{perm}");
    assert_eq!(perm["permission"], "read", "{perm}");
    assert_eq!(perm["user"]["permissions"]["triage"], true);
    assert_eq!(perm["user"]["permissions"]["push"], false);
    let (_, list) = s.call(Some(&owner), "GET", &format!("/repos/{full}/collaborators?affiliation=direct"), None).await;
    let a = list.as_array().unwrap().iter().find(|c| c["login"] == alice.login.as_str()).cloned().unwrap();
    assert_eq!(a["role_name"], "Labeler", "{a}");
    // Base roles still work; unknown names are rejected.
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/repos/{full}/collaborators/{}", alice.login), Some(json!({"permission": "maintain"}))).await;
    assert_eq!(st, 204);
    let (_, perm) = s.call(Some(&owner), "GET", &format!("/repos/{full}/collaborators/{}/permission", alice.login), None).await;
    assert_eq!(perm["role_name"], "maintain");
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/repos/{full}/collaborators/{}", alice.login), Some(json!({"permission": "nope"}))).await;
    assert_eq!(st, 422);

    // An invitation with a custom role: reported with its base role, granted on acceptance.
    let o = forge::db::user_by_login(&s.state.db, &owner.login).await.unwrap();
    let ctx = forge::db::load_repo(&s.state.db, Some(&o), &org, "app").await.unwrap();
    let b = forge::db::user_by_login(&s.state.db, &bob.login).await.unwrap();
    let inv = forge::api::invitations::create_invitation(&s.state, &ctx, &o, &b, "Labeler").await.map_err(|e| e.message).unwrap();
    assert!(forge::api::invitations::create_invitation(&s.state, &ctx, &o, &b, "Nope").await.is_err());
    let (_, mine) = s.call(Some(&bob), "GET", "/user/repository_invitations", None).await;
    let i = mine.as_array().unwrap().iter().find(|i| i["id"] == inv.id).cloned().unwrap();
    assert_eq!(i["permissions"], "triage", "{i}");
    let (st, _) = s.call(Some(&bob), "PATCH", &format!("/user/repository_invitations/{}", inv.id), None).await;
    assert_eq!(st, 204);
    let (_, perm) = s.call(Some(&owner), "GET", &format!("/repos/{full}/collaborators/{}/permission", bob.login), None).await;
    assert_eq!(perm["role_name"], "Labeler", "{perm}");
}

async fn site_admin(s: &TestServer) -> TestUser {
    let login = unique("adm");
    let u = forge::forge_api::create_user(&s.state, &login, Some(&format!("{login}@example.com")), "password123", true).await.map_err(|e| e.message).unwrap();
    let (_, token) = forge::forge_api::create_token_for(&s.state, u.id, "test").await.map_err(|e| e.message).unwrap();
    TestUser { login, token }
}

/// Enterprise rulesets: site admins manage them; they apply to the organization repositories
/// their `organization_name` condition selects, and are enforced like org rulesets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enterprise_rulesets() {
    let s = TestServer::start().await;
    let admin = site_admin(&s).await;
    let owner = s.user().await;
    let org = unique("eorg");
    let (st, _) = forge_call(&s, Some(&owner), "POST", "/orgs", Some(json!({"login": org}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": "app", "auto_init": true}))).await;
    assert_eq!(st, 201);
    let (_, slug) = forge_call(&s, None, "GET", "/enterprise", None).await;
    let e = slug["slug"].as_str().unwrap().to_string();
    let body = json!({
        "name": unique("ent"), "target": "branch", "enforcement": "active",
        "conditions": {
            "ref_name": {"include": ["refs/heads/locked/**"], "exclude": []},
            "repository_name": {"include": ["~ALL"], "exclude": []},
            "organization_name": {"include": [org.clone()], "exclude": []},
        },
        "rules": [{"type": "creation"}],
        "bypass_actors": [],
    });
    let (st, _) = s.call(Some(&owner), "POST", &format!("/enterprises/{e}/rulesets"), Some(body.clone())).await;
    assert_eq!(st, 403, "only site admins");
    let (st, rs) = s.call(Some(&admin), "POST", &format!("/enterprises/{e}/rulesets"), Some(body)).await;
    assert_eq!(st, 201, "{rs}");
    assert_eq!(rs["source_type"], "Enterprise");
    assert_eq!(rs["source"], e.as_str());
    let id = rs["id"].as_i64().unwrap();
    let (st, list) = s.call(Some(&admin), "GET", &format!("/enterprises/{e}/rulesets"), None).await;
    assert_eq!(st, 200);
    assert!(list.as_array().unwrap().iter().any(|r| r["id"] == id));

    // Inherited by the organization and its repositories...
    let (_, ol) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/rulesets"), None).await;
    assert!(ol.as_array().unwrap().iter().any(|r| r["id"] == id && r["source_type"] == "Enterprise"), "{ol}");
    let full = format!("{org}/app");
    let (_, rl) = s.call(Some(&owner), "GET", &format!("/repos/{full}/rulesets"), None).await;
    assert!(rl.as_array().unwrap().iter().any(|r| r["id"] == id), "{rl}");
    let (_, br) = s.call(Some(&owner), "GET", &format!("/repos/{full}/rules/branches/locked/x"), None).await;
    assert_eq!(br[0]["ruleset_source_type"], "Enterprise", "{br}");
    // ...but cannot be changed there.
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/rulesets/{id}"), None).await;
    assert_eq!(st, 404);

    // Enforced on ref creation through the API.
    let (_, main) = s.call(Some(&owner), "GET", &format!("/repos/{full}/branches/main"), None).await;
    let sha = main["commit"]["sha"].as_str().unwrap().to_string();
    let refs = format!("/repos/{full}/{}/refs", "git");
    let (st, v) = s.call(Some(&owner), "POST", &refs, Some(json!({"ref": "refs/heads/locked/a", "sha": sha}))).await;
    assert!(st == 422 || st == 409 || st == 403, "{st} {v}");
    let (st, _) = s.call(Some(&owner), "POST", &refs, Some(json!({"ref": "refs/heads/free", "sha": sha}))).await;
    assert_eq!(st, 201);

    // Excluding the organization lifts it; history records both versions.
    let (st, upd) = s.call(Some(&admin), "PUT", &format!("/enterprises/{e}/rulesets/{id}"),
        Some(json!({"conditions": {"ref_name": {"include": ["refs/heads/locked/**"], "exclude": []}, "repository_name": {"include": ["~ALL"], "exclude": []},
                    "organization_name": {"include": ["~ALL"], "exclude": [org.clone()]}}}))).await;
    assert_eq!(st, 200, "{upd}");
    let (_, br) = s.call(Some(&owner), "GET", &format!("/repos/{full}/rules/branches/locked/x"), None).await;
    assert_eq!(br.as_array().unwrap().len(), 0, "{br}");
    let (st, h) = s.call(Some(&admin), "GET", &format!("/enterprises/{e}/rulesets/{id}/history"), None).await;
    assert_eq!(st, 200, "{h}");
    assert_eq!(h.as_array().unwrap().len(), 2, "{h}");
    let v = h[0]["version_id"].as_i64().unwrap();
    let (st, one) = s.call(Some(&admin), "GET", &format!("/enterprises/{e}/rulesets/{id}/history/{v}"), None).await;
    assert_eq!(st, 200, "{one}");
    let (st, _) = s.call(Some(&admin), "DELETE", &format!("/enterprises/{e}/rulesets/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&admin), "GET", &format!("/enterprises/{e}/rulesets/{id}"), None).await;
    assert_eq!(st, 404);
}

/// Pull request options of PATCH /repos: stored, reported, validated, and used as merge defaults.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merge_commit_message_options() {
    use base64::Engine;
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let full = format!("{}/{repo}", u.login);
    let (_, r) = s.call(Some(&u), "GET", &format!("/repos/{full}"), None).await;
    assert_eq!((r["merge_commit_title"].as_str(), r["merge_commit_message"].as_str()), (Some("MERGE_MESSAGE"), Some("PR_TITLE")));
    assert_eq!(r["allow_update_branch"], false);
    let (st, r) = s.call(Some(&u), "PATCH", &format!("/repos/{full}"),
        Some(json!({"merge_commit_title": "PR_TITLE", "merge_commit_message": "PR_BODY", "allow_update_branch": true, "web_commit_signoff_required": true,
                    "squash_merge_commit_title": "PR_TITLE", "squash_merge_commit_message": "BLANK"}))).await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["merge_commit_title"], "PR_TITLE");
    assert_eq!(r["merge_commit_message"], "PR_BODY");
    assert_eq!(r["squash_merge_commit_message"], "BLANK");
    assert_eq!(r["use_squash_pr_title_as_default"], true);
    assert_eq!(r["allow_update_branch"], true);
    assert_eq!(r["web_commit_signoff_required"], true);
    let (_, again) = s.call(Some(&u), "GET", &format!("/repos/{full}"), None).await;
    assert_eq!(again["merge_commit_message"], "PR_BODY", "persisted");
    let (st, _) = s.call(Some(&u), "PATCH", &format!("/repos/{full}"), Some(json!({"merge_commit_title": "MERGE_MESSAGE", "merge_commit_message": "PR_BODY"}))).await;
    assert_eq!(st, 422, "MERGE_MESSAGE only goes with PR_TITLE");
    let (st, _) = s.call(Some(&u), "PATCH", &format!("/repos/{full}"), Some(json!({"squash_merge_commit_title": "NOPE"}))).await;
    assert_eq!(st, 422);

    // A merge without an explicit message uses the defaults.
    let (_, main) = s.call(Some(&u), "GET", &format!("/repos/{full}/branches/main"), None).await;
    let refs = format!("/repos/{full}/{}/refs", "git");
    let (st, _) = s.call(Some(&u), "POST", &refs, Some(json!({"ref": "refs/heads/topic", "sha": main["commit"]["sha"]}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "PUT", &format!("/repos/{full}/contents/x.txt"),
        Some(json!({"message": "add x", "branch": "topic", "content": base64::engine::general_purpose::STANDARD.encode("x")}))).await;
    assert_eq!(st, 201);
    let (st, pr) = s.call(Some(&u), "POST", &format!("/repos/{full}/pulls"), Some(json!({"title": "Add x", "body": "Adds the x file.", "head": "topic", "base": "main"}))).await;
    assert_eq!(st, 201, "{pr}");
    let (st, m) = s.call(Some(&u), "PUT", &format!("/repos/{full}/pulls/{}/merge", pr["number"]), Some(json!({"merge_method": "merge"}))).await;
    assert_eq!(st, 200, "{m}");
    let (_, c) = s.call(Some(&u), "GET", &format!("/repos/{full}/commits/{}", m["sha"].as_str().unwrap()), None).await;
    assert_eq!(c["commit"]["message"], format!("Add x (#{})\n\nAdds the x file.", pr["number"]), "{c}");
}
