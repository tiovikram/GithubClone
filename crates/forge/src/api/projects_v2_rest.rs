//! REST API for Projects (v2) (`/orgs/{org}/projectsV2...`, `/users/{username}/projectsV2...`):
//! projects, fields, items (issues, pull requests, draft issues), item field values and views.
//! Built on the same service layer as the GraphQL API (`api::projects_v2`), so both see the same
//! data. Projects themselves are created through GraphQL (`createProjectV2`), like on GitHub.
//!
//! Lists use GitHub's cursor pagination (`before`/`after`, `per_page`, `Link` rel next/prev).
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};

use super::no_content;
use super::projects_v2::{self as svc, Access, FieldRow, FieldValue, ItemRow, NewOption, ProjectRow};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json as Body;
use crate::ids;
use crate::json::issue::IssueRow;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/projectsV2", get(list_org))
        .route("/users/{username}/projectsV2", get(list_user))
        .route("/orgs/{org}/projectsV2/{number}", get(get_org))
        .route("/users/{username}/projectsV2/{number}", get(get_user))
        .route("/orgs/{org}/projectsV2/{number}/drafts", post(draft_org))
        .route("/user/{account_id}/projectsV2/{number}/drafts", post(draft_me))
        .route("/orgs/{org}/projectsV2/{number}/fields", get(fields_org).post(add_field_org))
        .route("/users/{username}/projectsV2/{number}/fields", get(fields_user).post(add_field_user))
        .route("/orgs/{org}/projectsV2/{number}/fields/{field_id}", get(field_org))
        .route("/users/{username}/projectsV2/{number}/fields/{field_id}", get(field_user))
        .route("/orgs/{org}/projectsV2/{number}/items", get(items_org).post(add_item_org))
        .route("/users/{username}/projectsV2/{number}/items", get(items_user).post(add_item_user))
        .route("/orgs/{org}/projectsV2/{number}/items/{item_id}", get(item_org).patch(update_item_org).delete(delete_item_org))
        .route("/users/{username}/projectsV2/{number}/items/{item_id}", get(item_user).patch(update_item_user).delete(delete_item_user))
        .route("/orgs/{org}/projectsV2/{number}/views", post(view_org))
        .route("/users/{username}/projectsV2/{number}/views", post(view_user))
        .route("/orgs/{org}/projectsV2/{number}/views/{view_number}/items", get(view_items_org))
        .route("/users/{username}/projectsV2/{number}/views/{view_number}/items", get(view_items_user))
}

// ---------------------------------------------------------------------------------------------
// Context

enum Owner {
    Org(String),
    Login(String),
    /// `/user/{user_id}` and `/users/{user_id}/.../views` address the user by numeric id.
    Id(String),
}

/// A project resolved for the viewer.
struct Ctx {
    owner: User,
    project: ProjectRow,
    access: Access,
    /// `{api}/orgs/o/projectsV2/1` or `{api}/users/u/projectsV2/1`.
    url: String,
    html: String,
}

async fn resolve_owner(s: &AppState, o: &Owner) -> ApiResult<User> {
    match o {
        Owner::Org(l) => crate::orgs::org_by_login(s, l).await,
        Owner::Login(l) => db::user_by_login(&s.db, l).await,
        Owner::Id(id) => match id.parse::<i64>() {
            Ok(n) => match db::user_by_id(&s.db, n).await {
                Ok(u) => Ok(u),
                Err(_) => db::user_by_login(&s.db, id).await,
            },
            Err(_) => db::user_by_login(&s.db, id).await,
        },
    }
}

fn ns(owner: &User) -> &'static str {
    if owner.kind == "Organization" { "orgs" } else { "users" }
}

async fn ctx(s: &AppState, viewer: Option<&User>, o: Owner, number: i64) -> ApiResult<Ctx> {
    let owner = resolve_owner(s, &o).await?;
    if matches!(o, Owner::Login(_)) && owner.kind == "Organization" {
        return Err(ApiError::not_found());
    }
    let project = svc::by_number(s, viewer, owner.id, number).await?.ok_or_else(ApiError::not_found)?;
    let access = svc::access(s, viewer, &project).await?;
    let url = format!("{}/{}/{}/projectsV2/{}", s.api_url(), ns(&owner), owner.login, project.number);
    let html = format!("{}/{}/{}/projects/{}", s.base_url(), ns(&owner), owner.login, project.number);
    Ok(Ctx { owner, project, access, url, html })
}

impl Ctx {
    fn require_write(&self) -> ApiResult<()> {
        match self.access {
            Access::Write => Ok(()),
            Access::Read => Err(ApiError::forbidden("You don't have permission to modify this project.")),
            Access::None => Err(ApiError::not_found()),
        }
    }
}

fn esc(s: &str) -> String {
    crate::graphql::repo::html_escape(s)
}

fn text(s: &str) -> Value {
    json!({ "raw": s, "html": esc(s) })
}

// ---------------------------------------------------------------------------------------------
// Cursor pagination

#[derive(Deserialize, Default)]
struct CursorParams {
    per_page: Option<i64>,
    before: Option<String>,
    after: Option<String>,
    q: Option<String>,
    fields: Option<String>,
}

fn encode_cursor(pos: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("cursor:{pos}"))
}

fn decode_cursor(c: &str) -> Option<usize> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(c.trim_end_matches('=')).ok()?;
    String::from_utf8(raw).ok()?.strip_prefix("cursor:")?.parse().ok()
}

