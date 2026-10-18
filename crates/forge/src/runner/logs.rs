//! Job logs in object storage. While a job runs, the runner appends chunks
//! (`actions/logs/{job}/{chunk}`); when it completes they are compacted into one object
//! (`actions/logs/{job}/log.txt`) with per-step byte ranges, so logs survive any pod.
use bytes::Bytes;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStoreExt, PutPayload};
use serde_json::{json, Value};

use super::engine::JobRow;
use crate::state::AppState;

/// `2026-10-17T06:30:00.1234567Z` (GitHub's log line timestamp format).
pub fn stamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.7fZ").to_string()
}

fn chunk_path(job: i64, id: i64) -> ObjPath {
    ObjPath::from(format!("actions/logs/{job}/{id:012}"))
}

pub async fn append(s: &AppState, job_id: i64, step: i32, data: &[u8]) -> anyhow::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let id: i64 = sqlx::query_scalar("INSERT INTO actions_log_chunks (job_id, step, size) VALUES ($1, $2, $3) RETURNING id")
        .bind(job_id)
        .bind(step)
        .bind(data.len() as i32)
        .fetch_one(&s.db)
        .await?;
    s.store.put(&chunk_path(job_id, id), PutPayload::from(Bytes::copy_from_slice(data))).await?;
    Ok(())
}

