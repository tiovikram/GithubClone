//! `Repository.object(expression:|oid:)` and the `Tree` / `Blob` git objects (used by
//! `gh repo read-dir`, which lists a directory via `object(expression: "REF:path") { ... on Tree }`).
use std::sync::Arc;

use async_graphql::{Context, Object, ID};

use super::enums::{GitObjectId, Uri};
use super::repo::{st, GCommit, GitObject};
use crate::db::RepoCtx;
use crate::ids;

/// Resolves a git revision expression (`REF:path`, `REF`, or an oid) in `repo` to a git object.
/// `HEAD` means the repository's default branch. Unknown refs/paths resolve to `None`.
pub async fn resolve(ctx: &Context<'_>, repo: Arc<RepoCtx>, expression: Option<String>, oid: Option<String>) -> async_graphql::Result<Option<GitObject>> {
    let s = st(ctx);
    let rev = match (expression, oid) {
        (Some(e), _) => {
            let (r, path) = match e.split_once(':') {
                Some((r, p)) => (r.to_string(), Some(p.trim_start_matches('/').to_string())),
                None => (e.clone(), None),
            };
            let r = if r.is_empty() || r == "HEAD" { format!("refs/heads/{}", repo.repo.default_branch) } else { r };
            if r.starts_with('-') {
                return Ok(None);
            }
            match path {
                Some(p) => format!("{r}:{p}"),
                None => r,
            }
        }
        (None, Some(o)) => o,
        (None, None) => return Ok(None),
    };
    let api = |e: crate::error::ApiError| async_graphql::Error::new(e.message);
    let Some(out) = crate::api::plat::git_read(s, repo.repo.id, &["rev-parse", "--verify", "--quiet", "--end-of-options", &rev]).await.map_err(api)? else {
        return Ok(None);
    };
    let sha = String::from_utf8_lossy(&out).trim().to_string();
    let Some(kind) = crate::api::plat::git_read(s, repo.repo.id, &["cat-file", "-t", &sha]).await.map_err(api)? else {
        return Ok(None);
    };
    let base = match rev.split_once(':') {
        Some((_, p)) => p.trim_end_matches('/').to_string(),
        None => String::new(),
    };
    Ok(match String::from_utf8_lossy(&kind).trim() {
        "commit" => Some(GitObject::Commit(GCommit::new(repo, sha))),
        "tree" => Some(GitObject::Tree(GTree { repo, oid: sha, path: base })),
        "blob" => Some(GitObject::Blob(GBlob { repo, oid: sha })),
        _ => None,
    })
}

#[derive(Clone)]
pub struct GTree {
    pub repo: Arc<RepoCtx>,
    pub oid: String,
    /// Path of this tree within the repository ("" for the root).
    pub path: String,
}

#[derive(Clone)]
pub struct GBlob {
    pub repo: Arc<RepoCtx>,
    pub oid: String,
}

pub struct TreeEntry {
    repo: Arc<RepoCtx>,
    name: String,
    path: String,
    kind: String,
    mode: i32,
    oid: String,
    size: i32,
}

#[Object(name = "Tree")]
impl GTree {
    pub async fn id(&self) -> ID {
        ID(ids::encode_str("Tree", &format!("{}:{}", self.repo.repo.id, self.oid)))
    }
    pub async fn oid(&self) -> GitObjectId {
        GitObjectId(self.oid.clone())
    }
    pub async fn abbreviated_oid(&self) -> String {
        self.oid.chars().take(7).collect()
    }
    pub async fn commit_url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/tree/{}", st(ctx).base_url(), self.repo.full_name(), self.oid))
    }
    async fn entries(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<Vec<TreeEntry>>> {
        let out = crate::api::plat::git_read(st(ctx), self.repo.repo.id, &["ls-tree", "-l", "-z", &self.oid])
            .await
            .map_err(|e| async_graphql::Error::new(e.message))?
            .unwrap_or_default();
        let mut entries = Vec::new();
        for rec in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let rec = String::from_utf8_lossy(rec);
            let Some((meta, name)) = rec.split_once('\t') else { continue };
            let f: Vec<&str> = meta.split_whitespace().collect();
            if f.len() < 4 {
                continue;
            }
            let path = if self.path.is_empty() { name.to_string() } else { format!("{}/{}", self.path, name) };
            entries.push(TreeEntry {
                repo: self.repo.clone(),
                name: name.to_string(),
                path,
                kind: f[1].to_string(),
                mode: i32::from_str_radix(f[0], 8).unwrap_or(0),
                oid: f[2].to_string(),
                size: f[3].parse().unwrap_or(0),
            });
        }
        Ok(Some(entries))
    }
}

