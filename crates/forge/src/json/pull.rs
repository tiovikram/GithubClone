//! Pull requests, reviews, review comments, PR commits and files: batched loaders and GitHub
//! JSON builders (`pull-request`, `pull-request-simple`, `pull-request-review`,
//! `pull-request-review-comment`, `review-comment`, `commit`, `diff-entry`).
use std::collections::HashMap;

use serde_json::{json, Value};
use sqlx::PgPool;

use super::issue::{self, IssueFull};
use super::{ts, ts_opt};
use crate::db::{self, RepoCtx, User};
use crate::error::ApiResult;
use crate::ids;
use crate::pulls::git::{CommitInfo, FileDiff};
use crate::pulls::reviews::{ReviewCommentRow, ReviewRow};
use crate::pulls::{Pull, PullRow, PullStatus};
use crate::state::AppState;

/// Everything needed to render a pull request.
#[derive(Debug, Clone)]
pub struct PullFull {
    pub issue: IssueFull,
    pub pr: PullRow,
    /// None when the head repository was deleted or is invisible to the viewer.
    pub head_repo: Option<RepoCtx>,
    pub merged_by: Option<User>,
    pub requested_reviewers: Vec<User>,
    pub review_comments: i64,
    /// Present for the full `pull-request` shape.
    pub status: Option<PullStatus>,
}

/// Batch-loads labels/assignees/milestones (via `issue::load_full`), head repos, reviewers,
/// review comment counts, and (when `with_status`) mergeability for many PRs.
pub async fn load(state: &AppState, ctx: &RepoCtx, viewer: Option<&User>, pulls: Vec<Pull>, with_status: bool) -> ApiResult<Vec<PullFull>> {
    let db = &state.db;
    let issue_ids: Vec<i64> = pulls.iter().map(|p| p.issue.id).collect();
    let issues = issue::load_full(db, ctx, pulls.iter().map(|p| p.issue.clone()).collect()).await?;
    let merged_by_ids: Vec<i64> = pulls.iter().filter_map(|p| p.pr.merged_by_id).collect();
    let merged_by = issue::users_by_ids(db, &merged_by_ids).await?;
    let mut head_repos: HashMap<i64, Option<RepoCtx>> = HashMap::new();
    for p in &pulls {
        if let Some(h) = p.pr.head_repo_id {
            if h != ctx.repo.id && !head_repos.contains_key(&h) {
                head_repos.insert(h, db::load_repo_by_id(db, viewer, h).await.ok());
            }
        }
    }
    let mut reviewers = crate::pulls::requested_reviewers(db, &issue_ids).await?;
    let counts: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT rc.issue_id, count(*) FROM review_comments rc LEFT JOIN pull_reviews r ON r.id = rc.review_id
          WHERE rc.issue_id = ANY($1) AND (r.state IS NULL OR r.state <> 'PENDING') GROUP BY rc.issue_id",
    )
    .bind(&issue_ids)
    .fetch_all(db)
    .await?;
    let counts: HashMap<i64, i64> = counts.into_iter().collect();
    let mut out = Vec::with_capacity(pulls.len());
    for (p, i) in pulls.into_iter().zip(issues) {
        let status = if with_status { Some(crate::pulls::pull_status(state, &p).await?) } else { None };
        let head_repo = match p.pr.head_repo_id {
            Some(h) if h == ctx.repo.id => Some(ctx.clone()),
            Some(h) => head_repos.get(&h).cloned().flatten(),
            None => None,
        };
        out.push(PullFull {
            merged_by: p.pr.merged_by_id.and_then(|id| merged_by.get(&id).cloned()),
            requested_reviewers: reviewers.remove(&p.issue.id).unwrap_or_default(),
            review_comments: counts.get(&p.issue.id).copied().unwrap_or(0),
            head_repo,
            status,
            issue: i,
            pr: p.pr,
        });
    }
    Ok(out)
}

pub async fn load_one(state: &AppState, ctx: &RepoCtx, viewer: Option<&User>, pull: Pull, with_status: bool) -> ApiResult<PullFull> {
    Ok(load(state, ctx, viewer, vec![pull], with_status).await?.pop().expect("one in, one out"))
}

fn link(href: String) -> Value {
    json!({ "href": href })
}

fn branch(api: &str, base: &str, repo: Option<&RepoCtx>, refname: &str, sha: &str) -> Value {
    match repo {
        Some(r) => json!({
            "label": format!("{}:{}", r.owner.login, refname),
            "ref": refname,
            "sha": sha,
            "user": super::user::simple(api, base, &r.owner),
            "repo": super::repo::repo(api, base, r),
        }),
        None => json!({
            "label": format!("unknown:{refname}"),
            "ref": refname,
            "sha": sha,
            "user": issue::ghost(api, base),
            "repo": null,
        }),
    }
}

