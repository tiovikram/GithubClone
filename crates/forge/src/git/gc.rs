//! Storage garbage collection: repack fragmented repos, delete unreferenced pack objects.
use std::collections::HashSet;

use futures::TryStreamExt;
use object_store::path::Path as ObjPath;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;

use crate::state::AppState;

/// Repos with more packs than this are repacked into one.
const MAX_PACKS: usize = 16;

pub async fn run(state: &AppState) -> anyhow::Result<()> {
    let rows: Vec<(i64, i32)> =
        sqlx::query_as("SELECT repo_id, cardinality(packs) FROM repo_storage WHERE cardinality(packs) > $1")
            .bind(MAX_PACKS as i32)
            .fetch_all(&state.db)
            .await?;
    for (repo_id, n) in rows {
        tracing::info!(repo_id, packs = n, "repacking");
        if let Err(e) = state.git.repack(repo_id).await {
            tracing::error!(repo_id, error = %e, "repack failed");
        }
    }
    // Delete objects of deleted repos.
    let trash: Vec<String> = sqlx::query_scalar("SELECT prefix FROM storage_trash").fetch_all(&state.db).await?;
    for prefix in trash {
        let p = ObjPath::from(prefix.trim_end_matches('/'));
        let objs: Vec<_> = state.store.list(Some(&p)).try_collect().await?;
        for o in objs {
            state.store.delete(&o.location).await?;
        }
        sqlx::query("DELETE FROM storage_trash WHERE prefix = $1").bind(&prefix).execute(&state.db).await?;
    }
    // Delete packs no manifest references, if older than an hour (in-flight writes upload
    // packs before committing the manifest).
    let live: Vec<(i64, Vec<String>)> = sqlx::query_as("SELECT repo_id, packs FROM repo_storage").fetch_all(&state.db).await?;
    let live: HashSet<String> = live
        .into_iter()
        .flat_map(|(id, packs)| packs.into_iter().map(move |p| format!("repos/{id}/packs/{p}")))
        .collect();
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
    let all: Vec<_> = state.store.list(Some(&ObjPath::from("repos"))).try_collect().await?;
    for o in all {
        let key = o.location.to_string();
        let stem = key.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(key.clone());
        if key.contains("/packs/") && !live.contains(&stem) && o.last_modified < cutoff {
            tracing::info!(%key, "deleting unreferenced pack");
            state.store.delete(&o.location).await?;
        }
    }
    Ok(())
}
