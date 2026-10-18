//! Custom properties: org-level definitions for repositories (`/orgs/{org}/properties/schema`),
//! their values on the org's repositories (`/orgs/{org}/properties/values`), and values on the
//! organization itself (`/organizations/{org}/org-properties/values`, whose definitions live at
//! enterprise level).
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::orgs::{audit_quiet, org_by_login, require_capability, require_member, require_owner};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/properties/schema", get(list_defs).patch(upsert_defs))
        .route("/orgs/{org}/properties/schema/{name}", get(get_def).put(put_def).delete(delete_def))
        .route("/orgs/{org}/properties/values", get(list_values).patch(set_values))
        .route("/organizations/{org}/org-properties/values", get(get_org_values).patch(set_org_values))
}

const WRITE_DEFS: &[&str] = &["write_organization_custom_properties"];
const WRITE_VALUES: &[&str] = &["edit_org_custom_properties_values", "write_organization_custom_properties"];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PropertyDef {
    pub org_id: i64,
    pub property_name: String,
    pub value_type: String,
    pub required: bool,
    pub default_value: Option<Value>,
    pub description: Option<String>,
    pub allowed_values: Option<Vec<String>>,
    pub values_editable_by: String,
    pub require_explicit_values: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

fn def_json(s: &AppState, o: &User, d: &PropertyDef) -> Value {
    json!({
        "property_name": d.property_name,
        "url": format!("{}/orgs/{}/properties/schema/{}", s.api_url(), o.login, d.property_name),
        "source_type": "organization",
        "value_type": d.value_type,
        "required": d.required,
        "default_value": d.default_value,
        "description": d.description,
        "allowed_values": d.allowed_values,
        "values_editable_by": d.values_editable_by,
        "require_explicit_values": d.require_explicit_values,
    })
}

pub async fn definitions(db: &sqlx::PgPool, org_id: i64) -> ApiResult<Vec<PropertyDef>> {
    Ok(sqlx::query_as("SELECT * FROM custom_property_definitions WHERE org_id = $1 ORDER BY property_name").bind(org_id).fetch_all(db).await?)
}

#[derive(Deserialize, Default, Clone)]
pub struct DefInput {
    pub property_name: Option<String>,
    pub value_type: Option<String>,
    pub required: Option<bool>,
    pub default_value: Option<Value>,
    pub description: Option<String>,
    pub allowed_values: Option<Vec<String>>,
    pub values_editable_by: Option<String>,
    pub require_explicit_values: Option<bool>,
}

fn bad(field: &str, message: &str) -> ApiError {
    let mut e = ApiError::validation("CustomProperty", field, "invalid");
    e.errors = Some(json!([{ "resource": "CustomProperty", "field": field, "code": "invalid", "message": message }]));
    e
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 75 && n.chars().all(|c| c.is_ascii_alphanumeric() || "_-.$".contains(c))
}

/// Checks a value against a definition. `null` means "unset".
pub fn check_value(d: &PropertyDef, v: &Value) -> ApiResult<()> {
    let allowed = d.allowed_values.clone().unwrap_or_default();
    let ok = match (d.value_type.as_str(), v) {
        (_, Value::Null) => true,
        ("string", Value::String(x)) => x.chars().count() <= 75,
        ("url", Value::String(x)) => url::Url::parse(x).is_ok_and(|u| matches!(u.scheme(), "http" | "https")),
        ("true_false", Value::String(x)) => x == "true" || x == "false",
        ("single_select", Value::String(x)) => allowed.contains(x),
        ("multi_select", Value::Array(xs)) => xs.iter().all(|x| x.as_str().is_some_and(|x| allowed.iter().any(|a| a == x))),
        _ => false,
    };
    if ok { Ok(()) } else { Err(bad("value", &format!("Invalid value for property {}", d.property_name))) }
}

fn validate_def(name: &str, req: &DefInput) -> ApiResult<PropertyDef> {
    if !valid_name(name) {
        return Err(bad("property_name", "Property names may only contain letters, digits, _, -, . and $ (max 75)."));
    }
    let vt = req.value_type.clone().ok_or_else(|| ApiError::validation("CustomProperty", "value_type", "missing_field"))?;
    if !["string", "single_select", "multi_select", "true_false", "url"].contains(&vt.as_str()) {
        return Err(bad("value_type", "Invalid value_type"));
    }
    let select = vt.ends_with("_select");
    let allowed = req.allowed_values.clone().filter(|a| !a.is_empty());
    if select && allowed.is_none() {
        return Err(bad("allowed_values", "Select properties need allowed_values."));
    }
    if !select && allowed.is_some() {
        return Err(bad("allowed_values", "allowed_values are only valid for select properties."));
    }
    if allowed.as_ref().is_some_and(|a| a.len() > 200 || a.iter().any(|x| x.is_empty() || x.len() > 75)) {
        return Err(bad("allowed_values", "Too many or invalid allowed values."));
    }
    let editable = req.values_editable_by.clone().unwrap_or_else(|| "org_actors".into());
    if !["org_actors", "org_and_repo_actors"].contains(&editable.as_str()) {
        return Err(bad("values_editable_by", "Invalid values_editable_by"));
    }
    let def = PropertyDef {
        org_id: 0,
        property_name: name.to_string(),
        value_type: vt,
        required: req.required.unwrap_or(false),
        default_value: req.default_value.clone().filter(|v| !v.is_null()),
        description: req.description.clone(),
        allowed_values: allowed,
        values_editable_by: editable,
        require_explicit_values: req.require_explicit_values.unwrap_or(false),
        created_at: chrono::Utc::now(),
    };
    if def.required && def.default_value.is_none() {
        return Err(bad("default_value", "Required properties need a default value."));
    }
    if let Some(dv) = &def.default_value {
        check_value(&def, dv).map_err(|_| bad("default_value", "The default value is not valid for this property."))?;
    }
    Ok(def)
}

async fn upsert_def(tx: &mut sqlx::PgConnection, org: &User, d: &PropertyDef) -> ApiResult<PropertyDef> {
    Ok(sqlx::query_as(
        "INSERT INTO custom_property_definitions (org_id, property_name, value_type, required, default_value, description,
            allowed_values, values_editable_by, require_explicit_values) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (org_id, property_name) DO UPDATE SET value_type = $3, required = $4, default_value = $5, description = $6,
            allowed_values = $7, values_editable_by = $8, require_explicit_values = $9
         RETURNING *",
    )
    .bind(org.id)
    .bind(&d.property_name)
    .bind(&d.value_type)
    .bind(d.required)
    .bind(&d.default_value)
    .bind(&d.description)
    .bind(&d.allowed_values)
    .bind(&d.values_editable_by)
    .bind(d.require_explicit_values)
    .fetch_one(&mut *tx)
    .await?)
}

