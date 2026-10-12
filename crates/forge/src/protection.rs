//! Branch protection enforcement. The configuration is stored by `api::repo_extras`
//! (`branch_protections.config`, GitHub's `PUT /branches/{b}/protection` body); this module
//! applies it to:
//! - git pushes (HTTP and SSH receive-pack) through a generated `pre-receive` hook
//!   (`push_guard`), with GitHub's `GH006` messages;
//! - API ref updates / deletes (`check_ref_update`, `check_ref_delete`), contents writes and
//!   `POST /merges` (`check_direct_write`);
//! - pull request merges (`check_pull_merge`) and the `blocked` merge state (`merge_blockers`).
//!
//! Admins bypass review / status-check requirements unless `enforce_admins` is set (like
//! GitHub); force-push and deletion rules apply to everyone.
use std::collections::HashSet;
use std::path::PathBuf;

use axum::http::StatusCode;
use serde_json::Value;
use sqlx::PgPool;

use crate::db::{Perm, RepoCtx};
use crate::error::{ApiError, ApiResult};
use crate::pulls::{MergeMethod, Pull};
use crate::state::AppState;

/// Stored protection for one branch.
#[derive(Debug, Clone)]
pub struct Rules {
    pub branch: String,
    pub cfg: Value,
}

fn flag(cfg: &Value, k: &str) -> bool {
    cfg.get(k).and_then(Value::as_bool).unwrap_or(false)
}

impl Rules {
    pub fn enforce_admins(&self) -> bool {
        flag(&self.cfg, "enforce_admins")
    }
    pub fn allow_force_pushes(&self) -> bool {
        flag(&self.cfg, "allow_force_pushes")
    }
    pub fn allow_deletions(&self) -> bool {
        flag(&self.cfg, "allow_deletions")
    }
    pub fn linear_history(&self) -> bool {
        flag(&self.cfg, "required_linear_history")
    }
    pub fn lock_branch(&self) -> bool {
        flag(&self.cfg, "lock_branch")
    }
    fn reviews(&self) -> Option<&Value> {
        self.cfg.get("required_pull_request_reviews").filter(|v| v.is_object())
    }
    /// Pull requests are required (direct pushes rejected).
    pub fn requires_pull_request(&self) -> bool {
        self.reviews().is_some()
    }
    pub fn required_approvals(&self) -> i64 {
        self.reviews().and_then(|r| r.get("required_approving_review_count")).and_then(Value::as_i64).unwrap_or(1).max(0)
    }
    pub fn dismiss_stale_reviews(&self) -> bool {
        self.reviews().is_some_and(|r| flag(r, "dismiss_stale_reviews"))
    }
    fn status_checks(&self) -> Option<&Value> {
        self.cfg.get("required_status_checks").filter(|v| v.is_object())
    }
    pub fn strict(&self) -> bool {
        self.status_checks().is_some_and(|s| flag(s, "strict"))
    }
    /// Required status contexts (`contexts` and `checks[].context`).
    pub fn required_contexts(&self) -> Vec<String> {
        let Some(sc) = self.status_checks() else { return vec![] };
        let mut out: Vec<String> = Vec::new();
        for c in sc.get("contexts").and_then(Value::as_array).into_iter().flatten() {
            if let Some(c) = c.as_str() {
                out.push(c.to_string());
            }
        }
        for c in sc.get("checks").and_then(Value::as_array).into_iter().flatten() {
            if let Some(c) = c.get("context").and_then(Value::as_str) {
                out.push(c.to_string());
            }
        }
        out.sort();
        out.dedup();
        out
    }
    /// Whether `perm` bypasses the review / status-check / lock requirements.
    pub fn bypass(&self, perm: Perm) -> bool {
        perm >= Perm::Admin && !self.enforce_admins()
    }
}

