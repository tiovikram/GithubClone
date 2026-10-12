//! Issues, labels, milestones: row types, batched loaders and GitHub JSON builders.
//! Shared by the issues and pulls REST modules and by GraphQL.
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{opt_str, ts, ts_opt};
use crate::db::{RepoCtx, User};
use crate::error::ApiResult;
use crate::ids;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IssueRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub is_pull: bool,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub state_reason: Option<String>,
    pub user_id: Option<i64>,
    pub milestone_id: Option<i64>,
    pub locked: bool,
    pub active_lock_reason: Option<String>,
    pub comments_count: i64,
    pub closed_by_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LabelRow {
    pub id: i64,
    pub repo_id: i64,
    pub name: String,
    pub color: String,
    pub description: Option<String>,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MilestoneRow {
    pub id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub title: String,
    pub description: Option<String>,
    pub state: String,
    pub due_on: Option<DateTime<Utc>>,
    pub creator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// A milestone with its creator and open/closed issue counts.
#[derive(Debug, Clone)]
pub struct MilestoneFull {
    pub row: MilestoneRow,
    pub creator: Option<User>,
    pub open_issues: i64,
    pub closed_issues: i64,
}

/// Everything needed to render an issue (or the issue half of a pull request).
#[derive(Debug, Clone)]
pub struct IssueFull {
    pub row: IssueRow,
    pub user: Option<User>,
    pub labels: Vec<LabelRow>,
    pub assignees: Vec<User>,
    pub milestone: Option<MilestoneFull>,
    pub closed_by: Option<User>,
    pub reactions: HashMap<String, i64>,
    pub author_association: String,
    /// `merged_at` for pull requests (used in the `pull_request` stub of issue JSON).
    pub merged_at: Option<DateTime<Utc>>,
}

pub const REACTIONS: [&str; 8] = ["+1", "-1", "laugh", "hooray", "confused", "heart", "rocket", "eyes"];

pub async fn users_by_ids(db: &PgPool, ids: &[i64]) -> ApiResult<HashMap<i64, User>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(ids).fetch_all(db).await?;
    Ok(users.into_iter().map(|u| (u.id, u)).collect())
}

/// Reaction counts per subject id for one subject type (issue, issue_comment, review_comment, release).
pub async fn reaction_counts(db: &PgPool, subject_type: &str, ids: &[i64]) -> ApiResult<HashMap<i64, HashMap<String, i64>>> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT subject_id, content, count(*) FROM reactions WHERE subject_type = $1 AND subject_id = ANY($2)
          GROUP BY subject_id, content",
    )
    .bind(subject_type)
    .bind(ids)
    .fetch_all(db)
    .await?;
    let mut out: HashMap<i64, HashMap<String, i64>> = HashMap::new();
    for (id, content, n) in rows {
        out.entry(id).or_default().insert(content, n);
    }
    Ok(out)
}

/// GitHub `author_association` for users relative to a repository, batched.
pub async fn associations(db: &PgPool, ctx: &RepoCtx, user_ids: &[i64]) -> ApiResult<HashMap<i64, String>> {
    let mut out = HashMap::new();
    if user_ids.is_empty() {
        return Ok(out);
    }
    let collabs: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM repo_collaborators WHERE repo_id = $1 AND user_id = ANY($2)")
        .bind(ctx.repo.id)
        .bind(user_ids)
        .fetch_all(db)
        .await?;
    let members: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM org_members WHERE org_id = $1 AND user_id = ANY($2)")
        .bind(ctx.owner.id)
        .bind(user_ids)
        .fetch_all(db)
        .await?;
    for &id in user_ids {
        let a = if id == ctx.owner.id {
            "OWNER"
        } else if members.contains(&id) {
            "MEMBER"
        } else if collabs.contains(&id) {
            "COLLABORATOR"
        } else {
            "NONE"
        };
        out.insert(id, a.to_string());
    }
    Ok(out)
}

pub async fn load_milestones(db: &PgPool, ids: &[i64]) -> ApiResult<HashMap<i64, MilestoneFull>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<MilestoneRow> = sqlx::query_as("SELECT * FROM milestones WHERE id = ANY($1)").bind(ids).fetch_all(db).await?;
    let counts: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT milestone_id, count(*) FILTER (WHERE state = 'open'), count(*) FILTER (WHERE state = 'closed')
           FROM issues WHERE milestone_id = ANY($1) GROUP BY milestone_id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    let counts: HashMap<i64, (i64, i64)> = counts.into_iter().map(|(id, o, c)| (id, (o, c))).collect();
    let creator_ids: Vec<i64> = rows.iter().filter_map(|m| m.creator_id).collect();
    let creators = users_by_ids(db, &creator_ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let (open_issues, closed_issues) = counts.get(&row.id).copied().unwrap_or((0, 0));
            let creator = row.creator_id.and_then(|id| creators.get(&id).cloned());
            (row.id, MilestoneFull { row, creator, open_issues, closed_issues })
        })
        .collect())
}

