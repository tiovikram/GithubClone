//! SSH public keys: parsing/validation, fingerprints, and the service layer for user keys
//! (`ssh_keys`) and deploy keys (`deploy_keys`). REST handlers live in `api/keys.rs`.
use chrono::{DateTime, Utc};
use russh::keys::ssh_key::{Algorithm, HashAlg, PublicKey};
use serde_json::json;
use sqlx::PgPool;

use crate::db::User;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SshKeyRow {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub key: String,
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeployKeyRow {
    pub id: i64,
    pub repo_id: i64,
    pub title: String,
    pub key: String,
    pub fingerprint: String,
    pub read_only: bool,
    pub added_by_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

/// A validated public key.
#[derive(Debug, Clone)]
pub struct ParsedKey {
    /// Normalized `algo base64` (no comment), as GitHub returns it.
    pub key: String,
    /// `SHA256:...` (OpenSSH format).
    pub fingerprint: String,
    pub comment: String,
}

/// SHA256 fingerprint in OpenSSH format (`SHA256:<base64 without padding>`).
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

fn invalid_key() -> ApiError {
    custom_key_error("key is invalid. You must supply a key in OpenSSH public key format")
}

fn custom_key_error(message: &str) -> ApiError {
    ApiError {
        status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        message: "Validation Failed".into(),
        errors: Some(json!([{ "resource": "PublicKey", "code": "custom", "field": "key", "message": message }])),
    }
}

/// Parses an OpenSSH public key line. Accepts ssh-ed25519, ecdsa-sha2-nistp{256,384,521} and
/// ssh-rsa (>= 1024 bits).
pub fn parse_public_key(text: &str) -> ApiResult<ParsedKey> {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return Err(invalid_key());
    }
    let key = PublicKey::from_openssh(text).map_err(|_| invalid_key())?;
    match key.algorithm() {
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. } => {}
        Algorithm::Rsa { .. } => {
            let bits = key.key_data().rsa().map(|r| r.n().as_positive_bytes().map(|b| b.len() * 8).unwrap_or(0)).unwrap_or(0);
            if bits < 1024 {
                return Err(invalid_key());
            }
        }
        _ => return Err(invalid_key()),
    }
    let mut bare = key.clone();
    bare.set_comment("");
    let normalized = bare.to_openssh().map_err(|_| invalid_key())?.trim().to_string();
    Ok(ParsedKey { fingerprint: fingerprint(&key), key: normalized, comment: key.comment().to_string() })
}

/// Serializes key insertion so a fingerprint can't land in both tables concurrently.
const KEY_LOCK: i64 = 0x5353_4800_0001;

async fn fingerprint_in_use(tx: &mut sqlx::PgConnection, fp: &str) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ssh_keys WHERE fingerprint = $1)
             OR EXISTS(SELECT 1 FROM deploy_keys WHERE fingerprint = $1)",
    )
    .bind(fp)
    .fetch_one(&mut *tx)
    .await?)
}

pub struct NewKey {
    pub title: Option<String>,
    pub key: Option<String>,
}

fn validate_new(input: &NewKey) -> ApiResult<(ParsedKey, String)> {
    let Some(text) = input.key.as_deref() else {
        return Err(ApiError::validation("PublicKey", "key", "missing_field"));
    };
    let parsed = parse_public_key(text)?;
    let title = input.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| parsed.comment.clone());
    Ok((parsed, title))
}

