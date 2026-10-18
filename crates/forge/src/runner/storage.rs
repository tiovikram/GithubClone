//! Artifacts and caches in object storage (`actions/artifacts/{repo}/{id}.zip`,
//! `actions/caches/{repo}/{id}.tgz`), uploaded by runners and served to the REST API.
use axum::body::Body;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use object_store::path::Path as ObjPath;
use object_store::ObjectStoreExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Largest artifact or cache entry a runner may upload (FORGE_ACTIONS_MAX_UPLOAD bytes, 2 GiB).
pub fn max_upload() -> u64 {
    std::env::var("FORGE_ACTIONS_MAX_UPLOAD").ok().and_then(|v| v.parse().ok()).unwrap_or(2 << 30)
}

/// Total cache size per repository before least-recently used entries are evicted (GitHub: 10 GB).
pub const DEFAULT_CACHE_LIMIT_GB: i32 = 10;

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct ArtifactRow {
    pub id: i64,
    pub repo_id: i64,
    pub run_id: Option<i64>,
    pub job_id: Option<i64>,
    pub name: String,
    pub size_in_bytes: i64,
    pub object_key: String,
    pub digest: Option<String>,
    pub head_branch: Option<String>,
    pub head_sha: Option<String>,
    pub expired: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct CacheRow {
    pub id: i64,
    pub repo_id: i64,
    pub key: String,
    pub version: String,
    pub git_ref: String,
    pub size_in_bytes: i64,
    pub object_key: String,
    pub created_at: DateTime<Utc>,
    pub last_accessed_at: DateTime<Utc>,
}

/// Streams a request body into object storage; returns (size, sha256 hex).
pub async fn write_body(s: &AppState, key: &str, body: Body) -> anyhow::Result<(i64, String)> {
    let mut w = object_store::buffered::BufWriter::new(s.store.clone(), ObjPath::from(key.to_string()));
    let mut hasher = Sha256::new();
    let mut size: i64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = w.abort().await;
                anyhow::bail!("upload interrupted: {e}");
            }
        };
        hasher.update(&chunk);
        size += chunk.len() as i64;
        if size as u64 > max_upload() {
            let _ = w.abort().await;
            anyhow::bail!("upload exceeds the maximum size ({} bytes)", max_upload());
        }
        if let Err(e) = w.write_all(&chunk).await {
            let _ = w.abort().await;
            return Err(e.into());
        }
    }
    w.shutdown().await?;
    Ok((size, hex::encode(hasher.finalize())))
}

pub async fn read_object(s: &AppState, key: &str) -> ApiResult<object_store::GetResult> {
    s.store.get(&ObjPath::from(key.to_string())).await.map_err(|e| match e {
        object_store::Error::NotFound { .. } => ApiError::not_found(),
        e => ApiError::internal(e),
    })
}

/// Stores an artifact uploaded by job `job_id` of `run_id`. Names are unique per run (409).
pub async fn create_artifact(s: &AppState, run: &crate::api::actions::RunRow, job_id: i64, name: &str, retention_days: Option<i64>, body: Body) -> ApiResult<ArtifactRow> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\', ':', '<', '>', '|', '*', '?', '"', '\r', '\n']) {
        return Err(ApiError::unprocessable(format!("The artifact name is not valid: {name}")));
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM actions_artifacts WHERE run_id = $1 AND name = $2")
        .bind(run.id)
        .bind(name)
        .fetch_optional(&s.db)
        .await?;
    if exists.is_some() {
        return Err(ApiError::conflict(format!("An artifact with the name '{name}' already exists in this workflow run")));
    }
    let key = format!("actions/artifacts/{}/{}-{}.zip", run.repo_id, run.id, crate::auth::random_string(12));
    let (size, digest) = write_body(s, &key, body).await.map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    let days = retention_days.unwrap_or(90).clamp(1, 400);
    let row: Option<ArtifactRow> = sqlx::query_as(
        "INSERT INTO actions_artifacts (repo_id, run_id, job_id, name, size_in_bytes, object_key, digest, head_branch, head_sha, expires_at)
         SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, now() + make_interval(days => $10)
          WHERE NOT EXISTS (SELECT 1 FROM actions_artifacts WHERE run_id = $2 AND name = $4)
         RETURNING *",
    )
    .bind(run.repo_id)
    .bind(run.id)
    .bind(job_id)
    .bind(name)
    .bind(size)
    .bind(&key)
    .bind(format!("sha256:{digest}"))
    .bind(&run.head_branch)
    .bind(&run.head_sha)
    .bind(days as i32)
    .fetch_optional(&s.db)
    .await?;
    match row {
        Some(r) => Ok(r),
        None => {
            let _ = s.store.delete(&ObjPath::from(key)).await;
            Err(ApiError::conflict(format!("An artifact with the name '{name}' already exists in this workflow run")))
        }
    }
}

pub async fn delete_artifact(s: &AppState, a: &ArtifactRow) -> ApiResult<()> {
    sqlx::query("DELETE FROM actions_artifacts WHERE id = $1").bind(a.id).execute(&s.db).await?;
    let _ = s.store.delete(&ObjPath::from(a.object_key.clone())).await;
    Ok(())
}

/// Marks artifacts past their retention as expired and deletes their content.
pub async fn expire_artifacts(s: &AppState) -> ApiResult<()> {
    let rows: Vec<(i64, String)> = sqlx::query_as("UPDATE actions_artifacts SET expired = TRUE, updated_at = now() WHERE NOT expired AND expires_at < now() RETURNING id, object_key")
        .fetch_all(&s.db)
        .await?;
    for (_, key) in rows {
        let _ = s.store.delete(&ObjPath::from(key)).await;
    }
    Ok(())
}

