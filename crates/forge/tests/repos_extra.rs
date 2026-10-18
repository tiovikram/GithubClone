//! Repos build-out area: environments, Pages, settings toggles, transfer, dispatch, rulesets
//! history/rule suites, ref activity, stats, users (emails, social accounts), issues (pins,
//! dependencies), gist comments, notifications, invitations, feeds, codes of conduct,
//! dependency graph — and the shared repository visibility rule used by lists and search.
mod common;

use std::time::Duration;

use base64::Engine;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

async fn put_file(s: &TestServer, u: &TestUser, full: &str, path: &str, content: &str) -> Value {
    let (st, v) = s.call(Some(u), "PUT", &format!("/repos/{full}/contents/{path}"), Some(json!({"message": format!("add {path}"), "content": b64(content)}))).await;
    assert_eq!(st, 201, "{v}");
    v
}

/// Creates an organization owned by `owner` (forge's self-service endpoint).
async fn new_org(s: &TestServer, owner: &TestUser) -> String {
    let org = unique("org");
    let res = s
        .client()
        .post(format!("{}/api/forge/orgs", s.base))
        .header("Authorization", format!("token {}", owner.token))
        .header("X-Forge-CSRF", "1")
        .json(&json!({ "login": org }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 201, "{}", res.text().await.unwrap());
    org
}

async fn org_with_member(s: &TestServer, owner: &TestUser, member: &TestUser) -> String {
    let org = new_org(s, owner).await;
    let (st, v) = s.call(Some(owner), "PUT", &format!("/orgs/{org}/memberships/{}", member.login), Some(json!({ "role": "member" }))).await;
    assert_eq!(st, 200, "{v}");
    org
}

fn names(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|r| r["name"].as_str().map(str::to_string)).collect()).unwrap_or_default()
}

async fn wait_for(mut f: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..50 {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Visibility (security regression): org base permission `none`

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base_permission_none_hides_private_org_repos_from_lists_and_search() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = org_with_member(&s, &owner, &member).await;
    let secret = unique("secret");
    let (st, v) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": secret, "private": true, "auto_init": true}))).await;
    assert_eq!(st, 201, "{v}");
    let (st, issue) = s.call(Some(&owner), "POST", &format!("/repos/{org}/{secret}/issues"), Some(json!({"title": format!("hidden {secret}")}))).await;
    assert_eq!(st, 201, "{issue}");
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({"default_repository_permission": "none"}))).await;
    assert_eq!(st, 200);

    let visible = |label: &'static str| {
        let (s, member, org, secret) = (&s, &member, org.clone(), secret.clone());
        async move {
            let (_, a) = s.call(Some(member), "GET", &format!("/orgs/{org}/repos?per_page=100"), None).await;
            let (_, b) = s.call(Some(member), "GET", "/user/repos?per_page=100", None).await;
            let (_, c) = s.call(Some(member), "GET", &format!("/users/{org}/repos?per_page=100"), None).await;
            let (_, d) = s.call(Some(member), "GET", &format!("/search/repositories?q={secret}"), None).await;
            let (_, e) = s.call(Some(member), "GET", &format!("/search/issues?q={secret}"), None).await;
            let (_, f) = s.call(Some(member), "GET", "/issues?filter=all&state=all", None).await;
            let (_, g) = s.call(Some(member), "GET", &format!("/orgs/{org}/issues?filter=all&state=all"), None).await;
            let (st, _) = s.call(Some(member), "GET", &format!("/repos/{org}/{secret}"), None).await;
            let lists = [
                names(&a).contains(&secret),
                names(&b).contains(&secret),
                names(&c).contains(&secret),
                d["total_count"].as_i64().unwrap_or(0) > 0,
                e["total_count"].as_i64().unwrap_or(0) > 0,
                f.to_string().contains(&secret),
                g.to_string().contains(&secret),
            ];
            let direct = st == 200;
            // Every list must agree with the direct permission check.
            assert!(lists.iter().all(|x| *x == direct), "{label}: direct={direct} lists={lists:?}");
            direct
        }
    };
    assert!(!visible("base none").await, "a member without grants must not see the private repo");

    // A team grant makes it visible everywhere.
    let (st, team) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({"name": unique("t")}))).await;
    assert_eq!(st, 201, "{team}");
    let slug = team["slug"].as_str().unwrap().to_string();
    let (st, v) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{slug}/memberships/{}", member.login), Some(json!({"role": "member"}))).await;
    assert_eq!(st, 200, "{v}");
    let (st, v) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{slug}/repos/{org}/{secret}"), Some(json!({"permission": "pull"}))).await;
    assert!(st == 204 || st == 200, "{st} {v}");
    assert!(visible("team grant").await);

    // Removing the grant hides it again; base `read` shows it.
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/teams/{slug}/repos/{org}/{secret}"), None).await;
    assert_eq!(st, 204);
    assert!(!visible("grant removed").await);
    s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({"default_repository_permission": "read"}))).await;
    assert!(visible("base read").await);
}

