//! SCIM 2.0 provisioning for the enterprise (`/scim/v2/enterprises/{enterprise}/...`).
//!
//! Users: provisioning creates a forge user (login from `userName`); `active: false` suspends
//! it, `active: true` unsuspends; the `enterprise_owner` role makes it a site admin; DELETE
//! removes the SCIM identity and suspends the user.
//! Groups: mirrored as enterprise-wide `external_groups` rows (orgs area) with their
//! `external_group_members`, so organization teams can link them (team membership then follows
//! the group), and enterprise teams whose `group_id` is the group mirror its members.
//! Site administrators only.
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::User;
use crate::enterprise::{self as ent, SiteAdmin};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const LIST_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";

pub fn routes() -> Router<AppState> {
    let b = "/scim/v2/enterprises/{enterprise}";
    Router::new()
        .route(&format!("{b}/Users"), get(list_users).post(create_user))
        .route(&format!("{b}/Users/{{id}}"), get(get_user).put(put_user).patch(patch_user).delete(delete_user))
        .route(&format!("{b}/Groups"), get(list_groups).post(create_group))
        .route(&format!("{b}/Groups/{{id}}"), get(get_group).put(put_group).patch(patch_group).delete(delete_group))
}

fn scim(status: StatusCode, v: Value) -> Response {
    let mut r = (status, axum::Json(v)).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/scim+json"));
    r
}

fn scim_error(status: StatusCode, detail: &str) -> ApiError {
    let mut e = ApiError::new(status, detail);
    e.errors = None;
    e
}

fn base(s: &AppState) -> String {
    format!("{}/scim/v2/enterprises/{}", s.api_url(), ent::slug())
}

#[derive(Deserialize)]
struct ListQuery {
    filter: Option<String>,
    #[serde(rename = "startIndex")]
    start_index: Option<i64>,
    count: Option<i64>,
    #[serde(rename = "excludedAttributes")]
    excluded_attributes: Option<String>,
}

/// `attr eq "value"` (the only filter GitHub supports).
fn parse_filter(f: &str) -> Option<(String, String)> {
    let mut parts = f.splitn(3, ' ');
    let attr = parts.next()?.to_string();
    if !parts.next()?.eq_ignore_ascii_case("eq") {
        return None;
    }
    Some((attr, parts.next()?.trim().trim_matches('"').to_string()))
}

fn list_response(total: i64, start: i64, items: Vec<Value>) -> Response {
    scim(
        StatusCode::OK,
        json!({ "schemas": [LIST_SCHEMA], "totalResults": total, "Resources": items.clone(), "startIndex": start, "itemsPerPage": items.len() }),
    )
}

// ---------------------------------------------------------------------------------------------
// Users

