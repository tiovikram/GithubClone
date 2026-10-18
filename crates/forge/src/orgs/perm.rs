//! Effective repository permission inside an organization.
//!
//! GitHub's rules, which `db::permission` delegates to for org-owned repositories:
//! - org owners (`org_members.role = 'admin'`) are admins of every repository;
//! - other members get the org's *base permission* (`default_repository_permission`, default
//!   `read`; `none` gives no access to private repositories beyond what is granted below);
//! - plus the permission of every team they belong to (and of the teams' ancestors: child teams
//!   inherit their parents' repository access);
//! - plus the base role of organization roles assigned to them directly or through a team
//!   (`all_repo_*`, `security_manager`, custom roles with a `base_role`);
//! - plus a direct collaborator grant (also how outside collaborators get access).
//!
//! Team and collaborator grants may name a custom repository role; it counts as its base role.
//! The highest level wins.
use std::collections::HashMap;

use sqlx::PgPool;

use crate::db::Perm;
use crate::error::ApiResult;

/// Base permission level for org members from the org setting string.
pub fn base_perm(setting: &str) -> Perm {
    match setting {
        "admin" => Perm::Admin,
        "write" => Perm::Write,
        "read" => Perm::Read,
        _ => Perm::None,
    }
}

/// The org's `default_repository_permission` (GitHub's default `read`).
pub async fn default_permission(db: &PgPool, org_id: i64) -> ApiResult<String> {
    Ok(sqlx::query_scalar("SELECT default_repository_permission FROM org_settings WHERE org_id = $1")
        .bind(org_id)
        .fetch_optional(db)
        .await?
        .unwrap_or_else(|| "read".to_string()))
}

/// Resolves a stored grant (built-in level or custom repository role name) to a level.
pub fn resolve_grant(grant: &str, custom: &HashMap<String, String>) -> Perm {
    match Perm::from_str(grant) {
        Perm::None => custom.get(&grant.to_lowercase()).map(|b| Perm::from_str(b)).unwrap_or(Perm::None),
        p => p,
    }
}

/// `lower(name) -> base_role` of the org's custom repository roles.
pub async fn custom_roles(db: &PgPool, org_id: i64) -> ApiResult<HashMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT lower(name), base_role FROM custom_repo_roles WHERE org_id = $1")
        .bind(org_id)
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Per user: (org role if a member, every grant that applies to `repo_id`).
async fn grants(db: &PgPool, org_id: i64, repo_id: i64, user_ids: &[i64]) -> ApiResult<Vec<(i64, Option<String>, Vec<String>)>> {
    Ok(sqlx::query_as(
        "WITH RECURSIVE u AS (SELECT DISTINCT unnest($3::bigint[]) AS user_id),
         mt(user_id, team_id) AS (
             SELECT tm.user_id, tm.team_id FROM team_members tm JOIN teams t ON t.id = tm.team_id
              WHERE t.org_id = $1 AND tm.user_id IN (SELECT user_id FROM u)
             UNION
             SELECT mt.user_id, t.parent_id FROM mt JOIN teams t ON t.id = mt.team_id WHERE t.parent_id IS NOT NULL
         )
         SELECT u.user_id,
                (SELECT m.role FROM org_members m WHERE m.org_id = $1 AND m.user_id = u.user_id),
                ARRAY(SELECT tr.permission FROM team_repos tr JOIN mt ON mt.team_id = tr.team_id
                       WHERE mt.user_id = u.user_id AND tr.repo_id = $2)
                || ARRAY(SELECT c.permission FROM repo_collaborators c WHERE c.repo_id = $2 AND c.user_id = u.user_id)
                || ARRAY(SELECT r.base_role FROM org_roles r
                          WHERE r.base_role IS NOT NULL AND (r.org_id IS NULL OR r.org_id = $1)
                            AND (EXISTS(SELECT 1 FROM org_role_users ru WHERE ru.org_id = $1 AND ru.role_id = r.id AND ru.user_id = u.user_id)
                              OR EXISTS(SELECT 1 FROM org_role_teams rt JOIN mt ON mt.team_id = rt.team_id
                                         WHERE rt.org_id = $1 AND rt.role_id = r.id AND mt.user_id = u.user_id)))
           FROM u",
    )
    .bind(org_id)
    .bind(repo_id)
    .bind(user_ids)
    .fetch_all(db)
    .await?)
}

/// Effective permission of each user in `user_ids` on an org-owned repository, ignoring
/// repository visibility (the caller adds `Read` for public repositories).
pub async fn org_repo_permissions(db: &PgPool, org_id: i64, repo_id: i64, user_ids: &[i64]) -> ApiResult<HashMap<i64, Perm>> {
    let rows = grants(db, org_id, repo_id, user_ids).await?;
    let base = default_permission(db, org_id).await?;
    let needs_custom = rows.iter().any(|(_, _, g)| g.iter().any(|x| Perm::from_str(x) == Perm::None));
    let custom = if needs_custom { custom_roles(db, org_id).await? } else { HashMap::new() };
    let mut out = HashMap::new();
    for (uid, role, gs) in rows {
        let mut p = match role.as_deref() {
            Some("admin") => Perm::Admin,
            Some(_) => base_perm(&base),
            None => Perm::None,
        };
        for g in &gs {
            p = p.max(resolve_grant(g, &custom));
        }
        out.insert(uid, p);
    }
    Ok(out)
}

/// Effective permission of one user on an org-owned repository (visibility not included).
pub async fn org_repo_permission(db: &PgPool, org_id: i64, repo_id: i64, user_id: i64) -> ApiResult<Perm> {
    Ok(org_repo_permissions(db, org_id, repo_id, &[user_id]).await?.remove(&user_id).unwrap_or(Perm::None))
}

/// Effective permission of `viewer` on each of the given org repositories, including
/// visibility (public repositories are readable by everyone) and site admins.
pub async fn org_repo_permissions_for_repos(db: &PgPool, org_id: i64, repo_ids: &[i64], viewer: &crate::db::User) -> ApiResult<HashMap<i64, Perm>> {
    let repos: Vec<(i64, bool)> = sqlx::query_as("SELECT id, private FROM repositories WHERE id = ANY($1) AND owner_id = $2")
        .bind(repo_ids)
        .bind(org_id)
        .fetch_all(db)
        .await?;
    let mut out = HashMap::new();
    for (id, private) in repos {
        let mut p = if viewer.site_admin { Perm::Admin } else { org_repo_permission(db, org_id, id, viewer.id).await? };
        if !private {
            p = p.max(Perm::Read);
        }
        out.insert(id, p);
    }
    Ok(out)
}

/// The permission a team (with its ancestors) has on a repository, and the grant string
/// (`pull`/`push`/…, or a custom role name) of the strongest grant.
pub async fn team_repo_permission(db: &PgPool, org_id: i64, team_id: i64, repo_id: i64) -> ApiResult<Option<(Perm, String)>> {
    let grants: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE anc(id) AS (SELECT $1::bigint UNION SELECT t.parent_id FROM teams t JOIN anc ON t.id = anc.id WHERE t.parent_id IS NOT NULL)
         SELECT permission FROM team_repos WHERE repo_id = $2 AND team_id IN (SELECT id FROM anc)",
    )
    .bind(team_id)
    .bind(repo_id)
    .fetch_all(db)
    .await?;
    if grants.is_empty() {
        return Ok(None);
    }
    let custom = custom_roles(db, org_id).await?;
    Ok(grants.into_iter().map(|g| (resolve_grant(&g, &custom), g)).max_by_key(|(p, _)| *p))
}
