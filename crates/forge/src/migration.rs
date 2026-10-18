//! Migrations (exports) of user and organization repositories, like GitHub's migrations API.
//!
//! `start` records a migration (`pending`) and enqueues a job on the standard queue. The job
//! (`run_job`, any replica) moves it to `exporting`, writes a migration archive and uploads it to
//! object storage, then marks it `exported` (`failed` on error). The archive is a `.tar.gz` in
//! GitHub's migration archive layout as far as it is documented:
//!
//! ```text
//! schema.json                         {"version": "1.2.0", "github_sha": ...}
//! organizations_000001.json           organization migrations
//! users_000001.json                   every user referenced below
//! repositories_000001.json            repositories (labels embedded, as GitHub does)
//! milestones_000001.json
//! issues_000001.json, pull_requests_000001.json
//! issue_comments_000001.json, pull_request_reviews_000001.json,
//! pull_request_review_comments_000001.json, releases_000001.json
//! repositories/<owner>/<repo>.bundle  `git bundle --all` of each repository
//! ```
//!
//! Records reference each other by URL (`{base}/<owner>/<repo>/issues/1`, `{base}/<login>`),
//! like GitHub's archives. Downloads redirect to a short-lived signed URL
//! (`/api/forge/migrations/{id}/archive?token=…`). With `lock_repositories`, the repositories
//! are locked (`migration_locks`) until unlocked: pushes (HTTP, SSH) and REST writes are refused.
use std::path::{Path, PathBuf};

use object_store::ObjectStoreExt as _;

use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::jobs::Job;
use crate::state::AppState;

