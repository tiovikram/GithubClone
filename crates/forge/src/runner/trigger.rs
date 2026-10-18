//! Workflow triggering: repository events (push, pull_request, issues, release, ...) reach
//! [`on_event`] through `webhooks::enqueue_event_ctx`; `workflow_dispatch` comes from the REST
//! API ([`dispatch`]); `schedule` crons are fired by [`spawn_scheduler`], which runs in every
//! `forge serve` replica but does work only while holding a Postgres advisory lock.
use std::time::Duration;

use chrono::{DateTime, Timelike, Utc};
use serde_json::{json, Map, Value};

use super::engine::{self, NewRun};
use super::token::BOT_LOGIN;
use super::workflow::{self, EventInfo};
use crate::api::actions::RunRow;
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const ZERO: &str = "0000000000000000000000000000000000000000";
/// Advisory lock key of the scheduler ("forge-actions-scheduler").
const SCHEDULER_LOCK: i64 = 0x0f0e_ac71_0005_c4ed;

async fn git(s: &AppState, repo_id: i64, args: &[&str]) -> Option<String> {
    crate::api::plat::git_read(s, repo_id, args).await.ok().flatten().map(|b| String::from_utf8_lossy(&b).to_string())
}

/// Workflow files (path, text) in `.github/workflows` at `rev`.
pub async fn workflow_files(s: &AppState, repo_id: i64, rev: &str) -> Vec<(String, String)> {
    let Some(list) = git(s, repo_id, &["ls-tree", &format!("{rev}:.github/workflows")]).await else { return vec![] };
    let mut out = Vec::new();
    for line in list.lines() {
        let Some((meta, name)) = line.split_once('\t') else { continue };
        let f: Vec<&str> = meta.split_whitespace().collect();
        if f.get(1) != Some(&"blob") || !(name.ends_with(".yml") || name.ends_with(".yaml")) {
            continue;
        }
        if let Some(text) = git(s, repo_id, &["cat-file", "blob", f[2]]).await {
            out.push((format!(".github/workflows/{name}"), text));
        }
    }
    out
}

async fn disabled_workflows(s: &AppState, repo_id: i64) -> Vec<i64> {
    sqlx::query_scalar("SELECT workflow_id FROM actions_workflow_states WHERE repo_id = $1 AND state <> 'active'")
        .bind(repo_id)
        .fetch_all(&s.db)
        .await
        .unwrap_or_default()
}

async fn resolve(s: &AppState, repo_id: i64, rev: &str) -> Option<String> {
    git(s, repo_id, &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")]).await.map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
}

/// Called for every repository event (after the webhook payload is complete).
/// Boxed: creating a run creates check runs, which emit webhook events, which call this again
/// (those events are ignored, but the future type must not be recursive).
pub fn on_event<'a>(s: &'a AppState, ctx: &'a RepoCtx, event: &'a str, payload: &'a Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if let Err(e) = on_event_inner(s, ctx, event, payload).await {
            tracing::warn!(repo = ctx.repo.id, event, error = %e.message, "actions trigger failed");
        }
    })
}