/// `pull-request-simple` (list items, review-request responses).
pub fn simple(api: &str, base: &str, ctx: &RepoCtx, p: &PullFull) -> Value {
    let full_name = ctx.full_name();
    let r = &p.issue.row;
    let n = r.number;
    let url = format!("{api}/repos/{full_name}/pulls/{n}");
    let html = format!("{base}/{full_name}/pull/{n}");
    let issue_url = format!("{api}/repos/{full_name}/issues/{n}");
    let statuses = format!("{api}/repos/{full_name}/statuses/{}", p.pr.head_sha);
    let labels: Vec<Value> = p
        .issue
        .labels
        .iter()
        .map(|l| {
            let mut v = issue::label(api, &full_name, l);
            v["archived_by"] = Value::Null;
            v
        })
        .collect();
    let user = super::issue::user_or_ghost(api, base, p.issue.user.as_ref());
    let merge_commit_sha = if p.pr.merged { p.pr.merge_commit_sha.clone() } else { p.pr.test_merge_sha.clone() };
    json!({
        "url": url,
        "id": r.id,
        "node_id": ids::encode("PullRequest", r.id),
        "html_url": html,
        "diff_url": format!("{html}.diff"),
        "patch_url": format!("{html}.patch"),
        "issue_url": issue_url,
        "commits_url": format!("{url}/commits"),
        "review_comments_url": format!("{url}/comments"),
        "review_comment_url": format!("{api}/repos/{full_name}/pulls/comments{{/number}}"),
        "comments_url": format!("{issue_url}/comments"),
        "statuses_url": statuses,
        "number": n,
        "state": r.state,
        "locked": r.locked,
        "title": r.title,
        "user": user,
        "body": super::opt_str(&r.body),
        "labels": labels,
        "milestone": p.issue.milestone.as_ref().map(|m| issue::milestone(api, base, &full_name, m)),
        "active_lock_reason": super::opt_str(&r.active_lock_reason),
        "created_at": ts(r.created_at),
        "updated_at": ts(r.updated_at),
        "closed_at": ts_opt(r.closed_at),
        "merged_at": ts_opt(p.pr.merged_at),
        "merge_commit_sha": merge_commit_sha,
        "assignee": p.issue.assignees.first().map(|u| super::user::simple(api, base, u)),
        "assignees": p.issue.assignees.iter().map(|u| super::user::simple(api, base, u)).collect::<Vec<_>>(),
        "requested_reviewers": p.requested_reviewers.iter().map(|u| super::user::simple(api, base, u)).collect::<Vec<_>>(),
        "requested_teams": [],
        "head": branch(api, base, p.head_repo.as_ref(), &p.pr.head_ref, &p.pr.head_sha),
        "base": branch(api, base, Some(ctx), &p.pr.base_ref, &p.pr.base_sha),
        "_links": {
            "self": link(url.clone()),
            "html": link(html.clone()),
            "issue": link(issue_url.clone()),
            "comments": link(format!("{issue_url}/comments")),
            "review_comments": link(format!("{url}/comments")),
            "review_comment": link(format!("{api}/repos/{full_name}/pulls/comments{{/number}}")),
            "commits": link(format!("{url}/commits")),
            "statuses": link(format!("{api}/repos/{full_name}/statuses/{}", p.pr.head_sha)),
        },
        "author_association": p.issue.author_association,
        "auto_merge": null,
        "draft": p.pr.draft,
    })
}

/// Same as `simple` but with label descriptions coerced to strings: `pull-request-simple`
/// declares `labels[].description` as a non-nullable string.
pub fn simple_listed(api: &str, base: &str, ctx: &RepoCtx, p: &PullFull) -> Value {
    let mut v = simple(api, base, ctx, p);
    if let Some(labels) = v["labels"].as_array_mut() {
        for l in labels {
            if l["description"].is_null() {
                l["description"] = json!("");
            }
        }
    }
    v
}

