//! Actions with no runner attached: workflow enable/disable, dispatched runs (queued until cancelled),
//! run cancel/rerun/delete, and secrets/variables for repositories and environments.
mod common;

use base64::Engine;
use common::{TestServer, TestUser};
use serde_json::{json, Value};

async fn put_file(s: &TestServer, u: &TestUser, rp: &str, path: &str, content: &str) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(content);
    let (st, v) = s.call(Some(u), "PUT", &format!("{rp}/contents/{path}"), Some(json!({"message": format!("add {path}"), "content": b64}))).await;
    assert!(st == 201 || st == 200, "{st} {v}");
}

async fn setup(s: &TestServer) -> (TestUser, String, i64) {
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{}", u.login, repo);
    put_file(s, &u, &rp, ".github/workflows/ci.yml", "name: CI\non:\n  push:\n  workflow_dispatch:\njobs:\n  b:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n").await;
    put_file(s, &u, &rp, ".github/workflows/push-only.yml", "name: Push only\non: push\njobs:\n  b:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n").await;
    let (st, wfs) = s.call(Some(&u), "GET", &format!("{rp}/actions/workflows"), None).await;
    assert_eq!(st, 200);
    assert_eq!(wfs["total_count"], 2, "{wfs}");
    let ci = wfs["workflows"].as_array().unwrap().iter().find(|w| w["name"] == "CI").unwrap()["id"].as_i64().unwrap();
    (u, rp, ci)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_state_dispatch_and_run_lifecycle() {
    let s = TestServer::start().await;
    let (u, rp, ci) = setup(&s).await;
    let other = s.user().await;

    // Disable -> state reflected; dispatch refused while disabled.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/workflows/{ci}/disable"), None).await;
    assert_eq!(st, 204);
    let (_, w) = s.call(Some(&u), "GET", &format!("{rp}/actions/workflows/ci.yml"), None).await;
    assert_eq!(w["state"], "disabled_manually");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/{ci}/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/workflows/{ci}/enable"), None).await;
    assert_eq!(st, 204);

    // Non-collaborators can't toggle or dispatch.
    let (st, _) = s.call(Some(&other), "PUT", &format!("{rp}/actions/workflows/{ci}/disable"), None).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&other), "POST", &format!("{rp}/actions/workflows/{ci}/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 403);

    // Validation: unknown ref, workflow without workflow_dispatch.
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/{ci}/dispatches"), Some(json!({"ref": "nope"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/push-only.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 422);

    // Dispatch twice -> two queued runs with increasing run numbers.
    for _ in 0..2 {
        let (st, v) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/ci.yml/dispatches"), Some(json!({"ref": "main", "inputs": {}}))).await;
        assert_eq!(st, 204, "{v}");
    }
    let (st, runs) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?branch=main&event=workflow_dispatch&status=queued"), None).await;
    assert_eq!(st, 200);
    assert_eq!(runs["total_count"], 2, "{runs}");
    let run = &runs["workflow_runs"][0];
    // run numbers also count the two push runs (both workflow files trigger ci.yml on push)
    assert_eq!(run["run_number"], 4);
    assert_eq!(run["status"], "queued");
    assert_eq!(run["event"], "workflow_dispatch");
    assert_eq!(run["name"], "CI");
    assert_eq!(run["head_branch"], "main");
    assert_eq!(run["head_commit"]["id"], run["head_sha"]);
    assert_eq!(run["actor"]["login"], u.login.as_str());
    let id = run["id"].as_i64().unwrap();
    let (_, per_wf) = s.call(Some(&u), "GET", &format!("{rp}/actions/workflows/{ci}/runs"), None).await;
    assert_eq!(per_wf["total_count"], 4, "2 push + 2 dispatch runs");
    let (_, none) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?status=success"), None).await;
    assert_eq!(none["total_count"], 0);

    // One queued job (no runner picks it up); no artifacts or logs yet.
    let (st, jobs) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/jobs"), None).await;
    assert_eq!((st, jobs["total_count"].clone(), jobs["jobs"][0]["status"].clone()), (200, json!(1), json!("queued")));
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/logs"), None).await;
    assert_eq!(st, 404);
    let (st, arts) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/artifacts"), None).await;
    assert_eq!((st, arts["total_count"].clone()), (200, json!(0)));

    // Cancel -> completed/cancelled; second cancel conflicts; rerun requeues as attempt 2.
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{id}/cancel"), None).await;
    assert_eq!(st, 202);
    let (_, r) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}"), None).await;
    assert_eq!((r["status"].clone(), r["conclusion"].clone()), (json!("completed"), json!("cancelled")));
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{id}/cancel"), None).await;
    assert_eq!(st, 409);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{id}/rerun"), None).await;
    assert_eq!(st, 201);
    let (_, r) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}"), None).await;
    assert_eq!((r["status"].clone(), r["run_attempt"].clone()), (json!("queued"), json!(2)));
    let (st, a1) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/attempts/1"), None).await;
    assert_eq!((st, a1["run_attempt"].clone(), a1["conclusion"].clone()), (200, json!(1), json!("cancelled")));
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/attempts/3"), None).await;
    assert_eq!(st, 404);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{id}/rerun"), None).await;
    assert_eq!(st, 403, "a queued run can't be re-run");

    // Delete.
    let (st, _) = s.call(Some(&other), "DELETE", &format!("{rp}/actions/runs/{id}"), None).await;
    assert_eq!(st, 403);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/runs/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}"), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secrets_and_variables() {
    let s = TestServer::start().await;
    let (u, rp, _) = setup(&s).await;
    let other = s.user().await;

    // Public key: stable per repository, 32 bytes.
    let (st, pk) = s.call(Some(&u), "GET", &format!("{rp}/actions/secrets/public-key"), None).await;
    assert_eq!(st, 200);
    let key = base64::engine::general_purpose::STANDARD.decode(pk["key"].as_str().unwrap()).unwrap();
    assert_eq!(key.len(), 32);
    let (_, pk2) = s.call(Some(&u), "GET", &format!("{rp}/actions/secrets/public-key"), None).await;
    assert_eq!(pk, pk2);
    let key_id = pk["key_id"].as_str().unwrap();

    // Create (201), update (204), wrong key id (422), bad name (422); value never returned.
    let body = json!({"encrypted_value": "c2VhbGVk", "key_id": key_id});
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/secrets/API_KEY"), Some(body.clone())).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/secrets/API_KEY"), Some(body.clone())).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/secrets/X"), Some(json!({"encrypted_value": "c2VhbGVk", "key_id": "1"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/actions/secrets/GITHUB_TOKEN"), Some(body.clone())).await;
    assert_eq!(st, 422);
    let (st, list) = s.call(Some(&u), "GET", &format!("{rp}/actions/secrets"), None).await;
    assert_eq!(st, 200);
    assert_eq!(list["total_count"], 1);
    assert_eq!(list["secrets"][0]["name"], "API_KEY");
    assert!(list["secrets"][0].get("encrypted_value").is_none());
    let (st, _) = s.call(Some(&other), "GET", &format!("{rp}/actions/secrets"), None).await;
    assert_eq!(st, 403);

    // Environment secrets are separate from repository secrets.
    let (st, _) = s.call(Some(&u), "PUT", &format!("{rp}/environments/prod/secrets/DEPLOY"), Some(body.clone())).await;
    assert_eq!(st, 201);
    let (_, env_list) = s.call(Some(&u), "GET", &format!("{rp}/environments/prod/secrets"), None).await;
    assert_eq!(env_list["total_count"], 1);
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/actions/secrets"), None).await;
    assert_eq!(list["total_count"], 1);

    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/secrets/API_KEY"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/secrets/API_KEY"), None).await;
    assert_eq!(st, 404);

    // Variables: create, duplicate (409, which gh uses to switch to PATCH), update, get, delete.
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/variables"), Some(json!({"name": "greeting", "value": "hello"}))).await;
    assert_eq!(st, 201);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/variables"), Some(json!({"name": "GREETING", "value": "x"}))).await;
    assert_eq!(st, 409);
    let (st, _) = s.call(Some(&u), "PATCH", &format!("{rp}/actions/variables/GREETING"), Some(json!({"value": "howdy"}))).await;
    assert_eq!(st, 204);
    let (st, v) = s.call(Some(&u), "GET", &format!("{rp}/actions/variables/GREETING"), None).await;
    assert_eq!((st, v["value"].clone()), (200, json!("howdy")));
    let (_, list): (u16, Value) = s.call(Some(&u), "GET", &format!("{rp}/actions/variables"), None).await;
    assert_eq!(list["total_count"], 1);
    assert_eq!(list["variables"][0]["name"], "GREETING");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/variables/GREETING"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/actions/variables/GREETING"), None).await;
    assert_eq!(st, 404);
}