async fn on_event_inner(s: &AppState, ctx: &RepoCtx, event: &str, payload: &Value) -> ApiResult<()> {
    // Events caused by GITHUB_TOKEN never trigger new runs (GitHub's recursion guard), and
    // Actions' own events don't either.
    if payload.pointer("/sender/login").and_then(Value::as_str) == Some(BOT_LOGIN) {
        return Ok(());
    }
    if matches!(event, "workflow_run" | "workflow_job" | "check_run" | "check_suite" | "workflow_dispatch" | "schedule" | "ping" | "meta") {
        return Ok(());
    }
    if !super::settings::actions_allowed(s, ctx.repo.id).await {
        return Ok(());
    }
    let actor = match payload.pointer("/sender/id").and_then(Value::as_i64) {
        Some(id) => db::user_by_id(&s.db, id).await.ok(),
        None => None,
    };
    let action = payload.get("action").and_then(Value::as_str);
    let mut paths: Option<Vec<String>> = None;
    let (rev, git_ref, head_branch, head_sha, github_sha, filter_ref, pull_requests) = match event {
        "push" => {
            let r = payload["ref"].as_str().unwrap_or_default().to_string();
            let after = payload["after"].as_str().unwrap_or_default().to_string();
            if after.is_empty() || after == ZERO || payload["deleted"].as_bool() == Some(true) {
                return Ok(());
            }
            let short = r.trim_start_matches("refs/heads/").trim_start_matches("refs/tags/").to_string();
            let mut p: Vec<String> = Vec::new();
            for c in payload["commits"].as_array().cloned().unwrap_or_default() {
                for k in ["added", "removed", "modified"] {
                    for f in c[k].as_array().cloned().unwrap_or_default() {
                        if let Some(f) = f.as_str() {
                            if !p.iter().any(|x| x == f) {
                                p.push(f.to_string());
                            }
                        }
                    }
                }
            }
            if !p.is_empty() {
                paths = Some(p);
            }
            (after.clone(), r.clone(), Some(short), after.clone(), after, Some(r), json!([]))
        }
        "pull_request" | "pull_request_target" => {
            let pr = &payload["pull_request"];
            let number = pr["number"].as_i64().unwrap_or(0);
            let head_sha = pr.pointer("/head/sha").and_then(Value::as_str).unwrap_or_default().to_string();
            let base_ref = pr.pointer("/base/ref").and_then(Value::as_str).unwrap_or_default().to_string();
            let base_sha = pr.pointer("/base/sha").and_then(Value::as_str).unwrap_or_default().to_string();
            let head_ref = pr.pointer("/head/ref").and_then(Value::as_str).map(str::to_string);
            if head_sha.is_empty() {
                return Ok(());
            }
            if let Some(out) = git(s, ctx.repo.id, &["diff", "--name-only", &format!("{base_sha}...{head_sha}")]).await {
                paths = Some(out.lines().map(str::to_string).collect());
            }
            let api = s.api_url();
            let minimal = json!([{
                "id": pr["id"], "number": number, "url": format!("{api}/repos/{}/pulls/{number}", ctx.full_name()),
                "head": { "ref": head_ref, "sha": head_sha, "repo": { "id": pr.pointer("/head/repo/id"), "url": pr.pointer("/head/repo/url"), "name": pr.pointer("/head/repo/name") } },
                "base": { "ref": base_ref, "sha": base_sha, "repo": { "id": ctx.repo.id, "url": format!("{api}/repos/{}", ctx.full_name()), "name": ctx.repo.name } },
            }]);
            if event == "pull_request_target" {
                let base_head = resolve(s, ctx.repo.id, &format!("refs/heads/{base_ref}")).await.unwrap_or(base_sha.clone());
                (base_head.clone(), format!("refs/heads/{base_ref}"), head_ref, head_sha, base_head, Some(base_ref), minimal)
            } else {
                let merge = resolve(s, ctx.repo.id, &format!("refs/pull/{number}/merge")).await;
                let (rev, git_ref) = match &merge {
                    Some(m) => (m.clone(), format!("refs/pull/{number}/merge")),
                    None => (head_sha.clone(), format!("refs/pull/{number}/head")),
                };
                (rev.clone(), git_ref, head_ref, head_sha, rev, Some(base_ref), minimal)
            }
        }
        _ => {
            let def = format!("refs/heads/{}", ctx.repo.default_branch);
            let Some(sha) = resolve(s, ctx.repo.id, &def).await else { return Ok(()) };
            let (git_ref, branch) = match (event, payload.pointer("/release/tag_name").and_then(Value::as_str)) {
                ("release", Some(tag)) => (format!("refs/tags/{tag}"), Some(tag.to_string())),
                _ => (def.clone(), Some(ctx.repo.default_branch.clone())),
            };
            (sha.clone(), git_ref, branch, sha.clone(), sha, None, json!([]))
        }
    };
    let files = workflow_files(s, ctx.repo.id, &rev).await;
    if event == "push" && git_ref == format!("refs/heads/{}", ctx.repo.default_branch) {
        refresh_schedules(s, ctx.repo.id, &files).await;
    }
    if files.is_empty() {
        return Ok(());
    }
    let disabled = disabled_workflows(s, ctx.repo.id).await;
    for (path, text) in files {
        if disabled.contains(&engine::workflow_id(ctx.repo.id, &path)) {
            continue;
        }
        let ev = EventInfo { event, action, git_ref: filter_ref.as_deref(), paths: paths.as_deref() };
        let triggered = match workflow::parse(&text) {
            Ok(w) => w.triggered_by(&ev),
            // Invalid files still get a (failed) run for events they name, like GitHub.
            Err(_) => workflow::parse_yaml(&text)
                .ok()
                .and_then(|v| v.get("on").cloned())
                .is_some_and(|on| match on {
                    Value::String(x) => x == event,
                    Value::Array(a) => a.iter().any(|x| x.as_str() == Some(event)),
                    Value::Object(m) => m.contains_key(event),
                    _ => false,
                }),
        };
        if !triggered {
            continue;
        }
        let display_title = match event {
            "pull_request" | "pull_request_target" => payload.pointer("/pull_request/title").and_then(Value::as_str).map(str::to_string),
            "issues" | "issue_comment" => payload.pointer("/issue/title").and_then(Value::as_str).map(str::to_string),
            "release" => payload.pointer("/release/name").and_then(Value::as_str).map(str::to_string),
            _ => None,
        };
        let nr = NewRun {
            workflow_path: path,
            workflow_text: text,
            event: event.to_string(),
            payload: payload.clone(),
            actor: actor.clone(),
            head_branch: head_branch.clone(),
            git_ref: git_ref.clone(),
            head_sha: head_sha.clone(),
            github_sha: github_sha.clone(),
            inputs: None,
            pull_requests: pull_requests.clone(),
            display_title,
        };
        engine::create_run(s, ctx, nr).await?;
    }
    Ok(())
}

