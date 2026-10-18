//! Actions secret encryption.
//!
//! Clients encrypt a secret with libsodium's `crypto_box_seal` (X25519 + XSalsa20-Poly1305)
//! against the owner's public key from `GET .../secrets/public-key`; forge stores that
//! ciphertext unchanged. Each owner (repository or organization; environments use their
//! repository's key) has an X25519 key pair. Its private half is stored sealed with the
//! **master key**, so a database dump alone does not reveal secrets when the master key lives
//! outside the database:
//!
//! - `FORGE_SECRET_KEY` (any string; the X25519 master secret is `sha256(value)`), recommended
//!   for production (mount it from a Kubernetes Secret). Every pod must see the same value.
//! - otherwise a random master key generated once and stored in `signing_secrets`
//!   (`actions-master-key`), which protects only against leaks of individual columns.
//!
//! A sealed private key records which master sealed it (`v1:env:` / `v1:db:`), so switching to
//! `FORGE_SECRET_KEY` later keeps older keys readable as long as the DB master still exists.
use base64::Engine;
use crypto_box::aead::OsRng;
use crypto_box::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn env_master() -> Option<SecretKey> {
    let v = std::env::var("FORGE_SECRET_KEY").ok().filter(|v| !v.is_empty())?;
    let d: [u8; 32] = Sha256::digest(v.as_bytes()).into();
    Some(SecretKey::from_bytes(d))
}

static DB_MASTER: OnceCell<[u8; 32]> = OnceCell::const_new();

async fn db_master(s: &AppState) -> ApiResult<SecretKey> {
    let bytes = DB_MASTER
        .get_or_try_init(|| async {
            let fresh: [u8; 32] = rand::random();
            sqlx::query("INSERT INTO signing_secrets (name, secret) VALUES ('actions-master-key', $1) ON CONFLICT DO NOTHING")
                .bind(B64.encode(fresh))
                .execute(&s.db)
                .await?;
            let stored: String = sqlx::query_scalar("SELECT secret FROM signing_secrets WHERE name = 'actions-master-key'").fetch_one(&s.db).await?;
            let v = B64.decode(stored).map_err(ApiError::internal)?;
            <[u8; 32]>::try_from(v.as_slice()).map_err(ApiError::internal)
        })
        .await?;
    Ok(SecretKey::from_bytes(*bytes))
}

/// Seals an owner's private key with the current master key.
async fn seal_private(s: &AppState, sk: &SecretKey) -> ApiResult<String> {
    let (tag, master) = match env_master() {
        Some(m) => ("env", m),
        None => ("db", db_master(s).await?),
    };
    let ct = master.public_key().seal(&mut OsRng, &sk.to_bytes()).map_err(|_| ApiError::internal("seal failed"))?;
    Ok(format!("v1:{tag}:{}", B64.encode(ct)))
}

async fn open_private(s: &AppState, sealed: &str) -> ApiResult<SecretKey> {
    let mut parts = sealed.splitn(3, ':');
    let (Some("v1"), Some(tag), Some(data)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(ApiError::internal("unknown sealed key format"));
    };
    let master = match tag {
        "env" => env_master().ok_or_else(|| ApiError::internal("actions key sealed with FORGE_SECRET_KEY, which is not set"))?,
        _ => db_master(s).await?,
    };
    let raw = master.unseal(&B64.decode(data).map_err(ApiError::internal)?).map_err(|_| ApiError::internal("cannot unseal actions key (wrong FORGE_SECRET_KEY?)"))?;
    SecretKey::from_slice(&raw).map_err(ApiError::internal)
}

/// The owner's (key_id, base64 public key), creating a key pair on first use. Keys created
/// before secrets became decryptable (no private half) are replaced with a new key id.
pub async fn public_key(s: &AppState, kind: &str, id: i64) -> ApiResult<(String, String)> {
    let row: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT key_id, key, sealed_private_key FROM actions_public_keys WHERE owner_kind = $1 AND owner_id = $2")
            .bind(kind)
            .bind(id)
            .fetch_optional(&s.db)
            .await?;
    if let Some((key_id, key, Some(_))) = row {
        return Ok((key_id, key));
    }
    let sk = SecretKey::from_bytes(rand::random());
    let key = B64.encode(sk.public_key().as_bytes());
    let key_id = (rand::random::<u64>() >> 4).to_string();
    let sealed = seal_private(s, &sk).await?;
    sqlx::query(
        "INSERT INTO actions_public_keys (owner_kind, owner_id, key_id, key, sealed_private_key) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (owner_kind, owner_id) DO UPDATE SET key_id = EXCLUDED.key_id, key = EXCLUDED.key,
             sealed_private_key = EXCLUDED.sealed_private_key, created_at = now()
           WHERE actions_public_keys.sealed_private_key IS NULL",
    )
    .bind(kind)
    .bind(id)
    .bind(&key_id)
    .bind(&key)
    .bind(&sealed)
    .execute(&s.db)
    .await?;
    Ok(sqlx::query_as("SELECT key_id, key FROM actions_public_keys WHERE owner_kind = $1 AND owner_id = $2")
        .bind(kind)
        .bind(id)
        .fetch_one(&s.db)
        .await?)
}

/// The owner's private key, if it has one.
pub async fn private_key(s: &AppState, kind: &str, id: i64) -> ApiResult<Option<(String, SecretKey)>> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT key_id, sealed_private_key FROM actions_public_keys WHERE owner_kind = $1 AND owner_id = $2")
            .bind(kind)
            .bind(id)
            .fetch_optional(&s.db)
            .await?;
    match row {
        Some((key_id, Some(sealed))) => Ok(Some((key_id, open_private(s, &sealed).await?))),
        _ => Ok(None),
    }
}

/// Opens a base64 sealed box with `sk`; None when it is not a valid box for this key.
pub fn open_sealed(sk: &SecretKey, encrypted_b64: &str) -> Option<String> {
    let ct = B64.decode(encrypted_b64.trim()).ok()?;
    let pt = sk.unseal(&ct).ok()?;
    String::from_utf8(pt).ok()
}

/// Seals `plaintext` for a base64 public key (what a client does; used by tests and tooling).
pub fn seal_for(public_key_b64: &str, plaintext: &str) -> ApiResult<String> {
    let raw = B64.decode(public_key_b64).map_err(|_| ApiError::bad_request("invalid public key"))?;
    let pk = PublicKey::from_slice(&raw).map_err(|_| ApiError::bad_request("invalid public key"))?;
    let ct = pk.seal(&mut OsRng, plaintext.as_bytes()).map_err(|_| ApiError::internal("seal failed"))?;
    Ok(B64.encode(ct))
}
