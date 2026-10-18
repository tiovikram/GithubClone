//! GitHub Actions execution end to end: workflows pushed to a repository run on an in-process
//! `forge runner` (shell executor) registered with a repository registration token.
mod common;

use std::time::Duration;

use base64::Engine;
use common::{TestServer, TestUser};
use serde_json::{json, Value};

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

/// Starts a runner for the repository; aborted when the handle is dropped.
async fn start_runner(s: &TestServer, u: &TestUser, rp: &str, labels: &str) -> tokio::task::JoinHandle<()> {
    let (st, tok) = s.call(Some(u), "POST", &format!("{rp}/actions/runners/registration-token"), None).await;
    assert_eq!(st, 201, "{tok}");
    let work = s.dir.path().join(format!("runner-{}", common::unique("w")));
    let args = forge::runner::agent::RunnerArgs {
        url: Some(s.base.clone()),
        token: Some(tok["token"].as_str().unwrap().to_string()),
        name: Some(common::unique("runner-")),
        labels: labels.to_string(),
        executor: "shell".into(),
        work_dir: Some(work),
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

async fn latest_run(s: &TestServer, u: &TestUser, rp: &str, event: &str) -> Value {
    for _ in 0..100 {
        let (_, runs) = s.call(Some(u), "GET", &format!("{rp}/actions/runs?event={event}"), None).await;
        if let Some(r) = runs["workflow_runs"].get(0) {
            return r.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no {event} run");
}

async fn wait_run(s: &TestServer, u: &TestUser, rp: &str, id: i64, attempt: i64) -> Value {
    for _ in 0..600 {
        let (_, r) = s.call(Some(u), "GET", &format!("{rp}/actions/runs/{id}"), None).await;
        if r["status"] == "completed" && r["run_attempt"] == attempt {
            let (_, jobs) = s.call(Some(u), "GET", &format!("{rp}/actions/runs/{id}/jobs"), None).await;
            if jobs["jobs"].as_array().unwrap().iter().all(|j| j["status"] == "completed") {
                return r;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let (_, jobs) = s.call(Some(u), "GET", &format!("{rp}/actions/runs/{id}/jobs"), None).await;
    panic!("run {id} did not complete: {jobs}");
}

async fn jobs(s: &TestServer, u: &TestUser, rp: &str, id: i64) -> Vec<Value> {
    let (_, j) = s.call(Some(u), "GET", &format!("{rp}/actions/runs/{id}/jobs"), None).await;
    j["jobs"].as_array().unwrap().clone()
}

/// GET that follows redirects (the signed download URLs) and returns the body bytes.
async fn download(s: &TestServer, u: &TestUser, path: &str) -> (u16, Vec<u8>) {
    let res = s.client().get(format!("{}{path}", s.api)).header("Authorization", format!("token {}", u.token)).send().await.unwrap();
    let st = res.status().as_u16();
    (st, res.bytes().await.unwrap().to_vec())
}

async fn job_log(s: &TestServer, u: &TestUser, rp: &str, job: &Value) -> String {
    let (st, b) = download(s, u, &format!("{rp}/actions/jobs/{}/logs", job["id"])).await;
    assert_eq!(st, 200);
    String::from_utf8_lossy(&b).to_string()
}

const CI: &str = r#"name: CI
on:
  push:
    branches: [main]
env:
  WF: wf-env
jobs:
  build:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        n: [1, 2]
    outputs:
      val: ${{ steps.out.outputs.val }}
    steps:
      - uses: actions/checkout@v4
      - name: Show
        run: |
          echo "matrix=${{ matrix.n }} wf=$WF event=${{ github.event_name }} ci=$CI repo=$GITHUB_REPOSITORY"
          cat README.md | head -1
          echo "FROM_ENV=set-${{ matrix.n }}" >> "$GITHUB_ENV"
          echo "$PWD/bin" >> "$GITHUB_PATH"
          mkdir -p bin && printf '#!/bin/sh\necho tool-ran\n' > bin/mytool && chmod +x bin/mytool
      - id: out
        run: echo "val=out-${{ matrix.n }}" >> "$GITHUB_OUTPUT"
      - run: |
          echo "from-env=$FROM_ENV"
          mytool
      - run: mkdir -p dist && echo "artifact ${{ matrix.n }}" > dist/a.txt
      - uses: actions/upload-artifact@v4
        with:
          name: dist-${{ matrix.n }}
          path: dist/
      - uses: actions/cache@v4
        with:
          path: cached
          key: cache-${{ matrix.n }}-${{ hashFiles('README.md') }}
      - run: mkdir -p cached && echo x > cached/f
  report:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - run: echo "needs=${{ needs.build.outputs.val }} result=${{ needs.build.result }}"
      - uses: actions/download-artifact@v4
        with:
          path: got
      - run: cat got/dist-1/a.txt got/dist-2/a.txt
      - run: exit 7
        continue-on-error: true
      - if: failure()
        run: echo never-runs
  pr-only:
    needs: report
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    steps:
      - run: echo never
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_runs_matrix_needs_outputs_artifacts_caches_logs() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let _runner = start_runner(&s, &u, &rp, "").await;
    put_file(&s, &u, &rp, ".github/workflows/ci.yml", CI).await;
    let run = latest_run(&s, &u, &rp, "push").await;
    let id = run["id"].as_i64().unwrap();
    let run = wait_run(&s, &u, &rp, id, 1).await;
    let js = jobs(&s, &u, &rp, id).await;
    assert_eq!(run["conclusion"], "success", "{run} {js:#?}");
    let names: Vec<&str> = js.iter().map(|j| j["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["build (1)", "build (2)", "report", "pr-only"], "jobs in file order, matrix expanded");
    assert_eq!(js[3]["conclusion"], "skipped");
    assert!(js[0]["runner_name"].as_str().unwrap().starts_with("runner-"));
    assert!(js[0]["steps"].as_array().unwrap().len() >= 9, "{}", js[0]["steps"]);
    assert_eq!(js[0]["steps"][0]["name"], "Set up job");

    // Logs: expressions, env, GITHUB_ENV, GITHUB_PATH, outputs, needs, artifacts.
    let b1 = job_log(&s, &u, &rp, &js[0]).await;
    assert!(b1.contains("matrix=1 wf=wf-env event=push ci=true"), "{b1}");
    assert!(b1.contains("from-env=set-1") && b1.contains("tool-ran"), "{b1}");
    assert!(!b1.contains("ghs_"), "job token must be masked: {b1}");
    let rep = job_log(&s, &u, &rp, &js[2]).await;
    assert!(rep.contains("needs=out-") && rep.contains("result=success"), "{rep}");
    assert!(rep.contains("artifact 1") && rep.contains("artifact 2"), "{rep}");
    assert!(rep.contains("exit code 7") && !rep.contains("never-runs"), "{rep}");
    // the continue-on-error step failed but its conclusion is success
    let steps = js[2]["steps"].as_array().unwrap();
    assert!(steps.iter().any(|st| st["name"] == "Run exit 7" && st["conclusion"] == "success"), "{steps:?}");
    assert!(steps.iter().any(|st| st["name"] == "Run echo never-runs" && st["conclusion"] == "skipped"), "{steps:?}");

    // Run logs: a zip with one file per job and per step.
    let (st, zip) = download(&s, &u, &format!("{rp}/actions/runs/{id}/logs")).await;
    assert_eq!(st, 200);
    let files = forge::runner::zip::read_all(&zip).unwrap();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"0_build (1).txt") && names.contains(&"2_report.txt"), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("build (1)/3_Show")), "{names:?}");

    // Artifacts.
    let (_, arts) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/artifacts"), None).await;
    assert_eq!(arts["total_count"], 2, "{arts}");
    let a = arts["artifacts"].as_array().unwrap().iter().find(|a| a["name"] == "dist-1").unwrap().clone();
    let (st, z) = download(&s, &u, &format!("{rp}/actions/artifacts/{}/zip", a["id"])).await;
    assert_eq!(st, 200);
    assert_eq!(forge::runner::zip::read_all(&z).unwrap(), vec![("a.txt".to_string(), b"artifact 1\n".to_vec())]);
    let (_, by_name) = s.call(Some(&u), "GET", &format!("{rp}/actions/artifacts?name=dist-2"), None).await;
    assert_eq!(by_name["total_count"], 1);

    // Caches were saved by the post step; usage reflects them.
    let (_, caches) = s.call(Some(&u), "GET", &format!("{rp}/actions/caches"), None).await;
    assert_eq!(caches["total_count"], 2, "{caches}");
    assert_eq!(caches["actions_caches"][0]["ref"], "refs/heads/main");
    let (_, usage) = s.call(Some(&u), "GET", &format!("{rp}/actions/cache/usage"), None).await;
    assert_eq!(usage["active_caches_count"], 2);

    // Each job is a check run on the head commit, in the run's check suite.
    let (_, cr) = s.call(Some(&u), "GET", &format!("{rp}/check-runs/{}", js[0]["id"]), None).await;
    assert_eq!((cr["name"].clone(), cr["conclusion"].clone()), (json!("build (1)"), json!("success")), "{cr}");
    assert_eq!(cr["check_suite"]["id"], run["check_suite_id"]);

    // A second push restores the caches (exact hit: no post step saves again).
    put_file(&s, &u, &rp, "other.txt", "x").await;
    let (_, runs) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?event=push"), None).await;
    let id2 = runs["workflow_runs"][0]["id"].as_i64().unwrap();
    assert_ne!(id2, id);
    wait_run(&s, &u, &rp, id2, 1).await;
    let js2 = jobs(&s, &u, &rp, id2).await;
    let log = job_log(&s, &u, &rp, &js2[0]).await;
    assert!(log.contains("Cache restored from key: cache-1-"), "{log}");
    assert!(!js2[0]["steps"].as_array().unwrap().iter().any(|st| st["name"].as_str().unwrap().starts_with("Post")), "{}", js2[0]["steps"]);

    // Deleting a run deletes its artifacts.
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/runs/{id}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/actions/artifacts/{}", a["id"]), None).await;
    assert_eq!(st, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failures_reruns_cancel_and_timeouts() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let _runner = start_runner(&s, &u, &rp, "").await;
    let wf = r#"name: Flaky
on: workflow_dispatch
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        k: [ok, bad]
    steps:
      - run: |
          if [ "${{ matrix.k }}" = bad ] && [ "${{ github.run_attempt }}" = 1 ]; then echo boom; exit 1; fi
          echo fine
  after:
    needs: test
    runs-on: ubuntu-latest
    steps:
      - run: echo after
  cleanup:
    needs: test
    if: always()
    runs-on: ubuntu-latest
    steps:
      - run: echo cleanup
"#;
    put_file(&s, &u, &rp, ".github/workflows/flaky.yml", wf).await;
    let (st, v) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/flaky.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 204, "{v}");
    let run = latest_run(&s, &u, &rp, "workflow_dispatch").await;
    let id = run["id"].as_i64().unwrap();
    let run = wait_run(&s, &u, &rp, id, 1).await;
    assert_eq!(run["conclusion"], "failure");
    let js = jobs(&s, &u, &rp, id).await;
    let concl = |js: &[Value], n: &str| js.iter().find(|j| j["name"] == n).map(|j| j["conclusion"].clone()).unwrap_or(Value::Null);
    assert_eq!(concl(&js, "test (bad)"), "failure");
    assert_eq!(concl(&js, "test (ok)"), "success");
    assert_eq!(concl(&js, "after"), "skipped");
    assert_eq!(concl(&js, "cleanup"), "success", "if: always() runs after a failed dependency");
    let bad = js.iter().find(|j| j["name"] == "test (bad)").unwrap();
    assert!(job_log(&s, &u, &rp, bad).await.contains("boom"));

    // Re-run failed jobs: only the failed matrix instance and its dependents run again.
    let ok_id = js.iter().find(|j| j["name"] == "test (ok)").unwrap()["id"].clone();
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{id}/rerun-failed-jobs"), Some(json!({}))).await;
    assert_eq!(st, 201);
    let run = wait_run(&s, &u, &rp, id, 2).await;
    assert_eq!(run["conclusion"], "success", "{}", json!(jobs(&s, &u, &rp, id).await));
    let js2 = jobs(&s, &u, &rp, id).await;
    assert_eq!(concl(&js2, "after"), "success");
    let ok2 = js2.iter().find(|j| j["name"] == "test (ok)").unwrap();
    assert_eq!(ok2["run_attempt"], 2);
    assert_ne!(ok2["id"], ok_id);
    let (_, all) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/jobs?filter=all"), None).await;
    assert_eq!(all["total_count"], 8);
    let (_, a1) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/attempts/1"), None).await;
    assert_eq!(a1["conclusion"], "failure");
    let (_, a1j) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/attempts/1/jobs"), None).await;
    assert_eq!(a1j["total_count"], 4);
    let (st, _) = download(&s, &u, &format!("{rp}/actions/runs/{id}/attempts/1/logs")).await;
    assert_eq!(st, 200);
    // Re-run a single job.
    let after = js2.iter().find(|j| j["name"] == "after").unwrap();
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/jobs/{}/rerun", after["id"]), Some(json!({}))).await;
    assert_eq!(st, 201);
    wait_run(&s, &u, &rp, id, 3).await;

    // Cancel a running job; timeouts.
    let slow = r#"name: Slow
on: workflow_dispatch
jobs:
  sleepy:
    runs-on: ubuntu-latest
    steps:
      - run: echo started && sleep 120
  timeout:
    runs-on: ubuntu-latest
    timeout-minutes: 0.05
    steps:
      - run: sleep 60
"#;
    put_file(&s, &u, &rp, ".github/workflows/slow.yml", slow).await;
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/slow.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 204);
    let (_, runs) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?event=workflow_dispatch"), None).await;
    let sid = runs["workflow_runs"][0]["id"].as_i64().unwrap();
    // wait until the sleepy job is running and the other one timed out
    for _ in 0..400 {
        let js = jobs(&s, &u, &rp, sid).await;
        if js.iter().any(|j| j["name"] == "sleepy" && j["status"] == "in_progress") && js.iter().any(|j| j["name"] == "timeout" && j["status"] == "completed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let t0 = std::time::Instant::now();
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{sid}/cancel"), None).await;
    assert_eq!(st, 202);
    let run = wait_run(&s, &u, &rp, sid, 1).await;
    assert!(t0.elapsed() < Duration::from_secs(60), "cancel stops the running step");
    assert_eq!(run["conclusion"], "cancelled");
    let js = jobs(&s, &u, &rp, sid).await;
    assert_eq!(concl(&js, "sleepy"), "cancelled");
    assert_eq!(concl(&js, "timeout"), "failure");
    let t = js.iter().find(|j| j["name"] == "timeout").unwrap();
    assert!(job_log(&s, &u, &rp, t).await.contains("maximum execution time"));
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{sid}/cancel"), None).await;
    assert_eq!(st, 409);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_inputs_composite_actions_job_token_scope() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    // A composite action in another (public) repository on this forge.
    let action_repo = s.create_repo(&u, true).await;
    let arp = format!("/repos/{}/{action_repo}", u.login);
    put_file(
        &s,
        &u,
        &arp,
        "action.yml",
        "name: Greet\ninputs:\n  who:\n    default: world\noutputs:\n  said:\n    value: ${{ steps.g.outputs.said }}\nruns:\n  using: composite\n  steps:\n    - id: g\n      shell: bash\n      run: echo \"hello ${{ inputs.who }}\" && echo \"said=hello ${{ inputs.who }}\" >> $GITHUB_OUTPUT\n",
    )
    .await;
    let (_, tags) = s.call(Some(&u), "GET", &format!("{arp}/commits/main"), None).await;
    let (st, _) = s.call(Some(&u), "POST", &format!("{arp}/git/refs"), Some(json!({"ref": "refs/tags/v1", "sha": tags["sha"]}))).await;
    assert_eq!(st, 201);
    // A private repository the job token must not see.
    let (st, _) = s.call(Some(&u), "POST", "/user/repos", Some(json!({"name": format!("{repo}-secret"), "private": true}))).await;
    assert_eq!(st, 201);
    let _runner = start_runner(&s, &u, &rp, "gpu").await;
    let wf = format!(
        r#"name: Manual
on:
  workflow_dispatch:
    inputs:
      who:
        required: true
      loud:
        type: boolean
        default: false
jobs:
  go:
    runs-on: [self-hosted, gpu]
    defaults:
      run:
        shell: bash
    steps:
      - id: greet
        uses: {owner}/{action_repo}@v1
        with:
          who: ${{{{ inputs.who }}}}
      - run: echo "composite said '${{{{ steps.greet.outputs.said }}}}' loud=${{{{ inputs.loud }}}}"
      - if: inputs.loud
        run: echo LOUD
      - name: token scope
        run: |
          own=$(curl -s -o /dev/null -w '%{{http_code}}' -H "Authorization: token ${{{{ github.token }}}}" "$GITHUB_API_URL/repos/$GITHUB_REPOSITORY")
          other=$(curl -s -o /dev/null -w '%{{http_code}}' -H "Authorization: token ${{{{ secrets.GITHUB_TOKEN }}}}" "$GITHUB_API_URL/repos/{owner}/{repo}-secret")
          echo "own=$own other=$other"
      - uses: someone/not-here@v9
"#,
        owner = u.login
    );
    put_file(&s, &u, &rp, ".github/workflows/manual.yml", &wf).await;
    // Required input missing -> 422; unknown input -> 422.
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/manual.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/manual.yml/dispatches"), Some(json!({"ref": "main", "inputs": {"who": "x", "nope": 1}}))).await;
    assert_eq!(st, 422);
    let (st, v) = s
        .call(Some(&u), "POST", &format!("{rp}/actions/workflows/manual.yml/dispatches"), Some(json!({"ref": "main", "inputs": {"who": "forge", "loud": "true"}, "return_run_details": true})))
        .await;
    assert_eq!(st, 200, "{v}");
    let id = v["workflow_run_id"].as_i64().unwrap();
    let run = wait_run(&s, &u, &rp, id, 1).await;
    assert_eq!(run["conclusion"], "failure", "the unknown action fails the job");
    let js = jobs(&s, &u, &rp, id).await;
    assert_eq!(js[0]["labels"], json!(["self-hosted", "gpu"]));
    let log = job_log(&s, &u, &rp, &js[0]).await;
    assert!(log.contains("hello forge"), "{log}");
    assert!(log.contains("composite said 'hello forge' loud=true"), "{log}");
    assert!(log.contains("LOUD"), "{log}");
    assert!(log.contains("own=200 other=404"), "{log}");
    assert!(log.contains("does not download actions from github.com"), "{log}");
    // The job token died with the job.
    let token = log.lines().find(|l| l.contains("ghs_")).map(str::to_string);
    assert!(token.is_none(), "token masked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runners_api_labels_jit_and_schedules() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let other = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let (st, _) = s.call(Some(&other), "POST", &format!("{rp}/actions/runners/registration-token"), None).await;
    assert_eq!(st, 403);
    let (st, jit) = s.call(Some(&u), "POST", &format!("{rp}/actions/runners/generate-jitconfig"), Some(json!({"name": "jit-1", "runner_group_id": 1, "labels": ["self-hosted", "linux", "big"]}))).await;
    assert_eq!(st, 201, "{jit}");
    let rid = jit["runner"]["id"].as_i64().unwrap();
    assert!(jit["encoded_jit_config"].as_str().unwrap().len() > 20);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runners/generate-jitconfig"), Some(json!({"name": "jit-1", "runner_group_id": 1, "labels": ["x"]}))).await;
    assert_eq!(st, 409);
    let (_, list) = s.call(Some(&u), "GET", &format!("{rp}/actions/runners"), None).await;
    assert_eq!(list["total_count"], 1);
    assert_eq!(list["runners"][0]["status"], "offline");
    let (st, l) = s.call(Some(&u), "POST", &format!("{rp}/actions/runners/{rid}/labels"), Some(json!({"labels": ["fast"]}))).await;
    assert_eq!(st, 200);
    let names: Vec<&str> = l["labels"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"fast") && names.contains(&"big") && names.contains(&"self-hosted"), "{names:?}");
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/runners/{rid}/labels/self-hosted"), None).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/runners/{rid}/labels/nope"), None).await;
    assert_eq!(st, 404);
    let (st, l) = s.call(Some(&u), "PUT", &format!("{rp}/actions/runners/{rid}/labels"), Some(json!({"labels": ["only"]}))).await;
    assert_eq!((st, l["total_count"].clone()), (200, json!(4)));
    let (_, l) = s.call(Some(&u), "DELETE", &format!("{rp}/actions/runners/{rid}/labels"), None).await;
    assert_eq!(l["total_count"], 3);

    // The JIT runner takes one job and is removed.
    let wf = "name: J\non: workflow_dispatch\njobs:\n  a:\n    runs-on: [self-hosted, linux]\n    steps:\n      - run: echo jit-ok\n";
    put_file(&s, &u, &rp, ".github/workflows/j.yml", wf).await;
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/j.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 204);
    let args = forge::runner::agent::RunnerArgs {
        url: Some(s.base.clone()),
        token: None,
        name: None,
        labels: String::new(),
        executor: "shell".into(),
        work_dir: Some(s.dir.path().join("jit")),
        config: None,
        jitconfig: Some(jit["encoded_jit_config"].as_str().unwrap().to_string()),
        ephemeral: false,
        once: true,
        slots: 1,
        remove_token: None,
        keep: false,
    };
    tokio::time::timeout(Duration::from_secs(120), forge::runner::agent::main(args)).await.expect("jit runner finishes").unwrap();
    let run = latest_run(&s, &u, &rp, "workflow_dispatch").await;
    let run = wait_run(&s, &u, &rp, run["id"].as_i64().unwrap(), 1).await;
    assert_eq!(run["conclusion"], "success");
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/actions/runners/{rid}"), None).await;
    assert_eq!(st, 404, "ephemeral runner removed after its job");

    // Remove token / downloads.
    let (st, rt) = s.call(Some(&u), "POST", &format!("{rp}/actions/runners/remove-token"), None).await;
    assert_eq!(st, 201);
    assert!(rt["expires_at"].is_string());
    let (_, dl) = s.call(Some(&u), "GET", &format!("{rp}/actions/runners/downloads"), None).await;
    assert!(dl[0]["download_url"].as_str().unwrap().ends_with("/_actions/runner/forge"));

    // Schedules: pushing a cron workflow to the default branch registers it; the scheduler fires
    // it once per matching minute.
    let cron = "name: Nightly\non:\n  schedule:\n    - cron: '*/5 * * * *'\njobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo cron\n";
    put_file(&s, &u, &rp, ".github/workflows/nightly.yml", cron).await;
    let minute = chrono::Utc::now().date_naive().and_hms_opt(0, 5, 0).unwrap().and_utc() + chrono::Duration::days(1);
    let fired = forge::runner::trigger::fire_schedules(&s.state, minute).await.map_err(|e| e.message).unwrap();
    assert!(fired >= 1);
    let again = forge::runner::trigger::fire_schedules(&s.state, minute).await.map_err(|e| e.message).unwrap();
    assert_eq!(again, 0, "a minute fires once");
    let r = latest_run(&s, &u, &rp, "schedule").await;
    assert_eq!(r["name"], "Nightly");
    let not_due = minute + chrono::Duration::minutes(1);
    let (_, before) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?event=schedule"), None).await;
    forge::runner::trigger::fire_schedules(&s.state, not_due).await.map_err(|e| e.message).unwrap();
    let (_, after) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?event=schedule"), None).await;
    assert_eq!(before["total_count"], after["total_count"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enterprise_runner_environment_reviews_and_repository_dispatch() {
    let s = TestServer::start().await;
    // a site administrator, an org and an org repository
    let login = common::unique("adm");
    let adm = forge::forge_api::create_user(&s.state, &login, Some(&format!("{login}@example.com")), "password123", true).await.map_err(|e| e.message).unwrap();
    let (_, token) = forge::forge_api::create_token_for(&s.state, adm.id, "t").await.map_err(|e| e.message).unwrap();
    let u = TestUser { login, token };
    let org = common::unique("org");
    let res = s.client().post(format!("{}/api/forge/orgs", s.base)).header("Authorization", format!("token {}", u.token)).json(&json!({"login": org})).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 201);
    let (_, o) = s.call(Some(&u), "GET", &format!("/orgs/{org}"), None).await;
    let name = common::unique("r");
    let (st, _) = s.call(Some(&u), "POST", &format!("/orgs/{org}/repos"), Some(json!({"name": name, "auto_init": true}))).await;
    assert_eq!(st, 201);
    let rp = format!("/repos/{org}/{name}");
    // environment `prod` requires the admin's review
    let (st, env) = s.call(Some(&u), "PUT", &format!("{rp}/environments/prod"), Some(json!({"reviewers": [{"type": "User", "id": adm.id}]}))).await;
    assert_eq!(st, 200, "{env}");
    let wf = "name: Deploy\non:\n  workflow_dispatch:\n  repository_dispatch:\n    types: [go]\njobs:\n  ship:\n    runs-on: ubuntu-latest\n    environment: prod\n    steps:\n      - run: echo shipped-${{ github.event_name }}\n";
    put_file(&s, &u, &rp, ".github/workflows/deploy.yml", wf).await;
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/deploy.yml/dispatches"), Some(json!({"ref": "main"}))).await;
    assert_eq!(st, 204);
    let run = latest_run(&s, &u, &rp, "workflow_dispatch").await;
    let id = run["id"].as_i64().unwrap();
    assert_eq!(run["status"], "waiting");
    let (_, pend) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/pending_deployments"), None).await;
    assert_eq!(pend[0]["environment"]["name"], "prod", "{pend}");
    assert_eq!(pend[0]["current_user_can_approve"], true);
    assert_eq!(pend[0]["reviewers"][0]["reviewer"]["login"], u.login.as_str());

    // An enterprise runner (JIT) restricted by its runner group to this org takes the job.
    let slug = forge::enterprise::slug();
    let (st, g) = s
        .call(Some(&u), "POST", &format!("/enterprises/{slug}/actions/runner-groups"), Some(json!({"name": common::unique("g"), "visibility": "selected", "selected_organization_ids": [o["id"]], "allows_public_repositories": true})))
        .await;
    assert_eq!(st, 201, "{g}");
    let (st, jit) = s
        .call(Some(&u), "POST", &format!("/enterprises/{slug}/actions/runners/generate-jitconfig"), Some(json!({"name": common::unique("ent-"), "runner_group_id": g["id"], "labels": ["self-hosted"]})))
        .await;
    assert_eq!(st, 201, "{jit}");
    assert_eq!(jit["runner"]["runner_group_id"], g["id"]);
    let (st, er) = s.call(Some(&u), "GET", &format!("/enterprises/{slug}/actions/runners/{}", jit["runner"]["id"]), None).await;
    assert_eq!(st, 200, "enterprise API sees the runner: {er}");

    let (st, deps) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{id}/pending_deployments"), Some(json!({"environment_ids": [env["id"]], "state": "approved", "comment": "ok"}))).await;
    assert_eq!(st, 200, "{deps}");
    assert_eq!(deps[0]["environment"], "prod");
    let (_, appr) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/approvals"), None).await;
    assert_eq!(appr[0]["state"], "approved");
    let args = forge::runner::agent::RunnerArgs {
        url: Some(s.base.clone()),
        token: None,
        name: None,
        labels: String::new(),
        executor: "shell".into(),
        work_dir: Some(s.dir.path().join("ent")),
        config: None,
        jitconfig: Some(jit["encoded_jit_config"].as_str().unwrap().to_string()),
        ephemeral: false,
        once: true,
        slots: 1,
        remove_token: None,
        keep: false,
    };
    tokio::time::timeout(Duration::from_secs(120), forge::runner::agent::main(args)).await.expect("enterprise runner finishes").unwrap();
    let run = wait_run(&s, &u, &rp, id, 1).await;
    assert_eq!(run["conclusion"], "success");
    let js = jobs(&s, &u, &rp, id).await;
    assert!(job_log(&s, &u, &rp, &js[0]).await.contains("shipped-workflow_dispatch"));
    let (_, pend) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{id}/pending_deployments"), None).await;
    assert_eq!(pend, json!([]));
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{id}/pending_deployments"), Some(json!({"environment_ids": [env["id"]], "state": "approved", "comment": "again"}))).await;
    assert_eq!(st, 422, "nothing pending any more");

    // repository_dispatch with a matching type triggers the workflow (and waits for review).
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/dispatches"), Some(json!({"event_type": "go", "client_payload": {"x": 1}}))).await;
    assert_eq!(st, 204);
    let r = latest_run(&s, &u, &rp, "repository_dispatch").await;
    assert_eq!(r["status"], "waiting");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{}/pending_deployments", r["id"]), Some(json!({"environment_ids": [env["id"]], "state": "rejected", "comment": "no"}))).await;
    assert_eq!(st, 200);
    let r = wait_run(&s, &u, &rp, r["id"].as_i64().unwrap(), 1).await;
    assert_eq!(r["conclusion"], "failure");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/dispatches"), Some(json!({"event_type": "other"}))).await;
    assert_eq!(st, 204);
    let (_, runs) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?event=repository_dispatch"), None).await;
    assert_eq!(runs["total_count"], 1, "types filter");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn github_token_permissions_recursion_guard_and_custom_gates() {
    let s = TestServer::start().await;
    let u = s.user().await;
    let repo = s.create_repo(&u, true).await;
    let rp = format!("/repos/{}/{repo}", u.login);
    let _runner = start_runner(&s, &u, &rp, "").await;
    // Default workflow permissions are read-only: pushing with GITHUB_TOKEN fails unless the
    // job asks for `contents: write`. Pushes made by the token never trigger workflows.
    let wf = r#"name: Push
on:
  workflow_dispatch:
  push:
    branches: [from-job-*]
jobs:
  ro:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: git push origin HEAD:refs/heads/from-job-ro && echo pushed || echo push-refused
  rw:
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - run: |
          git -c user.name=ci -c user.email=ci@example.com commit --allow-empty -qm "from job"
          git push origin HEAD:refs/heads/from-job-rw && echo pushed-rw
      - run: |
          code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: token ${{ github.token }}" "$GITHUB_API_URL/repos/$GITHUB_REPOSITORY/issues" -d '{"title":"x"}')
          echo "issue-create=$code"
"#;
    put_file(&s, &u, &rp, ".github/workflows/push.yml", wf).await;
    let (st, v) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/push.yml/dispatches"), Some(json!({"ref": "main", "return_run_details": true}))).await;
    assert_eq!(st, 200, "{v}");
    let id = v["workflow_run_id"].as_i64().unwrap();
    let run = wait_run(&s, &u, &rp, id, 1).await;
    assert_eq!(run["conclusion"], "success", "{}", json!(jobs(&s, &u, &rp, id).await));
    let js = jobs(&s, &u, &rp, id).await;
    let ro = job_log(&s, &u, &rp, js.iter().find(|j| j["name"] == "ro").unwrap()).await;
    assert!(ro.contains("push-refused"), "{ro}");
    let rw = job_log(&s, &u, &rp, js.iter().find(|j| j["name"] == "rw").unwrap()).await;
    assert!(rw.contains("pushed-rw"), "{rw}");
    assert!(rw.contains("issue-create=403"), "only contents was granted: {rw}");
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/branches/from-job-rw"), None).await;
    assert_eq!(st, 200);
    let (st, _) = s.call(Some(&u), "GET", &format!("{rp}/branches/from-job-ro"), None).await;
    assert_eq!(st, 404);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, pushes) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs?event=push&branch=from-job-rw"), None).await;
    assert_eq!(pushes["total_count"], 0, "pushes by GITHUB_TOKEN don't trigger runs");
    // The token died with the job.
    let tok_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM actions_job_tokens WHERE job_id = ANY($1)")
        .bind(js.iter().map(|j| j["id"].as_i64().unwrap()).collect::<Vec<_>>())
        .fetch_one(&s.state.db)
        .await
        .unwrap();
    assert_eq!(tok_rows, 0);

    // Custom deployment protection rule: the job waits until the rule approves.
    let (st, env) = s.call(Some(&u), "PUT", &format!("{rp}/environments/gate"), Some(json!({}))).await;
    assert_eq!(st, 200, "{env}");
    sqlx::query("INSERT INTO environment_protection_rules (environment_id, integration_id) VALUES ($1, 424242)")
        .bind(env["id"].as_i64().unwrap())
        .execute(&s.state.db)
        .await
        .unwrap();
    let gated = "name: Gated\non: workflow_dispatch\njobs:\n  a:\n    runs-on: ubuntu-latest\n    environment: gate\n    steps:\n      - run: echo through-the-gate\n";
    put_file(&s, &u, &rp, ".github/workflows/gated.yml", gated).await;
    let (_, v) = s.call(Some(&u), "POST", &format!("{rp}/actions/workflows/gated.yml/dispatches"), Some(json!({"ref": "main", "return_run_details": true}))).await;
    let gid = v["workflow_run_id"].as_i64().unwrap();
    let (_, r) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{gid}"), None).await;
    assert_eq!(r["status"], "waiting");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{gid}/deployment_protection_rule"), Some(json!({"environment_name": "nope", "state": "approved", "comment": "x"}))).await;
    assert_eq!(st, 422);
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{gid}/deployment_protection_rule"), Some(json!({"environment_name": "gate", "comment": "looking"}))).await;
    assert_eq!(st, 204);
    let (_, r) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{gid}"), None).await;
    assert_eq!(r["status"], "waiting", "a comment does not open the gate");
    let (st, _) = s.call(Some(&u), "POST", &format!("{rp}/actions/runs/{gid}/deployment_protection_rule"), Some(json!({"environment_name": "gate", "state": "approved", "comment": "ok"}))).await;
    assert_eq!(st, 204);
    let r = wait_run(&s, &u, &rp, gid, 1).await;
    assert_eq!(r["conclusion"], "success");
    let (_, appr) = s.call(Some(&u), "GET", &format!("{rp}/actions/runs/{gid}/approvals"), None).await;
    assert_eq!(appr, json!([]), "gate reviews are not environment approvals");
}
