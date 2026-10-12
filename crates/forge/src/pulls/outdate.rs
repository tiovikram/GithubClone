//! Review comments follow the diff when a pull request's head (or merge base) moves, like
//! GitHub: `original_*` fields never change; `line`/`start_line`/`position`/`commit_id` are
//! re-mapped onto the new head when the commented lines are untouched, and `line`/`position`
//! become null ("outdated") when the commented lines changed or left the diff.
use std::collections::HashMap;
use std::path::Path;

use super::git::{self, EMPTY_TREE};
use super::reviews::ReviewCommentRow;
use crate::git::cmd::GitCmd;
use crate::state::AppState;

/// `-a,b` / `+c,d` of a hunk header: ((old_start, old_count), (new_start, new_count)).
fn hunk_ranges(h: &str) -> ((i64, i64), (i64, i64)) {
    let parse = |v: &str| {
        let mut it = v.split(',');
        let start = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let count = it.next().map(|x| x.parse().unwrap_or(1)).unwrap_or(1);
        (start, count)
    };
    let (mut old, mut new) = ((0, 0), (0, 0));
    for part in h.split_whitespace().skip(1) {
        if let Some(v) = part.strip_prefix('-') {
            old = parse(v);
        } else if let Some(v) = part.strip_prefix('+') {
            new = parse(v);
            break;
        }
    }
    (old, new)
}

/// Maps `line` of the old file through a unified diff `patch` (old -> new). None when the line
/// itself was removed or modified.
pub fn map_line(patch: &str, line: i64) -> Option<i64> {
    let mut delta = 0i64;
    let mut old = 0i64;
    for l in patch.split('\n') {
        match l.chars().next() {
            Some('@') => {
                let ((start, count), _) = hunk_ranges(l);
                // A pure insertion (`-a,0`) goes after old line `a`.
                old = if count == 0 { start + 1 } else { start };
            }
            Some('+') => {
                if old <= line {
                    delta += 1;
                }
            }
            Some('-') => {
                if old == line {
                    return None;
                }
                if old < line {
                    delta -= 1;
                }
                old += 1;
            }
            Some('\\') => {}
            _ => old += 1,
        }
    }
    Some(line + delta)
}

/// Patch of `path` between two revisions, "" when the file is unchanged, None when it is gone
/// on the `to` side (deleted/renamed away) or the diff failed.
async fn file_patch(g: &GitCmd, repo: &Path, from: &str, to: &str, path: &str) -> Option<String> {
    if from == to {
        return Some(String::new());
    }
    let files = git::diff(g, repo, from, to, Some(path)).await.ok()?;
    match files.into_iter().find(|f| f.old_path.as_deref() == Some(path) || f.filename() == path) {
        None => Some(String::new()),
        Some(f) if f.new_path.as_deref() != Some(path) => None,
        Some(f) => Some(f.patch),
    }
}

/// New placement of a comment: (line, start_line, position), or None when outdated.
type Placed = Option<(Option<i64>, Option<i64>, Option<i64>)>;

/// Re-maps the current (non-outdated) review comments of PR `issue_id` from their commit onto
/// `new_head`. `old_mb`/`new_mb` are the merge bases before/after (LEFT-side comments live on
/// the merge base). Errors are logged; comments are left alone on git failures.
pub async fn follow_head(state: &AppState, base_repo_id: i64, issue_id: i64, old_mb: Option<&str>, new_mb: Option<&str>, new_head: &str) {
    if let Err(e) = follow_head_inner(state, base_repo_id, issue_id, old_mb, new_mb, new_head).await {
        tracing::warn!(error = %format!("{e:#}"), issue_id, "re-mapping review comments failed");
    }
}

