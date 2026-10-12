//! Gists, stored in Postgres (no git backing: `git_pull_url` points at a URL that 404s).
use std::collections::{BTreeMap, HashMap};

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/gists", get(list_mine).post(create))
        .route("/gists/public", get(list_public))
        .route("/gists/starred", get(list_starred))
        .route("/users/{username}/gists", get(list_for_user))
        .route("/gists/{gist_id}", get(get_one).patch(update).delete(delete))
        .route("/gists/{gist_id}/star", get(is_starred).put(star).delete(unstar))
        .route("/gists/{gist_id}/forks", get(list_forks).post(fork))
        .route("/gists/{gist_id}/commits", get(list_commits))
        .route("/gists/{gist_id}/comments", get(list_comments))
        .route("/gists/{gist_id}/{sha}", get(get_revision))
}

/// Raw file downloads (outside /api): `/gist/{login}/{id}/raw/[{version}/]{filename}`.
pub fn raw_routes() -> Router<AppState> {
    Router::new().route("/gist/{login}/{gist_id}/raw/{*rest}", get(raw))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GistRow {
    pub id: i64,
    pub gist_id: String,
    pub owner_id: i64,
    pub description: Option<String>,
    pub public: bool,
    pub fork_of: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HistoryRow {
    pub id: i64,
    pub gist_id: i64,
    pub version: String,
    pub user_id: Option<i64>,
    pub additions: i64,
    pub deletions: i64,
    pub files: Value,
    pub committed_at: DateTime<Utc>,
}

fn language(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "rs" => "Rust",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" => "TypeScript",
        "py" => "Python",
        "go" => "Go",
        "rb" => "Ruby",
        "md" | "markdown" => "Markdown",
        "txt" => "Text",
        "json" => "JSON",
        "yml" | "yaml" => "YAML",
        "toml" => "TOML",
        "sh" | "bash" => "Shell",
        "c" | "h" => "C",
        "cpp" | "cc" | "hpp" => "C++",
        "java" => "Java",
        "kt" => "Kotlin",
        "swift" => "Swift",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        "php" => "PHP",
        "cs" => "C#",
        "lua" => "Lua",
        "ex" | "exs" => "Elixir",
        "hs" => "Haskell",
        "diff" | "patch" => "Diff",
        _ => return None,
    })
}

fn mime_of(name: &str) -> String {
    if language(name) == Some("Text") || !name.contains('.') {
        return "text/plain".into();
    }
    mime_guess::from_path(name).first_or_text_plain().essence_str().to_string()
}

const TRUNCATE_AT: usize = 1024 * 1024;

fn file_json(s: &AppState, owner: &User, g: &GistRow, version: &str, name: &str, content: &str, with_content: bool) -> Value {
    let mut v = json!({
        "filename": name,
        "type": mime_of(name),
        "language": language(name),
        "raw_url": format!("{}/gist/{}/{}/raw/{version}/{}", s.base_url(), owner.login, g.gist_id, crate::json::release::seg(name)),
        "size": content.len(),
    });
    if with_content {
        let truncated = content.len() > TRUNCATE_AT;
        let mut c = content;
        if truncated {
            let mut cut = TRUNCATE_AT;
            while !content.is_char_boundary(cut) {
                cut -= 1;
            }
            c = &content[..cut];
        }
        v["truncated"] = json!(truncated);
        v["content"] = json!(c);
        v["encoding"] = json!("utf-8");
    }
    v
}

fn history_json(s: &AppState, g: &GistRow, h: &HistoryRow, user: Option<&User>) -> Value {
    json!({
        "user": user.map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).unwrap_or(Value::Null),
        "version": h.version,
        "committed_at": crate::json::ts(h.committed_at),
        "change_status": { "total": h.additions + h.deletions, "additions": h.additions, "deletions": h.deletions },
        "url": format!("{}/gists/{}/{}", s.api_url(), g.gist_id, h.version),
    })
}