#[derive(sqlx::FromRow, Clone)]
struct ScimUser {
    id: String,
    user_id: i64,
    external_id: Option<String>,
    active: bool,
    data: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn user_json(s: &AppState, r: &ScimUser) -> ApiResult<Value> {
    let u = crate::db::user_by_id(&s.db, r.user_id).await?;
    let groups: Vec<(String, String)> = sqlx::query_as(
        "SELECT g.id, g.display_name FROM ent_scim_groups g JOIN external_group_members m ON m.group_id = g.external_group_id
          WHERE m.user_id = $1 ORDER BY g.display_name",
    )
    .bind(r.user_id)
    .fetch_all(&s.db)
    .await?;
    let b = base(s);
    let d = &r.data;
    let mut roles = d.get("roles").cloned().filter(Value::is_array).unwrap_or_else(|| json!([]));
    if u.site_admin && !roles.as_array().unwrap().iter().any(|x| x["value"] == "enterprise_owner") {
        roles.as_array_mut().unwrap().push(json!({"value": "enterprise_owner", "primary": false}));
    }
    let emails = d.get("emails").cloned().filter(Value::is_array).unwrap_or_else(|| match &u.email {
        Some(e) => json!([{"value": e, "type": "work", "primary": true}]),
        None => json!([]),
    });
    Ok(json!({
        "schemas": [USER_SCHEMA],
        "id": r.id,
        "externalId": r.external_id,
        "active": r.active,
        "userName": d.get("userName").cloned().unwrap_or(json!(u.login)),
        "name": d.get("name").cloned().filter(Value::is_object).unwrap_or_else(|| json!({})),
        "displayName": d.get("displayName").cloned().unwrap_or(json!(u.name)),
        "emails": emails,
        "roles": roles,
        "groups": groups.iter().map(|(id, n)| json!({"value": id, "$ref": format!("{b}/Groups/{id}"), "display": n})).collect::<Vec<_>>(),
        "meta": {
            "resourceType": "User",
            "created": crate::json::ts(r.created_at),
            "lastModified": crate::json::ts(r.updated_at),
            "location": format!("{b}/Users/{}", r.id),
        },
    }))
}

/// A forge login from a SCIM userName (the local part of an email, invalid characters -> `-`).
fn login_from(user_name: &str) -> String {
    let base = user_name.split('@').next().unwrap_or(user_name);
    let mut s: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').chars().take(39).collect()
}

struct UserInput {
    external_id: Option<String>,
    active: bool,
    user_name: String,
    display_name: Option<String>,
    email: Option<String>,
    owner: bool,
    data: Value,
}

fn parse_user(b: &Value) -> ApiResult<UserInput> {
    let bad = |f: &str| ApiError::validation("ScimUser", f, "missing_field");
    let user_name = b.get("userName").and_then(Value::as_str).filter(|x| !x.is_empty()).ok_or_else(|| bad("userName"))?.to_string();
    let emails = b.get("emails").and_then(Value::as_array).cloned().unwrap_or_default();
    let email = emails
        .iter()
        .find(|e| e.get("primary").and_then(Value::as_bool) == Some(true))
        .or_else(|| emails.first())
        .and_then(|e| e.get("value").and_then(Value::as_str))
        .map(str::to_string);
    let roles = b.get("roles").and_then(Value::as_array).cloned().unwrap_or_default();
    let owner = roles.iter().any(|r| matches!(r.get("value").and_then(Value::as_str), Some("enterprise_owner" | "981df190-8801-4618-a08a-d91f6206c954")));
    let display_name = b.get("displayName").and_then(Value::as_str).map(str::to_string).or_else(|| {
        let n = b.get("name")?;
        n.get("formatted").and_then(Value::as_str).map(str::to_string).or_else(|| {
            let g = n.get("givenName").and_then(Value::as_str).unwrap_or("");
            let f = n.get("familyName").and_then(Value::as_str).unwrap_or("");
            Some(format!("{g} {f}").trim().to_string()).filter(|x| !x.is_empty())
        })
    });
    let data = json!({
        "userName": user_name,
        "name": b.get("name").cloned().unwrap_or(json!({})),
        "displayName": display_name,
        "emails": emails,
        "roles": roles,
    });
    Ok(UserInput {
        external_id: b.get("externalId").and_then(Value::as_str).map(str::to_string),
        active: b.get("active").and_then(Value::as_bool).unwrap_or(true),
        user_name,
        display_name,
        email,
        owner,
        data,
    })
}

/// Applies identity attributes to the forge user (name, email, site admin, suspension).
async fn apply_user(s: &AppState, actor: &User, u: &User, i: &UserInput) -> ApiResult<()> {
    sqlx::query("UPDATE users SET name = COALESCE($2, name), email = COALESCE($3, email), updated_at = now() WHERE id = $1")
        .bind(u.id)
        .bind(&i.display_name)
        .bind(&i.email)
        .execute(&s.db)
        .await?;
    if i.owner != u.site_admin && u.id != actor.id {
        ent::set_site_admin(s, actor, u, i.owner).await?;
    }
    let u = crate::db::user_by_id(&s.db, u.id).await?;
    let suspended = ent::suspended_at(&s.db, u.id).await?.is_some();
    if i.active == suspended {
        if !i.active && u.site_admin {
            // Site admins are demoted before suspension when the IdP deactivates them.
            ent::set_site_admin(s, actor, &u, false).await?;
        }
        let u = crate::db::user_by_id(&s.db, u.id).await?;
        ent::set_suspended(s, Some(actor), &u, !i.active, Some("SCIM deprovisioning")).await?;
    }
    Ok(())
}

async fn load_user(s: &AppState, e: &str, id: &str) -> ApiResult<ScimUser> {
    ent::check_enterprise(e)?;
    sqlx::query_as("SELECT * FROM ent_scim_users WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(|| scim_error(StatusCode::NOT_FOUND, "Resource not found"))
}

async fn list_users(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let (attr, val) = match q.filter.as_deref().filter(|f| !f.is_empty()) {
        Some(f) => parse_filter(f).ok_or_else(|| scim_error(StatusCode::BAD_REQUEST, "Invalid filter"))?,
        None => (String::new(), String::new()),
    };
    let cond = match attr.as_str() {
        "" => "TRUE",
        "userName" => "lower(data->>'userName') = lower($1)",
        "externalId" => "external_id = $1",
        "id" => "id = $1",
        _ => return Err(scim_error(StatusCode::BAD_REQUEST, "Unsupported filter attribute")),
    };
    let start = q.start_index.unwrap_or(1).max(1);
    let count = q.count.unwrap_or(100).clamp(0, 100);
    let rows: Vec<ScimUser> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM ent_scim_users WHERE {cond} ORDER BY created_at, id LIMIT $2 OFFSET $3")))
        .bind(&val)
        .bind(count)
        .bind(start - 1)
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM ent_scim_users WHERE {cond}"))).bind(&val).fetch_one(&s.db).await?;
    let mut items = Vec::new();
    for r in &rows {
        items.push(user_json(&s, r).await?);
    }
    Ok(list_response(total, start, items))
}

async fn create_user(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(e): Path<String>, Json(b): Json<Value>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let i = parse_user(&b)?;
    let login = login_from(&i.user_name);
    if !crate::forge_api::valid_login(&login) {
        return Err(ApiError::validation("ScimUser", "userName", "invalid"));
    }
    if let Some(x) = &i.external_id {
        let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ent_scim_users WHERE external_id = $1)").bind(x).fetch_one(&s.db).await?;
        if taken {
            return Err(scim_error(StatusCode::CONFLICT, "User with this externalId already exists"));
        }
    }
    let u = match ent::admin_create_user(&s, Some(&a), &login, i.email.as_deref(), false).await {
        Ok(u) => u,
        Err(e) if e.status == StatusCode::UNPROCESSABLE_ENTITY => return Err(scim_error(StatusCode::CONFLICT, "User already exists")),
        Err(e) => return Err(e),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let r: ScimUser = sqlx::query_as("INSERT INTO ent_scim_users (id, user_id, external_id, active, data) VALUES ($1, $2, $3, $4, $5) RETURNING *")
        .bind(&id)
        .bind(u.id)
        .bind(&i.external_id)
        .bind(i.active)
        .bind(&i.data)
        .fetch_one(&s.db)
        .await?;
    apply_user(&s, &a, &u, &i).await?;
    ent::audit(&s, "external_identity.provision", Some(&a), json!({"user": u.login, "user_id": u.id, "scim_id": id})).await;
    let v = user_json(&s, &r).await?;
    let mut res = scim(StatusCode::CREATED, v);
    if let Ok(l) = HeaderValue::from_str(&format!("{}/Users/{id}", base(&s))) {
        res.headers_mut().insert(header::LOCATION, l);
    }
    Ok(res)
}

async fn get_user(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, String)>) -> ApiResult<Response> {
    let r = load_user(&s, &e, &id).await?;
    Ok(scim(StatusCode::OK, user_json(&s, &r).await?))
}

async fn save_user(s: &AppState, a: &User, r: &ScimUser, i: &UserInput) -> ApiResult<Response> {
    let r: ScimUser = sqlx::query_as("UPDATE ent_scim_users SET external_id = $2, active = $3, data = $4, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(&r.id)
        .bind(&i.external_id)
        .bind(i.active)
        .bind(&i.data)
        .fetch_one(&s.db)
        .await?;
    let u = crate::db::user_by_id(&s.db, r.user_id).await?;
    apply_user(s, a, &u, i).await?;
    ent::audit(s, "external_identity.update", Some(a), json!({"user": u.login, "user_id": u.id, "scim_id": r.id})).await;
    Ok(scim(StatusCode::OK, user_json(s, &r).await?))
}

async fn put_user(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, id)): Path<(String, String)>, Json(b): Json<Value>) -> ApiResult<Response> {
    let r = load_user(&s, &e, &id).await?;
    let i = parse_user(&b)?;
    save_user(&s, &a, &r, &i).await
}

