//! Resolution of `GET /compare/{basehead}` sides, including other repositories of the same fork
//! network: `base...owner:branch` (owner's fork) and `base...owner:repo:branch`, either side.
//!
//! Cross-repository comparisons run in a throw-away bare repository whose object store borrows
//! both repositories' caches through `objects/info/alternates` (no objects are copied and
//! neither repository is modified).
use std::path::{Path, PathBuf};

use super::gitapi::{self, GitResult};
use crate::db::{self, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::git::read;
use crate::state::AppState;

/// Where and what to compare.
pub struct Sides {
    pub path: PathBuf,
    /// Revisions resolvable in `path` (shas for cross-repository sides).
    pub base: String,
    pub head: String,
    _tmp: Option<TempRepo>,
}

struct TempRepo(PathBuf);

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Repository ids of the fork network containing `repo_id`.
async fn network(s: &AppState, repo_id: i64) -> ApiResult<Vec<i64>> {
    let root: i64 = sqlx::query_scalar(
        "WITH RECURSIVE up(id, parent, depth) AS (
             SELECT id, fork_parent_id, 0 FROM repositories WHERE id = $1
             UNION ALL SELECT r.id, r.fork_parent_id, up.depth + 1 FROM repositories r JOIN up ON r.id = up.parent WHERE up.depth < 50)
         SELECT id FROM up ORDER BY depth DESC LIMIT 1",
    )
    .bind(repo_id)
    .fetch_one(&s.db)
    .await?;
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE net(id) AS (SELECT $1::bigint UNION SELECT r.id FROM repositories r JOIN net ON r.fork_parent_id = net.id)
         SELECT id FROM net",
    )
    .bind(root)
    .fetch_all(&s.db)
    .await?)
}

/// One side: `ref`, `owner:ref` or `owner:repo:ref`. Returns (repository, ref); None = this repo.
async fn side_repo(s: &AppState, viewer: Option<&User>, ctx: &RepoCtx, net: &mut Option<Vec<i64>>, side: &str) -> ApiResult<(Option<RepoCtx>, String)> {
    let Some((owner, rest)) = side.split_once(':') else { return Ok((None, side.to_string())) };
    let (repo_name, rev) = match rest.split_once(':') {
        Some((r, rev)) => (Some(r), rev),
        None => (None, rest),
    };
    let same_owner = owner.eq_ignore_ascii_case(&ctx.owner.login);
    if same_owner && repo_name.is_none_or(|r| r.eq_ignore_ascii_case(&ctx.repo.name)) {
        return Ok((None, rev.to_string()));
    }
    if net.is_none() {
        *net = Some(network(s, ctx.repo.id).await?);
    }
    let ids = net.as_ref().unwrap();
    let candidates: Vec<(i64, String)> = sqlx::query_as(
        "SELECT r.id, r.name FROM repositories r JOIN users u ON u.id = r.owner_id
          WHERE r.id = ANY($1) AND lower(u.login) = lower($2) ORDER BY (lower(r.name) = lower($3)) DESC, r.id",
    )
    .bind(ids)
    .bind(owner)
    .bind(repo_name.unwrap_or(&ctx.repo.name))
    .fetch_all(&s.db)
    .await?;
    let id = match repo_name {
        Some(n) => candidates.iter().find(|(_, name)| name.eq_ignore_ascii_case(n)).map(|c| c.0),
        None => candidates.first().map(|c| c.0),
    }
    .ok_or_else(ApiError::not_found)?;
    let other = db::load_repo_by_id(&s.db, viewer, id).await?;
    Ok((Some(other), rev.to_string()))
}

async fn resolve_in(s: &AppState, repo: &RepoCtx, rev: &str) -> ApiResult<(PathBuf, String)> {
    let path = gitapi::sync(s, repo).await?;
    let sha = read::resolve_commit(&s.git.git, &path, rev).await.api()?.ok_or_else(ApiError::not_found)?;
    Ok((path, sha))
}

fn objects_dir(repo: &Path) -> String {
    let abs = std::path::absolute(repo).unwrap_or_else(|_| repo.to_path_buf());
    abs.join("objects").to_string_lossy().into_owned()
}

/// Resolves both sides of a comparison in `ctx`'s fork network.
pub async fn resolve(s: &AppState, viewer: Option<&User>, ctx: &RepoCtx, base: &str, head: &str) -> ApiResult<Sides> {
    let mut net = None;
    let (base_repo, base_rev) = side_repo(s, viewer, ctx, &mut net, base).await?;
    let (head_repo, head_rev) = side_repo(s, viewer, ctx, &mut net, head).await?;
    if base_repo.is_none() && head_repo.is_none() {
        return Ok(Sides { path: gitapi::sync(s, ctx).await?, base: base_rev, head: head_rev, _tmp: None });
    }
    let (bp, bsha) = resolve_in(s, base_repo.as_ref().unwrap_or(ctx), &base_rev).await?;
    let (hp, hsha) = resolve_in(s, head_repo.as_ref().unwrap_or(ctx), &head_rev).await?;
    let dir = s.cfg.cache_dir.join(".forge-tmp");
    tokio::fs::create_dir_all(&dir).await.map_err(ApiError::internal)?;
    let tmp = TempRepo(dir.join(format!("cmp-{}.git", crate::auth::random_string(12).to_lowercase())));
    s.git.git.run(&dir, &["init", "-q", "--bare", tmp.0.to_str().unwrap_or_default()]).await.api()?;
    let alternates = format!("{}\n{}\n", objects_dir(&bp), objects_dir(&hp));
    tokio::fs::write(tmp.0.join("objects/info/alternates"), alternates).await.map_err(ApiError::internal)?;
    Ok(Sides { path: tmp.0.clone(), base: bsha, head: hsha, _tmp: Some(tmp) })
}