/// Adds an SSH key to `user`'s account (POST /user/keys). 422 "key is already in use" if the
/// key is registered anywhere (any user, or as a deploy key).
pub async fn add_user_key(db: &PgPool, user: &User, input: NewKey) -> ApiResult<SshKeyRow> {
    let (parsed, title) = validate_new(&input)?;
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(KEY_LOCK).execute(&mut *tx).await?;
    if fingerprint_in_use(&mut tx, &parsed.fingerprint).await? {
        return Err(custom_key_error("key is already in use"));
    }
    let row: SshKeyRow = sqlx::query_as(
        "INSERT INTO ssh_keys (user_id, title, key, fingerprint) VALUES ($1, $2, $3, $4) RETURNING *",
    )
    .bind(user.id)
    .bind(&title)
    .bind(&parsed.key)
    .bind(&parsed.fingerprint)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn list_user_keys(db: &PgPool, user_id: i64, limit: i64, offset: i64) -> ApiResult<(Vec<SshKeyRow>, i64)> {
    let rows = sqlx::query_as("SELECT * FROM ssh_keys WHERE user_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?;
    let total = sqlx::query_scalar("SELECT count(*) FROM ssh_keys WHERE user_id = $1").bind(user_id).fetch_one(db).await?;
    Ok((rows, total))
}

pub async fn get_user_key(db: &PgPool, user_id: i64, key_id: i64) -> ApiResult<SshKeyRow> {
    sqlx::query_as("SELECT * FROM ssh_keys WHERE id = $1 AND user_id = $2")
        .bind(key_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn delete_user_key(db: &PgPool, user_id: i64, key_id: i64) -> ApiResult<()> {
    let n = sqlx::query("DELETE FROM ssh_keys WHERE id = $1 AND user_id = $2")
        .bind(key_id)
        .bind(user_id)
        .execute(db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

pub struct NewDeployKey {
    pub title: Option<String>,
    pub key: Option<String>,
    pub read_only: Option<bool>,
}

/// Adds a deploy key to a repository (POST /repos/{o}/{r}/keys). Read-only unless
/// `read_only == Some(false)`. Caller must have checked admin permission.
pub async fn add_deploy_key(db: &PgPool, repo_id: i64, actor: &User, input: NewDeployKey) -> ApiResult<DeployKeyRow> {
    let (parsed, title) = validate_new(&NewKey { title: input.title, key: input.key })?;
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(KEY_LOCK).execute(&mut *tx).await?;
    if fingerprint_in_use(&mut tx, &parsed.fingerprint).await? {
        return Err(custom_key_error("key is already in use"));
    }
    let row: DeployKeyRow = sqlx::query_as(
        "INSERT INTO deploy_keys (repo_id, title, key, fingerprint, read_only, added_by_id)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(repo_id)
    .bind(&title)
    .bind(&parsed.key)
    .bind(&parsed.fingerprint)
    .bind(input.read_only.unwrap_or(true))
    .bind(actor.id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn list_deploy_keys(db: &PgPool, repo_id: i64, limit: i64, offset: i64) -> ApiResult<(Vec<DeployKeyRow>, i64)> {
    let rows = sqlx::query_as("SELECT * FROM deploy_keys WHERE repo_id = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(repo_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?;
    let total = sqlx::query_scalar("SELECT count(*) FROM deploy_keys WHERE repo_id = $1").bind(repo_id).fetch_one(db).await?;
    Ok((rows, total))
}

pub async fn get_deploy_key(db: &PgPool, repo_id: i64, key_id: i64) -> ApiResult<DeployKeyRow> {
    sqlx::query_as("SELECT * FROM deploy_keys WHERE id = $1 AND repo_id = $2")
        .bind(key_id)
        .bind(repo_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn delete_deploy_key(db: &PgPool, repo_id: i64, key_id: i64) -> ApiResult<()> {
    let n = sqlx::query("DELETE FROM deploy_keys WHERE id = $1 AND repo_id = $2")
        .bind(key_id)
        .bind(repo_id)
        .execute(db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

/// Who a presented SSH key authenticates as.
#[derive(Debug, Clone)]
pub enum KeyOwner {
    User { key_id: i64, user: User },
    Deploy(DeployKeyRow),
}

/// Resolves a key fingerprint to its owner (user key or deploy key).
pub async fn owner_by_fingerprint(db: &PgPool, fp: &str) -> anyhow::Result<Option<KeyOwner>> {
    let user: Option<(i64, i64)> = sqlx::query_as("SELECT id, user_id FROM ssh_keys WHERE fingerprint = $1")
        .bind(fp)
        .fetch_optional(db)
        .await?;
    if let Some((key_id, user_id)) = user {
        let u: Option<User> = sqlx::query_as("SELECT * FROM users WHERE id = $1").bind(user_id).fetch_optional(db).await?;
        return Ok(u.map(|user| KeyOwner::User { key_id, user }));
    }
    let dk: Option<DeployKeyRow> = sqlx::query_as("SELECT * FROM deploy_keys WHERE fingerprint = $1")
        .bind(fp)
        .fetch_optional(db)
        .await?;
    Ok(dk.map(KeyOwner::Deploy))
}

/// Records that a key was used (best effort).
pub async fn touch(db: &PgPool, owner: &KeyOwner) {
    let res = match owner {
        KeyOwner::User { key_id, .. } => {
            sqlx::query("UPDATE ssh_keys SET last_used = now() WHERE id = $1").bind(key_id).execute(db).await
        }
        KeyOwner::Deploy(d) => {
            sqlx::query("UPDATE deploy_keys SET last_used = now() WHERE id = $1").bind(d.id).execute(db).await
        }
    };
    if let Err(e) = res {
        tracing::warn!(error = %e, "failed to record ssh key use");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_ed25519() {
        let k = parse_public_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me@host\n",
        )
        .unwrap();
        assert_eq!(k.key, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl");
        assert_eq!(k.comment, "me@host");
        assert!(k.fingerprint.starts_with("SHA256:"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_public_key("ssh-ed25519 notbase64").is_err());
        assert!(parse_public_key("").is_err());
        assert!(parse_public_key("hello world").is_err());
    }
}
