//! GHES Manage API (`/manage/v1/...`, served under /api/v3 like the rest of the spec) for forge.
//!
//! forge is not an appliance: it runs as stateless pods over Postgres + object storage. These
//! endpoints report that view honestly:
//! - nodes / cluster / replication status: the forge processes that recorded a heartbeat in the
//!   last 5 minutes (`ent_nodes`), with live checks of Postgres, object storage and the git
//!   cache; there are no GHES-style replicas (replication is the storage layer's job);
//! - settings: stored in `ent_settings` and applied immediately (a config apply run is recorded
//!   and completes at once); `signup_enabled` is enforced by `/api/forge/signup`;
//!   `repository_invitations.enabled` (forge-only; default `FORGE_REPO_INVITATIONS`) switches
//!   `PUT /collaborators/{u}` to the invitation flow;
//! - maintenance mode: real (503 for everyone but site admins / the IP exception list);
//! - license: forge has no license; it reports an unlimited, perpetual one;
//! - SSH access keys, upgrades: recorded; forge is upgraded by deploying a new image.
//! Site administrators only.
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::enterprise::{self as ent, Maintenance, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let m = "/manage/v1";
    Router::new()
        .route(&format!("{m}/access/ssh"), get(get_ssh).post(add_ssh).delete(delete_ssh))
        .route(&format!("{m}/checks/system-requirements"), get(system_requirements))
        .route(&format!("{m}/cluster/status"), get(cluster_status))
        .route(&format!("{m}/config/apply"), get(apply_status).post(apply))
        .route(&format!("{m}/config/apply/events"), get(apply_events))
        .route(&format!("{m}/config/init"), post(init))
        .route(&format!("{m}/config/license"), get(license_info).put(license_upload))
        .route(&format!("{m}/config/license/check"), get(license_check))
        .route(&format!("{m}/config/nodes"), get(nodes))
        .route(&format!("{m}/config/settings"), get(get_settings).put(put_settings))
        .route(&format!("{m}/maintenance"), get(get_maintenance).post(set_maintenance))
        .route(&format!("{m}/replication/status"), get(replication_status))
        .route(&format!("{m}/upgrade/apply"), post(upgrade_apply))
        .route(&format!("{m}/upgrade/download"), post(upgrade_download))
        .route(&format!("{m}/upgrade/download/status"), get(upgrade_download_status))
        .route(&format!("{m}/upgrade/status"), get(upgrade_status))
        .route(&format!("{m}/version"), get(version))
}

const GHES_VERSION: &str = "3.14.0";

#[derive(sqlx::FromRow, Clone)]
struct Node {
    uuid: String,
    hostname: String,
    version: String,
    started_at: chrono::DateTime<chrono::Utc>,
}

/// Live forge nodes (heartbeat in the last 5 minutes); always includes this process.
async fn live_nodes(s: &AppState) -> ApiResult<Vec<Node>> {
    ent::record_heartbeat(&s.db).await?;
    Ok(sqlx::query_as("SELECT uuid, hostname, version, started_at FROM ent_nodes WHERE last_seen_at > now() - interval '5 minutes' ORDER BY started_at, uuid")
        .fetch_all(&s.db)
        .await?)
}

#[derive(Deserialize)]
struct NodeQuery {
    uuid: Option<String>,
    #[allow(dead_code)]
    cluster_roles: Option<String>,
}

fn filter_nodes(nodes: Vec<Node>, uuid: &Option<String>) -> Vec<Node> {
    nodes.into_iter().filter(|n| uuid.as_ref().is_none_or(|u| *u == n.uuid)).collect()
}