pub const QUEUE: &str = "migration";
/// Lifetime of signed archive download URLs.
pub const ARCHIVE_TOKEN_TTL_SECS: i64 = 300;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MigrationRow {
    pub id: i64,
    pub guid: String,
    pub owner_id: i64,
    pub created_by: Option<i64>,
    pub state: String,
    pub lock_repositories: bool,
    pub exclude_metadata: bool,
    pub exclude_git_data: bool,
    pub exclude_attachments: bool,
    pub exclude_releases: bool,
    pub exclude_owner_projects: bool,
    pub org_metadata_only: bool,
    pub exclude: Vec<String>,
    pub archive_key: Option<String>,
    pub archive_size: Option<i64>,
    pub archive_deleted: bool,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StartMigration {
    pub repositories: Option<Vec<String>>,
    pub lock_repositories: Option<bool>,
    pub exclude_metadata: Option<bool>,
    pub exclude_git_data: Option<bool>,
    pub exclude_attachments: Option<bool>,
    pub exclude_releases: Option<bool>,
    pub exclude_owner_projects: Option<bool>,
    pub org_metadata_only: Option<bool>,
    pub exclude: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------------------------
// Service

/// Starts a migration of `owner`'s repositories (`owner` = the actor or an org they own).
pub async fn start(s: &AppState, actor: &User, owner: &User, input: StartMigration) -> ApiResult<MigrationRow> {
    let names = input.repositories.clone().unwrap_or_default();
    let org_only = input.org_metadata_only.unwrap_or(false);
    if input.repositories.is_none() {
        return Err(ApiError::validation("Migration", "repositories", "missing_field"));
    }
    if names.is_empty() && !org_only {
        return Err(ApiError::validation("Migration", "repositories", "missing_field"));
    }
    if org_only && owner.kind != "Organization" {
        return Err(ApiError::validation("Migration", "org_metadata_only", "invalid"));
    }
    let exclude = input.exclude.clone().unwrap_or_default();
    if exclude.iter().any(|e| e != "repositories") {
        return Err(ApiError::validation("Migration", "exclude", "invalid"));
    }
    let mut repo_ids = Vec::new();
    for n in &names {
        let (o, r) = match n.split_once('/') {
            Some((o, r)) => (o.to_string(), r.to_string()),
            None => (owner.login.clone(), n.clone()),
        };
        let ctx = match db::load_repo(&s.db, Some(actor), &o, &r).await {
            Ok(c) if c.owner.id == owner.id && c.perm >= Perm::Admin => c,
            _ => {
                let mut e = ApiError::validation("Migration", "repositories", "invalid");
                e.errors = Some(json!([{"resource": "Migration", "field": "repositories", "code": "invalid", "message": format!("Repository {n} could not be found or you do not have admin access to it.")}]));
                return Err(e);
            }
        };
        if !repo_ids.contains(&ctx.repo.id) {
            repo_ids.push(ctx.repo.id);
        }
    }
    let lock = input.lock_repositories.unwrap_or(false);
    let mut tx = s.db.begin().await?;
    let m: MigrationRow = sqlx::query_as(
        "INSERT INTO migrations (guid, owner_id, created_by, lock_repositories, exclude_metadata, exclude_git_data,
             exclude_attachments, exclude_releases, exclude_owner_projects, org_metadata_only, exclude)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING *",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(owner.id)
    .bind(actor.id)
    .bind(lock)
    .bind(input.exclude_metadata.unwrap_or(false))
    .bind(input.exclude_git_data.unwrap_or(false))
    .bind(input.exclude_attachments.unwrap_or(false))
    .bind(input.exclude_releases.unwrap_or(false))
    .bind(input.exclude_owner_projects.unwrap_or(false))
    .bind(org_only)
    .bind(&exclude)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO migration_repos (migration_id, repo_id) SELECT $1, unnest($2::bigint[])")
        .bind(m.id)
        .bind(&repo_ids)
        .execute(&mut *tx)
        .await?;
    if lock && !repo_ids.is_empty() {
        let locked = sqlx::query("INSERT INTO migration_locks (repo_id, migration_id) SELECT unnest($2::bigint[]), $1 ON CONFLICT DO NOTHING")
            .bind(m.id)
            .bind(&repo_ids)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if locked as usize != repo_ids.len() {
            return Err(ApiError::unprocessable("A repository is already locked by another migration."));
        }
    }
    crate::jobs::enqueue(&mut *tx, QUEUE, &json!({ "migration_id": m.id })).await?;
    tx.commit().await?;
    Ok(m)
}

pub async fn by_id(db: &sqlx::PgPool, owner: &User, id: i64) -> ApiResult<MigrationRow> {
    sqlx::query_as("SELECT * FROM migrations WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(owner.id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn repo_ids(db: &sqlx::PgPool, m: &MigrationRow) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT repo_id FROM migration_repos WHERE migration_id = $1 ORDER BY repo_id").bind(m.id).fetch_all(db).await?)
}

/// Deletes the archive (object storage and record). The migration itself stays.
pub async fn delete_archive(s: &AppState, m: &MigrationRow) -> ApiResult<()> {
    if m.state != "exported" || m.archive_deleted {
        return Err(ApiError::not_found());
    }
    if let Some(k) = &m.archive_key {
        let _ = s.store.delete(&object_store::path::Path::from(k.as_str())).await;
    }
    sqlx::query("UPDATE migrations SET archive_key = NULL, archive_deleted = TRUE, updated_at = now() WHERE id = $1").bind(m.id).execute(&s.db).await?;
    Ok(())
}

/// Unlocks repository `name` of the migration (404 if it is not part of it).
pub async fn unlock(s: &AppState, m: &MigrationRow, name: &str) -> ApiResult<()> {
    let repo: Option<i64> = sqlx::query_scalar(
        "SELECT r.id FROM migration_repos mr JOIN repositories r ON r.id = mr.repo_id WHERE mr.migration_id = $1 AND lower(r.name) = lower($2)",
    )
    .bind(m.id)
    .bind(name.rsplit('/').next().unwrap_or(name))
    .fetch_optional(&s.db)
    .await?;
    let id = repo.ok_or_else(ApiError::not_found)?;
    let n = sqlx::query("DELETE FROM migration_locks WHERE repo_id = $1 AND migration_id = $2").bind(id).bind(m.id).execute(&s.db).await?.rows_affected();
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(())
}

pub async fn is_locked(db: &sqlx::PgPool, repo_id: i64) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM migration_locks WHERE repo_id = $1)")
        .bind(repo_id)
        .fetch_one(db)
        .await
        .unwrap_or(false)
}

