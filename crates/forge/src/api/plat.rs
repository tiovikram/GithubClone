//! Helpers shared by the platform-area modules (releases, commit comments, search, ...).
use std::collections::HashMap;

use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::db::{RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Maps commit author emails to forge users: by `users.email` (case-insensitive) or by the
/// noreply form forge itself uses (`{id}+{login}@users.noreply.forge`).
pub async fn users_by_emails(db: &PgPool, emails: &[String]) -> ApiResult<HashMap<String, User>> {
    let mut out = HashMap::new();
    if emails.is_empty() {
        return Ok(out);
    }
    let lower: Vec<String> = emails.iter().map(|e| e.to_lowercase()).collect();
    let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE lower(email) = ANY($1)").bind(&lower).fetch_all(db).await?;
    for u in users {
        if let Some(e) = &u.email {
            out.insert(e.to_lowercase(), u.clone());
        }
    }
    let mut noreply_ids = Vec::new();
    for e in &lower {
        if let Some(local) = e.strip_suffix("@users.noreply.forge") {
            if let Some((id, _)) = local.split_once('+') {
                if let Ok(id) = id.parse::<i64>() {
                    noreply_ids.push((id, e.clone()));
                }
            }
        }
    }
    if !noreply_ids.is_empty() {
        let ids: Vec<i64> = noreply_ids.iter().map(|(i, _)| *i).collect();
        let users = crate::json::issue::users_by_ids(db, &ids).await?;
        for (id, e) in noreply_ids {
            if let Some(u) = users.get(&id) {
                out.insert(e, u.clone());
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Reactions on platform-owned subjects (commit comments, releases).

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReactionRow {
    pub id: i64,
    pub subject_type: String,
    pub subject_id: i64,
    pub user_id: i64,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub fn reaction_json(s: &AppState, r: &ReactionRow, u: Option<&User>) -> Value {
    json!({
        "id": r.id,
        "node_id": crate::ids::encode("Reaction", r.id),
        "user": u.map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).unwrap_or(Value::Null),
        "content": r.content,
        "created_at": crate::json::ts(r.created_at),
    })
}

#[derive(Deserialize)]
pub struct ReactionQuery {
    pub content: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

pub async fn list_reactions(s: &AppState, subject_type: &str, subject_id: i64, q: &ReactionQuery) -> ApiResult<Vec<Value>> {
    let p = super::PageParams { page: q.page, per_page: q.per_page };
    let rows: Vec<ReactionRow> = sqlx::query_as(
        "SELECT id, subject_type, subject_id, user_id, content, created_at FROM reactions
          WHERE subject_type = $1 AND subject_id = $2 AND ($3::text IS NULL OR content = $3)
          ORDER BY id LIMIT $4 OFFSET $5",
    )
    .bind(subject_type)
    .bind(subject_id)
    .bind(&q.content)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.user_id).collect();
    let users = crate::json::issue::users_by_ids(&s.db, &ids).await?;
    Ok(rows.iter().map(|r| reaction_json(s, r, users.get(&r.user_id))).collect())
}

#[derive(Deserialize)]
pub struct CreateReaction {
    pub content: String,
}

/// Creates (or returns the existing) reaction. 201 when created, 200 when it already existed.
pub async fn create_reaction(s: &AppState, subject_type: &str, subject_id: i64, user: &User, content: &str, allowed: &[&str]) -> ApiResult<Response> {
    if !allowed.contains(&content) {
        return Err(ApiError::validation("Reaction", "content", "invalid"));
    }
    let inserted: Option<ReactionRow> = sqlx::query_as(
        "INSERT INTO reactions (subject_type, subject_id, user_id, content) VALUES ($1,$2,$3,$4)
         ON CONFLICT DO NOTHING RETURNING id, subject_type, subject_id, user_id, content, created_at",
    )
    .bind(subject_type)
    .bind(subject_id)
    .bind(user.id)
    .bind(content)
    .fetch_optional(&s.db)
    .await?;
    let (row, status) = match inserted {
        Some(r) => (r, StatusCode::CREATED),
        None => (
            sqlx::query_as(
                "SELECT id, subject_type, subject_id, user_id, content, created_at FROM reactions
                  WHERE subject_type = $1 AND subject_id = $2 AND user_id = $3 AND content = $4",
            )
            .bind(subject_type)
            .bind(subject_id)
            .bind(user.id)
            .bind(content)
            .fetch_one(&s.db)
            .await?,
            StatusCode::OK,
        ),
    };
    Ok((status, axum::Json(reaction_json(s, &row, Some(user)))).into_response())
}

pub async fn delete_reaction(s: &AppState, ctx: &RepoCtx, subject_type: &str, subject_id: i64, reaction_id: i64, user: &User) -> ApiResult<Response> {
    let owner: Option<i64> = sqlx::query_scalar("SELECT user_id FROM reactions WHERE id = $1 AND subject_type = $2 AND subject_id = $3")
        .bind(reaction_id)
        .bind(subject_type)
        .bind(subject_id)
        .fetch_optional(&s.db)
        .await?;
    let Some(owner) = owner else { return Err(ApiError::not_found()) };
    if owner != user.id && ctx.perm < crate::db::Perm::Admin {
        return Err(ApiError::forbidden("Must have admin rights to Repository."));
    }
    sqlx::query("DELETE FROM reactions WHERE id = $1").bind(reaction_id).execute(&s.db).await?;
    Ok(super::no_content())
}

/// Renders `(subject_id -> counts)` for a subject type (re-export for convenience).
pub async fn reaction_counts(db: &PgPool, subject_type: &str, ids: &[i64]) -> ApiResult<HashMap<i64, HashMap<String, i64>>> {
    crate::json::issue::reaction_counts(db, subject_type, ids).await
}

/// `{"total_count": N, <key>: [...]}` envelope used by Actions list endpoints.
pub fn total_envelope(key: &str, items: Vec<Value>, total: i64) -> Value {
    let mut v = json!({ "total_count": total });
    v[key] = Value::Array(items);
    v
}

/// Runs `git` on a synced cache of the repository; returns stdout or None on non-zero exit.
pub async fn git_read(s: &AppState, repo_id: i64, args: &[&str]) -> ApiResult<Option<Vec<u8>>> {
    let path = s.git.sync(repo_id).await?;
    let (ok, out, _) = s.git.git.try_run(&path, args).await?;
    Ok(ok.then_some(out))
}