/// Loads labels, assignees, users, milestone, reactions and associations for many issues at once.
pub async fn load_full(db: &PgPool, ctx: &RepoCtx, rows: Vec<IssueRow>) -> ApiResult<Vec<IssueFull>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let label_rows: Vec<(i64, i64, i64, String, String, Option<String>, bool, DateTime<Utc>)> = sqlx::query_as(
        "SELECT il.issue_id, l.id, l.repo_id, l.name, l.color, l.description, l.is_default, l.created_at
           FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = ANY($1) ORDER BY l.name",
    )
    .bind(&ids)
    .fetch_all(db)
    .await?;
    let mut labels: HashMap<i64, Vec<LabelRow>> = HashMap::new();
    for (issue_id, id, repo_id, name, color, description, is_default, created_at) in label_rows {
        labels.entry(issue_id).or_default().push(LabelRow { id, repo_id, name, color, description, is_default, created_at });
    }
    let assignee_rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT issue_id, user_id FROM issue_assignees WHERE issue_id = ANY($1) ORDER BY user_id")
            .bind(&ids)
            .fetch_all(db)
            .await?;
    let merged: Vec<(i64, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT issue_id, merged_at FROM pull_requests WHERE issue_id = ANY($1)")
            .bind(&ids)
            .fetch_all(db)
            .await?;
    let merged: HashMap<i64, Option<DateTime<Utc>>> = merged.into_iter().collect();
    let mut user_ids: Vec<i64> = rows.iter().flat_map(|r| [r.user_id, r.closed_by_id]).flatten().collect();
    user_ids.extend(assignee_rows.iter().map(|(_, u)| *u));
    user_ids.sort_unstable();
    user_ids.dedup();
    let users = users_by_ids(db, &user_ids).await?;
    let milestone_ids: Vec<i64> = rows.iter().filter_map(|r| r.milestone_id).collect();
    let milestones = load_milestones(db, &milestone_ids).await?;
    let mut reactions = reaction_counts(db, "issue", &ids).await?;
    let author_ids: Vec<i64> = rows.iter().filter_map(|r| r.user_id).collect();
    let assoc = associations(db, ctx, &author_ids).await?;
    let mut assignees: HashMap<i64, Vec<User>> = HashMap::new();
    for (issue_id, uid) in assignee_rows {
        if let Some(u) = users.get(&uid) {
            assignees.entry(issue_id).or_default().push(u.clone());
        }
    }
    Ok(rows
        .into_iter()
        .map(|row| IssueFull {
            user: row.user_id.and_then(|id| users.get(&id).cloned()),
            labels: labels.remove(&row.id).unwrap_or_default(),
            assignees: assignees.remove(&row.id).unwrap_or_default(),
            milestone: row.milestone_id.and_then(|id| milestones.get(&id).cloned()),
            closed_by: row.closed_by_id.and_then(|id| users.get(&id).cloned()),
            reactions: reactions.remove(&row.id).unwrap_or_default(),
            author_association: row.user_id.and_then(|id| assoc.get(&id).cloned()).unwrap_or_else(|| "NONE".into()),
            merged_at: merged.get(&row.id).cloned().flatten(),
            row,
        })
        .collect())
}

pub async fn load_one(db: &PgPool, ctx: &RepoCtx, row: IssueRow) -> ApiResult<IssueFull> {
    Ok(load_full(db, ctx, vec![row]).await?.pop().expect("one row in, one row out"))
}

/// Fetch an issue or PR row by number.
pub async fn issue_by_number(db: &PgPool, repo_id: i64, number: i64) -> ApiResult<IssueRow> {
    sqlx::query_as("SELECT * FROM issues WHERE repo_id = $1 AND number = $2")
        .bind(repo_id)
        .bind(number)
        .fetch_optional(db)
        .await?
        .ok_or_else(crate::error::ApiError::not_found)
}

/// The deleted-user placeholder GitHub shows for authors that no longer exist.
pub fn ghost(api: &str, base: &str) -> Value {
    let u = User {
        id: 10137,
        login: "ghost".into(),
        kind: "User".into(),
        name: None,
        email: None,
        password_hash: None,
        bio: None,
        company: None,
        location: None,
        blog: String::new(),
        twitter_username: None,
        site_admin: false,
        created_at: DateTime::<Utc>::UNIX_EPOCH,
        updated_at: DateTime::<Utc>::UNIX_EPOCH,
    };
    super::user::simple(api, base, &u)
}