// ---------------------------------------------------------------------------------------------
// Environments

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn environments_policies_and_permissions() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let full = format!("{}/{repo}", u.login);
    let (_, me) = s.call(Some(&u), "GET", "/user", None).await;
    let (st, env) = s.call(Some(&u), "PUT", &format!("/repos/{full}/environments/prod"), Some(json!({
        "wait_timer": 10, "reviewers": [{"type": "User", "id": me["id"]}],
        "deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}))).await;
    assert_eq!(st, 200, "{env}");
    let types: Vec<&str> = env["protection_rules"].as_array().unwrap().iter().map(|r| r["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["wait_timer", "required_reviewers", "branch_policy"]);
    assert_eq!(env["deployment_branch_policy"]["custom_branch_policies"], true);

    // Invalid settings.
    let (st, _) = s.call(Some(&u), "PUT", &format!("/repos/{full}/environments/prod"), Some(json!({"wait_timer": 50000}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "PUT", &format!("/repos/{full}/environments/prod"), Some(json!({"deployment_branch_policy": {"protected_branches": true, "custom_branch_policies": true}}))).await;
    assert_eq!(st, 422);

    // Branch policies: create, duplicate -> 303, list.
    let (st, p) = s.call(Some(&u), "POST", &format!("/repos/{full}/environments/prod/deployment-branch-policies"), Some(json!({"name": "release/*"}))).await;
    assert_eq!(st, 200, "{p}");
    let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let dup = no_redirect.post(format!("{}/repos/{full}/environments/prod/deployment-branch-policies", s.api)).header("Authorization", format!("token {}", u.token)).json(&json!({"name": "release/*"}))
        .send().await.unwrap();
    assert_eq!(dup.status().as_u16(), 303);
    let (_, l) = s.call(Some(&u), "GET", &format!("/repos/{full}/environments/prod/deployment-branch-policies"), None).await;
    assert_eq!(l["total_count"], 1);

    // Others can't change environments of a repo they only read.
    let (st, _) = s.call(Some(&other), "PUT", &format!("/repos/{full}/environments/x"), Some(json!({}))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&other), "GET", &format!("/repos/{full}/environments/prod"), None).await;
    assert_eq!(st, 200);

    // Deployments create their environment implicitly.
    let (st, _) = s.call(Some(&u), "POST", &format!("/repos/{full}/deployments"), Some(json!({"ref": "main", "environment": "staging", "required_contexts": []}))).await;
    assert_eq!(st, 201);
    let (_, all) = s.call(Some(&u), "GET", &format!("/repos/{full}/environments"), None).await;
    assert_eq!(all["total_count"], 2, "{all}");

    let (st, _) = s.call(Some(&u), "DELETE", &format!("/repos/{full}/environments/prod"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("/repos/{full}/environments/prod"), None).await;
    assert_eq!(st, 404);
}

// ---------------------------------------------------------------------------------------------
// Pages

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pages_legacy_build_is_served() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let full = format!("{}/{repo}", u.login);
    put_file(&s, &u, &full, "index.html", "<h1>hello pages</h1>").await;
    put_file(&s, &u, &full, "about/index.html", "about").await;
    let (st, site) = s.call(Some(&u), "POST", &format!("/repos/{full}/pages"), Some(json!({"source": {"branch": "main", "path": "/"}}))).await;
    assert_eq!(st, 201, "{site}");
    assert_eq!(site["build_type"], "legacy");
    let (st, _) = s.call(Some(&u), "POST", &format!("/repos/{full}/pages"), Some(json!({"source": {"branch": "main"}}))).await;
    assert_eq!(st, 409);
    let ok = wait_for(async || {
        let (_, b) = s.call(Some(&u), "GET", &format!("/repos/{full}/pages/builds/latest"), None).await;
        b["status"] == "built"
    })
    .await;
    assert!(ok, "build finished");
    let page = s.client().get(format!("{}/pages/{full}/", s.base)).send().await.unwrap();
    assert_eq!(page.status().as_u16(), 200);
    assert!(page.headers()["content-security-policy"].to_str().unwrap().contains("sandbox"));
    assert_eq!(page.text().await.unwrap(), "<h1>hello pages</h1>");
    let dir = s.client().get(format!("{}/pages/{full}/about", s.base)).send().await.unwrap();
    assert_eq!(dir.status().as_u16(), 200, "redirect followed to about/");
    let missing = s.client().get(format!("{}/pages/{full}/nope.html", s.base)).send().await.unwrap();
    assert_eq!(missing.status().as_u16(), 404);

    // A push to the source branch rebuilds the site.
    put_file(&s, &u, &full, "new.html", "fresh").await;
    let ok = wait_for(async || s.client().get(format!("{}/pages/{full}/new.html", s.base)).send().await.unwrap().status().as_u16() == 200).await;
    assert!(ok, "rebuilt on push");
    let (_, builds) = s.call(Some(&u), "GET", &format!("/repos/{full}/pages/builds"), None).await;
    assert!(builds.as_array().unwrap().len() >= 2);

    let (st, _) = s.call(Some(&u), "DELETE", &format!("/repos/{full}/pages"), None).await;
    assert_eq!(st, 204);
    let gone = s.client().get(format!("{}/pages/{full}/", s.base)).send().await.unwrap();
    assert_eq!(gone.status().as_u16(), 404);
}

// ---------------------------------------------------------------------------------------------
// Settings, dispatch, transfer

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn toggles_dispatch_and_transfer() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let full = format!("{}/{repo}", u.login);
    let (st, _) = s.call(Some(&u), "GET", &format!("/repos/{full}/vulnerability-alerts"), None).await;
    assert_eq!(st, 404);
    assert_eq!(s.call(Some(&u), "PUT", &format!("/repos/{full}/vulnerability-alerts"), None).await.0, 204);
    assert_eq!(s.call(Some(&u), "GET", &format!("/repos/{full}/vulnerability-alerts"), None).await.0, 204);
    assert_eq!(s.call(Some(&other), "PUT", &format!("/repos/{full}/vulnerability-alerts"), None).await.0, 403);
    assert_eq!(s.call(Some(&u), "PUT", &format!("/repos/{full}/immutable-releases"), None).await.0, 204);
    let (_, im) = s.call(Some(&u), "GET", &format!("/repos/{full}/immutable-releases"), None).await;
    assert_eq!(im["enabled"], true);
    // Published releases' assets are locked; drafts are not.
    for (draft, want) in [(false, 422), (true, 201)] {
        let (st, rel) = s.call(Some(&u), "POST", &format!("/repos/{full}/releases"), Some(json!({"tag_name": unique("v"), "draft": draft}))).await;
        assert_eq!(st, 201, "{rel}");
        let up = s
            .client()
            .post(format!("{}/api/uploads/repos/{full}/releases/{}/assets?name=a.txt", s.base, rel["id"]))
            .header("Authorization", format!("token {}", u.token))
            .header("Content-Type", "text/plain")
            .body("x")
            .send()
            .await
            .unwrap();
        assert_eq!(up.status().as_u16(), want, "draft={draft}");
    }

    // repository_dispatch is delivered to hooks subscribed to it.
    let (st, h) = s.call(Some(&u), "POST", &format!("/repos/{full}/hooks"), Some(json!({"config": {"url": "http://127.0.0.1:9/hook"}, "events": ["repository_dispatch"]}))).await;
    assert_eq!(st, 201, "{h}");
    assert_eq!(s.call(Some(&u), "POST", &format!("/repos/{full}/dispatches"), Some(json!({"event_type": "deploy", "client_payload": {"x": 1}}))).await.0, 204);
    assert_eq!(s.call(Some(&u), "POST", &format!("/repos/{full}/dispatches"), Some(json!({}))).await.0, 422);
    assert_eq!(s.call(Some(&other), "POST", &format!("/repos/{full}/dispatches"), Some(json!({"event_type": "x"}))).await.0, 403);
    let (_, d) = s.call(Some(&u), "GET", &format!("/repos/{full}/hooks/{}/deliveries", h["id"]), None).await;
    assert!(d.as_array().unwrap().iter().any(|x| x["event"] == "repository_dispatch" && x["action"] == "deploy"), "{d}");

    // Transfer to an org the user belongs to; the old path keeps working.
    let org = new_org(&s, &u).await;
    assert_eq!(s.call(Some(&other), "POST", &format!("/repos/{full}/transfer"), Some(json!({"new_owner": org}))).await.0, 403);
    let (st, v) = s.call(Some(&u), "POST", &format!("/repos/{full}/transfer"), Some(json!({"new_owner": org}))).await;
    assert_eq!(st, 202, "{v}");
    assert_eq!(v["owner"]["login"], u.login.as_str(), "response keeps the original owner");
    let (st, r) = s.call(Some(&u), "GET", &format!("/repos/{full}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(r["full_name"], format!("{org}/{repo}"));
    let (st, _) = s.call(Some(&u), "GET", &format!("/repos/{org}/{repo}"), None).await;
    assert_eq!(st, 200);
}

// ---------------------------------------------------------------------------------------------
// Rulesets history, rule suites, ref activity, stats, protection contexts

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rule_suites_activity_and_stats() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let full = format!("{}/{repo}", u.login);
    let (st, rs) = s.call(Some(&u), "POST", &format!("/repos/{full}/rulesets"), Some(json!({
        "name": "no-creation", "target": "branch", "enforcement": "evaluate",
        "conditions": {"ref_name": {"include": ["~ALL"], "exclude": []}}, "rules": [{"type": "creation"}]}))).await;
    assert_eq!(st, 201, "{rs}");
    let (_, main) = s.call(Some(&u), "GET", &format!("/repos/{full}/git/ref/heads/main"), None).await;
    let sha = main["object"]["sha"].as_str().unwrap().to_string();
    let (st, _) = s.call(Some(&u), "POST", &format!("/repos/{full}/git/refs"), Some(json!({"ref": "refs/heads/feature", "sha": sha}))).await;
    assert_eq!(st, 201);
    let (st, suites) = s.call(Some(&u), "GET", &format!("/repos/{full}/rulesets/rule-suites"), None).await;
    assert_eq!(st, 200, "{suites}");
    let suite = suites.as_array().unwrap().iter().find(|x| x["ref"] == "refs/heads/feature").cloned().expect("suite for the new branch");
    assert_eq!(suite["evaluation_result"], "fail");
    let (_, one) = s.call(Some(&u), "GET", &format!("/repos/{full}/rulesets/rule-suites/{}", suite["id"]), None).await;
    assert_eq!(one["rule_evaluations"][0]["rule_type"], "creation");
    assert_eq!(one["rule_evaluations"][0]["result"], "fail");

    let (st, _) = s.call(Some(&u), "PUT", &format!("/repos/{full}/rulesets/{}", rs["id"]), Some(json!({"enforcement": "disabled"}))).await;
    assert_eq!(st, 200);
    let (_, hist) = s.call(Some(&u), "GET", &format!("/repos/{full}/rulesets/{}/history", rs["id"]), None).await;
    assert_eq!(hist.as_array().unwrap().len(), 2, "{hist}");

    let (_, act) = s.call(Some(&u), "GET", &format!("/repos/{full}/activity"), None).await;
    let kinds: Vec<&str> = act.as_array().unwrap().iter().map(|a| a["activity_type"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"branch_creation"), "{act}");
    let (_, act) = s.call(Some(&u), "GET", &format!("/repos/{full}/activity?activity_type=branch_creation&ref=feature"), None).await;
    assert_eq!(act.as_array().unwrap().len(), 1);

    let (st, pc) = s.call(Some(&u), "GET", &format!("/repos/{full}/stats/punch_card"), None).await;
    assert_eq!(st, 200);
    assert_eq!(pc.as_array().unwrap().len(), 168);
    let total: i64 = pc.as_array().unwrap().iter().map(|x| x[2].as_i64().unwrap()).sum();
    assert!(total >= 1);
    let (_, ca) = s.call(Some(&u), "GET", &format!("/repos/{full}/stats/commit_activity"), None).await;
    assert_eq!(ca.as_array().unwrap().len(), 52);
    let empty = s.create_repo(&u, false).await;
    assert_eq!(s.call(Some(&u), "GET", &format!("/repos/{}/{empty}/stats/code_frequency", u.login), None).await.0, 204);

    // Required status check contexts.
    let (st, _) = s.call(Some(&u), "PUT", &format!("/repos/{full}/branches/feature/protection"), Some(json!({
        "required_status_checks": {"strict": false, "contexts": ["a"]}, "enforce_admins": false, "required_pull_request_reviews": null, "restrictions": null}))).await;
    assert_eq!(st, 200);
    let (_, c) = s.call(Some(&u), "POST", &format!("/repos/{full}/branches/feature/protection/required_status_checks/contexts"), Some(json!(["b"]))).await;
    assert_eq!(c, json!(["a", "b"]));
    let (_, c) = s.call(Some(&u), "DELETE", &format!("/repos/{full}/branches/feature/protection/required_status_checks/contexts"), Some(json!({"contexts": ["a"]}))).await;
    assert_eq!(c, json!(["b"]));
    let (_, prot) = s.call(Some(&u), "GET", &format!("/repos/{full}/branches/feature/protection"), None).await;
    assert_eq!(prot["required_status_checks"]["contexts"], json!(["b"]));
    // Restrictions are for organization repositories.
    assert_eq!(s.call(Some(&u), "GET", &format!("/repos/{full}/branches/feature/protection/restrictions"), None).await.0, 404);
}

// ---------------------------------------------------------------------------------------------
// Users

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emails_social_accounts_and_hovercard() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let e = format!("{}@extra.example.com", unique("e"));
    let (st, v) = s.call(Some(&u), "POST", "/user/emails", Some(json!([e]))).await;
    assert_eq!(st, 201, "{v}");
    let (_, list) = s.call(Some(&u), "GET", "/user/emails", None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["primary"], true);
    // Taken by someone else / invalid / primary can't be deleted.
    assert_eq!(s.call(Some(&other), "POST", "/user/emails", Some(json!({"emails": [e]}))).await.0, 422);
    assert_eq!(s.call(Some(&u), "POST", "/user/emails", Some(json!("not-an-email"))).await.0, 422);
    let primary = list[0]["email"].as_str().unwrap().to_string();
    assert_eq!(s.call(Some(&u), "DELETE", "/user/emails", Some(json!([primary]))).await.0, 422);
    assert_eq!(s.call(Some(&u), "DELETE", "/user/emails", Some(json!([e]))).await.0, 204);

    let (st, added) = s.call(Some(&u), "POST", "/user/social_accounts", Some(json!({"account_urls": ["https://x.com/someone", "https://example.com/me"]}))).await;
    assert_eq!(st, 201);
    assert_eq!(added[0]["provider"], "twitter");
    assert_eq!(added[1]["provider"], "generic");
    let (_, pub_list) = s.call(None, "GET", &format!("/users/{}/social_accounts", u.login), None).await;
    assert_eq!(pub_list.as_array().unwrap().len(), 2);
    assert_eq!(s.call(Some(&u), "DELETE", "/user/social_accounts", Some(json!({"account_urls": ["https://x.com/someone"]}))).await.0, 204);

    let (_, me) = s.call(Some(&u), "GET", "/user", None).await;
    let (st, byid) = s.call(Some(&other), "GET", &format!("/user/{}", me["id"]), None).await;
    assert_eq!(st, 200);
    assert_eq!(byid["login"], u.login.as_str());
    let repo = s.create_repo(&u, false).await;
    let (_, r) = s.call(Some(&u), "GET", &format!("/repos/{}/{repo}", u.login), None).await;
    let (_, hc) = s.call(Some(&other), "GET", &format!("/users/{}/hovercard?subject_type=repository&subject_id={}", u.login, r["id"]), None).await;
    assert_eq!(hc["contexts"][0]["message"], "Owns this repository");
    assert_eq!(s.call(Some(&other), "GET", &format!("/users/{}/hovercard?subject_type=repository", u.login), None).await.0, 422);
}

// ---------------------------------------------------------------------------------------------
// Issues, gists

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_pins_dependencies_and_gist_comments() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let full = format!("{}/{repo}", u.login);
    let (_, a) = s.call(Some(&u), "POST", &format!("/repos/{full}/issues"), Some(json!({"title": "a"}))).await;
    let (_, b) = s.call(Some(&u), "POST", &format!("/repos/{full}/issues"), Some(json!({"title": "b"}))).await;
    let (an, bn) = (a["number"].as_i64().unwrap(), b["number"].as_i64().unwrap());
    let (st, _) = s.call(Some(&u), "POST", &format!("/repos/{full}/issues/{an}/dependencies/blocked_by"), Some(json!({"issue_id": b["id"]}))).await;
    assert_eq!(st, 201);
    // Duplicates and cycles are rejected.
    assert_eq!(s.call(Some(&u), "POST", &format!("/repos/{full}/issues/{an}/dependencies/blocked_by"), Some(json!({"issue_id": b["id"]}))).await.0, 422);
    assert_eq!(s.call(Some(&u), "POST", &format!("/repos/{full}/issues/{bn}/dependencies/blocked_by"), Some(json!({"issue_id": a["id"]}))).await.0, 422);
    assert_eq!(s.call(Some(&other), "POST", &format!("/repos/{full}/issues/{bn}/dependencies/blocked_by"), Some(json!({"issue_id": a["id"]}))).await.0, 403);
    let (_, blocking) = s.call(Some(&u), "GET", &format!("/repos/{full}/issues/{bn}/dependencies/blocking"), None).await;
    assert_eq!(blocking[0]["number"], an);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("/repos/{full}/issues/{an}/dependencies/blocked_by/{}", b["id"]), None).await;
    assert_eq!(st, 200);

    let (_, c) = s.call(Some(&u), "POST", &format!("/repos/{full}/issues/{an}/comments"), Some(json!({"body": "important"}))).await;
    let (st, pinned) = s.call(Some(&u), "PUT", &format!("/repos/{full}/issues/comments/{}/pin", c["id"]), None).await;
    assert_eq!(st, 200, "{pinned}");
    assert_eq!(pinned["pin"]["pinned_by"]["login"], u.login.as_str());
    assert_eq!(s.call(Some(&other), "PUT", &format!("/repos/{full}/issues/comments/{}/pin", c["id"]), None).await.0, 403);
    assert_eq!(s.call(Some(&u), "DELETE", &format!("/repos/{full}/issues/comments/{}/pin", c["id"]), None).await.0, 204);
    assert_eq!(s.call(Some(&u), "DELETE", &format!("/repos/{full}/issues/comments/{}/pin", c["id"]), None).await.0, 404);

    let (_, g) = s.call(Some(&u), "POST", "/gists", Some(json!({"public": true, "files": {"a.txt": {"content": "x"}}}))).await;
    let gid = g["id"].as_str().unwrap();
    let (st, gc) = s.call(Some(&other), "POST", &format!("/gists/{gid}/comments"), Some(json!({"body": "nice"}))).await;
    assert_eq!(st, 201, "{gc}");
    assert_eq!(gc["author_association"], "NONE");
    // Only the author edits; the gist owner may delete.
    assert_eq!(s.call(Some(&u), "PATCH", &format!("/gists/{gid}/comments/{}", gc["id"]), Some(json!({"body": "x"}))).await.0, 404);
    assert_eq!(s.call(Some(&other), "PATCH", &format!("/gists/{gid}/comments/{}", gc["id"]), Some(json!({"body": "nicer"}))).await.0, 200);
    let (_, l) = s.call(None, "GET", &format!("/gists/{gid}/comments"), None).await;
    assert_eq!(l[0]["body"], "nicer");
    assert_eq!(s.call(Some(&u), "DELETE", &format!("/gists/{gid}/comments/{}", gc["id"]), None).await.0, 204);
}

// ---------------------------------------------------------------------------------------------
// Notifications & invitations

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notifications_for_participants_and_watchers() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let collab = s.user().await;
    let stranger = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let full = format!("{}/{repo}", owner.login);
    assert_eq!(s.call(Some(&owner), "PUT", &format!("/repos/{full}/collaborators/{}", collab.login), Some(json!({"permission": "push"}))).await.0, 204);
    let (_, issue) = s.call(Some(&owner), "POST", &format!("/repos/{full}/issues"), Some(json!({"title": "hello"}))).await;
    let n = issue["number"].as_i64().unwrap();
    // The owner's own activity doesn't notify them.
    let (_, mine) = s.call(Some(&owner), "GET", "/notifications", None).await;
    assert_eq!(mine.as_array().unwrap().len(), 0, "{mine}");
    let (st, _) = s.call(Some(&collab), "POST", &format!("/repos/{full}/issues/{n}/comments"), Some(json!({"body": format!("cc @{}", stranger.login)}))).await;
    assert_eq!(st, 201);
    let (_, list) = s.call(Some(&owner), "GET", "/notifications", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let t = &list[0];
    assert_eq!(t["subject"]["type"], "Issue");
    assert_eq!(t["reason"], "author");
    assert!(t["subject"]["latest_comment_url"].as_str().unwrap().contains("/issues/comments/"));
    // A mentioned user who can't read the repo... can for a public repo: reason mention.
    let (_, st_list) = s.call(Some(&stranger), "GET", "/notifications", None).await;
    assert_eq!(st_list[0]["reason"], "mention", "{st_list}");
    // The commenter doesn't get their own comment.
    let (_, c_list) = s.call(Some(&collab), "GET", "/notifications", None).await;
    assert_eq!(c_list.as_array().unwrap().len(), 0);

    let tid = t["id"].as_str().unwrap();
    assert_eq!(s.call(Some(&stranger), "GET", &format!("/notifications/threads/{tid}"), None).await.0, 404);
    assert_eq!(s.call(Some(&owner), "PATCH", &format!("/notifications/threads/{tid}"), None).await.0, 205);
    let (_, unread) = s.call(Some(&owner), "GET", "/notifications", None).await;
    assert_eq!(unread.as_array().unwrap().len(), 0);
    let (_, all) = s.call(Some(&owner), "GET", "/notifications?all=true", None).await;
    assert_eq!(all[0]["unread"], false);
    // Ignoring the thread stops further notifications.
    let (st, sub) = s.call(Some(&owner), "PUT", &format!("/notifications/threads/{tid}/subscription"), Some(json!({"ignored": true}))).await;
    assert_eq!(st, 200);
    assert_eq!(sub["ignored"], true);
    s.call(Some(&collab), "POST", &format!("/repos/{full}/issues/{n}/comments"), Some(json!({"body": "again"}))).await;
    let (_, after) = s.call(Some(&owner), "GET", "/notifications", None).await;
    assert_eq!(after.as_array().unwrap().len(), 0, "{after}");
    assert_eq!(s.call(Some(&owner), "DELETE", &format!("/notifications/threads/{tid}"), None).await.0, 204);
    let (_, done) = s.call(Some(&owner), "GET", "/notifications?all=true", None).await;
    assert_eq!(done.as_array().unwrap().len(), 0);

    // Private repo: a mentioned user without access gets nothing.
    let (_, pr) = s.call(Some(&owner), "POST", "/user/repos", Some(json!({"name": unique("p"), "private": true, "auto_init": true}))).await;
    let pfull = pr["full_name"].as_str().unwrap().to_string();
    s.call(Some(&owner), "PUT", &format!("/repos/{pfull}/collaborators/{}", collab.login), Some(json!({"permission": "push"}))).await;
    let (_, pi) = s.call(Some(&owner), "POST", &format!("/repos/{pfull}/issues"), Some(json!({"title": "p"}))).await;
    s.call(Some(&collab), "POST", &format!("/repos/{pfull}/issues/{}/comments", pi["number"]), Some(json!({"body": format!("@{}", stranger.login)}))).await;
    let (_, sl) = s.call(Some(&stranger), "GET", "/notifications?all=true", None).await;
    assert!(!sl.to_string().contains(&pfull), "{sl}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitations_accept_and_decline() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let invitee = s.user().await;
    let repo = s.create_repo(&owner, true).await;
    let full = format!("{}/{repo}", owner.login);
    // GHES adds collaborators directly; invitations are created by the service layer
    // (github.com behaviour with FORGE_REPO_INVITATIONS=1).
    let ctx = forge::db::load_repo(&s.state.db, None, &owner.login, &repo).await.map_err(|e| e.message).unwrap();
    let ctx = forge::db::RepoCtx { perm: forge::db::Perm::Admin, ..ctx };
    let inviter = forge::db::user_by_login(&s.state.db, &owner.login).await.map_err(|e| e.message).unwrap();
    let target = forge::db::user_by_login(&s.state.db, &invitee.login).await.map_err(|e| e.message).unwrap();
    let inv = forge::api::invitations::create_invitation(&s.state, &ctx, &inviter, &target, "write").await.map_err(|e| e.message).unwrap();
    let (_, list) = s.call(Some(&owner), "GET", &format!("/repos/{full}/invitations"), None).await;
    assert_eq!(list[0]["permissions"], "write");
    let (st, upd) = s.call(Some(&owner), "PATCH", &format!("/repos/{full}/invitations/{}", inv.id), Some(json!({"permissions": "read"}))).await;
    assert_eq!(st, 200);
    assert_eq!(upd["permissions"], "read");
    let (_, mine) = s.call(Some(&invitee), "GET", "/user/repository_invitations", None).await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    let (_, n) = s.call(Some(&invitee), "GET", "/notifications", None).await;
    assert_eq!(n[0]["reason"], "invitation");
    assert_eq!(s.call(Some(&owner), "PATCH", &format!("/user/repository_invitations/{}", inv.id), None).await.0, 404, "only the invitee accepts");
    assert_eq!(s.call(Some(&invitee), "PATCH", &format!("/user/repository_invitations/{}", inv.id), None).await.0, 204);
    let (_, perm) = s.call(Some(&owner), "GET", &format!("/repos/{full}/collaborators/{}/permission", invitee.login), None).await;
    assert_eq!(perm["permission"], "read");
    let inv2 = forge::api::invitations::create_invitation(&s.state, &ctx, &inviter, &target, "admin").await.map_err(|e| e.message).unwrap();
    assert_eq!(s.call(Some(&invitee), "DELETE", &format!("/user/repository_invitations/{}", inv2.id), None).await.0, 204);
    let (_, list) = s.call(Some(&owner), "GET", &format!("/repos/{full}/invitations"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------------------------
// Feeds, codes of conduct, dependency graph

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn feeds_codes_of_conduct_and_dependency_graph() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let full = format!("{}/{repo}", u.login);
    let (_, f) = s.call(Some(&u), "GET", "/feeds", None).await;
    let private = f["current_user_url"].as_str().unwrap().to_string();
    let atom = s.client().get(&private).send().await.unwrap();
    assert_eq!(atom.status().as_u16(), 200);
    assert!(atom.text().await.unwrap().contains("<feed"));
    let bad = s.client().get(private.replace("token=", "token=x")).send().await.unwrap();
    assert_eq!(bad.status().as_u16(), 404);

    let (_, cocs) = s.call(None, "GET", "/codes_of_conduct", None).await;
    assert_eq!(cocs.as_array().unwrap().len(), 2);
    let (_, cc) = s.call(None, "GET", "/codes_of_conduct/contributor_covenant", None).await;
    assert!(cc["body"].as_str().unwrap().contains("Our Pledge"));
    assert_eq!(s.call(None, "GET", "/codes_of_conduct/nope", None).await.0, 404);

    put_file(&s, &u, &full, "package.json", r#"{"dependencies":{"left-pad":"^1.3.0"}}"#).await;
    put_file(&s, &u, &full, "rust/Cargo.toml", "[dependencies]\nserde = \"1.0\"\n").await;
    let (st, sbom) = s.call(Some(&u), "GET", &format!("/repos/{full}/dependency-graph/sbom"), None).await;
    assert_eq!(st, 200);
    let refs: Vec<String> = sbom["sbom"]["packages"].as_array().unwrap().iter().filter_map(|p| p["externalRefs"][0]["referenceLocator"].as_str().map(str::to_string)).collect();
    assert!(refs.contains(&"pkg:npm/left-pad@1.3.0".to_string()), "{refs:?}");
    assert!(refs.contains(&"pkg:cargo/serde@1.0".to_string()), "{refs:?}");
    assert_eq!(sbom["sbom"]["spdxVersion"], "SPDX-2.3");

    let (_, main) = s.call(Some(&u), "GET", &format!("/repos/{full}/git/ref/heads/main"), None).await;
    let base = main["object"]["sha"].as_str().unwrap().to_string();
    put_file(&s, &u, &full, "requirements.txt", "requests==2.31.0\n").await;
    let (st, diff) = s.call(Some(&u), "GET", &format!("/repos/{full}/dependency-graph/compare/{base}...main"), None).await;
    assert_eq!(st, 200, "{diff}");
    assert_eq!(diff.as_array().unwrap().len(), 1, "{diff}");
    assert_eq!(diff[0]["change_type"], "added");
    assert_eq!(diff[0]["name"], "requests");

    let (st, snap) = s.call(Some(&u), "POST", &format!("/repos/{full}/dependency-graph/snapshots"), Some(json!({
        "version": 0, "sha": base, "ref": "refs/heads/main", "job": {"id": "1", "correlator": "c"},
        "detector": {"name": "d", "version": "1", "url": "https://example.com"}, "scanned": "2026-01-01T00:00:00Z",
        "manifests": {"m": {"name": "m", "resolved": {"x": {"package_url": "pkg:golang/example.com/x@v1.0.0"}}}}}))).await;
    assert_eq!(st, 201, "{snap}");
    let (_, sbom) = s.call(Some(&u), "GET", &format!("/repos/{full}/dependency-graph/sbom"), None).await;
    assert!(sbom.to_string().contains("pkg:golang/example.com/x@v1.0.0"));
    assert_eq!(s.call(Some(&u), "POST", &format!("/repos/{full}/dependency-graph/snapshots"), Some(json!({"version": 0}))).await.0, 422);
}