/// Names of the protected branches of a repository.
pub async fn protected_branches(db: &PgPool, repo_id: i64) -> HashSet<String> {
    sqlx::query_scalar::<_, String>("SELECT branch FROM branch_protections WHERE repo_id = $1")
        .bind(repo_id)
        .fetch_all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// The protection rules of `branch`, if it is protected.
pub async fn rules(db: &PgPool, repo_id: i64, branch: &str) -> ApiResult<Option<Rules>> {
    let cfg: Option<Value> = sqlx::query_scalar("SELECT config FROM branch_protections WHERE repo_id = $1 AND branch = $2")
        .bind(repo_id)
        .bind(branch)
        .fetch_optional(db)
        .await?;
    Ok(cfg.map(|cfg| Rules { branch: branch.to_string(), cfg }))
}

async fn all_rules(db: &PgPool, repo_id: i64) -> ApiResult<Vec<Rules>> {
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT branch, config FROM branch_protections WHERE repo_id = $1")
        .bind(repo_id)
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().map(|(branch, cfg)| Rules { branch, cfg }).collect())
}

/// Required contexts whose latest status on `sha` is not `success`.
pub async fn missing_contexts(db: &PgPool, repo_id: i64, sha: &str, contexts: &[String]) -> ApiResult<Vec<String>> {
    if contexts.is_empty() {
        return Ok(vec![]);
    }
    let ok: Vec<String> = sqlx::query_scalar(
        "SELECT context FROM (SELECT DISTINCT ON (context) context, state FROM commit_statuses
           WHERE repo_id = $1 AND sha = $2 AND context = ANY($3) ORDER BY context, id DESC) s WHERE state = 'success'",
    )
    .bind(repo_id)
    .bind(sha)
    .bind(contexts)
    .fetch_all(db)
    .await?;
    Ok(contexts.iter().filter(|c| !ok.contains(c)).cloned().collect())
}

/// GitHub's wording for unmet required status checks.
pub fn status_message(missing: &[String], total: usize) -> String {
    match missing {
        [one] if total == 1 => format!("Required status check \"{one}\" is expected."),
        _ => format!("{} of {total} required status checks are expected.", missing.len()),
    }
}

const PR_REQUIRED: &str = "Changes must be made through a pull request.";
const LOCKED: &str = "Cannot change this locked branch";
const NO_FORCE: &str = "Cannot force-push to this branch";
const NO_MERGES: &str = "This branch must not contain merge commits.";

/// Policy for a direct (non-PR) update of `branch` to `new_sha` by a caller with `perm`
/// (None = the new commit is not known yet, i.e. it will be created by the write).
/// Returns the violation message, if any.
pub async fn direct_update_violation(db: &PgPool, repo_id: i64, r: &Rules, perm: Perm, new_sha: Option<&str>) -> ApiResult<Option<String>> {
    if r.bypass(perm) {
        return Ok(None);
    }
    if r.lock_branch() {
        return Ok(Some(LOCKED.into()));
    }
    if r.requires_pull_request() {
        return Ok(Some(PR_REQUIRED.into()));
    }
    let contexts = r.required_contexts();
    if !contexts.is_empty() {
        let missing = match new_sha {
            Some(sha) => missing_contexts(db, repo_id, sha, &contexts).await?,
            None => contexts.clone(),
        };
        if !missing.is_empty() {
            return Ok(Some(status_message(&missing, contexts.len())));
        }
    }
    Ok(None)
}

fn violation(status: StatusCode, msg: &str) -> ApiError {
    ApiError::new(status, msg)
}

/// Contents writes and `POST /merges` creating a new commit on `branch` (409 on violation).
/// `merge_commit`: the write creates a merge commit (linear-history rule).
pub async fn check_direct_write(s: &AppState, ctx: &RepoCtx, branch: &str, merge_commit: bool) -> ApiResult<()> {
    let Some(r) = rules(&s.db, ctx.repo.id, branch).await? else { return Ok(()) };
    if let Some(msg) = direct_update_violation(&s.db, ctx.repo.id, &r, ctx.perm, None).await? {
        return Err(violation(StatusCode::CONFLICT, &msg));
    }
    if merge_commit && r.linear_history() {
        return Err(violation(StatusCode::CONFLICT, NO_MERGES));
    }
    Ok(())
}

