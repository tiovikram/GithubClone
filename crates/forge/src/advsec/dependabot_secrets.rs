//! Dependabot secrets (repository and organization) and the organization's Dependabot
//! repository access list.
//!
//! Secrets work like Actions secrets (`api::actions_secrets`): clients seal values with
//! libsodium `crypto_box_seal` for the owner's Dependabot public key; forge keeps the X25519
//! private key sealed with its master key (`api::actions_settings::crypto`, owner kinds
//! `dependabot-repo` / `dependabot-org`, distinct from the Actions keys), rejects values that do
//! not open, and decrypts them only through [`secrets_for_update`].
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::actions_settings::crypto;
use crate::api::{no_content, PageParams};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    let repo = "/repos/{owner}/{repo}/dependabot/secrets";
    let org = "/orgs/{org}/dependabot/secrets";
    Router::new()
        .route(repo, get(repo_list))
        .route(&format!("{repo}/public-key"), get(repo_key))
        .route(&format!("{repo}/{{secret_name}}"), get(repo_get).put(repo_put).delete(repo_delete))
        .route(org, get(org_list))
        .route(&format!("{org}/public-key"), get(org_key))
        .route(&format!("{org}/{{secret_name}}"), get(org_get).put(org_put).delete(org_delete))
        .route(&format!("{org}/{{secret_name}}/repositories"), get(org_repos).put(org_set_repos))
        .route(&format!("{org}/{{secret_name}}/repositories/{{repository_id}}"), put(org_add_repo).delete(org_remove_repo))
        .route("/orgs/{org}/dependabot/repository-access", get(access_get).patch(access_update))
        .route("/orgs/{org}/dependabot/repository-access/default-level", put(access_default))
}

#[derive(Clone)]
enum Owner {
    Repo(Box<RepoCtx>),
    Org(User),
}

impl Owner {
    fn key(&self) -> (&'static str, i64) {
        match self {
            Owner::Repo(c) => ("dependabot-repo", c.repo.id),
            Owner::Org(o) => ("dependabot-org", o.id),
        }
    }
    fn repo_id(&self) -> Option<i64> {
        match self {
            Owner::Repo(c) => Some(c.repo.id),
            Owner::Org(_) => None,
        }
    }
    fn org_id(&self) -> Option<i64> {
        match self {
            Owner::Org(o) => Some(o.id),
            Owner::Repo(_) => None,
        }
    }
    fn base(&self, s: &AppState) -> String {
        match self {
            Owner::Repo(c) => format!("{}/repos/{}/dependabot/secrets", s.api_url(), c.full_name()),
            Owner::Org(o) => format!("{}/orgs/{}/dependabot/secrets", s.api_url(), o.login),
        }
    }
}

async fn repo_owner(s: &AppState, u: &User, o: &str, r: &str) -> ApiResult<Owner> {
    let ctx = db::load_repo(&s.db, Some(u), o, r).await?;
    ctx.require(Perm::Admin)?;
    Ok(Owner::Repo(Box::new(ctx)))
}

async fn org_owner(s: &AppState, u: &User, org: &str) -> ApiResult<Owner> {
    let o = crate::orgs::org_by_login(s, org).await?;
    crate::orgs::require_owner(s, &o, u).await?;
    Ok(Owner::Org(o))
}

#[derive(sqlx::FromRow)]
struct SecretRow {
    id: i64,
    name: String,
    visibility: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn secret_json(s: &AppState, owner: &Owner, r: &SecretRow) -> Value {
    let mut v = json!({ "name": r.name, "created_at": crate::json::ts(r.created_at), "updated_at": crate::json::ts(r.updated_at) });
    if let Owner::Org(_) = owner {
        let vis = r.visibility.clone().unwrap_or_else(|| "private".into());
        if vis == "selected" {
            v["selected_repositories_url"] = json!(format!("{}/{}/repositories", owner.base(s), r.name));
        }
        v["visibility"] = json!(vis);
    }
    v
}

const OWNER: &str = "COALESCE(repo_id, 0) = COALESCE($1, 0) AND COALESCE(org_id, 0) = COALESCE($2, 0)";

async fn get_row(s: &AppState, owner: &Owner, name: &str) -> ApiResult<SecretRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, name, visibility, created_at, updated_at FROM dependabot_secrets WHERE {OWNER} AND upper(name) = upper($3)"
    )))
    .bind(owner.repo_id())
    .bind(owner.org_id())
    .bind(name)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(ApiError::not_found)
}

