//! SSH key shapes: `key`, `key-simple`, `deploy-key`.
use serde_json::{json, Value};

use super::{ts, ts_opt};
use crate::ssh::keys::{DeployKeyRow, SshKeyRow};

/// `key` (GET/POST /user/keys).
pub fn key(api: &str, k: &SshKeyRow) -> Value {
    json!({
        "key": k.key,
        "id": k.id,
        "url": format!("{api}/user/keys/{}", k.id),
        "title": k.title,
        "created_at": ts(k.created_at),
        "verified": true,
        "read_only": false,
        "last_used": ts_opt(k.last_used),
    })
}

/// `key-simple` (GET /users/{username}/keys).
pub fn key_simple(k: &SshKeyRow) -> Value {
    json!({ "id": k.id, "key": k.key })
}

/// `deploy-key` (GET/POST /repos/{owner}/{repo}/keys).
pub fn deploy_key(api: &str, full_name: &str, k: &DeployKeyRow, added_by: Option<&str>) -> Value {
    json!({
        "id": k.id,
        "key": k.key,
        "url": format!("{api}/repos/{full_name}/keys/{}", k.id),
        "title": k.title,
        "verified": true,
        "created_at": ts(k.created_at),
        "read_only": k.read_only,
        "added_by": added_by,
        "last_used": ts_opt(k.last_used),
        "enabled": true,
    })
}
