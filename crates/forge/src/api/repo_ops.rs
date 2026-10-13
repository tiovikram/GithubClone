//! Repository operations GitHub exposes as dedicated GraphQL mutations
//! (`archiveRepository`/`unarchiveRepository`, used by `gh repo archive|unarchive`).
//! REST does the same through `PATCH /repos/{o}/{r}` with `archived`.
use crate::db::{Perm, Repo, RepoCtx, User};
use crate::error::ApiResult;
use crate::state::AppState;

/// Archives or unarchives a repository (admin only).
pub async fn set_archived(state: &AppState, ctx: &RepoCtx, actor: &User, archived: bool) -> ApiResult<RepoCtx> {
    let _ = actor;
    ctx.require(Perm::Admin)?;
    let r: Repo = sqlx::query_as("UPDATE repositories SET archived = $2, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(ctx.repo.id)
        .bind(archived)
        .fetch_one(&state.db)
        .await?;
    Ok(RepoCtx { repo: r, owner: ctx.owner.clone(), perm: ctx.perm })
}