/// Creates or updates property definitions (owners or holders of
/// `write_organization_custom_properties`). Values no longer valid for a changed definition are
/// dropped.
pub async fn upsert_definitions(s: &AppState, org: &User, actor: &User, inputs: Vec<(String, DefInput)>) -> ApiResult<Vec<PropertyDef>> {
    require_capability(s, org, actor, WRITE_DEFS).await?;
    if inputs.is_empty() || inputs.len() > 100 {
        return Err(bad("properties", "Between 1 and 100 properties are required."));
    }
    let defs: Vec<PropertyDef> = inputs.iter().map(|(n, r)| validate_def(n, r)).collect::<ApiResult<_>>()?;
    let mut tx = s.db.begin().await?;
    let mut out = Vec::new();
    for d in &defs {
        let saved = upsert_def(&mut tx, org, d).await?;
        let vals: Vec<(i64, Value)> = sqlx::query_as(
            "SELECT v.repo_id, v.value FROM repo_custom_property_values v JOIN repositories r ON r.id = v.repo_id
              WHERE r.owner_id = $1 AND v.property_name = $2",
        )
        .bind(org.id)
        .bind(&d.property_name)
        .fetch_all(&mut *tx)
        .await?;
        for (rid, v) in vals {
            if check_value(&saved, &v).is_err() {
                sqlx::query("DELETE FROM repo_custom_property_values WHERE repo_id = $1 AND property_name = $2")
                    .bind(rid)
                    .bind(&d.property_name)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        out.push(saved);
    }
    tx.commit().await?;
    for d in &out {
        audit_quiet(&s.db, org, Some(actor), "custom_property_definition.update", json!({ "property_name": d.property_name })).await;
    }
    Ok(out)
}

async fn list_defs(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_member(&s, &o, &u).await?;
    let defs = definitions(&s.db, o.id).await?;
    Ok(Json(json!(defs.iter().map(|d| def_json(&s, &o, d)).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct DefsBody {
    properties: Option<Vec<DefInput>>,
}

async fn upsert_defs(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<DefsBody>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    let props = b.properties.ok_or_else(|| ApiError::validation("CustomProperty", "properties", "missing_field"))?;
    let mut inputs = Vec::new();
    for p in props {
        let n = p.property_name.clone().ok_or_else(|| ApiError::validation("CustomProperty", "property_name", "missing_field"))?;
        inputs.push((n, p));
    }
    let saved = upsert_definitions(&s, &o, &u, inputs).await?;
    Ok(Json(json!(saved.iter().map(|d| def_json(&s, &o, d)).collect::<Vec<_>>())))
}

async fn load_def(s: &AppState, o: &User, name: &str) -> ApiResult<PropertyDef> {
    sqlx::query_as("SELECT * FROM custom_property_definitions WHERE org_id = $1 AND property_name = $2")
        .bind(o.id)
        .bind(name)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn get_def(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, name)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_member(&s, &o, &u).await?;
    Ok(Json(def_json(&s, &o, &load_def(&s, &o, &name).await?)))
}

async fn put_def(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, name)): Path<(String, String)>, Json(b): Json<DefInput>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    let saved = upsert_definitions(&s, &o, &u, vec![(name, b)]).await?;
    Ok(Json(def_json(&s, &o, &saved[0])))
}

async fn delete_def(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, name)): Path<(String, String)>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_DEFS).await?;
    let mut tx = s.db.begin().await?;
    let n = sqlx::query("DELETE FROM custom_property_definitions WHERE org_id = $1 AND property_name = $2").bind(o.id).bind(&name).execute(&mut *tx).await?;
    if n.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    sqlx::query("DELETE FROM repo_custom_property_values WHERE property_name = $2 AND repo_id IN (SELECT id FROM repositories WHERE owner_id = $1)")
        .bind(o.id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit_quiet(&s.db, &o, Some(&u), "custom_property_definition.destroy", json!({ "property_name": name })).await;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Values on repositories

/// Effective custom property values of a repository: explicit values, plus defaults of
/// properties that have one.
pub async fn repo_property_values(db: &sqlx::PgPool, org_id: i64, repo_id: i64) -> ApiResult<Vec<(String, Value)>> {
    let defs = definitions(db, org_id).await?;
    let vals: Vec<(String, Value)> = sqlx::query_as("SELECT property_name, value FROM repo_custom_property_values WHERE repo_id = $1")
        .bind(repo_id)
        .fetch_all(db)
        .await?;
    Ok(defs
        .iter()
        .filter_map(|d| {
            vals.iter()
                .find(|(n, _)| *n == d.property_name)
                .map(|(_, v)| v.clone())
                .or_else(|| d.default_value.clone())
                .map(|v| (d.property_name.clone(), v))
        })
        .collect())
}

/// Sets (or, with `null`, unsets) property values on repositories of the org.
pub async fn set_repo_property_values(s: &AppState, org: &User, actor: &User, repo_ids: &[(i64, String)], props: &[(String, Value)]) -> ApiResult<()> {
    let defs = definitions(&s.db, org.id).await?;
    for (name, v) in props {
        let d = defs.iter().find(|d| d.property_name == *name).ok_or_else(|| bad("property_name", &format!("Property {name} is not defined.")))?;
        check_value(d, v)?;
        if v.is_null() && d.required {
            return Err(bad("value", &format!("Property {name} is required.")));
        }
    }
    let mut tx = s.db.begin().await?;
    for (rid, _) in repo_ids {
        for (name, v) in props {
            if v.is_null() {
                sqlx::query("DELETE FROM repo_custom_property_values WHERE repo_id = $1 AND property_name = $2").bind(rid).bind(name).execute(&mut *tx).await?;
            } else {
                sqlx::query(
                    "INSERT INTO repo_custom_property_values (repo_id, property_name, value) VALUES ($1,$2,$3)
                     ON CONFLICT (repo_id, property_name) DO UPDATE SET value = $3",
                )
                .bind(rid)
                .bind(name)
                .bind(v)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    for (_, full) in repo_ids {
        audit_quiet(&s.db, org, Some(actor), "repo.custom_property_values_updated", json!({ "repo": full })).await;
    }
    Ok(())
}

#[derive(Deserialize, Default)]
struct ValuesQuery {
    repository_query: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_values(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<ValuesQuery>) -> ApiResult<Paged> {
    let o = org_by_login(&s, &org).await?;
    require_member(&s, &o, &u).await?;
    let p = PageParams { page: q.page, per_page: q.per_page };
    // `repository_query` supports free text on the name and `props.<name>:<value>`
    let mut name_terms = Vec::new();
    let mut prop_terms = Vec::new();
    for t in q.repository_query.as_deref().unwrap_or("").split_whitespace() {
        match t.strip_prefix("props.").and_then(|r| r.split_once(':')) {
            Some((n, v)) => prop_terms.push((n.to_string(), v.to_string())),
            None if t.contains(':') => {}
            None => name_terms.push(t.to_lowercase()),
        }
    }
    let repos: Vec<db::Repo> = sqlx::query_as("SELECT * FROM repositories WHERE owner_id = $1 ORDER BY id").bind(o.id).fetch_all(&s.db).await?;
    let ids: Vec<i64> = repos.iter().map(|r| r.id).collect();
    let perms = crate::orgs::perm::org_repo_permissions_for_repos(&s.db, o.id, &ids, &u).await?;
    let mut items = Vec::new();
    for r in repos {
        if perms.get(&r.id).is_none_or(|p| *p < Perm::Read) {
            continue;
        }
        if !name_terms.iter().all(|t| r.name.to_lowercase().contains(t)) {
            continue;
        }
        let vals = repo_property_values(&s.db, o.id, r.id).await?;
        let matches = prop_terms.iter().all(|(n, v)| {
            vals.iter().any(|(pn, pv)| pn == n && (pv.as_str() == Some(v) || pv.as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(v)))))
        });
        if !matches {
            continue;
        }
        items.push(json!({
            "repository_id": r.id,
            "repository_name": r.name,
            "repository_full_name": format!("{}/{}", o.login, r.name),
            "properties": vals.iter().map(|(n, v)| json!({ "property_name": n, "value": v })).collect::<Vec<_>>(),
        }));
    }
    let total = items.len() as i64;
    let page: Vec<Value> = items.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), page))
}

#[derive(Deserialize)]
struct PropValue {
    property_name: Option<String>,
    #[serde(default)]
    value: Value,
}

fn parse_props(props: Option<Vec<PropValue>>) -> ApiResult<Vec<(String, Value)>> {
    let props = props.ok_or_else(|| ApiError::validation("CustomPropertyValue", "properties", "missing_field"))?;
    props
        .into_iter()
        .map(|p| {
            let n = p.property_name.ok_or_else(|| ApiError::validation("CustomPropertyValue", "property_name", "missing_field"))?;
            Ok((n, p.value))
        })
        .collect()
}

#[derive(Deserialize)]
struct SetValuesBody {
    repository_names: Option<Vec<String>>,
    properties: Option<Vec<PropValue>>,
}

async fn set_values(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<SetValuesBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_capability(&s, &o, &u, WRITE_VALUES).await?;
    let names = b.repository_names.ok_or_else(|| ApiError::validation("CustomPropertyValue", "repository_names", "missing_field"))?;
    if names.is_empty() || names.len() > 30 {
        return Err(bad("repository_names", "Between 1 and 30 repositories can be updated at once."));
    }
    let props = parse_props(b.properties)?;
    let mut repos = Vec::new();
    for n in &names {
        let ctx = db::load_repo(&s.db, Some(&u), &o.login, n).await.map_err(|_| bad("repository_names", &format!("Repository {n} not found.")))?;
        repos.push((ctx.repo.id, ctx.full_name()));
    }
    set_repo_property_values(&s, &o, &u, &repos, &props).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Values on the organization itself

fn check_org_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.chars().count() <= 75,
        Value::Array(a) => a.iter().all(|x| x.is_string()),
        _ => false,
    }
}

async fn get_org_values(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    let o = org_by_login(&s, &org).await?;
    require_member(&s, &o, &u).await?;
    let vals: Vec<(String, Value)> = sqlx::query_as("SELECT property_name, value FROM org_custom_property_values WHERE org_id = $1 ORDER BY property_name")
        .bind(o.id)
        .fetch_all(&s.db)
        .await?;
    Ok(Json(json!(vals.iter().map(|(n, v)| json!({ "property_name": n, "value": v })).collect::<Vec<_>>())))
}

#[derive(Deserialize)]
struct OrgValuesBody {
    properties: Option<Vec<PropValue>>,
}

async fn set_org_values(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<OrgValuesBody>) -> ApiResult<Response> {
    let o = org_by_login(&s, &org).await?;
    require_owner(&s, &o, &u).await?;
    let props = parse_props(b.properties)?;
    for (n, v) in &props {
        if !valid_name(n) || !check_org_value(v) {
            return Err(bad("value", &format!("Invalid value for property {n}")));
        }
    }
    let mut tx = s.db.begin().await?;
    for (n, v) in &props {
        if v.is_null() {
            sqlx::query("DELETE FROM org_custom_property_values WHERE org_id = $1 AND property_name = $2").bind(o.id).bind(n).execute(&mut *tx).await?;
        } else {
            sqlx::query(
                "INSERT INTO org_custom_property_values (org_id, property_name, value) VALUES ($1,$2,$3)
                 ON CONFLICT (org_id, property_name) DO UPDATE SET value = $3",
            )
            .bind(o.id)
            .bind(n)
            .bind(v)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    audit_quiet(&s.db, &o, Some(&u), "org.custom_property_values_updated", json!({})).await;
    Ok(no_content())
}