/// Middleware helper (called from `apps::scope::middleware`): refuses writes to locked
/// repositories. Callers that cannot see the repository get 404, never the lock message.
pub async fn lock_guard(s: &AppState, req: &mut axum::extract::Request, method: &Method, path: &str, query: Option<&str>) -> Option<Response> {
    let write = !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (owner, repo, git) = match segs.as_slice() {
        ["api", "v3", "repos", o, r, ..] | ["api", "uploads", "repos", o, r, ..] if write => (*o, *r, false),
        [o, r, "git-receive-pack"] => (*o, *r, true),
        [o, r, "info", "refs"] if query.is_some_and(|q| q.contains("service=git-receive-pack")) => (*o, *r, true),
        _ => return None,
    };
    let name = repo.strip_suffix(".git").unwrap_or(repo);
    let locked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM migration_locks l JOIN repositories r ON r.id = l.repo_id JOIN users u ON u.id = r.owner_id
          WHERE lower(u.login) = lower($1) AND lower(r.name) = lower($2))",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&s.db)
    .await
    .unwrap_or(false);
    if !locked {
        return None;
    }
    let (mut parts, body) = std::mem::take(req).into_parts();
    let viewer = match crate::auth::authenticate(&mut parts, s).await {
        Ok(crate::auth::AuthResult::User(u)) => Some(u),
        _ => None,
    };
    let visible = db::load_repo(&s.db, viewer.as_ref(), owner, name).await.is_ok();
    *req = axum::extract::Request::from_parts(parts, body);
    const MSG: &str = "Repository has been locked for migration.";
    Some(match (git, visible) {
        (true, true) => (StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "text/plain")], format!("{MSG}\n")).into_response(),
        (true, false) => (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain")], "Repository not found\n").into_response(),
        (false, true) => ApiError::forbidden(MSG).into_response(),
        (false, false) => ApiError::not_found().into_response(),
    })
}

// ---------------------------------------------------------------------------------------------
// JSON

pub async fn migration_json(s: &AppState, m: &MigrationRow, owner: &User, viewer: &User, with_repos: bool) -> ApiResult<Value> {
    let api = s.api_url();
    let base = s.base_url();
    let url = if owner.kind == "Organization" {
        format!("{api}/orgs/{}/migrations/{}", owner.login, m.id)
    } else {
        format!("{api}/user/migrations/{}", m.id)
    };
    let repos = if with_repos && !m.exclude.iter().any(|e| e == "repositories") {
        let ids = repo_ids(&s.db, m).await?;
        let mut out = Vec::new();
        for ctx in crate::apps::repos_by_ids(&s.db, &ids, Perm::None).await? {
            let perm = db::permission(&s.db, Some(viewer), &ctx.repo, &ctx.owner).await?;
            out.push(crate::json::repo::repo(&api, base, &RepoCtx { perm, ..ctx }));
        }
        out
    } else {
        vec![]
    };
    Ok(json!({
        "id": m.id,
        "node_id": crate::ids::encode("Migration", m.id),
        "owner": crate::json::user::simple(&api, base, owner),
        "guid": m.guid,
        "state": m.state,
        "lock_repositories": m.lock_repositories,
        "exclude_metadata": m.exclude_metadata,
        "exclude_git_data": m.exclude_git_data,
        "exclude_attachments": m.exclude_attachments,
        "exclude_releases": m.exclude_releases,
        "exclude_owner_projects": m.exclude_owner_projects,
        "org_metadata_only": m.org_metadata_only,
        "repositories": repos,
        "url": url,
        "archive_url": format!("{url}/archive"),
        "exclude": m.exclude,
        "created_at": crate::json::ts(m.created_at),
        "updated_at": crate::json::ts(m.updated_at),
    }))
}

// ---------------------------------------------------------------------------------------------
// Signed downloads

fn mac(secret: &str, id: i64, exp: i64) -> String {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("any key length");
    m.update(format!("migration\0{id}\0{exp}").as_bytes());
    hex::encode(&m.finalize().into_bytes()[..20])
}

