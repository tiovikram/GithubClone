//! Row types and common lookups shared by REST, GraphQL and git.
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::{ApiError, ApiResult};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub login: String,
    pub kind: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password_hash: Option<String>,
    pub bio: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub blog: String,
    pub twitter_username: Option<String>,
    pub site_admin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Repo {
    pub id: i64,
    pub owner_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub private: bool,
    pub visibility: String,
    pub default_branch: String,
    pub fork_parent_id: Option<i64>,
    pub archived: bool,
    pub is_template: bool,
    pub has_issues: bool,
    pub has_projects: bool,
    pub has_wiki: bool,
    pub has_discussions: bool,
    pub allow_merge_commit: bool,
    pub allow_squash_merge: bool,
    pub allow_rebase_merge: bool,
    pub allow_auto_merge: bool,
    pub delete_branch_on_merge: bool,
    pub topics: Vec<String>,
    pub language: Option<String>,
    pub size_kb: i64,
    pub next_number: i64,
    pub stargazers_count: i64,
    pub forks_count: i64,
    pub open_issues_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub pushed_at: Option<DateTime<Utc>>,
}

/// Permission level of a user on a repository, ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Perm {
    None,
    Read,
    Triage,
    Write,
    Maintain,
    Admin,
}

impl Perm {
    pub fn from_str(s: &str) -> Perm {
        match s {
            "pull" | "read" => Perm::Read,
            "triage" => Perm::Triage,
            "push" | "write" => Perm::Write,
            "maintain" => Perm::Maintain,
            "admin" => Perm::Admin,
            _ => Perm::None,
        }
    }
    /// GitHub's `permission` string (e.g. collaborator permission endpoint).
    pub fn as_str(self) -> &'static str {
        match self {
            Perm::Admin => "admin",
            Perm::Maintain => "maintain",
            Perm::Write => "write",
            Perm::Triage => "triage",
            Perm::Read => "read",
            Perm::None => "none",
        }
    }
}

/// A repository resolved together with its owner and the caller's permission.
#[derive(Debug, Clone)]
pub struct RepoCtx {
    pub repo: Repo,
    pub owner: User,
    pub perm: Perm,
}

impl RepoCtx {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner.login, self.repo.name)
    }
    pub fn require(&self, p: Perm) -> ApiResult<()> {
        if self.perm >= p {
            Ok(())
        } else if self.perm >= Perm::Read {
            Err(ApiError::forbidden("Must have admin rights to Repository."))
                .map_err(|mut e| {
                    if p <= Perm::Write {
                        e.message = "Resource not accessible by personal access token".into();
                    }
                    e
                })
        } else {
            Err(ApiError::not_found())
        }
    }
}

pub async fn user_by_login(db: &PgPool, login: &str) -> ApiResult<User> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE lower(login) = lower($1)")
        .bind(login)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn user_by_id(db: &PgPool, id: i64) -> ApiResult<User> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn repo_by_id(db: &PgPool, id: i64) -> ApiResult<Repo> {
    sqlx::query_as::<_, Repo>("SELECT * FROM repositories WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Computes the permission `viewer` has on `repo` (owned by `owner`).
pub async fn permission(db: &PgPool, viewer: Option<&User>, repo: &Repo, owner: &User) -> ApiResult<Perm> {
    let base = if repo.private { Perm::None } else { Perm::Read };
    let Some(u) = viewer else { return Ok(base) };
    if u.site_admin || u.id == owner.id {
        return Ok(Perm::Admin);
    }
    let mut best = base;
    if owner.kind == "Organization" {
        let role: Option<String> =
            sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
                .bind(owner.id)
                .bind(u.id)
                .fetch_optional(db)
                .await?;
        match role.as_deref() {
            Some("admin") => return Ok(Perm::Admin),
            Some(_) => best = best.max(Perm::Write),
            None => {}
        }
    }
    let collab: Option<String> =
        sqlx::query_scalar("SELECT permission FROM repo_collaborators WHERE repo_id = $1 AND user_id = $2")
            .bind(repo.id)
            .bind(u.id)
            .fetch_optional(db)
            .await?;
    if let Some(p) = collab {
        best = best.max(Perm::from_str(&p));
    }
    Ok(best)
}

/// Resolve `owner/name` for `viewer`. Invisible repos are reported as 404.
pub async fn load_repo(db: &PgPool, viewer: Option<&User>, owner: &str, name: &str) -> ApiResult<RepoCtx> {
    let name = name.strip_suffix(".git").unwrap_or(name);
    let found = match user_by_login(db, owner).await {
        Ok(o) => sqlx::query_as::<_, Repo>("SELECT * FROM repositories WHERE owner_id = $1 AND lower(name) = lower($2)")
            .bind(o.id)
            .bind(name)
            .fetch_optional(db)
            .await?
            .map(|r| (o, r)),
        Err(_) => None,
    };
    let (owner, repo) = match found {
        Some(x) => x,
        // Transferred repositories keep answering at their old path (repos area).
        None => {
            let id: i64 = sqlx::query_scalar("SELECT repo_id FROM repo_redirects WHERE lower(owner_login) = lower($1) AND lower(name) = lower($2)")
                .bind(owner)
                .bind(name)
                .fetch_optional(db)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let repo = repo_by_id(db, id).await?;
            (user_by_id(db, repo.owner_id).await?, repo)
        }
    };
    let perm = permission(db, viewer, &repo, &owner).await?;
    if perm == Perm::None {
        return Err(ApiError::not_found());
    }
    Ok(RepoCtx { repo, owner, perm })
}

pub async fn load_repo_by_id(db: &PgPool, viewer: Option<&User>, id: i64) -> ApiResult<RepoCtx> {
    let repo = repo_by_id(db, id).await?;
    let owner = user_by_id(db, repo.owner_id).await?;
    let perm = permission(db, viewer, &repo, &owner).await?;
    if perm == Perm::None {
        return Err(ApiError::not_found());
    }
    Ok(RepoCtx { repo, owner, perm })
}

/// Allocates the next issue/PR number for a repository (shared sequence, like GitHub).
pub async fn next_issue_number<'e, E: sqlx::PgExecutor<'e>>(db: E, repo_id: i64) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("UPDATE repositories SET next_number = next_number + 1 WHERE id = $1 RETURNING next_number - 1")
        .bind(repo_id)
        .fetch_one(db)
        .await?)
}
