//! Credentials of the Actions system:
//!
//! - **Job tokens** (`GITHUB_TOKEN`, prefix `ghs_`): minted when a runner claims a job, valid for
//!   that job's repository only, until the job completes (deleted) or expires. They authenticate
//!   as the `github-actions[bot]` user; [`scope_middleware`] puts the token's scope in a
//!   task-local for the request, and `db::permission` asks [`scoped_permission`] for the bot's
//!   permission on a repository: the token's permission (read or write, never admin) on its own
//!   repository, read on public repositories, nothing elsewhere. Without a scope (e.g. a spawned
//!   task) the bot has no access at all (fail closed).
//! - **Runner credentials** (prefix `ghr_`): issued at registration, identify one runner.
//! - **Registration / remove tokens**: short-lived, created through the REST API.
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use chrono::{DateTime, Utc};

use crate::auth::sha256_hex;
use crate::db::{Perm, Repo, User};
use crate::error::ApiResult;
use crate::state::AppState;

pub const BOT_LOGIN: &str = "github-actions[bot]";
pub const JOB_TOKEN_PREFIX: &str = "ghs_";
pub const RUNNER_CREDENTIAL_PREFIX: &str = "ghr_";

#[derive(Clone, Debug)]
pub struct JobScope {
    pub job_id: i64,
    pub repo_id: i64,
    pub perm: Perm,
}

tokio::task_local! {
    static SCOPE: Option<JobScope>;
}

/// The `github-actions[bot]` user, created on first use.
pub async fn bot_user(db: &sqlx::PgPool) -> ApiResult<User> {
    if let Some(u) = sqlx::query_as::<_, User>("SELECT * FROM users WHERE lower(login) = lower($1)").bind(BOT_LOGIN).fetch_optional(db).await? {
        return Ok(u);
    }
    sqlx::query("INSERT INTO users (login, kind, name) VALUES ($1, 'Bot', 'github-actions') ON CONFLICT DO NOTHING")
        .bind(BOT_LOGIN)
        .execute(db)
        .await?;
    Ok(sqlx::query_as::<_, User>("SELECT * FROM users WHERE lower(login) = lower($1)").bind(BOT_LOGIN).fetch_one(db).await?)
}

pub fn is_actions_bot(u: &User) -> bool {
    u.kind == "Bot" && u.login.eq_ignore_ascii_case(BOT_LOGIN)
}

/// Mints a job token for `job_id` on `repo_id`.
pub async fn mint_job_token(db: &sqlx::PgPool, job_id: i64, repo_id: i64, perm: Perm, expires_at: DateTime<Utc>) -> ApiResult<String> {
    let token = format!("{JOB_TOKEN_PREFIX}{}", crate::auth::random_string(36));
    sqlx::query("INSERT INTO actions_job_tokens (token_hash, job_id, repo_id, perm, expires_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(sha256_hex(&token))
        .bind(job_id)
        .bind(repo_id)
        .bind(if perm >= Perm::Write { "write" } else { "read" })
        .bind(expires_at)
        .execute(db)
        .await?;
    Ok(token)
}

/// The scope of a job token, if it is valid.
pub async fn job_scope(db: &sqlx::PgPool, token: &str) -> ApiResult<Option<JobScope>> {
    if !token.starts_with(JOB_TOKEN_PREFIX) {
        return Ok(None);
    }
    let row: Option<(i64, i64, String)> =
        sqlx::query_as("SELECT job_id, repo_id, perm FROM actions_job_tokens WHERE token_hash = $1 AND expires_at > now()")
            .bind(sha256_hex(token))
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(job_id, repo_id, perm)| JobScope { job_id, repo_id, perm: if perm == "write" { Perm::Write } else { Perm::Read } }))
}

/// Called by `auth::user_for_token` for `ghs_` tokens: the bot user if the token is valid.
pub async fn user_for_job_token(db: &sqlx::PgPool, token: &str) -> ApiResult<Option<User>> {
    if job_scope(db, token).await?.is_none() {
        return Ok(None);
    }
    Ok(Some(bot_user(db).await?))
}

/// Called by `db::permission` for the bot user.
pub fn scoped_permission(repo: &Repo) -> Perm {
    let public = if repo.private { Perm::None } else { Perm::Read };
    SCOPE
        .try_with(|s| match s {
            Some(sc) if sc.repo_id == repo.id => sc.perm,
            Some(_) => public,
            None => Perm::None,
        })
        .unwrap_or(Perm::None)
}