/// Slices `items` by `before`/`after` and renders the page with a `Link` header.
fn cursor_page(s: &AppState, uri: &OriginalUri, p: &CursorParams, items: Vec<Value>) -> ApiResult<Response> {
    let per = p.per_page.unwrap_or(30).clamp(1, 100) as usize;
    let bad = || ApiError::validation("Cursor", "after", "invalid");
    let (start, end) = if let Some(a) = p.after.as_deref().filter(|a| !a.is_empty()) {
        let s0 = decode_cursor(a).ok_or_else(bad)? + 1;
        (s0, s0 + per)
    } else if let Some(b) = p.before.as_deref().filter(|b| !b.is_empty()) {
        let e = decode_cursor(b).ok_or_else(bad)?;
        (e.saturating_sub(per), e)
    } else {
        (0, per)
    };
    let total = items.len();
    let start = start.min(total);
    let end = end.min(total);
    let page: Vec<Value> = items[start..end].to_vec();
    let mut links = Vec::new();
    let url = |k: &str, c: String| {
        let mut pairs: Vec<(String, String)> = uri
            .0
            .query()
            .map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect())
            .unwrap_or_default();
        pairs.retain(|(k, _)| k != "after" && k != "before" && k != "per_page" && k != "page");
        pairs.push(("per_page".into(), per.to_string()));
        pairs.push((k.into(), c));
        let q = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        format!("{}{}?{q}", s.base_url(), uri.0.path())
    };
    if end < total && end > 0 {
        links.push(format!("<{}>; rel=\"next\"", url("after", encode_cursor(end - 1))));
    }
    if start > 0 {
        links.push(format!("<{}>; rel=\"prev\"", url("before", encode_cursor(start))));
    }
    let mut res = (StatusCode::OK, Json(Value::Array(page))).into_response();
    if !links.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&links.join(", ")) {
            res.headers_mut().insert(header::LINK, v);
        }
    }
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// JSON

async fn user_json(s: &AppState, id: Option<i64>, fallback: &User) -> Value {
    let u = match id {
        Some(id) => db::user_by_id(&s.db, id).await.ok(),
        None => None,
    };
    crate::json::user::simple(&s.api_url(), s.base_url(), u.as_ref().unwrap_or(fallback))
}

async fn project_json(s: &AppState, owner: &User, p: &ProjectRow) -> Value {
    json!({
        "id": p.id,
        "node_id": ids::encode("ProjectV2", p.id),
        "owner": crate::json::user::simple(&s.api_url(), s.base_url(), owner),
        "creator": user_json(s, p.creator_id, owner).await,
        "title": p.title,
        "description": p.readme,
        "public": p.public,
        "closed_at": crate::json::ts_opt(p.closed_at),
        "created_at": crate::json::ts(p.created_at),
        "updated_at": crate::json::ts(p.updated_at),
        "number": p.number,
        "short_description": p.short_description,
        "deleted_at": Value::Null,
        "deleted_by": Value::Null,
        "state": if p.closed { "closed" } else { "open" },
        "latest_status_update": Value::Null,
        "is_template": p.template,
    })
}

/// REST `data_type` of a stored field type (`None` for types the REST API does not expose).
fn rest_type(t: &str) -> Option<&'static str> {
    Some(match t {
        "ASSIGNEES" => "assignees",
        "LINKED_PULL_REQUESTS" => "linked_pull_requests",
        "REVIEWERS" => "reviewers",
        "LABELS" => "labels",
        "MILESTONE" => "milestone",
        "REPOSITORY" => "repository",
        "TITLE" => "title",
        "TEXT" => "text",
        "SINGLE_SELECT" => "single_select",
        "NUMBER" => "number",
        "DATE" => "date",
        "ITERATION" => "iteration",
        "PARENT_ISSUE" => "parent_issue",
        "SUB_ISSUES_PROGRESS" => "sub_issues_progress",
        _ => return None,
    })
}

fn option_json(o: &Value) -> Value {
    json!({
        "id": o["id"],
        "name": text(o["name"].as_str().unwrap_or("")),
        "color": o["color"].as_str().unwrap_or("GRAY"),
        "description": text(o["description"].as_str().unwrap_or("")),
    })
}

fn iteration_json(i: &Value) -> Value {
    let start = i["start_date"].as_str().unwrap_or("");
    let duration = i["duration"].as_i64().unwrap_or(14);
    let completed = NaiveDate::parse_from_str(start, "%Y-%m-%d")
        .map(|d| d + chrono::Duration::days(duration) <= chrono::Utc::now().date_naive())
        .unwrap_or(false);
    json!({
        "id": i["id"],
        "start_date": start,
        "duration": duration,
        "title": text(i["title"].as_str().unwrap_or("")),
        "completed": completed,
    })
}

fn field_json(c: &Ctx, f: &FieldRow) -> Value {
    let mut v = json!({
        "id": f.id,
        "node_id": ids::encode(f.gql_kind(), f.id),
        "project_url": c.url,
        "name": f.name,
        "data_type": rest_type(&f.data_type).unwrap_or("text"),
        "created_at": crate::json::ts(f.created_at),
        "updated_at": crate::json::ts(f.updated_at),
    });
    if f.data_type == "SINGLE_SELECT" {
        v["options"] = Value::Array(f.options.as_array().into_iter().flatten().map(option_json).collect());
    }
    if f.data_type == "ITERATION" {
        v["configuration"] = json!({
            "start_day": f.config["start_day"].as_i64().unwrap_or(1),
            "duration": f.config["duration"].as_i64().unwrap_or(14),
            "iterations": f.config["iterations"].as_array().into_iter().flatten().map(iteration_json).collect::<Vec<_>>(),
        });
    }
    v
}

/// Content of issue/PR items, keyed by issue id, for issues the viewer can read.
struct Contents {
    json: HashMap<i64, Value>,
    rows: HashMap<i64, IssueRow>,
    ctxs: HashMap<i64, RepoCtx>,
}

