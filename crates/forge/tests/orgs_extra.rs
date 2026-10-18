//! Organizations build-out: base repository permission, teams (nesting, repo access,
//! visibility), custom repository roles, org roles, outside collaborators, private registries
//! (sealed-box secrets), custom properties, audit log.
mod common;

use base64::Engine;
use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn org_with(s: &TestServer, owner: &TestUser, members: &[&TestUser]) -> String {
    let org = unique("org");
    // `/admin/organizations` is for site admins (security #20): use the self-service endpoint.
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
    for m in members {
        let (st, v) = s.call(Some(owner), "PUT", &format!("/orgs/{org}/memberships/{}", m.login), Some(json!({ "role": "member" }))).await;
        assert_eq!(st, 200, "{v}");
    }
    org
}

async fn org_repo(s: &TestServer, owner: &TestUser, org: &str, private: bool) -> String {
    let name = unique("r");
    let (st, v) = s.call(Some(owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({ "name": name, "private": private, "auto_init": true }))).await;
    assert_eq!(st, 201, "{v}");
    name
}

/// The viewer's permission as reported by `permissions` in GET /repos (404 = no access).
async fn perm_of(s: &TestServer, u: &TestUser, org: &str, repo: &str) -> &'static str {
    let (st, v) = s.call(Some(u), "GET", &format!("/repos/{org}/{repo}"), None).await;
    if st == 404 {
        return "none";
    }
    assert_eq!(st, 200, "{v}");
    let p = &v["permissions"];
    for k in ["admin", "maintain", "push", "triage", "pull"] {
        if p[k] == json!(true) {
            return k;
        }
    }
    "none"
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base_permission_defaults_to_read_and_is_honoured() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = org_with(&s, &owner, &[&member]).await;
    let private = org_repo(&s, &owner, &org, true).await;
    let public = org_repo(&s, &owner, &org, false).await;

    // GitHub's default is read: members can read (not write) every repository
    let (_, o) = s.call(Some(&owner), "GET", &format!("/orgs/{org}"), None).await;
    assert_eq!(o["default_repository_permission"], "read");
    assert_eq!(perm_of(&s, &member, &org, &private).await, "pull");
    let (st, _) = s.call(Some(&member), "POST", &format!("/repos/{org}/{private}/issues"), Some(json!({ "title": "x" }))).await;
    assert_eq!(st, 201, "read can open issues");
    let (st, _) = s.call(Some(&member), "POST", &format!("/repos/{org}/{private}/labels"), Some(json!({ "name": "nope", "color": "ffffff" }))).await;
    assert_eq!(st, 403, "read cannot write");
    // members don't see the private settings
    let (_, o) = s.call(Some(&member), "GET", &format!("/orgs/{org}"), None).await;
    assert!(o.get("default_repository_permission").is_none());

    // only owners change it
    let (st, _) = s.call(Some(&member), "PATCH", &format!("/orgs/{org}"), Some(json!({ "default_repository_permission": "admin" }))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "default_repository_permission": "bogus" }))).await;
    assert_eq!(st, 422);

    for (setting, private_perm, public_perm) in [("none", "none", "pull"), ("write", "push", "push"), ("admin", "admin", "admin")] {
        let (st, v) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "default_repository_permission": setting }))).await;
        assert_eq!(st, 200, "{v}");
        assert_eq!(v["default_repository_permission"], setting);
        assert_eq!(perm_of(&s, &member, &org, &private).await, private_perm, "{setting}");
        assert_eq!(perm_of(&s, &member, &org, &public).await, public_perm, "{setting}");
    }
    // the collaborators list follows the base permission too
    s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "default_repository_permission": "none" }))).await;
    let (_, list) = s.call(Some(&owner), "GET", &format!("/repos/{org}/{private}/collaborators"), None).await;
    assert!(!list.to_string().contains(&format!("\"{}\"", member.login)), "{list}");

    // and is recorded in the audit log
    let (st, log) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/audit-log?phrase=action:org.update_default_repository_permission"), None).await;
    assert_eq!(st, 200, "{log}");
    assert!(log.as_array().unwrap().len() >= 3, "{log}");
    let (st, _) = s.call(Some(&member), "GET", &format!("/orgs/{org}/audit-log"), None).await;
    assert_eq!(st, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_repo_creation_privileges_and_creator_admin() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = org_with(&s, &owner, &[&member]).await;
    let name = unique("m");
    let (st, _) = s.call(Some(&member), "POST", &format!("/orgs/{org}/repos"), Some(json!({ "name": name, "private": true }))).await;
    assert_eq!(st, 201);
    assert_eq!(perm_of(&s, &member, &org, &name).await, "admin", "creators administer their repo");
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "members_allowed_repository_creation_type": "private" }))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&member), "POST", &format!("/orgs/{org}/repos"), Some(json!({ "name": unique("m"), "private": false }))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&member), "POST", &format!("/orgs/{org}/repos"), Some(json!({ "name": unique("m"), "private": true }))).await;
    assert_eq!(st, 201);
    let (_, o) = s.call(Some(&owner), "GET", &format!("/orgs/{org}"), None).await;
    assert_eq!(o["members_can_create_public_repositories"], false);
    assert_eq!(o["members_allowed_repository_creation_type"], "private");
    s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "members_can_create_repositories": false }))).await;
    let (st, _) = s.call(Some(&member), "POST", &format!("/orgs/{org}/repos"), Some(json!({ "name": unique("m"), "private": true }))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({ "name": unique("m") }))).await;
    assert_eq!(st, 201, "owners always can");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_repo_access_is_inherited_by_child_teams() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = org_with(&s, &owner, &[&member]).await;
    s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "default_repository_permission": "none" }))).await;
    let repo = org_repo(&s, &owner, &org, true).await;
    let (st, parent) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({ "name": unique("Parent "), "privacy": "closed" }))).await;
    assert_eq!(st, 201, "{parent}");
    let (st, child) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({ "name": unique("child"), "parent_team_id": parent["id"] }))).await;
    assert_eq!(st, 201, "{child}");
    assert_eq!(child["privacy"], "closed", "nested teams are closed");
    assert_eq!(child["parent"]["id"], parent["id"]);
    let (ps, cs) = (parent["slug"].as_str().unwrap().to_string(), child["slug"].as_str().unwrap().to_string());

    // secret teams cannot be nested; no cycles
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({ "name": unique("x"), "parent_team_id": parent["id"], "privacy": "secret" }))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}/teams/{ps}"), Some(json!({ "parent_team_id": child["id"] }))).await;
    assert_eq!(st, 422);

    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{cs}/memberships/{}", member.login), Some(json!({ "role": "member" }))).await;
    assert_eq!(st, 200);
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "none");
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{ps}/repos/{org}/{repo}"), Some(json!({ "permission": "push" }))).await;
    assert_eq!(st, 204);
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "push", "inherited from the parent team");
    // parent team lists child-team members as inherited
    let (_, m) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/teams/{ps}/members"), None).await;
    assert!(m.to_string().contains(&member.login));
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/teams/{cs}/repos/{org}/{repo}"), None).await;
    assert_eq!(st, 204);
    let (_, list) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/teams/{ps}/repos"), None).await;
    assert_eq!(list[0]["role_name"], "write");
    let (_, t) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/teams/{ps}"), None).await;
    assert_eq!(t["repos_count"], 1);

    // members (not maintainers) cannot manage the team
    let (st, _) = s.call(Some(&member), "PATCH", &format!("/orgs/{org}/teams/{cs}"), Some(json!({ "description": "x" }))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/teams/{ps}/repos/{org}/{repo}"), None).await;
    assert_eq!(st, 204);
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "none");

    // deleting the parent deletes the child
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/teams/{}", parent["id"]), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&owner), "GET", &format!("/teams/{}", child["id"]), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secret_teams_are_visible_to_their_members_and_owners_only() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let a = s.user().await;
    let b = s.user().await;
    let outsider = s.user().await;
    let org = org_with(&s, &owner, &[&a, &b]).await;
    let (_, t) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({ "name": unique("hidden"), "privacy": "secret" }))).await;
    let slug = t["slug"].as_str().unwrap().to_string();
    s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{slug}/memberships/{}", a.login), None).await;
    let (st, _) = s.call(Some(&a), "GET", &format!("/orgs/{org}/teams/{slug}"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&b), "GET", &format!("/orgs/{org}/teams/{slug}"), None).await;
    assert_eq!(st, 404);
    let (_, list) = s.call(Some(&b), "GET", &format!("/orgs/{org}/teams"), None).await;
    assert!(!list.to_string().contains(&slug));
    let (st, _) = s.call(Some(&outsider), "GET", &format!("/teams/{}", t["id"]), None).await;
    assert_eq!(st, 404);
    // legacy add-member requires org membership
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/teams/{}/members/{}", t["id"], outsider.login), None).await;
    assert_eq!(st, 422);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn custom_repo_roles_and_org_roles_grant_their_base_role() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = org_with(&s, &owner, &[&member]).await;
    s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "default_repository_permission": "none" }))).await;
    let repo = org_repo(&s, &owner, &org, true).await;

    let (st, role) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/custom-repository-roles"),
        Some(json!({ "name": "Labeler", "base_role": "triage", "permissions": ["add_label"] }))).await;
    assert_eq!(st, 201, "{role}");
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/custom-repository-roles"),
        Some(json!({ "name": "Bad", "base_role": "triage", "permissions": ["fly"] }))).await;
    assert_eq!(st, 422);
    let (_, t) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({ "name": unique("labelers"), "privacy": "closed" }))).await;
    let slug = t["slug"].as_str().unwrap().to_string();
    s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{slug}/memberships/{}", member.login), None).await;
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{slug}/repos/{org}/{repo}"), Some(json!({ "permission": "Labeler" }))).await;
    assert_eq!(st, 204);
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "triage");
    let (_, list) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/teams/{slug}/repos"), None).await;
    assert_eq!(list[0]["role_name"], "Labeler");
    // renaming follows grants; deleting falls back to the base role
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}/custom-repository-roles/{}", role["id"]), Some(json!({ "name": "Triager+", "base_role": "maintain" }))).await;
    assert_eq!(st, 200);
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "maintain");
    s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/custom-repository-roles/{}", role["id"]), None).await;
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "maintain");
    s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/teams/{slug}/repos/{org}/{repo}"), None).await;
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "none");

    // predefined all_repo_write (8133) gives write on every repository
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/organization-roles/users/{}/8133", member.login), None).await;
    assert_eq!(st, 204);
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "push");
    let (_, users) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/organization-roles/8133/users"), None).await;
    assert_eq!(users[0]["assignment"], "direct");
    s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/organization-roles/users/{}", member.login), None).await;
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "none");

    // a custom org role with read_audit_logs, assigned through a team, opens the audit log;
    // security managers (a team role) read every repository
    let (st, r) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/organization-roles"), Some(json!({ "name": "Auditor", "permissions": ["read_audit_logs"] }))).await;
    assert_eq!(st, 201, "{r}");
    let (st, _) = s.call(Some(&member), "GET", &format!("/orgs/{org}/audit-log"), None).await;
    assert_eq!(st, 403);
    s.call(Some(&owner), "PUT", &format!("/orgs/{org}/organization-roles/teams/{slug}/{}", r["id"]), None).await;
    let (st, _) = s.call(Some(&member), "GET", &format!("/orgs/{org}/audit-log"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/security-managers/teams/{slug}"), None).await;
    assert_eq!(st, 204);
    assert_eq!(perm_of(&s, &member, &org, &repo).await, "pull");
    let (_, sm) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/security-managers"), None).await;
    assert_eq!(sm[0]["slug"], json!(slug));
    // role assignment requires membership
    let outsider = s.user().await;
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/organization-roles/users/{}/8133", outsider.login), None).await;
    assert_eq!(st, 422);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn converting_to_outside_collaborator_keeps_team_access() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = org_with(&s, &owner, &[&member]).await;
    s.call(Some(&owner), "PATCH", &format!("/orgs/{org}"), Some(json!({ "default_repository_permission": "none" }))).await;
    let kept = org_repo(&s, &owner, &org, true).await;
    let other = org_repo(&s, &owner, &org, true).await;
    let (_, t) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({ "name": unique("t"), "privacy": "closed" }))).await;
    let slug = t["slug"].as_str().unwrap().to_string();
    s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{slug}/memberships/{}", member.login), None).await;
    s.call(Some(&owner), "PUT", &format!("/orgs/{org}/teams/{slug}/repos/{org}/{kept}"), Some(json!({ "permission": "push" }))).await;

    // last-owner protection
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/outside_collaborators/{}", owner.login), None).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/outside_collaborators/{}", member.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/members/{}", member.login), None).await;
    assert_eq!(st, 404);
    assert_eq!(perm_of(&s, &member, &org, &kept).await, "push");
    assert_eq!(perm_of(&s, &member, &org, &other).await, "none");
    let (_, list) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/outside_collaborators"), None).await;
    assert_eq!(list[0]["login"], json!(member.login));
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/outside_collaborators/{}", owner.login), None).await;
    assert_eq!(st, 422, "members are not outside collaborators");
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/outside_collaborators/{}", member.login), None).await;
    assert_eq!(st, 204);
    assert_eq!(perm_of(&s, &member, &org, &kept).await, "none");
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/members/{}", owner.login), None).await;
    assert_eq!(st, 403, "the last owner cannot leave");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_registry_secrets_are_sealed_boxes() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = org_with(&s, &owner, &[&member]).await;
    let b64 = base64::engine::general_purpose::STANDARD;
    let (st, k) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/private-registries/public-key"), None).await;
    assert_eq!(st, 200, "{k}");
    let pk: [u8; 32] = b64.decode(k["key"].as_str().unwrap()).unwrap().try_into().unwrap();
    let sealed = b64.encode(forge::orgs::sealed_box::seal(b"npm-token", &pk));
    let body = |ev: &str, kid: &Value| json!({ "registry_type": "npm_registry", "url": "https://npm.example.com", "visibility": "all", "encrypted_value": ev, "key_id": kid });

    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/private-registries"), Some(body(&b64.encode([7u8; 60]), &k["key_id"]))).await;
    assert_eq!(st, 422, "not a sealed box for our key");
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/private-registries"), Some(body(&sealed, &json!("1")))).await;
    assert_eq!(st, 422, "wrong key id");
    let (st, _) = s.call(Some(&member), "POST", &format!("/orgs/{org}/private-registries"), Some(body(&sealed, &k["key_id"]))).await;
    assert_eq!(st, 403);
    let (st, c) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/private-registries"), Some(body(&sealed, &k["key_id"]))).await;
    assert_eq!(st, 201, "{c}");
    assert_eq!(c["name"], "NPM_REGISTRY_SECRET");
    assert_eq!(c["auth_type"], "token");
    assert!(!c.to_string().contains(&sealed), "the secret is never returned");
    let (_, c2) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/private-registries"), Some(body(&sealed, &k["key_id"]))).await;
    assert_eq!(c2["name"], "NPM_REGISTRY_SECRET_2");

    // forge can open the stored secret
    let o = forge::db::user_by_login(&s.state.db, &org).await.map_err(|e| e.message).unwrap();
    let ev: String = sqlx::query_scalar("SELECT encrypted_value FROM org_private_registries WHERE org_id = $1 AND name = 'NPM_REGISTRY_SECRET'")
        .bind(o.id)
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    let plain = forge::api::orgs_registries::open_secret(&s.state, &o, &ev).await.map_err(|e| e.message).unwrap();
    assert_eq!(plain.as_deref(), Some(&b"npm-token"[..]));

    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}/private-registries/NPM_REGISTRY_SECRET"), Some(json!({ "visibility": "selected", "selected_repository_ids": [1] }))).await;
    assert_eq!(st, 422, "selected repos must belong to the org");
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}/private-registries/NPM_REGISTRY_SECRET"), Some(json!({ "username": "bot", "auth_type": "username_password" }))).await;
    assert_eq!(st, 204);
    let (_, g) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/private-registries/NPM_REGISTRY_SECRET"), None).await;
    assert_eq!(g["username"], "bot");
    let (_, l) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/private-registries"), None).await;
    assert_eq!(l["total_count"], 2);
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/private-registries/NPM_REGISTRY_SECRET_2"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/private-registries/NPM_REGISTRY_SECRET_2"), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn custom_properties_validate_values() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let org = org_with(&s, &owner, &[]).await;
    let repo = org_repo(&s, &owner, &org, false).await;
    let (st, v) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/properties/schema/env"),
        Some(json!({ "value_type": "single_select", "allowed_values": ["prod", "dev"], "required": true, "default_value": "dev" }))).await;
    assert_eq!(st, 200, "{v}");
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/properties/schema/bad"), Some(json!({ "value_type": "single_select" }))).await;
    assert_eq!(st, 422, "select needs allowed values");
    let (_, vals) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/properties/values"), None).await;
    assert_eq!(vals[0]["properties"][0]["value"], "dev", "default applies");
    let set = |val: Value| json!({ "repository_names": [repo.clone()], "properties": [{ "property_name": "env", "value": val }] });
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}/properties/values"), Some(set(json!("staging")))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}/properties/values"), Some(set(json!(null)))).await;
    assert_eq!(st, 422, "required");
    let (st, _) = s.call(Some(&owner), "PATCH", &format!("/orgs/{org}/properties/values"), Some(set(json!("prod")))).await;
    assert_eq!(st, 204);
    let (_, vals) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/properties/values?repository_query=props.env:prod"), None).await;
    assert_eq!(vals.as_array().unwrap().len(), 1);
    assert_eq!(vals[0]["properties"][0]["value"], "prod");
}
