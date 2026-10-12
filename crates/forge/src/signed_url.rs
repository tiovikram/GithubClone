//! Short-lived signed tokens for URLs that are followed without credentials, like GitHub's
//! codeload `?token=` on archive redirects (the API's `Authorization` header is not carried
//! across the redirect). The HMAC key lives in Postgres (`signing_secrets`) so every pod
//! issues and accepts the same tokens.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::PgPool;

/// Lifetime of archive tokens (GitHub's codeload tokens last a few minutes).
pub const ARCHIVE_TOKEN_TTL_SECS: i64 = 300;

fn cache() -> &'static Mutex<HashMap<String, String>> {
    static C: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The named server secret, generated on first use (race-free across pods).
pub async fn server_secret(db: &PgPool, name: &str) -> anyhow::Result<String> {
    if let Some(s) = cache().lock().unwrap().get(name) {
        return Ok(s.clone());
    }
    let fresh = crate::auth::random_string(48);
    sqlx::query("INSERT INTO signing_secrets (name, secret) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING")
        .bind(name)
        .bind(&fresh)
        .execute(db)
        .await?;
    let s: String = sqlx::query_scalar("SELECT secret FROM signing_secrets WHERE name = $1").bind(name).fetch_one(db).await?;
    cache().lock().unwrap().insert(name.to_string(), s.clone());
    Ok(s)
}

fn mac(secret: &str, repo_id: i64, rev: &str, exp: i64) -> String {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("any key length");
    m.update(format!("archive\0{repo_id}\0{rev}\0{exp}").as_bytes());
    hex::encode(&m.finalize().into_bytes()[..20])
}

/// A token authorizing a download of `rev` of repository `repo_id` until `now + TTL`.
pub async fn archive_token(db: &PgPool, repo_id: i64, rev: &str) -> anyhow::Result<String> {
    let secret = server_secret(db, "archive").await?;
    let exp = chrono::Utc::now().timestamp() + ARCHIVE_TOKEN_TTL_SECS;
    Ok(format!("{exp:x}{}", mac(&secret, repo_id, rev, exp)))
}

/// Whether `token` is a valid, unexpired archive token for (`repo_id`, `rev`).
pub async fn verify_archive_token(db: &PgPool, repo_id: i64, rev: &str, token: &str) -> bool {
    if token.len() <= 40 || !token.is_ascii() {
        return false;
    }
    let (exp_hex, sig) = token.split_at(token.len() - 40);
    let Ok(exp) = i64::from_str_radix(exp_hex, 16) else { return false };
    if exp < chrono::Utc::now().timestamp() {
        return false;
    }
    let Ok(secret) = server_secret(db, "archive").await else { return false };
    let want = mac(&secret, repo_id, rev, exp);
    // Constant-time comparison.
    want.len() == sig.len() && want.bytes().zip(sig.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}