/// Full `pull-request` (GET/POST/PATCH single PR).
pub fn full(api: &str, base: &str, ctx: &RepoCtx, p: &PullFull) -> Value {
    let mut v = simple(api, base, ctx, p);
    let st = p.status.as_ref();
    let o = v.as_object_mut().unwrap();
    o.insert("merged".into(), json!(p.pr.merged));
    o.insert("mergeable".into(), json!(st.and_then(|s| s.mergeable)));
    o.insert("rebaseable".into(), json!(st.and_then(|s| s.rebaseable)));
    o.insert("mergeable_state".into(), json!(st.map(|s| s.mergeable_state).unwrap_or("unknown")));
    o.insert("merged_by".into(), p.merged_by.as_ref().map(|u| super::user::simple(api, base, u)).unwrap_or(Value::Null));
    o.insert("comments".into(), json!(p.issue.row.comments_count));
    o.insert("review_comments".into(), json!(p.review_comments));
    o.insert("maintainer_can_modify".into(), json!(p.pr.maintainer_can_modify));
    o.insert("commits".into(), json!(p.pr.commits_count));
    o.insert("additions".into(), json!(p.pr.additions));
    o.insert("deletions".into(), json!(p.pr.deletions));
    o.insert("changed_files".into(), json!(p.pr.changed_files));
    v
}

// ---------------------------------------------------------------------------------------------
// Reviews and review comments

/// Users and author associations for review/comment authors.
pub struct Authors {
    pub users: HashMap<i64, User>,
    pub assoc: HashMap<i64, String>,
}

pub async fn authors(db: &PgPool, ctx: &RepoCtx, ids: &[i64]) -> ApiResult<Authors> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    Ok(Authors { users: issue::users_by_ids(db, &ids).await?, assoc: issue::associations(db, ctx, &ids).await? })
}

impl Authors {
    fn user(&self, api: &str, base: &str, id: Option<i64>) -> Value {
        match id.and_then(|i| self.users.get(&i)) {
            Some(u) => super::user::simple(api, base, u),
            None => Value::Null,
        }
    }
    fn assoc(&self, id: Option<i64>) -> String {
        id.and_then(|i| self.assoc.get(&i).cloned()).unwrap_or_else(|| "NONE".into())
    }
}

/// `pull-request-review`.
pub fn review(api: &str, base: &str, ctx: &RepoCtx, number: i64, r: &ReviewRow, a: &Authors) -> Value {
    let full_name = ctx.full_name();
    let html = format!("{base}/{full_name}/pull/{number}#pullrequestreview-{}", r.id);
    let pr_url = format!("{api}/repos/{full_name}/pulls/{number}");
    let mut v = json!({
        "id": r.id,
        "node_id": ids::encode("PullRequestReview", r.id),
        "user": a.user(api, base, r.user_id),
        "body": r.body,
        "state": r.state,
        "html_url": html,
        "pull_request_url": pr_url,
        "_links": { "html": link(html.clone()), "pull_request": link(pr_url.clone()) },
        "commit_id": r.commit_id,
        "author_association": a.assoc(r.user_id),
    });
    if let Some(t) = r.submitted_at {
        v["submitted_at"] = json!(ts(t));
    }
    v
}

/// `pull-request-review-comment`.
pub fn review_comment(api: &str, base: &str, ctx: &RepoCtx, number: i64, c: &ReviewCommentRow, a: &Authors, reactions: &HashMap<String, i64>) -> Value {
    let full_name = ctx.full_name();
    let url = format!("{api}/repos/{full_name}/pulls/comments/{}", c.id);
    let html = format!("{base}/{full_name}/pull/{number}#discussion_r{}", c.id);
    let pr_url = format!("{api}/repos/{full_name}/pulls/{number}");
    let mut v = json!({
        "url": url,
        "pull_request_review_id": c.review_id,
        "id": c.id,
        "node_id": ids::encode("PullRequestReviewComment", c.id),
        "diff_hunk": c.diff_hunk,
        "path": c.path,
        "commit_id": c.commit_id,
        "original_commit_id": c.original_commit_id,
        "user": a.user(api, base, c.user_id),
        "body": c.body,
        "created_at": ts(c.created_at),
        "updated_at": ts(c.updated_at),
        "html_url": html,
        "pull_request_url": pr_url,
        "author_association": a.assoc(c.user_id),
        "_links": { "self": link(url.clone()), "html": link(html.clone()), "pull_request": link(pr_url.clone()) },
        "start_line": c.start_line,
        "original_start_line": c.original_start_line,
        "start_side": c.start_side,
        "side": c.side,
        "subject_type": c.subject_type,
        "reactions": issue::reactions_json(api, &url, reactions),
    });
    let o = v.as_object_mut().unwrap();
    for (k, val) in [
        ("position", c.position),
        ("original_position", c.original_position),
        ("line", c.line),
        ("original_line", c.original_line),
        ("in_reply_to_id", c.in_reply_to_id),
    ] {
        if let Some(x) = val {
            o.insert(k.into(), json!(x));
        }
    }
    v
}

