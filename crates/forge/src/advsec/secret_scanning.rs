//! Secret scanning engine.
//!
//! - **Incremental scans.** After refs change (`hooks::after_refs_changed`) a job on the
//!   Postgres queue (`advsec.secret_scan`) scans the commits that are new to the repository
//!   (`git log <new> --not <old> <other refs>`), at most 1000 commits per push, blobs up to
//!   1 MiB, text only. Each changed blob is scanned once per job; every (commit, path) where it
//!   appears becomes an alert location.
//! - **Backfill.** Enabling secret scanning schedules a scan of the whole history of all
//!   branches and tags (at most 10000 commits).
//! - **Alerts** are keyed by (repository, secret type, sha256(secret)), so the same secret in
//!   many files is one alert with many locations.
//! - **Forge tokens** (`gho_`/`ghp_`/`ghs_`/`ghu_`/`ghr_`) are checked against the token table
//!   (validity `active`/`inactive`); found in a *public* repository, they are revoked at once
//!   and the alert is resolved as `revoked` (GitHub does the same for its own tokens).
//! - **Push protection** (`secret_scanning_push_protection`): pushes run a `pre-receive` hook
//!   (`forge hook pre-receive`, the forge binary itself, `FORGE_HOOK_BIN` overrides) that scans
//!   the incoming commits and rejects the push (GH013) listing each secret with an unblock URL
//!   whose last segment is the *placeholder id*. Contents API writes are checked in-process and
//!   answer 409 with the placeholder ids in `errors`. `POST .../push-protection-bypasses`
//!   allows a placeholder for 3 hours (or opens a bypass request under delegated bypass).
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::patterns::{self, Finding};
use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::git::cmd::GitCmd;
use crate::state::AppState;

pub const MAX_BLOB: usize = 1024 * 1024;
const MAX_COMMITS_INCREMENTAL: usize = 1000;
const MAX_COMMITS_BACKFILL: usize = 10_000;
const MAX_BLOBS: usize = 20_000;
pub const BYPASS_HOURS: i64 = 3;

pub fn secret_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Deterministic placeholder id for a secret in a repository (what the unblock URL carries).
pub fn placeholder_id(repo_id: i64, secret_type: &str, secret: &str) -> String {
    const A: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let d = Sha256::digest(format!("{repo_id}:{secret_type}:{secret}").as_bytes());
    d.iter().take(27).map(|b| A[(*b as usize) % 62] as char).collect()
}

// ---------------------------------------------------------------------------------------------
// Collecting the blobs of a commit range

/// A blob as it appears in a commit.
#[derive(Debug, Clone)]
pub struct Occurrence {
    pub commit: String,
    pub path: String,
    pub blob: String,
}

