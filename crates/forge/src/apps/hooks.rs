//! App webhooks: one `webhooks` row per GitHub App (`app_id`), delivered through the standard
//! webhook queue (`crate::webhooks`). Apps always receive `installation` and
//! `installation_repositories` events; repository events (push, issues, check_run, ...) reach
//! them for repositories of their installations when the installation subscribes to the event.
use serde_json::{json, Map, Value};

use super::{AppRow, InstallationRow};
use crate::db::{RepoCtx, User};
use crate::state::AppState;
use crate::webhooks::HookRow;

/// The app's webhook row.
pub async fn app_hook(s: &AppState, app_id: i64) -> sqlx::Result<Option<HookRow>> {
    sqlx::query_as("SELECT * FROM webhooks WHERE app_id = $1 ORDER BY id LIMIT 1").bind(app_id).fetch_optional(&s.db).await
}

fn deliverable(h: &HookRow) -> bool {
    h.active && h.config.get("url").and_then(Value::as_str).is_some_and(|u| !u.is_empty())
}

/// Emits an installation-level event (`installation`, `installation_repositories`) to the app.
/// `extra` holds event-specific keys. Delivery problems are logged, never returned.
pub async fn emit_installation(s: &AppState, app: &AppRow, inst: &InstallationRow, event: &str, action: &str, sender: &User, extra: Map<String, Value>) {
    let res: anyhow::Result<()> = async {
        let Some(hook) = app_hook(s, app.id).await? else { return Ok(()) };
        if !deliverable(&hook) {
            return Ok(());
        }
        let api = s.api_url();
        let mut payload = json!({
            "action": action,
            "installation": super::installation_json_with(s, inst, app).await.map_err(|e| anyhow::anyhow!(e.message))?,
            "sender": crate::json::user::simple(&api, s.base_url(), sender),
        });
        if let Some(o) = payload.as_object_mut() {
            o.extend(extra);
        }
        crate::webhooks::create_delivery(s, &hook, event, None, payload, false).await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        tracing::error!(error = %format!("{e:#}"), "{event} app webhook failed");
    }
}

#[derive(sqlx::FromRow)]
struct Target {
    installation_id: i64,
    hook_id: i64,
}

/// Called by `webhooks::enqueue_event_ctx`: delivers a repository event to the apps installed on
/// that repository whose installation subscribes to `event`.
pub async fn fan_out(s: &AppState, ctx: &RepoCtx, event: &str, payload: &Value) -> anyhow::Result<()> {
    let targets: Vec<Target> = sqlx::query_as(
        "SELECT i.id AS installation_id, w.id AS hook_id
           FROM app_installations i JOIN webhooks w ON w.app_id = i.app_id
          WHERE i.target_id = $1 AND i.suspended_at IS NULL AND w.active AND $3 = ANY(i.events)
            AND (i.repository_selection = 'all'
                 OR EXISTS (SELECT 1 FROM app_installation_repos r WHERE r.installation_id = i.id AND r.repo_id = $2))",
    )
    .bind(ctx.owner.id)
    .bind(ctx.repo.id)
    .bind(event)
    .fetch_all(&s.db)
    .await?;
    for t in targets {
        let Some(hook) = sqlx::query_as::<_, HookRow>("SELECT * FROM webhooks WHERE id = $1").bind(t.hook_id).fetch_optional(&s.db).await? else {
            continue;
        };
        if !deliverable(&hook) {
            continue;
        }
        let mut p = payload.clone();
        p["installation"] = json!({ "id": t.installation_id, "node_id": crate::ids::encode("Installation", t.installation_id) });
        crate::webhooks::create_delivery(s, &hook, event, Some(ctx.repo.id), p, false).await?;
    }
    Ok(())
}