async fn load_contents(s: &AppState, viewer: Option<&User>, items: &[ItemRow]) -> ApiResult<Contents> {
    let ids: Vec<i64> = items.iter().filter_map(|i| i.issue_id).collect();
    let rows: Vec<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE id = ANY($1)").bind(&ids).fetch_all(&s.db).await?;
    let mut repo_ids: Vec<i64> = rows.iter().map(|r| r.repo_id).collect();
    repo_ids.sort_unstable();
    repo_ids.dedup();
    let ctxs = super::issues::repo_ctxs(s, viewer, &repo_ids).await?;
    let rows: Vec<IssueRow> = rows.into_iter().filter(|r| ctxs.get(&r.repo_id).is_some_and(|c| c.perm >= Perm::Read)).collect();
    let rendered = super::issues::issues_json_multi(s, viewer, rows.clone(), false).await?;
    let json = rows.iter().map(|r| r.id).zip(rendered).collect();
    Ok(Contents { json, rows: rows.into_iter().map(|r| (r.id, r)).collect(), ctxs })
}

fn draft_json(s: &AppState, item: &ItemRow, creator: &Value) -> Value {
    let _ = s;
    json!({
        "id": item.id,
        "node_id": ids::encode("DraftIssue", item.id),
        "title": item.draft_title,
        "body": item.draft_body,
        "user": creator,
        "created_at": crate::json::ts(item.created_at),
        "updated_at": crate::json::ts(item.updated_at),
    })
}

