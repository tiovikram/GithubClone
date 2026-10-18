//! Credentials of the Actions system:
//!
//! - **Job tokens** (`GITHUB_TOKEN`): installation access tokens (`ghs_…`) of the built-in
//!   `github-actions` app (as on GitHub), minted when a runner claims a job. Each token covers
//!   only the job's repository, with the job's `permissions:` (or the repository's default
//!   workflow permissions), and dies when the job completes or at its timeout + 30 min. The
//!   apps area enforces them uniformly on REST, GraphQL and git smart HTTP
//!   (`apps::scope::middleware`); they act as `github-actions[bot]`.
//! - **Runner credentials** (`fgr_…`): issued at registration, identify one runner; the runner
//!   protocol (`protocol.rs`) accepts them only for jobs the runner holds.
//! - **Registration / remove tokens**: short-lived, created through the REST API (or the
//!   static `FORGE_RUNNER_REGISTRATION_TOKEN`).
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use crate::auth::sha256_hex;
use crate::db::{RepoCtx, User};
use crate::error::ApiResult;

pub const BOT_LOGIN: &str = "github-actions[bot]";
pub const ACTIONS_APP_SLUG: &str = "github-actions";
/// GitHub's id of the GitHub Actions app (used for the built-in app when free).
pub const ACTIONS_APP_ID: i64 = 15368;
pub const RUNNER_CREDENTIAL_PREFIX: &str = "fgr_";

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

/// Every permission a workflow token can hold (GitHub's `permissions:` keys, app names).
pub const TOKEN_PERMISSIONS: &[(&str, &str)] = &[
    ("actions", "actions"),
    ("checks", "checks"),
    ("contents", "contents"),
    ("deployments", "deployments"),
    ("discussions", "discussions"),
    ("issues", "issues"),
    ("packages", "packages"),
    ("pages", "pages"),
    ("pull-requests", "pull_requests"),
    ("repository-projects", "repository_projects"),
    ("security-events", "security_events"),
    ("statuses", "statuses"),
];

/// The built-in `github-actions` app (id 15368 when free), created on first use.
pub async fn actions_app(db: &sqlx::PgPool) -> ApiResult<i64> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM apps WHERE slug = $1").bind(ACTIONS_APP_SLUG).fetch_optional(db).await? {
        return Ok(id);
    }
    let bot = bot_user(db).await?;
    let mut all = Map::new();
    for (_, p) in TOKEN_PERMISSIONS {
        all.insert(p.to_string(), json!("write"));
    }
    all.insert("metadata".into(), json!("read"));
    let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM apps WHERE id = $1)").bind(ACTIONS_APP_ID).fetch_one(db).await?;
    let insert = format!(
        "INSERT INTO apps ({}slug, name, owner_id, bot_user_id, description, external_url, client_id, client_secret_hash, permissions, public)
         VALUES ({}$1, 'GitHub Actions', $2, $2, 'Automate your workflow from idea to production', 'https://help.github.com/en/actions',
                 'Iv1.github-actions', $3, $4, TRUE)
         ON CONFLICT DO NOTHING",
        if taken { "" } else { "id, " },
        if taken { "" } else { "15368, " },
    );
    sqlx::query(sqlx::AssertSqlSafe(insert))
        .bind(ACTIONS_APP_SLUG)
        .bind(bot.id)
        .bind(sha256_hex(&crate::auth::random_string(40)))
        .bind(Value::Object(all))
        .execute(db)
        .await?;
    Ok(sqlx::query_scalar("SELECT id FROM apps WHERE slug = $1").bind(ACTIONS_APP_SLUG).fetch_one(db).await?)
}

/// The app's installation on the repository owner, created on first use.
async fn installation(db: &sqlx::PgPool, app_id: i64, owner: &User) -> ApiResult<i64> {
    let target_type = if owner.kind == "Organization" { "Organization" } else { "User" };
    let perms = sqlx::query_scalar::<_, Value>("SELECT permissions FROM apps WHERE id = $1").bind(app_id).fetch_one(db).await?;
    sqlx::query(
        "INSERT INTO app_installations (app_id, target_id, target_type, repository_selection, permissions)
         VALUES ($1, $2, $3, 'all', $4) ON CONFLICT DO NOTHING",
    )
    .bind(app_id)
    .bind(owner.id)
    .bind(target_type)
    .bind(&perms)
    .execute(db)
    .await?;
    Ok(sqlx::query_scalar("SELECT id FROM app_installations WHERE app_id = $1 AND target_id = $2").bind(app_id).bind(owner.id).fetch_one(db).await?)
}

/// Mints the GITHUB_TOKEN of a job: an installation token for `ctx`'s repository only, with
/// `permissions` (app permission names -> `read`/`write`).
pub async fn mint_job_token(db: &sqlx::PgPool, job_id: i64, ctx: &RepoCtx, permissions: &Map<String, Value>, expires_at: DateTime<Utc>) -> ApiResult<String> {
    let app = actions_app(db).await?;
    let inst = installation(db, app, &ctx.owner).await?;
    let token = format!("ghs_{}", crate::auth::random_string(36));
    let hash = sha256_hex(&token);
    let mut perms = permissions.clone();
    perms.insert("metadata".into(), json!("read"));
    sqlx::query("INSERT INTO app_installation_tokens (installation_id, token_hash, repository_ids, permissions, expires_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(inst)
        .bind(&hash)
        .bind(vec![ctx.repo.id])
        .bind(Value::Object(perms.clone()))
        .bind(expires_at)
        .execute(db)
        .await?;
    let contents = perms.get("contents").and_then(Value::as_str).unwrap_or("read");
    sqlx::query("INSERT INTO actions_job_tokens (token_hash, job_id, repo_id, perm, expires_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(&hash)
        .bind(job_id)
        .bind(ctx.repo.id)
        .bind(if contents == "write" { "write" } else { "read" })
        .bind(expires_at)
        .execute(db)
        .await?;
    Ok(token)
}

/// Revokes the job tokens of a finished job.
pub async fn revoke_job_tokens(db: &sqlx::PgPool, job_id: i64) -> ApiResult<()> {
    sqlx::query("DELETE FROM app_installation_tokens WHERE token_hash IN (SELECT token_hash FROM actions_job_tokens WHERE job_id = $1)")
        .bind(job_id)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM actions_job_tokens WHERE job_id = $1").bind(job_id).execute(db).await?;
    Ok(())
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

/// `FORGE_RUNNER_REGISTRATION_TOKEN`: a long-lived, instance-wide registration token shared by
/// the server and runner pods through a Secret (Kubernetes runners register on start without
/// database access or a human creating a token). Re-seeded at every server start.
pub async fn seed_static_registration_token(db: &sqlx::PgPool) -> ApiResult<()> {
    let Ok(t) = std::env::var("FORGE_RUNNER_REGISTRATION_TOKEN") else { return Ok(()) };
    let t = t.trim();
    if t.len() < 16 {
        tracing::warn!("FORGE_RUNNER_REGISTRATION_TOKEN is shorter than 16 characters; ignored");
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO actions_runner_tokens (token_hash, kind, expires_at) VALUES ($1, 'registration', now() + interval '100 years')
         ON CONFLICT (token_hash) DO UPDATE SET expires_at = EXCLUDED.expires_at",
    )
    .bind(sha256_hex(t))
    .execute(db)
    .await?;
    Ok(())
}