/// Validates dispatch inputs against the workflow's declaration.
fn check_inputs(declared: &Map<String, Value>, given: &Map<String, Value>) -> ApiResult<Map<String, Value>> {
    let unexpected: Vec<&String> = given.keys().filter(|k| !declared.contains_key(*k)).collect();
    if !unexpected.is_empty() {
        let list = unexpected.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ");
        return Err(ApiError::unprocessable(format!("Unexpected inputs provided: [{list}]")));
    }
    let mut out = Map::new();
    for (k, spec) in declared {
        let v = given.get(k).cloned();
        let required = spec.get("required").and_then(Value::as_bool).unwrap_or(false);
        if required && v.is_none() && spec.get("default").is_none() {
            return Err(ApiError::unprocessable(format!("Required input '{k}' not provided")));
        }
        let v = v.or_else(|| spec.get("default").cloned());
        if let Some(v) = v {
            let sv = super::expr::to_string(&v);
            if spec.get("type").and_then(Value::as_str) == Some("choice") {
                let opts: Vec<String> = spec.get("options").and_then(Value::as_array).map(|a| a.iter().map(super::expr::to_string).collect()).unwrap_or_default();
                if !opts.contains(&sv) {
                    return Err(ApiError::unprocessable(format!("Provided value '{sv}' for input '{k}' not in the list of allowed values")));
                }
            }
            if spec.get("type").and_then(Value::as_str) == Some("boolean") && !matches!(sv.as_str(), "true" | "false") {
                return Err(ApiError::unprocessable(format!("Provided value '{sv}' for input '{k}' is not a boolean")));
            }
            out.insert(k.clone(), Value::String(sv));
        }
    }
    Ok(out)
}