pub fn user_or_ghost(api: &str, base: &str, u: Option<&User>) -> Value {
    match u {
        Some(u) => super::user::simple(api, base, u),
        None => ghost(api, base),
    }
}

pub fn reactions_json(api: &str, url: &str, counts: &HashMap<String, i64>) -> Value {
    let _ = api;
    let mut v = json!({ "url": format!("{url}/reactions") });
    let mut total = 0;
    for r in REACTIONS {
        let n = counts.get(r).copied().unwrap_or(0);
        total += n;
        v[r] = json!(n);
    }
    v["total_count"] = json!(total);
    v
}

pub fn label(api: &str, full_name: &str, l: &LabelRow) -> Value {
    json!({
        "id": l.id,
        "node_id": ids::encode("Label", l.id),
        "url": format!("{api}/repos/{full_name}/labels/{}", urlencode(&l.name)),
        "name": l.name,
        "color": l.color,
        "default": l.is_default,
        "description": opt_str(&l.description),
    })
}

pub fn milestone(api: &str, base: &str, full_name: &str, m: &MilestoneFull) -> Value {
    let r = &m.row;
    let url = format!("{api}/repos/{full_name}/milestones/{}", r.number);
    json!({
        "url": url,
        "html_url": format!("{base}/{full_name}/milestone/{}", r.number),
        "labels_url": format!("{url}/labels"),
        "id": r.id,
        "node_id": ids::encode("Milestone", r.id),
        "number": r.number,
        "state": r.state,
        "title": r.title,
        "description": opt_str(&r.description),
        "creator": m.creator.as_ref().map(|u| super::user::simple(api, base, u)),
        "open_issues": m.open_issues,
        "closed_issues": m.closed_issues,
        "created_at": ts(r.created_at),
        "updated_at": ts(r.updated_at),
        "closed_at": ts_opt(r.closed_at),
        "due_on": ts_opt(r.due_on),
    })
}

/// `issue` schema. PRs get a `pull_request` stub like GitHub's issues endpoints return.
pub fn issue(api: &str, base: &str, ctx: &RepoCtx, i: &IssueFull) -> Value {
    let full_name = ctx.full_name();
    let r = &i.row;
    let url = format!("{api}/repos/{full_name}/issues/{}", r.number);
    let html_kind = if r.is_pull { "pull" } else { "issues" };
    let mut v = json!({
        "id": r.id,
        "node_id": ids::encode(if r.is_pull { "PullRequest" } else { "Issue" }, r.id),
        "url": url,
        "repository_url": format!("{api}/repos/{full_name}"),
        "labels_url": format!("{url}/labels{{/name}}"),
        "comments_url": format!("{url}/comments"),
        "events_url": format!("{url}/events"),
        "html_url": format!("{base}/{full_name}/{html_kind}/{}", r.number),
        "number": r.number,
        "state": r.state,
        "state_reason": opt_str(&r.state_reason),
        "title": r.title,
        "body": opt_str(&r.body),
        "user": user_or_ghost(api, base, i.user.as_ref()),
        "labels": i.labels.iter().map(|l| label(api, &full_name, l)).collect::<Vec<_>>(),
        "assignee": i.assignees.first().map(|u| super::user::simple(api, base, u)),
        "assignees": i.assignees.iter().map(|u| super::user::simple(api, base, u)).collect::<Vec<_>>(),
        "milestone": i.milestone.as_ref().map(|m| milestone(api, base, &full_name, m)),
        "locked": r.locked,
        "active_lock_reason": opt_str(&r.active_lock_reason),
        "comments": r.comments_count,
        "closed_at": ts_opt(r.closed_at),
        "created_at": ts(r.created_at),
        "updated_at": ts(r.updated_at),
        "closed_by": i.closed_by.as_ref().map(|u| super::user::simple(api, base, u)),
        "author_association": i.author_association,
        "reactions": reactions_json(api, &url, &i.reactions),
        "timeline_url": format!("{url}/timeline"),
        "performed_via_github_app": null,
    });
    if r.is_pull {
        let purl = format!("{api}/repos/{full_name}/pulls/{}", r.number);
        let html = format!("{base}/{full_name}/pull/{}", r.number);
        v["pull_request"] = json!({
            "url": purl,
            "html_url": html,
            "diff_url": format!("{html}.diff"),
            "patch_url": format!("{html}.patch"),
            "merged_at": ts_opt(i.merged_at),
        });
        v["draft"] = json!(false);
    }
    v
}

pub fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}