/// Applies SCIM PatchOp operations to a JSON resource (`replace`/`add` with or without a path,
/// `remove` with a path). String values of boolean attributes ("false") are accepted.
fn apply_ops(resource: &mut Value, ops: &[Value]) -> ApiResult<()> {
    for op in ops {
        let kind = op.get("op").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let path = op.get("path").and_then(Value::as_str);
        let mut value = op.get("value").cloned().unwrap_or(Value::Null);
        if let Value::String(x) = &value {
            if x == "true" || x == "false" {
                if path.is_some_and(|p| p.eq_ignore_ascii_case("active")) {
                    value = json!(x == "true");
                }
            }
        }
        match (kind.as_str(), path) {
            ("replace" | "add", None) => {
                let Value::Object(m) = value else { return Err(scim_error(StatusCode::BAD_REQUEST, "Invalid patch value")) };
                for (k, v) in m {
                    set_path(resource, &k, v);
                }
            }
            ("replace" | "add", Some(p)) => set_path(resource, p, value),
            ("remove", Some(p)) => set_path(resource, p, Value::Null),
            _ => return Err(scim_error(StatusCode::BAD_REQUEST, "Invalid patch operation")),
        }
    }
    Ok(())
}

fn set_path(resource: &mut Value, path: &str, v: Value) {
    let mut parts = path.splitn(2, '.');
    let head = parts.next().unwrap_or(path);
    match parts.next() {
        Some(rest) => {
            if !resource[head].is_object() {
                resource[head] = json!({});
            }
            set_path(&mut resource[head], rest, v);
        }
        None => resource[head] = v,
    }
}