/// `workflow_dispatch`: runs the workflow file as it is at `rev` (a commit on `git_ref`).
pub async fn dispatch(s: &AppState, ctx: &RepoCtx, actor: &User, path: &str, branch: Option<String>, git_ref: &str, sha: &str, inputs: Option<Value>) -> ApiResult<RunRow> {
    if !super::settings::actions_allowed(s, ctx.repo.id).await {
        return Err(ApiError::forbidden("Actions are disabled for this repository."));
    }
    let files = workflow_files(s, ctx.repo.id, sha).await;
    let Some((_, text)) = files.into_iter().find(|(p, _)| p == path) else {
        return Err(ApiError::unprocessable(format!("Workflow does not exist at ref {git_ref}")));
    };
    let w = workflow::parse(&text).map_err(|e| ApiError::unprocessable(format!("Invalid workflow file: {e}")))?;
    if !w.on.contains_key("workflow_dispatch") {
        return Err(ApiError::unprocessable("Workflow does not have 'workflow_dispatch' trigger"));
    }
    let given = match inputs {
        Some(Value::Object(m)) => m,
        Some(Value::Null) | None => Map::new(),
        Some(_) => return Err(ApiError::unprocessable("inputs must be an object")),
    };
    let inputs = check_inputs(&w.dispatch_inputs(), &given)?;
    let api = s.api_url();
    let payload = json!({
        "inputs": inputs,
        "ref": git_ref,
        "repository": crate::webhooks::repo_payload(s, ctx),
        "sender": crate::json::user::simple(&api, s.base_url(), actor),
        "workflow": path,
    });
    let nr = NewRun {
        workflow_path: path.to_string(),
        workflow_text: text,
        event: "workflow_dispatch".into(),
        payload,
        actor: Some(actor.clone()),
        head_branch: branch,
        git_ref: git_ref.to_string(),
        head_sha: sha.to_string(),
        github_sha: sha.to_string(),
        inputs: Some(Value::Object(inputs)),
        pull_requests: json!([]),
        display_title: Some(w.name.clone().unwrap_or_else(|| path.to_string())),
    };
    engine::create_run(s, ctx, nr).await
}

/// Replaces the stored crons of a repository from its default-branch workflow files.
pub async fn refresh_schedules(s: &AppState, repo_id: i64, files: &[(String, String)]) {
    let mut want: Vec<(String, String)> = Vec::new();
    for (path, text) in files {
        if let Ok(w) = workflow::parse(text) {
            for c in w.crons() {
                if super::cron::Cron::parse(&c).is_some() {
                    want.push((path.clone(), c));
                }
            }
        }
    }
    let paths: Vec<String> = want.iter().map(|(p, _)| p.clone()).collect();
    let crons: Vec<String> = want.iter().map(|(_, c)| c.clone()).collect();
    let _ = sqlx::query(
        "DELETE FROM actions_schedules WHERE repo_id = $1
           AND (workflow_path, cron) NOT IN (SELECT * FROM unnest($2::text[], $3::text[]))",
    )
    .bind(repo_id)
    .bind(&paths)
    .bind(&crons)
    .execute(&s.db)
    .await;
    let _ = sqlx::query(
        "INSERT INTO actions_schedules (repo_id, workflow_path, cron, last_fired_at)
         SELECT $1, p, c, date_trunc('minute', now()) FROM unnest($2::text[], $3::text[]) AS t(p, c)
         ON CONFLICT DO NOTHING",
    )
    .bind(repo_id)
    .bind(&paths)
    .bind(&crons)
    .execute(&s.db)
    .await;
}

