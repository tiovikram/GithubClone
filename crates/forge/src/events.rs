//! Post-write processing: things that must happen after refs change (push or API writes).
use crate::db::{RepoCtx, User};
use crate::git::RefUpdates;
use crate::state::AppState;

/// Called after a successful push has been committed.
pub async fn on_push(state: &AppState, ctx: &RepoCtx, pusher: &User, updates: &RefUpdates) -> anyhow::Result<()> {
    let changed = updates.changed();
    if changed.is_empty() {
        return Ok(());
    }
    sqlx::query("UPDATE repositories SET pushed_at = now(), updated_at = now() WHERE id = $1")
        .bind(ctx.repo.id)
        .execute(&state.db)
        .await?;

    // First push into an empty repo, or default branch missing: point HEAD at a pushed branch,
    // preferring the configured default, then main/master (GitHub behaviour).
    let branches: Vec<&String> = updates.after.keys().filter(|r| r.starts_with("refs/heads/")).collect();
    let default_ref = format!("refs/heads/{}", ctx.repo.default_branch);
    if !branches.is_empty() && !updates.after.contains_key(&default_ref) {
        let pick = ["refs/heads/main", "refs/heads/master"]
            .iter()
            .find(|c| updates.after.contains_key(**c))
            .map(|s| s.to_string())
            .unwrap_or_else(|| branches[0].clone());
        let branch = pick.trim_start_matches("refs/heads/").to_string();
        set_default_branch(state, ctx.repo.id, &branch).await?;
    }
    crate::hooks::after_refs_changed(state, ctx, Some(pusher), &changed).await?;
    Ok(())
}

/// Changes the default branch (HEAD symref + repositories.default_branch).
pub async fn set_default_branch(state: &AppState, repo_id: i64, branch: &str) -> anyhow::Result<()> {
    let git = state.git.git.clone();
    let target = format!("refs/heads/{branch}");
    state
        .git
        .write(repo_id, |path| async move {
            git.run(&path, &["symbolic-ref", "HEAD", &target]).await?;
            Ok(())
        })
        .await?;
    sqlx::query("UPDATE repositories SET default_branch = $2, updated_at = now() WHERE id = $1")
        .bind(repo_id)
        .bind(branch)
        .execute(&state.db)
        .await?;
    Ok(())
}