async fn archived_at(s: &AppState, ids: &[i64]) -> ApiResult<HashMap<i64, chrono::DateTime<chrono::Utc>>> {
    let rows: Vec<(i64, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as("SELECT id, archived_at FROM projects_v2_items WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(&s.db)
        .await?;
    Ok(rows.into_iter().filter_map(|(i, t)| t.map(|t| (i, t))).collect())
}

/// The value of `f` for an item, in GitHub's REST shape.
fn field_value(s: &AppState, f: &FieldRow, item: &ItemRow, content: Option<&Value>, issue: Option<&IssueRow>, repo: Option<&RepoCtx>, stored: &HashMap<i64, Value>) -> Value {
    let c = content.cloned().unwrap_or(Value::Null);
    match f.data_type.as_str() {
        "TITLE" => match issue {
            Some(i) => json!({
                "raw": i.title, "html": esc(&i.title), "number": i.number, "url": c["html_url"],
                "issue_id": i.id, "state": i.state, "state_reason": i.state_reason, "is_draft": c["draft"].as_bool().unwrap_or(false),
            }),
            None => text(item.draft_title.as_deref().unwrap_or("")),
        },
        "ASSIGNEES" => c.get("assignees").cloned().unwrap_or(json!([])),
        "LABELS" => c.get("labels").cloned().unwrap_or(json!([])),
        "MILESTONE" => c.get("milestone").cloned().unwrap_or(Value::Null),
        "REPOSITORY" => repo.map(|r| crate::json::repo::repo(&s.api_url(), s.base_url(), r)).unwrap_or(Value::Null),
        "LINKED_PULL_REQUESTS" | "REVIEWERS" => json!([]),
        "TEXT" => stored.get(&f.id).map(|v| v["text"].clone()).unwrap_or(Value::Null),
        "NUMBER" => stored.get(&f.id).map(|v| v["number"].clone()).unwrap_or(Value::Null),
        "DATE" => stored.get(&f.id).map(|v| v["date"].clone()).unwrap_or(Value::Null),
        "SINGLE_SELECT" => stored
            .get(&f.id)
            .and_then(|v| v["option_id"].as_str())
            .and_then(|o| f.options.as_array().and_then(|a| a.iter().find(|x| x["id"] == o)))
            .map(option_json)
            .unwrap_or(Value::Null),
        "ITERATION" => stored
            .get(&f.id)
            .and_then(|v| v["iteration_id"].as_str())
            .and_then(|i| f.iteration(i))
            .map(|i| {
                let mut v = iteration_json(&i);
                if let Some(o) = v.as_object_mut() {
                    o.remove("completed");
                }
                v
            })
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Parses `fields` (`1,2,3`) into field ids.
fn requested_fields(p: Option<&str>) -> Vec<i64> {
    p.unwrap_or("").split(',').filter_map(|x| x.trim().parse().ok()).collect()
}

/// Renders items (`projects-v2-item-with-content`), dropping those whose content the viewer can't
/// read. `fields`: the field ids to include (default: Title).
async fn items_json(s: &AppState, viewer: Option<&User>, c: &Ctx, items: &[ItemRow], fields: &[i64]) -> ApiResult<Vec<Value>> {
    let all_fields = svc::fields(s, c.project.id).await?;
    let wanted: Vec<&FieldRow> = if fields.is_empty() {
        all_fields.iter().filter(|f| f.data_type == "TITLE").collect()
    } else {
        all_fields.iter().filter(|f| fields.contains(&f.id) && rest_type(&f.data_type).is_some()).collect()
    };
    let contents = load_contents(s, viewer, items).await?;
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let archived = archived_at(s, &ids).await?;
    let values: Vec<(i64, i64, Value)> = sqlx::query_as("SELECT item_id, field_id, value FROM projects_v2_item_values WHERE item_id = ANY($1)")
        .bind(&ids)
        .fetch_all(&s.db)
        .await?;
    let mut stored: HashMap<i64, HashMap<i64, Value>> = HashMap::new();
    for (i, f, v) in values {
        stored.entry(i).or_default().insert(f, v);
    }
    let creator_ids: Vec<i64> = items.iter().filter_map(|i| i.creator_id).collect();
    let creators = crate::json::issue::users_by_ids(&s.db, &creator_ids).await?;
    let empty = HashMap::new();
    let mut out = Vec::new();
    for item in items {
        let creator = crate::json::issue::user_or_ghost(&s.api_url(), s.base_url(), item.creator_id.and_then(|id| creators.get(&id)));
        let (content_type, content, issue) = match item.issue_id {
            Some(iid) => {
                let Some(row) = contents.rows.get(&iid) else { continue };
                (if row.is_pull { "PullRequest" } else { "Issue" }, contents.json.get(&iid).cloned(), Some(row))
            }
            None => ("DraftIssue", Some(draft_json(s, item, &creator)), None),
        };
        let repo = issue.and_then(|i| contents.ctxs.get(&i.repo_id));
        let st = stored.get(&item.id).unwrap_or(&empty);
        let fvals: Vec<Value> = wanted
            .iter()
            .map(|f| {
                json!({
                    "id": f.id,
                    "name": f.name,
                    "data_type": rest_type(&f.data_type).unwrap_or("text"),
                    "value": field_value(s, f, item, content.as_ref(), issue, repo, st),
                })
            })
            .collect();
        out.push(json!({
            "id": item.id,
            "node_id": ids::encode("ProjectV2Item", item.id),
            "project_url": c.url,
            "content_type": content_type,
            "content": content,
            "creator": creator,
            "created_at": crate::json::ts(item.created_at),
            "updated_at": crate::json::ts(item.updated_at),
            "archived_at": crate::json::ts_opt(archived.get(&item.id).copied().or(if item.archived { Some(item.updated_at) } else { None })),
            "item_url": format!("{}/items/{}", c.url, item.id),
            "fields": fvals,
        }));
    }
    Ok(out)
}

/// `projects-v2-item-simple`.
async fn item_simple(s: &AppState, viewer: Option<&User>, c: &Ctx, item: &ItemRow) -> ApiResult<Value> {
    let mut v = items_json(s, viewer, c, std::slice::from_ref(item), &[]).await?.pop().ok_or_else(ApiError::not_found)?;
    if let Some(o) = v.as_object_mut() {
        o.remove("fields");
    }
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// Item filtering (`q` and view filters)

/// Applies a projects filter: `is:issue|pr|draft|open|closed|archived` (and `-is:`), free text in
/// the title. Without `is:archived`, `exclude_archived` hides archived items.
fn filter_items(items: Vec<ItemRow>, contents: &Contents, q: Option<&str>, exclude_archived: bool) -> Vec<ItemRow> {
    let mut checks: Vec<(bool, String)> = Vec::new();
    let mut words = Vec::new();
    let mut archived_mentioned = false;
    for t in q.unwrap_or("").split_whitespace() {
        let (neg, t) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t),
        };
        if let Some(k) = t.strip_prefix("is:") {
            if k == "archived" {
                archived_mentioned = true;
            }
            checks.push((neg, k.to_lowercase()));
        } else if !t.contains(':') {
            words.push(t.to_lowercase());
        }
    }
    items
        .into_iter()
        .filter(|i| {
            let row = i.issue_id.and_then(|id| contents.rows.get(&id));
            if i.issue_id.is_some() && row.is_none() {
                return false;
            }
            if exclude_archived && !archived_mentioned && i.archived {
                return false;
            }
            let title = row.map(|r| r.title.clone()).or(i.draft_title.clone()).unwrap_or_default().to_lowercase();
            if !words.iter().all(|w| title.contains(w.as_str())) {
                return false;
            }
            checks.iter().all(|(neg, k)| {
                let hit = match k.as_str() {
                    "issue" => row.is_some_and(|r| !r.is_pull),
                    "pr" => row.is_some_and(|r| r.is_pull),
                    "draft" => row.is_none(),
                    "open" => row.is_some_and(|r| r.state == "open") || row.is_none(),
                    "closed" => row.is_some_and(|r| r.state == "closed"),
                    "archived" => i.archived,
                    _ => true,
                };
                hit != *neg
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Handlers: projects

async fn list(s: AppState, uri: OriginalUri, viewer: Option<User>, o: Owner, p: CursorParams) -> ApiResult<Response> {
    let owner = resolve_owner(&s, &o).await?;
    if matches!(o, Owner::Login(_)) && owner.kind == "Organization" {
        return Err(ApiError::not_found());
    }
    let rows = svc::list_for_owner(&s, viewer.as_ref(), &owner, p.q.as_deref()).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        items.push(project_json(&s, &owner, r).await);
    }
    cursor_page(&s, &uri, &p, items)
}

async fn list_org(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path(org): Path<String>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    list(s, uri, v.0, Owner::Org(org), p).await
}
async fn list_user(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path(u): Path<String>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    list(s, uri, v.0, Owner::Login(u), p).await
}

async fn get_project(s: AppState, viewer: Option<User>, o: Owner, number: i64) -> ApiResult<Json<Value>> {
    let c = ctx(&s, viewer.as_ref(), o, number).await?;
    Ok(Json(project_json(&s, &c.owner, &c.project).await))
}

async fn get_org(State(s): State<AppState>, v: MaybeUser, Path((org, n)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    get_project(s, v.0, Owner::Org(org), n).await
}
async fn get_user(State(s): State<AppState>, v: MaybeUser, Path((u, n)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    get_project(s, v.0, Owner::Login(u), n).await
}

// ---------------------------------------------------------------------------------------------
// Handlers: draft items

#[derive(Deserialize)]
struct DraftInput {
    title: Option<String>,
    body: Option<String>,
}

async fn create_draft(s: AppState, actor: User, o: Owner, number: i64, input: DraftInput) -> ApiResult<Response> {
    let c = ctx(&s, Some(&actor), o, number).await?;
    let title = input.title.ok_or_else(|| ApiError::validation("ProjectV2Item", "title", "missing_field"))?;
    let item = svc::add_draft(&s, &actor, c.project.id, &title, input.body.as_deref()).await?;
    let v = item_simple(&s, Some(&actor), &c, &item).await?;
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

async fn draft_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, i64)>, Body(b): Body<DraftInput>) -> ApiResult<Response> {
    create_draft(s, u, Owner::Org(org), n, b).await
}
async fn draft_me(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, n)): Path<(String, i64)>, Body(b): Body<DraftInput>) -> ApiResult<Response> {
    create_draft(s, u, Owner::Id(id), n, b).await
}

// ---------------------------------------------------------------------------------------------
// Handlers: fields

async fn list_fields(s: AppState, uri: OriginalUri, viewer: Option<User>, o: Owner, number: i64, p: CursorParams) -> ApiResult<Response> {
    let c = ctx(&s, viewer.as_ref(), o, number).await?;
    let fields = svc::fields(&s, c.project.id).await?;
    let items = fields.iter().filter(|f| rest_type(&f.data_type).is_some()).map(|f| field_json(&c, f)).collect();
    cursor_page(&s, &uri, &p, items)
}

async fn fields_org(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path((org, n)): Path<(String, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    list_fields(s, uri, v.0, Owner::Org(org), n, p).await
}
async fn fields_user(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path((u, n)): Path<(String, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    list_fields(s, uri, v.0, Owner::Login(u), n, p).await
}

async fn get_field(s: AppState, viewer: Option<User>, o: Owner, number: i64, field_id: i64) -> ApiResult<Json<Value>> {
    let c = ctx(&s, viewer.as_ref(), o, number).await?;
    let f = svc::field_by_id(&s, field_id)
        .await?
        .filter(|f| f.project_id == c.project.id && rest_type(&f.data_type).is_some())
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(field_json(&c, &f)))
}

async fn field_org(State(s): State<AppState>, v: MaybeUser, Path((org, n, f)): Path<(String, i64, i64)>) -> ApiResult<Json<Value>> {
    get_field(s, v.0, Owner::Org(org), n, f).await
}
async fn field_user(State(s): State<AppState>, v: MaybeUser, Path((u, n, f)): Path<(String, i64, i64)>) -> ApiResult<Json<Value>> {
    get_field(s, v.0, Owner::Login(u), n, f).await
}

#[derive(Deserialize)]
struct OptionInput {
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct IterationInput {
    title: Option<String>,
    start_date: Option<String>,
    duration: Option<i64>,
}

#[derive(Deserialize)]
struct IterationConfig {
    start_date: Option<String>,
    duration: Option<i64>,
    iterations: Option<Vec<IterationInput>>,
}

#[derive(Deserialize)]
struct FieldInput {
    issue_field_id: Option<i64>,
    name: Option<String>,
    data_type: Option<String>,
    single_select_options: Option<Vec<OptionInput>>,
    iteration_configuration: Option<IterationConfig>,
}

const COLORS: &[&str] = &["BLUE", "GRAY", "GREEN", "ORANGE", "PINK", "PURPLE", "RED", "YELLOW"];

fn parse_date(field: &str, s: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| ApiError::validation("ProjectV2Field", field, "invalid"))
}

async fn add_field(s: AppState, actor: User, o: Owner, number: i64, input: FieldInput) -> ApiResult<Response> {
    let c = ctx(&s, Some(&actor), o, number).await?;
    c.require_write()?;
    if input.issue_field_id.is_some() {
        return Err(ApiError::unprocessable("Issue fields are not available on this server."));
    }
    let name = input.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("ProjectV2Field", "name", "missing_field"))?;
    let ty = match input.data_type.as_deref() {
        Some("text") => "TEXT",
        Some("number") => "NUMBER",
        Some("date") => "DATE",
        Some("single_select") => "SINGLE_SELECT",
        Some("iteration") => "ITERATION",
        Some(_) => return Err(ApiError::validation("ProjectV2Field", "data_type", "invalid")),
        None => return Err(ApiError::validation("ProjectV2Field", "data_type", "missing_field")),
    };
    let mut options = Vec::new();
    if ty == "SINGLE_SELECT" {
        for o in input.single_select_options.unwrap_or_default() {
            let color = o.color.unwrap_or_else(|| "GRAY".into()).to_uppercase();
            if !COLORS.contains(&color.as_str()) {
                return Err(ApiError::validation("ProjectV2Field", "single_select_options", "invalid"));
            }
            let n = o.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("ProjectV2Field", "single_select_options", "missing_field"))?;
            options.push(NewOption { name: n, color, description: o.description.unwrap_or_default() });
        }
        if options.is_empty() {
            return Err(ApiError::validation("ProjectV2Field", "single_select_options", "missing_field"));
        }
    }
    let (mut duration, mut start, mut iterations) = (None, None, None);
    if ty == "ITERATION" {
        let cfg = input.iteration_configuration.ok_or_else(|| ApiError::validation("ProjectV2Field", "iteration_configuration", "missing_field"))?;
        duration = Some(cfg.duration.filter(|d| (1..=365).contains(d)).ok_or_else(|| ApiError::validation("ProjectV2Field", "duration", "invalid"))?);
        start = Some(parse_date("start_date", cfg.start_date.as_deref().unwrap_or(""))?);
        if let Some(its) = cfg.iterations.filter(|i| !i.is_empty()) {
            let mut list = Vec::new();
            for i in its {
                let sd = parse_date("start_date", i.start_date.as_deref().unwrap_or(""))?;
                let d = i.duration.filter(|d| (1..=365).contains(d)).ok_or_else(|| ApiError::validation("ProjectV2Field", "duration", "invalid"))?;
                let t = i.title.filter(|t| !t.trim().is_empty()).ok_or_else(|| ApiError::validation("ProjectV2Field", "title", "missing_field"))?;
                list.push(json!({ "id": crate::auth::random_string(8).to_lowercase(), "title": t, "start_date": sd.to_string(), "duration": d }));
            }
            iterations = Some(list);
        }
    }
    let mut f = svc::create_field(&s, &actor, c.project.id, &name, ty, options, duration, start).await?;
    if let Some(list) = iterations {
        let mut cfg = f.config.clone();
        cfg["iterations"] = Value::Array(list);
        f = sqlx::query_as("UPDATE projects_v2_fields SET config = $2, updated_at = now() WHERE id = $1 RETURNING *")
            .bind(f.id)
            .bind(cfg)
            .fetch_one(&s.db)
            .await?;
    }
    Ok((StatusCode::CREATED, Json(field_json(&c, &f))).into_response())
}

async fn add_field_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, i64)>, Body(b): Body<FieldInput>) -> ApiResult<Response> {
    add_field(s, u, Owner::Org(org), n, b).await
}
async fn add_field_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((l, n)): Path<(String, i64)>, Body(b): Body<FieldInput>) -> ApiResult<Response> {
    add_field(s, u, Owner::Login(l), n, b).await
}

// ---------------------------------------------------------------------------------------------
// Handlers: items

async fn list_items(s: AppState, uri: OriginalUri, viewer: Option<User>, o: Owner, number: i64, p: CursorParams) -> ApiResult<Response> {
    let c = ctx(&s, viewer.as_ref(), o, number).await?;
    let items = svc::items(&s, c.project.id).await?;
    let contents = load_contents(&s, viewer.as_ref(), &items).await?;
    let items = filter_items(items, &contents, p.q.as_deref(), false);
    let rendered = items_json(&s, viewer.as_ref(), &c, &items, &requested_fields(p.fields.as_deref())).await?;
    cursor_page(&s, &uri, &p, rendered)
}

async fn items_org(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path((org, n)): Path<(String, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    list_items(s, uri, v.0, Owner::Org(org), n, p).await
}
async fn items_user(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path((u, n)): Path<(String, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    list_items(s, uri, v.0, Owner::Login(u), n, p).await
}

async fn load_item(s: &AppState, c: &Ctx, item_id: i64) -> ApiResult<ItemRow> {
    svc::item_by_id(s, item_id).await?.filter(|i| i.project_id == c.project.id).ok_or_else(ApiError::not_found)
}

async fn get_item(s: AppState, viewer: Option<User>, o: Owner, number: i64, item_id: i64, p: CursorParams) -> ApiResult<Json<Value>> {
    let c = ctx(&s, viewer.as_ref(), o, number).await?;
    let item = load_item(&s, &c, item_id).await?;
    let v = items_json(&s, viewer.as_ref(), &c, &[item], &requested_fields(p.fields.as_deref())).await?.pop().ok_or_else(ApiError::not_found)?;
    Ok(Json(v))
}

async fn item_org(State(s): State<AppState>, v: MaybeUser, Path((org, n, i)): Path<(String, i64, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Json<Value>> {
    get_item(s, v.0, Owner::Org(org), n, i, p).await
}
async fn item_user(State(s): State<AppState>, v: MaybeUser, Path((u, n, i)): Path<(String, i64, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Json<Value>> {
    get_item(s, v.0, Owner::Login(u), n, i, p).await
}

#[derive(Deserialize)]
struct AddItemInput {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<i64>,
    owner: Option<String>,
    repo: Option<String>,
    number: Option<i64>,
}

async fn add_item(s: AppState, actor: User, o: Owner, number: i64, input: AddItemInput) -> ApiResult<Response> {
    let c = ctx(&s, Some(&actor), o, number).await?;
    c.require_write()?;
    let want_pull = match input.kind.as_deref() {
        Some("Issue") => false,
        Some("PullRequest") => true,
        Some(_) => return Err(ApiError::validation("ProjectV2Item", "type", "invalid")),
        None => return Err(ApiError::validation("ProjectV2Item", "type", "missing_field")),
    };
    let issue: IssueRow = match (input.id, &input.owner, &input.repo, input.number) {
        (Some(id), _, _, _) => {
            let row: IssueRow = sqlx::query_as("SELECT * FROM issues WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
            let repo = db::repo_by_id(&s.db, row.repo_id).await?;
            let owner = db::user_by_id(&s.db, repo.owner_id).await?;
            if db::permission(&s.db, Some(&actor), &repo, &owner).await? < Perm::Read {
                return Err(ApiError::not_found());
            }
            row
        }
        (None, Some(ow), Some(r), Some(n)) => {
            let rc = db::load_repo(&s.db, Some(&actor), ow, r).await?;
            crate::json::issue::issue_by_number(&s.db, rc.repo.id, n).await?
        }
        _ => return Err(ApiError::validation("ProjectV2Item", "id", "missing_field")),
    };
    if issue.is_pull != want_pull {
        return Err(ApiError::unprocessable(format!("The content is not a{}.", if want_pull { " pull request" } else { "n issue" })));
    }
    let item = svc::add_issue(&s, &actor, c.project.id, &issue).await?;
    let v = item_simple(&s, Some(&actor), &c, &item).await?;
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

async fn add_item_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, i64)>, Body(b): Body<AddItemInput>) -> ApiResult<Response> {
    add_item(s, u, Owner::Org(org), n, b).await
}
async fn add_item_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((l, n)): Path<(String, i64)>, Body(b): Body<AddItemInput>) -> ApiResult<Response> {
    add_item(s, u, Owner::Login(l), n, b).await
}

#[derive(Deserialize)]
struct FieldUpdate {
    id: Option<i64>,
    #[serde(default)]
    value: Value,
}

#[derive(Deserialize)]
struct UpdateItemInput {
    fields: Option<Vec<FieldUpdate>>,
}

async fn update_item(s: AppState, actor: User, o: Owner, number: i64, item_id: i64, input: UpdateItemInput) -> ApiResult<Json<Value>> {
    let c = ctx(&s, Some(&actor), o, number).await?;
    c.require_write()?;
    let item = load_item(&s, &c, item_id).await?;
    let updates = input.fields.ok_or_else(|| ApiError::validation("ProjectV2Item", "fields", "missing_field"))?;
    let mut touched = Vec::new();
    for u in updates {
        let fid = u.id.ok_or_else(|| ApiError::validation("ProjectV2Item", "fields", "missing_field"))?;
        let f = svc::field_by_id(&s, fid).await?.filter(|f| f.project_id == c.project.id).ok_or_else(ApiError::not_found)?;
        touched.push(f.id);
        let pid = c.project.id;
        if u.value.is_null() {
            if f.is_custom() {
                svc::clear_value(&s, &actor, pid, item.id, f.id).await?;
                continue;
            }
            return Err(ApiError::unprocessable(format!("The {} field can't be cleared.", f.name)));
        }
        let bad = || ApiError::unprocessable(format!("The value does not match the field's data type ({}).", rest_type(&f.data_type).unwrap_or("unknown")));
        let as_str = |v: &Value| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string()));
        match f.data_type.as_str() {
            "TEXT" => {
                svc::set_value(&s, &actor, pid, item.id, f.id, FieldValue::Text(u.value.as_str().ok_or_else(bad)?.to_string())).await?;
            }
            "NUMBER" => {
                let n = u.value.as_f64().or_else(|| u.value.as_str().and_then(|x| x.parse().ok())).ok_or_else(bad)?;
                svc::set_value(&s, &actor, pid, item.id, f.id, FieldValue::Number(n)).await?;
            }
            "DATE" => {
                let d = NaiveDate::parse_from_str(u.value.as_str().ok_or_else(bad)?.get(..10).unwrap_or(""), "%Y-%m-%d").map_err(|_| bad())?;
                svc::set_value(&s, &actor, pid, item.id, f.id, FieldValue::Date(d)).await?;
            }
            "SINGLE_SELECT" => {
                svc::set_value(&s, &actor, pid, item.id, f.id, FieldValue::SingleSelect(as_str(&u.value).ok_or_else(bad)?)).await?;
            }
            "ITERATION" => {
                svc::set_value(&s, &actor, pid, item.id, f.id, FieldValue::Iteration(as_str(&u.value).ok_or_else(bad)?)).await?;
            }
            "TITLE" if item.issue_id.is_none() => {
                svc::update_draft(&s, &actor, item.id, Some(u.value.as_str().ok_or_else(bad)?), None).await?;
            }
            _ => return Err(ApiError::unprocessable(format!("The field of type {} is currently not supported", rest_type(&f.data_type).unwrap_or("unknown")))),
        }
    }
    let item = load_item(&s, &c, item_id).await?;
    let v = items_json(&s, Some(&actor), &c, &[item], &touched).await?.pop().ok_or_else(ApiError::not_found)?;
    Ok(Json(v))
}

async fn update_item_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, i)): Path<(String, i64, i64)>, Body(b): Body<UpdateItemInput>) -> ApiResult<Json<Value>> {
    update_item(s, u, Owner::Org(org), n, i, b).await
}
async fn update_item_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((l, n, i)): Path<(String, i64, i64)>, Body(b): Body<UpdateItemInput>) -> ApiResult<Json<Value>> {
    update_item(s, u, Owner::Login(l), n, i, b).await
}

async fn delete_item(s: AppState, actor: User, o: Owner, number: i64, item_id: i64) -> ApiResult<Response> {
    let c = ctx(&s, Some(&actor), o, number).await?;
    c.require_write()?;
    svc::delete_item(&s, &actor, c.project.id, item_id).await?;
    Ok(no_content())
}

async fn delete_item_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, i)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    delete_item(s, u, Owner::Org(org), n, i).await
}
async fn delete_item_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((l, n, i)): Path<(String, i64, i64)>) -> ApiResult<Response> {
    delete_item(s, u, Owner::Login(l), n, i).await
}

