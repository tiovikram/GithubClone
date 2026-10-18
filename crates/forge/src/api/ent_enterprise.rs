//! Enterprise-scoped endpoints (`/enterprises/{enterprise}/...`, `/enterprise/live-migrations`):
//! audit log and audit log streams, push-rule bypass requests, code security defaults and
//! enablement, enterprise custom properties, ruleset history, GitHub Advanced Security billing
//! and live migrations. Site administrators only (org billing: org owners too).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::User;
use crate::enterprise::{self as ent, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let e = "/enterprises/{enterprise}";
    Router::new()
        .route(&format!("{e}/audit-log"), get(audit_log))
        .route(&format!("{e}/audit-log/stream-key"), get(stream_key))
        .route(&format!("{e}/audit-log/streams"), get(list_streams).post(create_stream))
        .route(&format!("{e}/audit-log/streams/{{stream_id}}"), get(get_stream).put(update_stream).delete(delete_stream))
        .route(&format!("{e}/bypass-requests/push-rules"), get(bypass_requests))
        .route(&format!("{e}/code_security_and_analysis"), get(get_code_security).patch(patch_code_security))
        .route(&format!("{e}/{{security_product}}/{{enablement}}"), post(product_enablement))
        .route(&format!("{e}/org-properties/schema"), get(list_org_props).patch(upsert_org_props))
        .route(&format!("{e}/org-properties/schema/{{name}}"), get(get_org_prop).put(put_org_prop).delete(delete_org_prop))
        .route(&format!("{e}/org-properties/values"), get(list_org_values).patch(set_org_values))
        .route(&format!("{e}/properties/schema"), get(list_repo_props).patch(upsert_repo_props))
        .route(&format!("{e}/properties/schema/organizations/{{org}}/{{name}}/promote"), put(promote_prop))
        .route(&format!("{e}/properties/schema/{{name}}"), get(get_repo_prop).put(put_repo_prop).delete(delete_repo_prop))
        .route(&format!("{e}/rulesets/{{ruleset_id}}/history"), get(ruleset_history))
        .route(&format!("{e}/rulesets/{{ruleset_id}}/history/{{version_id}}"), get(ruleset_version))
        .route(&format!("{e}/settings/billing/advanced-security"), get(billing_enterprise))
        .route("/orgs/{org}/settings/billing/advanced-security", get(billing_org))
        .route("/enterprise/live-migrations", get(list_migrations).post(create_migration))
        .route("/enterprise/live-migrations/{migration_id}", get(get_migration))
        .route("/enterprise/live-migrations/{migration_id}/{action}", post(migration_action))
}

fn ms(t: chrono::DateTime<chrono::Utc>) -> i64 {
    t.timestamp_millis()
}

// ---------------------------------------------------------------------------------------------
// Audit log: instance events (ent_audit_log) and organization events (org_audit_log)