fn valid_name(name: &str) -> ApiResult<()> {
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && !name.to_ascii_uppercase().starts_with("GITHUB_");
    if ok { Ok(()) } else { Err(ApiError::validation("Secret", "name", "invalid")) }
}

#[derive(Deserialize)]
struct PutBody {
    encrypted_value: Option<String>,
    key_id: Option<String>,
    visibility: Option<String>,
    selected_repository_ids: Option<Vec<Value>>,
}

async fn put_secret(s: &AppState, owner: &Owner, name: &str, b: PutBody) -> ApiResult<bool> {
    valid_name(name)?;
    let value = b.encrypted_value.filter(|v| !v.is_empty()).ok_or_else(|| ApiError::validation("Secret", "encrypted_value", "missing_field"))?;
    let key_id = b.key_id.filter(|v| !v.is_empty()).ok_or_else(|| ApiError::validation("Secret", "key_id", "missing_field"))?;
    let (kind, oid) = owner.key();
    let (current, _) = crypto::public_key(s, kind, oid).await?;
    if key_id != current {
        return Err(ApiError::validation("Secret", "key_id", "invalid"));
    }
    match crypto::private_key(s, kind, oid).await? {
        Some((_, sk)) if crypto::open_sealed(&sk, &value).is_some() => {}
        _ => return Err(ApiError::validation("Secret", "encrypted_value", "invalid")),
    }
    let visibility = match owner {
        Owner::Org(_) => match b.visibility.as_deref() {
            Some(v @ ("all" | "private" | "selected")) => Some(v.to_string()),
            Some(_) => return Err(ApiError::validation("Secret", "visibility", "invalid")),
            None => return Err(ApiError::validation("Secret", "visibility", "missing_field")),
        },
        Owner::Repo(_) => None,
    };
    let selected = match (owner, &b.selected_repository_ids) {
        (Owner::Org(o), Some(ids)) => {
            let ids: Vec<i64> = ids.iter().filter_map(|v| v.as_i64().or_else(|| v.as_str().and_then(|x| x.parse().ok()))).collect();
            Some(crate::api::actions_secrets::org_repo_ids(s, o, &ids).await?)
        }
        _ => None,
    };
    let (id, created): (i64, bool) = sqlx::query_as(
        "INSERT INTO dependabot_secrets (repo_id, org_id, name, encrypted_value, key_id, visibility) VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (COALESCE(repo_id, 0), COALESCE(org_id, 0), name) DO UPDATE
            SET encrypted_value = EXCLUDED.encrypted_value, key_id = EXCLUDED.key_id,
                visibility = COALESCE(EXCLUDED.visibility, dependabot_secrets.visibility), updated_at = now()
         RETURNING id, (xmax = 0)",
    )
    .bind(owner.repo_id())
    .bind(owner.org_id())
    .bind(name.to_ascii_uppercase())
    .bind(&value)
    .bind(&key_id)
    .bind(&visibility)
    .fetch_one(&s.db)
    .await?;
    if let Some(ids) = selected {
        replace_selected(s, id, &ids).await?;
    }
    Ok(created)
}

