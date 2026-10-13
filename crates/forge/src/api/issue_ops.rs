//! Issue operations GitHub only exposes through GraphQL (`transferIssue`, `pinIssue`/`unpinIssue`,
//! `createLinkedBranch`/`Issue.linkedBranches`), used by `gh issue transfer|pin|unpin|develop`.
//! Service layer only; the GraphQL adapters live in `graphql::ghcompat`.
use std::collections::HashSet;

use serde_json::json;

use crate::api::issues::{record_event, resolve_assignees, resolve_labels};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::json::issue::IssueRow;
use crate::pulls::custom_error;
use crate::state::AppState;

/// GitHub allows three pinned issues per repository.
pub const MAX_PINNED: i64 = 3;

/// Moves an issue to `dest` (same owner, like GitHub). The issue gets the next number in the
/// destination; comments, events, reactions and assignees who can be assigned there move along;
/// labels are matched by name (created when `create_labels_if_missing`, dropped otherwise); the
/// milestone and any pin are dropped. A `transferred` event is recorded on the issue.
pub async fn transfer_issue(
    state: &AppState,
    src: &RepoCtx,
    actor: &User,
    issue: &IssueRow,
    dest: &RepoCtx,
    create_labels_if_missing: bool,
) -> ApiResult<IssueRow> {
    if issue.is_pull {
        return Err(custom_error("Issue", "Pull requests cannot be transferred."));
    }
    src.require(Perm::Write)?;
    if dest.perm < Perm::Write {
        return Err(ApiError::forbidden(format!("{} does not have the correct permissions to execute `TransferIssue`", actor.login)));
    }
    if dest.repo.id == src.repo.id {
        return Err(custom_error("Issue", "The issue is already in this repository."));
    }
    if dest.owner.id != src.owner.id {
        return Err(custom_error("Issue", "Issues can only be transferred between repositories owned by the same user or organization."));
    }
    if !dest.repo.has_issues {
        return Err(custom_error("Issue", "Issues are disabled for the destination repository."));
    }
    if src.repo.private && !dest.repo.private {
        return Err(custom_error("Issue", "Issues cannot be transferred from a private repository to a public repository."));
    }

    let mut tx = state.db.begin().await?;
    let cur: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1 AND repo_id = $2 FOR UPDATE")
        .bind(issue.id)
        .bind(src.repo.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;

    // Labels by name in the destination.
    let names: Vec<String> = sqlx::query_scalar("SELECT l.name FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = $1")
        .bind(cur.id)
        .fetch_all(&mut *tx)
        .await?;
    let names = if create_labels_if_missing {
        names
    } else {
        let existing: HashSet<String> = sqlx::query_scalar("SELECT lower(name) FROM labels WHERE repo_id = $1")
            .bind(dest.repo.id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
        names.into_iter().filter(|n| existing.contains(&n.to_lowercase())).collect()
    };
    let labels = resolve_labels(&mut tx, dest.repo.id, &names, create_labels_if_missing).await?;
    sqlx::query("DELETE FROM issue_labels WHERE issue_id = $1").bind(cur.id).execute(&mut *tx).await?;
    for l in &labels {
        sqlx::query("INSERT INTO issue_labels (issue_id, label_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(cur.id)
            .bind(l.id)
            .execute(&mut *tx)
            .await?;
    }

    // Assignees who can't be assigned in the destination are dropped.
    let logins: Vec<String> = sqlx::query_scalar("SELECT u.login FROM issue_assignees a JOIN users u ON u.id = a.user_id WHERE a.issue_id = $1")
        .bind(cur.id)
        .fetch_all(&mut *tx)
        .await?;
    let keep: Vec<i64> = resolve_assignees(&mut tx, dest, &logins, false).await?.into_iter().map(|u| u.id).collect();
    sqlx::query("DELETE FROM issue_assignees WHERE issue_id = $1 AND NOT (user_id = ANY($2))")
        .bind(cur.id)
        .bind(&keep)
        .execute(&mut *tx)
        .await?;

    let number = db::next_issue_number(&mut *tx, dest.repo.id).await?;
    let row: IssueRow = sqlx::query_as(
        "UPDATE issues SET repo_id = $2, number = $3, milestone_id = NULL, updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(cur.id)
    .bind(dest.repo.id)
    .bind(number)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE issue_comments SET repo_id = $2 WHERE issue_id = $1")
        .bind(cur.id)
        .bind(dest.repo.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE issue_events SET repo_id = $2 WHERE issue_id = $1")
        .bind(cur.id)
        .bind(dest.repo.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM pinned_issues WHERE issue_id = $1").bind(cur.id).execute(&mut *tx).await?;
    // The open-issues trigger only watches `state`; move the count by hand.
    if row.state == "open" {
        sqlx::query("UPDATE repositories SET open_issues_count = GREATEST(open_issues_count - 1, 0) WHERE id = $1")
            .bind(src.repo.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE repositories SET open_issues_count = open_issues_count + 1 WHERE id = $1")
            .bind(dest.repo.id)
            .execute(&mut *tx)
            .await?;
    }
    record_event(
        &mut tx,
        dest.repo.id,
        row.id,
        Some(actor.id),
        "transferred",
        None,
        json!({ "from": src.full_name(), "from_number": cur.number }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

/// Whether the issue is pinned to its repository.
pub async fn is_pinned(state: &AppState, issue_id: i64) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pinned_issues WHERE issue_id = $1)")
        .bind(issue_id)
        .fetch_one(&state.db)
        .await?)
}

/// Pins an issue (triage access; at most [`MAX_PINNED`] per repository). Idempotent.
pub async fn pin_issue(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow) -> ApiResult<()> {
    ctx.require(Perm::Triage)?;
    if issue.is_pull {
        return Err(custom_error("Issue", "Pull requests cannot be pinned."));
    }
    let mut tx = state.db.begin().await?;
    // Serialize pinning per repository so the limit holds under concurrency.
    sqlx::query("SELECT id FROM repositories WHERE id = $1 FOR UPDATE").bind(ctx.repo.id).execute(&mut *tx).await?;
    let already: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pinned_issues WHERE issue_id = $1)")
        .bind(issue.id)
        .fetch_one(&mut *tx)
        .await?;
    if !already {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pinned_issues WHERE repo_id = $1").bind(ctx.repo.id).fetch_one(&mut *tx).await?;
        if n >= MAX_PINNED {
            return Err(custom_error("Issue", format!("You can only pin up to {MAX_PINNED} issues.")));
        }
        sqlx::query("INSERT INTO pinned_issues (issue_id, repo_id, pinned_by) VALUES ($1, $2, $3)")
            .bind(issue.id)
            .bind(ctx.repo.id)
            .bind(actor.id)
            .execute(&mut *tx)
            .await?;
        record_event(&mut tx, ctx.repo.id, issue.id, Some(actor.id), "pinned", None, json!({})).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Unpins an issue (triage access). Idempotent.
pub async fn unpin_issue(state: &AppState, ctx: &RepoCtx, actor: &User, issue: &IssueRow) -> ApiResult<()> {
    ctx.require(Perm::Triage)?;
    let mut tx = state.db.begin().await?;
    let removed = sqlx::query("DELETE FROM pinned_issues WHERE issue_id = $1").bind(issue.id).execute(&mut *tx).await?.rows_affected();
    if removed > 0 {
        record_event(&mut tx, ctx.repo.id, issue.id, Some(actor.id), "unpinned", None, json!({})).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Default name GitHub gives a branch created from an issue: `{number}-{slugified title}`.
pub fn default_branch_name(issue: &IssueRow) -> String {
    let mut slug = String::new();
    for c in issue.title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(60).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { issue.number.to_string() } else { format!("{}-{slug}", issue.number) }
}

/// A branch linked to an issue.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkedBranchRow {
    pub id: i64,
    pub issue_id: i64,
    pub repo_id: i64,
    pub ref_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Creates branch `name` (default: [`default_branch_name`]) at `oid` in `target` (the issue's
/// repository or another one the actor can push to) and links it to the issue.
pub async fn create_linked_branch(
    state: &AppState,
    issue_ctx: &RepoCtx,
    actor: &User,
    issue: &IssueRow,
    target: &RepoCtx,
    oid: &str,
    name: Option<&str>,
) -> ApiResult<LinkedBranchRow> {
    if issue.is_pull {
        return Err(custom_error("LinkedBranch", "Branches can only be linked to issues."));
    }
    issue_ctx.require(Perm::Read)?;
    target.require(Perm::Write)?;
    let short = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| default_branch_name(issue));
    let short = short.strip_prefix("refs/heads/").unwrap_or(&short).to_string();
    let refname = format!("refs/heads/{short}");
    crate::api::git_data::create_ref(state, target, actor, &refname, oid).await?;
    let row: LinkedBranchRow = sqlx::query_as(
        "INSERT INTO linked_branches (issue_id, repo_id, ref_name) VALUES ($1, $2, $3)
         ON CONFLICT (issue_id, repo_id, ref_name) DO UPDATE SET created_at = linked_branches.created_at RETURNING *",
    )
    .bind(issue.id)
    .bind(target.repo.id)
    .bind(&refname)
    .fetch_one(&state.db)
    .await?;
    Ok(row)
}

/// Branches linked to an issue that still exist, oldest first, with their current tip.
pub async fn linked_branches(state: &AppState, issue_id: i64) -> ApiResult<Vec<(LinkedBranchRow, String)>> {
    let rows: Vec<LinkedBranchRow> = sqlx::query_as("SELECT * FROM linked_branches WHERE issue_id = $1 ORDER BY id")
        .bind(issue_id)
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    let mut refs_by_repo = std::collections::HashMap::new();
    for r in rows {
        if !refs_by_repo.contains_key(&r.repo_id) {
            let (_, refs) = state.git.refs(r.repo_id).await?;
            refs_by_repo.insert(r.repo_id, refs);
        }
        if let Some(sha) = refs_by_repo[&r.repo_id].get(&r.ref_name).cloned() {
            out.push((r, sha));
        }
    }
    Ok(out)
}
