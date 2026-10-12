//! Extension point for features reacting to ref changes (pull request head updates,
//! refs/pull/N/head maintenance, webhooks...). Feature owners add calls here.
use crate::db::{RepoCtx, User};
use crate::state::AppState;

/// `changed` is (refname, old_sha, new_sha); zero sha means created/deleted.
pub async fn after_refs_changed(
    _state: &AppState,
    _ctx: &RepoCtx,
    _actor: Option<&User>,
    _changed: &[(String, String, String)],
) -> anyhow::Result<()> {
    crate::api::issues::on_refs_changed(_state, _ctx, _actor, _changed).await;
    Ok(())
}
