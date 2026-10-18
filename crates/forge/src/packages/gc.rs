//! Package garbage collection, run by `forge gc`:
//! 1. purge packages and versions deleted more than 30 days ago (no longer restorable);
//! 2. drop abandoned upload sessions (24 h) and their chunk objects;
//! 3. forget repository blob links older than 24 h that no manifest uses;
//! 4. delete blobs nothing references and nobody has touched for an hour (a push in progress
//!    touches the blobs it is about to reference, see `packages::touch_blobs`).
use futures::TryStreamExt;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use object_store::path::Path as ObjPath;

use crate::state::AppState;

#[derive(Debug, Default)]
pub struct GcStats {
    pub packages_purged: u64,
    pub versions_purged: u64,
    pub uploads_dropped: u64,
    pub blobs_deleted: u64,
}

pub async fn run(state: &AppState) -> anyhow::Result<GcStats> {
    run_with(state, chrono::Duration::days(super::RESTORE_DAYS), chrono::Duration::hours(1), chrono::Duration::hours(24)).await
}

/// `run` with explicit ages (tests use zero).
pub async fn run_with(state: &AppState, restore: chrono::Duration, blob_grace: chrono::Duration, upload_ttl: chrono::Duration) -> anyhow::Result<GcStats> {
    let mut st = GcStats::default();
    let now = chrono::Utc::now();
    st.packages_purged = sqlx::query("DELETE FROM packages WHERE deleted_at IS NOT NULL AND deleted_at <= $1")
        .bind(now - restore)
        .execute(&state.db)
        .await?
        .rows_affected();
    st.versions_purged = sqlx::query("DELETE FROM package_versions WHERE deleted_at IS NOT NULL AND deleted_at <= $1")
        .bind(now - restore)
        .execute(&state.db)
        .await?
        .rows_affected();

    let stale: Vec<(String, Vec<String>)> = sqlx::query_as("DELETE FROM package_uploads WHERE updated_at <= $1 RETURNING id, parts")
        .bind(now - upload_ttl)
        .fetch_all(&state.db)
        .await?;
    st.uploads_dropped = stale.len() as u64;
    for (id, _) in &stale {
        let prefix = ObjPath::from(format!("packages/uploads/{id}"));
        let objs: Vec<_> = state.store.list(Some(&prefix)).try_collect().await?;
        for o in objs {
            let _ = state.store.delete(&o.location).await;
        }
    }

    sqlx::query("DELETE FROM package_repo_blobs WHERE last_seen_at <= $1").bind(now - upload_ttl).execute(&state.db).await?;

    let dead: Vec<String> = sqlx::query_scalar(
        "DELETE FROM package_blobs b WHERE b.last_seen_at <= $1
            AND NOT EXISTS (SELECT 1 FROM package_version_blobs vb WHERE vb.digest = b.digest)
            AND NOT EXISTS (SELECT 1 FROM package_repo_blobs rb WHERE rb.digest = b.digest)
          RETURNING b.digest",
    )
    .bind(now - blob_grace)
    .fetch_all(&state.db)
    .await?;
    for d in &dead {
        match state.store.delete(&super::blob_key(d)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => st.blobs_deleted += 1,
            Err(e) => tracing::warn!(digest = %d, error = %e, "deleting package blob failed"),
        }
    }
    tracing::info!(?st, "package gc");
    Ok(st)
}