/// `PATCH /git/refs/heads/{branch}` (422 on violation).
pub async fn check_ref_update(s: &AppState, ctx: &RepoCtx, refname: &str, new_sha: &str) -> ApiResult<()> {
    let Some(branch) = refname.strip_prefix("refs/heads/") else { return Ok(()) };
    let Some(r) = rules(&s.db, ctx.repo.id, branch).await? else { return Ok(()) };
    if let Some(msg) = direct_update_violation(&s.db, ctx.repo.id, &r, ctx.perm, Some(new_sha)).await? {
        return Err(violation(StatusCode::UNPROCESSABLE_ENTITY, &msg));
    }
    let need_ff = !r.allow_force_pushes();
    if need_ff || r.linear_history() {
        let path = s.git.sync(ctx.repo.id).await?;
        let git = &s.git.git;
        let (_, refs) = s.git.refs(ctx.repo.id).await?;
        if let Some(old) = refs.get(refname) {
            if need_ff && old != new_sha && !crate::git::read::is_ancestor(git, &path, old, new_sha).await.unwrap_or(false) {
                return Err(violation(StatusCode::UNPROCESSABLE_ENTITY, NO_FORCE));
            }
            if r.linear_history() {
                let range = format!("{old}..{new_sha}");
                let (ok, out, _) = git.try_run(&path, &["rev-list", "--merges", "-n", "1", &range, "--"]).await?;
                if ok && !out.is_empty() {
                    return Err(violation(StatusCode::UNPROCESSABLE_ENTITY, NO_MERGES));
                }
            }
        }
    }
    Ok(())
}

/// `DELETE /git/refs/heads/{branch}` (422 when deletions are not allowed).
pub async fn check_ref_delete(s: &AppState, ctx: &RepoCtx, refname: &str) -> ApiResult<()> {
    let Some(branch) = refname.strip_prefix("refs/heads/") else { return Ok(()) };
    let Some(r) = rules(&s.db, ctx.repo.id, branch).await? else { return Ok(()) };
    if !r.allow_deletions() {
        return Err(violation(StatusCode::UNPROCESSABLE_ENTITY, "Cannot delete this protected branch"));
    }
    Ok(())
}

/// Approvals counted for branch protection: users whose latest decisive review is APPROVED,
/// who have write access, excluding the author; with `dismiss_stale_reviews`, only approvals of
/// the current head commit count.
pub async fn approval_count(s: &AppState, ctx: &RepoCtx, pull: &Pull, r: &Rules) -> ApiResult<i64> {
    let latest: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT ON (user_id) user_id, state, commit_id FROM pull_reviews
          WHERE issue_id = $1 AND state IN ('APPROVED', 'CHANGES_REQUESTED', 'DISMISSED') AND user_id IS NOT NULL
          ORDER BY user_id, id DESC",
    )
    .bind(pull.issue.id)
    .fetch_all(&s.db)
    .await?;
    let mut n = 0;
    for (uid, st, commit) in latest {
        if st != "APPROVED" || Some(uid) == pull.issue.user_id {
            continue;
        }
        if r.dismiss_stale_reviews() && commit.as_deref() != Some(pull.pr.head_sha.as_str()) {
            continue;
        }
        let Ok(u) = crate::db::user_by_id(&s.db, uid).await else { continue };
        let perm = crate::db::permission(&s.db, Some(&u), &ctx.repo, &ctx.owner).await?;
        if perm >= Perm::Write {
            n += 1;
        }
    }
    Ok(n)
}

