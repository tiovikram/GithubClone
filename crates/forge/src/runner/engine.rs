//! Server-side orchestration of workflow runs: run creation, job planning (needs, `if:`,
//! matrix), status aggregation, cancellation, re-runs and commit statuses.
//!
//! Every state change of a run goes through [`advance`], which locks the run row
//! (`SELECT ... FOR UPDATE`) so concurrent job completions on different pods serialize.
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use super::expr::{self, Ctx, Status};
use super::token;
use super::workflow::{self, Workflow};
use crate::api::actions::RunRow;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct JobRow {
    pub id: i64,
    pub run_id: i64,
    pub repo_id: i64,
    pub run_attempt: i32,
    pub job_key: String,
    pub name: String,
    pub matrix: Option<Value>,
    pub needs: Vec<String>,
    pub definition: Value,
    pub labels: Vec<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub steps: Value,
    pub outputs: Value,
    pub runner_id: Option<i64>,
    pub runner_name: Option<String>,
    pub cancel_requested: bool,
    pub log_key: Option<String>,
    pub log_size: Option<i64>,
    pub step_logs: Option<Value>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub timeout_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// What a new run is for.
pub struct NewRun {
    pub workflow_path: String,
    pub workflow_text: String,
    pub event: String,
    /// `github.event`
    pub payload: Value,
    pub actor: Option<User>,
    pub head_branch: Option<String>,
    /// full ref (`refs/heads/main`, `refs/pull/1/merge`, `refs/tags/v1`)
    pub git_ref: String,
    /// commit the run is reported on (PR head for pull requests)
    pub head_sha: String,
    /// GITHUB_SHA (merge commit for pull requests)
    pub github_sha: String,
    pub inputs: Option<Value>,
    pub pull_requests: Value,
    pub display_title: Option<String>,
}

pub fn workflow_id(repo_id: i64, path: &str) -> i64 {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(format!("{repo_id}:{path}").as_bytes());
    (u64::from_be_bytes(h[..8].try_into().unwrap()) >> 17) as i64
}

async fn simple_commit(s: &AppState, repo_id: i64, sha: &str) -> Option<Value> {
    let out = crate::api::plat::git_read(s, repo_id, &["log", "-1", "--format=%H%x00%T%x00%an%x00%ae%x00%cn%x00%ce%x00%cI%x00%B", sha]).await.ok()??;
    let text = String::from_utf8_lossy(&out);
    let f: Vec<&str> = text.splitn(8, '\0').collect();
    if f.len() < 8 {
        return None;
    }
    let when = DateTime::parse_from_rfc3339(f[6]).map(|t| crate::json::ts(t.with_timezone(&Utc))).unwrap_or_default();
    Some(json!({
        "id": f[0], "tree_id": f[1], "message": f[7].trim_end(), "timestamp": when,
        "author": { "name": f[2], "email": f[3] },
        "committer": { "name": f[4], "email": f[5] },
    }))
}

/// Creates a run (attempt 1) and its initial jobs. An invalid workflow file yields a run that
/// completes immediately with conclusion `failure` (GitHub's startup failure).
pub async fn create_run(s: &AppState, ctx: &RepoCtx, nr: NewRun) -> ApiResult<RunRow> {
    let parsed = workflow::parse(&nr.workflow_text);
    let wf_name = match &parsed {
        Ok(w) => w.name.clone().unwrap_or_else(|| nr.workflow_path.clone()),
        Err(_) => workflow::display_name(&nr.workflow_text, &nr.workflow_path),
    };
    let wid = workflow_id(ctx.repo.id, &nr.workflow_path);
    let head_commit = simple_commit(s, ctx.repo.id, &nr.head_sha).await;
    let display_title = nr
        .display_title
        .clone()
        .or_else(|| head_commit.as_ref().and_then(|c| c["message"].as_str()).map(|m| m.lines().next().unwrap_or("").to_string()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| wf_name.clone());
    let definition = parsed.as_ref().map(|w| w.raw.clone()).ok();
    let actor_id = nr.actor.as_ref().map(|u| u.id);
    let mut run: Option<RunRow> = None;
    for _ in 0..5 {
        run = sqlx::query_as(
            "INSERT INTO actions_runs (repo_id, workflow_id, workflow_path, name, run_number, event, status, head_branch, head_sha,
                 head_commit, inputs, actor_id, triggering_actor_id, git_ref, github_sha, display_title, event_payload, definition,
                 pull_requests)
             SELECT $1, $2, $3, $4, COALESCE(MAX(run_number), 0) + 1, $5, 'queued', $6, $7, $8, $9, $10, $10, $11, $12, $13, $14, $15, $16
               FROM actions_runs WHERE repo_id = $1 AND workflow_id = $2
             ON CONFLICT (repo_id, workflow_id, run_number) DO NOTHING RETURNING *",
        )
        .bind(ctx.repo.id)
        .bind(wid)
        .bind(&nr.workflow_path)
        .bind(&wf_name)
        .bind(&nr.event)
        .bind(&nr.head_branch)
        .bind(&nr.head_sha)
        .bind(&head_commit)
        .bind(&nr.inputs)
        .bind(actor_id)
        .bind(&nr.git_ref)
        .bind(&nr.github_sha)
        .bind(&display_title)
        .bind(&nr.payload)
        .bind(&definition)
        .bind(&nr.pull_requests)
        .fetch_optional(&s.db)
        .await?;
        if run.is_some() {
            break;
        }
    }
    let run = run.ok_or_else(|| ApiError::conflict("Could not allocate a run number, try again"))?;
    sqlx::query("INSERT INTO actions_run_attempts (run_id, attempt, status, triggering_actor_id) VALUES ($1, 1, 'queued', $2)")
        .bind(run.id)
        .bind(actor_id)
        .execute(&s.db)
        .await?;
    match parsed {
        Ok(w) => {
            create_attempt_jobs(s, &run, 1, &w, None).await?;
            advance(s, run.id).await?;
        }
        Err(e) => {
            tracing::info!(repo = ctx.repo.id, path = %nr.workflow_path, error = %e, "invalid workflow");
            sqlx::query("UPDATE actions_runs SET status = 'completed', conclusion = 'failure', completed_at = now(), updated_at = now() WHERE id = $1")
                .bind(run.id)
                .execute(&s.db)
                .await?;
            sqlx::query("UPDATE actions_run_attempts SET status = 'completed', conclusion = 'failure', updated_at = now() WHERE run_id = $1")
                .bind(run.id)
                .execute(&s.db)
                .await?;
        }
    }
    let run = sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(run.id).fetch_one(&s.db).await?;
    Ok(run)
}

/// Inserts the jobs of a new attempt: one `pending` placeholder per workflow job (planned by
/// `advance`). `carry` maps job keys to completed rows of the previous attempt that are kept.
async fn create_attempt_jobs(s: &AppState, run: &RunRow, attempt: i32, w: &Workflow, carry: Option<&HashMap<String, Vec<JobRow>>>) -> ApiResult<()> {
    for (key, def) in &w.jobs {
        if let Some(rows) = carry.and_then(|c| c.get(key)) {
            for r in rows {
                sqlx::query(
                    "INSERT INTO actions_jobs (run_id, repo_id, run_attempt, job_key, name, matrix, needs, definition, labels, status, conclusion,
                         steps, outputs, runner_id, runner_name, log_key, log_size, step_logs, created_at, started_at, completed_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'completed', $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)",
                )
                .bind(run.id)
                .bind(run.repo_id)
                .bind(attempt)
                .bind(key)
                .bind(&r.name)
                .bind(&r.matrix)
                .bind(&r.needs)
                .bind(&r.definition)
                .bind(&r.labels)
                .bind(&r.conclusion)
                .bind(&r.steps)
                .bind(&r.outputs)
                .bind(r.runner_id)
                .bind(&r.runner_name)
                .bind(&r.log_key)
                .bind(r.log_size)
                .bind(&r.step_logs)
                .bind(r.created_at)
                .bind(r.started_at)
                .bind(r.completed_at)
                .execute(&s.db)
                .await?;
            }
            continue;
        }
        let name = def.get("name").map(expr::to_string).filter(|n| !n.contains("${{")).unwrap_or_else(|| key.clone());
        sqlx::query(
            "INSERT INTO actions_jobs (run_id, repo_id, run_attempt, job_key, name, needs, definition, labels, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending')",
        )
        .bind(run.id)
        .bind(run.repo_id)
        .bind(attempt)
        .bind(key)
        .bind(&name)
        .bind(workflow::needs_of(def))
        .bind(def)
        .bind(workflow::runs_on_labels(def.get("runs-on").unwrap_or(&Value::Null)))
        .execute(&s.db)
        .await?;
    }
    Ok(())
}