fn unquote(p: &str) -> String {
    if !(p.starts_with('"') && p.ends_with('"') && p.len() >= 2) {
        return p.to_string();
    }
    let inner = &p[1..p.len() - 1];
    let mut out = Vec::new();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let c = b[i + 1];
            if c.is_ascii_digit() && i + 3 < b.len() {
                if let Ok(v) = u8::from_str_radix(&inner[i + 1..i + 4], 8) {
                    out.push(v);
                    i += 4;
                    continue;
                }
            }
            out.push(match c {
                b'n' => b'\n',
                b't' => b'\t',
                other => other,
            });
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Added/modified blobs of the commits selected by `revs` (fed to `git log --stdin`, so it may
/// contain `--not`), newest first, at most `max_commits` commits.
pub async fn occurrences(git: &GitCmd, repo: &Path, revs: &str, max_commits: usize) -> anyhow::Result<Vec<Occurrence>> {
    let n = format!("-n{max_commits}");
    let out = git
        .run_input(
            repo,
            &["-c", "core.quotePath=false", "log", "--stdin", "--no-renames", "--raw", "--no-abbrev", "--root", "--diff-merges=first-parent", "--format=@@%H", &n],
            revs.as_bytes(),
        )
        .await?;
    let text = String::from_utf8_lossy(&out);
    let mut res = Vec::new();
    let mut commit = String::new();
    for line in text.lines() {
        if let Some(c) = line.strip_prefix("@@") {
            commit = c.trim().to_string();
            continue;
        }
        // ":100644 100644 <old> <new> M\tpath"
        let Some(rest) = line.strip_prefix(':') else { continue };
        let Some((meta, path)) = rest.split_once('\t') else { continue };
        let f: Vec<&str> = meta.split_whitespace().collect();
        if f.len() < 5 || f[4].starts_with('D') || f[1] == "160000" || f[1] == "120000" {
            continue;
        }
        res.push(Occurrence { commit: commit.clone(), path: unquote(path), blob: f[3].to_string() });
    }
    Ok(res)
}

/// Scans the blobs of `occ`: (occurrence index list, findings) per distinct blob.
pub async fn scan_occurrences(git: &GitCmd, repo: &Path, occ: &[Occurrence], non_provider: bool) -> anyhow::Result<Vec<(Vec<usize>, Vec<Finding>)>> {
    let mut by_blob: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, o) in occ.iter().enumerate() {
        by_blob.entry(o.blob.as_str()).or_default().push(i);
    }
    let blobs: Vec<String> = by_blob.keys().take(MAX_BLOBS).map(|b| b.to_string()).collect();
    // Sizes first so large blobs are never read.
    let info = crate::git::read::object_info(git, repo, &blobs).await?;
    let small: Vec<String> = blobs
        .iter()
        .zip(info)
        .filter(|(_, i)| i.as_ref().is_some_and(|(_, kind, size)| kind == "blob" && (*size as usize) <= MAX_BLOB))
        .map(|(b, _)| b.clone())
        .collect();
    let mut out = Vec::new();
    for chunk in small.chunks(500) {
        let objs = crate::git::read::cat_objects(git, repo, chunk).await?;
        for (sha, o) in chunk.iter().zip(objs) {
            let Some(o) = o else { continue };
            let head = &o.data[..o.data.len().min(8000)];
            if head.contains(&0) {
                continue; // binary
            }
            let text = String::from_utf8_lossy(&o.data);
            let found = patterns::scan(&text, non_provider);
            if !found.is_empty() {
                out.push((by_blob[sha.as_str()].clone(), found));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Scan jobs

/// Records a pending scan and queues it.
pub async fn schedule_scan(state: &AppState, repo_id: i64, kind: &str, changes: Option<Vec<Value>>, actor: Option<i64>) {
    let id: Option<i64> = sqlx::query_scalar("INSERT INTO secret_scanning_scans (repo_id, kind) VALUES ($1, $2) RETURNING id")
        .bind(repo_id)
        .bind(kind)
        .fetch_one(&state.db)
        .await
        .ok();
    super::enqueue(state, super::Q_SECRET_SCAN, json!({ "repo_id": repo_id, "scan_id": id, "kind": kind, "changes": changes, "actor": actor })).await;
}

pub async fn run_scan_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let repo_id = payload["repo_id"].as_i64().unwrap_or(0);
    let scan_id = payload["scan_id"].as_i64();
    let set_status = |status: &'static str| {
        let db = state.db.clone();
        async move {
            if let Some(id) = scan_id {
                let _ = sqlx::query(
                    "UPDATE secret_scanning_scans SET status = $2,
                        started_at = CASE WHEN $2 = 'running' THEN now() ELSE started_at END,
                        completed_at = CASE WHEN $2 IN ('completed', 'failed') THEN now() ELSE completed_at END WHERE id = $1",
                )
                .bind(id)
                .bind(status)
                .execute(&db)
                .await;
            }
        }
    };
    let Ok(ctx) = super::system_ctx(&state.db, repo_id).await else { return Ok(()) };
    if !super::enabled(&state.db, repo_id, "secret_scanning").await {
        set_status("completed").await;
        return Ok(());
    }
    set_status("running").await;
    let res = scan_repo(state, &ctx, payload).await;
    set_status(if res.is_ok() { "completed" } else { "failed" }).await;
    res
}

async fn scan_repo(state: &AppState, ctx: &RepoCtx, payload: &Value) -> anyhow::Result<()> {
    let path = state.git.sync(ctx.repo.id).await?;
    let git = &state.git.git;
    let (_, refs) = state.git.refs(ctx.repo.id).await?;
    let mut revs = String::new();
    let max = if payload["kind"] == "backfill" || payload["changes"].is_null() {
        revs.push_str("--branches\n--tags\n");
        MAX_COMMITS_BACKFILL
    } else {
        let changes: Vec<(String, String, String)> = payload["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| Some((c[0].as_str()?.to_string(), c[1].as_str()?.to_string(), c[2].as_str()?.to_string())))
            .collect();
        let changed: HashSet<&str> = changes.iter().map(|(r, _, _)| r.as_str()).collect();
        let mut nots = Vec::new();
        for (_, old, new) in &changes {
            if crate::git::read::is_sha(new) && refs.values().any(|v| v == new) {
                revs.push_str(new);
                revs.push('\n');
            }
            if old != crate::webhooks::ZERO_SHA {
                nots.push(old.clone());
            }
        }
        for (name, sha) in refs.iter() {
            if !changed.contains(name.as_str()) && (name.starts_with("refs/heads/") || name.starts_with("refs/tags/")) {
                nots.push(sha.clone());
            }
        }
        if revs.is_empty() {
            return Ok(());
        }
        revs.push_str("--not\n");
        for n in nots.iter().filter(|n| crate::git::read::is_sha(n)) {
            revs.push_str(n);
            revs.push('\n');
        }
        MAX_COMMITS_INCREMENTAL
    };
    let occ = occurrences(git, &path, &revs, max).await?;
    let non_provider = super::enabled(&state.db, ctx.repo.id, "secret_scanning_non_provider_patterns").await;
    let found = scan_occurrences(git, &path, &occ, non_provider).await?;
    let mut created = Vec::new();
    for (idxs, findings) in found {
        for f in findings {
            let alert = upsert_alert(state, ctx, &f).await?;
            if alert.1 {
                created.push(alert.0);
            }
            for &i in &idxs {
                let o = &occ[i];
                sqlx::query(
                    "INSERT INTO secret_scanning_locations (alert_id, commit_sha, blob_sha, path, start_line, end_line, start_column, end_column)
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING",
                )
                .bind(alert.0)
                .bind(&o.commit)
                .bind(&o.blob)
                .bind(&o.path)
                .bind(f.start_line)
                .bind(f.end_line)
                .bind(f.start_column)
                .bind(f.end_column)
                .execute(&state.db)
                .await?;
            }
        }
    }
    for id in created {
        super::secret_scanning_api::emit_webhook(state, ctx, id, "created").await;
    }
    Ok(())
}

/// Creates (or reopens nothing; resolved alerts stay resolved) the alert for a finding.
/// Returns (alert id, created).
async fn upsert_alert(state: &AppState, ctx: &RepoCtx, f: &Finding) -> anyhow::Result<(i64, bool)> {
    let hash = secret_hash(&f.secret);
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM secret_scanning_alerts WHERE repo_id = $1 AND secret_type = $2 AND secret_hash = $3")
        .bind(ctx.repo.id)
        .bind(f.secret_type)
        .bind(&hash)
        .fetch_optional(&state.db)
        .await?
    {
        return Ok((id, false));
    }
    let placeholder = placeholder_id(ctx.repo.id, f.secret_type, &f.secret);
    let bypass: Option<(Option<i64>, chrono::DateTime<chrono::Utc>, Option<i64>)> =
        sqlx::query_as("SELECT actor_id, created_at, request_id FROM secret_scanning_push_bypasses WHERE repo_id = $1 AND placeholder_id = $2")
            .bind(ctx.repo.id)
            .bind(&placeholder)
            .fetch_optional(&state.db)
            .await?;
    let mut tx = state.db.begin().await?;
    let number = super::next_number(&mut *tx, ctx.repo.id, "secret_scanning").await.map_err(|e| anyhow::anyhow!(e.message))?;
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO secret_scanning_alerts (repo_id, number, secret_type, secret, secret_hash, push_protection_bypassed,
                                             push_protection_bypassed_by, push_protection_bypassed_at, bypass_request_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT (repo_id, secret_type, secret_hash) DO NOTHING RETURNING id",
    )
    .bind(ctx.repo.id)
    .bind(number)
    .bind(f.secret_type)
    .bind(&f.secret)
    .bind(&hash)
    .bind(bypass.is_some())
    .bind(bypass.as_ref().and_then(|b| b.0))
    .bind(bypass.as_ref().map(|b| b.1))
    .bind(bypass.as_ref().and_then(|b| b.2))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = id else {
        tx.rollback().await?;
        let id = sqlx::query_scalar("SELECT id FROM secret_scanning_alerts WHERE repo_id = $1 AND secret_type = $2 AND secret_hash = $3")
            .bind(ctx.repo.id)
            .bind(f.secret_type)
            .bind(&hash)
            .fetch_one(&state.db)
            .await?;
        return Ok((id, false));
    };
    tx.commit().await?;
    if f.secret_type.starts_with("github_") {
        check_forge_token(state, ctx, id, &f.secret).await?;
    }
    Ok((id, true))
}

/// Validity of forge's own tokens, and auto-revocation in public repositories.
async fn check_forge_token(state: &AppState, ctx: &RepoCtx, alert_id: i64, secret: &str) -> anyhow::Result<()> {
    let hash = crate::auth::sha256_hex(secret);
    let owner: Option<i64> = sqlx::query_scalar("SELECT user_id FROM tokens WHERE token_hash = $1").bind(&hash).fetch_optional(&state.db).await?;
    let Some(user_id) = owner else {
        sqlx::query("UPDATE secret_scanning_alerts SET validity = 'inactive' WHERE id = $1").bind(alert_id).execute(&state.db).await?;
        return Ok(());
    };
    if ctx.repo.private {
        sqlx::query("UPDATE secret_scanning_alerts SET validity = 'active' WHERE id = $1").bind(alert_id).execute(&state.db).await?;
        return Ok(());
    }
    sqlx::query("DELETE FROM tokens WHERE token_hash = $1").bind(&hash).execute(&state.db).await?;
    sqlx::query(
        "UPDATE secret_scanning_alerts SET validity = 'inactive', state = 'resolved', resolution = 'revoked', resolved_at = now(),
                resolution_comment = 'forge revoked this token automatically because it was exposed in a public repository.', updated_at = now()
          WHERE id = $1",
    )
    .bind(alert_id)
    .execute(&state.db)
    .await?;
    tracing::warn!(repo = ctx.repo.id, user = user_id, "revoked a forge token exposed in a public repository");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Push protection

/// Token types push protection blocks for a repository: the patterns' defaults, overridden by
/// the enterprise and then the organization pattern configuration.
pub async fn protected_types(state: &AppState, ctx: &RepoCtx) -> HashSet<String> {
    let mut on: HashSet<String> = patterns::token_types().iter().filter(|p| p.push_protected).map(|p| p.slug.to_string()).collect();
    let mut rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT owner_kind, token_type, setting FROM secret_scanning_pattern_settings
          WHERE (owner_kind = 'enterprise' AND owner_id = 0) OR (owner_kind = 'org' AND owner_id = $1)",
    )
    .bind(ctx.owner.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    rows.sort_by_key(|(k, _, _)| if k == "enterprise" { 0 } else { 1 });
    for (_, t, setting) in rows {
        match setting.as_str() {
            "enabled" => {
                on.insert(t);
            }
            "disabled" => {
                on.remove(&t);
            }
            _ => {}
        }
    }
    on
}

async fn active_bypasses(state: &AppState, repo_id: i64) -> Vec<String> {
    sqlx::query_scalar("SELECT placeholder_id FROM secret_scanning_push_bypasses WHERE repo_id = $1 AND expire_at > now()")
        .bind(repo_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
}

/// A blocked secret.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Blocked {
    pub placeholder_id: String,
    pub token_type: String,
    pub display_name: String,
    pub commit: String,
    pub path: String,
    pub line: i32,
    pub branch: String,
}

async fn record_blocks(db: &sqlx::PgPool, repo_id: i64, actor: Option<i64>, blocks: &[Blocked]) {
    for b in blocks {
        let _ = sqlx::query(
            "INSERT INTO secret_scanning_push_blocks (repo_id, placeholder_id, token_type, path, branch, actor_id) VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT (repo_id, placeholder_id) DO UPDATE SET path = $4, branch = $5, actor_id = $6, created_at = now()",
        )
        .bind(repo_id)
        .bind(&b.placeholder_id)
        .bind(&b.token_type)
        .bind(&b.path)
        .bind(&b.branch)
        .bind(actor)
        .execute(db)
        .await;
    }
}

fn violation_message(base_url: &str, full_name: &str, refname: &str, blocks: &[Blocked]) -> String {
    let mut m = format!(
        "GH013: Repository rule violations found for {refname}.\n\n- GITHUB PUSH PROTECTION\n  —————————————————————————————————————————\n    Resolve the following violations before pushing again\n\n    - Push cannot contain secrets\n\n"
    );
    let mut seen = HashSet::new();
    for b in blocks {
        if !seen.insert(&b.placeholder_id) {
            continue;
        }
        m.push_str(&format!("      —— {} ——————————————————————————\n        locations:\n", b.display_name));
        for x in blocks.iter().filter(|x| x.placeholder_id == b.placeholder_id) {
            if x.commit.is_empty() {
                m.push_str(&format!("          - path: {}:{}\n", x.path, x.line));
            } else {
                m.push_str(&format!("          - commit: {}\n            path: {}:{}\n", x.commit, x.path, x.line));
            }
        }
        m.push_str(&format!(
            "\n        (?) To push, remove secret from commit(s) or follow this URL to allow the secret.\n        {base_url}/{full_name}/security/secret-scanning/unblock-secret/{}\n\n",
            b.placeholder_id
        ));
    }
    m
}

/// Contents API (and other in-process writes): 409 when `files` contain unbypassed secrets.
pub async fn check_api_push(state: &AppState, ctx: &RepoCtx, actor: &User, branch: &str, files: &[(&str, &[u8])]) -> ApiResult<()> {
    if !super::enabled(&state.db, ctx.repo.id, "secret_scanning").await || !super::enabled(&state.db, ctx.repo.id, "secret_scanning_push_protection").await {
        return Ok(());
    }
    let types = protected_types(state, ctx).await;
    let bypassed = active_bypasses(state, ctx.repo.id).await;
    let non_provider = super::enabled(&state.db, ctx.repo.id, "secret_scanning_non_provider_patterns").await;
    let mut blocks = Vec::new();
    for (path, data) in files {
        if data.len() > MAX_BLOB || data[..data.len().min(8000)].contains(&0) {
            continue;
        }
        for f in patterns::scan(&String::from_utf8_lossy(data), non_provider) {
            let pid = placeholder_id(ctx.repo.id, f.secret_type, &f.secret);
            if !types.contains(f.secret_type) || bypassed.contains(&pid) {
                continue;
            }
            let display = patterns::by_slug(f.secret_type).map(|p| p.display_name).unwrap_or(f.secret_type);
            blocks.push(Blocked { placeholder_id: pid, token_type: f.secret_type.into(), display_name: display.into(), commit: String::new(), path: path.to_string(), line: f.start_line, branch: branch.into() });
        }
    }
    if blocks.is_empty() {
        return Ok(());
    }
    record_blocks(&state.db, ctx.repo.id, Some(actor.id), &blocks).await;
    let mut e = ApiError::conflict(format!(
        "Repository rule violations found\n\n{}",
        violation_message(state.base_url(), &ctx.full_name(), &format!("refs/heads/{branch}"), &blocks)
    ));
    e.errors = Some(json!(blocks
        .iter()
        .map(|b| json!({ "resource": "SecretScanning", "field": b.path, "code": "secret_detected", "placeholder_id": b.placeholder_id, "token_type": b.token_type,
                          "message": format!("Push cannot contain secrets: {} at {}:{}", b.display_name, b.path, b.line) }))
        .collect::<Vec<_>>()));
    Err(e)
}

/// Rules handed to the `pre-receive` hook process.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct HookRules {
    pub repo_id: i64,
    pub base_url: String,
    pub full_name: String,
    pub git_bin: String,
    pub types: Vec<String>,
    pub bypassed: Vec<String>,
    pub non_provider: bool,
    pub found_path: PathBuf,
}

const HOOK: &str = r#"#!/bin/sh
# forge pre-receive (generated by crates/forge/src/advsec/secret_scanning.rs):
# branch protection first, then secret scanning push protection.
input=$(cat)
if [ -n "$FORGE_PROTECT_RULES" ] && [ -x "$FORGE_PROTECT_HOOK" ]; then
  printf '%s\n' "$input" | "$FORGE_PROTECT_HOOK" || exit 1
fi
if [ -n "$FORGE_ADVSEC_RULES" ] && [ -f "$FORGE_ADVSEC_RULES" ]; then
  printf '%s\n' "$input" | "$FORGE_ADVSEC_BIN" hook pre-receive || exit 1
fi
exit 0
"#;

/// The forge executable the hook runs (`FORGE_HOOK_BIN`, else this process if it is forge).
fn hook_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FORGE_HOOK_BIN") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let exe = std::env::current_exe().ok()?;
    (exe.file_name()?.to_str()? == "forge").then_some(exe)
}

/// Per-push hook configuration for push protection. Dropping it records the blocked secrets the
/// hook reported (so their placeholder ids can be bypassed) and removes the files.
pub struct PushGuard {
    hooks_dir: PathBuf,
    rules: PathBuf,
    found: PathBuf,
    bin: PathBuf,
    protect_hook: PathBuf,
    db: sqlx::PgPool,
    repo_id: i64,
    actor: i64,
}

impl PushGuard {
    /// Points `git receive-pack` at the combined hook. Call after the branch protection
    /// guard's `apply` (this one chains it).
    pub fn apply(&self, cmd: &mut tokio::process::Command) {
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.hooksPath")
            .env("GIT_CONFIG_VALUE_0", &self.hooks_dir)
            .env("FORGE_ADVSEC_RULES", &self.rules)
            .env("FORGE_ADVSEC_BIN", &self.bin)
            .env("FORGE_PROTECT_HOOK", &self.protect_hook);
    }
}

impl Drop for PushGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.rules);
        let found = std::fs::read_to_string(&self.found).unwrap_or_default();
        let _ = std::fs::remove_file(&self.found);
        let blocks: Vec<Blocked> = found.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        if blocks.is_empty() {
            return;
        }
        let (db, repo_id, actor) = (self.db.clone(), self.repo_id, self.actor);
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { record_blocks(&db, repo_id, Some(actor), &blocks).await });
        }
    }
}