/// Why a pull request can't be merged into its protected base by a caller with `perm`
/// (None = no protection violation). `strict` checks need the git repository.
pub async fn merge_violation(s: &AppState, ctx: &RepoCtx, pull: &Pull, perm: Perm, method: Option<MergeMethod>) -> ApiResult<Option<String>> {
    let Some(r) = rules(&s.db, ctx.repo.id, &pull.pr.base_ref).await? else { return Ok(None) };
    if matches!(method, Some(MergeMethod::Merge)) && r.linear_history() {
        return Ok(Some("Merge commits are not allowed on this protected branch.".into()));
    }
    if r.bypass(perm) {
        return Ok(None);
    }
    if r.lock_branch() {
        return Ok(Some(LOCKED.into()));
    }
    if r.requires_pull_request() {
        let need = r.required_approvals();
        if need > 0 && approval_count(s, ctx, pull, &r).await? < need {
            let noun = if need == 1 { "review is" } else { "reviews are" };
            return Ok(Some(format!("At least {need} approving {noun} required by reviewers with write access.")));
        }
    }
    let contexts = r.required_contexts();
    if !contexts.is_empty() {
        let missing = missing_contexts(&s.db, ctx.repo.id, &pull.pr.head_sha, &contexts).await?;
        if !missing.is_empty() {
            return Ok(Some(status_message(&missing, contexts.len())));
        }
        if r.strict() {
            let path = crate::pulls::repo_for_reading(s, pull).await?;
            let (_, refs) = s.git.refs(ctx.repo.id).await?;
            if let Some(base) = refs.get(&format!("refs/heads/{}", pull.pr.base_ref)) {
                if !crate::git::read::is_ancestor(&s.git.git, &path, base, &pull.pr.head_sha).await.unwrap_or(false) {
                    return Ok(Some("Head branch is out of date. Update the branch and try again.".into()));
                }
            }
        }
    }
    Ok(None)
}

/// 405 when the merge is blocked by the base branch's protection.
pub async fn check_pull_merge(s: &AppState, ctx: &RepoCtx, pull: &Pull, method: MergeMethod) -> ApiResult<()> {
    match merge_violation(s, ctx, pull, ctx.perm, Some(method)).await? {
        Some(msg) => Err(ApiError::new(StatusCode::METHOD_NOT_ALLOWED, msg)),
        None => Ok(()),
    }
}

/// (blocked, review_required) for the PR status shown to everyone (GitHub computes these
/// without admin bypass).
pub async fn merge_blockers(s: &AppState, pull: &Pull) -> ApiResult<(bool, bool)> {
    let Some(r) = rules(&s.db, pull.pr.repo_id, &pull.pr.base_ref).await? else { return Ok((false, false)) };
    let repo = crate::db::repo_by_id(&s.db, pull.pr.repo_id).await?;
    let owner = crate::db::user_by_id(&s.db, repo.owner_id).await?;
    let ctx = RepoCtx { repo, owner, perm: Perm::Write };
    let review_required = r.requires_pull_request() && approval_count(s, &ctx, pull, &r).await? < r.required_approvals();
    let blocked = merge_violation(s, &ctx, pull, Perm::Write, None).await?.is_some();
    Ok((blocked, review_required))
}

// ---------------------------------------------------------------------------------------------
// Git pushes: pre-receive hook

/// The hook. Reads `$FORGE_PROTECT_RULES`, lines of
/// `r <ref> <flags>` (d = no deletions, f = no force pushes, l = linear history,
/// p = pull request required, s = status checks required, k = locked),
/// `s <ref> <message>` (status-check message) and `o <ref> <sha>` (shas satisfying the checks).
const HOOK: &str = r#"#!/bin/sh
# forge branch protection (generated by crates/forge/src/protection.rs)
rules="$FORGE_PROTECT_RULES"
[ -n "$rules" ] && [ -f "$rules" ] || exit 0
z=0000000000000000000000000000000000000000
rc=0
while read -r old new ref; do
  flags=""; smsg=""; okshas=" "
  while read -r kind r rest; do
    [ "$r" = "$ref" ] || continue
    case "$kind" in
      r) flags="$rest" ;;
      s) smsg="$rest" ;;
      o) okshas="$okshas$rest " ;;
    esac
  done < "$rules"
  [ -n "$flags" ] || continue
  fail=""
  if [ "$new" = "$z" ]; then
    case "$flags" in *d*) fail="Cannot delete this branch" ;; esac
  else
    case "$flags" in *k*) fail="Cannot change this locked branch" ;; esac
    if [ -z "$fail" ]; then case "$flags" in *p*) fail="Changes must be made through a pull request." ;; esac; fi
    if [ -z "$fail" ]; then case "$flags" in *s*)
      case "$okshas" in *" $new "*) ;; *) fail="$smsg" ;; esac ;; esac; fi
    if [ -z "$fail" ] && [ "$old" != "$z" ]; then case "$flags" in *f*)
      git merge-base --is-ancestor "$old" "$new" || fail="Cannot force-push to this branch" ;; esac; fi
    if [ -z "$fail" ]; then case "$flags" in *l*)
      if [ "$old" = "$z" ]; then range="$new"; else range="$old..$new"; fi
      [ -z "$(git rev-list --merges -n 1 "$range" --)" ] || fail="This branch must not contain merge commits." ;; esac; fi
  fi
  if [ -n "$fail" ]; then
    echo "error: GH006: Protected branch update failed for $ref." >&2
    echo "error: $fail" >&2
    rc=1
  fi
