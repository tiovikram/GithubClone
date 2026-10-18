//! Organization private registry configurations (`/orgs/{org}/private-registries`).
//!
//! Secrets follow GitHub's secrets scheme: the client fetches the org's registry public key,
//! encrypts the token/password as a libsodium sealed box and sends `encrypted_value` + `key_id`.
//! Unlike Actions secrets (where forge never needs the plaintext), forge keeps the X25519
//! private half so a registry secret can be opened when used; it checks on write that the value
//! really is a sealed box for the current key (422 otherwise). Only the ciphertext is stored and
//! it is never returned.
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{created, no_content, PageParams};
use crate::auth::AuthUser;
use crate::db::User;
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::orgs::{audit_quiet, org_by_login, require_capability, sealed_box};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/private-registries", get(list).post(create))
        .route("/orgs/{org}/private-registries/public-key", get(public_key))
        .route("/orgs/{org}/private-registries/{name}", get(get_one).patch(update).delete(delete))
}

const READ: &[&str] = &["read_organization_private_registries", "write_organization_private_registries"];
const WRITE: &[&str] = &["write_organization_private_registries"];

pub const REGISTRY_TYPES: &[&str] = &[
    "maven_repository", "nuget_feed", "goproxy_server", "npm_registry", "rubygems_server", "cargo_registry", "composer_repository",
    "docker_registry", "git_source", "helm_registry", "hex_organization", "hex_repository", "pub_repository", "python_index",
    "terraform_registry",
];
const AUTH_TYPES: &[&str] = &["token", "username_password", "oidc_azure", "oidc_aws", "oidc_jfrog", "oidc_cloudsmith", "oidc_gcp"];
/// OIDC settings (strings), and the ones each OIDC auth type requires.
const OIDC_FIELDS: &[&str] = &[
    "tenant_id", "client_id", "aws_region", "account_id", "role_name", "domain", "domain_owner", "jfrog_oidc_provider_name",
    "audience", "identity_mapping_name", "namespace", "service_slug", "api_host", "workload_identity_provider", "service_account",
];
fn oidc_required(auth: &str) -> &'static [&'static str] {
    match auth {
        "oidc_azure" => &["tenant_id", "client_id"],
        "oidc_aws" => &["aws_region", "account_id", "role_name", "domain", "domain_owner"],
        "oidc_jfrog" => &["jfrog_oidc_provider_name"],
        "oidc_cloudsmith" => &["namespace", "service_slug"],
        "oidc_gcp" => &["workload_identity_provider", "service_account"],
        _ => &[],
    }
}

// ---------------------------------------------------------------------------------------------
// Key pair

/// The org's registry key pair `(key_id, public, secret)`, created on first use.
pub async fn registry_key(s: &AppState, org: &User) -> ApiResult<(String, [u8; 32], [u8; 32])> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let (sk, pk) = sealed_box::keypair();
    let key_id = (rand::random::<u64>() >> 4).to_string();
    sqlx::query("INSERT INTO org_registry_keys (org_id, key_id, public_key, private_key) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(org.id)
        .bind(&key_id)
        .bind(b64.encode(pk))
        .bind(b64.encode(sk))
        .execute(&s.db)
        .await?;
    let (id, p, k): (String, String, String) = sqlx::query_as("SELECT key_id, public_key, private_key FROM org_registry_keys WHERE org_id = $1")
        .bind(org.id)
        .fetch_one(&s.db)
        .await?;
    let dec = |x: &str| -> ApiResult<[u8; 32]> {
        b64.decode(x).ok().and_then(|v| v.try_into().ok()).ok_or_else(|| ApiError::internal("corrupt registry key"))
    };
    Ok((id, dec(&p)?, dec(&k)?))
}

/// Opens a stored registry secret (for consumers of the configuration).
pub async fn open_secret(s: &AppState, org: &User, encrypted_value: &str) -> ApiResult<Option<Vec<u8>>> {
    let (_, pk, sk) = registry_key(s, org).await?;
    let raw = base64::engine::general_purpose::STANDARD.decode(encrypted_value).ok();
    Ok(raw.and_then(|r| sealed_box::open(&r, &sk, &pk)))
}