#[derive(Deserialize)]
struct AuditQuery {
    phrase: Option<String>,
    #[allow(dead_code)]
    include: Option<String>,
    after: Option<String>,
    before: Option<String>,
    order: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    doc: String,
    action: String,
    actor_id: Option<i64>,
    actor: Option<String>,
    user_login: Option<String>,
    org_id: Option<i64>,
    org: Option<String>,
    repo: Option<String>,
    team: Option<String>,
    data: Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Parses `key:value` qualifiers of the audit log `phrase` (action, actor, user, org, repo,
/// team, created:>=DATE / <=DATE / DATE). Unknown words are matched against the action.
fn parse_phrase(p: &str) -> Vec<(String, String)> {
    p.split_whitespace()
        .map(|w| match w.split_once(':') {
            Some((k, v)) => (k.to_lowercase(), v.trim_matches('"').to_string()),
            None => ("action".to_string(), w.to_string()),
        })
        .collect()
}

async fn audit_log(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Path(e): Path<String>, Query(q): Query<AuditQuery>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    let mut conds: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    let mut add = |sql: &str, v: String, binds: &mut Vec<String>| {
        binds.push(v);
        conds.push(sql.replace("$N", &format!("${}", binds.len())));
    };
    for (k, v) in parse_phrase(q.phrase.as_deref().unwrap_or("")) {
        match k.as_str() {
            "action" => add("(action = $N OR action LIKE $N || '.%')", v, &mut binds),
            "actor" => add("lower(actor) = lower($N)", v, &mut binds),
            "user" => add("lower(user_login) = lower($N)", v, &mut binds),
            "org" => add("lower(org) = lower($N)", v, &mut binds),
            "repo" => add("lower(repo) = lower($N)", v, &mut binds),
            "team" => add("lower(team) = lower($N)", v, &mut binds),
            "created" => {
                let (op, d) = if let Some(d) = v.strip_prefix(">=") {
                    (">=", d)
                } else if let Some(d) = v.strip_prefix("<=") {
                    ("<", d)
                } else if let Some(d) = v.strip_prefix('>') {
                    (">", d)
                } else if let Some(d) = v.strip_prefix('<') {
                    ("<", d)
                } else {
                    ("=", v.as_str())
                };
                let d = d.to_string();
                if chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").is_err() {
                    return Err(ApiError::validation("AuditLog", "phrase", "invalid"));
                }
                let sql = match op {
                    "=" => "created_at::date = $N::date".to_string(),
                    "<" if v.starts_with("<=") => "created_at < $N::date + 1".to_string(),
                    op => format!("created_at {op} $N::date"),
                };
                add(&sql, d, &mut binds);
            }
            _ => {}
        }
    }
    // Cursor pagination: after/before are document ids ("<source>-<id>") of the last row seen.
    let asc = q.order.as_deref() == Some("asc");
    let union = "SELECT 'e-' || id AS doc, id AS seq, 0 AS src, action, actor_id, actor, user_login, org_id, org, repo, NULL::text AS team, data, created_at FROM ent_audit_log
                 UNION ALL
                 SELECT 'o-' || a.id, a.id, 1, a.action, a.actor_id, a.actor, a.user_login, a.org_id, u.login, a.repo, a.team, a.data, a.created_at
                   FROM org_audit_log a LEFT JOIN users u ON u.id = a.org_id";
    for (cursor, after) in [(q.after.as_deref(), true), (q.before.as_deref(), false)] {
        let Some(c) = cursor else { continue };
        let ts: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT created_at FROM ({union}) x WHERE doc = $1"))).bind(c).fetch_optional(&s.db).await?;
        let Some(ts) = ts else { continue };
        let forward = after != asc; // "after" in the requested order
        let op = if forward { "<" } else { ">" };
        add(&format!("(created_at, doc) {op} ($N::timestamptz, '{}')", c.replace('\'', "")), ts.to_rfc3339(), &mut binds);
    }
    let where_ = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
    let dir = if asc { "ASC" } else { "DESC" };
    let n = binds.len();
    let sql = format!("SELECT doc, action, actor_id, actor, user_login, org_id, org, repo, team, data, created_at FROM ({union}) x {where_}
                       ORDER BY created_at {dir}, doc {dir} LIMIT ${} OFFSET ${}", n + 1, n + 2);
    let mut qq = sqlx::query_as::<_, AuditRow>(sqlx::AssertSqlSafe(sql));
    for b in &binds {
        qq = qq.bind(b);
    }
    let rows = qq.bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let slug = ent::slug();
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut o = Map::new();
            o.insert("@timestamp".into(), json!(ms(r.created_at)));
            o.insert("_document_id".into(), json!(r.doc));
            o.insert("action".into(), json!(r.action));
            o.insert("created_at".into(), json!(ms(r.created_at)));
            o.insert("business".into(), json!(slug));
            o.insert("business_id".into(), json!(1));
            if let Some(a) = &r.actor {
                o.insert("actor".into(), json!(a));
            }
            if let Some(a) = r.actor_id {
                o.insert("actor_id".into(), json!(a));
            }
            if let Some(u) = &r.user_login {
                o.insert("user".into(), json!(u));
            }
            if let Some(x) = &r.org {
                o.insert("org".into(), json!(x));
            }
            if let Some(x) = r.org_id {
                o.insert("org_id".into(), json!(x));
            }
            if let Some(x) = &r.repo {
                o.insert("repo".into(), json!(x));
                o.insert("repository".into(), json!(x));
            }
            if let Some(x) = &r.team {
                o.insert("team".into(), json!(x));
            }
            if let Some(ip) = r.data.get("actor_ip").and_then(Value::as_str) {
                o.insert("actor_ip".into(), json!(ip));
            }
            o.insert("data".into(), if r.data.is_object() { r.data.clone() } else { json!({}) });
            Value::Object(o)
        })
        .collect();
    Ok(Paged::new(&uri, &s, p, None, items).into_response())
}

// Streams ------------------------------------------------------------------------------------

/// The X25519 key vendors' secrets are sealed with (libsodium sealed box), generated once.
async fn stream_keypair(s: &AppState) -> ApiResult<(String, [u8; 32], [u8; 32])> {
    let b64 = base64::engine::general_purpose::STANDARD;
    if let Some(v) = ent::get_setting(&s.db, "audit_stream_key").await? {
        let get = |k: &str| -> Option<[u8; 32]> { b64.decode(v.get(k)?.as_str()?).ok()?.try_into().ok() };
        if let (Some(id), Some(sk), Some(pk)) = (v.get("key_id").and_then(Value::as_str), get("private"), get("public")) {
            return Ok((id.to_string(), sk, pk));
        }
    }
    let (sk, pk) = crate::orgs::sealed_box::keypair();
    let id = format!("{}", rand::random::<u32>() as u64 * 1000 + 1);
    let v = json!({"key_id": id, "private": b64.encode(sk), "public": b64.encode(pk)});
    sqlx::query("INSERT INTO ent_settings (key, value) VALUES ('audit_stream_key', $1) ON CONFLICT (key) DO NOTHING").bind(&v).execute(&s.db).await?;
    Box::pin(stream_keypair(s)).await
}

async fn stream_key(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    let (id, _, pk) = stream_keypair(&s).await?;
    Ok(Json(json!({ "key_id": id, "key": base64::engine::general_purpose::STANDARD.encode(pk) })))
}