async fn follow_head_inner(state: &AppState, base_repo_id: i64, issue_id: i64, old_mb: Option<&str>, new_mb: Option<&str>, new_head: &str) -> anyhow::Result<()> {
    let comments: Vec<ReviewCommentRow> = sqlx::query_as(
        "SELECT * FROM review_comments WHERE issue_id = $1 AND commit_id <> $2 AND (line IS NOT NULL OR subject_type = 'file')",
    )
    .bind(issue_id)
    .bind(new_head)
    .fetch_all(&state.db)
    .await?;
    if comments.is_empty() {
        return Ok(());
    }
    let repo = state.git.sync(base_repo_id).await?;
    let g = &state.git.git;
    let old_base = old_mb.unwrap_or(EMPTY_TREE);
    let new_base = new_mb.unwrap_or(EMPTY_TREE);
    // The PR diff at the new head, per path (None = path not in the diff).
    let mut pr_patch: HashMap<String, Option<String>> = HashMap::new();
    for c in comments {
        if !pr_patch.contains_key(&c.path) {
            let files = git::diff(g, &repo, new_base, new_head, Some(&c.path)).await.unwrap_or_default();
            let p = files.into_iter().find(|f| f.filename() == c.path || f.old_path.as_deref() == Some(c.path.as_str())).map(|f| f.patch);
            pr_patch.insert(c.path.clone(), p);
        }
        let Some(cur) = pr_patch.get(&c.path).cloned().flatten() else {
            mark(state, &c, None, new_head).await?;
            continue;
        };
        if c.subject_type == "file" {
            mark(state, &c, Some((None, None, None)), new_head).await?;
            continue;
        }
        if !git::has_commit(g, &repo, &c.commit_id).await {
            continue;
        }
        let left = c.side == "LEFT";
        // Diff that moves the commented file version to its new version.
        let (from, to) = if left { (old_base, new_base) } else { (c.commit_id.as_str(), new_head) };
        let Some(mv) = file_patch(g, &repo, from, to, &c.path).await else {
            mark(state, &c, None, new_head).await?;
            continue;
        };
        let placed: Placed = (|| {
            let line = map_line(&mv, c.line?)?;
            let start = match c.start_line {
                Some(s) => {
                    let start_left = c.start_side.as_deref().map(|x| x == "LEFT").unwrap_or(left);
                    // Mixed-side ranges: only re-map when both ends are on the same side.
                    if start_left != left {
                        return None;
                    }
                    let ns = map_line(&mv, s)?;
                    git::find_line(&cur, ns, if left { "LEFT" } else { "RIGHT" })?;
                    Some(ns)
                }
                None => None,
            };
            let pl = git::find_line(&cur, line, if left { "LEFT" } else { "RIGHT" })?;
            Some((Some(line), start, Some(pl.position)))
        })();
        mark(state, &c, placed, new_head).await?;
    }
    Ok(())
}

async fn mark(state: &AppState, c: &ReviewCommentRow, placed: Placed, new_head: &str) -> anyhow::Result<()> {
    match placed {
        Some((line, start, position)) => {
            sqlx::query("UPDATE review_comments SET line = $2, start_line = $3, position = $4, commit_id = $5 WHERE id = $1")
                .bind(c.id)
                .bind(line)
                .bind(start)
                .bind(position)
                .bind(new_head)
                .execute(&state.db)
                .await?;
        }
        None => {
            sqlx::query("UPDATE review_comments SET line = NULL, start_line = NULL, position = NULL WHERE id = $1")
                .bind(c.id)
                .execute(&state.db)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::map_line;

    #[test]
    fn maps_lines_through_hunks() {
        // old: 1..10; insert 2 lines after 2, change line 5, delete line 8.
        let p = "@@ -2,0 +3,2 @@\n+n1\n+n2\n@@ -5 +7 @@\n-five\n+FIVE\n@@ -8 +9,0 @@\n-eight";
        assert_eq!(map_line(p, 1), Some(1));
        assert_eq!(map_line(p, 2), Some(2));
        assert_eq!(map_line(p, 3), Some(5));
        assert_eq!(map_line(p, 4), Some(6));
        assert_eq!(map_line(p, 5), None);
        assert_eq!(map_line(p, 6), Some(8));
        assert_eq!(map_line(p, 8), None);
        assert_eq!(map_line(p, 9), Some(10));
        assert_eq!(map_line("", 7), Some(7));
    }
}