async fn chunks(s: &AppState, job_id: i64) -> anyhow::Result<Vec<(i64, i32, Bytes)>> {
    let rows: Vec<(i64, i32)> = sqlx::query_as("SELECT id, step FROM actions_log_chunks WHERE job_id = $1 ORDER BY id").bind(job_id).fetch_all(&s.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, step) in rows {
        match s.store.get(&chunk_path(job_id, id)).await {
            Ok(r) => out.push((id, step, r.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

/// Compacts the chunks of a finished job into one object. Idempotent.
pub async fn compact(s: &AppState, job_id: i64) -> anyhow::Result<()> {
    let parts = chunks(s, job_id).await?;
    if parts.is_empty() {
        return Ok(());
    }
    let existing: Option<(Option<String>, Option<Value>)> = sqlx::query_as("SELECT log_key, step_logs FROM actions_jobs WHERE id = $1").bind(job_id).fetch_optional(&s.db).await?;
    let mut full: Vec<u8> = Vec::new();
    let mut ranges: Vec<(i32, i64, i64)> = Vec::new();
    // A compacted log may exist already (late chunks after a force-cancel): keep it first.
    if let Some((Some(key), steps)) = &existing {
        if let Ok(r) = s.store.get(&ObjPath::from(key.as_str())).await {
            full.extend_from_slice(&r.bytes().await?);
            if let Some(Value::Array(a)) = steps {
                for e in a {
                    for r in e["ranges"].as_array().cloned().unwrap_or_default() {
                        ranges.push((e["number"].as_i64().unwrap_or(0) as i32, r[0].as_i64().unwrap_or(0), r[1].as_i64().unwrap_or(0)));
                    }
                }
            }
        }
    }
    for (_, step, data) in &parts {
        let off = full.len() as i64;
        full.extend_from_slice(data);
        match ranges.last_mut() {
            Some((st, o, l)) if st == step && *o + *l == off => *l += data.len() as i64,
            _ => ranges.push((*step, off, data.len() as i64)),
        }
    }
    let mut by_step: std::collections::BTreeMap<i32, Vec<[i64; 2]>> = Default::default();
    for (st, o, l) in ranges {
        by_step.entry(st).or_default().push([o, l]);
    }
    let step_logs: Vec<Value> = by_step.into_iter().map(|(n, r)| json!({ "number": n, "ranges": r })).collect();
    let key = format!("actions/logs/{job_id}/log-{}.txt", crate::auth::random_string(8));
    let size = full.len() as i64;
    s.store.put(&ObjPath::from(key.as_str()), PutPayload::from(Bytes::from(full))).await?;
    sqlx::query("UPDATE actions_jobs SET log_key = $2, log_size = $3, step_logs = $4 WHERE id = $1")
        .bind(job_id)
        .bind(&key)
        .bind(size)
        .bind(json!(step_logs))
        .execute(&s.db)
        .await?;
    if let Some((Some(old), _)) = existing {
        if old != key && !shared_log(s, &old).await {
            let _ = s.store.delete(&ObjPath::from(old.as_str())).await;
        }
    }
    let ids: Vec<i64> = parts.iter().map(|(id, _, _)| *id).collect();
    sqlx::query("DELETE FROM actions_log_chunks WHERE id = ANY($1)").bind(&ids).execute(&s.db).await?;
    for id in ids {
        let _ = s.store.delete(&chunk_path(job_id, id)).await;
    }
    Ok(())
}

/// Whether more than one job row (re-run attempts carry logs over) references the log.
async fn shared_log(s: &AppState, key: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actions_jobs WHERE log_key = $1").bind(key).fetch_one(&s.db).await.map(|n| n > 1).unwrap_or(true)
}

/// The whole log of a job (so far).
pub async fn job_log(s: &AppState, job: &JobRow) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    if let Some(key) = &job.log_key {
        if let Ok(r) = s.store.get(&ObjPath::from(key.as_str())).await {
            out.extend_from_slice(&r.bytes().await?);
        }
    }
    for (_, _, d) in chunks(s, job.id).await? {
        out.extend_from_slice(&d);
    }
    Ok(out)
}

/// Per-step logs of a job: step number -> bytes.
pub async fn step_logs(s: &AppState, job: &JobRow) -> anyhow::Result<std::collections::BTreeMap<i32, Vec<u8>>> {
    let mut out: std::collections::BTreeMap<i32, Vec<u8>> = Default::default();
    if let (Some(key), Some(Value::Array(steps))) = (&job.log_key, &job.step_logs) {
        if let Ok(r) = s.store.get(&ObjPath::from(key.as_str())).await {
            let full = r.bytes().await?;
            for e in steps {
                let n = e["number"].as_i64().unwrap_or(0) as i32;
                for r in e["ranges"].as_array().cloned().unwrap_or_default() {
                    let (o, l) = (r[0].as_u64().unwrap_or(0) as usize, r[1].as_u64().unwrap_or(0) as usize);
                    if let Some(sl) = full.get(o..o + l) {
                        out.entry(n).or_default().extend_from_slice(sl);
                    }
                }
            }
        }
    }
    for (_, step, d) in chunks(s, job.id).await? {
        out.entry(step).or_default().extend_from_slice(&d);
    }
    Ok(out)
}

/// Deletes all logs of the given jobs.
pub async fn delete_logs(s: &AppState, jobs: &[JobRow]) -> anyhow::Result<()> {
    for j in jobs {
        if let Some(key) = &j.log_key {
            sqlx::query("UPDATE actions_jobs SET log_key = NULL, log_size = NULL, step_logs = NULL WHERE id = $1").bind(j.id).execute(&s.db).await?;
            if !shared_log(s, key).await {
                let _ = s.store.delete(&ObjPath::from(key.as_str())).await;
            }
        }
        for (id, _, _) in chunks(s, j.id).await? {
            let _ = s.store.delete(&chunk_path(j.id, id)).await;
        }
        sqlx::query("DELETE FROM actions_log_chunks WHERE job_id = $1").bind(j.id).execute(&s.db).await?;
    }
    Ok(())
}

/// File-name-safe job/step names (GitHub strips characters that are invalid in paths).
pub fn safe_name(n: &str) -> String {
    let s: String = n.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '<' | '>' | '|' | '*' | '?' | '"' | '\0')).collect();
    let s = s.trim().to_string();
    s.chars().take(90).collect()
}

/// Logs archive of a run attempt: `{n}_{job}.txt` per job and `{job}/{step}_{name}.txt` per step.
pub async fn run_zip(s: &AppState, jobs: &[JobRow]) -> anyhow::Result<Vec<u8>> {
    let mut z = super::zip::ZipWriter::new();
    for (i, j) in jobs.iter().enumerate() {
        if j.status == "pending" || (j.status == "completed" && j.conclusion.as_deref() == Some("skipped") && j.log_key.is_none()) {
            continue;
        }
        let jn = j.name.replace('/', "");
        z.add(&format!("{i}_{jn}.txt"), &job_log(s, j).await?);
        let per = step_logs(s, j).await?;
        let steps = j.steps.as_array().cloned().unwrap_or_default();
        for st in steps {
            let n = st["number"].as_i64().unwrap_or(0) as i32;
            let name = safe_name(st["name"].as_str().unwrap_or(""));
            if let Some(data) = per.get(&n) {
                z.add(&format!("{jn}/{n}_{name}.txt"), data);
            }
        }
    }
    Ok(z.finish())
}