#[derive(sqlx::FromRow)]
struct StreamRow {
    id: i64,
    stream_type: String,
    vendor_specific: Value,
    enabled: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    paused_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn stream_json(r: &StreamRow) -> Value {
    let v = &r.vendor_specific;
    let details = ["container", "name", "bucket", "domain", "site"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    json!({
        "id": r.id,
        "stream_type": r.stream_type,
        "stream_details": details,
        "enabled": r.enabled,
        "created_at": crate::json::ts(r.created_at),
        "updated_at": crate::json::ts(r.updated_at),
        "paused_at": r.paused_at.map(crate::json::ts),
    })
}

#[derive(Deserialize)]
struct StreamBody {
    enabled: Option<bool>,
    stream_type: Option<String>,
    vendor_specific: Option<Value>,
}

const STREAM_TYPES: &[&str] = &["Azure Blob Storage", "Azure Event Hubs", "Amazon S3", "Splunk", "HTTPS Event Collector", "Google Cloud Storage", "Datadog"];

/// Validates the body; every `encrypted_*` field must open with the stream key.
async fn check_stream(s: &AppState, b: &StreamBody) -> ApiResult<(bool, String, Value)> {
    let enabled = b.enabled.ok_or_else(|| ApiError::validation("AuditLogStream", "enabled", "missing_field"))?;
    let ty = b.stream_type.clone().ok_or_else(|| ApiError::validation("AuditLogStream", "stream_type", "missing_field"))?;
    if !STREAM_TYPES.contains(&ty.as_str()) {
        return Err(ApiError::validation("AuditLogStream", "stream_type", "invalid"));
    }
    let vs = b.vendor_specific.clone().filter(Value::is_object).ok_or_else(|| ApiError::validation("AuditLogStream", "vendor_specific", "missing_field"))?;
    let (key_id, sk, pk) = stream_keypair(s).await?;
    if vs.get("key_id").and_then(Value::as_str) != Some(key_id.as_str()) {
        return Err(ApiError::validation("AuditLogStream", "key_id", "invalid"));
    }
    for (k, v) in vs.as_object().unwrap() {
        if k.starts_with("encrypted_") {
            let raw = v.as_str().and_then(|x| base64::engine::general_purpose::STANDARD.decode(x).ok());
            if raw.and_then(|r| crate::orgs::sealed_box::open(&r, &sk, &pk)).is_none() {
                return Err(ApiError::validation("AuditLogStream", k, "invalid"));
            }
        }
    }
    Ok((enabled, ty, vs))
}

async fn list_streams(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    let rows: Vec<StreamRow> = sqlx::query_as("SELECT * FROM ent_audit_streams ORDER BY id").fetch_all(&s.db).await?;
    Ok(Json(json!(rows.iter().map(stream_json).collect::<Vec<_>>())))
}

async fn create_stream(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<StreamBody>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    let (enabled, ty, vs) = check_stream(&s, &b).await?;
    let r: StreamRow = sqlx::query_as(
        "INSERT INTO ent_audit_streams (stream_type, vendor_specific, enabled, paused_at) VALUES ($1, $2, $3, CASE WHEN $3 THEN NULL ELSE now() END) RETURNING *",
    )
    .bind(&ty)
    .bind(&vs)
    .bind(enabled)
    .fetch_one(&s.db)
    .await?;
    ent::audit(&s, "audit_log_streaming.create", Some(&u), json!({"stream_id": r.id, "stream_type": ty})).await;
    Ok(Json(stream_json(&r)))
}

async fn load_stream(s: &AppState, e: &str, id: i64) -> ApiResult<StreamRow> {
    ent::check_enterprise(e)?;
    sqlx::query_as("SELECT * FROM ent_audit_streams WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

async fn get_stream(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    Ok(Json(stream_json(&load_stream(&s, &e, id).await?)))
}

async fn update_stream(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, id)): Path<(String, i64)>, Json(b): Json<StreamBody>) -> ApiResult<Json<Value>> {
    let r = load_stream(&s, &e, id).await?;
    let (enabled, ty, vs) = check_stream(&s, &b).await?;
    let r: StreamRow = sqlx::query_as(
        "UPDATE ent_audit_streams SET stream_type = $2, vendor_specific = $3, enabled = $4, updated_at = now(),
            paused_at = CASE WHEN $4 THEN NULL ELSE COALESCE(paused_at, now()) END WHERE id = $1 RETURNING *",
    )
    .bind(r.id)
    .bind(&ty)
    .bind(&vs)
    .bind(enabled)
    .fetch_one(&s.db)
    .await?;
    ent::audit(&s, "audit_log_streaming.update", Some(&u), json!({"stream_id": r.id})).await;
    Ok(Json(stream_json(&r)))
}

async fn delete_stream(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let r = load_stream(&s, &e, id).await?;
    sqlx::query("DELETE FROM ent_audit_streams WHERE id = $1").bind(r.id).execute(&s.db).await?;
    ent::audit(&s, "audit_log_streaming.destroy", Some(&u), json!({"stream_id": r.id})).await;
    Ok(no_content())
}

/// Push-ruleset bypass requests of every repository (`crate::rules::bypass`).
async fn bypass_requests(
    State(s): State<AppState>,
    SiteAdmin(_): SiteAdmin,
    Path(e): Path<String>,
    Query(q): Query<crate::rules::bypass::ListFilter>,
) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(Value::Array(crate::rules::bypass::list(&s, None, None, &q).await?)))
}

// ---------------------------------------------------------------------------------------------
// Code security defaults and enablement

fn code_security_defaults() -> Value {
    json!({
        "advanced_security_enabled_for_new_repositories": false,
        "advanced_security_enabled_for_new_user_namespace_repositories": false,
        "dependabot_alerts_enabled_for_new_repositories": false,
        "secret_scanning_enabled_for_new_repositories": false,
        "secret_scanning_push_protection_enabled_for_new_repositories": false,
        "secret_scanning_push_protection_custom_link": null,
        "secret_scanning_non_provider_patterns_enabled_for_new_repositories": false,
    })
}

/// Enterprise defaults for new repositories (other areas may apply them on repo creation).
pub async fn code_security_settings(s: &AppState) -> ApiResult<Value> {
    ent::setting_with_defaults(&s.db, "code_security_and_analysis", code_security_defaults()).await
}

async fn get_code_security(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(code_security_settings(&s).await?))
}

async fn patch_code_security(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<Value>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let mut v = code_security_settings(&s).await?;
    let Some(o) = b.as_object() else { return Err(ApiError::unprocessable("Invalid request.")) };
    for (k, x) in o {
        let key = if k == "advanced_security_enabled_new_user_namespace_repos" { "advanced_security_enabled_for_new_user_namespace_repositories" } else { k };
        match (key, x) {
            ("secret_scanning_push_protection_custom_link", Value::String(l)) => {
                if url::Url::parse(l).is_err() {
                    return Err(ApiError::validation("SecurityAnalysis", key, "invalid"));
                }
                v[key] = x.clone();
            }
            ("secret_scanning_push_protection_custom_link", Value::Null) => v[key] = Value::Null,
            ("secret_scanning_non_provider_patterns_enabled_for_new_repositories", Value::Null) => {}
            (k2, Value::Bool(_)) if v.get(k2).is_some() => v[k2] = x.clone(),
            _ => return Err(ApiError::validation("SecurityAnalysis", key, "invalid")),
        }
    }
    ent::set_setting(&s.db, "code_security_and_analysis", &v).await?;
    ent::audit(&s, "business.security_center_settings_update", Some(&u), v.clone()).await;
    Ok(no_content())
}

