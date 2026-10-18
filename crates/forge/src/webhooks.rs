//! Webhook events and delivery.
//!
//! Other areas emit events with [`enqueue_event`]; it finds the matching active repo/org hooks,
//! records a delivery row per hook and enqueues a job on the standard queue (`crate::jobs`). The
//! worker POSTs the payload with GitHub's headers (`X-GitHub-Event`, `X-GitHub-Delivery`,
//! `X-Hub-Signature-256`, ...) and retries with backoff on failure. Events that have an activity
//! counterpart (IssuesEvent, PushEvent, ...) are also recorded in `activity_events`.
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{json, Map, Value};
use sha2::Sha256;

use crate::db::{self, Perm, RepoCtx, User};
use crate::jobs::Job;
use crate::state::AppState;

pub const QUEUE: &str = "webhook";
pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HookRow {
    pub id: i64,
    pub repo_id: Option<i64>,
    pub org_id: Option<i64>,
    pub config: Value,
    pub events: Vec<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub name: String,
    pub last_response: Value,
    /// Set for a GitHub App's webhook (apps area).
    #[sqlx(default)]
    pub app_id: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeliveryRow {
    pub id: i64,
    pub hook_id: i64,
    pub guid: String,
    pub event: String,
    pub action: Option<String>,
    pub repo_id: Option<i64>,
    pub payload: Value,
    pub redelivery: bool,
    pub status: String,
    pub status_code: i32,
    pub duration: f64,
    pub request_headers: Option<Value>,
    pub response_headers: Option<Value>,
    pub response_body: Option<String>,
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

/// Repository object as embedded in webhook payloads (no viewer permissions).
pub fn repo_payload(state: &AppState, ctx: &RepoCtx) -> Value {
    let mut v = crate::json::repo::repo(&state.api_url(), state.base_url(), ctx);
    if let Some(o) = v.as_object_mut() {
        o.remove("permissions");
    }
    v
}

/// Activity event type for a webhook event name, if GitHub records one.
fn activity_type(event: &str) -> Option<&'static str> {
    Some(match event {
        "issues" => "IssuesEvent",
        "issue_comment" => "IssueCommentEvent",
        "pull_request" => "PullRequestEvent",
        "pull_request_review" => "PullRequestReviewEvent",
        "pull_request_review_comment" => "PullRequestReviewCommentEvent",
        "release" => "ReleaseEvent",
        "create" => "CreateEvent",
        "delete" => "DeleteEvent",
        "fork" => "ForkEvent",
        "watch" => "WatchEvent",
        "commit_comment" => "CommitCommentEvent",
        "member" => "MemberEvent",
        "public" => "PublicEvent",
        "gollum" => "GollumEvent",
        _ => return None,
    })
}

/// Emits a webhook event for a repository.
///
/// `payload` is the event body without the common keys: pass `action` (if any), the subject
/// (`issue`, `pull_request`, `comment`, ...) and `sender` (simple-user JSON of the actor).
/// `repository` and `organization` are added here. Also records the matching activity event
/// (IssuesEvent, IssueCommentEvent, PullRequestEvent, ...). Never fails the caller's request on
/// delivery problems; returns an error only if the database is unavailable.
pub async fn enqueue_event(state: &AppState, repo_id: i64, event: &str, payload: Value) -> anyhow::Result<()> {
    let repo = db::repo_by_id(&state.db, repo_id).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let owner = db::user_by_id(&state.db, repo.owner_id).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let ctx = RepoCtx { repo, owner, perm: Perm::Admin };
    enqueue_event_ctx(state, &ctx, event, payload).await
}

/// Like [`enqueue_event`] when the caller already has the `RepoCtx`.
pub async fn enqueue_event_ctx(state: &AppState, ctx: &RepoCtx, event: &str, mut payload: Value) -> anyhow::Result<()> {
    // Draft releases are visible to writers only: they never become (public) activity events.
    let draft_release = event == "release" && payload.pointer("/release/draft").and_then(Value::as_bool) == Some(true);
    if let Some(kind) = activity_type(event).filter(|_| !draft_release) {
        let actor_id = payload.pointer("/sender/id").and_then(Value::as_i64);
        let mut ap = payload.clone();
        if let Some(o) = ap.as_object_mut() {
            o.remove("sender");
            o.remove("repository");
            o.remove("organization");
        }
        crate::activity::record(state, kind, actor_id, Some(ctx), ap).await?;
    }
    if let Some(o) = payload.as_object_mut() {
        o.entry("repository").or_insert_with(|| repo_payload(state, ctx));
        if ctx.owner.kind == "Organization" {
            o.entry("organization")
                .or_insert_with(|| crate::json::user::org_simple(&state.api_url(), state.base_url(), &ctx.owner));
        }
    }
    let hooks: Vec<HookRow> = sqlx::query_as(
        "SELECT * FROM webhooks WHERE active AND (repo_id = $1 OR org_id = $2)
            AND (events && ARRAY[$3, '*']::text[])",
    )
    .bind(ctx.repo.id)
    .bind(ctx.owner.id)
    .bind(event)
    .fetch_all(&state.db)
    .await?;
    for h in hooks {
        let mut p = payload.clone();
        if h.org_id.is_some() {
            p["hook_id"] = json!(h.id);
        }
        create_delivery(state, &h, event, Some(ctx.repo.id), p, false).await?;
    }
    // GitHub Apps installed on the repository and subscribed to the event.
    crate::apps::hooks::fan_out(state, ctx, event, &payload).await?;
    Ok(())
}

/// Emits an organization-level event (org hooks only).
pub async fn enqueue_org_event(state: &AppState, org: &User, event: &str, mut payload: Value) -> anyhow::Result<()> {
    if let Some(o) = payload.as_object_mut() {
        o.entry("organization")
            .or_insert_with(|| crate::json::user::org_simple(&state.api_url(), state.base_url(), org));
    }
    let hooks: Vec<HookRow> = sqlx::query_as(
        "SELECT * FROM webhooks WHERE active AND org_id = $1 AND (events && ARRAY[$2, '*']::text[])",
    )
    .bind(org.id)
    .bind(event)
    .fetch_all(&state.db)
    .await?;
    for h in hooks {
        create_delivery(state, &h, event, None, payload.clone(), false).await?;
    }
    Ok(())
}

/// Records a delivery for one hook and enqueues its job. Returns the delivery id.
pub async fn create_delivery(
    state: &AppState,
    hook: &HookRow,
    event: &str,
    repo_id: Option<i64>,
    payload: Value,
    redelivery: bool,
) -> anyhow::Result<i64> {
    let guid = uuid::Uuid::new_v4().to_string();
    let action = payload.get("action").and_then(Value::as_str).map(str::to_string);
    let mut tx = state.db.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO webhook_deliveries (hook_id, guid, event, action, repo_id, payload, redelivery)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(hook.id)
    .bind(&guid)
    .bind(event)
    .bind(action)
    .bind(repo_id)
    .bind(&payload)
    .bind(redelivery)
    .fetch_one(&mut *tx)
    .await?;
    crate::jobs::enqueue(&mut *tx, QUEUE, &json!({ "delivery_id": id })).await?;
    tx.commit().await?;
    Ok(id)
}

fn client(insecure: bool) -> &'static reqwest::Client {
    static SECURE: OnceLock<reqwest::Client> = OnceLock::new();
    static INSECURE: OnceLock<reqwest::Client> = OnceLock::new();
    let cell = if insecure { &INSECURE } else { &SECURE };
    cell.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            // SSRF: names resolving to loopback/private/link-local addresses are refused at
            // connect time (literal IPs are checked in `deliver`).
            .dns_resolver(crate::security::PublicOnlyResolver)
            .tls_danger_accept_invalid_certs(insecure)
            .user_agent("GitHub-Hookshot/forge")
            .build()
            .expect("http client")
    })
}