/// Fires the crons due in minute `minute`. Each row fires at most once per minute even if
/// several schedulers race (conditional UPDATE).
pub async fn fire_schedules(s: &AppState, minute: DateTime<Utc>) -> ApiResult<usize> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as("SELECT repo_id, workflow_path, cron FROM actions_schedules WHERE last_fired_at IS NULL OR last_fired_at < $1")
        .bind(minute)
        .fetch_all(&s.db)
        .await?;
    let mut fired = 0;
    for (repo_id, path, cron) in rows {
        let Some(c) = super::cron::Cron::parse(&cron) else { continue };
        if !c.matches(minute) {
            continue;
        }
        let claimed = sqlx::query(
            "UPDATE actions_schedules SET last_fired_at = $4 WHERE repo_id = $1 AND workflow_path = $2 AND cron = $3
               AND (last_fired_at IS NULL OR last_fired_at < $4)",
        )
        .bind(repo_id)
        .bind(&path)
        .bind(&cron)
        .bind(minute)
        .execute(&s.db)
        .await?
        .rows_affected();
        if claimed == 0 {
            continue;
        }
        let Ok(repo) = db::repo_by_id(&s.db, repo_id).await else { continue };
        let Ok(owner) = db::user_by_id(&s.db, repo.owner_id).await else { continue };
        if repo.archived {
            continue;
        }
        let ctx = RepoCtx { repo, owner, perm: crate::db::Perm::Admin };
        if disabled_workflows(s, repo_id).await.contains(&engine::workflow_id(repo_id, &path)) {
            continue;
        }
        let def = format!("refs/heads/{}", ctx.repo.default_branch);
        let Some(sha) = resolve(s, repo_id, &def).await else { continue };
        let Some((_, text)) = workflow_files(s, repo_id, &sha).await.into_iter().find(|(p, _)| *p == path) else { continue };
        let payload = json!({ "schedule": cron, "repository": crate::webhooks::repo_payload(s, &ctx) });
        let nr = NewRun {
            workflow_path: path,
            workflow_text: text,
            event: "schedule".into(),
            payload,
            actor: None,
            head_branch: Some(ctx.repo.default_branch.clone()),
            git_ref: def,
            head_sha: sha.clone(),
            github_sha: sha,
            inputs: None,
            pull_requests: json!([]),
            display_title: None,
        };
        if engine::create_run(s, &ctx, nr).await.is_ok() {
            fired += 1;
        }
    }
    Ok(fired)
}

/// One scheduler round under the advisory lock: fire crons, reap lost/timed-out jobs, expire
/// artifacts and tokens. Returns false if another pod holds the lock.
pub async fn tick(s: &AppState) -> anyhow::Result<bool> {
    let mut conn = s.db.acquire().await?;
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)").bind(SCHEDULER_LOCK).fetch_one(&mut *conn).await?;
    if !got {
        return Ok(false);
    }
    let res: anyhow::Result<()> = async {
        let now = Utc::now();
        let minute = now.with_second(0).and_then(|t| t.with_nanosecond(0)).unwrap_or(now);
        fire_schedules(s, minute).await.map_err(|e| anyhow::anyhow!(e.message))?;
        engine::reap(s).await.map_err(|e| anyhow::anyhow!(e.message))?;
        super::storage::expire_artifacts(s).await.map_err(|e| anyhow::anyhow!(e.message))?;
        sqlx::query("DELETE FROM actions_job_tokens WHERE expires_at < now()").execute(&s.db).await?;
        sqlx::query("DELETE FROM actions_runner_tokens WHERE expires_at < now() - interval '1 day'").execute(&s.db).await?;
        Ok(())
    }
    .await;
    sqlx::query("SELECT pg_advisory_unlock($1)").bind(SCHEDULER_LOCK).execute(&mut *conn).await?;
    res?;
    Ok(true)
}

/// Starts the scheduler loop (every replica; only the lock holder works).
pub fn spawn_scheduler(s: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(&s).await {
                tracing::warn!(error = %format!("{e:#}"), "actions scheduler tick failed");
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    })
}