/// The redirect target for an exported archive.
pub async fn signed_archive_url(s: &AppState, m: &MigrationRow) -> ApiResult<String> {
    let secret = crate::signed_url::server_secret(&s.db, "migration").await.map_err(ApiError::internal)?;
    let exp = Utc::now().timestamp() + ARCHIVE_TOKEN_TTL_SECS;
    Ok(format!("{}/api/forge/migrations/{}/archive?token={exp:x}{}", s.base_url(), m.id, mac(&secret, m.id, exp)))
}

pub async fn verify_archive_token(s: &AppState, id: i64, token: &str) -> bool {
    if token.len() <= 40 || !token.is_ascii() {
        return false;
    }
    let (exp_hex, sig) = token.split_at(token.len() - 40);
    let Ok(exp) = i64::from_str_radix(exp_hex, 16) else { return false };
    if exp < Utc::now().timestamp() {
        return false;
    }
    let Ok(secret) = crate::signed_url::server_secret(&s.db, "migration").await else { return false };
    let want = mac(&secret, id, exp);
    want.len() == sig.len() && want.bytes().zip(sig.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// Streams an archive (after `verify_archive_token`).
pub async fn stream_archive(s: &AppState, id: i64) -> ApiResult<Response> {
    let m: MigrationRow = sqlx::query_as("SELECT * FROM migrations WHERE id = $1").bind(id).fetch_optional(&s.db).await?.ok_or_else(ApiError::not_found)?;
    let key = m.archive_key.clone().filter(|_| m.state == "exported").ok_or_else(ApiError::not_found)?;
    let got = s.store.get(&object_store::path::Path::from(key)).await.map_err(|e| match e {
        object_store::Error::NotFound { .. } => ApiError::not_found(),
        e => ApiError::internal(e),
    })?;
    let mut res = Response::new(axum::body::Body::from_stream(got.into_stream()));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/gzip"));
    if let Some(n) = m.archive_size {
        h.insert(header::CONTENT_LENGTH, header::HeaderValue::from(n as u64));
    }
    if let Ok(v) = header::HeaderValue::from_str(&format!("attachment; filename=migration_archive_{}.tar.gz", m.guid)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    crate::security::harden_user_content(&mut res);
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// The export job

pub async fn run_job(s: &AppState, job: &Job) -> anyhow::Result<()> {
    let id = job.payload.get("migration_id").and_then(Value::as_i64).ok_or_else(|| anyhow::anyhow!("bad job"))?;
    let Some(m) = sqlx::query_as::<_, MigrationRow>("SELECT * FROM migrations WHERE id = $1").bind(id).fetch_optional(&s.db).await? else {
        return Ok(()); // deleted
    };
    if m.state == "exported" {
        return Ok(());
    }
    sqlx::query("UPDATE migrations SET state = 'exporting', updated_at = now() WHERE id = $1").bind(id).execute(&s.db).await?;
    match export(s, &m).await {
        Ok((key, size)) => {
            sqlx::query("UPDATE migrations SET state = 'exported', archive_key = $2, archive_size = $3, error = NULL, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(key)
                .bind(size)
                .execute(&s.db)
                .await?;
            Ok(())
        }
        Err(e) if job.attempts < job.max_attempts => {
            sqlx::query("UPDATE migrations SET state = 'pending', error = $2, updated_at = now() WHERE id = $1").bind(id).bind(format!("{e:#}")).execute(&s.db).await?;
            Err(e)
        }
        Err(e) => {
            tracing::warn!(migration = id, error = %format!("{e:#}"), "migration export failed");
            sqlx::query("UPDATE migrations SET state = 'failed', error = $2, updated_at = now() WHERE id = $1").bind(id).bind(format!("{e:#}")).execute(&s.db).await?;
            Ok(())
        }
    }
}

fn write_json(dir: &Path, name: &str, items: &[Value]) -> anyhow::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    std::fs::write(dir.join(format!("{name}_000001.json")), serde_json::to_vec_pretty(items)?)?;
    Ok(())
}

fn ts(t: Option<DateTime<Utc>>) -> Value {
    crate::json::ts_opt(t)
}

struct Collected {
    users: Vec<i64>,
    repositories: Vec<Value>,
    milestones: Vec<Value>,
    issues: Vec<Value>,
    pulls: Vec<Value>,
    comments: Vec<Value>,
    reviews: Vec<Value>,
    review_comments: Vec<Value>,
    releases: Vec<Value>,
}

type IssueRec = (i64, i64, bool, String, Option<String>, String, Option<i64>, Option<i64>, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>);

async fn collect_repo(s: &AppState, m: &MigrationRow, ctx: &RepoCtx, out: &mut Collected, logins: &std::collections::HashMap<i64, String>) -> anyhow::Result<()> {
    let base = s.base_url();
    let repo_url = format!("{base}/{}", ctx.full_name());
    let user_url = |id: Option<i64>| -> Value {
        id.and_then(|i| logins.get(&i)).map(|l| json!(format!("{base}/{l}"))).unwrap_or(Value::Null)
    };
    let labels: Vec<(i64, String, String, Option<String>, DateTime<Utc>)> =
        sqlx::query_as("SELECT id, name, color, description, created_at FROM labels WHERE repo_id = $1 ORDER BY id").bind(ctx.repo.id).fetch_all(&s.db).await?;
    out.repositories.push(json!({
        "type": "repository",
        "url": repo_url,
        "owner": format!("{base}/{}", ctx.owner.login),
        "name": ctx.repo.name,
        "description": ctx.repo.description,
        "website": ctx.repo.homepage,
        "private": ctx.repo.private,
        "has_issues": ctx.repo.has_issues,
        "has_wiki": ctx.repo.has_wiki,
        "has_downloads": true,
        "labels": labels.iter().map(|(_, n, c, d, t)| json!({
            "url": format!("{repo_url}/labels/{}", crate::json::git::enc_path(n)),
            "name": n, "color": c, "description": d, "created_at": crate::json::ts(*t),
        })).collect::<Vec<_>>(),
        "collaborators": [],
        "created_at": crate::json::ts(ctx.repo.created_at),
        "git_url": format!("tarball://root/repositories/{}.bundle", ctx.full_name()),
        "default_branch": ctx.repo.default_branch,
        "public_keys": [],
        "archived": ctx.repo.archived,
    }));
    if m.exclude_metadata {
        return Ok(());
    }
    let milestones: Vec<(i64, i64, String, Option<String>, String, Option<DateTime<Utc>>, Option<i64>, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, number, title, description, state, due_on, creator_id, created_at, updated_at, closed_at FROM milestones WHERE repo_id = $1 ORDER BY number",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    for (_, number, title, desc, state, due, creator, created, updated, closed) in &milestones {
        out.milestones.push(json!({
            "type": "milestone", "url": format!("{repo_url}/milestones/{number}"), "repository": repo_url,
            "user": user_url(*creator), "title": title, "description": desc, "state": state, "due_on": ts(*due),
            "created_at": crate::json::ts(*created), "updated_at": crate::json::ts(*updated), "closed_at": ts(*closed),
        }));
    }
    let milestone_url = |mid: Option<i64>| -> Value {
        mid.and_then(|id| milestones.iter().find(|x| x.0 == id)).map(|x| json!(format!("{repo_url}/milestones/{}", x.1))).unwrap_or(Value::Null)
    };
    let issues: Vec<IssueRec> = sqlx::query_as(
        "SELECT id, number, is_pull, title, body, state, user_id, milestone_id, created_at, updated_at, closed_at FROM issues WHERE repo_id = $1 ORDER BY number",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    let issue_labels: Vec<(i64, String)> = sqlx::query_as(
        "SELECT il.issue_id, l.name FROM issue_labels il JOIN labels l ON l.id = il.label_id JOIN issues i ON i.id = il.issue_id WHERE i.repo_id = $1",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    let assignees: Vec<(i64, i64)> =
        sqlx::query_as("SELECT a.issue_id, a.user_id FROM issue_assignees a JOIN issues i ON i.id = a.issue_id WHERE i.repo_id = $1").bind(ctx.repo.id).fetch_all(&s.db).await?;
    let pulls: Vec<(i64, String, String, String, String, bool, Option<DateTime<Utc>>, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT issue_id, head_ref, head_sha, base_ref, base_sha, merged, merged_at, merged_by_id, merge_commit_sha FROM pull_requests WHERE repo_id = $1",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    let issue_url = |id: i64| -> String {
        let i = issues.iter().find(|x| x.0 == id);
        match i {
            Some(x) if x.2 => format!("{repo_url}/pull/{}", x.1),
            Some(x) => format!("{repo_url}/issues/{}", x.1),
            None => repo_url.clone(),
        }
    };
    for (id, _, is_pull, title, body, state, uid, mid, created, updated, closed) in &issues {
        let lbls: Vec<String> = issue_labels
            .iter()
            .filter(|(i, _)| i == id)
            .map(|(_, n)| format!("{repo_url}/labels/{}", crate::json::git::enc_path(n)))
            .collect();
        let asg: Vec<Value> = assignees.iter().filter(|(i, _)| i == id).map(|(_, u)| user_url(Some(*u))).collect();
        let mut rec = json!({
            "url": issue_url(*id), "repository": repo_url, "user": user_url(*uid), "title": title, "body": body,
            "assignee": asg.first().cloned().unwrap_or(Value::Null), "assignees": asg, "milestone": milestone_url(*mid),
            "labels": lbls, "reactions": [], "state": state,
            "closed_at": ts(*closed), "created_at": crate::json::ts(*created), "updated_at": crate::json::ts(*updated),
        });
        if *is_pull {
            rec["type"] = json!("pull_request");
            if let Some(p) = pulls.iter().find(|p| p.0 == *id) {
                rec["head"] = json!({ "ref": p.1, "sha": p.2, "repo": repo_url, "user": user_url(Some(ctx.owner.id)) });
                rec["base"] = json!({ "ref": p.3, "sha": p.4, "repo": repo_url, "user": user_url(Some(ctx.owner.id)) });
                rec["merged_at"] = ts(p.6);
                rec["merged_by"] = user_url(p.7);
                rec["merge_commit_sha"] = json!(p.8);
            }
            out.pulls.push(rec);
        } else {
            rec["type"] = json!("issue");
            out.issues.push(rec);
        }
    }
    let comments: Vec<(i64, i64, Option<i64>, String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, issue_id, user_id, body, created_at, updated_at FROM issue_comments WHERE repo_id = $1 ORDER BY id",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    for (cid, iid, uid, body, created, updated) in comments {
        let parent = issue_url(iid);
        let is_pull = issues.iter().any(|x| x.0 == iid && x.2);
        let mut c = json!({
            "type": "issue_comment", "url": format!("{parent}#issuecomment-{cid}"), "user": user_url(uid), "body": body,
            "reactions": [], "created_at": crate::json::ts(created), "updated_at": crate::json::ts(updated),
        });
        c[if is_pull { "pull_request" } else { "issue" }] = json!(parent);
        out.comments.push(c);
    }
    let reviews: Vec<(i64, i64, Option<i64>, String, String, Option<String>, Option<DateTime<Utc>>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT r.id, r.issue_id, r.user_id, r.body, r.state, r.commit_id, r.submitted_at, r.created_at
           FROM pull_reviews r JOIN issues i ON i.id = r.issue_id WHERE i.repo_id = $1 AND r.state <> 'PENDING' ORDER BY r.id",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    for (rid, iid, uid, body, state, commit, submitted, created) in reviews {
        let parent = issue_url(iid);
        out.reviews.push(json!({
            "type": "pull_request_review", "url": format!("{parent}#pullrequestreview-{rid}"), "pull_request": parent,
            "user": user_url(uid), "body": body, "head_sha": commit, "state": match state.as_str() {
                "APPROVED" => 40, "CHANGES_REQUESTED" => 30, "DISMISSED" => 20, _ => 1 },
            "reactions": [], "created_at": crate::json::ts(created), "submitted_at": ts(submitted),
        }));
    }
    let rcs: Vec<(i64, i64, Option<i64>, Option<i64>, String, String, String, String, Option<i64>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, issue_id, review_id, user_id, body, path, commit_id, diff_hunk, position, created_at FROM review_comments WHERE repo_id = $1 ORDER BY id",
    )
    .bind(ctx.repo.id)
    .fetch_all(&s.db)
    .await?;
    for (cid, iid, rid, uid, body, path, commit, hunk, pos, created) in rcs {
        let parent = issue_url(iid);
        out.review_comments.push(json!({
            "type": "pull_request_review_comment", "url": format!("{parent}#discussion_r{cid}"), "pull_request": parent,
            "pull_request_review": rid.map(|r| format!("{parent}#pullrequestreview-{r}")), "user": user_url(uid), "body": body,
            "path": path, "commit_id": commit, "diff_hunk": hunk, "position": pos, "reactions": [], "created_at": crate::json::ts(created),
        }));
    }
    if !m.exclude_releases {
        let rels: Vec<(i64, String, String, Option<String>, Option<String>, bool, bool, Option<i64>, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT id, tag_name, target_commitish, name, body, draft, prerelease, author_id, created_at, published_at FROM releases WHERE repo_id = $1 ORDER BY id",
        )
        .bind(ctx.repo.id)
        .fetch_all(&s.db)
        .await?;
        for (_, tag, target, name, body, draft, pre, author, created, published) in rels {
            out.releases.push(json!({
                "type": "release", "url": format!("{repo_url}/releases/tag/{}", crate::json::git::enc_path(&tag)), "repository": repo_url,
                "user": user_url(author), "name": name, "tag_name": tag, "body": body, "state": if draft { "draft" } else { "published" },
                "pending_tag": tag, "prerelease": pre, "target_commitish": target, "release_assets": [],
                "published_at": ts(published), "created_at": crate::json::ts(created),
            }));
        }
    }
    Ok(())
}

async fn export(s: &AppState, m: &MigrationRow) -> anyhow::Result<(String, i64)> {
    let owner = db::user_by_id(&s.db, m.owner_id).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let ids = repo_ids(&s.db, m).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let repos = crate::apps::repos_by_ids(&s.db, &ids, Perm::Admin).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let work: PathBuf = s.cfg.cache_dir.join("migrations").join(&m.guid);
    let _ = tokio::fs::remove_dir_all(&work).await;
    let root = work.join("archive");
    tokio::fs::create_dir_all(root.join("repositories")).await?;
    // Every user referenced by the exported records.
    let user_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, login FROM users WHERE id = $1 OR id IN (
             SELECT user_id FROM issues WHERE repo_id = ANY($2) UNION SELECT user_id FROM issue_comments WHERE repo_id = ANY($2)
             UNION SELECT user_id FROM review_comments WHERE repo_id = ANY($2)
             UNION SELECT r.user_id FROM pull_reviews r JOIN issues i ON i.id = r.issue_id WHERE i.repo_id = ANY($2)
             UNION SELECT a.user_id FROM issue_assignees a JOIN issues i ON i.id = a.issue_id WHERE i.repo_id = ANY($2)
             UNION SELECT author_id FROM releases WHERE repo_id = ANY($2) UNION SELECT creator_id FROM milestones WHERE repo_id = ANY($2)
             UNION SELECT merged_by_id FROM pull_requests WHERE repo_id = ANY($2)
             UNION SELECT user_id FROM org_members WHERE org_id = $1)",
    )
    .bind(owner.id)
    .bind(&ids)
    .fetch_all(&s.db)
    .await?;
    let logins: std::collections::HashMap<i64, String> = user_rows.iter().cloned().collect();
    let mut c = Collected {
        users: user_rows.iter().map(|(i, _)| *i).collect(),
        repositories: vec![],
        milestones: vec![],
        issues: vec![],
        pulls: vec![],
        comments: vec![],
        reviews: vec![],
        review_comments: vec![],
        releases: vec![],
    };
    if !m.org_metadata_only {
        for ctx in &repos {
            collect_repo(s, m, ctx, &mut c, &logins).await?;
            if !m.exclude_git_data {
                let dir = root.join("repositories").join(&ctx.owner.login);
                tokio::fs::create_dir_all(&dir).await?;
                let bundle = dir.join(format!("{}.bundle", ctx.repo.name));
                let path = s.git.sync(ctx.repo.id).await?;
                let target = bundle.to_string_lossy().to_string();
                // An empty repository has nothing to bundle (git refuses); it is skipped.
                let _ = s.git.git.try_run(&path, &["bundle", "create", &target, "--all"]).await?;
            }
        }
    }
    let base = s.base_url().to_string();
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1) ORDER BY id").bind(&c.users).fetch_all(&s.db).await?;
    let users_json: Vec<Value> = users
        .iter()
        .filter(|u| u.kind != "Organization")
        .map(|u| {
            json!({
                "type": "user", "url": format!("{base}/{}", u.login), "login": u.login, "name": u.name, "company": u.company,
                "website": if u.blog.is_empty() { Value::Null } else { json!(u.blog) }, "location": u.location,
                "emails": u.email.iter().map(|e| json!({"address": e, "primary": true, "verified": false})).collect::<Vec<_>>(),
                "billing_plan": null, "created_at": crate::json::ts(u.created_at),
            })
        })
        .collect();
    let root2 = root.clone();
    let mut orgs_json = vec![];
    if owner.kind == "Organization" {
        let members: Vec<(String, String)> = sqlx::query_as("SELECT u.login, m.role FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 ORDER BY u.login")
            .bind(owner.id)
            .fetch_all(&s.db)
            .await?;
        orgs_json.push(json!({
            "type": "organization", "url": format!("{base}/{}", owner.login), "login": owner.login, "name": owner.name,
            "description": owner.bio, "website": if owner.blog.is_empty() { Value::Null } else { json!(owner.blog) }, "location": owner.location,
            "email": owner.email, "members": members.iter().map(|(l, r)| json!({"user": format!("{base}/{l}"), "role": r, "state": "active"})).collect::<Vec<_>>(),
            "created_at": crate::json::ts(owner.created_at),
        }));
    }
    let schema = json!({ "version": "1.2.0", "github_sha": "forge" });
    let c2 = c;
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        std::fs::write(root2.join("schema.json"), serde_json::to_vec_pretty(&schema)?)?;
        write_json(&root2, "organizations", &orgs_json)?;
        write_json(&root2, "users", &users_json)?;
        write_json(&root2, "repositories", &c2.repositories)?;
        write_json(&root2, "milestones", &c2.milestones)?;
        write_json(&root2, "issues", &c2.issues)?;
        write_json(&root2, "pull_requests", &c2.pulls)?;
        write_json(&root2, "issue_comments", &c2.comments)?;
        write_json(&root2, "pull_request_reviews", &c2.reviews)?;
        write_json(&root2, "pull_request_review_comments", &c2.review_comments)?;
        write_json(&root2, "releases", &c2.releases)?;
        Ok(())
    })
    .await??;
    let tarball = work.join("archive.tar.gz");
    let (src, dst) = (root.clone(), tarball.clone());
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let f = std::fs::File::create(&dst)?;
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        tar.follow_symlinks(false);
        tar.append_dir_all(".", &src)?;
        tar.into_inner()?.finish()?;
        Ok(())
    })
    .await??;
    let size = tokio::fs::metadata(&tarball).await?.len() as i64;
    let key = format!("migrations/{}/{}.tar.gz", m.id, m.guid);
    {
        use tokio::io::AsyncWriteExt;
        let mut w = object_store::buffered::BufWriter::new(s.store.clone(), object_store::path::Path::from(key.as_str()));
        let mut f = tokio::fs::File::open(&tarball).await?;
        if let Err(e) = tokio::io::copy(&mut f, &mut w).await {
            let _ = w.abort().await;
            return Err(e.into());
        }
        w.shutdown().await?;
    }
    let _ = tokio::fs::remove_dir_all(&work).await;
    Ok((key, size))
}
