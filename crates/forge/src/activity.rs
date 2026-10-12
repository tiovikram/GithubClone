//! Activity events (the GitHub Events API: PushEvent, CreateEvent, IssuesEvent, ...).
//! Most events are recorded by `crate::webhooks::enqueue_event`; call [`record`] directly for
//! events that have no webhook counterpart.
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::db::{RepoCtx, User};
use crate::state::AppState;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EventRow {
    pub id: i64,
    #[sqlx(rename = "type")]
    pub kind: String,
    pub actor_id: Option<i64>,
    pub repo_id: Option<i64>,
    pub org_id: Option<i64>,
    pub payload: Value,
    pub public: bool,
    pub created_at: DateTime<Utc>,
}

/// Records an activity event. `ctx` is the repository the event happened in, if any.
pub async fn record(state: &AppState, kind: &str, actor_id: Option<i64>, ctx: Option<&RepoCtx>, payload: Value) -> anyhow::Result<()> {
    let org_id = ctx.filter(|c| c.owner.kind == "Organization").map(|c| c.owner.id);
    let public = ctx.map(|c| !c.repo.private).unwrap_or(true);
    sqlx::query("INSERT INTO activity_events (type, actor_id, repo_id, org_id, payload, public) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(kind)
        .bind(actor_id)
        .bind(ctx.map(|c| c.repo.id))
        .bind(org_id)
        .bind(payload)
        .bind(public)
        .execute(&state.db)
        .await?;
    Ok(())
}

fn actor_json(api: &str, base: &str, u: Option<&User>) -> Value {
    match u {
        Some(u) => json!({
            "id": u.id,
            "login": u.login,
            "display_login": u.login,
            "gravatar_id": "",
            "url": format!("{api}/users/{}", u.login),
            "avatar_url": format!("{base}/avatars/{}", u.login),
        }),
        None => json!({
            "id": 10137, "login": "ghost", "display_login": "ghost", "gravatar_id": "",
            "url": format!("{api}/users/ghost"), "avatar_url": format!("{base}/avatars/ghost"),
        }),
    }
}

/// Renders events (batch-loading actors, repos and orgs).
pub async fn render(state: &AppState, rows: &[EventRow]) -> anyhow::Result<Vec<Value>> {
    let api = state.api_url();
    let base = state.base_url();
    let mut user_ids: Vec<i64> = rows.iter().filter_map(|r| r.actor_id).collect();
    user_ids.extend(rows.iter().filter_map(|r| r.org_id));
    let users = crate::json::issue::users_by_ids(&state.db, &user_ids).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let repo_ids: Vec<i64> = rows.iter().filter_map(|r| r.repo_id).collect();
    let repos: HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(
        "SELECT r.id, o.login || '/' || r.name FROM repositories r JOIN users o ON o.id = r.owner_id WHERE r.id = ANY($1)",
    )
    .bind(&repo_ids)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    Ok(rows
        .iter()
        .map(|r| {
            let name = r.repo_id.and_then(|id| repos.get(&id)).cloned().unwrap_or_default();
            let mut v = json!({
                "id": r.id.to_string(),
                "type": r.kind,
                "actor": actor_json(&api, base, r.actor_id.and_then(|id| users.get(&id))),
                "repo": {
                    "id": r.repo_id.unwrap_or(0),
                    "name": name,
                    "url": format!("{api}/repos/{name}"),
                },
                "payload": r.payload,
                "public": r.public,
                "created_at": crate::json::ts(r.created_at),
            });
            if let Some(org) = r.org_id.and_then(|id| users.get(&id)) {
                let mut o = actor_json(&api, base, Some(org));
                o["url"] = json!(format!("{api}/orgs/{}", org.login));
                v["org"] = o;
            }
            v
        })
        .collect())
}