#[derive(Deserialize)]
struct PatchBody {
    #[serde(rename = "Operations")]
    operations: Option<Vec<Value>>,
}

async fn patch_user(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, id)): Path<(String, String)>, Json(b): Json<PatchBody>) -> ApiResult<Response> {
    let r = load_user(&s, &e, &id).await?;
    let ops = b.operations.ok_or_else(|| scim_error(StatusCode::BAD_REQUEST, "Operations are required"))?;
    let mut doc = r.data.clone();
    doc["externalId"] = json!(r.external_id);
    doc["active"] = json!(r.active);
    apply_ops(&mut doc, &ops)?;
    // emails[type eq "work"].value style paths are reduced to the primary email.
    if let Some(v) = doc.get("emails[type eq \"work\"]").and_then(|x| x.get("value")).cloned() {
        doc["emails"] = json!([{"value": v, "type": "work", "primary": true}]);
    }
    let i = parse_user(&doc)?;
    save_user(&s, &a, &r, &i).await
}

async fn delete_user(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, id)): Path<(String, String)>) -> ApiResult<Response> {
    let r = load_user(&s, &e, &id).await?;
    let u = crate::db::user_by_id(&s.db, r.user_id).await?;
    sqlx::query("DELETE FROM ent_scim_users WHERE id = $1").bind(&r.id).execute(&s.db).await?;
    sqlx::query("DELETE FROM external_group_members WHERE user_id = $1 AND group_id IN (SELECT external_group_id FROM ent_scim_groups)")
        .bind(u.id)
        .execute(&s.db)
        .await?;
    if u.id != a.id {
        if u.site_admin {
            ent::set_site_admin(&s, &a, &u, false).await?;
        }
        let u = crate::db::user_by_id(&s.db, u.id).await?;
        if ent::suspended_at(&s.db, u.id).await?.is_none() {
            ent::set_suspended(&s, Some(&a), &u, true, Some("SCIM identity deleted")).await?;
        }
    }
    ent::audit(&s, "external_identity.deprovision", Some(&a), json!({"user": u.login, "user_id": u.id, "scim_id": r.id})).await;
    super::ent_teams::reconcile_all(&s).await?;
    Ok(super::no_content())
}