/// `review-comment` (GET /reviews/{id}/comments): `position` is required-nullable and
/// `original_position` required.
pub fn review_comment_legacy(api: &str, base: &str, ctx: &RepoCtx, number: i64, c: &ReviewCommentRow, a: &Authors, reactions: &HashMap<String, i64>) -> Value {
    let mut v = review_comment(api, base, ctx, number, c, a, reactions);
    v["position"] = json!(c.position);
    v["original_position"] = json!(c.original_position.unwrap_or(1));
    v
}

// ---------------------------------------------------------------------------------------------
// Commits and files

/// Forge users for commit author/committer emails (also `<id>+<login>@users.noreply.*`).
pub async fn users_by_emails(db: &PgPool, emails: &[String]) -> ApiResult<HashMap<String, User>> {
    let lower: Vec<String> = emails.iter().map(|e| e.to_lowercase()).collect();
    let mut out = HashMap::new();
    if lower.is_empty() {
        return Ok(out);
    }
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE lower(email) = ANY($1)").bind(&lower).fetch_all(db).await?;
    for u in users {
        if let Some(e) = &u.email {
            out.insert(e.to_lowercase(), u);
        }
    }
    let noreply: Vec<(String, i64)> = lower
        .iter()
        .filter(|e| !out.contains_key(*e))
        .filter_map(|e| {
            let (local, domain) = e.split_once('@')?;
            if !domain.starts_with("users.noreply") {
                return None;
            }
            let id = local.split_once('+').map(|(i, _)| i).unwrap_or(local).parse().ok()?;
            Some((e.clone(), id))
        })
        .collect();
    if !noreply.is_empty() {
        let ids: Vec<i64> = noreply.iter().map(|(_, i)| *i).collect();
        let users = issue::users_by_ids(db, &ids).await?;
        for (e, id) in noreply {
            if let Some(u) = users.get(&id) {
                out.insert(e, u.clone());
            }
        }
    }
    Ok(out)
}

/// `commit` schema (as returned by GET /pulls/{n}/commits).
pub fn commit(api: &str, base: &str, full_name: &str, c: &CommitInfo, users: &HashMap<String, User>) -> Value {
    let url = format!("{api}/repos/{full_name}/commits/{}", c.sha);
    let who = |email: &str| users.get(&email.to_lowercase()).map(|u| super::user::simple(api, base, u)).unwrap_or(Value::Null);
    json!({
        "url": url,
        "sha": c.sha,
        "node_id": ids::encode_str("Commit", &c.sha),
        "html_url": format!("{base}/{full_name}/commit/{}", c.sha),
        "comments_url": format!("{url}/comments"),
        "commit": {
            "url": format!("{api}/repos/{full_name}/git/commits/{}", c.sha),
            "author": { "name": c.author.name, "email": c.author.email, "date": ts(c.author_date()) },
            "committer": { "name": c.committer.name, "email": c.committer.email, "date": ts(c.committer_date()) },
            "message": c.message.trim_end_matches('\n'),
            "comment_count": 0,
            "tree": { "sha": c.tree, "url": format!("{api}/repos/{full_name}/git/trees/{}", c.tree) },
            "verification": { "verified": false, "reason": "unsigned", "payload": null, "signature": null, "verified_at": null },
        },
        "author": who(&c.author.email),
        "committer": who(&c.committer.email),
        "parents": c.parents.iter().map(|p| json!({
            "sha": p,
            "url": format!("{api}/repos/{full_name}/commits/{p}"),
            "html_url": format!("{base}/{full_name}/commit/{p}"),
        })).collect::<Vec<_>>(),
    })
}

/// `diff-entry` for a file of the PR diff at `head_sha`.
pub fn diff_entry(api: &str, base: &str, full_name: &str, head_sha: &str, f: &FileDiff) -> Value {
    let name = f.filename();
    let enc = name.split('/').map(issue::urlencode).collect::<Vec<_>>().join("/");
    let mut v = json!({
        "sha": f.new_sha.clone().or(f.old_sha.clone()),
        "filename": name,
        "status": f.status,
        "additions": f.additions,
        "deletions": f.deletions,
        "changes": f.additions + f.deletions,
        "blob_url": format!("{base}/{full_name}/blob/{head_sha}/{enc}"),
        "raw_url": format!("{base}/{full_name}/raw/{head_sha}/{enc}"),
        "contents_url": format!("{api}/repos/{full_name}/contents/{enc}?ref={head_sha}"),
    });
    if !f.binary {
        v["patch"] = json!(f.patch);
    }
    if f.status == "renamed" || f.status == "copied" {
        if let Some(old) = &f.old_path {
            v["previous_filename"] = json!(old);
        }
    }
    v
}