/// (name, ok, details) for the services this node depends on.
async fn service_checks(s: &AppState) -> Vec<(&'static str, bool, String)> {
    let db = sqlx::query("SELECT 1").execute(&s.db).await;
    let store = {
        use futures::StreamExt;
        let mut l = s.store.list(Some(&object_store::path::Path::from("repos")));
        match l.next().await {
            None | Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(e),
        }
    };
    let cache = tokio::fs::create_dir_all(&s.cfg.cache_dir).await;
    vec![
        ("postgresql", db.is_ok(), db.err().map(|e| e.to_string()).unwrap_or_else(|| "reachable".into())),
        ("object-storage", store.is_ok(), store.err().map(|e| e.to_string()).unwrap_or_else(|| "reachable".into())),
        ("git-cache", cache.is_ok(), cache.err().map(|e| e.to_string()).unwrap_or_else(|| s.cfg.cache_dir.display().to_string())),
    ]
}

fn status_of(ok: bool) -> &'static str {
    if ok { "OK" } else { "CRITICAL" }
}

async fn health_view(s: &AppState, uuid: &Option<String>, replication: bool) -> ApiResult<Value> {
    let nodes = filter_nodes(live_nodes(s).await?, uuid);
    let checks = service_checks(s).await;
    let me = &ent::node_identity().0;
    let mut all_ok = true;
    let nodes: Vec<Value> = nodes
        .iter()
        .map(|n| {
            // Shared services are checked from this node; other nodes report via heartbeat.
            let services: Vec<Value> = if replication {
                vec![json!({
                    "status": "OK",
                    "name": "storage-replication",
                    "details": "forge keeps no node-local state: repositories live in object storage and metadata in PostgreSQL, so there are no replicas to sync",
                })]
            } else if n.uuid == *me {
                checks.iter().map(|(name, ok, d)| json!({"status": status_of(*ok), "name": name, "details": d})).collect()
            } else {
                vec![json!({"status": "OK", "name": "forge", "details": format!("heartbeat within 5 minutes (version {})", n.version)})]
            };
            let ok = services.iter().all(|x| x["status"] == "OK");
            all_ok &= ok;
            json!({ "hostname": n.hostname, "status": status_of(ok), "services": services })
        })
        .collect();
    Ok(json!({ "status": if nodes.is_empty() { "UNKNOWN" } else { status_of(all_ok) }, "nodes": nodes }))
}

async fn cluster_status(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    Ok(Json(health_view(&s, &None, false).await?))
}

async fn replication_status(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<NodeQuery>) -> ApiResult<Json<Value>> {
    Ok(Json(health_view(&s, &q.uuid, true).await?))
}

async fn system_requirements(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    let nodes = live_nodes(&s).await?;
    let checks = service_checks(&s).await;
    let ok = checks.iter().all(|c| c.1);
    let me = &ent::node_identity().0;
    Ok(Json(json!({
        "status": if ok { "OK" } else { "FAILED" },
        "nodes": nodes.iter().map(|n| {
            let mine = n.uuid == *me;
            json!({
                "hostname": n.hostname,
                "status": if !mine || ok { "OK" } else { "FAILED" },
                "roles_status": [
                    {"status": if !mine || ok { "OK" } else { "FAILED" }, "role": "WebServer"},
                    {"status": if !mine || ok { "OK" } else { "FAILED" }, "role": "GitServer"},
                    {"status": if !mine || ok { "OK" } else { "FAILED" }, "role": "JobServer"},
                ],
            })
        }).collect::<Vec<_>>(),
    })))
}

async fn nodes(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<NodeQuery>) -> ApiResult<Json<Value>> {
    let nodes = filter_nodes(live_nodes(&s).await?, &q.uuid);
    Ok(Json(json!({
        "topology": if nodes.len() > 1 { "Cluster" } else { "SingleNode" },
        "nodes": nodes.iter().map(|n| json!({
            "hostname": n.hostname,
            "uuid": n.uuid,
            "replica": false,
            "cluster_roles": ["WebServer", "GitServer", "JobServer"],
        })).collect::<Vec<_>>(),
    })))
}