done
exit $rc
"#;

/// Per-push hook configuration; the rules file is removed on drop.
pub struct PushGuard {
    hooks_dir: PathBuf,
    rules: PathBuf,
}

impl PushGuard {
    /// Points `git receive-pack` at the protection hook.
    pub fn apply(&self, cmd: &mut tokio::process::Command) {
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.hooksPath")
            .env("GIT_CONFIG_VALUE_0", &self.hooks_dir)
            .env("FORGE_PROTECT_RULES", &self.rules);
    }
}

impl Drop for PushGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.rules);
    }
}

/// Ref names can't contain whitespace, but be defensive: such a line would confuse the hook.
fn safe(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c.is_whitespace())
}

/// Builds the hook configuration for a push to `ctx` by a caller with `ctx.perm`, or None when
/// the repository has no protected branches.
pub async fn push_guard(s: &AppState, ctx: &RepoCtx) -> anyhow::Result<Option<PushGuard>> {
    let all = all_rules(&s.db, ctx.repo.id).await.map_err(|e| anyhow::anyhow!(e.message))?;
    if all.is_empty() {
        return Ok(None);
    }
    let mut text = String::new();
    for r in &all {
        let refname = format!("refs/heads/{}", r.branch);
        if !safe(&refname) {
            continue;
        }
        let mut flags = String::from("-");
        if !r.allow_deletions() {
            flags.push('d');
        }
        if !r.allow_force_pushes() {
            flags.push('f');
        }
        if r.linear_history() {
            flags.push('l');
        }
        if !r.bypass(ctx.perm) {
            if r.lock_branch() {
                flags.push('k');
            }
            if r.requires_pull_request() {
                flags.push('p');
            }
            let contexts = r.required_contexts();
            if !contexts.is_empty() {
                flags.push('s');
                let n = contexts.len() as i64;
                text.push_str(&format!("s {refname} {}\n", status_message(&contexts, contexts.len()).replace('\n', " ")));
                // Commits that already satisfy every required context may be pushed directly.
                let shas: Vec<String> = sqlx::query_scalar(
                    "SELECT sha FROM (SELECT DISTINCT ON (sha, context) sha, context, state FROM commit_statuses
                       WHERE repo_id = $1 AND context = ANY($2) ORDER BY sha, context, id DESC) s
                     WHERE state = 'success' GROUP BY sha HAVING count(*) = $3 LIMIT 5000",
                )
                .bind(ctx.repo.id)
                .bind(&contexts)
                .bind(n)
                .fetch_all(&s.db)
                .await?;
                for sha in shas.iter().filter(|x| safe(x)) {
                    text.push_str(&format!("o {refname} {sha}\n"));
                }
            }
        }
        text.push_str(&format!("r {refname} {flags}\n"));
    }
    let hooks_dir = s.cfg.cache_dir.join(".forge-hooks");
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
    let rules = hooks_dir.join(format!("rules-{}", crate::auth::random_string(16)));
    tokio::fs::write(&rules, text).await?;
    Ok(Some(PushGuard { hooks_dir, rules }))
}