// ---------------------------------------------------------------------------------------------
// Handlers: views

#[derive(Debug, Clone, sqlx::FromRow)]
struct ViewRow {
    id: i64,
    #[allow(dead_code)]
    project_id: i64,
    number: i64,
    name: String,
    layout: String,
    filter: Option<String>,
    visible_fields: Value,
    sort_by: Value,
    group_by: Value,
    vertical_group_by: Value,
    creator_id: Option<i64>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn view_json(s: &AppState, c: &Ctx, v: &ViewRow) -> Value {
    json!({
        "id": v.id,
        "number": v.number,
        "name": v.name,
        "layout": v.layout,
        "node_id": ids::encode("ProjectV2View", v.id),
        "project_url": c.url,
        "html_url": format!("{}/views/{}", c.html, v.number),
        "creator": user_json(s, v.creator_id, &c.owner).await,
        "created_at": crate::json::ts(v.created_at),
        "updated_at": crate::json::ts(v.updated_at),
        "filter": v.filter,
        "visible_fields": v.visible_fields,
        "sort_by": v.sort_by,
        "group_by": v.group_by,
        "vertical_group_by": v.vertical_group_by,
    })
}

#[derive(Deserialize)]
struct ViewInput {
    name: Option<String>,
    layout: Option<String>,
    filter: Option<String>,
    visible_fields: Option<Vec<i64>>,
    sort_by: Option<Vec<Vec<Value>>>,
    group_by: Option<Vec<i64>>,
    vertical_group_by: Option<Vec<i64>>,
}

async fn create_view(s: AppState, actor: User, o: Owner, number: i64, input: ViewInput) -> ApiResult<Response> {
    let c = ctx(&s, Some(&actor), o, number).await?;
    c.require_write()?;
    let name = input.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| ApiError::validation("ProjectV2View", "name", "missing_field"))?;
    let layout = match input.layout.as_deref() {
        Some(l @ ("table" | "board" | "roadmap")) => l.to_string(),
        Some(_) => return Err(ApiError::validation("ProjectV2View", "layout", "invalid")),
        None => return Err(ApiError::validation("ProjectV2View", "layout", "missing_field")),
    };
    let field_ids: Vec<i64> = svc::fields(&s, c.project.id).await?.iter().map(|f| f.id).collect();
    let check = |ids: &[i64], field: &str| -> ApiResult<()> {
        if ids.iter().all(|i| field_ids.contains(i)) { Ok(()) } else { Err(ApiError::validation("ProjectV2View", field, "invalid")) }
    };
    let visible = input.visible_fields.unwrap_or_default();
    if layout == "roadmap" && !visible.is_empty() {
        return Err(ApiError::unprocessable("visible_fields is not applicable to roadmap layout views."));
    }
    check(&visible, "visible_fields")?;
    let group = input.group_by.unwrap_or_default();
    let vgroup = input.vertical_group_by.unwrap_or_default();
    if group.len() > 1 || vgroup.len() > 1 {
        return Err(ApiError::unprocessable("Views can be grouped by a single field."));
    }
    check(&group, "group_by")?;
    check(&vgroup, "vertical_group_by")?;
    let mut sort = Vec::new();
    for pair in input.sort_by.unwrap_or_default() {
        let (Some(fid), Some(dir)) = (pair.first().and_then(|v| v.as_i64()), pair.get(1).and_then(|v| v.as_str())) else {
            return Err(ApiError::validation("ProjectV2View", "sort_by", "invalid"));
        };
        if pair.len() != 2 || !matches!(dir, "asc" | "desc") || !field_ids.contains(&fid) {
            return Err(ApiError::validation("ProjectV2View", "sort_by", "invalid"));
        }
        sort.push(json!([fid, dir]));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('projects_v2_views:' || $1::text, 0))").bind(c.project.id).execute(&mut *tx).await?;
    // View 1 is every project's implicit default view.
    let v: ViewRow = sqlx::query_as(
        "INSERT INTO projects_v2_views (project_id, number, name, layout, filter, visible_fields, sort_by, group_by, vertical_group_by, creator_id)
         VALUES ($1, (SELECT GREATEST(COALESCE(MAX(number), 1), 1) + 1 FROM projects_v2_views WHERE project_id = $1), $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING *",
    )
    .bind(c.project.id)
    .bind(name.trim())
    .bind(&layout)
    .bind(input.filter.filter(|f| !f.is_empty()))
    .bind(json!(visible))
    .bind(Value::Array(sort))
    .bind(json!(group))
    .bind(json!(vgroup))
    .bind(actor.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE projects_v2 SET updated_at = now() WHERE id = $1").bind(c.project.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(view_json(&s, &c, &v).await)).into_response())
}

async fn view_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): Path<(String, i64)>, Body(b): Body<ViewInput>) -> ApiResult<Response> {
    create_view(s, u, Owner::Org(org), n, b).await
}
async fn view_user(State(s): State<AppState>, AuthUser(u): AuthUser, Path((id, n)): Path<(String, i64)>, Body(b): Body<ViewInput>) -> ApiResult<Response> {
    create_view(s, u, Owner::Id(id), n, b).await
}