/// `sha256=<hex hmac>` as sent in `X-Hub-Signature-256`.
pub fn signature_256(secret: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// `sha1=<hex hmac>` as sent in the legacy `X-Hub-Signature`.
pub fn signature_1(secret: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<sha1::Sha1> as KeyInit>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    format!("sha1={}", hex::encode(mac.finalize().into_bytes()))
}

fn cfg_str<'a>(cfg: &'a Value, k: &str) -> Option<&'a str> {
    cfg.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub async fn run_delivery_job(state: &AppState, job: &Job) -> anyhow::Result<()> {
    let id = job.payload.get("delivery_id").and_then(Value::as_i64).ok_or_else(|| anyhow::anyhow!("bad job"))?;
    deliver(state, id).await
}

/// Performs one delivery attempt and records the outcome. Errors (non-2xx, transport) are
/// returned so the queue retries.
pub async fn deliver(state: &AppState, delivery_id: i64) -> anyhow::Result<()> {
    let Some(d) = sqlx::query_as::<_, DeliveryRow>("SELECT * FROM webhook_deliveries WHERE id = $1")
        .bind(delivery_id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(()); // hook deleted
    };
    let Some(hook) = sqlx::query_as::<_, HookRow>("SELECT * FROM webhooks WHERE id = $1")
        .bind(d.hook_id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(());
    };
    let Some(url) = cfg_str(&hook.config, "url").map(str::to_string) else {
        record_result(state, &d, &hook, 0, "Invalid URL", None, None, None, 0.0).await?;
        return Ok(());
    };
    // SSRF: never deliver to internal addresses (cloud metadata, the forge itself, cluster
    // services) unless explicitly allowed. Names are checked by the client's resolver.
    let parsed = url::Url::parse(&url).ok().filter(|u| matches!(u.scheme(), "http" | "https"));
    let blocked = match parsed.as_ref().and_then(|u| u.host()) {
        None => true,
        Some(h) => !crate::security::webhook_allow_private() && crate::security::blocked_literal_host(&h),
    };
    if blocked {
        record_result(state, &d, &hook, 0, "Invalid URL: destination is not a public address", None, None, None, 0.0).await?;
        return Ok(());
    }
    let json_body = serde_json::to_vec(&d.payload)?;
    let form =cfg_str(&hook.config, "content_type") == Some("form");
    let body: Vec<u8> = if form {
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("payload", std::str::from_utf8(&json_body)?)
            .finish()
            .into_bytes()
    } else {
        json_body
    };
    let target_type = if hook.app_id.is_some() { "integration" } else if hook.org_id.is_some() { "organization" } else { "repository" };
    let target_id = hook.app_id.or(hook.org_id).or(hook.repo_id).unwrap_or(0);
    let mut headers: Vec<(String, String)> = vec![
        ("Accept".into(), "*/*".into()),
        ("Content-Type".into(), if form { "application/x-www-form-urlencoded" } else { "application/json" }.into()),
        ("User-Agent".into(), "GitHub-Hookshot/forge".into()),
        ("X-GitHub-Delivery".into(), d.guid.clone()),
        ("X-GitHub-Event".into(), d.event.clone()),
        ("X-GitHub-Hook-ID".into(), hook.id.to_string()),
        ("X-GitHub-Hook-Installation-Target-ID".into(), target_id.to_string()),
        ("X-GitHub-Hook-Installation-Target-Type".into(), target_type.into()),
        ("X-GitHub-Enterprise-Host".into(), state.cfg.host()),
        ("X-GitHub-Enterprise-Version".into(), "3.14.0".into()),
    ];
    if let Some(secret) = cfg_str(&hook.config, "secret") {
        headers.push(("X-Hub-Signature".into(), signature_1(secret, &body)));
        headers.push(("X-Hub-Signature-256".into(), signature_256(secret, &body)));
    }
    let insecure = matches!(hook.config.get("insecure_ssl"), Some(Value::String(s)) if s == "1")
        || matches!(hook.config.get("insecure_ssl"), Some(Value::Number(n)) if n.as_i64() == Some(1));
    let mut req = client(insecure).post(&url).body(body);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let req_headers: Map<String, Value> = headers
        .iter()
        .filter(|(k, _)| !k.starts_with("X-Hub-Signature"))
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    let started = Instant::now();
    let res = req.send().await;
    match res {
        Ok(r) => {
            let code = r.status().as_u16() as i32;
            let resp_headers: Map<String, Value> = r
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), json!(v.to_str().unwrap_or(""))))
                .collect();
            // Read at most 64 KiB of the response (a receiver could stream gigabytes).
            let mut raw: Vec<u8> = Vec::new();
            let mut r = r;
            while raw.len() < 65536 {
                match r.chunk().await {
                    Ok(Some(c)) => raw.extend_from_slice(&c),
                    _ => break,
                }
            }
            raw.truncate(65536);
            let mut text = String::from_utf8_lossy(&raw).into_owned();
            if text.len() > 65536 {
                let mut cut = 65536;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
            }
            let dur = started.elapsed().as_secs_f64();
            let ok = (200..300).contains(&code);
            let status = if ok { "OK".to_string() } else { format!("Invalid HTTP Response: {code}") };
            record_result(state, &d, &hook, code, &status, Some(Value::Object(req_headers)), Some(Value::Object(resp_headers)), Some(text), dur)
                .await?;
            if ok {
                Ok(())
            } else {
                anyhow::bail!("{status}")
            }
        }
        Err(e) => {
            let dur = started.elapsed().as_secs_f64();
            let status = if e.is_timeout() { "timed out".to_string() } else { "failed to connect".to_string() };
            record_result(state, &d, &hook, 0, &status, Some(Value::Object(req_headers)), None, None, dur).await?;
            anyhow::bail!("delivery {}: {e}", d.guid)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_result(
    state: &AppState,
    d: &DeliveryRow,
    hook: &HookRow,
    code: i32,
    status: &str,
    req_headers: Option<Value>,
    resp_headers: Option<Value>,
    body: Option<String>,
    duration: f64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE webhook_deliveries SET status = $2, status_code = $3, duration = $4, request_headers = $5,
            response_headers = $6, response_body = $7, attempts = attempts + 1, delivered_at = now()
          WHERE id = $1",
    )
    .bind(d.id)
    .bind(status)
    .bind(code)
    .bind(duration)
    .bind(req_headers)
    .bind(resp_headers)
    .bind(body)
    .execute(&state.db)
    .await?;
    let ok = (200..300).contains(&code);
    let last = json!({
        "code": if code == 0 { Value::Null } else { json!(code) },
        "status": if ok { "active" } else { "failed" },
        "message": if ok { Value::Null } else { json!(status) },
    });
    sqlx::query("UPDATE webhooks SET last_response = $2 WHERE id = $1")
        .bind(hook.id)
        .bind(last)
        .execute(&state.db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Push / create / delete events from ref changes.

struct LogCommit {
    sha: String,
    tree: String,
    author_name: String,
    author_email: String,
    author_date: String,
    committer_name: String,
    committer_email: String,
    committer_date: String,
    message: String,
    added: Vec<String>,
    removed: Vec<String>,
    modified: Vec<String>,
}

async fn log_commits(state: &AppState, repo_id: i64, refname: &str, old: &str, new: &str) -> anyhow::Result<Vec<LogCommit>> {
    let path = state.git.sync(repo_id).await?;
    let fmt = "--format=%x1e%H%x00%T%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%B%x00";
    let range;
    let exclude;
    let mut args: Vec<&str> = vec!["log", fmt, "--name-status", "--no-renames", "-n", "20", "--reverse"];
    if old == ZERO_SHA {
        exclude = format!("--exclude={refname}");
        args.extend([new, "--not", exclude.as_str(), "--all"]);
    } else {
        range = format!("{old}..{new}");
        args.push(range.as_str());
    }
    let (ok, out, _) = state.git.git.try_run(&path, &args).await?;
    if !ok {
        return Ok(vec![]);
    }
    let text = String::from_utf8_lossy(&out);
    let mut commits = Vec::new();
    for rec in text.split('\x1e').filter(|r| !r.trim().is_empty()) {
        let f: Vec<&str> = rec.splitn(10, '\0').collect();
        if f.len() < 10 {
            continue;
        }
        let (mut added, mut removed, mut modified) = (vec![], vec![], vec![]);
        for line in f[9].lines() {
            if let Some((st, p)) = line.split_once('\t') {
                match st.chars().next() {
                    Some('A') => added.push(p.to_string()),
                    Some('D') => removed.push(p.to_string()),
                    Some(_) => modified.push(p.to_string()),
                    None => {}
                }
            }
        }
        commits.push(LogCommit {
            sha: f[0].trim().to_string(),
            tree: f[1].to_string(),
            author_name: f[2].to_string(),
            author_email: f[3].to_string(),
            author_date: f[4].to_string(),
            committer_name: f[5].to_string(),
            committer_email: f[6].to_string(),
            committer_date: f[7].to_string(),
            message: f[8].trim_end().to_string(),
            added,
            removed,
            modified,
        });
    }
    Ok(commits)
}

/// Called from `crate::hooks::after_refs_changed`: emits push/create/delete webhooks and
/// records PushEvent/CreateEvent/DeleteEvent activity.
pub async fn on_refs_changed(
    state: &AppState,
    ctx: &RepoCtx,
    actor: Option<&User>,
    changed: &[(String, String, String)],
) -> anyhow::Result<()> {
    let api = state.api_url();
    let base = state.base_url().to_string();
    let full = ctx.full_name();
    let sender = actor.map(|u| crate::json::user::simple(&api, &base, u));
    let actor_id = actor.map(|u| u.id);
    for (refname, old, new) in changed {
        let (ref_type, short) = if let Some(b) = refname.strip_prefix("refs/heads/") {
            ("branch", b)
        } else if let Some(t) = refname.strip_prefix("refs/tags/") {
            ("tag", t)
        } else {
            continue; // refs/pull/* and other internal refs are not announced
        };
        let created = old == ZERO_SHA;
        let deleted = new == ZERO_SHA;
        let commits = if deleted { vec![] } else { log_commits(state, ctx.repo.id, refname, old, new).await.unwrap_or_default() };
        let forced = if created || deleted {
            false
        } else {
            let path = state.git.sync(ctx.repo.id).await?;
            let (ok, _, _) = state.git.git.try_run(&path, &["merge-base", "--is-ancestor", old, new]).await?;
            !ok
        };
        let commit_json = |c: &LogCommit| {
            json!({
                "id": c.sha,
                "tree_id": c.tree,
                "distinct": true,
                "message": c.message,
                "timestamp": c.committer_date,
                "url": format!("{base}/{full}/commit/{}", c.sha),
                "author": {"name": c.author_name, "email": c.author_email, "date": c.author_date},
                "committer": {"name": c.committer_name, "email": c.committer_email, "date": c.committer_date},
                "added": c.added, "removed": c.removed, "modified": c.modified,
            })
        };
        let compare = if created {
            format!("{base}/{full}/commit/{new}")
        } else if deleted {
            format!("{base}/{full}/compare/{}", &old[..12.min(old.len())])
        } else {
            format!("{base}/{full}/compare/{}...{}", &old[..12], &new[..12])
        };
        let pusher = match actor {
            Some(u) => json!({"name": u.login, "email": u.email.clone().unwrap_or_default()}),
            None => json!({"name": "forge", "email": null}),
        };
        let mut push = json!({
            "ref": refname,
            "before": old,
            "after": new,
            "created": created,
            "deleted": deleted,
            "forced": forced,
            "base_ref": null,
            "compare": compare,
            "commits": commits.iter().map(commit_json).collect::<Vec<_>>(),
            "head_commit": commits.last().map(commit_json),
            "pusher": pusher,
        });
        if let Some(s) = &sender {
            push["sender"] = s.clone();
        }
        // Activity PushEvent (activity payload shape) — not for deletions.
        if !deleted {
            let push_id: i64 = rand::random::<u32>() as i64;
            let act = json!({
                "repository_id": ctx.repo.id,
                "push_id": push_id,
                "size": commits.len(),
                "distinct_size": commits.len(),
                "ref": refname,
                "head": new,
                "before": old,
                "commits": commits.iter().map(|c| json!({
                    "sha": c.sha,
                    "author": {"email": c.author_email, "name": c.author_name},
                    "message": c.message,
                    "distinct": true,
                    "url": format!("{api}/repos/{full}/commits/{}", c.sha),
                })).collect::<Vec<_>>(),
            });
            crate::activity::record(state, "PushEvent", actor_id, Some(ctx), act).await?;
        }
        enqueue_event_ctx(state, ctx, "push", push).await?;
        if created {
            let mut p = json!({
                "ref": short,
                "ref_type": ref_type,
                "master_branch": ctx.repo.default_branch,
                "description": ctx.repo.description,
                "pusher_type": "user",
            });
            if let Some(s) = &sender {
                p["sender"] = s.clone();
            }
            enqueue_event_ctx(state, ctx, "create", p).await?;
        }
        if deleted {
            let mut p = json!({ "ref": short, "ref_type": ref_type, "pusher_type": "user" });
            if let Some(s) = &sender {
                p["sender"] = s.clone();
            }
            enqueue_event_ctx(state, ctx, "delete", p).await?;
        }
    }
    Ok(())
}