// ---------------------------------------------------------------------------------------------
// Groups

#[derive(sqlx::FromRow, Clone)]
struct ScimGroup {
    id: String,
    external_group_id: i64,
    external_id: Option<String>,
    display_name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn group_json(s: &AppState, g: &ScimGroup, with_members: bool) -> ApiResult<Value> {
    let b = base(s);
    let members: Vec<(String, String)> = if with_members {
        sqlx::query_as(
            "SELECT su.id, u.login FROM external_group_members m JOIN users u ON u.id = m.user_id
               JOIN ent_scim_users su ON su.user_id = u.id WHERE m.group_id = $1 ORDER BY u.login",
        )
        .bind(g.external_group_id)
        .fetch_all(&s.db)
        .await?
    } else {
        vec![]
    };
    let mut v = json!({
        "schemas": [GROUP_SCHEMA],
        "id": g.id,
        "externalId": g.external_id,
        "displayName": g.display_name,
        "meta": {
            "resourceType": "Group",
            "created": crate::json::ts(g.created_at),
            "lastModified": crate::json::ts(g.updated_at),
            "location": format!("{b}/Groups/{}", g.id),
        },
    });
    if with_members {
        v["members"] = json!(members.iter().map(|(id, login)| json!({"value": id, "$ref": format!("{b}/Users/{id}"), "display": login})).collect::<Vec<_>>());
    }
    Ok(v)
}

async fn load_group(s: &AppState, e: &str, id: &str) -> ApiResult<ScimGroup> {
    ent::check_enterprise(e)?;
    sqlx::query_as("SELECT * FROM ent_scim_groups WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(|| scim_error(StatusCode::NOT_FOUND, "Resource not found"))
}

/// forge user ids for SCIM member references (SCIM user ids).
async fn member_ids(s: &AppState, members: &[Value]) -> ApiResult<Vec<i64>> {
    let ids: Vec<String> = members.iter().filter_map(|m| m.get("value").and_then(Value::as_str).map(str::to_string)).collect();
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT id, user_id FROM ent_scim_users WHERE id = ANY($1)").bind(&ids).fetch_all(&s.db).await?;
    if rows.len() != ids.iter().collect::<std::collections::HashSet<_>>().len() {
        return Err(scim_error(StatusCode::BAD_REQUEST, "Unknown group member"));
    }
    Ok(rows.into_iter().map(|(_, u)| u).collect())
}

/// Replaces the group's members and propagates to linked org teams and enterprise teams.
async fn set_members(s: &AppState, actor: &User, g: &ScimGroup, users: &[i64]) -> ApiResult<()> {
    let old: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM external_group_members WHERE group_id = $1").bind(g.external_group_id).fetch_all(&s.db).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM external_group_members WHERE group_id = $1").bind(g.external_group_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO external_group_members (group_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(g.external_group_id)
        .bind(users)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE external_groups SET updated_at = now(), name = $2 WHERE id = $1").bind(g.external_group_id).bind(&g.display_name).execute(&mut *tx).await?;
    tx.commit().await?;
    // Organization teams linked to this group follow its membership.
    let teams: Vec<super::orgs_teams::TeamRow> =
        sqlx::query_as("SELECT t.* FROM team_external_groups x JOIN teams t ON t.id = x.team_id WHERE x.group_id = $1").bind(g.external_group_id).fetch_all(&s.db).await?;
    for t in &teams {
        let org = crate::db::user_by_id(&s.db, t.org_id).await?;
        for uid in users.iter().filter(|u| !old.contains(u)) {
            let target = crate::db::user_by_id(&s.db, *uid).await?;
            super::orgs_teams::set_team_membership(s, &org, t, actor, &target, "member", true).await?;
        }
        for uid in old.iter().filter(|u| !users.contains(u)) {
            let target = crate::db::user_by_id(&s.db, *uid).await?;
            super::orgs_teams::remove_team_membership(s, &org, t, actor, &target).await?;
        }
    }
    super::ent_teams::reconcile_all(s).await
}

async fn list_groups(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path(e): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let (attr, val) = match q.filter.as_deref().filter(|f| !f.is_empty()) {
        Some(f) => parse_filter(f).ok_or_else(|| scim_error(StatusCode::BAD_REQUEST, "Invalid filter"))?,
        None => (String::new(), String::new()),
    };
    let cond = match attr.as_str() {
        "" => "TRUE",
        "displayName" => "lower(display_name) = lower($1)",
        "externalId" => "external_id = $1",
        "id" => "id = $1",
        _ => return Err(scim_error(StatusCode::BAD_REQUEST, "Unsupported filter attribute")),
    };
    let start = q.start_index.unwrap_or(1).max(1);
    let count = q.count.unwrap_or(100).clamp(0, 100);
    let rows: Vec<ScimGroup> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT * FROM ent_scim_groups WHERE {cond} ORDER BY created_at, id LIMIT $2 OFFSET $3")))
        .bind(&val)
        .bind(count)
        .bind(start - 1)
        .fetch_all(&s.db)
        .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM ent_scim_groups WHERE {cond}"))).bind(&val).fetch_one(&s.db).await?;
    let with_members = !q.excluded_attributes.as_deref().is_some_and(|x| x.split(',').any(|a| a.trim() == "members"));
    let mut items = Vec::new();
    for g in &rows {
        items.push(group_json(&s, g, with_members).await?);
    }
    Ok(list_response(total, start, items))
}

fn group_fields(b: &Value) -> ApiResult<(String, Option<String>, Vec<Value>)> {
    let name = b.get("displayName").and_then(Value::as_str).filter(|x| !x.trim().is_empty()).ok_or_else(|| ApiError::validation("ScimGroup", "displayName", "missing_field"))?;
    let ext = b.get("externalId").and_then(Value::as_str).map(str::to_string);
    let members = b.get("members").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok((name.trim().to_string(), ext, members))
}

async fn create_group(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path(e): Path<String>, Json(b): Json<Value>) -> ApiResult<Response> {
    ent::check_enterprise(&e)?;
    let (name, ext, members) = group_fields(&b)?;
    if let Some(x) = &ext {
        let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ent_scim_groups WHERE external_id = $1)").bind(x).fetch_one(&s.db).await?;
        if taken {
            return Err(scim_error(StatusCode::CONFLICT, "Group with this externalId already exists"));
        }
    }
    let users = member_ids(&s, &members).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut tx = s.db.begin().await?;
    let gid: i64 = sqlx::query_scalar("INSERT INTO external_groups (org_id, name) VALUES (NULL, $1) RETURNING id").bind(&name).fetch_one(&mut *tx).await?;
    let g: ScimGroup = sqlx::query_as("INSERT INTO ent_scim_groups (id, external_group_id, external_id, display_name) VALUES ($1, $2, $3, $4) RETURNING *")
        .bind(&id)
        .bind(gid)
        .bind(&ext)
        .bind(&name)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    set_members(&s, &a, &g, &users).await?;
    ent::audit(&s, "external_group.provision", Some(&a), json!({"group": name, "scim_id": id, "external_group_id": gid})).await;
    Ok(scim(StatusCode::CREATED, group_json(&s, &g, true).await?))
}

async fn get_group(State(s): State<AppState>, SiteAdmin(_): SiteAdmin, Path((e, id)): Path<(String, String)>, Query(q): Query<ListQuery>) -> ApiResult<Response> {
    let g = load_group(&s, &e, &id).await?;
    let with_members = !q.excluded_attributes.as_deref().is_some_and(|x| x.split(',').any(|a| a.trim() == "members"));
    Ok(scim(StatusCode::OK, group_json(&s, &g, with_members).await?))
}

async fn save_group(s: &AppState, a: &User, g: &ScimGroup, name: &str, ext: Option<String>, users: &[i64]) -> ApiResult<Response> {
    let g: ScimGroup = sqlx::query_as("UPDATE ent_scim_groups SET display_name = $2, external_id = $3, updated_at = now() WHERE id = $1 RETURNING *")
        .bind(&g.id)
        .bind(name)
        .bind(&ext)
        .fetch_one(&s.db)
        .await?;
    set_members(s, a, &g, users).await?;
    ent::audit(s, "external_group.update", Some(a), json!({"group": g.display_name, "scim_id": g.id})).await;
    Ok(scim(StatusCode::OK, group_json(s, &g, true).await?))
}

async fn put_group(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, id)): Path<(String, String)>, Json(b): Json<Value>) -> ApiResult<Response> {
    let g = load_group(&s, &e, &id).await?;
    let (name, ext, members) = group_fields(&b)?;
    let users = member_ids(&s, &members).await?;
    save_group(&s, &a, &g, &name, ext, &users).await
}