/// Builds the push protection hook for a push, or None when push protection is off.
pub async fn push_guard(state: &AppState, ctx: &RepoCtx, pusher: &User) -> anyhow::Result<Option<PushGuard>> {
    if !super::enabled(&state.db, ctx.repo.id, "secret_scanning").await || !super::enabled(&state.db, ctx.repo.id, "secret_scanning_push_protection").await {
        return Ok(None);
    }
    let Some(bin) = hook_bin() else {
        tracing::warn!("push protection: no forge executable for the pre-receive hook (set FORGE_HOOK_BIN)");
        return Ok(None);
    };
    let hooks_dir = state.cfg.cache_dir.join(".forge-advsec-hooks");
    tokio::fs::create_dir_all(&hooks_dir).await?;
    let hook = hooks_dir.join("pre-receive");
    if tokio::fs::read_to_string(&hook).await.ok().as_deref() != Some(HOOK) {
        let tmp = hooks_dir.join(format!(".pre-receive.{}", crate::auth::random_string(8)));
        tokio::fs::write(&tmp, HOOK).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).await?;
        }
        tokio::fs::rename(&tmp, &hook).await?;
    }
    let tag = crate::auth::random_string(16);
    let rules_path = hooks_dir.join(format!("rules-{tag}.json"));
    let found = hooks_dir.join(format!("found-{tag}.jsonl"));
    let rules = HookRules {
        repo_id: ctx.repo.id,
        base_url: state.base_url().to_string(),
        full_name: ctx.full_name(),
        git_bin: state.cfg.git_bin.clone(),
        types: protected_types(state, ctx).await.into_iter().collect(),
        bypassed: active_bypasses(state, ctx.repo.id).await,
        non_provider: super::enabled(&state.db, ctx.repo.id, "secret_scanning_non_provider_patterns").await,
        found_path: found.clone(),
    };
    tokio::fs::write(&rules_path, serde_json::to_vec(&rules)?).await?;
    Ok(Some(PushGuard {
        hooks_dir,
        rules: rules_path,
        found,
        bin,
        protect_hook: state.cfg.cache_dir.join(".forge-hooks").join("pre-receive"),
        db: state.db.clone(),
        repo_id: ctx.repo.id,
        actor: pusher.id,
    }))
}