async fn view_items(s: AppState, uri: OriginalUri, viewer: Option<User>, o: Owner, number: i64, view_number: i64, p: CursorParams) -> ApiResult<Response> {
    let c = ctx(&s, viewer.as_ref(), o, number).await?;
    let view: Option<ViewRow> = sqlx::query_as("SELECT * FROM projects_v2_views WHERE project_id = $1 AND number = $2")
        .bind(c.project.id)
        .bind(view_number)
        .fetch_optional(&s.db)
        .await?;
    if view.is_none() && view_number != 1 {
        return Err(ApiError::not_found());
    }
    let items = svc::items(&s, c.project.id).await?;
    let contents = load_contents(&s, viewer.as_ref(), &items).await?;
    let filter = view.as_ref().and_then(|v| v.filter.clone());
    let items = filter_items(items, &contents, filter.as_deref(), true);
    let mut fields = requested_fields(p.fields.as_deref());
    if fields.is_empty() {
        if let Some(v) = &view {
            fields = v.visible_fields.as_array().into_iter().flatten().filter_map(|x| x.as_i64()).collect();
        }
    }
    let rendered = items_json(&s, viewer.as_ref(), &c, &items, &fields).await?;
    cursor_page(&s, &uri, &p, rendered)
}

async fn view_items_org(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path((org, n, vn)): Path<(String, i64, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    view_items(s, uri, v.0, Owner::Org(org), n, vn, p).await
}
async fn view_items_user(State(s): State<AppState>, uri: OriginalUri, v: MaybeUser, Path((u, n, vn)): Path<(String, i64, i64)>, Query(p): Query<CursorParams>) -> ApiResult<Response> {
    view_items(s, uri, v.0, Owner::Login(u), n, vn, p).await
}