async fn replace_selected(s: &AppState, id: i64, ids: &[i64]) -> ApiResult<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("DELETE FROM dependabot_secret_repos WHERE secret_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO dependabot_secret_repos (secret_id, repo_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Decrypted Dependabot secrets available to a repository's dependency updates: repository
/// secrets override organization secrets visible to it (`all`, `private` for non-public
/// repositories, or `selected`).
pub async fn secrets_for_update(s: &AppState, repo_id: i64) -> ApiResult<Vec<(String, String)>> {
    let repo = db::repo_by_id(&s.db, repo_id).await?;
    let mut out: Vec<(String, String)> = Vec::new();
    let org_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT i.name, i.encrypted_value FROM dependabot_secrets i JOIN repositories r ON r.id = $1
          WHERE i.org_id = r.owner_id AND (COALESCE(i.visibility, 'private') = 'all'
             OR (COALESCE(i.visibility, 'private') = 'private' AND r.visibility <> 'public')
             OR (i.visibility = 'selected' AND EXISTS(SELECT 1 FROM dependabot_secret_repos x WHERE x.secret_id = i.id AND x.repo_id = r.id)))",
    )
    .bind(repo_id)
    .fetch_all(&s.db)
    .await?;
    if let Some((_, sk)) = crypto::private_key(s, "dependabot-org", repo.owner_id).await? {
        out.extend(org_rows.into_iter().filter_map(|(n, v)| crypto::open_sealed(&sk, &v).map(|p| (n, p))));
    }
    let repo_rows: Vec<(String, String)> = sqlx::query_as("SELECT name, encrypted_value FROM dependabot_secrets WHERE repo_id = $1").bind(repo_id).fetch_all(&s.db).await?;
    if let Some((_, sk)) = crypto::private_key(s, "dependabot-repo", repo_id).await? {
        for (n, v) in repo_rows {
            if let Some(p) = crypto::open_sealed(&sk, &v) {
                out.retain(|(x, _)| x != &n);
                out.push((n, p));
            }
        }
    }
    Ok(out)
}

fn envelope(s: &AppState, uri: &OriginalUri, p: PageParams, key: &str, items: Vec<Value>, total: i64) -> Response {
    let links = crate::api::Paged::new(uri, s, p, Some(total), vec![]).into_response();
    let mut res = (StatusCode::OK, Json(json!({ "total_count": total, key: items }))).into_response();
    if let Some(l) = links.headers().get(axum::http::header::LINK) {
        res.headers_mut().insert(axum::http::header::LINK, l.clone());
    }
    res
}

async fn list_resp(s: &AppState, owner: &Owner, uri: &OriginalUri, p: PageParams) -> ApiResult<Response> {
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM dependabot_secrets WHERE {OWNER}")))
        .bind(owner.repo_id())
        .bind(owner.org_id())
        .fetch_one(&s.db)
        .await?;
    let rows: Vec<SecretRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, name, visibility, created_at, updated_at FROM dependabot_secrets WHERE {OWNER} ORDER BY name LIMIT $3 OFFSET $4"
    )))
    .bind(owner.repo_id())
    .bind(owner.org_id())
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let items = rows.iter().map(|r| secret_json(s, owner, r)).collect();
    Ok(envelope(s, uri, p, "secrets", items, total))
}

async fn key_json(s: &AppState, owner: &Owner) -> ApiResult<Json<Value>> {
    let (kind, id) = owner.key();
    let (key_id, key) = crypto::public_key(s, kind, id).await?;
    Ok(Json(json!({ "key_id": key_id, "key": key })))
}

async fn put_resp(s: &AppState, owner: &Owner, name: &str, b: PutBody) -> ApiResult<Response> {
    if put_secret(s, owner, name, b).await? {
        Ok((StatusCode::CREATED, Json(json!({}))).into_response())
    } else {
        Ok(no_content())
    }
}

async fn delete_resp(s: &AppState, owner: &Owner, name: &str) -> ApiResult<Response> {
    let n = sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM dependabot_secrets WHERE {OWNER} AND upper(name) = upper($3)")))
        .bind(owner.repo_id())
        .bind(owner.org_id())
        .bind(name)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(no_content())
}

type RP = Path<(String, String)>;
type RNP = Path<(String, String, String)>;

