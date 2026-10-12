//! Batched loaders (async-graphql DataLoader) so connections of N items cost O(1) queries.
use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::{DataLoader, Loader};
use async_graphql::Context;

use crate::db::{self, RepoCtx, User};
use crate::json::issue::{IssueRow, LabelRow, MilestoneRow};
use crate::state::AppState;

use super::pull::PullRow;

pub struct Ld {
    pub state: AppState,
    pub viewer: Option<User>,
}

impl Ld {
    pub fn new(state: AppState, viewer: Option<User>) -> Self {
        Ld { state, viewer }
    }
    fn db(&self) -> &sqlx::PgPool {
        &self.state.db
    }
}

pub type LdErr = Arc<String>;

fn e(err: impl std::fmt::Display) -> LdErr {
    Arc::new(err.to_string())
}

pub fn loader<'a>(ctx: &'a Context<'_>) -> &'a DataLoader<Ld> {
    ctx.data_unchecked::<DataLoader<Ld>>()
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct UserId(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct RepoId(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct IssueId(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct LabelsOf(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct AssigneesOf(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct PullOf(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct MilestoneId(pub i64);
/// (subject_type is fixed by the key type) reaction counts for an issue/PR.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct IssueReactions(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct CommentReactions(pub i64);
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct CommentCount(pub i64);
/// (repo id, user id) -> author association.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct Assoc(pub i64, pub i64);

impl Loader<UserId> for Ld {
    type Value = Arc<User>;
    type Error = LdErr;
    async fn load(&self, keys: &[UserId]) -> Result<HashMap<UserId, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&ids).fetch_all(self.db()).await.map_err(e)?;
        Ok(users.into_iter().map(|u| (UserId(u.id), Arc::new(u))).collect())
    }
}

impl Loader<RepoId> for Ld {
    type Value = Arc<RepoCtx>;
    type Error = LdErr;
    async fn load(&self, keys: &[RepoId]) -> Result<HashMap<RepoId, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let repos: Vec<db::Repo> =
            sqlx::query_as("SELECT * FROM repositories WHERE id = ANY($1)").bind(&ids).fetch_all(self.db()).await.map_err(e)?;
        let owner_ids: Vec<i64> = repos.iter().map(|r| r.owner_id).collect();
        let owners: Vec<User> =
            sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&owner_ids).fetch_all(self.db()).await.map_err(e)?;
        let owners: HashMap<i64, User> = owners.into_iter().map(|u| (u.id, u)).collect();
        let mut out = HashMap::new();
        for repo in repos {
            let Some(owner) = owners.get(&repo.owner_id).cloned() else { continue };
            let perm = db::permission(self.db(), self.viewer.as_ref(), &repo, &owner).await.map_err(|x| e(x.message))?;
            if perm == db::Perm::None {
                continue;
            }
            out.insert(RepoId(repo.id), Arc::new(RepoCtx { repo, owner, perm }));
        }
        Ok(out)
    }
}

impl Loader<IssueId> for Ld {
    type Value = Arc<IssueRow>;
    type Error = LdErr;
    async fn load(&self, keys: &[IssueId]) -> Result<HashMap<IssueId, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let rows: Vec<IssueRow> = sqlx::query_as("SELECT * FROM issues WHERE id = ANY($1)").bind(&ids).fetch_all(self.db()).await.map_err(e)?;
        Ok(rows.into_iter().map(|r| (IssueId(r.id), Arc::new(r))).collect())
    }
}

impl Loader<LabelsOf> for Ld {
    type Value = Arc<Vec<LabelRow>>;
    type Error = LdErr;
    async fn load(&self, keys: &[LabelsOf]) -> Result<HashMap<LabelsOf, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let rows: Vec<(i64, i64, i64, String, String, Option<String>, bool, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT il.issue_id, l.id, l.repo_id, l.name, l.color, l.description, l.is_default, l.created_at
               FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = ANY($1) ORDER BY lower(l.name)",
        )
        .bind(&ids)
        .fetch_all(self.db())
        .await
        .map_err(e)?;
        let mut m: HashMap<i64, Vec<LabelRow>> = HashMap::new();
        for (issue_id, id, repo_id, name, color, description, is_default, created_at) in rows {
            m.entry(issue_id).or_default().push(LabelRow { id, repo_id, name, color, description, is_default, created_at });
        }
        Ok(keys.iter().map(|k| (*k, Arc::new(m.remove(&k.0).unwrap_or_default()))).collect())
    }
}

impl Loader<AssigneesOf> for Ld {
    type Value = Arc<Vec<Arc<User>>>;
    type Error = LdErr;
    async fn load(&self, keys: &[AssigneesOf]) -> Result<HashMap<AssigneesOf, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let pairs: Vec<(i64, i64)> =
            sqlx::query_as("SELECT issue_id, user_id FROM issue_assignees WHERE issue_id = ANY($1) ORDER BY user_id")
                .bind(&ids)
                .fetch_all(self.db())
                .await
                .map_err(e)?;
        let uids: Vec<i64> = pairs.iter().map(|p| p.1).collect();
        let users: Vec<User> = sqlx::query_as("SELECT * FROM users WHERE id = ANY($1)").bind(&uids).fetch_all(self.db()).await.map_err(e)?;
        let users: HashMap<i64, Arc<User>> = users.into_iter().map(|u| (u.id, Arc::new(u))).collect();
        let mut m: HashMap<i64, Vec<Arc<User>>> = HashMap::new();
        for (iid, uid) in pairs {
            if let Some(u) = users.get(&uid) {
                m.entry(iid).or_default().push(u.clone());
            }
        }
        Ok(keys.iter().map(|k| (*k, Arc::new(m.remove(&k.0).unwrap_or_default()))).collect())
    }
}