async fn version(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<NodeQuery>) -> ApiResult<Json<Value>> {
    let nodes = filter_nodes(live_nodes(&s).await?, &q.uuid);
    Ok(Json(json!(nodes
        .iter()
        .map(|n| json!({
            "hostname": n.hostname,
            "version": {
                // The GHES release whose API forge implements (also X-GitHub-Enterprise-Version);
                // the forge build itself is identified by build_id.
                "version": GHES_VERSION,
                // The schema only allows hypervisor names; forge runs in containers on any of them.
                "platform": "kvm",
                "build_id": crate::auth::sha256_hex(&format!("forge-{}", n.version))[..8].to_string(),
                "build_date": n.started_at.format("%Y-%m-%d").to_string(),
            },
        }))
        .collect::<Vec<_>>())))
}

// ---------------------------------------------------------------------------------------------
// Settings and config runs

fn settings_defaults(s: &AppState) -> Value {
    json!({
        "private_mode": false,
        "public_pages": false,
        "subdomain_isolation": false,
        "signup_enabled": true,
        "github_hostname": s.cfg.host(),
        "identicons_host": "dotcom",
        "http_proxy": null,
        "auth_mode": "default",
        "expire_sessions": false,
        "admin_password": null,
        "configuration_id": 1,
        "configuration_run_count": 0,
        "avatar": {"enabled": false, "uri": ""},
        "customer": {"name": ent::slug(), "email": "", "uuid": ent::node_identity().0, "secret_key_data": "", "public_key_data": ""},
        "license": {"seats": 0, "evaluation": false, "perpetual": true, "unlimited_seating": true, "support_key": "", "ssh_allowed": true, "cluster_support": true, "expire_at": "2126-01-01T00:00:00Z"},
        "github_ssl": {"enabled": s.base_url().starts_with("https://"), "cert": null, "key": null},
        "ldap": null,
        "cas": {"url": null},
        "saml": {"sso_url": null, "certificate": null, "certificate_path": null, "issuer": null, "idp_initiated_sso": false, "disable_admin_demote": false},
        "github_oauth": null,
        "smtp": {"enabled": false, "address": "", "authentication": "", "port": "", "domain": "", "username": "", "user_name": "", "enable_starttls_auto": false, "password": "", "discard-to-noreply-address": true, "support_address": "", "support_address_type": "email", "noreply_address": ""},
        "ntp": {"primary_server": "", "secondary_server": ""},
        "timezone": null,
        "snmp": {"enabled": false, "community": ""},
        "syslog": {"enabled": false, "server": null, "protocol_name": "udp"},
        "assets": null,
        "pages": {"enabled": false},
        "collectd": {"enabled": false, "server": null, "port": 25826, "encryption": null, "username": null, "password": null},
        "mapping": {"enabled": false, "tileserver": null, "basemap": "", "token": null},
        "load_balancer": null,
        "prometheus": {"enabled": false, "trusted_ips": null},
        // forge: repository invitations (github.com flow) instead of GHES's direct collaborator add.
        "repository_invitations": {"enabled": ent::repo_invitations_env_default()},
    })
}

/// Management console settings (stored values over defaults). `signup_enabled` is read by
/// `/api/forge/signup`.
pub async fn manage_settings(s: &AppState) -> ApiResult<Value> {
    let mut v = ent::setting_with_defaults(&s.db, "manage_settings", settings_defaults(s)).await?;
    // ldap/github_oauth are objects in the schema but absent by default
    if let Some(o) = v.as_object_mut() {
        for k in ["ldap", "github_oauth"] {
            if o.get(k).is_some_and(Value::is_null) {
                o.remove(k);
            }
        }
        o.insert("admin_password".into(), Value::Null);
    }
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_config_runs").fetch_one(&s.db).await?;
    v["configuration_run_count"] = json!(runs);
    Ok(v)
}

async fn get_settings(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    Ok(Json(manage_settings(&s).await?))
}

