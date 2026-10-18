//! The "standard queue": a portable Postgres-backed job queue.
//!
//! Jobs live in the `jobs` table. Every forge pod runs a worker (`spawn_worker`) that claims due
//! jobs with `FOR UPDATE SKIP LOCKED` and leases them by moving `run_at` forward, so any number of
//! replicas can run concurrently and a pod killed mid-job only delays that job until the lease
//! expires. Failed jobs are retried with exponential backoff up to `max_attempts`.
//!
//! To add a job type, pick a queue name and add an arm to `dispatch`.
use std::time::Duration;

use serde_json::Value;

use crate::state::AppState;

/// How long a claimed job is hidden from other workers.
const LEASE_SECS: i64 = 120;
const BATCH: i64 = 8;

#[derive(Debug, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub queue: String,
    pub payload: Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

/// Enqueues a job to run as soon as possible.
pub async fn enqueue<'e, E: sqlx::PgExecutor<'e>>(db: E, queue: &str, payload: &Value) -> sqlx::Result<i64> {
    sqlx::query_scalar("INSERT INTO jobs (queue, payload) VALUES ($1, $2) RETURNING id")
        .bind(queue)
        .bind(payload)
        .fetch_one(db)
        .await
}

/// Backoff before retry number `attempt` (1-based): 10s, 30s, 2m, 10m, 30m, 1h...
pub fn backoff_secs(attempt: i32) -> i64 {
    match attempt {
        0 | 1 => 10,
        2 => 30,
        3 => 120,
        4 => 600,
        5 => 1800,
        _ => 3600,
    }
}

/// Claims up to `limit` due jobs (leasing them) and returns them.
pub async fn claim(db: &sqlx::PgPool, limit: i64) -> sqlx::Result<Vec<Job>> {
    sqlx::query_as(
        "WITH due AS (
             SELECT id FROM jobs WHERE run_at <= now() ORDER BY run_at, id LIMIT $1 FOR UPDATE SKIP LOCKED
         )
         UPDATE jobs j SET run_at = now() + make_interval(secs => $2), attempts = j.attempts + 1
           FROM due WHERE j.id = due.id
         RETURNING j.id, j.queue, j.payload, j.attempts, j.max_attempts",
    )
    .bind(limit)
    .bind(LEASE_SECS as f64)
    .fetch_all(db)
    .await
}

async fn finish(db: &sqlx::PgPool, job: &Job, result: anyhow::Result<()>) -> sqlx::Result<()> {
    match result {
        Ok(()) => {
            sqlx::query("DELETE FROM jobs WHERE id = $1").bind(job.id).execute(db).await?;
        }
        Err(e) if job.attempts >= job.max_attempts => {
            tracing::warn!(job = job.id, queue = %job.queue, error = %format!("{e:#}"), "job failed permanently");
            sqlx::query("DELETE FROM jobs WHERE id = $1").bind(job.id).execute(db).await?;
        }
        Err(e) => {
            sqlx::query("UPDATE jobs SET run_at = now() + make_interval(secs => $2), last_error = $3 WHERE id = $1")
                .bind(job.id)
                .bind(backoff_secs(job.attempts) as f64)
                .bind(format!("{e:#}"))
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

async fn dispatch(state: &AppState, job: &Job) -> anyhow::Result<()> {
    match job.queue.as_str() {
        crate::webhooks::QUEUE => crate::webhooks::run_delivery_job(state, job).await,
        crate::migration::QUEUE => crate::migration::run_job(state, job).await,
        other => anyhow::bail!("unknown queue {other}"),
    }
}

/// Runs one polling round: claims due jobs and processes them concurrently. Returns how many
/// jobs were processed.
pub async fn run_once(state: &AppState) -> anyhow::Result<usize> {
    let jobs = claim(&state.db, BATCH).await?;
    let n = jobs.len();
    let mut set = tokio::task::JoinSet::new();
    for job in jobs {
        let st = state.clone();
        set.spawn(async move {
            let res = dispatch(&st, &job).await;
            if let Err(e) = finish(&st.db, &job, res).await {
                tracing::error!(job = job.id, error = %e, "could not record job result");
            }
        });
    }
    while set.join_next().await.is_some() {}
    Ok(n)
}

/// Starts the background worker. Safe to run in every replica.
pub fn spawn_worker(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match run_once(&state).await {
                Ok(n) if n > 0 => continue,
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "job worker poll failed"),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
}