/// `forge hook pre-receive`: reads `<old> <new> <ref>` lines, scans the commits each push
/// introduces (`<new> --not --all`; the quarantined objects are visible to git), prints GH013
/// for unbypassed secrets of protected types and exits 1. No database access.
pub async fn hook_pre_receive() -> i32 {
    let Ok(rules_path) = std::env::var("FORGE_ADVSEC_RULES") else { return 0 };
    let Ok(raw) = std::fs::read(&rules_path) else { return 0 };
    let Ok(rules) = serde_json::from_slice::<HookRules>(&raw) else { return 0 };
    let mut input = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    let repo = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let git = GitCmd::new(rules.git_bin.clone());
    let types: HashSet<&str> = rules.types.iter().map(String::as_str).collect();
    let mut all_blocks = Vec::new();
    let mut messages = Vec::new();
    for line in input.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 3 || f[1] == crate::webhooks::ZERO_SHA || !crate::git::read::is_sha(f[1]) {
            continue;
        }
        let refname = f[2];
        let branch = refname.strip_prefix("refs/heads/").unwrap_or(refname);
        let revs = format!("{}\n--not\n--all\n", f[1]);
        let occ = match occurrences(&git, &repo, &revs, MAX_COMMITS_INCREMENTAL).await {
            Ok(o) => o,
            Err(e) => {
                eprintln!("warning: secret scanning could not inspect the push: {e}");
                continue;
            }
        };
        let found = match scan_occurrences(&git, &repo, &occ, rules.non_provider).await {
            Ok(f) => f,
            Err(_) => continue,
        };
        let mut blocks = Vec::new();
        for (idxs, findings) in found {
            for fd in findings {
                let pid = placeholder_id(rules.repo_id, fd.secret_type, &fd.secret);
                if !types.contains(fd.secret_type) || rules.bypassed.contains(&pid) {
                    continue;
                }
                let display = patterns::by_slug(fd.secret_type).map(|p| p.display_name).unwrap_or(fd.secret_type);
                for &i in &idxs {
                    blocks.push(Blocked {
                        placeholder_id: pid.clone(),
                        token_type: fd.secret_type.into(),
                        display_name: display.into(),
                        commit: occ[i].commit.clone(),
                        path: occ[i].path.clone(),
                        line: fd.start_line,
                        branch: branch.into(),
                    });
                }
            }
        }
        if !blocks.is_empty() {
            messages.push(violation_message(&rules.base_url, &rules.full_name, refname, &blocks));
            all_blocks.extend(blocks);
        }
    }
    if all_blocks.is_empty() {
        return 0;
    }
    let lines: String = all_blocks.iter().filter_map(|b| serde_json::to_string(b).ok()).map(|l| l + "\n").collect();
    let _ = std::fs::write(&rules.found_path, lines);
    for m in messages {
        for l in m.lines() {
            eprintln!("error: {l}");
        }
    }
    1
}