const PRODUCTS: &[&str] = &["advanced_security", "dependabot_alerts", "secret_scanning", "secret_scanning_push_protection"];

/// Enables/disables a security product on every repository (stored per repository in the
/// orgs area's `repo_security_products`).
async fn product_enablement(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, product, enablement)): Path<(String, String, String)>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    if !PRODUCTS.contains(&product.as_str()) {
        return Err(ApiError::not_found());
    }
    let enabled = match enablement.as_str() {
        "enable_all" => true,
        "disable_all" => false,
        _ => return Err(ApiError::unprocessable("enablement must be enable_all or disable_all")),
    };
    sqlx::query(
        "INSERT INTO repo_security_products (repo_id, product, enabled) SELECT id, $1, $2 FROM repositories
         ON CONFLICT (repo_id, product) DO UPDATE SET enabled = $2, updated_at = now()",
    )
    .bind(&product)
    .bind(enabled)
    .execute(&s.db)
    .await?;
    ent::audit(&s, &format!("business.{enablement}_{product}"), Some(&u), json!({})).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Enterprise custom properties

#[derive(Debug, Clone, sqlx::FromRow)]
struct PropRow {
    kind: String,
    property_name: String,
    value_type: String,
    required: bool,
    default_value: Option<Value>,
    description: Option<String>,
    allowed_values: Option<Vec<String>>,
    values_editable_by: Option<String>,
    require_explicit_values: bool,
    #[allow(dead_code)]
    created_at: chrono::DateTime<chrono::Utc>,
}

fn prop_json(s: &AppState, p: &PropRow) -> Value {
    let seg = if p.kind == "org" { "org-properties" } else { "properties" };
    let mut v = json!({
        "property_name": p.property_name,
        "url": format!("{}/enterprises/{}/{seg}/schema/{}", s.api_url(), ent::slug(), p.property_name),
        "source_type": "enterprise",
        "value_type": p.value_type,
        "required": p.required,
        "default_value": p.default_value,
        "description": p.description,
        "allowed_values": p.allowed_values,
        "values_editable_by": p.values_editable_by,
    });
    if p.kind == "repo" {
        v["require_explicit_values"] = json!(p.require_explicit_values);
    }
    v
}

#[derive(Deserialize, Clone, Default)]
struct PropInput {
    property_name: Option<String>,
    value_type: Option<String>,
    required: Option<bool>,
    default_value: Option<Value>,
    description: Option<String>,
    allowed_values: Option<Vec<String>>,
    values_editable_by: Option<String>,
    require_explicit_values: Option<bool>,
}

fn prop_bad(field: &str) -> ApiError {
    ApiError::validation("CustomProperty", field, "invalid")
}

fn validate_prop(kind: &str, name: &str, i: &PropInput) -> ApiResult<PropRow> {
    if name.is_empty() || name.len() > 75 || !name.chars().all(|c| c.is_ascii_alphanumeric() || "_-.$".contains(c)) {
        return Err(prop_bad("property_name"));
    }
    let vt = i.value_type.clone().ok_or_else(|| ApiError::validation("CustomProperty", "value_type", "missing_field"))?;
    if !["string", "single_select", "multi_select", "true_false", "url"].contains(&vt.as_str()) {
        return Err(prop_bad("value_type"));
    }
    let select = vt.ends_with("_select");
    let allowed = i.allowed_values.clone().filter(|a| !a.is_empty());
    if select != allowed.is_some() {
        return Err(prop_bad("allowed_values"));
    }
    let editable_ok: &[&str] = if kind == "org" { &["enterprise_actors", "enterprise_and_org_actors"] } else { &["org_actors", "org_and_repo_actors"] };
    let editable = i.values_editable_by.clone().unwrap_or_else(|| editable_ok[0].to_string());
    if !editable_ok.contains(&editable.as_str()) {
        return Err(prop_bad("values_editable_by"));
    }
    let default = i.default_value.clone().filter(|v| !v.is_null());
    let required = i.required.unwrap_or(false);
    if required && default.is_none() {
        return Err(prop_bad("default_value"));
    }
    if let Some(d) = &default {
        let ok = match (vt.as_str(), d) {
            ("multi_select", Value::Array(xs)) => xs.iter().all(|x| x.as_str().is_some_and(|x| allowed.iter().flatten().any(|a| a == x))),
            ("single_select", Value::String(x)) => allowed.iter().flatten().any(|a| a == x),
            ("true_false", Value::String(x)) => x == "true" || x == "false",
            ("string" | "url", Value::String(_)) => true,
            _ => false,
        };
        if !ok {
            return Err(prop_bad("default_value"));
        }
    }
    Ok(PropRow {
        kind: kind.into(),
        property_name: name.into(),
        value_type: vt,
        required,
        default_value: default,
        description: i.description.clone(),
        allowed_values: allowed,
        values_editable_by: Some(editable),
        require_explicit_values: i.require_explicit_values.unwrap_or(false),
        created_at: chrono::Utc::now(),
    })
}

