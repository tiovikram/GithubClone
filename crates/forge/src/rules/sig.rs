//! Commit signature verification for the `required_signatures` rule.
//!
//! A commit counts as verified when its signature checks out against a signing key of a forge
//! user whose email (primary, `/user/emails` or the forge noreply address) is the committer's
//! email, as on GitHub:
//! - SSH signatures (`gpg.format=ssh`): SSH signing keys (`/user/ssh_signing_keys`), verified
//!   in-process (ed25519, ECDSA) or with `ssh-keygen -Y verify` (RSA and anything else) when
//!   `ssh-keygen` is installed.
//! - OpenPGP signatures: GPG keys (`/user/gpg_keys`), verified with `gpg` in a throwaway home
//!   directory when `gpg` is installed; without `gpg` they count as unverified.
//! - x509 (S/MIME) signatures are not supported (unverified).
use std::collections::HashMap;

use russh::keys::ssh_key::{PublicKey, SshSig};

use crate::git::read::CommitInfo;
use crate::state::AppState;

/// Signing keys of the users owning `email`: (ssh keys, gpg armored keys).
async fn keys_for_email(s: &AppState, email: &str) -> (Vec<String>, Vec<String>) {
    let lower = email.to_lowercase();
    let mut ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM users WHERE lower(email) = $1 UNION SELECT user_id FROM user_emails WHERE lower(email) = $1 AND verified",
    )
    .bind(&lower)
    .fetch_all(&s.db)
    .await
    .unwrap_or_default();
    if let Some(local) = lower.strip_suffix("@users.noreply.forge") {
        if let Some(id) = local.split_once('+').and_then(|(id, _)| id.parse::<i64>().ok()) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return (vec![], vec![]);
    }
    let ssh: Vec<String> = sqlx::query_scalar("SELECT key FROM ssh_signing_keys WHERE user_id = ANY($1)").bind(&ids).fetch_all(&s.db).await.unwrap_or_default();
    let gpg: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(raw_key, public_key) FROM gpg_keys WHERE user_id = ANY($1) AND primary_key_id IS NULL AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(&ids)
    .fetch_all(&s.db)
    .await
    .unwrap_or_default();
    (ssh, gpg)
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

async fn ssh_keygen_verify(key: &str, sig: &str, payload: &[u8]) -> bool {
    let Some(bin) = which("ssh-keygen") else { return false };
    let Ok(dir) = tempdir() else { return false };
    let signers = dir.join("allowed_signers");
    let sigf = dir.join("sig");
    let ok = async {
        tokio::fs::write(&signers, format!("signer {}\n", key.trim())).await.ok()?;
        tokio::fs::write(&sigf, sig).await.ok()?;
        let mut child = tokio::process::Command::new(bin)
            .args(["-Y", "verify", "-f"])
            .arg(&signers)
            .args(["-I", "signer", "-n", "git", "-s"])
            .arg(&sigf)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take()?;
        stdin.write_all(payload).await.ok()?;
        drop(stdin);
        Some(child.wait().await.ok()?.success())
    }
    .await
    .unwrap_or(false);
    let _ = tokio::fs::remove_dir_all(&dir).await;
    ok
}

fn tempdir() -> std::io::Result<std::path::PathBuf> {
    let d = std::env::temp_dir().join(format!("forge-sig-{}", crate::auth::random_string(12)));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

async fn verify_ssh(keys: &[String], sig: &str, payload: &[u8]) -> bool {
    let Ok(parsed) = SshSig::from_pem(sig.trim()) else { return false };
    for k in keys {
        let Ok(pk) = PublicKey::from_openssh(k.trim()) else { continue };
        if pk.key_data() != parsed.public_key() {
            continue;
        }
        match pk.verify("git", payload, &parsed) {
            Ok(()) => return true,
            Err(russh::keys::ssh_key::Error::Signature) => continue,
            // Algorithms this build can't verify (e.g. RSA): ask ssh-keygen.
            Err(_) => {
                if ssh_keygen_verify(k, sig, payload).await {
                    return true;
                }
            }
        }
    }
    false
}

async fn verify_gpg(keys: &[String], sig: &str, payload: &[u8]) -> bool {
    if keys.is_empty() {
        return false;
    }
    let Some(bin) = which("gpg") else { return false };
    let Ok(home) = tempdir() else { return false };
    let ok = async {
        let keyf = home.join("keys.asc");
        tokio::fs::write(&keyf, keys.join("\n")).await.ok()?;
        let sigf = home.join("sig.asc");
        let payf = home.join("payload");
        tokio::fs::write(&sigf, sig).await.ok()?;
        tokio::fs::write(&payf, payload).await.ok()?;
        let base = |c: &mut tokio::process::Command| {
            c.arg("--homedir").arg(&home).args(["--batch", "--no-tty", "--quiet"]).kill_on_drop(true);
        };
        let mut imp = tokio::process::Command::new(&bin);
        base(&mut imp);
        imp.arg("--import").arg(&keyf).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        imp.status().await.ok()?;
        let mut v = tokio::process::Command::new(&bin);
        base(&mut v);
        v.args(["--status-fd", "1", "--verify"]).arg(&sigf).arg(&payf).stderr(std::process::Stdio::null());
        let out = v.output().await.ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        Some(out.status.success() && text.contains("[GNUPG:] VALIDSIG"))
    }
    .await
    .unwrap_or(false);
    let _ = tokio::fs::remove_dir_all(&home).await;
    ok
}

/// Whether one commit carries a verified signature.
pub async fn verified(s: &AppState, c: &CommitInfo, cache: &mut HashMap<String, (Vec<String>, Vec<String>)>) -> bool {
    let (Some(sig), Some(payload)) = (&c.signature, &c.payload) else { return false };
    let email = c.committer.email.to_lowercase();
    if !cache.contains_key(&email) {
        let keys = keys_for_email(s, &email).await;
        cache.insert(email.clone(), keys);
    }
    let (ssh, gpg) = cache.get(&email).cloned().unwrap_or_default();
    if sig.contains("BEGIN SSH SIGNATURE") {
        verify_ssh(&ssh, sig, payload.as_bytes()).await
    } else if sig.contains("BEGIN PGP SIGNATURE") {
        verify_gpg(&gpg, sig, payload.as_bytes()).await
    } else {
        false
    }
}

/// Shas of the commits without a verified signature.
pub async fn unverified(s: &AppState, commits: &[CommitInfo]) -> Vec<String> {
    let mut cache = HashMap::new();
    let mut out = Vec::new();
    for c in commits {
        if !verified(s, c, &mut cache).await {
            out.push(c.sha.clone());
        }
    }
    out
}
