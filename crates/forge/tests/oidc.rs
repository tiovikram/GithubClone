//! Actions OIDC end to end: a workflow with `permissions: id-token: write` requests ID tokens
//! from `ACTIONS_ID_TOKEN_REQUEST_URL` on an in-process `forge runner`; the test verifies the
//! JWTs against the issuer's JWKS and checks the claims.
mod common;

use std::time::Duration;

use aws_lc_rs::signature::{RsaPublicKeyComponents, RSA_PKCS1_2048_8192_SHA256};
use base64::Engine;
use common::{TestServer, TestUser};
use serde_json::{json, Value};

const B64URL: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

async fn put_file(s: &TestServer, u: &TestUser, rp: &str, path: &str, content: &str) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(content);
    let (_, cur) = s.call(Some(u), "GET", &format!("{rp}/contents/{path}"), None).await;
    let mut body = json!({"message": format!("update {path}"), "content": b64});
    if let Some(sha) = cur.get("sha") {
        body["sha"] = sha.clone();
    }
    let (st, v) = s.call(Some(u), "PUT", &format!("{rp}/contents/{path}"), Some(body)).await;
    assert!(st == 201 || st == 200, "{st} {v}");
}

async fn start_runner(s: &TestServer, u: &TestUser, rp: &str) -> tokio::task::JoinHandle<()> {
    let (st, tok) = s.call(Some(u), "POST", &format!("{rp}/actions/runners/registration-token"), None).await;
    assert_eq!(st, 201, "{tok}");
    let args = forge::runner::agent::RunnerArgs {
        url: Some(s.base.clone()),
        token: Some(tok["token"].as_str().unwrap().to_string()),
        name: Some(common::unique("runner-")),
        labels: String::new(),
        executor: "shell".into(),
        work_dir: Some(s.dir.path().join(common::unique("w"))),
        config: None,
        jitconfig: None,
        ephemeral: false,
        once: false,
        slots: 2,
        remove_token: None,
        keep: false,
    };
    tokio::spawn(async move {
        if let Err(e) = forge::runner::agent::main(args).await {
            eprintln!("runner exited: {e:#}");
        }
    })
}