fn merge(dst: &mut Value, src: &Value) {
    match (dst, src) {
        (Value::Object(d), Value::Object(s)) => {
            for (k, v) in s {
                match d.get_mut(k) {
                    Some(x) if x.is_object() && v.is_object() => merge(x, v),
                    _ => {
                        d.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (d, s) => *d = s.clone(),
    }
}

async fn put_settings(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<Value>) -> ApiResult<Response> {
    let Some(obj) = b.as_object() else { return Err(ApiError::bad_request("Settings must be a JSON object")) };
    let defaults = settings_defaults(&s);
    for k in obj.keys() {
        if defaults.get(k).is_none() && k != "ldap" && k != "github_oauth" {
            return Err(ApiError::bad_request(format!("Unknown setting: {k}")));
        }
    }
    if obj.get("signup_enabled").is_some_and(|v| !v.is_boolean()) {
        return Err(ApiError::bad_request("signup_enabled must be a boolean"));
    }
    if let Some(ri) = obj.get("repository_invitations") {
        if !ri.is_object() || ri.get("enabled").is_some_and(|v| !v.is_boolean()) {
            return Err(ApiError::bad_request("repository_invitations.enabled must be a boolean"));
        }
    }
    let mut clean = b.clone();
    if let Some(o) = clean.as_object_mut() {
        o.remove("configuration_run_count");
        o.remove("admin_password");
    }
    // Read-modify-write under a row lock so concurrent PUTs of different keys don't lose updates.
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO ent_settings (key, value) VALUES ('manage_settings', '{}'::jsonb) ON CONFLICT (key) DO NOTHING").execute(&mut *tx).await?;
    let mut stored: Value = sqlx::query_scalar("SELECT value FROM ent_settings WHERE key = 'manage_settings' FOR UPDATE").fetch_one(&mut *tx).await?;
    merge(&mut stored, &clean);
    sqlx::query("UPDATE ent_settings SET value = $1, updated_at = now() WHERE key = 'manage_settings'").bind(&stored).execute(&mut *tx).await?;
    tx.commit().await?;
    ent::audit(&s, "business.manage_settings_update", Some(&u), json!({"keys": obj.keys().collect::<Vec<_>>()})).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(sqlx::FromRow)]
struct RunRow {
    id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    successful: Option<bool>,
    events: Value,
}

#[derive(Deserialize)]
struct RunBody {
    run_id: Option<String>,
}

/// Settings take effect as soon as they are stored; a run records that and completes at once.
async fn apply(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(body): Json<RunBody>) -> ApiResult<Json<Value>> {
    let id = body.run_id.filter(|x| !x.is_empty()).unwrap_or_else(|| crate::auth::random_string(10).to_lowercase());
    let (_, host, _) = ent::node_identity();
    let now = chrono::Utc::now();
    let events = json!([
        {"timestamp": crate::json::ts(now), "severity_text": "INFO", "body": "Settings validated", "event_name": "config_apply.validate", "topology": "SingleNode", "hostname": host, "config_run_id": id},
        {"timestamp": crate::json::ts(now), "severity_text": "INFO", "body": "Settings are read from PostgreSQL by every forge process; nothing to restart", "event_name": "config_apply.done", "topology": "SingleNode", "hostname": host, "config_run_id": id},
    ]);
    sqlx::query(
        "INSERT INTO ent_config_runs (id, requested_by, finished_at, successful, events) VALUES ($1, $2, now(), TRUE, $3)
         ON CONFLICT (id) DO UPDATE SET finished_at = now(), successful = TRUE, events = $3",
    )
    .bind(&id)
    .bind(u.id)
    .bind(&events)
    .execute(&s.db)
    .await?;
    ent::audit(&s, "business.config_apply", Some(&u), json!({"run_id": id})).await;
    Ok(Json(json!({ "run_id": id })))
}

#[derive(Deserialize)]
struct RunQuery {
    run_id: Option<String>,
    last_request_id: Option<String>,
}

async fn latest_run(s: &AppState, id: Option<&str>) -> ApiResult<Option<RunRow>> {
    Ok(sqlx::query_as("SELECT id, created_at, finished_at, successful, events FROM ent_config_runs WHERE ($1::text IS NULL OR id = $1) ORDER BY created_at DESC LIMIT 1")
        .bind(id)
        .fetch_optional(&s.db)
        .await?)
}

async fn apply_status(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<RunQuery>) -> ApiResult<Json<Value>> {
    let run = latest_run(&s, q.run_id.as_deref()).await?;
    if q.run_id.is_some() && run.is_none() {
        return Err(ApiError::not_found());
    }
    let (_, host, _) = ent::node_identity();
    Ok(Json(match run {
        Some(r) => json!({
            "running": r.finished_at.is_none(),
            "successful": r.successful.unwrap_or(false),
            "nodes": [{ "run_id": r.id, "hostname": host, "running": r.finished_at.is_none(), "successful": r.successful.unwrap_or(false) }],
        }),
        None => json!({ "running": false, "successful": true, "nodes": [] }),
    }))
}

async fn apply_events(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<RunQuery>) -> ApiResult<Json<Value>> {
    let run = latest_run(&s, None).await?;
    let (_, host, _) = ent::node_identity();
    let events = run.as_ref().map(|r| r.events.clone()).unwrap_or_else(|| json!([]));
    let last = run.as_ref().map(|r| format!("{}-{}", r.id, r.created_at.timestamp())).unwrap_or_default();
    let unchanged = q.last_request_id.as_deref() == Some(last.as_str());
    Ok(Json(json!({
        "nodes": [{ "node": host, "last_request_id": last, "events": if unchanged { json!([]) } else { events } }],
    })))
}

#[derive(Deserialize)]
struct InitBody {
    license: Option<String>,
    password: Option<String>,
}

/// First-time configuration: stores the (unvalidated) license and the management password.
async fn init(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<InitBody>) -> ApiResult<Response> {
    let lic = b.license.filter(|x| !x.is_empty()).ok_or_else(|| ApiError::bad_request("license is required"))?;
    let pw = b.password.filter(|x| x.len() >= 8).ok_or_else(|| ApiError::bad_request("password must have at least 8 characters"))?;
    let hash = tokio::task::spawn_blocking(move || crate::auth::hash_password(&pw)).await.map_err(ApiError::internal)??;
    ent::set_setting(&s.db, "manage_password", &json!({ "hash": hash })).await?;
    ent::set_setting(&s.db, "license", &json!({ "data": lic, "uploaded_at": crate::json::ts(chrono::Utc::now()) })).await?;
    ent::audit(&s, "business.config_init", Some(&u), json!({})).await;
    Ok(StatusCode::ACCEPTED.into_response())
}

async fn license_info(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    let used: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'User' AND suspended_at IS NULL").fetch_one(&s.db).await?;
    let _ = used;
    Ok(Json(json!({
        "advancedSecurityEnabled": true,
        "advancedSecuritySeats": 0,
        "clusterSupport": true,
        "company": ent::slug(),
        "croquetSupport": true,
        "customTerms": false,
        "evaluation": false,
        "expireAt": "2126-01-01T00:00:00Z",
        "insightsEnabled": false,
        "insightsExpireAt": "2126-01-01T00:00:00Z",
        "learningLabEvaluationExpires": "2126-01-01T00:00:00Z",
        "learningLabSeats": 0,
        "perpetual": true,
        "referenceNumber": "forge",
        "seats": 0,
        "sshAllowed": true,
        "supportKey": "",
        "unlimitedSeating": true,
    })))
}

#[derive(Deserialize)]
struct UploadQuery {
    apply: Option<bool>,
}

async fn license_upload(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Query(q): Query<UploadQuery>, Json(b): Json<InitBody>) -> ApiResult<Response> {
    let lic = b.license.filter(|x| !x.is_empty()).ok_or_else(|| ApiError::bad_request("license is required"))?;
    ent::set_setting(&s.db, "license", &json!({ "data": lic, "uploaded_at": crate::json::ts(chrono::Utc::now()) })).await?;
    ent::audit(&s, "business.license_upload", Some(&u), json!({})).await;
    Ok(if q.apply.unwrap_or(false) { StatusCode::ACCEPTED } else { StatusCode::CREATED }.into_response())
}

async fn license_check(SiteAdmin(_): SiteAdmin) -> Json<Value> {
    Json(json!({ "status": "valid" }))
}

// ---------------------------------------------------------------------------------------------
// Maintenance

fn maintenance_json(m: &Maintenance, host: &str, uuid: &str) -> Value {
    let mut v = json!({
        "hostname": host,
        "uuid": uuid,
        "status": m.status(),
        "connection_services": [],
        "can_unset_maintenance": true,
        "ip_exception_list": m.ip_exception_list,
        "maintenance_mode_message": m.message,
    });
    if let Some(w) = m.when {
        v["scheduled_time"] = json!(w.format("%Y-%m-%d").to_string());
    }
    v
}

async fn get_maintenance(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<NodeQuery>) -> ApiResult<Json<Value>> {
    ent::invalidate_maintenance_cache();
    let m = ent::maintenance(&s).await;
    let nodes = filter_nodes(live_nodes(&s).await?, &q.uuid);
    Ok(Json(json!(nodes.iter().map(|n| maintenance_json(&m, &n.hostname, &n.uuid)).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct MaintenanceBody {
    enabled: Option<bool>,
    #[allow(dead_code)]
    uuid: Option<String>,
    when: Option<String>,
    ip_exception_list: Option<Vec<String>>,
    maintenance_mode_message: Option<String>,
}

async fn set_maintenance(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<MaintenanceBody>) -> ApiResult<Json<Value>> {
    let enabled = b.enabled.ok_or_else(|| ApiError::bad_request("enabled is required"))?;
    let when = match b.when.as_deref().filter(|w| !w.is_empty() && *w != "now") {
        Some(w) => Some(
            chrono::DateTime::parse_from_rfc3339(w)
                .map_err(|_| ApiError::bad_request("when must be an ISO 8601 date-time"))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    for ip in b.ip_exception_list.iter().flatten() {
        let addr = ip.split('/').next().unwrap_or("");
        if addr.parse::<std::net::IpAddr>().is_err() {
            return Err(ApiError::bad_request(format!("invalid IP address or CIDR: {ip}")));
        }
    }
    let v = json!({
        "enabled": enabled,
        "when": when.map(crate::json::ts),
        "ip_exception_list": b.ip_exception_list.unwrap_or_default(),
        "maintenance_mode_message": b.maintenance_mode_message.unwrap_or_default(),
    });
    ent::set_setting(&s.db, "maintenance", &v).await?;
    ent::invalidate_maintenance_cache();
    ent::audit(&s, if enabled { "business.maintenance_on" } else { "business.maintenance_off" }, Some(&u), v.clone()).await;
    let nodes = live_nodes(&s).await?;
    let msg = if enabled { "maintenance mode enabled" } else { "maintenance mode disabled" };
    Ok(Json(json!(nodes.iter().map(|n| json!({"hostname": n.hostname, "uuid": n.uuid, "message": msg})).collect::<Vec<_>>())))
}

// ---------------------------------------------------------------------------------------------
// SSH access (recorded; forge pods have no administrative shell)

async fn ssh_keys(s: &AppState) -> ApiResult<Vec<Value>> {
    Ok(ent::get_setting(&s.db, "manage_ssh_keys").await?.and_then(|v| v.as_array().cloned()).unwrap_or_default())
}

async fn get_ssh(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    Ok(Json(json!(ssh_keys(&s).await?)))
}

#[derive(Deserialize)]
struct KeyBody {
    key: Option<String>,
}

fn fingerprint(key: &str) -> ApiResult<String> {
    let blob = key.split_whitespace().nth(1).ok_or_else(|| ApiError::bad_request("key is not an OpenSSH public key"))?;
    let raw = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, blob).map_err(|_| ApiError::bad_request("key is not an OpenSSH public key"))?;
    let digest = <sha2::Sha256 as sha2::Digest>::digest(&raw);
    Ok(format!("SHA256:{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, digest)))
}

async fn add_ssh(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<KeyBody>) -> ApiResult<Json<Value>> {
    let key = b.key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).ok_or_else(|| ApiError::bad_request("key is required"))?;
    let fp = fingerprint(&key)?;
    let mut keys = ssh_keys(&s).await?;
    let modified = !keys.iter().any(|k| k["fingerprint"] == fp);
    if modified {
        keys.push(json!({"key": key, "fingerprint": fp}));
        ent::set_setting(&s.db, "manage_ssh_keys", &json!(keys)).await?;
        ent::audit(&s, "business.manage_ssh_key_add", Some(&u), json!({"fingerprint": fp})).await;
    }
    let nodes = live_nodes(&s).await?;
    Ok(Json(json!(nodes.iter().map(|n| json!({
        "hostname": n.hostname, "uuid": n.uuid, "message": if modified { "SSH key added successfully" } else { "SSH key already exists" }, "modified": modified,
    })).collect::<Vec<_>>())))
}

async fn delete_ssh(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<KeyBody>) -> ApiResult<Json<Value>> {
    let key = b.key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).ok_or_else(|| ApiError::bad_request("key is required"))?;
    let fp = fingerprint(&key)?;
    let mut keys = ssh_keys(&s).await?;
    let before = keys.len();
    keys.retain(|k| k["fingerprint"] != fp);
    let removed = keys.len() != before;
    if removed {
        ent::set_setting(&s.db, "manage_ssh_keys", &json!(keys)).await?;
        ent::audit(&s, "business.manage_ssh_key_remove", Some(&u), json!({"fingerprint": fp})).await;
    }
    let nodes = live_nodes(&s).await?;
    Ok(Json(json!(nodes.iter().map(|n| json!({
        "hostname": n.hostname, "uuid": n.uuid, "message": if removed { "SSH key removed successfully" } else { "SSH key not found" },
    })).collect::<Vec<_>>())))
}

// ---------------------------------------------------------------------------------------------
// Upgrades: forge is upgraded by rolling out a new container image.

const UPGRADE_MSG: &str = "forge is upgraded by deploying a new image (kubectl set image / helm upgrade); there is no upgrade package";

async fn upgrade_apply(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    let nodes = live_nodes(&s).await?;
    Ok(Json(json!(nodes.iter().map(|n| json!({"hostname": n.hostname, "uuid": n.uuid, "message": UPGRADE_MSG, "error": "not supported"})).collect::<Vec<_>>())))
}

async fn upgrade_download(State(s): State<AppState>, SiteAdmin(_): SiteAdmin) -> ApiResult<Json<Value>> {
    let nodes = live_nodes(&s).await?;
    Ok(Json(json!(nodes
        .iter()
        .map(|n| json!({"hostname": n.hostname, "uuid": n.uuid, "download_id": "", "message": UPGRADE_MSG, "error": "not supported"}))
        .collect::<Vec<_>>())))
}

async fn upgrade_download_status(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<NodeQuery>) -> ApiResult<Json<Value>> {
    let nodes = filter_nodes(live_nodes(&s).await?, &q.uuid);
    Ok(Json(json!({
        "download_id": "",
        "package_name": "",
        "status": "not_started",
        "error_message": UPGRADE_MSG,
        "nodes": nodes.iter().map(|n| json!({"hostname": n.hostname, "uuid": n.uuid, "download_id": "", "package_name": "", "status": "not_started", "progress": "0", "error": ""})).collect::<Vec<_>>(),
        "orchestration": {"phase": "completed", "error_message": "", "max_parallel": 1, "queued": 0, "in_flight": 0, "completed": 0, "failed": 0, "total": 0},
    })))
}

async fn upgrade_status(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<NodeQuery>) -> ApiResult<Json<Value>> {
    let nodes = filter_nodes(live_nodes(&s).await?, &q.uuid);
    Ok(Json(json!(nodes
        .iter()
        .map(|n| json!({
            "hostname": n.hostname,
            "uuid": n.uuid,
            "release_version": n.version,
            "release_sha": null,
            "status": "not_started",
            "pre_upgrade_status": "not_started",
            "upgrade_last_completed_step": null,
            "is_running": false,
            "failed": false,
            "steps": [],
            "messages": [UPGRADE_MSG],
            "error": "",
        }))
        .collect::<Vec<_>>())))
}