async fn patch_group(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, id)): Path<(String, String)>, Json(b): Json<PatchBody>) -> ApiResult<Response> {
    let g = load_group(&s, &e, &id).await?;
    let ops = b.operations.ok_or_else(|| scim_error(StatusCode::BAD_REQUEST, "Operations are required"))?;
    let current: Vec<(String, i64)> = sqlx::query_as(
        "SELECT su.id, m.user_id FROM external_group_members m JOIN ent_scim_users su ON su.user_id = m.user_id WHERE m.group_id = $1",
    )
    .bind(g.external_group_id)
    .fetch_all(&s.db)
    .await?;
    let mut members: Vec<String> = current.iter().map(|(id, _)| id.clone()).collect();
    let mut name = g.display_name.clone();
    let mut ext = g.external_id.clone();
    for op in &ops {
        let kind = op.get("op").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let path = op.get("path").and_then(Value::as_str).unwrap_or("");
        let value = op.get("value").cloned().unwrap_or(Value::Null);
        let refs = |v: &Value| -> Vec<String> {
            match v {
                Value::Array(a) => a.iter().filter_map(|m| m.get("value").and_then(Value::as_str).map(str::to_string)).collect(),
                Value::String(x) => vec![x.clone()],
                _ => vec![],
            }
        };
        match (kind.as_str(), path) {
            ("add", "members") => {
                for m in refs(&value) {
                    if !members.contains(&m) {
                        members.push(m);
                    }
                }
            }
            ("replace", "members") => members = refs(&value),
            ("remove", "members") => {
                let r = refs(&value);
                if r.is_empty() { members.clear() } else { members.retain(|m| !r.contains(m)) }
            }
            ("remove", p) if p.starts_with("members[value eq") => {
                let v = p.split('"').nth(1).unwrap_or("").to_string();
                members.retain(|m| *m != v);
            }
            ("replace" | "add", "displayName") => name = value.as_str().unwrap_or(&name).to_string(),
            ("replace" | "add", "externalId") => ext = value.as_str().map(str::to_string),
            ("replace" | "add", "") => {
                if let Some(n) = value.get("displayName").and_then(Value::as_str) {
                    name = n.to_string();
                }
                if let Some(x) = value.get("externalId").and_then(Value::as_str) {
                    ext = Some(x.to_string());
                }
                if let Some(m) = value.get("members") {
                    members = refs(m);
                }
            }
            _ => return Err(scim_error(StatusCode::BAD_REQUEST, "Invalid patch operation")),
        }
    }
    let users = member_ids(&s, &members.iter().map(|m| json!({"value": m})).collect::<Vec<_>>()).await?;
    save_group(&s, &a, &g, &name, ext, &users).await
}

async fn delete_group(State(s): State<AppState>, SiteAdmin(a): SiteAdmin, Path((e, id)): Path<(String, String)>) -> ApiResult<Response> {
    let g = load_group(&s, &e, &id).await?;
    set_members(&s, &a, &g, &[]).await?;
    sqlx::query("UPDATE ent_teams SET group_id = NULL WHERE group_id = $1").bind(&g.id).execute(&s.db).await?;
    sqlx::query("DELETE FROM ent_scim_groups WHERE id = $1").bind(&g.id).execute(&s.db).await?;
    sqlx::query("DELETE FROM external_groups WHERE id = $1").bind(g.external_group_id).execute(&s.db).await?;
    ent::audit(&s, "external_group.delete", Some(&a), json!({"group": g.display_name, "scim_id": g.id})).await;
    Ok(super::no_content())
}