/// Base gist JSON (shared by list and detail views).
fn base_json(s: &AppState, g: &GistRow, owner: &User, files: &BTreeMap<String, String>, version: &str, with_content: bool) -> Value {
    let api = s.api_url();
    let url = format!("{api}/gists/{}", g.gist_id);
    let mut fm = Map::new();
    for (name, content) in files {
        fm.insert(name.clone(), file_json(s, owner, g, version, name, content, with_content));
    }
    json!({
        "url": url,
        "forks_url": format!("{url}/forks"),
        "commits_url": format!("{url}/commits"),
        "id": g.gist_id,
        "node_id": crate::ids::encode("Gist", g.id),
        "git_pull_url": format!("{}/gist/{}.git", s.base_url(), g.gist_id),
        "git_push_url": format!("{}/gist/{}.git", s.base_url(), g.gist_id),
        "html_url": format!("{}/gist/{}/{}", s.base_url(), owner.login, g.gist_id),
        "files": fm,
        "public": g.public,
        "created_at": crate::json::ts(g.created_at),
        "updated_at": crate::json::ts(g.updated_at),
        "description": g.description,
        "comments": 0,
        "comments_enabled": true,
        "user": null,
        "comments_url": format!("{url}/comments"),
        "owner": crate::json::user::simple(&api, s.base_url(), owner),
        "truncated": false,
    })
}

async fn files_of(s: &AppState, ids: &[i64]) -> ApiResult<HashMap<i64, BTreeMap<String, String>>> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as("SELECT gist_id, filename, content FROM gist_files WHERE gist_id = ANY($1)")
        .bind(ids)
        .fetch_all(&s.db)
        .await?;
    let mut out: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
    for (g, f, c) in rows {
        out.entry(g).or_default().insert(f, c);
    }
    Ok(out)
}