impl Loader<PullOf> for Ld {
    type Value = Arc<PullRow>;
    type Error = LdErr;
    async fn load(&self, keys: &[PullOf]) -> Result<HashMap<PullOf, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let rows: Vec<PullRow> =
            sqlx::query_as("SELECT * FROM pull_requests WHERE issue_id = ANY($1)").bind(&ids).fetch_all(self.db()).await.map_err(e)?;
        Ok(rows.into_iter().map(|r| (PullOf(r.issue_id), Arc::new(r))).collect())
    }
}

impl Loader<MilestoneId> for Ld {
    type Value = Arc<MilestoneRow>;
    type Error = LdErr;
    async fn load(&self, keys: &[MilestoneId]) -> Result<HashMap<MilestoneId, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let rows: Vec<MilestoneRow> =
            sqlx::query_as("SELECT * FROM milestones WHERE id = ANY($1)").bind(&ids).fetch_all(self.db()).await.map_err(e)?;
        Ok(rows.into_iter().map(|r| (MilestoneId(r.id), Arc::new(r))).collect())
    }
}

async fn reactions(db: &sqlx::PgPool, kind: &str, ids: &[i64]) -> Result<HashMap<i64, HashMap<String, i64>>, LdErr> {
    crate::json::issue::reaction_counts(db, kind, ids).await.map_err(|x| e(x.message))
}

impl Loader<IssueReactions> for Ld {
    type Value = Arc<HashMap<String, i64>>;
    type Error = LdErr;
    async fn load(&self, keys: &[IssueReactions]) -> Result<HashMap<IssueReactions, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let mut m = reactions(self.db(), "issue", &ids).await?;
        Ok(keys.iter().map(|k| (*k, Arc::new(m.remove(&k.0).unwrap_or_default()))).collect())
    }
}

impl Loader<CommentReactions> for Ld {
    type Value = Arc<HashMap<String, i64>>;
    type Error = LdErr;
    async fn load(&self, keys: &[CommentReactions]) -> Result<HashMap<CommentReactions, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let mut m = reactions(self.db(), "issue_comment", &ids).await?;
        Ok(keys.iter().map(|k| (*k, Arc::new(m.remove(&k.0).unwrap_or_default()))).collect())
    }
}

impl Loader<CommentCount> for Ld {
    type Value = i64;
    type Error = LdErr;
    async fn load(&self, keys: &[CommentCount]) -> Result<HashMap<CommentCount, Self::Value>, Self::Error> {
        let ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let rows: Vec<(i64, i64)> =
            sqlx::query_as("SELECT issue_id, count(*) FROM issue_comments WHERE issue_id = ANY($1) GROUP BY issue_id")
                .bind(&ids)
                .fetch_all(self.db())
                .await
                .map_err(e)?;
        let m: HashMap<i64, i64> = rows.into_iter().collect();
        Ok(keys.iter().map(|k| (*k, m.get(&k.0).copied().unwrap_or(0))).collect())
    }
}

impl Loader<Assoc> for Ld {
    type Value = String;
    type Error = LdErr;
    async fn load(&self, keys: &[Assoc]) -> Result<HashMap<Assoc, Self::Value>, Self::Error> {
        let repo_ids: Vec<i64> = keys.iter().map(|k| k.0).collect();
        let user_ids: Vec<i64> = keys.iter().map(|k| k.1).collect();
        // owner, org membership and collaborator status per (repo, user) pair in one query
        let rows: Vec<(i64, i64, bool, bool, bool)> = sqlx::query_as(
            "SELECT k.repo_id, k.user_id,
                    r.owner_id = k.user_id,
                    EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = k.user_id),
                    EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = k.user_id)
               FROM unnest($1::bigint[], $2::bigint[]) AS k(repo_id, user_id)
               JOIN repositories r ON r.id = k.repo_id",
        )
        .bind(&repo_ids)
        .bind(&user_ids)
        .fetch_all(self.db())
        .await
        .map_err(e)?;
        let mut out = HashMap::new();
        for (rid, uid, owner, member, collab) in rows {
            let a = if owner {
                "OWNER"
            } else if member {
                "MEMBER"
            } else if collab {
                "COLLABORATOR"
            } else {
                "NONE"
            };
            out.insert(Assoc(rid, uid), a.to_string());
        }
        Ok(out)
    }
}

/// Convenience accessors used by resolvers.
pub async fn user(ctx: &Context<'_>, id: Option<i64>) -> async_graphql::Result<Option<Arc<User>>> {
    match id {
        Some(id) => Ok(loader(ctx).load_one(UserId(id)).await.map_err(|x| async_graphql::Error::new(x.to_string()))?),
        None => Ok(None),
    }
}

pub async fn repo(ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<Arc<RepoCtx>>> {
    loader(ctx).load_one(RepoId(id)).await.map_err(|x| async_graphql::Error::new(x.to_string()))
}

pub fn lerr(x: LdErr) -> async_graphql::Error {
    async_graphql::Error::new(x.to_string())
}