async fn public_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ).await?;
    let (key_id, pk, _) = registry_key(&s, &o).await?;
    Ok(Json(json!({ "key_id": key_id, "key": base64::engine::general_purpose::STANDARD.encode(pk) })))
}

// ---------------------------------------------------------------------------------------------
// Rows and JSON

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RegistryRow {
    pub org_id: i64,
    pub name: String,
    pub registry_type: String,
    pub auth_type: Option<String>,
    pub url: String,
    pub username: Option<String>,
    pub replaces_base: bool,
    pub visibility: String,
    pub selected_repository_ids: Vec<i64>,
    pub oidc: Value,
    pub encrypted_value: Option<String>,
    pub key_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn registry_json(r: &RegistryRow, with_selected: bool) -> Value {
    let mut v = json!({
        "name": r.name,
        "registry_type": r.registry_type,
        "url": r.url,
        "replaces_base": r.replaces_base,
        "visibility": r.visibility,
        "created_at": crate::json::ts(r.created_at),
        "updated_at": crate::json::ts(r.updated_at),
    });
    if let Some(u) = &r.username {
        v["username"] = json!(u);
    }
    if let Some(a) = &r.auth_type {
        v["auth_type"] = json!(a);
    }
    if let Some(m) = r.oidc.as_object() {
        for (k, val) in m {
            v[k.as_str()] = val.clone();
        }
    }
    if with_selected && r.visibility == "selected" {
        v["selected_repository_ids"] = json!(r.selected_repository_ids);
    }
    v
}

async fn load(s: &AppState, org: &User, name: &str) -> ApiResult<RegistryRow> {
    sqlx::query_as("SELECT * FROM org_private_registries WHERE org_id = $1 AND name = $2")
        .bind(org.id)
        .bind(name)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

// ---------------------------------------------------------------------------------------------
// Validation

#[derive(Deserialize, Default)]
pub struct RegistryInput {
    pub registry_type: Option<String>,
    pub url: Option<String>,
    #[serde(default, deserialize_with = "crate::orgs::double_option")]
    pub username: Option<Option<String>>,
    pub replaces_base: Option<bool>,
    pub encrypted_value: Option<String>,
    pub key_id: Option<String>,
    pub visibility: Option<String>,
    pub selected_repository_ids: Option<Vec<i64>>,
    pub auth_type: Option<String>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

fn bad(field: &str, message: &str) -> ApiError {
    let mut e = ApiError::validation("PrivateRegistry", field, "invalid");
    e.errors = Some(json!([{ "resource": "PrivateRegistry", "field": field, "code": "invalid", "message": message }]));
    e
}

/// Validates the merged configuration (`cur` is the existing row on update).
async fn validate(s: &AppState, org: &User, req: &RegistryInput, cur: Option<&RegistryRow>) -> ApiResult<RegistryRow> {
    let registry_type = req.registry_type.clone().or_else(|| cur.map(|c| c.registry_type.clone()));
    let registry_type = registry_type.ok_or_else(|| ApiError::validation("PrivateRegistry", "registry_type", "missing_field"))?;
    if !REGISTRY_TYPES.contains(&registry_type.as_str()) {
        return Err(bad("registry_type", "Unsupported registry_type"));
    }
    let url = req.url.clone().or_else(|| cur.map(|c| c.url.clone())).ok_or_else(|| ApiError::validation("PrivateRegistry", "url", "missing_field"))?;
    if !url::Url::parse(&url).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
        return Err(bad("url", "url must be an http(s) URL"));
    }
    let visibility = req.visibility.clone().or_else(|| cur.map(|c| c.visibility.clone()));
    let visibility = visibility.ok_or_else(|| ApiError::validation("PrivateRegistry", "visibility", "missing_field"))?;
    if !["all", "private", "selected"].contains(&visibility.as_str()) {
        return Err(bad("visibility", "visibility must be all, private or selected"));
    }
    let selected = match (&req.selected_repository_ids, visibility.as_str()) {
        (Some(_), v) if v != "selected" => return Err(bad("selected_repository_ids", "selected_repository_ids requires visibility selected")),
        (Some(ids), _) => ids.clone(),
        (None, "selected") => cur.map(|c| c.selected_repository_ids.clone()).unwrap_or_default(),
        (None, _) => vec![],
    };
    if !selected.is_empty() {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE id = ANY($1) AND owner_id = $2").bind(&selected).bind(org.id).fetch_one(&s.db).await?;
        if n as usize != selected.iter().collect::<std::collections::HashSet<_>>().len() {
            return Err(bad("selected_repository_ids", "Repositories must belong to the organization"));
        }
    }
    let username = match &req.username {
        Some(u) => u.clone().filter(|x| !x.is_empty()),
        None => cur.and_then(|c| c.username.clone()),
    };
    let auth_type = req
        .auth_type
        .clone()
        .or_else(|| cur.and_then(|c| c.auth_type.clone()))
        .unwrap_or_else(|| if username.is_some() { "username_password".into() } else { "token".into() });
    if !AUTH_TYPES.contains(&auth_type.as_str()) {
        return Err(bad("auth_type", "Unsupported auth_type"));
    }
    if auth_type == "username_password" && username.is_none() {
        return Err(ApiError::validation("PrivateRegistry", "username", "missing_field"));
    }
    // OIDC settings: only for OIDC auth types, all required ones present
    let mut oidc = cur.and_then(|c| c.oidc.as_object().cloned()).unwrap_or_default();
    for (k, v) in &req.rest {
        if !OIDC_FIELDS.contains(&k.as_str()) {
            continue;
        }
        match v {
            Value::String(x) if !x.is_empty() => {
                oidc.insert(k.clone(), json!(x));
            }
            Value::Null => {
                oidc.remove(k);
            }
            _ => return Err(bad(k, &format!("{k} must be a string"))),
        }
    }
    if !auth_type.starts_with("oidc_") {
        oidc.clear();
    }
    for f in oidc_required(&auth_type) {
        if !oidc.contains_key(*f) {
            return Err(ApiError::validation("PrivateRegistry", f, "missing_field"));
        }
    }
    // secret: sealed box against the current key (required for token / username_password)
    let (mut encrypted, mut key_id) = (cur.and_then(|c| c.encrypted_value.clone()), cur.and_then(|c| c.key_id.clone()));
    if req.encrypted_value.is_some() || req.key_id.is_some() {
        let ev = req.encrypted_value.clone().ok_or_else(|| ApiError::validation("PrivateRegistry", "encrypted_value", "missing_field"))?;
        let kid = req.key_id.clone().ok_or_else(|| ApiError::validation("PrivateRegistry", "key_id", "missing_field"))?;
        let (cur_kid, pk, sk) = registry_key(s, org).await?;
        if kid != cur_kid {
            return Err(bad("key_id", "key_id does not match the organization's private registry public key"));
        }
        let raw = base64::engine::general_purpose::STANDARD.decode(&ev).map_err(|_| bad("encrypted_value", "encrypted_value must be base64"))?;
        if sealed_box::open(&raw, &sk, &pk).is_none() {
            return Err(bad("encrypted_value", "encrypted_value is not a sealed box for the organization's public key"));
        }
        (encrypted, key_id) = (Some(ev), Some(kid));
    }
    if matches!(auth_type.as_str(), "token" | "username_password") && encrypted.is_none() {
        return Err(ApiError::validation("PrivateRegistry", "encrypted_value", "missing_field"));
    }
    if auth_type.starts_with("oidc_") {
        (encrypted, key_id) = (None, None);
    }
    let now = chrono::Utc::now();
    Ok(RegistryRow {
        org_id: org.id,
        name: cur.map(|c| c.name.clone()).unwrap_or_default(),
        registry_type,
        auth_type: Some(auth_type),
        url,
        username,
        replaces_base: req.replaces_base.unwrap_or(cur.is_some_and(|c| c.replaces_base)),
        visibility,
        selected_repository_ids: selected,
        oidc: Value::Object(oidc),
        encrypted_value: encrypted,
        key_id,
        created_at: cur.map(|c| c.created_at).unwrap_or(now),
        updated_at: now,
    })
}

// ---------------------------------------------------------------------------------------------
// Handlers

async fn list(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ).await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM org_private_registries WHERE org_id = $1").bind(o.id).fetch_one(&s.db).await?;
    let rows: Vec<RegistryRow> = sqlx::query_as("SELECT * FROM org_private_registries WHERE org_id = $1 ORDER BY name LIMIT $2 OFFSET $3")
        .bind(o.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    Ok(Json(json!({ "total_count": total, "configurations": rows.iter().map(|r| registry_json(r, false)).collect::<Vec<_>>() })))
}

/// Creates a configuration; GitHub names it after the registry type (`MAVEN_REPOSITORY_SECRET`,
/// then `_2`, `_3` … for further ones of the same type).
pub async fn create_registry(s: &AppState, org: &User, actor: &User, req: RegistryInput) -> ApiResult<RegistryRow> {
    require_capability(s, org, actor, WRITE).await?;
    let row = validate(s, org, &req, None).await?;
    let base = format!("{}_SECRET", row.registry_type.to_uppercase());
    for i in 1..1000 {
        let name = if i == 1 { base.clone() } else { format!("{base}_{i}") };
        let r: Option<RegistryRow> = sqlx::query_as(
            "INSERT INTO org_private_registries (org_id, name, registry_type, auth_type, url, username, replaces_base, visibility,
                selected_repository_ids, oidc, encrypted_value, key_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
             ON CONFLICT DO NOTHING RETURNING *",
        )
        .bind(org.id)
        .bind(&name)
        .bind(&row.registry_type)
        .bind(&row.auth_type)
        .bind(&row.url)
        .bind(&row.username)
        .bind(row.replaces_base)
        .bind(&row.visibility)
        .bind(&row.selected_repository_ids)
        .bind(&row.oidc)
        .bind(&row.encrypted_value)
        .bind(&row.key_id)
        .fetch_optional(&s.db)
        .await?;
        if let Some(r) = r {
            audit_quiet(&s.db, org, Some(actor), "private_registry_configuration.create", json!({ "name": r.name, "registry_type": r.registry_type })).await;
            return Ok(r);
        }
    }
    Err(ApiError::unprocessable("Too many private registry configurations of this type."))
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(req): Json<RegistryInput>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    let r = create_registry(&s, &o, &u, req).await?;
    Ok(created(registry_json(&r, true)))
}

async fn get_one(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, READ).await?;
    Ok(Json(registry_json(&load(&s, &o, &name).await?, false)))
}

async fn update(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, name)): Path<(String, String)>, Json(req): Json<RegistryInput>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE).await?;
    let cur = load(&s, &o, &name).await?;
    let row = validate(&s, &o, &req, Some(&cur)).await?;
    sqlx::query(
        "UPDATE org_private_registries SET registry_type = $3, auth_type = $4, url = $5, username = $6, replaces_base = $7,
            visibility = $8, selected_repository_ids = $9, oidc = $10, encrypted_value = $11, key_id = $12, updated_at = now()
          WHERE org_id = $1 AND name = $2",
    )
    .bind(o.id)
    .bind(&name)
    .bind(&row.registry_type)
    .bind(&row.auth_type)
    .bind(&row.url)
    .bind(&row.username)
    .bind(row.replaces_base)
    .bind(&row.visibility)
    .bind(&row.selected_repository_ids)
    .bind(&row.oidc)
    .bind(&row.encrypted_value)
    .bind(&row.key_id)
    .execute(&s.db)
    .await?;
    audit_quiet(&s.db, &o, Some(&u), "private_registry_configuration.update", json!({ "name": name })).await;
    Ok(no_content())
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, name)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE).await?;
    let n = sqlx::query("DELETE FROM org_private_registries WHERE org_id = $1 AND name = $2").bind(o.id).bind(&name).execute(&s.db).await?;
    if n.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    audit_quiet(&s.db, &o, Some(&u), "private_registry_configuration.destroy", json!({ "name": name })).await;
    Ok(no_content())
}