async fn repo_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((o, r)): RP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    list_resp(&s, &repo_owner(&s, &u, &o, &r).await?, &uri, p).await
}
async fn repo_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r)): RP) -> ApiResult<Json<Value>> {
    key_json(&s, &repo_owner(&s, &u, &o, &r).await?).await
}
async fn repo_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RNP) -> ApiResult<Json<Value>> {
    let ow = repo_owner(&s, &u, &o, &r).await?;
    Ok(Json(secret_json(&s, &ow, &get_row(&s, &ow, &n).await?)))
}
async fn repo_put(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RNP, Json(b): Json<PutBody>) -> ApiResult<Response> {
    put_resp(&s, &repo_owner(&s, &u, &o, &r).await?, &n, b).await
}
async fn repo_delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, n)): RNP) -> ApiResult<Response> {
    delete_resp(&s, &repo_owner(&s, &u, &o, &r).await?, &n).await
}
async fn org_list(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Response> {
    list_resp(&s, &org_owner(&s, &u, &org).await?, &uri, p).await
}
async fn org_key(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>) -> ApiResult<Json<Value>> {
    key_json(&s, &org_owner(&s, &u, &org).await?).await
}
async fn org_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): RP) -> ApiResult<Json<Value>> {
    let ow = org_owner(&s, &u, &org).await?;
    Ok(Json(secret_json(&s, &ow, &get_row(&s, &ow, &n).await?)))
}
async fn org_put(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): RP, Json(b): Json<PutBody>) -> ApiResult<Response> {
    put_resp(&s, &org_owner(&s, &u, &org).await?, &n, b).await
}
async fn org_delete(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): RP) -> ApiResult<Response> {
    delete_resp(&s, &org_owner(&s, &u, &org).await?, &n).await
}

async fn selected_row(s: &AppState, ow: &Owner, name: &str) -> ApiResult<SecretRow> {
    let row = get_row(s, ow, name).await?;
    if row.visibility.as_deref() != Some("selected") {
        return Err(ApiError::conflict("The visibility must be set to 'selected' to manage the selected repositories."));
    }
    Ok(row)
}

async fn org_repos(State(s): State<AppState>, AuthUser(u): AuthUser, uri: OriginalUri, Path((org, n)): RP, Query(p): Query<PageParams>) -> ApiResult<Response> {
    let ow = org_owner(&s, &u, &org).await?;
    let Owner::Org(o) = &ow else { unreachable!() };
    let row = get_row(&s, &ow, &n).await?;
    let ids: Vec<i64> = if row.visibility.as_deref() == Some("selected") {
        sqlx::query_scalar("SELECT repo_id FROM dependabot_secret_repos WHERE secret_id = $1 ORDER BY repo_id").bind(row.id).fetch_all(&s.db).await?
    } else {
        vec![]
    };
    let total = ids.len() as i64;
    let page: Vec<i64> = ids.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    let repos: Vec<db::Repo> = sqlx::query_as("SELECT * FROM repositories WHERE id = ANY($1) ORDER BY id").bind(&page).fetch_all(&s.db).await?;
    let items = repos
        .into_iter()
        .map(|repo| crate::json::repo::repo(&s.api_url(), s.base_url(), &RepoCtx { repo, owner: o.clone(), perm: Perm::Admin }))
        .collect();
    Ok(envelope(&s, &uri, p, "repositories", items, total))
}

#[derive(Deserialize)]
struct SelectedBody {
    selected_repository_ids: Option<Vec<i64>>,
}

async fn org_set_repos(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n)): RP, Json(b): Json<SelectedBody>) -> ApiResult<Response> {
    let ow = org_owner(&s, &u, &org).await?;
    let Owner::Org(o) = &ow else { unreachable!() };
    let ids = b.selected_repository_ids.ok_or_else(|| ApiError::validation("Secret", "selected_repository_ids", "missing_field"))?;
    let row = selected_row(&s, &ow, &n).await?;
    let ids = crate::api::actions_secrets::org_repo_ids(&s, o, &ids).await?;
    replace_selected(&s, row.id, &ids).await?;
    Ok(no_content())
}

async fn org_add_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ow = org_owner(&s, &u, &org).await?;
    let Owner::Org(o) = &ow else { unreachable!() };
    let row = selected_row(&s, &ow, &n).await?;
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM repositories WHERE id = $1 AND owner_id = $2)").bind(id).bind(o.id).fetch_one(&s.db).await?;
    if !owned {
        return Err(ApiError::not_found());
    }
    sqlx::query("INSERT INTO dependabot_secret_repos (secret_id, repo_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(row.id).bind(id).execute(&s.db).await?;
    Ok(no_content())
}

