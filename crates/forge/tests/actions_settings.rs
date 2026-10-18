//! Actions settings: decryptable secrets and the job-injection services, selected repositories
//! of org secrets/variables, permission policies and their enforcement, runner groups.
mod common;

use common::{unique, TestServer, TestUser};
use forge::api::actions_settings::{self, crypto};
use serde_json::{json, Value};

async fn site_admin(s: &TestServer) -> TestUser {
    let login = unique("adm");
    let u = forge::forge_api::create_user(&s.state, &login, Some(&format!("{login}@example.com")), "password123", true)
        .await
        .map_err(|e| e.message)
        .unwrap();
    let (_, token) = forge::forge_api::create_token_for(&s.state, u.id, "test").await.map_err(|e| e.message).unwrap();
    TestUser { login, token }
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
    assert_eq!(res.status().as_u16(), 201);
    login
}

async fn org_repo(s: &TestServer, u: &TestUser, org: &str, private: bool) -> (String, i64) {
    let name = unique("r");
    let (st, r) = s.call(Some(u), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": name, "private": private, "auto_init": true}))).await;
    assert_eq!(st, 201, "{r}");
    (format!("/repos/{org}/{name}"), r["id"].as_i64().unwrap())
}

async fn repo_id(s: &TestServer, u: &TestUser, rp: &str) -> i64 {
    s.call(Some(u), "GET", rp, None).await.1["id"].as_i64().unwrap()
}

/// Seals `value` for the key at `{base}/secrets/public-key` and PUTs it as `name`.
async fn put_secret(s: &TestServer, u: &TestUser, base: &str, name: &str, value: &str, extra: Value) -> u16 {
    let (st, pk) = s.call(Some(u), "GET", &format!("{base}/secrets/public-key"), None).await;
    assert_eq!(st, 200);
    let mut body = json!({
        "encrypted_value": crypto::seal_for(pk["key"].as_str().unwrap(), value).unwrap(),
        "key_id": pk["key_id"],
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    s.call(Some(u), "PUT", &format!("{base}/secrets/{name}"), Some(body)).await.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secrets_are_decrypted_for_jobs_with_github_precedence() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let org = create_org(&s, &u).await;
    let (rp, rid) = org_repo(&s, &u, &org, true).await;
    let (other_rp, other_id) = org_repo(&s, &u, &org, false).await;
    let oa = format!("/orgs/{org}/actions");

    // Org secrets: `all`, `private` (not for the public repo) and `selected`.
    assert_eq!(put_secret(&s, &u, &oa, "SHARED", "from-org", json!({"visibility": "all"})).await, 201);
    assert_eq!(put_secret(&s, &u, &oa, "ORG_ONLY", "org-all", json!({"visibility": "all"})).await, 201);
    assert_eq!(put_secret(&s, &u, &oa, "PRIV", "private-only", json!({"visibility": "private"})).await, 201);
    assert_eq!(put_secret(&s, &u, &oa, "PICKED", "picked", json!({"visibility": "selected", "selected_repository_ids": [other_id]})).await, 201);
    // Repository and environment secrets shadow org ones.
    assert_eq!(put_secret(&s, &u, &format!("{rp}/actions"), "SHARED", "from-repo", json!({})).await, 201);
    assert_eq!(put_secret(&s, &u, &format!("{rp}/environments/prod"), "SHARED", "from-env", json!({})).await, 201);

    let job = actions_settings::secrets_for_job(&s.state, rid, None).await.map_err(|e| e.message).unwrap();
    assert_eq!(
        job,
        vec![("ORG_ONLY".into(), "org-all".into()), ("PRIV".into(), "private-only".into()), ("SHARED".into(), "from-repo".into())]
    );
    let env = actions_settings::secrets_for_job(&s.state, rid, Some("prod")).await.map_err(|e| e.message).unwrap();
    assert!(env.contains(&("SHARED".into(), "from-env".into())), "{env:?}");
    let other = actions_settings::secrets_for_job(&s.state, other_id, None).await.map_err(|e| e.message).unwrap();
    assert_eq!(other, vec![("ORG_ONLY".into(), "org-all".into()), ("PICKED".into(), "picked".into()), ("SHARED".into(), "from-org".into())]);

    // Selected repositories: list / remove / add, and the repo's view of shared org secrets.
    let (st, list) = s.call(Some(&u), "GET", &format!("{oa}/secrets/PICKED/repositories"), None).await;
    assert_eq!((st, list["total_count"].as_i64()), (200, Some(1)));
    assert_eq!(list["repositories"][0]["id"], other_id);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{oa}/secrets/PICKED/repositories/{other_id}"), None).await;
    assert_eq!(st, 204);
    let other = actions_settings::secrets_for_job(&s.state, other_id, None).await.unwrap();
    assert!(!other.iter().any(|(n, _)| n == "PICKED"));
    let (st, _) = s.call(Some(&u), "PUT", &format!("{oa}/secrets/PICKED/repositories/{rid}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{oa}/secrets/SHARED/repositories/{rid}"), None).await;
    assert_eq!(st, 409, "visibility is not 'selected'");
    let (_, shared) = s.call(Some(&u), "GET", &format!("{rp}/actions/organization-secrets"), None).await;
    let names: Vec<&str> = shared["secrets"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["ORG_ONLY", "PICKED", "PRIV", "SHARED"]);
    let (_, shared) = s.call(Some(&u), "GET", &format!("{other_rp}/actions/organization-secrets"), None).await;
    assert_eq!(shared["total_count"], 2, "{shared}");

    // A value that is not a sealed box for the current key is rejected.
    let (_, pk) = s.call(Some(&u), "GET", &format!("{rp}/actions/secrets/public-key"), None).await;
    let (st, e) = s
        .call(Some(&u), "PUT", &format!("{rp}/actions/secrets/BAD"), Some(json!({"encrypted_value": "c2VhbGVk", "key_id": pk["key_id"]})))
        .await;
    assert_eq!(st, 422, "{e}");
    let wrong = crypto::seal_for(&base64_key_of_other(&s, &u).await, "x").unwrap();
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/secrets/BAD"), Some(json!({"encrypted_value": wrong, "key_id": pk["key_id"]}))).await;
    assert_eq!(st, 422);

    // The private key is stored sealed with the master key, never in the clear.
    let sealed: String = sqlx::query_scalar("SELECT sealed_private_key FROM actions_public_keys WHERE owner_kind = 'repo' AND owner_id = $1")
        .bind(rid)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert!(sealed.starts_with("v1:"), "{sealed}");
}

/// Public key of a different (user-owned) repository.
async fn base64_key_of_other(s: &TestServer, u: &TestUser) -> String {
    let repo = s.create_repo(u, false).await;
    let (_, pk) = s.call(Some(u), "GET", &format!("/repos/{}/{repo}/actions/secrets/public-key", u.login), None).await;
    pk["key"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn variables_for_jobs_and_selected_repositories() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let org = create_org(&s, &u).await;
    let (rp, rid) = org_repo(&s, &u, &org, false).await;
    let ov = format!("/orgs/{org}/actions/variables");
    let (st, _) = s.call(Some(&u), "POST", &ov, Some(json!({"name": "REGION", "value": "eu", "visibility": "selected", "selected_repository_ids": [rid]}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &ov, Some(json!({"name": "TIER", "value": "gold", "visibility": "all"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/variables"), Some(json!({"name": "TIER", "value": "silver"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/environments/stage/variables"), Some(json!({"name": "REGION", "value": "us"}))).await;
    assert_eq!(st, 201);

    let vars = actions_settings::variables_for_job(&s.state, rid, None).await.unwrap();
    assert_eq!(vars, vec![("REGION".into(), "eu".into()), ("TIER".into(), "silver".into())]);
    let vars = actions_settings::variables_for_job(&s.state, rid, Some("stage")).await.unwrap();
    assert_eq!(vars, vec![("REGION".into(), "us".into()), ("TIER".into(), "silver".into())]);

    let (st, list) = s.call(Some(&u), "GET", &format!("{ov}/REGION/repositories"), None).await;
    assert_eq!((st, list["total_count"].as_i64()), (200, Some(1)));
    let (st, _) = s.call(Some(&u), "GET", &format!("{ov}/TIER/repositories"), None).await;
    assert_eq!(st, 409);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{ov}/REGION/repositories"), Some(json!({"selected_repository_ids": []}))).await;
    assert_eq!(st, 204);
    let vars = actions_settings::variables_for_job(&s.state, rid, None).await.unwrap();
    assert_eq!(vars, vec![("TIER".into(), "silver".into())]);
    // Repositories outside the org are rejected.
    let foreign = s.create_repo(&u, false).await;
    let fid = repo_id(&s, &u, &format!("/repos/{}/{foreign}", u.login)).await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("{ov}/REGION/repositories"), Some(json!({"selected_repository_ids": [fid]}))).await;
    assert_eq!(st, 422);
    let (_, shared) = s.call(Some(&u), "GET", &format!("{rp}/actions/organization-variables"), None).await;
    assert_eq!(shared["variables"][0]["name"], "TIER", "{shared}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permissions_are_enforced() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let member = s.user().await;
    let org = create_org(&s, &u).await;
    let (rp, rid) = org_repo(&s, &u, &org, false).await;
    let (_, rid2) = org_repo(&s, &u, &org, false).await;
    let op = format!("/orgs/{org}/actions/permissions");

    assert!(actions_settings::actions_allowed(&s.state, rid).await);
    // Org: only selected repositories.
    let (st, _) = s.call(Some(&u), "PUT", &op, Some(json!({"enabled_repositories": "selected"}))).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{op}/repositories"), Some(json!({"selected_repository_ids": [rid]}))).await;
    assert_eq!(st, 204);
    assert!(actions_settings::actions_allowed(&s.state, rid).await);
    assert!(!actions_settings::actions_allowed(&s.state, rid2).await);
    let (_, p) = s.call(Some(&u), "GET", &op, None).await;
    assert_eq!(p["enabled_repositories"], "selected");
    assert!(p["selected_repositories_url"].as_str().unwrap().ends_with("/actions/permissions/repositories"));
    // Repository switch.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions"), Some(json!({"enabled": false}))).await;
    assert_eq!(st, 204);
    assert!(!actions_settings::actions_allowed(&s.state, rid).await);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions"), Some(json!({}))).await;
    assert_eq!(st, 422, "enabled is required");
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions"), Some(json!({"enabled": true, "allowed_actions": "selected"}))).await;
    assert_eq!(st, 204);

    // Allowed actions: org local_only, repo selected with patterns.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions/selected-actions"), Some(json!({"github_owned_allowed": true, "patterns_allowed": ["octo/*"]}))).await;
    assert_eq!(st, 204);
    assert!(actions_settings::action_allowed(&s.state, rid, "actions/checkout@v4").await.unwrap());
    assert!(actions_settings::action_allowed(&s.state, rid, "octo/lint@v1").await.unwrap());
    assert!(!actions_settings::action_allowed(&s.state, rid, "evil/miner@v1").await.unwrap());
    assert!(actions_settings::action_allowed(&s.state, rid, "./.github/actions/local").await.unwrap());
    s.call(Some(&u), "PUT", &op, Some(json!({"enabled_repositories": "all", "allowed_actions": "local_only"}))).await;
    assert!(!actions_settings::action_allowed(&s.state, rid, "actions/checkout@v4").await.unwrap());
    assert!(actions_settings::action_allowed(&s.state, rid, &format!("{org}/shared-action@v1")).await.unwrap());

    // Workflow token: org restricts to read → repo cannot choose write (409).
    let wf = actions_settings::workflow_token_permissions(&s.state, rid).await.unwrap();
    assert!(!wf.write && !wf.can_approve_pull_request_reviews);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions/workflow"), Some(json!({"default_workflow_permissions": "write"}))).await;
    assert_eq!(st, 204);
    assert!(actions_settings::workflow_token_permissions(&s.state, rid).await.unwrap().write);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{op}/workflow"), Some(json!({"default_workflow_permissions": "read", "can_approve_pull_request_reviews": false}))).await;
    assert_eq!(st, 204);
    assert!(!actions_settings::workflow_token_permissions(&s.state, rid).await.unwrap().write);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions/workflow"), Some(json!({"default_workflow_permissions": "write"}))).await;
    assert_eq!(st, 409);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions/workflow"), Some(json!({"can_approve_pull_request_reviews": true}))).await;
    assert_eq!(st, 409);

    // Retention: repo is capped by the org's days.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{op}/artifact-and-log-retention"), Some(json!({"days": 30}))).await;
    assert_eq!(st, 204);
    let (_, r) = s.call(Some(&u), "GET", &format!("{rp}/actions/permissions/artifact-and-log-retention"), None).await;
    assert_eq!(r, json!({"days": 30, "maximum_allowed_days": 30}));
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions/artifact-and-log-retention"), Some(json!({"days": 60}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions/artifact-and-log-retention"), Some(json!({"days": 7}))).await;
    assert_eq!(st, 204);
    assert_eq!(actions_settings::retention_days(&s.state, rid).await.unwrap(), 7);

    // Self-hosted runners limited to selected repositories.
    s.call(Some(&u), "PUT", &format!("{op}/self-hosted-runners"), Some(json!({"enabled_repositories": "selected"}))).await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("{op}/self-hosted-runners/repositories/{rid2}"), None).await;
    assert_eq!(st, 204);
    assert!(actions_settings::self_hosted_runners_allowed(&s.state, rid2).await.unwrap());
    assert!(!actions_settings::self_hosted_runners_allowed(&s.state, rid).await.unwrap());
    s.call(Some(&u), "PUT", &format!("{op}/self-hosted-runners"), Some(json!({"enabled_repositories": "all"}))).await;
    let (st, _) = s.call(Some(&u), "PUT", &format!("{op}/self-hosted-runners/repositories/{rid2}"), None).await;
    assert_eq!(st, 409, "not 'selected'");

    // Fork PR policy (public repo: always runs, read-only, no secrets).
    let fp = actions_settings::fork_pr_policy(&s.state, rid).await.unwrap();
    assert!(fp.run_workflows_from_fork_pull_requests && !fp.send_secrets_and_variables);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions/fork-pr-contributor-approval"), Some(json!({"approval_policy": "nope"}))).await;
    assert_eq!(st, 422);

    // OIDC sub claim template.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/oidc/customization/sub"), Some(json!({"use_default": false}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/oidc/customization/sub"), Some(json!({"use_default": false, "include_claim_keys": ["repo", "context"]}))).await;
    assert_eq!(st, 201);
    assert_eq!(actions_settings::oidc_sub_claim_keys(&s.state, rid).await.unwrap(), Some(vec!["repo".into(), "context".into()]));
    let (_, o) = s.call(Some(&u), "GET", &format!("{rp}/actions/oidc/customization/sub"), None).await;
    assert_eq!(o, json!({"use_default": false, "include_claim_keys": ["repo", "context"]}));

    // Who may manage: org members get 403, strangers 404.
    let (st, _) = s.call(Some(&member), "GET", &op, None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&member), "GET", &format!("{rp}/actions/permissions"), None).await;
    assert_eq!(st, 403, "public repo, not an admin");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_dispatch_respects_disabled_actions() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let content = base64_encode("name: CI\non: workflow_dispatch\njobs: {}\n");
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/contents/.github/workflows/ci.yml"), Some(json!({"message": "ci", "content": content}))).await;
    assert_eq!(st, 201);
    s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions"), Some(json!({"enabled": false}))).await;
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/ci.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 422);
    s.call(Some(&u), "PUT", &format!("{rp}/actions/permissions"), Some(json!({"enabled": true}))).await;
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/ci.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 204);
}

fn base64_encode(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runner_groups() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let org = create_org(&s, &u).await;
    let (_, private_id) = org_repo(&s, &u, &org, true).await;
    let (_, public_id) = org_repo(&s, &u, &org, false).await;
    let g = format!("/orgs/{org}/actions/runner-groups");

    let (st, list) = s.call(Some(&u), "GET", &g, None).await;
    assert_eq!(st, 200);
    let default = list["runner_groups"].as_array().unwrap().iter().find(|x| x["default"] == true && x["inherited"] == false).unwrap().clone();
    assert_eq!(default["name"], "Default");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{g}/{}", default["id"]), None).await;
    assert_eq!(st, 422, "default group cannot be deleted");

    let (st, grp) = s.call(Some(&u), "POST", &g, Some(json!({"name": "builders", "visibility": "selected", "selected_repository_ids": [private_id]}))).await;
    assert_eq!(st, 201, "{grp}");
    let gid = grp["id"].as_i64().unwrap();
    assert!(grp["selected_repositories_url"].as_str().unwrap().ends_with(&format!("/runner-groups/{gid}/repositories")));
    let (st, _) = s.call(Some(&u), "POST", &g, Some(json!({"name": "Builders"}))).await;
    assert_eq!(st, 422, "duplicate name");
    assert!(actions_settings::runner_group_allows(&s.state, gid, private_id, None).await.unwrap());
    assert!(!actions_settings::runner_group_allows(&s.state, gid, public_id, None).await.unwrap());

    let (st, upd) = s
        .call(Some(&u), "PATCH", &format!("{g}/{gid}"), Some(json!({"name": "builders2", "restricted_to_workflows": true, "selected_workflows": [format!("{org}/*/.github/workflows/deploy.yml@*")]})))
        .await;
    assert_eq!((st, upd["name"].as_str()), (200, Some("builders2")));
    let wf_ok = format!("{org}/x/.github/workflows/deploy.yml@refs/heads/main");
    assert!(actions_settings::runner_group_allows(&s.state, gid, private_id, Some(&wf_ok)).await.unwrap());
    assert!(!actions_settings::runner_group_allows(&s.state, gid, private_id, Some("o/x/.github/workflows/ci.yml@main")).await.unwrap());

    let (st, _) = s.call(Some(&u), "PUT", &format!("{g}/{gid}/repositories/{public_id}"), None).await;
    assert_eq!(st, 204);
    let (_, repos) = s.call(Some(&u), "GET", &format!("{g}/{gid}/repositories"), None).await;
    assert_eq!(repos["total_count"], 2);
    // Public repositories still need allows_public_repositories.
    assert!(!actions_settings::runner_group_allows(&s.state, gid, public_id, Some(&wf_ok)).await.unwrap());
    let (_, runners) = s.call(Some(&u), "GET", &format!("{g}/{gid}/runners"), None).await;
    assert_eq!(runners, json!({"total_count": 0, "runners": []}));
    let (st, _) = s.call(Some(&u), "PUT", &format!("{g}/{gid}/runners/12345"), None).await;
    assert_eq!(st, 404, "unknown runner");
    // Runners without a group belong to the default group.
    assert_eq!(actions_settings::runner_group_of(&s.state, "org", repo_owner_id(&s, &org).await, 999).await.unwrap(), default["id"].as_i64().unwrap());
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{g}/{gid}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{g}/{gid}"), None).await;
    assert_eq!(st, 404);
}

async fn repo_owner_id(s: &TestServer, org: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM users WHERE login = $1").bind(org).fetch_one(&s.state.db).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enterprise_settings_are_for_site_admins() {
    let s = TestServer::start().await;
    let admin = site_admin(&s).await;
    let mut u = s.user().await;
    if s.call(Some(&u), "GET", "/user", None).await.1["site_admin"] == true {
        u = s.user().await; // the very first user of a fresh database is a site admin
    }
    let e = "/enterprises/forge/actions/permissions";
    let (st, _) = s.call(Some(&u), "GET", &format!("{e}/workflow"), None).await;
    assert_eq!(st, 404);
    let (st, wf) = s.call(Some(&admin), "GET", &format!("{e}/workflow"), None).await;
    assert_eq!(st, 200);
    assert!(wf["default_workflow_permissions"].is_string());
    let (st, ap) = s.call(Some(&admin), "GET", &format!("{e}/fork-pr-contributor-approval"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&admin), "PUT", &format!("{e}/fork-pr-contributor-approval"), Some(ap.clone())).await;
    assert_eq!(st, 204);
    let (st, groups) = s.call(Some(&admin), "GET", "/enterprises/forge/actions/runner-groups", None).await;
    assert_eq!(st, 200);
    assert!(groups["total_count"].as_i64().unwrap() >= 1);
}