async fn dispatch_and_wait(s: &TestServer, u: &TestUser, rp: &str, wf: &str) -> Vec<(String, String)> {
    let (st, v) = s.call(Some(u), "POST", &format!("{rp}/actions/workflows/{wf}/dispatches"), Some(json!({"ref": "main", "return_run_details": true}))).await;
    assert_eq!(st, 200, "{v}");
    let id = v["workflow_run_id"].as_i64().unwrap();
    for _ in 0..600 {
        let (_, r) = s.call(Some(u), "GET", &format!("{rp}/actions/runs/{id}"), None).await;
        if r["status"] == "completed" {
            let (_, jobs) = s.call(Some(u), "GET", &format!("{rp}/actions/runs/{id}/jobs"), None).await;
            let jobs = jobs["jobs"].as_array().unwrap().clone();
            if jobs.iter().all(|j| j["status"] == "completed") {
                let mut out = Vec::new();
                for j in jobs {
                    let res = s.client().get(format!("{}{rp}/actions/jobs/{}/logs", s.api, j["id"])).header("Authorization", format!("token {}", u.token)).send().await.unwrap();
                    out.push((j["name"].as_str().unwrap().to_string(), res.text().await.unwrap()));
                }
                return out;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("run {id} did not complete");
}

fn line_value(log: &str, key: &str) -> String {
    let l = log.lines().find(|l| l.contains(key) && !l.contains("echo")).unwrap_or_else(|| panic!("{key} missing in {log}"));
    l[l.find(key).unwrap() + key.len()..].trim().to_string()
}

/// Verifies `jwt` with the JWKS key named by its `kid`; returns the claims.
fn verify(jwt: &str, jwks: &Value) -> Value {
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3, "{jwt}");
    let header: Value = serde_json::from_slice(&B64URL.decode(parts[0]).unwrap()).unwrap();
    assert_eq!(header["alg"], "RS256");
    let key = jwks["keys"].as_array().unwrap().iter().find(|k| k["kid"] == header["kid"]).expect("kid in JWKS");
    let n = B64URL.decode(key["n"].as_str().unwrap()).unwrap();
    let e = B64URL.decode(key["e"].as_str().unwrap()).unwrap();
    let sig = B64URL.decode(parts[2]).unwrap();
    RsaPublicKeyComponents { n: &n, e: &e }
        .verify(&RSA_PKCS1_2048_8192_SHA256, format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
        .expect("valid signature");
    serde_json::from_slice(&B64URL.decode(parts[1]).unwrap()).unwrap()
}

const WF: &str = r#"name: OIDC
on: workflow_dispatch
jobs:
  oidc:
    runs-on: self-hosted
    permissions:
      contents: read
      id-token: write
    steps:
      - run: |
          echo "AUD=$(curl -sf -H "Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" "$ACTIONS_ID_TOKEN_REQUEST_URL&audience=sts.example.com" | sed 's/.*"value":"\([^"]*\)".*/\1/')"
          echo "DEF=$(curl -sf -H "Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" "$ACTIONS_ID_TOKEN_REQUEST_URL" | sed 's/.*"value":"\([^"]*\)".*/\1/')"
          echo "RT=$(printf %s "$ACTIONS_ID_TOKEN_REQUEST_TOKEN" | base64 | tr -d '\n')"
  plain:
    runs-on: self-hosted
    steps:
      - run: echo "URL=[${ACTIONS_ID_TOKEN_REQUEST_URL:-none}] TOKEN=[${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-none}]"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_requests_verifiable_id_tokens() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let _runner = start_runner(&s, &u, &rp).await;
    put_file(&s, &u, &rp, ".github/workflows/oidc.yml", WF).await;

    // Discovery (unauthenticated).
    let issuer = format!("{}/_services/token", s.base);
    let cfg: Value = s.client().get(format!("{issuer}/.well-known/openid-configuration")).send().await.unwrap().json().await.unwrap();
    assert_eq!(cfg["issuer"], json!(issuer));
    assert_eq!(cfg["jwks_uri"], json!(format!("{issuer}/.well-known/jwks")));
    assert_eq!(cfg["id_token_signing_alg_values_supported"], json!(["RS256"]));
    let jwks: Value = s.client().get(format!("{issuer}/.well-known/jwks")).send().await.unwrap().json().await.unwrap();
    assert_eq!(jwks["keys"][0]["kty"], "RSA");

    // Bad request tokens are refused.
    let r = s.client().get(format!("{issuer}/idtoken?api-version=2.0")).bearer_auth("fid_nope").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    let r = s.client().get(format!("{issuer}/idtoken?api-version=2.0")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);

    let logs = dispatch_and_wait(&s, &u, &rp, "oidc.yml").await;
    let oidc = &logs.iter().find(|(n, _)| n == "oidc").unwrap().1;
    let plain = &logs.iter().find(|(n, _)| n == "plain").unwrap().1;
    assert!(plain.contains("URL=[none] TOKEN=[none]"), "{plain}");

    let full = format!("{}/{repo}", u.login);
    let c = verify(&line_value(oidc, "AUD="), &jwks);
    assert_eq!(c["iss"], json!(issuer));
    assert_eq!(c["aud"], "sts.example.com");
    assert_eq!(c["sub"], json!(format!("repo:{full}:ref:refs/heads/main")));
    assert_eq!(c["repository"], json!(full));
    assert_eq!(c["repository_owner"], json!(u.login));
    assert_eq!(c["ref"], "refs/heads/main");
    assert_eq!(c["ref_type"], "branch");
    assert_eq!(c["event_name"], "workflow_dispatch");
    assert_eq!(c["workflow"], "OIDC");
    assert_eq!(c["actor"], json!(u.login));
    assert_eq!(c["runner_environment"], "self-hosted");
    assert_eq!(c["job_workflow_ref"], json!(format!("{full}/.github/workflows/oidc.yml@refs/heads/main")));
    assert_eq!(c["sha"].as_str().unwrap().len(), 40);
    let now = chrono::Utc::now().timestamp();
    assert!(c["exp"].as_i64().unwrap() > c["iat"].as_i64().unwrap() && c["iat"].as_i64().unwrap() <= now + 5);
    let d = verify(&line_value(oidc, "DEF="), &jwks);
    assert_eq!(d["aud"], json!(format!("{}/{}", s.base, u.login)), "default audience = owner URL");
    assert_ne!(c["jti"], d["jti"]);

    // The request token dies with the job.
    let rt = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(line_value(oidc, "RT=")).unwrap()).unwrap();
    assert!(rt.starts_with("fid_"));
    let r = s.client().get(format!("{issuer}/idtoken?api-version=2.0")).bearer_auth(&rt).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);

    // A custom subject template on the repository.
    let (st, v) = s
        .call(Some(&u), "PUT", &format!("{rp}/actions/oidc/customization/sub"), Some(json!({"use_default": false, "include_claim_keys": ["repo", "context", "job_workflow_ref"]})))
        .await;
    assert!(st == 201 || st == 200, "{st} {v}");
    let logs = dispatch_and_wait(&s, &u, &rp, "oidc.yml").await;
    let oidc = &logs.iter().find(|(n, _)| n == "oidc").unwrap().1;
    let c = verify(&line_value(oidc, "AUD="), &jwks);
    assert_eq!(c["sub"], json!(format!("repo:{full}:ref:refs/heads/main:job_workflow_ref:{full}/.github/workflows/oidc.yml@refs/heads/main")));
}