/// Result of a needed job (aggregated over its matrix instances).
fn aggregate_result(rows: &[&JobRow]) -> &'static str {
    let c: Vec<&str> = rows.iter().map(|r| r.conclusion.as_deref().unwrap_or("")).collect();
    if c.iter().any(|x| matches!(*x, "failure" | "timed_out" | "startup_failure")) {
        "failure"
    } else if c.iter().any(|x| *x == "cancelled") {
        "cancelled"
    } else if !c.is_empty() && c.iter().all(|x| *x == "skipped") {
        "skipped"
    } else {
        "success"
    }
}

/// `needs` context for a job: `{key: {result, outputs}}`.
pub fn needs_context(needs: &[String], jobs: &[JobRow]) -> Value {
    let mut m = Map::new();
    for n in needs {
        let rows: Vec<&JobRow> = jobs.iter().filter(|j| &j.job_key == n).collect();
        let mut outputs = Map::new();
        for r in &rows {
            if let Some(o) = r.outputs.as_object() {
                for (k, v) in o {
                    if !v.as_str().unwrap_or("").is_empty() || !outputs.contains_key(k) {
                        outputs.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        m.insert(n.clone(), json!({ "result": aggregate_result(&rows), "outputs": outputs }));
    }
    Value::Object(m)
}

/// Repository + organization variables (`vars` context); repository values win.
pub async fn vars_context(s: &AppState, ctx: &RepoCtx, environment: Option<&str>) -> ApiResult<Value> {
    super::settings::vars_for_job(s, ctx, environment).await
}

/// The `github` context of a run (the runner adds workspace, token and file paths).
pub fn github_context(s: &AppState, ctx: &RepoCtx, run: &RunRow, actor: Option<&User>, triggering: Option<&User>, job_key: &str) -> Value {
    let base = s.base_url();
    let full = ctx.full_name();
    let git_ref = run.git_ref.clone().unwrap_or_else(|| run.head_branch.as_ref().map(|b| format!("refs/heads/{b}")).unwrap_or_default());
    let (ref_type, ref_name) = if let Some(t) = git_ref.strip_prefix("refs/tags/") {
        ("tag", t.to_string())
    } else if let Some(b) = git_ref.strip_prefix("refs/heads/") {
        ("branch", b.to_string())
    } else {
        ("branch", git_ref.trim_start_matches("refs/pull/").replace("/merge", "/merge"))
    };
    let payload = run.event_payload.clone().unwrap_or_else(|| json!({}));
    let is_pr = run.event.starts_with("pull_request");
    let head_ref = if is_pr { payload.pointer("/pull_request/head/ref").and_then(Value::as_str).unwrap_or("").to_string() } else { String::new() };
    let base_ref = if is_pr { payload.pointer("/pull_request/base/ref").and_then(Value::as_str).unwrap_or("").to_string() } else { String::new() };
    let sha = run.github_sha.clone().unwrap_or_else(|| run.head_sha.clone());
    let login = |u: Option<&User>| u.map(|u| u.login.clone()).unwrap_or_default();
    let host = crate::json::repo::host_of(base);
    json!({
        "action": "",
        "action_path": "",
        "action_ref": "",
        "action_repository": "",
        "actor": login(actor),
        "actor_id": actor.map(|u| u.id.to_string()).unwrap_or_default(),
        "api_url": s.api_url(),
        "base_ref": base_ref,
        "event": payload,
        "event_name": run.event,
        "graphql_url": format!("{base}/api/graphql"),
        "head_ref": head_ref,
        "job": job_key,
        "ref": git_ref,
        "ref_name": ref_name,
        "ref_protected": false,
        "ref_type": ref_type,
        "repository": full,
        "repository_id": ctx.repo.id.to_string(),
        "repository_owner": ctx.owner.login,
        "repository_owner_id": ctx.owner.id.to_string(),
        "repositoryUrl": format!("git://{host}/{full}.git"),
        "retention_days": "90",
        "run_id": run.id.to_string(),
        "run_number": run.run_number.to_string(),
        "run_attempt": run.run_attempt.to_string(),
        "secret_source": "Actions",
        "server_url": base,
        "sha": sha,
        "triggering_actor": login(triggering.or(actor)),
        "workflow": run.name,
        "workflow_ref": format!("{full}/{}@{}", run.workflow_path, run.git_ref.clone().unwrap_or_default()),
        "workflow_sha": sha,
    })
}

/// Workflow-dispatch `inputs` context: given values (strings) merged with declared defaults,
/// booleans and numbers typed as declared.
pub fn inputs_context(run: &RunRow) -> Value {
    let declared = run
        .definition
        .as_ref()
        .and_then(|d| d.pointer("/on/workflow_dispatch/inputs").or_else(|| d.pointer("/true/workflow_dispatch/inputs")))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let given = run.inputs.clone().and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut out = Map::new();
    for (k, spec) in &declared {
        let v = given.get(k).cloned().or_else(|| spec.get("default").cloned()).unwrap_or(Value::Null);
        let typ = spec.get("type").and_then(Value::as_str).unwrap_or("string");
        let typed = match (typ, &v) {
            ("boolean", Value::String(s)) => Value::Bool(s.eq_ignore_ascii_case("true")),
            ("number", Value::String(s)) => s.parse::<f64>().map(|f| json!(f)).unwrap_or(v.clone()),
            ("boolean" | "number", other) => other.clone(),
            (_, Value::Null) => Value::String(String::new()),
            (_, other) => Value::String(expr::to_string(other)),
        };
        out.insert(k.clone(), typed);
    }
    for (k, v) in given {
        out.entry(k).or_insert(v);
    }
    Value::Object(out)
}

async fn run_ctx(s: &AppState, repo_id: i64) -> ApiResult<RepoCtx> {
    let repo = db::repo_by_id(&s.db, repo_id).await?;
    let owner = db::user_by_id(&s.db, repo.owner_id).await?;
    Ok(RepoCtx { repo, owner, perm: Perm::Admin })
}

async fn opt_user(s: &AppState, id: Option<i64>) -> Option<User> {
    match id {
        Some(i) => db::user_by_id(&s.db, i).await.ok(),
        None => None,
    }
}

/// Plans pending jobs whose needs are complete, evaluates their `if:`, and recomputes the run
/// status. Idempotent; safe to call any time.
pub async fn advance(s: &AppState, run_id: i64) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    let Some(run) = sqlx::query_as::<_, RunRow>("SELECT * FROM actions_runs WHERE id = $1 FOR UPDATE").bind(run_id).fetch_optional(&mut *tx).await? else {
        return Ok(());
    };
    let ctx = run_ctx(s, run.repo_id).await?;
    let mut changed_jobs: Vec<i64> = Vec::new();
    let mut cache: Option<(Value, Value, Value)> = None; // (github, inputs, vars)
    loop {
        let jobs: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2 ORDER BY id")
            .bind(run.id)
            .bind(run.run_attempt)
            .fetch_all(&mut *tx)
            .await?;
        let run_cancelled = run.status == "completed" && run.conclusion.as_deref() == Some("cancelled");
        let mut progressed = false;
        for j in jobs.iter().filter(|j| j.status == "pending") {
            let ready = j.needs.iter().all(|n| {
                let rows: Vec<&JobRow> = jobs.iter().filter(|x| &x.job_key == n).collect();
                !rows.is_empty() && rows.iter().all(|x| x.status == "completed")
            });
            if !ready {
                continue;
            }
            if cache.is_none() {
                let actor = opt_user(s, run.actor_id).await;
                let trig = opt_user(s, run.triggering_actor_id).await;
                cache = Some((
                    github_context(s, &ctx, &run, actor.as_ref(), trig.as_ref(), &j.job_key),
                    inputs_context(&run),
                    vars_context(s, &ctx, None).await?,
                ));
            }
            let (gh, inputs, vars) = cache.clone().unwrap();
            let mut gh = gh;
            gh["job"] = json!(j.job_key);
            let needs = needs_context(&j.needs, &jobs);
            let contexts = json!({ "github": gh, "inputs": inputs, "vars": vars, "needs": needs });
            let results: Vec<String> = needs.as_object().map(|m| m.values().map(|v| v["result"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
            let status = if run_cancelled || j.cancel_requested {
                Status::Cancelled
            } else if results.iter().any(|r| r != "success") {
                Status::Failure
            } else {
                Status::Success
            };
            let ectx = Ctx { contexts: &contexts, status, workspace: None };
            // GitHub: a dependency that failed or was skipped skips the job unless its `if:` says otherwise.
            let cond = j.definition.get("if");
            let run_it = match expr::eval_if(&ectx, cond) {
                Ok(b) => b && !(status == Status::Failure && cond.is_none()),
                Err(e) => {
                    fail_job_startup(&mut tx, j, &format!("Error evaluating 'if' of job '{}': {e}", j.job_key)).await?;
                    changed_jobs.push(j.id);
                    progressed = true;
                    continue;
                }
            };
            // skipped when a dependency was skipped and the condition is the default
            let run_it = run_it && !(cond.is_none() && results.iter().any(|r| r == "skipped"));
            if !run_it {
                sqlx::query("UPDATE actions_jobs SET status = 'completed', conclusion = 'skipped', started_at = now(), completed_at = now() WHERE id = $1")
                    .bind(j.id)
                    .execute(&mut *tx)
                    .await?;
                changed_jobs.push(j.id);
                progressed = true;
                continue;
            }
            match workflow::plan_job(&j.job_key, &j.definition, &contexts) {
                Ok(planned) => {
                    for (i, p) in planned.iter().enumerate() {
                        if i == 0 {
                            sqlx::query("UPDATE actions_jobs SET name = $2, matrix = $3, labels = $4, status = 'queued' WHERE id = $1")
                                .bind(j.id)
                                .bind(&p.name)
                                .bind(&p.matrix)
                                .bind(&p.labels)
                                .execute(&mut *tx)
                                .await?;
                            changed_jobs.push(j.id);
                        } else {
                            let id: i64 = sqlx::query_scalar(
                                "INSERT INTO actions_jobs (run_id, repo_id, run_attempt, job_key, name, matrix, needs, definition, labels, status)
                                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'queued') RETURNING id",
                            )
                            .bind(j.run_id)
                            .bind(j.repo_id)
                            .bind(j.run_attempt)
                            .bind(&j.job_key)
                            .bind(&p.name)
                            .bind(&p.matrix)
                            .bind(&j.needs)
                            .bind(&j.definition)
                            .bind(&p.labels)
                            .fetch_one(&mut *tx)
                            .await?;
                            changed_jobs.push(id);
                        }
                    }
                }
                Err(e) => {
                    fail_job_startup(&mut tx, j, &format!("Error planning job '{}': {e}", j.job_key)).await?;
                    changed_jobs.push(j.id);
                }
            }
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    recompute_run(&mut tx, &run).await?;
    tx.commit().await?;
    // Commit statuses outside the run lock.
    for id in changed_jobs {
        if let Ok(job) = sqlx::query_as::<_, JobRow>("SELECT * FROM actions_jobs WHERE id = $1").bind(id).fetch_one(&s.db).await {
            post_status(s, &ctx, &run, &job).await;
        }
    }
    Ok(())
}

async fn fail_job_startup(tx: &mut sqlx::PgConnection, j: &JobRow, msg: &str) -> ApiResult<()> {
    let steps = json!([{ "name": "Set up job", "status": "completed", "conclusion": "failure", "number": 1,
        "started_at": crate::json::ts(Utc::now()), "completed_at": crate::json::ts(Utc::now()) }]);
    sqlx::query("UPDATE actions_jobs SET status = 'completed', conclusion = 'failure', steps = $2, started_at = now(), completed_at = now() WHERE id = $1")
        .bind(j.id)
        .bind(&steps)
        .execute(&mut *tx)
        .await?;
    tracing::info!(job = j.id, "{msg}");
    Ok(())
}

/// Derives the run (and attempt) status from its jobs.
async fn recompute_run(tx: &mut sqlx::PgConnection, run: &RunRow) -> ApiResult<()> {
    let jobs: Vec<(String, Option<String>)> = sqlx::query_as("SELECT status, conclusion FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2")
        .bind(run.id)
        .bind(run.run_attempt)
        .fetch_all(&mut *tx)
        .await?;
    if jobs.is_empty() {
        return Ok(());
    }
    let all_done = jobs.iter().all(|(st, _)| st == "completed");
    let started = jobs.iter().any(|(st, c)| st == "in_progress" || (st == "completed" && c.as_deref() != Some("skipped")));
    let (status, conclusion): (&str, Option<&str>) = if all_done {
        let c: Vec<&str> = jobs.iter().map(|(_, c)| c.as_deref().unwrap_or("")).collect();
        let concl = if run.conclusion.as_deref() == Some("cancelled") || c.contains(&"cancelled") {
            "cancelled"
        } else if c.iter().any(|x| matches!(*x, "failure" | "timed_out" | "startup_failure")) {
            "failure"
        } else if c.iter().all(|x| *x == "skipped") {
            "skipped"
        } else {
            "success"
        };
        ("completed", Some(concl))
    } else if started {
        ("in_progress", None)
    } else {
        ("queued", None)
    };
    if run.status == "completed" && run.conclusion.as_deref() == Some("cancelled") && !all_done {
        // cancellation in progress: keep the run cancelled-pending until jobs stop
        return Ok(());
    }
    sqlx::query(
        "UPDATE actions_runs SET status = $2, conclusion = $3, updated_at = now(),
             completed_at = CASE WHEN $2 = 'completed' THEN COALESCE(completed_at, now()) ELSE NULL END
         WHERE id = $1 AND (status IS DISTINCT FROM $2 OR conclusion IS DISTINCT FROM $3)",
    )
    .bind(run.id)
    .bind(status)
    .bind(conclusion)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE actions_run_attempts SET status = $3, conclusion = $4, updated_at = now() WHERE run_id = $1 AND attempt = $2")
        .bind(run.id)
        .bind(run.run_attempt)
        .bind(status)
        .bind(conclusion)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// Commit status for a job: `<workflow> / <job> (<event>)`, like GitHub's check names.
pub async fn post_status(s: &AppState, ctx: &RepoCtx, run: &RunRow, job: &JobRow) {
    let (state, desc) = match (job.status.as_str(), job.conclusion.as_deref()) {
        ("completed", Some("success")) => {
            let secs = match (job.started_at, job.completed_at) {
                (Some(a), Some(b)) => (b - a).num_seconds().max(0),
                _ => 0,
            };
            ("success", format!("Successful in {secs}s"))
        }
        ("completed", Some("skipped")) => ("success", "Skipped".to_string()),
        ("completed", Some("cancelled")) => ("error", "Cancelled".to_string()),
        ("completed", _) => ("failure", "Failing".to_string()),
        ("in_progress", _) => ("pending", "In progress".to_string()),
        _ => ("pending", "Queued".to_string()),
    };
    let Ok(bot) = token::bot_user(&s.db).await else { return };
    let context = format!("{} / {} ({})", run.name, job.name, run.event);
    let target = format!("{}/{}/actions/runs/{}/job/{}", s.base_url(), ctx.full_name(), run.id, job.id);
    let _ = sqlx::query(
        "INSERT INTO commit_statuses (repo_id, sha, state, context, description, target_url, creator_id) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(ctx.repo.id)
    .bind(&run.head_sha)
    .bind(state)
    .bind(&context)
    .bind(&desc)
    .bind(&target)
    .bind(bot.id)
    .execute(&s.db)
    .await;
}

/// Cancels a run: unstarted jobs complete as cancelled, running jobs are asked to stop (the
/// runner kills them). `force` also completes running jobs immediately.
pub async fn cancel(s: &AppState, run: &RunRow, force: bool) -> ApiResult<()> {
    let n = sqlx::query("UPDATE actions_runs SET status = 'completed', conclusion = 'cancelled', updated_at = now(), completed_at = now() WHERE id = $1 AND (status <> 'completed' OR $2)")
        .bind(run.id)
        .bind(force && run.conclusion.as_deref() == Some("cancelled"))
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        // A cancelled run with jobs still stopping can be force-cancelled.
        let active: i64 = sqlx::query_scalar("SELECT count(*) FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2 AND status <> 'completed'")
            .bind(run.id)
            .bind(run.run_attempt)
            .fetch_one(&s.db)
            .await?;
        if !(force && active > 0) {
            return Err(ApiError::conflict("Cannot cancel a workflow run that is completed."));
        }
    }
    sqlx::query(
        "UPDATE actions_jobs SET status = 'completed', conclusion = 'cancelled', completed_at = now(), started_at = COALESCE(started_at, now())
          WHERE run_id = $1 AND run_attempt = $2 AND (status IN ('pending', 'queued') OR ($3 AND status = 'in_progress'))",
    )
    .bind(run.id)
    .bind(run.run_attempt)
    .bind(force)
    .execute(&s.db)
    .await?;
    sqlx::query("UPDATE actions_jobs SET cancel_requested = TRUE WHERE run_id = $1 AND run_attempt = $2 AND status = 'in_progress'")
        .bind(run.id)
        .bind(run.run_attempt)
        .execute(&s.db)
        .await?;
    if force {
        sqlx::query("DELETE FROM actions_job_tokens WHERE job_id IN (SELECT id FROM actions_jobs WHERE run_id = $1)").bind(run.id).execute(&s.db).await?;
    }
    sqlx::query("UPDATE actions_run_attempts SET status = 'completed', conclusion = 'cancelled', updated_at = now() WHERE run_id = $1 AND attempt = $2")
        .bind(run.id)
        .bind(run.run_attempt)
        .execute(&s.db)
        .await?;
    let ctx = run_ctx(s, run.repo_id).await?;
    let jobs: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2 AND conclusion = 'cancelled'")
        .bind(run.id)
        .bind(run.run_attempt)
        .fetch_all(&s.db)
        .await?;
    for j in &jobs {
        post_status(s, &ctx, run, j).await;
    }
    advance(s, run.id).await
}

/// Which jobs a re-run repeats.
pub enum Rerun {
    All,
    Failed,
    Job(i64),
}

/// Starts the next attempt of a completed run.
pub async fn rerun(s: &AppState, run: &RunRow, actor: &User, mode: Rerun) -> ApiResult<RunRow> {
    if run.status != "completed" {
        return Err(ApiError::forbidden(format!("This workflow run is not completed and cannot be re-run: {}", run.id)));
    }
    let Some(def) = &run.definition else {
        return Err(ApiError::forbidden("This workflow run cannot be re-run: the workflow file is invalid."));
    };
    let w = workflow::from_value(def.clone()).map_err(|e| ApiError::unprocessable(format!("Invalid workflow: {e}")))?;
    let prev: Vec<JobRow> = sqlx::query_as("SELECT * FROM actions_jobs WHERE run_id = $1 AND run_attempt = $2 ORDER BY id")
        .bind(run.id)
        .bind(run.run_attempt)
        .fetch_all(&s.db)
        .await?;
    // Keys to run again (plus everything downstream of them).
    let mut again: Vec<String> = match &mode {
        Rerun::All => w.jobs.iter().map(|(k, _)| k.clone()).collect(),
        Rerun::Failed => {
            let failed: Vec<String> = prev
                .iter()
                .filter(|j| !matches!(j.conclusion.as_deref(), Some("success" | "skipped" | "neutral")))
                .map(|j| j.job_key.clone())
                .collect();
            if failed.is_empty() {
                return Err(ApiError::forbidden("This workflow run has no failed jobs to re-run."));
            }
            failed
        }
        Rerun::Job(id) => {
            let j = prev.iter().find(|j| j.id == *id).ok_or_else(ApiError::not_found)?;
            vec![j.job_key.clone()]
        }
    };
    loop {
        let before = again.len();
        for (k, def) in &w.jobs {
            if !again.contains(k) && workflow::needs_of(def).iter().any(|n| again.contains(n)) {
                again.push(k.clone());
            }
        }
        if again.len() == before {
            break;
        }
    }
    let mut carry: HashMap<String, Vec<JobRow>> = HashMap::new();
    for j in &prev {
        if !again.contains(&j.job_key) {
            carry.entry(j.job_key.clone()).or_default().push(j.clone());
        }
    }
    let attempt = run.run_attempt + 1;
    let updated: RunRow = sqlx::query_as(
        "UPDATE actions_runs SET status = 'queued', conclusion = NULL, run_attempt = $2, triggering_actor_id = $3,
             run_started_at = now(), updated_at = now(), completed_at = NULL WHERE id = $1 AND run_attempt = $4 RETURNING *",
    )
    .bind(run.id)
    .bind(attempt)
    .bind(actor.id)
    .bind(run.run_attempt)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError::conflict("The run was re-run concurrently"))?;
    sqlx::query("INSERT INTO actions_run_attempts (run_id, attempt, status, triggering_actor_id) VALUES ($1, $2, 'queued', $3) ON CONFLICT DO NOTHING")
        .bind(run.id)
        .bind(attempt)
        .bind(actor.id)
        .execute(&s.db)
        .await?;
    create_attempt_jobs(s, &updated, attempt, &w, Some(&carry)).await?;
    advance(s, run.id).await?;
    Ok(sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(run.id).fetch_one(&s.db).await?)
}

/// Marks a job completed and moves the run forward.
pub async fn finish_job(s: &AppState, job_id: i64, conclusion: &str, outputs: Option<Value>, steps: Option<Value>) -> ApiResult<()> {
    let job: Option<JobRow> = sqlx::query_as(
        "UPDATE actions_jobs SET status = 'completed', conclusion = $2, outputs = COALESCE($3, outputs), steps = COALESCE($4, steps),
             completed_at = now() WHERE id = $1 AND status <> 'completed' RETURNING *",
    )
    .bind(job_id)
    .bind(conclusion)
    .bind(&outputs)
    .bind(&steps)
    .fetch_optional(&s.db)
    .await?;
    sqlx::query("DELETE FROM actions_job_tokens WHERE job_id = $1").bind(job_id).execute(&s.db).await?;
    let job = match job {
        Some(j) => j,
        None => {
            // already completed (e.g. force-cancelled): still record the final steps/outputs
            let j: JobRow = sqlx::query_as("SELECT * FROM actions_jobs WHERE id = $1").bind(job_id).fetch_one(&s.db).await?;
            if let Some(st) = steps {
                sqlx::query("UPDATE actions_jobs SET steps = $2 WHERE id = $1").bind(job_id).bind(st).execute(&s.db).await?;
            }
            super::logs::compact(s, job_id).await.ok();
            return advance(s, j.run_id).await;
        }
    };
    if let Err(e) = super::logs::compact(s, job_id).await {
        tracing::warn!(job = job_id, error = %format!("{e:#}"), "log compaction failed");
    }
    let run: RunRow = sqlx::query_as("SELECT * FROM actions_runs WHERE id = $1").bind(job.run_id).fetch_one(&s.db).await?;
    let ctx = run_ctx(s, run.repo_id).await?;
    post_status(s, &ctx, &run, &job).await;
    advance(s, job.run_id).await
}

/// Server-side safety net (scheduler loop): fails jobs whose runner stopped sending heartbeats
/// or that ran past their timeout.
pub async fn reap(s: &AppState) -> ApiResult<usize> {
    let lost: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT id, (timeout_at IS NOT NULL AND timeout_at < now() - interval '2 minutes') FROM actions_jobs
          WHERE status = 'in_progress' AND (heartbeat_at < now() - interval '3 minutes' OR timeout_at < now() - interval '2 minutes')",
    )
    .fetch_all(&s.db)
    .await?;
    for (id, timed_out) in &lost {
        let concl = if *timed_out { "cancelled" } else { "failure" };
        let msg = if *timed_out {
            "##[error]The job has exceeded the maximum execution time and the runner did not stop it.\n"
        } else {
            "##[error]The runner has lost communication with the server. Verify the machine is running and has a healthy network connection.\n"
        };
        let _ = super::logs::append(s, *id, 0, format!("{} {msg}", super::logs::stamp()).as_bytes()).await;
        finish_job(s, *id, concl, None, None).await?;
    }
    Ok(lost.len())
}
