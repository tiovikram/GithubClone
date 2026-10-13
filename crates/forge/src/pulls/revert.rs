//! Reverting a merged pull request (GraphQL `revertPullRequest`, `gh pr revert`): a new branch
//! `revert-{n}-{head}` off the current base with one commit undoing the PR's net change
//! (`merge_base..head`, which is correct whatever the merge method was), and a PR for it.
use super::git::{self, RawIdent};
use super::{create_pull, custom_error, halt, identity, pull_by_number, unhalt, CreatePull, Pull};
use crate::db::{Perm, RepoCtx, User};
use crate::error::ApiResult;
use crate::git::plumbing;
use crate::state::AppState;

#[derive(Debug, Clone, Default)]
pub struct RevertInput {
    /// Default: `Revert "<title>"`.
    pub title: Option<String>,
    /// Default: `Reverts owner/repo#N`.
    pub body: Option<String>,
    pub draft: bool,
}

/// Opens a pull request reverting merged PR `number`. Requires write access.
pub async fn revert_pull(state: &AppState, ctx: &RepoCtx, actor: &User, number: i64, input: RevertInput) -> ApiResult<Pull> {
    ctx.require(Perm::Write)?;
    let pull = pull_by_number(&state.db, ctx.repo.id, number).await?;
    if !pull.pr.merged {
        return Err(custom_error("PullRequest", "Only merged pull requests can be reverted."));
    }
    let title = input.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| format!("Revert \"{}\"", pull.issue.title));
    let body = input.body.unwrap_or_else(|| format!("Reverts {}#{number}", ctx.full_name()));
    let base_ref = pull.pr.base_ref.clone();
    let wanted = format!("revert-{number}-{}", pull.pr.head_ref);
    let git_cmd = state.git.git.clone();
    let who: RawIdent = identity(actor);
    let p = pull.clone();
    let commit_msg = format!(
        "Revert \"{}\"\n\nThis reverts pull request #{number}{}.\n",
        p.issue.title,
        p.pr.merge_commit_sha.as_deref().map(|m| format!(" (commit {m})")).unwrap_or_default()
    );
    let (branch, updates) = state
        .git
        .write(ctx.repo.id, |path| async move {
            let git = &git_cmd;
            let base_sha = plumbing::rev_parse(git, &path, &format!("refs/heads/{}", p.pr.base_ref))
                .await
                .ok_or_else(|| halt(custom_error("PullRequest", "Base branch was deleted")))?;
            let head = p.pr.head_sha.clone();
            if !git::has_commit(git, &path, &head).await {
                return Err(halt(custom_error("PullRequest", "The pull request's commits are no longer available.")));
            }
            let before = match &p.pr.merge_base_sha {
                Some(mb) => mb.clone(),
                None => git::merge_base(git, &path, &p.pr.base_sha, &head)
                    .await?
                    .ok_or_else(|| halt(custom_error("PullRequest", "Could not determine the changes to revert.")))?,
            };
            // Three-way merge: current base ("ours") with the pre-PR state ("theirs"), using the
            // PR head as merge base, i.e. the current base minus the PR's changes.
            let tree = git::merge_tree(git, &path, &base_sha, &before, Some(&head))
                .await?
                .ok_or_else(|| halt(custom_error("PullRequest", "This pull request can't be reverted automatically: merge conflict.")))?;
            let sha = git::commit_tree(git, &path, &tree, &[&base_sha], &commit_msg, &who, &who).await?;
            let mut branch = wanted.clone();
            let mut i = 1;
            while plumbing::rev_parse(git, &path, &format!("refs/heads/{branch}")).await.is_some() {
                branch = format!("{wanted}-{i}");
                i += 1;
            }
            plumbing::update_ref(git, &path, &format!("refs/heads/{branch}"), &sha, None).await?;
            Ok(branch)
        })
        .await
        .map_err(unhalt)?;
    crate::hooks::after_refs_changed(state, ctx, Some(actor), &updates.changed()).await?;
    create_pull(
        state,
        ctx,
        actor,
        CreatePull { title: Some(title), body: Some(body), head: branch, base: base_ref, draft: Some(input.draft), ..Default::default() },
    )
    .await
}
