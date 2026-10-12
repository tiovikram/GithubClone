//! Extension point for features reacting to ref changes (pull request head updates,
//! refs/pull/N/head maintenance, webhooks...). Feature owners add calls here.
use crate::db::{RepoCtx, User};
use crate::state::AppState;

/// `changed` is (refname, old_sha, new_sha); zero sha means created/deleted.
pub async fn after_refs_changed(
    state: &AppState,
    ctx: &RepoCtx,
    actor: Option<&User>,
    changed: &[(String, String, String)],
) -> anyhow::Result<()> {
    if !changed.is_empty() {
        // Repository size/language, recomputed in the background.
        crate::git::stats::schedule_update(state, ctx.repo.id);
    }
    crate::pulls::on_refs_changed(state, ctx, actor, changed).await?;
    crate::api::issues::on_refs_changed(state, ctx, actor, changed).await;
    Ok(())
}
