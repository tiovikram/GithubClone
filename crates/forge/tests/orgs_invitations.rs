//! Organization invitations (pending memberships, accept/decline, teams, failed invitations),
//! organization blocks, the members `role` filter / concealed members, and owner rename.
mod common;

use common::{unique, TestServer, TestUser};
use serde_json::{json, Value};

async fn forge(s: &TestServer, u: &TestUser, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = s
        .client()
        .request(method.parse().unwrap(), format!("{}/api/forge{path}", s.base))
        .header("Authorization", format!("token {}", u.token))
        .header("X-Forge-CSRF", "1");
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let st = res.status().as_u16();
    let text = res.text().await.unwrap();
    (st, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn new_org(s: &TestServer, owner: &TestUser) -> String {
    let org = unique("org");
    let (st, v) = forge(s, owner, "POST", "/orgs", Some(json!({ "login": org }))).await;
    assert_eq!(st, 201, "{v}");
    org
}

async fn user_id(s: &TestServer, u: &TestUser) -> i64 {
    let (_, v) = s.call(Some(u), "GET", "/user", None).await;
    v["id"].as_i64().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitation_is_pending_until_accepted_and_adds_teams() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let invitee = s.user().await;
    let other = s.user().await;
    let org = new_org(&s, &owner).await;
    let (st, team) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/teams"), Some(json!({ "name": "Core" }))).await;
    assert_eq!(st, 201, "{team}");
    let tid = team["id"].as_i64().unwrap();

    // only owners invite; validation
    let iid = user_id(&s, &invitee).await;
    let (st, _) = s.call(Some(&other), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": iid }))).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": iid, "role": "boss" }))).await;
    assert_eq!(st, 422);

    let (st, inv) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": iid, "role": "direct_member", "team_ids": [tid] }))).await;
    assert_eq!(st, 201, "{inv}");
    assert_eq!(inv["login"], json!(invitee.login));
    assert_eq!(inv["team_count"], 1);
    assert!(inv["inviter"]["login"] == json!(owner.login));
    // a second invitation for the same user is rejected
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": iid }))).await;
    assert_eq!(st, 422);

    let (_, list) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/invitations"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, teams) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/invitations/{}/teams", inv["id"]), None).await;
    assert_eq!(teams[0]["slug"], "core");
    let (_, tinv) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/teams/core/invitations"), None).await;
    assert_eq!(tinv.as_array().unwrap().len(), 1);

    // not a member yet: pending membership, no access
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/members/{}", invitee.login), None).await;
    assert_eq!(st, 404);
    let (st, m) = s.call(Some(&invitee), "GET", &format!("/user/memberships/orgs/{org}"), None).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["state"], "pending");
    let (_, pend) = s.call(Some(&invitee), "GET", "/user/memberships/orgs?state=pending", None).await;
    assert_eq!(pend.as_array().unwrap().len(), 1);
    let (_, act) = s.call(Some(&invitee), "GET", "/user/memberships/orgs?state=active", None).await;
    assert_eq!(act.as_array().unwrap().len(), 0);
    let (_, m2) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/memberships/{}", invitee.login), None).await;
    assert_eq!(m2["state"], "pending");

    // accept
    let (st, m) = s.call(Some(&invitee), "PATCH", &format!("/user/memberships/orgs/{org}"), Some(json!({ "state": "active" }))).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["state"], "active");
    assert_eq!(m["role"], "member");
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/members/{}", invitee.login), None).await;
    assert_eq!(st, 204);
    let (st, tm) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/teams/core/memberships/{}", invitee.login), None).await;
    assert_eq!(st, 200, "{tm}");
    let (_, list) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/invitations"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
    // inviting a member fails
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": iid }))).await;
    assert_eq!(st, 422);

    // the members role filter
    let (_, admins) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/members?role=admin"), None).await;
    assert_eq!(admins.as_array().unwrap().iter().map(|u| u["login"].clone()).collect::<Vec<_>>(), vec![json!(owner.login)]);
    let (_, members) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/members?role=member"), None).await;
    assert_eq!(members.as_array().unwrap().iter().map(|u| u["login"].clone()).collect::<Vec<_>>(), vec![json!(invitee.login)]);
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/members?role=nope"), None).await;
    assert_eq!(st, 422);
    // non-members only see public members
    let (_, seen) = s.call(Some(&other), "GET", &format!("/orgs/{org}/members"), None).await;
    assert_eq!(seen.as_array().unwrap().len(), 0);
    let (st, _) = s.call(Some(&invitee), "PUT", &format!("/orgs/{org}/public_members/{}", invitee.login), None).await;
    assert_eq!(st, 204);
    let (_, seen) = s.call(Some(&other), "GET", &format!("/orgs/{org}/members"), None).await;
    assert_eq!(seen.as_array().unwrap().len(), 1);
    let (_, seen) = s.call(Some(&invitee), "GET", &format!("/orgs/{org}/members"), None).await;
    assert_eq!(seen.as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decline_cancel_email_and_owner_invitations() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let a = s.user().await;
    let b = s.user().await;
    let org = new_org(&s, &owner).await;

    // by email (the test users' emails are <login>@example.com), as owner
    let (st, inv) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "email": format!("{}@example.com", a.login), "role": "admin" }))).await;
    assert_eq!(st, 201, "{inv}");
    assert_eq!(inv["login"], json!(a.login));
    // an unknown email stays an email invitation
    let (st, inv2) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "email": "nobody@example.org" }))).await;
    assert_eq!(st, 201);
    assert_eq!(inv2["login"], Value::Null);
    assert_eq!(inv2["email"], "nobody@example.org");
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/invitations/{}", inv2["id"]), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/invitations/{}", inv2["id"]), None).await;
    assert_eq!(st, 404);

    let (st, m) = s.call(Some(&a), "PATCH", &format!("/user/memberships/orgs/{org}"), Some(json!({ "state": "active" }))).await;
    assert_eq!(st, 200);
    assert_eq!(m["role"], "admin");

    // decline (forge-only endpoint)
    let bid = user_id(&s, &b).await;
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": bid }))).await;
    assert_eq!(st, 201);
    let (st, _) = forge(&s, &b, "DELETE", &format!("/user/memberships/orgs/{org}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&b), "GET", &format!("/user/memberships/orgs/{org}"), None).await;
    assert_eq!(st, 404);
    let (st, _) = forge(&s, &b, "DELETE", &format!("/user/memberships/orgs/{org}"), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&b), "PATCH", &format!("/user/memberships/orgs/{org}"), Some(json!({ "state": "active" }))).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_users_fail_invitations_and_cannot_interact() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let pest = s.user().await;
    let org = new_org(&s, &owner).await;
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/memberships/{}", member.login), Some(json!({ "role": "member" }))).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/repos"), Some(json!({ "name": "pub", "auto_init": true }))).await;
    assert_eq!(st, 201);
    let pid = user_id(&s, &pest).await;
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": pid }))).await;
    assert_eq!(st, 201);

    // members can't be blocked; only owners block
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/blocks/{}", member.login), None).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&member), "PUT", &format!("/orgs/{org}/blocks/{}", pest.login), None).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/blocks/{}", pest.login), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/blocks/{}", pest.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/blocks/{}", pest.login), None).await;
    assert_eq!(st, 204);
    let (_, list) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/blocks"), None).await;
    assert_eq!(list[0]["login"], json!(pest.login));

    // the pending invitation failed
    let (_, pending) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/invitations"), None).await;
    assert_eq!(pending.as_array().unwrap().len(), 0);
    let (_, failed) = s.call(Some(&owner), "GET", &format!("/orgs/{org}/failed_invitations"), None).await;
    assert_eq!(failed[0]["login"], json!(pest.login));
    assert!(failed[0]["failed_reason"].as_str().unwrap().contains("blocked"));
    assert!(failed[0]["failed_at"].is_string());
    let (st, _) = s.call(Some(&pest), "GET", &format!("/user/memberships/orgs/{org}"), None).await;
    assert_eq!(st, 404);

    // blocked: no invitations, no direct adds, no issues or comments
    let (st, _) = s.call(Some(&owner), "POST", &format!("/orgs/{org}/invitations"), Some(json!({ "invitee_id": pid }))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/memberships/{}", pest.login), Some(json!({ "role": "member" }))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&pest), "POST", &format!("/repos/{org}/pub/issues"), Some(json!({ "title": "spam" }))).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&member), "POST", &format!("/repos/{org}/pub/issues"), Some(json!({ "title": "real" }))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&pest), "POST", &format!("/repos/{org}/pub/issues/1/comments"), Some(json!({ "body": "spam" }))).await;
    assert_eq!(st, 403);

    // unblock restores
    let (st, _) = s.call(Some(&owner), "DELETE", &format!("/orgs/{org}/blocks/{}", pest.login), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&pest), "POST", &format!("/repos/{org}/pub/issues"), Some(json!({ "title": "ok now" }))).await;
    assert_eq!(st, 201);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owners_rename_their_organization() {
    let s = TestServer::start().await;
    let owner = s.user().await;
    let member = s.user().await;
    let org = new_org(&s, &owner).await;
    let (st, _) = s.call(Some(&owner), "PUT", &format!("/orgs/{org}/memberships/{}", member.login), Some(json!({ "role": "member" }))).await;
    assert_eq!(st, 200);
    let new = unique("renamed");
    let (st, _) = forge(&s, &member, "PATCH", &format!("/orgs/{org}"), Some(json!({ "login": new }))).await;
    assert_eq!(st, 403);
    let (st, _) = forge(&s, &owner, "PATCH", &format!("/orgs/{org}"), Some(json!({ "login": "bad name!" }))).await;
    assert_eq!(st, 422);
    let (st, _) = forge(&s, &owner, "PATCH", &format!("/orgs/{org}"), Some(json!({ "login": member.login }))).await;
    assert_eq!(st, 422);
    let (st, v) = forge(&s, &owner, "PATCH", &format!("/orgs/{org}"), Some(json!({ "login": new }))).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["login"], json!(new));
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{new}"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&owner), "GET", &format!("/orgs/{org}"), None).await;
    assert_eq!(st, 404);
}
