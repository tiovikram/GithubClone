//! Forks: `POST /repos/{owner}/{repo}/forks` and `GET /repos/{owner}/{repo}/forks`.
//! A fork gets its own repository row and storage manifest; pack objects are copied
//! server-side in object storage, so the fork is cloneable immediately.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use object_store::path::Path as ObjPath;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use serde::Deserialize;

use super::gitapi;
use super::{PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, Repo, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::git::parse_refs;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/repos/{owner}/{repo}/forks", get(list_forks).post(create_fork_h))
}

/// Copies `from`'s storage (heads + tags, or only `only_branch`) to the new repository `to`.
pub async fn copy_storage(s: &AppState, from: i64, to: i64, only_branch: Option<&str>) -> anyhow::Result<()> {
    let m = s.git.manifest(from).await?;
    for pack in &m.packs {
        for ext in ["pack", "idx"] {
            let src = ObjPath::from(format!("repos/{from}/packs/{pack}.{ext}"));
            let dst = ObjPath::from(format!("repos/{to}/packs/{pack}.{ext}"));
            s.store.copy(&src, &dst).await?;
        }
    }
    let refs = parse_refs(&m.refs);
    let mut text = String::new();
    for (name, sha) in &refs {
        let keep = match only_branch {
            Some(b) => name == &format!("refs/heads/{b}"),
            None => name.starts_with("refs/heads/") || name.starts_with("refs/tags/"),
        };
        if keep {
            text.push_str(&format!("{sha} {name}\n"));
        }
    }
    let head = match only_branch {
        Some(b) => format!("refs/heads/{b}"),
        None => m.head.clone(),
    };
    sqlx::query(
        "INSERT INTO repo_storage (repo_id, version, head, refs, packs) VALUES ($1, 1, $2, $3, $4)
         ON CONFLICT (repo_id) DO UPDATE SET version = repo_storage.version + 1, head = $2, refs = $3, packs = $4, updated_at = now()",
    )
    .bind(to)
    .bind(&head)
    .bind(&text)
    .bind(&m.packs)
    .execute(&s.db)
    .await?;
    Ok(())
}

/// Forks `source` into `owner` (a user, or an organization the actor belongs to).
pub async fn create_fork(
    s: &AppState,
    source: &RepoCtx,
    actor: &User,
    owner: &User,
    name: Option<String>,
    default_branch_only: bool,
) -> ApiResult<RepoCtx> {
    // Existing fork of this repo in the target account: return it (GitHub is idempotent).
    let existing: Option<Repo> = sqlx::query_as("SELECT * FROM repositories WHERE owner_id = $1 AND fork_parent_id = $2 ORDER BY id LIMIT 1")
        .bind(owner.id)
        .bind(source.repo.id)
        .fetch_optional(&s.db)
        .await?;
    if let Some(r) = existing {
        let perm = db::permission(&s.db, Some(actor), &r, owner).await?;
        return Ok(RepoCtx { repo: r, owner: owner.clone(), perm });
    }
    if owner.id == source.owner.id && name.as_deref().is_none_or(|n| n.eq_ignore_ascii_case(&source.repo.name)) {
        return Err(ApiError::unprocessable("Cannot fork a repository into the account that owns it"));
    }
    let base = name.filter(|n| !n.is_empty()).unwrap_or_else(|| source.repo.name.clone());
    if !super::repos::valid_repo_name(&base) {
        return Err(ApiError::validation("Repository", "name", "invalid"));
    }
    let r = &source.repo;
    let mut repo: Option<Repo> = None;
    for i in 0..100 {
        let candidate = if i == 0 { base.clone() } else { format!("{base}-{i}") };
        repo = sqlx::query_as(
            "INSERT INTO repositories (owner_id, name, description, homepage, private, visibility, default_branch,
                fork_parent_id, has_issues, has_projects, has_wiki, has_discussions, is_template,
                allow_squash_merge, allow_merge_commit, allow_rebase_merge, allow_auto_merge, delete_branch_on_merge,
                language, size_kb, pushed_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,false,$9,$10,false,false,$11,$12,$13,$14,$15,$16,$17,$18)
             ON CONFLICT DO NOTHING RETURNING *",
        )
        .bind(owner.id)
        .bind(&candidate)
        .bind(&r.description)
        .bind(&r.homepage)
        .bind(r.private)
        .bind(&r.visibility)
        .bind(&r.default_branch)
        .bind(r.id)
        .bind(r.has_projects)
        .bind(r.has_wiki)
        .bind(r.allow_squash_merge)
        .bind(r.allow_merge_commit)
        .bind(r.allow_rebase_merge)
        .bind(r.allow_auto_merge)
        .bind(r.delete_branch_on_merge)
        .bind(&r.language)
        .bind(r.size_kb)
        .bind(r.pushed_at)
        .fetch_optional(&s.db)
        .await?;
        if repo.is_some() {
            break;
        }
    }
    let repo = repo.ok_or_else(|| ApiError::validation("Repository", "name", "already_exists"))?;
    let only = default_branch_only.then_some(r.default_branch.as_str());
    if let Err(e) = copy_storage(s, r.id, repo.id, only).await {
        // Don't leave a half-created fork behind.
        let _ = sqlx::query("DELETE FROM repositories WHERE id = $1").bind(repo.id).execute(&s.db).await;
        let _ = s.git.delete_repo(repo.id).await;
        return Err(ApiError::internal(format!("fork storage copy failed: {e:#}")));
    }
    sqlx::query("UPDATE repositories SET forks_count = forks_count + 1 WHERE id = $1").bind(r.id).execute(&s.db).await?;
    crate::orgs::grant_creator_admin(&s.db, owner, repo.id, actor).await?;
    let perm = db::permission(&s.db, Some(actor), &repo, owner).await?;
    Ok(RepoCtx { repo, owner: owner.clone(), perm })
}