async fn upsert_prop(db: &mut sqlx::PgConnection, p: &PropRow) -> ApiResult<PropRow> {
    Ok(sqlx::query_as(
        "INSERT INTO ent_custom_properties (kind, property_name, value_type, required, default_value, description, allowed_values,
            values_editable_by, require_explicit_values) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (kind, property_name) DO UPDATE SET value_type = $3, required = $4, default_value = $5, description = $6,
            allowed_values = $7, values_editable_by = $8, require_explicit_values = $9 RETURNING *",
    )
    .bind(&p.kind)
    .bind(&p.property_name)
    .bind(&p.value_type)
    .bind(p.required)
    .bind(&p.default_value)
    .bind(&p.description)
    .bind(&p.allowed_values)
    .bind(&p.values_editable_by)
    .bind(p.require_explicit_values)
    .fetch_one(db)
    .await?)
}

async fn list_props(s: &AppState, kind: &str) -> ApiResult<Json<Value>> {
    let rows: Vec<PropRow> = sqlx::query_as("SELECT * FROM ent_custom_properties WHERE kind = $1 ORDER BY property_name").bind(kind).fetch_all(&s.db).await?;
    Ok(Json(json!(rows.iter().map(|p| prop_json(s, p)).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct PropsBody {
    properties: Option<Vec<PropInput>>,
}

async fn upsert_props(s: &AppState, actor: &User, kind: &str, b: PropsBody) -> ApiResult<Json<Value>> {
    let inputs = b.properties.filter(|p| !p.is_empty() && p.len() <= 100).ok_or_else(|| ApiError::validation("CustomProperty", "properties", "missing_field"))?;
    let mut rows = Vec::new();
    for i in &inputs {
        let name = i.property_name.clone().ok_or_else(|| ApiError::validation("CustomProperty", "property_name", "missing_field"))?;
        rows.push(validate_prop(kind, &name, i)?);
    }
    let mut tx = s.db.begin().await?;
    let mut out = Vec::new();
    for r in &rows {
        out.push(upsert_prop(&mut tx, r).await?);
    }
    tx.commit().await?;
    ent::audit(s, "business.custom_property_update", Some(actor), json!({"kind": kind, "properties": rows.iter().map(|r| &r.property_name).collect::<Vec<_>>()})).await;
    Ok(Json(json!(out.iter().map(|p| prop_json(s, p)).collect::<Vec<_>>())))
}

async fn load_prop(s: &AppState, kind: &str, name: &str) -> ApiResult<PropRow> {
    sqlx::query_as("SELECT * FROM ent_custom_properties WHERE kind = $1 AND property_name = $2")
        .bind(kind)
        .bind(name)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn put_prop(s: &AppState, actor: &User, kind: &str, name: &str, i: PropInput) -> ApiResult<Json<Value>> {
    let r = validate_prop(kind, name, &i)?;
    let mut conn = s.db.acquire().await?;
    let r = upsert_prop(&mut conn, &r).await?;
    ent::audit(s, "business.custom_property_update", Some(actor), json!({"kind": kind, "property": name})).await;
    Ok(Json(prop_json(s, &r)))
}

async fn delete_prop(s: &AppState, actor: &User, kind: &str, name: &str) -> ApiResult<Response> {
    let p = load_prop(s, kind, name).await?;
    sqlx::query("DELETE FROM ent_custom_properties WHERE kind = $1 AND property_name = $2").bind(kind).bind(name).execute(&s.db).await?;
    if kind == "org" {
        sqlx::query("DELETE FROM org_custom_property_values WHERE property_name = $1").bind(&p.property_name).execute(&s.db).await?;
    }
    ent::audit(s, "business.custom_property_delete", Some(actor), json!({"kind": kind, "property": name})).await;
    Ok(no_content())
}

async fn list_org_props(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    list_props(&s, "org").await
}
async fn upsert_org_props(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<PropsBody>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    upsert_props(&s, &u, "org", b).await
}
async fn get_org_prop(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(prop_json(&s, &load_prop(&s, "org", &name).await?)))
}
async fn put_org_prop(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, name)): Path<(String, String)>, Json(b): Json<PropInput>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    put_prop(&s, &u, "org", &name, b).await
}
async fn delete_org_prop(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, name)): Path<(String, String)>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    delete_prop(&s, &u, "org", &name).await
}
async fn list_repo_props(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    list_props(&s, "repo").await
}
async fn upsert_repo_props(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<PropsBody>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    upsert_props(&s, &u, "repo", b).await
}
async fn get_repo_prop(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(prop_json(&s, &load_prop(&s, "repo", &name).await?)))
}
async fn put_repo_prop(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, name)): Path<(String, String)>, Json(b): Json<PropInput>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    put_prop(&s, &u, "repo", &name, b).await
}
async fn delete_repo_prop(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, name)): Path<(String, String)>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    delete_prop(&s, &u, "repo", &name).await
}