/// The job scope of the current request, if it was authenticated with a job token.
pub fn current_scope() -> Option<JobScope> {
    SCOPE.try_with(|s| s.clone()).ok().flatten()
}

fn bearer(req: &Request) -> Option<String> {
    use base64::Engine;
    let h = req.headers().get(axum::http::header::AUTHORIZATION)?.to_str().ok()?.trim().to_string();
    let (scheme, cred) = h.split_once(' ')?;
    let cred = cred.trim();
    match scheme.to_ascii_lowercase().as_str() {
        "token" | "bearer" => Some(cred.to_string()),
        "basic" => {
            let d = base64::engine::general_purpose::STANDARD.decode(cred).ok()?;
            let d = String::from_utf8(d).ok()?;
            Some(d.split_once(':').map(|(_, p)| p.to_string()).unwrap_or(d))
        }
        _ => None,
    }
}

/// Outermost middleware: requests carrying a job token run with that token's scope.
pub async fn scope_middleware(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let scope = match bearer(&req) {
        Some(t) if t.starts_with(JOB_TOKEN_PREFIX) => job_scope(&s.db, &t).await.ok().flatten(),
        _ => None,
    };
    SCOPE.scope(scope, next.run(req)).await
}

/// A runner identified by its credential.
#[derive(sqlx::FromRow, Clone, Debug)]
pub struct RunnerRow {
    pub id: i64,
    pub repo_id: Option<i64>,
    pub org_id: Option<i64>,
    pub name: String,
    pub os: String,
    pub arch: String,
    pub labels: Vec<String>,
    pub credential_hash: Option<String>,
    pub ephemeral: bool,
    pub version: Option<String>,
    pub runner_group_id: i64,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn runner_for_credential(db: &sqlx::PgPool, cred: &str) -> ApiResult<Option<RunnerRow>> {
    if !cred.starts_with(RUNNER_CREDENTIAL_PREFIX) {
        return Ok(None);
    }
    Ok(sqlx::query_as("SELECT * FROM actions_runners WHERE credential_hash = $1").bind(sha256_hex(cred)).fetch_optional(db).await?)
}

pub fn new_runner_credential() -> String {
    format!("{RUNNER_CREDENTIAL_PREFIX}{}", crate::auth::random_string(40))
}

/// Where a runner (or a registration token) belongs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunnerScope {
    Repo(i64),
    Org(i64),
    Enterprise,
}

impl RunnerScope {
    pub fn ids(&self) -> (Option<i64>, Option<i64>) {
        match self {
            RunnerScope::Repo(r) => (Some(*r), None),
            RunnerScope::Org(o) => (None, Some(*o)),
            RunnerScope::Enterprise => (None, None),
        }
    }
    pub fn of(repo_id: Option<i64>, org_id: Option<i64>) -> RunnerScope {
        match (repo_id, org_id) {
            (Some(r), _) => RunnerScope::Repo(r),
            (None, Some(o)) => RunnerScope::Org(o),
            _ => RunnerScope::Enterprise,
        }
    }
}

/// Creates a registration or remove token (GitHub: valid for one hour).
pub async fn create_runner_token(db: &sqlx::PgPool, kind: &str, scope: &RunnerScope, ttl_secs: i64) -> ApiResult<(String, DateTime<Utc>)> {
    let token = crate::auth::random_string(29).to_uppercase();
    let expires = Utc::now() + chrono::Duration::seconds(ttl_secs);
    let (repo_id, org_id) = scope.ids();
    sqlx::query("INSERT INTO actions_runner_tokens (token_hash, kind, repo_id, org_id, expires_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(sha256_hex(&token))
        .bind(kind)
        .bind(repo_id)
        .bind(org_id)
        .bind(expires)
        .execute(db)
        .await?;
    Ok((token, expires))
}

/// The scope of a valid registration/remove token.
pub async fn runner_token_scope(db: &sqlx::PgPool, kind: &str, token: &str) -> ApiResult<Option<RunnerScope>> {
    let row: Option<(Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT repo_id, org_id FROM actions_runner_tokens WHERE token_hash = $1 AND kind = $2 AND expires_at > now()")
            .bind(sha256_hex(token.trim()))
            .bind(kind)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(r, o)| RunnerScope::of(r, o)))
}