async fn latest_versions(s: &AppState, ids: &[i64]) -> ApiResult<HashMap<i64, String>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (gist_id) gist_id, version FROM gist_history WHERE gist_id = ANY($1) ORDER BY gist_id, id DESC",
    )
    .bind(ids)
    .fetch_all(&s.db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// List view (`base-gist`, no file contents), batched.
pub async fn gists_json(s: &AppState, rows: &[GistRow]) -> ApiResult<Vec<Value>> {
    let ids: Vec<i64> = rows.iter().map(|g| g.id).collect();
    let files = files_of(s, &ids).await?;
    let versions = latest_versions(s, &ids).await?;
    let owner_ids: Vec<i64> = rows.iter().map(|g| g.owner_id).collect();
    let owners = crate::json::issue::users_by_ids(&s.db, &owner_ids).await?;
    let empty = BTreeMap::new();
    Ok(rows
        .iter()
        .filter_map(|g| {
            let o = owners.get(&g.owner_id)?;
            let v = versions.get(&g.id).cloned().unwrap_or_default();
            Some(base_json(s, g, o, files.get(&g.id).unwrap_or(&empty), &v, false))
        })
        .collect())
}

/// Detail view (`gist-simple`: contents, forks, history, fork_of).
pub async fn gist_detail_json(s: &AppState, g: &GistRow) -> ApiResult<Value> {
    let owner = db::user_by_id(&s.db, g.owner_id).await?;
    let files = files_of(s, &[g.id]).await?.remove(&g.id).unwrap_or_default();
    let history: Vec<HistoryRow> = sqlx::query_as("SELECT * FROM gist_history WHERE gist_id = $1 ORDER BY id DESC LIMIT 100")
        .bind(g.id)
        .fetch_all(&s.db)
        .await?;
    let uids: Vec<i64> = history.iter().filter_map(|h| h.user_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &uids).await?;
    let version = history.first().map(|h| h.version.clone()).unwrap_or_default();
    let mut v = base_json(s, g, &owner, &files, &version, true);
    v["history"] = Value::Array(history.iter().map(|h| history_json(s, g, h, h.user_id.and_then(|u| users.get(&u)))).collect());
    let forks: Vec<GistRow> = sqlx::query_as("SELECT * FROM gists WHERE fork_of = $1 ORDER BY id LIMIT 100").bind(g.id).fetch_all(&s.db).await?;
    let fork_owner_ids: Vec<i64> = forks.iter().map(|f| f.owner_id).collect();
    let fork_owners = crate::json::issue::users_by_ids(&s.db, &fork_owner_ids).await?;
    v["forks"] = Value::Array(
        forks
            .iter()
            .filter_map(|f| {
                let o = fork_owners.get(&f.owner_id)?;
                let (followers, following) = (0, 0);
                Some(json!({
                    "id": f.gist_id,
                    "url": format!("{}/gists/{}", s.api_url(), f.gist_id),
                    "user": crate::json::user::public(&s.api_url(), s.base_url(), o, 0, followers, following),
                    "created_at": crate::json::ts(f.created_at),
                    "updated_at": crate::json::ts(f.updated_at),
                }))
            })
            .collect(),
    );
    v["fork_of"] = Value::Null;
    if let Some(parent) = g.fork_of {
        if let Ok(p) = load_gist_by_pk(s, parent).await {
            let mut pj = gists_json(s, std::slice::from_ref(&p)).await?;
            if let Some(x) = pj.pop() {
                v["fork_of"] = x;
            }
        }
    }
    Ok(v)
}

async fn load_gist_by_pk(s: &AppState, id: i64) -> ApiResult<GistRow> {
    sqlx::query_as("SELECT * FROM gists WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

pub async fn load_gist(s: &AppState, gist_id: &str) -> ApiResult<GistRow> {
    sqlx::query_as("SELECT * FROM gists WHERE gist_id = $1").bind(gist_id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)
}

fn line_diff(old: &str, new: &str) -> (i64, i64) {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for l in old.lines() {
        *counts.entry(l).or_default() += 1;
    }
    let mut additions = 0;
    for l in new.lines() {
        match counts.get_mut(l) {
            Some(n) if *n > 0 => *n -= 1,
            _ => additions += 1,
        }
    }
    let deletions = counts.values().sum();
    (additions, deletions)
}

fn new_version() -> String {
    hex::encode(rand::random::<[u8; 20]>())
}

async fn record_history(tx: &mut sqlx::PgConnection, gist_pk: i64, user_id: i64, old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> ApiResult<()> {
    let (mut add, mut del) = (0, 0);
    for (name, content) in new {
        let (a, d) = line_diff(old.get(name).map(String::as_str).unwrap_or(""), content);
        add += a;
        del += d;
    }
    for (name, content) in old {
        if !new.contains_key(name) {
            del += content.lines().count() as i64;
        }
    }
    sqlx::query("INSERT INTO gist_history (gist_id, version, user_id, additions, deletions, files) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(gist_pk)
        .bind(new_version())
        .bind(user_id)
        .bind(add)
        .bind(del)
        .bind(serde_json::to_value(new).unwrap_or_default())
        .execute(&mut *tx)
        .await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateGist {
    pub description: Option<String>,
    pub files: Option<HashMap<String, Option<FileInput>>>,
    pub public: Option<Value>,
}

#[derive(Deserialize, Clone)]
pub struct FileInput {
    pub content: Option<String>,
    pub filename: Option<String>,
}

fn files_invalid() -> ApiError {
    ApiError::validation("Gist", "files", "missing_field")
}

pub async fn create_gist(s: &AppState, owner: &User, req: CreateGist) -> ApiResult<GistRow> {
    let input = req.files.unwrap_or_default();
    let mut files = BTreeMap::new();
    for (name, f) in input {
        let content = f.and_then(|f| f.content).unwrap_or_default();
        if name.trim().is_empty() || content.is_empty() || name.contains('/') {
            return Err(ApiError::validation("Gist", "files", "missing_field"));
        }
        files.insert(name, content);
    }
    if files.is_empty() {
        return Err(files_invalid());
    }
    let public = match req.public {
        Some(Value::Bool(b)) => b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    };
    let mut tx = s.db.begin().await?;
    let g: GistRow = sqlx::query_as("INSERT INTO gists (gist_id, owner_id, description, public) VALUES ($1,$2,$3,$4) RETURNING *")
        .bind(hex::encode(rand::random::<[u8; 16]>()))
        .bind(owner.id)
        .bind(req.description)
        .bind(public)
        .fetch_one(&mut *tx)
        .await?;
    for (name, content) in &files {
        sqlx::query("INSERT INTO gist_files (gist_id, filename, content) VALUES ($1,$2,$3)")
            .bind(g.id)
            .bind(name)
            .bind(content)
            .execute(&mut *tx)
            .await?;
    }
    record_history(&mut tx, g.id, owner.id, &BTreeMap::new(), &files).await?;
    tx.commit().await?;
    Ok(g)
}

#[derive(Deserialize)]
pub struct UpdateGist {
    pub description: Option<String>,
    pub files: Option<HashMap<String, Option<FileInput>>>,
}

pub async fn update_gist(s: &AppState, g: &GistRow, actor: &User, req: UpdateGist) -> ApiResult<GistRow> {
    if g.owner_id != actor.id {
        return Err(ApiError::not_found());
    }
    let mut tx = s.db.begin().await?;
    let old: BTreeMap<String, String> = sqlx::query_as::<_, (String, String)>("SELECT filename, content FROM gist_files WHERE gist_id = $1 FOR UPDATE")
        .bind(g.id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
    let mut new = old.clone();
    for (name, f) in req.files.unwrap_or_default() {
        match f {
            None => {
                new.remove(&name);
            }
            Some(f) => {
                let content = f.content.clone().or_else(|| new.get(&name).cloned());
                let target = f.filename.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| name.clone());
                match content {
                    Some(c) if !c.is_empty() => {
                        new.remove(&name);
                        new.insert(target, c);
                    }
                    Some(_) => {
                        new.remove(&name);
                    }
                    None => return Err(ApiError::validation("Gist", "files", "missing_field")),
                }
            }
        }
    }
    if new.is_empty() {
        return Err(files_invalid());
    }
    if new != old {
        sqlx::query("DELETE FROM gist_files WHERE gist_id = $1").bind(g.id).execute(&mut *tx).await?;
        for (name, content) in &new {
            sqlx::query("INSERT INTO gist_files (gist_id, filename, content) VALUES ($1,$2,$3)")
                .bind(g.id)
                .bind(name)
                .bind(content)
                .execute(&mut *tx)
                .await?;
        }
        record_history(&mut tx, g.id, actor.id, &old, &new).await?;
    }
    let g: GistRow = sqlx::query_as("UPDATE gists SET description = COALESCE($2, description), updated_at = now() WHERE id = $1 RETURNING *")
        .bind(g.id)
        .bind(req.description)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(g)
}

#[derive(Deserialize)]
struct ListQuery {
    since: Option<DateTime<Utc>>,
    page: Option<i64>,
    per_page: Option<i64>,
}

impl ListQuery {
    fn pages(&self) -> PageParams {
        PageParams { page: self.page, per_page: self.per_page }
    }
}

async fn list_where(s: &AppState, uri: &OriginalUri, cond: &str, id: i64, q: &ListQuery) -> ApiResult<Paged> {
    let p = q.pages();
    let sql = format!(
        "SELECT g.* FROM gists g WHERE ({cond}) AND ($2::timestamptz IS NULL OR g.updated_at >= $2) ORDER BY g.updated_at DESC, g.id DESC LIMIT $3 OFFSET $4"
    );
    let rows: Vec<GistRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).bind(q.since).bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let items = gists_json(s, &rows).await?;
    Ok(Paged::new(uri, s, p, None, items))
}

async fn list_mine(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    match u {
        Some(u) => list_where(&s, &uri, "g.owner_id = $1", u.id, &q).await,
        None => list_where(&s, &uri, "g.public AND $1 = $1", 0, &q).await,
    }
}

async fn list_public(State(s): State<AppState>, uri: OriginalUri, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    list_where(&s, &uri, "g.public AND $1 = $1", 0, &q).await
}

async fn list_starred(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    list_where(&s, &uri, "g.id IN (SELECT gist_id FROM gist_stars WHERE user_id = $1)", u.id, &q).await
}

async fn list_for_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(v): MaybeUser,
    Path(login): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    if v.as_ref().is_some_and(|v| v.id == u.id) {
        list_where(&s, &uri, "g.owner_id = $1", u.id, &q).await
    } else {
        list_where(&s, &uri, "g.owner_id = $1 AND g.public", u.id, &q).await
    }
}

async fn create(State(s): State<AppState>, AuthUser(u): AuthUser, Json(req): Json<CreateGist>) -> ApiResult<Response> {
    let g = create_gist(&s, &u, req).await?;
    Ok(created(gist_detail_json(&s, &g).await?))
}

async fn get_one(State(s): State<AppState>, _u: MaybeUser, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let g = load_gist(&s, &id).await?;
    Ok(Json(gist_detail_json(&s, &g).await?))
}

async fn update(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>, Json(req): Json<UpdateGist>) -> ApiResult<Json<Value>> {
    let g = load_gist(&s, &id).await?;
    let g = update_gist(&s, &g, &u, req).await?;
    Ok(Json(gist_detail_json(&s, &g).await?))
}

async fn delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    if g.owner_id != u.id {
        return Err(ApiError::not_found());
    }
    sqlx::query("DELETE FROM gists WHERE id = $1").bind(g.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn is_starred(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    let yes: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM gist_stars WHERE user_id = $1 AND gist_id = $2)")
        .bind(u.id)
        .bind(g.id)
        .fetch_one(&s.db)
        .await?;
    if yes {
        Ok(no_content())
    } else {
        Err(ApiError::not_found())
    }
}

async fn star(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    sqlx::query("INSERT INTO gist_stars (user_id, gist_id) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(u.id).bind(g.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn unstar(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    sqlx::query("DELETE FROM gist_stars WHERE user_id = $1 AND gist_id = $2").bind(u.id).bind(g.id).execute(&s.db).await?;
    Ok(no_content())
}

async fn list_forks(State(s): State<AppState>, uri: OriginalUri, Path(id): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let g = load_gist(&s, &id).await?;
    let rows: Vec<GistRow> = sqlx::query_as("SELECT * FROM gists WHERE fork_of = $1 ORDER BY id LIMIT $2 OFFSET $3")
        .bind(g.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = gists_json(&s, &rows).await?;
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn fork(State(s): State<AppState>, AuthUser(u): AuthUser, Path(id): Path<String>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    if g.owner_id == u.id {
        return Err(ApiError::validation("Gist", "forks", "custom"));
    }
    let files = files_of(&s, &[g.id]).await?.remove(&g.id).unwrap_or_default();
    let mut tx = s.db.begin().await?;
    let f: GistRow = sqlx::query_as("INSERT INTO gists (gist_id, owner_id, description, public, fork_of) VALUES ($1,$2,$3,$4,$5) RETURNING *")
        .bind(hex::encode(rand::random::<[u8; 16]>()))
        .bind(u.id)
        .bind(&g.description)
        .bind(g.public)
        .bind(g.id)
        .fetch_one(&mut *tx)
        .await?;
    for (name, content) in &files {
        sqlx::query("INSERT INTO gist_files (gist_id, filename, content) VALUES ($1,$2,$3)").bind(f.id).bind(name).bind(content).execute(&mut *tx).await?;
    }
    record_history(&mut tx, f.id, u.id, &BTreeMap::new(), &files).await?;
    tx.commit().await?;
    Ok(created(gists_json(&s, std::slice::from_ref(&f)).await?.remove(0)))
}

async fn list_commits(State(s): State<AppState>, uri: OriginalUri, Path(id): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let g = load_gist(&s, &id).await?;
    let rows: Vec<HistoryRow> = sqlx::query_as("SELECT * FROM gist_history WHERE gist_id = $1 ORDER BY id DESC LIMIT $2 OFFSET $3")
        .bind(g.id)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let uids: Vec<i64> = rows.iter().filter_map(|h| h.user_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &uids).await?;
    let items = rows.iter().map(|h| history_json(&s, &g, h, h.user_id.and_then(|u| users.get(&u)))).collect();
    Ok(Paged::new(&uri, &s, p, None, items))
}

async fn list_comments(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    load_gist(&s, &id).await?;
    Ok(Json(json!([])))
}

async fn get_revision(State(s): State<AppState>, Path((id, sha)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let g = load_gist(&s, &id).await?;
    let h: HistoryRow = sqlx::query_as("SELECT * FROM gist_history WHERE gist_id = $1 AND version = $2")
        .bind(g.id)
        .bind(&sha)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let owner = db::user_by_id(&s.db, g.owner_id).await?;
    let files: BTreeMap<String, String> = serde_json::from_value(h.files.clone()).unwrap_or_default();
    let mut v = base_json(&s, &g, &owner, &files, &h.version, true);
    v["history"] = json!([history_json(&s, &g, &h, None)]);
    Ok(Json(v))
}

async fn raw(State(s): State<AppState>, Path((_login, id, rest)): Path<(String, String, String)>) -> ApiResult<Response> {
    let g = load_gist(&s, &id).await?;
    let (version, name) = match rest.split_once('/') {
        Some((v, n)) => (Some(v.to_string()), n.to_string()),
        None => (None, rest.clone()),
    };
    let content: Option<String> = match version {
        Some(v) => {
            let files: Option<Value> = sqlx::query_scalar("SELECT files FROM gist_history WHERE gist_id = $1 AND version = $2")
                .bind(g.id)
                .bind(&v)
                .fetch_optional(&s.db)
                .await?;
            files.and_then(|f| f.get(&name).and_then(Value::as_str).map(str::to_string))
        }
        None => sqlx::query_scalar("SELECT content FROM gist_files WHERE gist_id = $1 AND filename = $2")
            .bind(g.id)
            .bind(&name)
            .fetch_optional(&s.db)
            .await?,
    };
    let content = content.ok_or_else(ApiError::not_found)?;
    let mut res = content.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    Ok(res)
}