/// Promotes an organization's repository property definition to the enterprise: the
/// definition moves to `ent_custom_properties` (repositories keep their values).
async fn promote_prop(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((e, org, name)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    let o = crate::orgs::org_by_login(&s, &org).await?;
    let defs = super::orgs_props::definitions(&s.db, o.id).await?;
    let d = defs.into_iter().find(|d| d.property_name == name).ok_or_else(ApiError::not_found)?;
    let row = PropRow {
        kind: "repo".into(),
        property_name: d.property_name.clone(),
        value_type: d.value_type,
        required: d.required,
        default_value: d.default_value,
        description: d.description,
        allowed_values: d.allowed_values,
        values_editable_by: Some(d.values_editable_by),
        require_explicit_values: d.require_explicit_values,
        created_at: chrono::Utc::now(),
    };
    let mut tx = s.db.begin().await?;
    let r = upsert_prop(&mut tx, &row).await?;
    sqlx::query("DELETE FROM custom_property_definitions WHERE org_id = $1 AND property_name = $2").bind(o.id).bind(&name).execute(&mut *tx).await?;
    tx.commit().await?;
    crate::orgs::audit_quiet(&s.db, &o, Some(&u), "custom_property.promote", json!({"property": name})).await;
    Ok(Json(prop_json(&s, &r)))
}

#[derive(Deserialize)]
struct OrgValuesBody {
    organization_logins: Option<Vec<String>>,
    properties: Option<Vec<Value>>,
}

async fn list_org_values(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, uri: OriginalUri, Path(e): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    ent::check_enterprise(&e)?;
    let orgs: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE kind = 'Organization' ORDER BY id LIMIT $1 OFFSET $2")
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'Organization'").fetch_one(&s.db).await?;
    let ids: Vec<i64> = orgs.iter().map(|o| o.id).collect();
    let vals: Vec<(i64, String, Value)> =
        sqlx::query_as("SELECT org_id, property_name, value FROM org_custom_property_values WHERE org_id = ANY($1) ORDER BY property_name")
            .bind(&ids)
            .fetch_all(&s.db)
            .await?;
    let items = orgs
        .iter()
        .map(|o| {
            json!({
                "organization_id": o.id,
                "organization_login": o.login,
                "properties": vals.iter().filter(|v| v.0 == o.id).map(|v| json!({"property_name": v.1, "value": v.2})).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn set_org_values(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path(e): Path<String>, Json(b): Json<OrgValuesBody>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let logins = b.organization_logins.filter(|l| !l.is_empty()).ok_or_else(|| ApiError::validation("CustomProperty", "organization_logins", "missing_field"))?;
    let props = b.properties.ok_or_else(|| ApiError::validation("CustomProperty", "properties", "missing_field"))?;
    let defs: Vec<PropRow> = sqlx::query_as("SELECT * FROM ent_custom_properties WHERE kind = 'org'").fetch_all(&s.db).await?;
    let mut parsed = Vec::new();
    for p in props {
        let name = p.get("property_name").and_then(Value::as_str).ok_or_else(|| ApiError::validation("CustomProperty", "property_name", "missing_field"))?;
        let d = defs.iter().find(|d| d.property_name == name).ok_or_else(|| prop_bad("property_name"))?;
        let v = p.get("value").cloned().unwrap_or(Value::Null);
        let allowed = d.allowed_values.clone().unwrap_or_default();
        let ok = match (d.value_type.as_str(), &v) {
            (_, Value::Null) => !d.required,
            ("string", Value::String(x)) => x.chars().count() <= 75,
            ("url", Value::String(x)) => url::Url::parse(x).is_ok(),
            ("true_false", Value::String(x)) => x == "true" || x == "false",
            ("single_select", Value::String(x)) => allowed.contains(x),
            ("multi_select", Value::Array(xs)) => xs.iter().all(|x| x.as_str().is_some_and(|x| allowed.iter().any(|a| a == x))),
            _ => false,
        };
        if !ok {
            return Err(prop_bad("value"));
        }
        parsed.push((name.to_string(), v));
    }
    let mut orgs = Vec::new();
    for l in &logins {
        orgs.push(crate::orgs::org_by_login(&s, l).await.map_err(|_| prop_bad("organization_logins"))?);
    }
    let mut tx = s.db.begin().await?;
    for o in &orgs {
        for (n, v) in &parsed {
            if v.is_null() {
                sqlx::query("DELETE FROM org_custom_property_values WHERE org_id = $1 AND property_name = $2").bind(o.id).bind(n).execute(&mut *tx).await?;
            } else {
                sqlx::query(
                    "INSERT INTO org_custom_property_values (org_id, property_name, value) VALUES ($1, $2, $3)
                     ON CONFLICT (org_id, property_name) DO UPDATE SET value = $3",
                )
                .bind(o.id)
                .bind(n)
                .bind(v)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    for o in &orgs {
        crate::orgs::audit_quiet(&s.db, o, Some(&u), "org.custom_property_values_update", json!({})).await;
    }
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Ruleset history of enterprise rulesets (`/enterprises/{e}/rulesets`, see `api::rulesets`).

async fn ruleset_history(
    State(s): State<AppState>,
    SiteAdmin(_): SiteAdmin,
    uri: OriginalUri,
    Path((e, id)): Path<(String, i64)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    ent::check_enterprise(&e)?;
    super::repos_rules::versions_page(&s, &uri, &super::rulesets::Level::Enterprise, id, p).await
}

async fn ruleset_version(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id, v)): Path<(String, i64, i64)>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(super::repos_rules::one_version(&s, &super::rulesets::Level::Enterprise, id, v).await?))
}

// ---------------------------------------------------------------------------------------------
// Advanced Security billing: committers = distinct users who pushed to each active
// (non-archived) repository in the last 90 days (from recorded PushEvents).

#[derive(Deserialize)]
struct BillingQuery {
    advanced_security_product: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct CommitterRow {
    repo: String,
    login: String,
    email: Option<String>,
    last_push: chrono::DateTime<chrono::Utc>,
}

async fn billing(s: &AppState, owner: Option<i64>, q: BillingQuery) -> ApiResult<Value> {
    if let Some(p) = &q.advanced_security_product {
        if !matches!(p.as_str(), "code_security" | "secret_protection") {
            return Err(ApiError::validation("Billing", "advanced_security_product", "invalid"));
        }
    }
    let p = PageParams { page: q.page, per_page: q.per_page };
    let rows: Vec<CommitterRow> = sqlx::query_as(
        "SELECT o.login || '/' || r.name AS repo, u.login, u.email, max(e.created_at) AS last_push
           FROM activity_events e JOIN repositories r ON r.id = e.repo_id JOIN users o ON o.id = r.owner_id JOIN users u ON u.id = e.actor_id
          WHERE e.type = 'PushEvent' AND e.created_at > now() - interval '90 days' AND NOT r.archived
            AND ($1::bigint IS NULL OR r.owner_id = $1)
          GROUP BY 1, u.login, u.email ORDER BY 1, u.login",
    )
    .bind(owner)
    .fetch_all(&s.db)
    .await?;
    let mut repos: Vec<(String, Vec<&CommitterRow>)> = Vec::new();
    for r in &rows {
        match repos.last_mut() {
            Some((n, v)) if *n == r.repo => v.push(r),
            _ => repos.push((r.repo.clone(), vec![r])),
        }
    }
    let total_count = repos.len();
    let total = super::ent_admin::active_committers(s, owner).await?;
    let page: Vec<Value> = repos
        .iter()
        .skip(p.offset() as usize)
        .take(p.limit() as usize)
        .map(|(name, cs)| {
            json!({
                "name": name,
                "advanced_security_committers": cs.len(),
                "advanced_security_committers_breakdown": cs.iter().map(|c| json!({
                    "user_login": c.login,
                    "last_pushed_date": c.last_push.format("%Y-%m-%d").to_string(),
                    "last_pushed_email": c.email.clone().unwrap_or_default(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "total_advanced_security_committers": total,
        "total_count": total_count,
        "maximum_advanced_security_committers": total,
        "purchased_advanced_security_committers": 0,
        "repositories": page,
    }))
}

async fn billing_enterprise(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Query(q): Query<BillingQuery>) -> ApiResult<Json<Value>> {
    ent::check_enterprise(&e)?;
    Ok(Json(billing(&s, None, q).await?))
}

async fn billing_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<BillingQuery>) -> ApiResult<Json<Value>> {
    let o = crate::orgs::org_by_login(&s, &org).await?;
    crate::orgs::require_owner(&s, &o, &u).await?;
    Ok(Json(billing(&s, Some(o.id), q).await?))
}

// ---------------------------------------------------------------------------------------------
// Live migrations: records with GHES's state machine. forge does not transfer data to the
// target (that would need credentials for, and traffic to, another GitHub instance); starting
// a migration fails it with a terminal failure that says so.

#[derive(sqlx::FromRow, Clone)]
struct MigrationRow {
    id: String,
    source_organization_login: String,
    source_repository_name: String,
    target_organization_login: String,
    target_repository_name: String,
    target_api_endpoint: String,
    #[allow(dead_code)]
    pat_name: String,
    target_visibility: Option<String>,
    status: String,
    messages: Value,
    #[allow(dead_code)]
    created_by: Option<i64>,
    created_at: chrono::DateTime<chrono::Utc>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn migration_json(s: &AppState, m: &MigrationRow) -> Value {
    let target_base = m.target_api_endpoint.trim_end_matches('/').trim_end_matches("/api/v3").to_string();
    json!({
        "migration_id": m.id,
        "status": m.status,
        "source_organization_login": m.source_organization_login,
        "target_organization_login": m.target_organization_login,
        "source_repository_name": m.source_repository_name,
        "target_repository_name": m.target_repository_name,
        "target_visibility": m.target_visibility,
        "target_api_endpoint": m.target_api_endpoint,
        "target_migration_id": 0,
        "created_at": crate::json::ts(m.created_at),
        "started_at": m.started_at.map(crate::json::ts),
        "completed_at": m.completed_at.map(crate::json::ts),
        "expires_at": m.expires_at.map(crate::json::ts),
        "repositories": [{
            "source_repository_url": format!("{}/{}/{}", s.base_url(), m.source_organization_login, m.source_repository_name),
            "target_repository_url": format!("{target_base}/{}/{}", m.target_organization_login, m.target_repository_name),
            "status": m.status,
        }],
    })
}

#[derive(Deserialize)]
struct MigrationsQuery {
    page_size: Option<i64>,
    after: Option<String>,
    status: Option<String>,
}

async fn list_migrations(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Query(q): Query<MigrationsQuery>) -> ApiResult<Json<Value>> {
    let size = q.page_size.unwrap_or(30).clamp(1, 100);
    let rows: Vec<MigrationRow> = sqlx::query_as(
        "SELECT * FROM ent_live_migrations WHERE ($1::text IS NULL OR status = $1)
           AND ($2::text IS NULL OR (created_at, id) < (SELECT created_at, id FROM ent_live_migrations WHERE id = $2))
         ORDER BY created_at DESC, id DESC LIMIT $3",
    )
    .bind(&q.status)
    .bind(&q.after)
    .bind(size + 1)
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ent_live_migrations WHERE ($1::text IS NULL OR status = $1)").bind(&q.status).fetch_one(&s.db).await?;
    let more = rows.len() as i64 > size;
    let rows = &rows[..rows.len().min(size as usize)];
    let mut v = json!({ "migrations": rows.iter().map(|m| migration_json(&s, m)).collect::<Vec<_>>(), "total_count": total });
    if more {
        v["next_cursor"] = json!(rows.last().map(|m| m.id.clone()));
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct CreateMigration {
    source_organization_login: Option<String>,
    source_repository_name: Option<String>,
    target_organization_login: Option<String>,
    target_repository_name: Option<String>,
    target_api_endpoint: Option<String>,
    pat_name: Option<String>,
    target_visibility: Option<String>,
}

async fn create_migration(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Json(b): Json<CreateMigration>) -> ApiResult<Response> {
    let req = |v: Option<String>, f: &str| v.filter(|x| !x.trim().is_empty()).ok_or_else(|| ApiError::validation("LiveMigration", f, "missing_field"));
    let so = req(b.source_organization_login, "source_organization_login")?;
    let sr = req(b.source_repository_name, "source_repository_name")?;
    let to = req(b.target_organization_login, "target_organization_login")?;
    let tr = req(b.target_repository_name, "target_repository_name")?;
    let ep = req(b.target_api_endpoint, "target_api_endpoint")?;
    let pat = req(b.pat_name, "pat_name")?;
    if url::Url::parse(&ep).ok().filter(|u| u.scheme() == "https").is_none() {
        return Err(ApiError::validation("LiveMigration", "target_api_endpoint", "invalid"));
    }
    if let Some(v) = &b.target_visibility {
        if !matches!(v.as_str(), "private" | "internal") {
            return Err(ApiError::validation("LiveMigration", "target_visibility", "invalid"));
        }
    }
    // The source repository must exist on this instance.
    crate::db::load_repo(&s.db, Some(&u), &so, &sr).await?;
    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ent_live_migrations WHERE lower(source_organization_login) = lower($1)
            AND lower(source_repository_name) = lower($2) AND status IN ('created', 'in_progress', 'paused'))",
    )
    .bind(&so)
    .bind(&sr)
    .fetch_one(&s.db)
    .await?;
    if busy {
        return Err(ApiError::conflict("A live migration for this repository is already active."));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let m: MigrationRow = sqlx::query_as(
        "INSERT INTO ent_live_migrations (id, source_organization_login, source_repository_name, target_organization_login,
            target_repository_name, target_api_endpoint, pat_name, target_visibility, created_by, expires_at, messages)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9, now() + interval '7 days', $10) RETURNING *",
    )
    .bind(&id)
    .bind(&so)
    .bind(&sr)
    .bind(&to)
    .bind(&tr)
    .bind(&ep)
    .bind(&pat)
    .bind(&b.target_visibility)
    .bind(u.id)
    .bind(json!([{"message_type": "info", "message": "Migration created", "created_at": crate::json::ts(chrono::Utc::now())}]))
    .fetch_one(&s.db)
    .await?;
    ent::audit(&s, "live_migration.create", Some(&u), json!({"migration_id": id, "repo": format!("{so}/{sr}")})).await;
    Ok((StatusCode::CREATED, axum::Json(json!({ "migration_id": m.id, "expires_at": m.expires_at.map(crate::json::ts) }))).into_response())
}

async fn load_migration(s: &AppState, id: &str) -> ApiResult<MigrationRow> {
    sqlx::query_as("SELECT * FROM ent_live_migrations WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

async fn get_migration(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let m = load_migration(&s, &id).await?;
    let failure = m
        .messages
        .as_array()
        .and_then(|a| a.iter().rev().find(|x| x["message_type"] == "error"))
        .map(|x| json!({"code": "target_not_supported", "summary": x["message"], "occurred_at": x["created_at"]}));
    let nwo = format!("{}/{}", m.source_organization_login, m.source_repository_name);
    Ok(Json(json!({
        "migration": migration_json(&s, &m),
        "target_state": {
            "status": m.status,
            "last_updated": crate::json::ts(m.completed_at.or(m.started_at).unwrap_or(m.created_at)),
            "target_unavailable": true,
            "repository_summaries": [],
            "repository_progress": [],
            "terminal_failure": failure.clone(),
        },
        "combined_state": {
            "status": m.status,
            "display_message": failure.as_ref().and_then(|f| f["summary"].as_str().map(str::to_string)).unwrap_or_else(|| format!("Migration {}", m.status)),
            "ready_for_cutover": false,
            "cutover_blockers": ["forge does not transfer repository data to the target instance"],
            "repositories": [{ "repository_nwo": nwo, "phase": m.status, "display_status": m.status }],
            "terminal_failure": failure,
        },
        "messages": m.messages,
    })))
}

async fn migration_action(State(s): State<AppState>, SiteAdmin(u): SiteAdmin, Path((id, action)): Path<(String, String)>, Json(_body): Json<Value>) -> ApiResult<Response> {
    let m = load_migration(&s, &id).await?;
    let terminal = matches!(m.status.as_str(), "failed" | "cancelled" | "completed");
    let conflict = || ApiError::conflict(format!("The migration is {} and cannot be changed this way.", m.status));
    let now = crate::json::ts(chrono::Utc::now());
    let (status, msg_type, msg): (&str, &str, String) = match action.as_str() {
        "cancel" if !terminal => ("cancelled", "info", "Migration cancelled".into()),
        "pause" if matches!(m.status.as_str(), "in_progress" | "created") => ("paused", "info", "Migration paused".into()),
        "resume" if m.status == "paused" => ("failed", "error", "forge cannot transfer repository data to another instance".into()),
        "start" if m.status == "created" => ("failed", "error", format!("forge cannot transfer repository data to {}; live migrations are recorded only", m.target_api_endpoint)),
        "cutover" | "revert-cutover" if matches!(m.status.as_str(), "created" | "in_progress" | "paused" | "failed") => {
            if action == "revert-cutover" {
                return Ok(Json(json!({
                    "success": true,
                    "unarchived_source_repository": false,
                    "in_progress_cutover_terminated": false,
                    "in_progress_migration_terminated": false,
                }))
                .into_response());
            }
            return Err(ApiError::unprocessable("The migration is not ready for cutover."));
        }
        "cancel" | "pause" | "resume" | "start" | "cutover" | "revert-cutover" => return Err(conflict()),
        _ => return Err(ApiError::not_found()),
    };
    sqlx::query(
        "UPDATE ent_live_migrations SET status = $2, messages = messages || $3::jsonb,
            started_at = CASE WHEN $4 THEN COALESCE(started_at, now()) ELSE started_at END,
            completed_at = CASE WHEN $2 IN ('failed', 'cancelled') THEN now() ELSE completed_at END
          WHERE id = $1",
    )
    .bind(&m.id)
    .bind(status)
    .bind(json!([{"message_type": msg_type, "message": msg, "created_at": now}]))
    .bind(action == "start")
    .execute(&s.db)
    .await?;
    ent::audit(&s, &format!("live_migration.{action}"), Some(&u), json!({"migration_id": m.id})).await;
    Ok(no_content())
}