// ---------------------------------------------------------------------------------------------
// Bypasses

/// The token type a placeholder was blocked for.
pub async fn blocked_token_type(state: &AppState, repo_id: i64, placeholder: &str) -> ApiResult<Option<(String, String, String)>> {
    Ok(sqlx::query_as("SELECT token_type, path, branch FROM secret_scanning_push_blocks WHERE repo_id = $1 AND placeholder_id = $2")
        .bind(repo_id)
        .bind(placeholder)
        .fetch_optional(&state.db)
        .await?)
}

/// Allows a blocked secret for `BYPASS_HOURS`. Returns the expiry.
pub async fn grant_bypass(state: &AppState, repo_id: i64, placeholder: &str, token_type: &str, reason: &str, actor: Option<i64>, request_id: Option<i64>) -> ApiResult<chrono::DateTime<chrono::Utc>> {
    let exp: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO secret_scanning_push_bypasses (repo_id, placeholder_id, token_type, reason, actor_id, request_id, expire_at)
         VALUES ($1,$2,$3,$4,$5,$6, now() + make_interval(hours => $7))
         ON CONFLICT (repo_id, placeholder_id) DO UPDATE SET reason = $4, actor_id = $5, request_id = $6,
             expire_at = now() + make_interval(hours => $7), created_at = now()
         RETURNING expire_at",
    )
    .bind(repo_id)
    .bind(placeholder)
    .bind(token_type)
    .bind(reason)
    .bind(actor)
    .bind(request_id)
    .bind(BYPASS_HOURS as i32)
    .fetch_one(&state.db)
    .await?;
    // An alert that already exists for this secret records the bypass.
    let alerts: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, secret_type, secret FROM secret_scanning_alerts WHERE repo_id = $1 AND secret_type = $2")
        .bind(repo_id)
        .bind(token_type)
        .fetch_all(&state.db)
        .await?;
    for (id, t, secret) in alerts {
        if placeholder_id(repo_id, &t, &secret) == placeholder {
            sqlx::query(
                "UPDATE secret_scanning_alerts SET push_protection_bypassed = true, push_protection_bypassed_by = $2,
                        push_protection_bypassed_at = now(), bypass_request_id = $3, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(actor)
            .bind(request_id)
            .execute(&state.db)
            .await?;
        }
    }
    Ok(exp)
}

/// An approved bypass request grants its placeholder.
pub async fn grant_bypass_from_request(state: &AppState, ctx: &RepoCtx, req: &super::requests::RequestRow) -> ApiResult<()> {
    let token_type = req.data.pointer("/0/secret_type").and_then(Value::as_str).unwrap_or("").to_string();
    let reason = req.reason.clone().unwrap_or_else(|| "false_positive".into());
    grant_bypass(state, ctx.repo.id, &req.resource_identifier, &token_type, &reason, req.requester_id, Some(req.id)).await?;
    Ok(())
}

/// Scan counters for the scan-history endpoint.
pub async fn scans(state: &AppState, repo_id: i64) -> ApiResult<HashMap<String, Vec<Value>>> {
    let rows: Vec<(String, String, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT kind, status, started_at, completed_at FROM secret_scanning_scans WHERE repo_id = $1 ORDER BY id DESC LIMIT 100",
    )
    .bind(repo_id)
    .fetch_all(&state.db)
    .await?;
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    for (kind, status, started, completed) in rows {
        out.entry(kind).or_default().push(json!({
            "type": "git",
            "status": status,
            "started_at": crate::json::ts_opt(started),
            "completed_at": crate::json::ts_opt(completed),
        }));
    }
    Ok(out)
}