#[derive(Deserialize, Default)]
struct ForkBody {
    organization: Option<String>,
    name: Option<String>,
    #[serde(default)]
    default_branch_only: bool,
}

async fn create_fork_h(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(b): Json<ForkBody>,
) -> ApiResult<Response> {
    let source = gitapi::load(&s, Some(&u), &owner, &repo).await?;
    let target = match b.organization.as_deref().filter(|o| !o.is_empty()) {
        Some(org) => {
            let o = db::user_by_login(&s.db, org).await.map_err(|_| ApiError::validation("Fork", "organization", "invalid"))?;
            if o.kind != "Organization" {
                return Err(ApiError::validation("Fork", "organization", "invalid"));
            }
            let member: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)")
                .bind(o.id)
                .bind(u.id)
                .fetch_one(&s.db)
                .await?;
            if !member && !u.site_admin {
                return Err(ApiError::forbidden("You do not have permission to create repositories in this organization."));
            }
            o
        }
        None => u.clone(),
    };
    let ctx = create_fork(&s, &source, &u, &target, b.name, b.default_branch_only).await?;
    let v = super::repos::full_json(&s, Some(&u), &ctx).await?;
    Ok((StatusCode::ACCEPTED, Json(v)).into_response())
}

#[derive(Deserialize, Default)]
struct ListQuery {
    sort: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn list_forks(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    uri: OriginalUri,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Response> {
    let ctx = gitapi::load(&s, u.as_ref(), &owner, &repo).await?;
    let pages = PageParams { page: q.page, per_page: q.per_page };
    let order = match q.sort.as_deref() {
        Some("oldest") => "r.created_at ASC, r.id ASC",
        Some("stargazers") | Some("watchers") => "r.stargazers_count DESC, r.id DESC",
        _ => "r.created_at DESC, r.id DESC",
    };
    let viewer = u.as_ref().map(|x| x.id).unwrap_or(-1);
    let visible = "(NOT r.private OR r.owner_id = $2 OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = $2)
                    OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = $2)
                    OR EXISTS(SELECT 1 FROM users a WHERE a.id = $2 AND a.site_admin))";
    let sql = format!(
        "SELECT r.* FROM repositories r WHERE r.fork_parent_id = $1 AND {visible} ORDER BY {order} LIMIT {} OFFSET {}",
        pages.limit(),
        pages.offset()
    );
    let rows: Vec<Repo> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(ctx.repo.id).bind(viewer).fetch_all(&s.db).await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM repositories r WHERE r.fork_parent_id = $1 AND {visible}"
    )))
    .bind(ctx.repo.id)
    .bind(viewer)
    .fetch_one(&s.db)
    .await?;
    let owner_ids: Vec<i64> = rows.iter().map(|r| r.owner_id).collect();
    let owners: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&owner_ids).fetch_all(&s.db).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(o) = owners.iter().find(|o| o.id == r.owner_id).cloned() else { continue };
        let perm = match u.as_ref() {
            Some(v) if v.id == o.id || v.site_admin => Perm::Admin,
            _ => db::permission(&s.db, u.as_ref(), &r, &o).await?,
        };
        let c = RepoCtx { repo: r, owner: o, perm };
        items.push(crate::json::repo::repo(&s.api_url(), s.base_url(), &c));
    }
    Ok(Paged::new(&uri, &s, pages, Some(total), items).into_response())
}