#[Object(name = "TreeEntry")]
impl TreeEntry {
    async fn name(&self) -> &str {
        &self.name
    }
    async fn name_raw(&self) -> String {
        base64_std(self.name.as_bytes())
    }
    async fn path(&self) -> Option<&str> {
        Some(&self.path)
    }
    async fn path_raw(&self) -> Option<String> {
        Some(base64_std(self.path.as_bytes()))
    }
    #[graphql(name = "type")]
    async fn kind(&self) -> &str {
        &self.kind
    }
    async fn mode(&self) -> i32 {
        self.mode
    }
    async fn oid(&self) -> GitObjectId {
        GitObjectId(self.oid.clone())
    }
    async fn size(&self) -> i32 {
        self.size
    }
    async fn extension(&self) -> Option<String> {
        self.name.rsplit_once('.').filter(|(b, _)| !b.is_empty()).map(|(_, e)| format!(".{e}"))
    }
    async fn is_generated(&self) -> bool {
        false
    }
    async fn line_count(&self) -> Option<i32> {
        None
    }
    async fn object(&self) -> Option<GitObject> {
        match self.kind.as_str() {
            "tree" => Some(GitObject::Tree(GTree { repo: self.repo.clone(), oid: self.oid.clone(), path: self.path.clone() })),
            "blob" => Some(GitObject::Blob(GBlob { repo: self.repo.clone(), oid: self.oid.clone() })),
            _ => None,
        }
    }
    async fn submodule(&self) -> Option<Submodule> {
        (self.kind == "commit").then(|| Submodule { oid: self.oid.clone(), path: self.path.clone() })
    }
}

pub struct Submodule {
    oid: String,
    path: String,
}

#[Object]
impl Submodule {
    async fn git_url(&self) -> Uri {
        Uri(String::new())
    }
    async fn branch(&self) -> Option<String> {
        None
    }
    async fn name(&self) -> &str {
        &self.path
    }
    async fn path(&self) -> &str {
        &self.path
    }
    async fn subproject_commit_oid(&self) -> Option<GitObjectId> {
        Some(GitObjectId(self.oid.clone()))
    }
}

impl GBlob {
    async fn content(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<u8>> {
        Ok(crate::api::plat::git_read(st(ctx), self.repo.repo.id, &["cat-file", "blob", &self.oid])
            .await
            .map_err(|e| async_graphql::Error::new(e.message))?
            .unwrap_or_default())
    }
}

#[Object(name = "Blob")]
impl GBlob {
    pub async fn id(&self) -> ID {
        ID(ids::encode_str("Blob", &format!("{}:{}", self.repo.repo.id, self.oid)))
    }
    pub async fn oid(&self) -> GitObjectId {
        GitObjectId(self.oid.clone())
    }
    pub async fn abbreviated_oid(&self) -> String {
        self.oid.chars().take(7).collect()
    }
    pub async fn commit_url(&self, ctx: &Context<'_>) -> Uri {
        Uri(format!("{}/{}/blob/{}", st(ctx).base_url(), self.repo.full_name(), self.oid))
    }
    async fn byte_size(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        Ok(self.content(ctx).await?.len() as i32)
    }
    async fn is_binary(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<bool>> {
        Ok(Some(self.content(ctx).await?.contains(&0)))
    }
    async fn is_truncated(&self) -> bool {
        false
    }
    async fn text(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<String>> {
        let c = self.content(ctx).await?;
        Ok(if c.contains(&0) { None } else { Some(String::from_utf8_lossy(&c).into_owned()) })
    }
}

fn base64_std(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}