async fn org_remove_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, n, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ow = org_owner(&s, &u, &org).await?;
    let row = selected_row(&s, &ow, &n).await?;
    sqlx::query("DELETE FROM dependabot_secret_repos WHERE secret_id = $1 AND repo_id = $2").bind(row.id).bind(id).execute(&s.db).await?;
    Ok(no_content())
}

// ---------------------------------------------------------------------------------------------
// Repository access

async fn access_owner(s: &AppState, u: &User, org: &str) -> ApiResult<User> {
    let o = crate::orgs::org_by_login(s, org).await?;
    crate::orgs::require_owner(s, &o, u).await?;
    Ok(o)
}

async fn access_get(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Json<Value>> {
    let o = access_owner(&s, &u, &org).await?;
    let level: Option<String> = sqlx::query_scalar("SELECT default_level FROM dependabot_org_settings WHERE org_id = $1").bind(o.id).fetch_optional(&s.db).await?.flatten();
    let repos: Vec<db::Repo> = sqlx::query_as(
        "SELECT r.* FROM dependabot_repository_access a JOIN repositories r ON r.id = a.repo_id WHERE a.org_id = $1 ORDER BY r.id LIMIT $2 OFFSET $3",
    )
    .bind(o.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let owners = super::users_by_id(&s.db, repos.iter().map(|r| r.owner_id)).await?;
    let items: Vec<Value> = repos
        .into_iter()
        .filter_map(|repo| {
            let owner = owners.get(&repo.owner_id)?.clone();
            Some(crate::json::repo::repo(&s.api_url(), s.base_url(), &RepoCtx { repo, owner, perm: Perm::Read }))
        })
        .collect();
    Ok(Json(json!({ "default_level": level, "accessible_repositories": items })))
}

#[derive(Deserialize)]
struct AccessBody {
    repository_ids_to_add: Option<Vec<i64>>,
    repository_ids_to_remove: Option<Vec<i64>>,
}

async fn access_update(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<AccessBody>) -> ApiResult<Response> {
    let o = access_owner(&s, &u, &org).await?;
    if b.repository_ids_to_add.is_none() && b.repository_ids_to_remove.is_none() {
        return Err(ApiError::validation("RepositoryAccess", "repository_ids_to_add", "missing_field"));
    }
    let add = b.repository_ids_to_add.unwrap_or_default();
    // Dependabot may read the org's own repositories (any visibility) and public ones.
    let ok: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE id = ANY($1) AND (owner_id = $2 OR NOT private)")
        .bind(&add)
        .bind(o.id)
        .fetch_one(&s.db)
        .await?;
    let mut uniq = add.clone();
    uniq.sort_unstable();
    uniq.dedup();
    if ok != uniq.len() as i64 {
        return Err(ApiError::validation("RepositoryAccess", "repository_ids_to_add", "invalid"));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO dependabot_repository_access (org_id, repo_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING")
        .bind(o.id)
        .bind(&uniq)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM dependabot_repository_access WHERE org_id = $1 AND repo_id = ANY($2)")
        .bind(o.id)
        .bind(b.repository_ids_to_remove.unwrap_or_default())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct DefaultLevel {
    default_level: Option<String>,
}

async fn access_default(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<DefaultLevel>) -> ApiResult<Response> {
    let o = access_owner(&s, &u, &org).await?;
    let level = b.default_level.ok_or_else(|| ApiError::validation("RepositoryAccess", "default_level", "missing_field"))?;
    if !["public", "internal"].contains(&level.as_str()) {
        return Err(ApiError::validation("RepositoryAccess", "default_level", "invalid"));
    }
    sqlx::query("INSERT INTO dependabot_org_settings (org_id, default_level) VALUES ($1, $2) ON CONFLICT (org_id) DO UPDATE SET default_level = $2")
        .bind(o.id)
        .bind(&level)
        .execute(&s.db)
        .await?;
    Ok(no_content())
}
