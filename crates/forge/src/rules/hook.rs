//! Ruleset evaluation of `git push`es.
//!
//! The pushed objects only exist in receive-pack's quarantine while its `pre-receive` hook
//! runs, so rules are checked from that hook: the branch-protection hook (`protection.rs`)
//! writes a request file into a per-push directory (`FORGE_RULESET_DIR`) with its working
//! directory, git's quarantine variables and the `<old> <new> <ref>` lines, then waits for a
//! response file. The server (which spawned receive-pack, so it is the same pod) polls the
//! directory, evaluates the updates in-process with full database access ([`super::evaluate`],
//! reading objects through the quarantine environment), records rule suites and bypass requests
//! and answers `ok` or `reject` followed by the text the hook prints (GH013).
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Actor, GitView, RefUpdate, Via};
use crate::db::RepoCtx;
use crate::state::AppState;

/// One push's ruleset channel; dropping it stops the poller and removes the directory.
pub struct RulesetHook {
    pub dir: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RulesetHook {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Starts the channel for a push to `ctx`, or None when no ruleset applies to the repository.
pub async fn start(s: &AppState, ctx: &RepoCtx, actor: Actor, root: &Path) -> anyhow::Result<Option<RulesetHook>> {
    let sets = super::repo_rulesets(s, ctx).await.map_err(|e| anyhow::anyhow!(e.message))?;
    if sets.is_empty() {
        return Ok(None);
    }
    let dir = root.join(format!("rs-{}", crate::auth::random_string(16)));
    tokio::fs::create_dir_all(&dir).await?;
    let (s2, ctx2, dir2) = (s.clone(), ctx.clone(), dir.clone());
    let task = tokio::spawn(async move {
        let req = dir2.join("req");
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if tokio::fs::try_exists(&req).await.unwrap_or(false) {
                let text = tokio::fs::read_to_string(&req).await.unwrap_or_default();
                let resp = answer(&s2, &ctx2, &actor, &text).await;
                let tmp = dir2.join("resp.tmp");
                if tokio::fs::write(&tmp, resp).await.is_ok() {
                    let _ = tokio::fs::rename(&tmp, dir2.join("resp")).await;
                }
                return;
            }
        }
    });
    Ok(Some(RulesetHook { dir, task }))
}

fn absolute(cwd: &Path, v: &str) -> String {
    if v.is_empty() || Path::new(v).is_absolute() {
        v.to_string()
    } else {
        cwd.join(v).display().to_string()
    }
}

/// Parses the hook's request and evaluates it.
async fn answer(s: &AppState, ctx: &RepoCtx, actor: &Actor, text: &str) -> String {
    let mut cwd = PathBuf::from(".");
    let mut env: Vec<(String, String)> = Vec::new();
    let mut updates = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("cwd ") {
            cwd = PathBuf::from(v);
            continue;
        }
        let var = if let Some(v) = line.strip_prefix("objdir ") {
            Some(("GIT_OBJECT_DIRECTORY", v))
        } else if let Some(v) = line.strip_prefix("alt ") {
            Some(("GIT_ALTERNATE_OBJECT_DIRECTORIES", v))
        } else if let Some(v) = line.strip_prefix("quarantine ") {
            Some(("GIT_QUARANTINE_PATH", v))
        } else {
            None
        };
        if let Some((k, v)) = var {
            if !v.is_empty() {
                env.push((k.to_string(), v.to_string()));
            }
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() == 3 && crate::git::read::is_sha(f[0]) && crate::git::read::is_sha(f[1]) {
            updates.push(RefUpdate { old: f[0].to_string(), new: f[1].to_string(), refname: f[2].to_string() });
        }
    }
    let env = env
        .into_iter()
        .map(|(k, v)| {
            let v = if k == "GIT_ALTERNATE_OBJECT_DIRECTORIES" {
                v.split(':').map(|p| absolute(&cwd, p)).collect::<Vec<_>>().join(":")
            } else {
                absolute(&cwd, &v)
            };
            (k, v)
        })
        .collect();
    let view = GitView { bin: s.cfg.git_bin.clone(), path: cwd, env };
    match super::evaluate(s, ctx, actor, Some(&view), &updates, Via::Push, None, true).await {
        Ok(outcome) => {
            let msg = outcome.push_message(s, ctx);
            format!("{}\n{msg}", if outcome.rejected() { "reject" } else { "ok" })
        }
        Err(e) => {
            tracing::error!(error = %e.message, repo = %ctx.full_name(), "ruleset evaluation failed");
            "reject\nerror: GH013: repository rules could not be evaluated; try again later.\n".to_string()
        }
    }
}
