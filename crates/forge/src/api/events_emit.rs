//! `issues`, `issue_comment` and `pull_request` webhook events (and through
//! `webhooks::enqueue_event_ctx` the matching activity events and notification threads),
//! emitted by the issue, comment and pull request services. Failures are logged, never
//! returned: the write already succeeded.
use serde_json::{json, Value};

use crate::db::{RepoCtx, User};
use crate::json::issue::{CommentRow, IssueRow};
use crate::pulls::Pull;
use crate::state::AppState;

fn sender(s: &AppState, u: &User) -> Value {
    crate::json::user::simple(&s.api_url(), s.base_url(), u)
}

async fn emit(s: &AppState, ctx: &RepoCtx, event: &str, payload: Value) {
    if let Err(e) = crate::webhooks::enqueue_event_ctx(s, ctx, event, payload).await {
        tracing::warn!(error = %format!("{e:#}"), event, "emitting webhook event failed");
    }
}

/// `issues` event (`opened`, `closed`, `reopened`, `edited`, `assigned`, ...). Pull requests
/// get [`pull_event`] instead.
pub async fn issue_event(s: &AppState, ctx: &RepoCtx, actor: &User, action: &str, row: &IssueRow, extra: Value) {
    if row.is_pull {
        if let Ok(pull) = crate::pulls::pull_by_issue_id(&s.db, row.id).await {
            pull_event(s, ctx, actor, action, pull, extra).await;
        }
        return;
    }
    let issue = match super::issues::issue_json(s, ctx, row.clone()).await {
        Ok(v) => v,
        Err(e) => return tracing::warn!(error = %e.message, "rendering issue for webhook"),
    };
    let mut p = json!({"action": action, "issue": issue, "sender": sender(s, actor)});
    if let (Some(o), Value::Object(x)) = (p.as_object_mut(), extra) {
        o.extend(x);
    }
    emit(s, ctx, "issues", p).await;
}

/// `issue_comment` event (`created`, `edited`, `deleted`) on an issue or pull request.
pub async fn comment_event(s: &AppState, ctx: &RepoCtx, actor: &User, action: &str, comment: &CommentRow, extra: Value) {
    let Ok(row) = sqlx::query_as::<_, IssueRow>("SELECT * FROM issues WHERE id = $1").bind(comment.issue_id).fetch_one(&s.db).await else { return };
    let (Ok(issue), Ok(c)) = (super::issues::issue_json(s, ctx, row).await, super::issue_comments::comment_json(s, ctx, comment.clone()).await) else { return };
    let mut p = json!({"action": action, "issue": issue, "comment": c, "sender": sender(s, actor)});
    if let (Some(o), Value::Object(x)) = (p.as_object_mut(), extra) {
        o.extend(x);
    }
    emit(s, ctx, "issue_comment", p).await;
}

/// `pull_request` event (`opened`, `closed`, `reopened`, `review_requested`, ...).
pub async fn pull_event(s: &AppState, ctx: &RepoCtx, actor: &User, action: &str, pull: Pull, extra: Value) {
    let number = pull.issue.number;
    let full = match crate::json::pull::load_one(s, ctx, Some(actor), pull, false).await {
        Ok(f) => f,
        Err(e) => return tracing::warn!(error = %e.message, "rendering pull request for webhook"),
    };
    let pr = crate::json::pull::full(&s.api_url(), s.base_url(), ctx, &full);
    let mut p = json!({"action": action, "number": number, "pull_request": pr, "sender": sender(s, actor)});
    if let (Some(o), Value::Object(x)) = (p.as_object_mut(), extra) {
        o.extend(x);
    }
    emit(s, ctx, "pull_request", p).await;
}