/// Finds a cache entry: exact `keys[0]` first, then prefix matches of every key (newest first),
/// on `git_ref` and then on the default branch (GitHub's cache scoping).
pub async fn find_cache(s: &AppState, repo_id: i64, keys: &[String], version: &str, refs: &[String]) -> ApiResult<Option<CacheRow>> {
    for r in refs {
        if let Some(k) = keys.first() {
            let hit: Option<CacheRow> = sqlx::query_as("SELECT * FROM actions_caches WHERE repo_id = $1 AND git_ref = $2 AND key = $3 AND version = $4")
                .bind(repo_id)
                .bind(r)
                .bind(k)
                .bind(version)
                .fetch_optional(&s.db)
                .await?;
            if hit.is_some() {
                return touch(s, hit).await;
            }
        }
        for k in keys {
            let hit: Option<CacheRow> = sqlx::query_as(
                "SELECT * FROM actions_caches WHERE repo_id = $1 AND git_ref = $2 AND starts_with(key, $3) AND version = $4 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(repo_id)
            .bind(r)
            .bind(k)
            .bind(version)
            .fetch_optional(&s.db)
            .await?;
            if hit.is_some() {
                return touch(s, hit).await;
            }
        }
    }
    Ok(None)
}

async fn touch(s: &AppState, c: Option<CacheRow>) -> ApiResult<Option<CacheRow>> {
    if let Some(c) = &c {
        sqlx::query("UPDATE actions_caches SET last_accessed_at = now() WHERE id = $1").bind(c.id).execute(&s.db).await?;
    }
    Ok(c)
}

/// Saves a cache entry (immutable: an existing key/version on the ref is a conflict).
pub async fn create_cache(s: &AppState, repo_id: i64, git_ref: &str, key: &str, version: &str, body: Body) -> ApiResult<CacheRow> {
    if key.is_empty() || key.len() > 512 || key.contains(',') {
        return Err(ApiError::unprocessable("Cache key is invalid: keys may not be empty, longer than 512 characters or contain commas"));
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM actions_caches WHERE repo_id = $1 AND git_ref = $2 AND key = $3 AND version = $4")
        .bind(repo_id)
        .bind(git_ref)
        .bind(key)
        .bind(version)
        .fetch_optional(&s.db)
        .await?;
    if exists.is_some() {
        return Err(ApiError::conflict(format!("Unable to reserve cache with key {key}, another job may be creating this cache.")));
    }
    let okey = format!("actions/caches/{repo_id}/{}.tgz", crate::auth::random_string(16));
    let (size, _) = write_body(s, &okey, body).await.map_err(|e| ApiError::bad_request(format!("{e:#}")))?;
    let row: Option<CacheRow> = sqlx::query_as(
        "INSERT INTO actions_caches (repo_id, key, version, git_ref, size_in_bytes, object_key) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (repo_id, git_ref, key, version) DO NOTHING RETURNING *",
    )
    .bind(repo_id)
    .bind(key)
    .bind(version)
    .bind(git_ref)
    .bind(size)
    .bind(&okey)
    .fetch_optional(&s.db)
    .await?;
    let Some(row) = row else {
        let _ = s.store.delete(&ObjPath::from(okey)).await;
        return Err(ApiError::conflict(format!("Unable to reserve cache with key {key}, another job may be creating this cache.")));
    };
    evict(s, repo_id).await?;
    Ok(row)
}

pub async fn cache_limit_gb(s: &AppState, repo_id: i64) -> ApiResult<i32> {
    let repo: Option<i32> = sqlx::query_scalar("SELECT repo_cache_size_limit_in_gb FROM actions_cache_policies WHERE scope_kind = 'repo' AND scope_id = $1")
        .bind(repo_id)
        .fetch_optional(&s.db)
        .await?
        .flatten();
    let ent: Option<(Option<i32>, Option<i32>)> =
        sqlx::query_as("SELECT repo_cache_size_limit_in_gb, max_repo_cache_size_limit_in_gb FROM actions_cache_policies WHERE scope_kind = 'enterprise' AND scope_id = 0")
            .fetch_optional(&s.db)
            .await?;
    let (ent_default, ent_max) = ent.unwrap_or((None, None));
    let limit = repo.or(ent_default).unwrap_or(DEFAULT_CACHE_LIMIT_GB);
    Ok(match ent_max {
        Some(m) => limit.min(m),
        None => limit,
    })
}

/// Evicts least recently used caches beyond the repository's size limit.
pub async fn evict(s: &AppState, repo_id: i64) -> ApiResult<()> {
    let limit = cache_limit_gb(s, repo_id).await? as i64 * 1024 * 1024 * 1024;
    let rows: Vec<CacheRow> = sqlx::query_as("SELECT * FROM actions_caches WHERE repo_id = $1 ORDER BY last_accessed_at DESC").bind(repo_id).fetch_all(&s.db).await?;
    let mut total = 0i64;
    for c in rows {
        total += c.size_in_bytes;
        if total > limit {
            delete_cache(s, &c).await?;
        }
    }
    // GitHub also removes entries not accessed for 7 days.
    let stale: Vec<CacheRow> = sqlx::query_as("SELECT * FROM actions_caches WHERE repo_id = $1 AND last_accessed_at < now() - interval '7 days'").bind(repo_id).fetch_all(&s.db).await?;
    for c in stale {
        delete_cache(s, &c).await?;
    }
    Ok(())
}

pub async fn delete_cache(s: &AppState, c: &CacheRow) -> ApiResult<()> {
    sqlx::query("DELETE FROM actions_caches WHERE id = $1").bind(c.id).execute(&s.db).await?;
    let _ = s.store.delete(&ObjPath::from(c.object_key.clone())).await;
    Ok(())
}
