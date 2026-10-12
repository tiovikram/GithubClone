//! Git repository storage. Object storage is the source of truth (immutable packs); Postgres
//! `repo_storage` is the atomic commit point (refs + pack list + version); each pod keeps a
//! disposable bare-repo cache. See docs/ARCHITECTURE.md "Git storage".
pub mod cmd;
pub mod gc;
pub mod plumbing;
pub mod http;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use futures::StreamExt;
use object_store::path::Path as ObjPath;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use object_store::ObjectStore;
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;

pub use cmd::GitCmd;

/// Ref name -> object id.
pub type Refs = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct Manifest {
    pub version: i64,
    pub head: String,
    pub refs: String,
    pub packs: Vec<String>,
}

/// What a write changed: refs before and after (for push handlers, PR updates, events).
#[derive(Debug, Clone, Default)]
pub struct RefUpdates {
    pub before: Refs,
    pub after: Refs,
}

impl RefUpdates {
    /// (ref, old, new) for every changed ref; missing side is the zero id.
    pub fn changed(&self) -> Vec<(String, String, String)> {
        const ZERO: &str = "0000000000000000000000000000000000000000";
        let mut out = Vec::new();
        let names: HashSet<&String> = self.before.keys().chain(self.after.keys()).collect();
        for n in names {
            let old = self.before.get(n).map(String::as_str).unwrap_or(ZERO);
            let new = self.after.get(n).map(String::as_str).unwrap_or(ZERO);
            if old != new {
                out.push((n.clone(), old.to_string(), new.to_string()));
            }
        }
        out.sort();
        out
    }
}

pub struct GitStore {
    db: PgPool,
    store: Arc<dyn ObjectStore>,
    cache_dir: PathBuf,
    pub git: GitCmd,
    /// Per-repo in-process locks guarding local cache mutation.
    local_locks: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
}

fn pack_key(repo_id: i64, file: &str) -> ObjPath {
    ObjPath::from(format!("repos/{repo_id}/packs/{file}"))
}

pub fn parse_refs(text: &str) -> Refs {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
        .filter_map(|l| l.split_once(' '))
        .map(|(sha, name)| (name.trim().to_string(), sha.trim().to_string()))
        .collect()
}

impl GitStore {
    pub fn new(db: PgPool, store: Arc<dyn ObjectStore>, cache_dir: PathBuf, git_bin: String) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        GitStore { db, store, cache_dir, git: GitCmd::new(git_bin), local_locks: Mutex::new(HashMap::new()) }
    }

    pub fn repo_path(&self, repo_id: i64) -> PathBuf {
        self.cache_dir.join(format!("{repo_id}.git"))
    }

    fn local_lock(&self, repo_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        self.local_locks.lock().unwrap().entry(repo_id).or_default().clone()
    }

    /// Creates the storage manifest for a new, empty repository.
    pub async fn init_repo(&self, repo_id: i64, default_branch: &str) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO repo_storage (repo_id, head) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(repo_id)
            .bind(format!("refs/heads/{default_branch}"))
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Marks a repository's objects for deletion and drops the local cache.
    pub async fn delete_repo(&self, repo_id: i64) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO storage_trash (prefix) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(format!("repos/{repo_id}/"))
            .execute(&self.db)
            .await?;
        let _ = tokio::fs::remove_dir_all(self.repo_path(repo_id)).await;
        Ok(())
    }

    pub async fn manifest(&self, repo_id: i64) -> anyhow::Result<Manifest> {
        let row: Option<(i64, String, String, Vec<String>)> =
            sqlx::query_as("SELECT version, head, refs, packs FROM repo_storage WHERE repo_id = $1")
                .bind(repo_id)
                .fetch_optional(&self.db)
                .await?;
        let Some((version, head, refs, packs)) = row else {
            bail!("repository {repo_id} has no storage manifest")
        };
        Ok(Manifest { version, head, refs, packs })
    }

    /// Committed refs straight from Postgres (no cache sync needed).
    pub async fn refs(&self, repo_id: i64) -> anyhow::Result<(String, Refs)> {
        let m = self.manifest(repo_id).await?;
        Ok((m.head, parse_refs(&m.refs)))
    }

    /// Ensures the local cache reflects the committed manifest; returns the bare repo path.
    pub async fn sync(&self, repo_id: i64) -> anyhow::Result<PathBuf> {
        let m = self.manifest(repo_id).await?;
        let lock = self.local_lock(repo_id);
        let _g = lock.lock().await;
        self.sync_locked(repo_id, &m).await
    }

    async fn sync_locked(&self, repo_id: i64, m: &Manifest) -> anyhow::Result<PathBuf> {
        let path = self.repo_path(repo_id);
        if !path.join("HEAD").exists() {
            self.create_local(&path).await?;
        }
        let version_file = path.join("forge-version");
        let local: i64 = tokio::fs::read_to_string(&version_file)
            .await
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(-1);
        if local == m.version {
            return Ok(path);
        }
        let pack_dir = path.join("objects/pack");
        tokio::fs::create_dir_all(&pack_dir).await?;
        for pack in &m.packs {
            if pack_dir.join(format!("{pack}.idx")).exists() {
                continue;
            }
            // .pack first, .idx last: git only considers packs that have an index.
            for ext in ["pack", "idx"] {
                let file = format!("{pack}.{ext}");
                self.download(&pack_key(repo_id, &file), &pack_dir.join(&file)).await?;
            }
        }
        self.write_refs(&path, &m.head, &m.refs).await?;
        write_atomic(&version_file, m.version.to_string().as_bytes()).await?;
        Ok(path)
    }

    async fn create_local(&self, path: &Path) -> anyhow::Result<()> {
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        self.git.run(&self.cache_dir, &["init", "--bare", "-q", tmp.to_str().unwrap()]).await?;
        for (k, v) in [
            ("receive.unpackLimit", "1"),
            ("transfer.unpackLimit", "1"),
            ("pack.writeReverseIndex", "false"),
            ("gc.auto", "0"),
            ("receive.autogc", "false"),
            ("receive.denyNonFastForwards", "false"),
            ("core.logAllRefUpdates", "false"),
            ("uploadpack.allowFilter", "true"),
            ("uploadpack.allowAnySHA1InWant", "true"),
            ("http.receivepack", "true"),
            ("core.bigFileThreshold", "512m"),
        ] {
            self.git.run(&tmp, &["config", k, v]).await?;
        }
        match tokio::fs::rename(&tmp, path).await {
            Ok(()) => Ok(()),
            Err(_) if path.join("HEAD").exists() => {
                let _ = tokio::fs::remove_dir_all(&tmp).await;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Replaces all refs in the local cache with `refs` (packed-refs text) and sets HEAD.
    async fn write_refs(&self, path: &Path, head: &str, refs: &str) -> anyhow::Result<()> {
        let mut body = String::from("# pack-refs with: peeled fully-peeled sorted \n");
        let mut sorted: Vec<(&str, &str)> = refs
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(sha, name)| (name, sha))
            .collect();
        sorted.sort();
        for (name, sha) in sorted {
            body.push_str(&format!("{sha} {name}\n"));
        }
        write_atomic(&path.join("packed-refs"), body.as_bytes()).await?;
        // Loose refs would shadow packed ones; the manifest is authoritative.
        let refs_dir = path.join("refs");
        let _ = tokio::fs::remove_dir_all(&refs_dir).await;
        for d in ["heads", "tags"] {
            tokio::fs::create_dir_all(refs_dir.join(d)).await?;
        }
        write_atomic(&path.join("HEAD"), format!("ref: {head}\n").as_bytes()).await?;
        Ok(())
    }

    async fn download(&self, key: &ObjPath, dest: &Path) -> anyhow::Result<()> {
        let tmp = dest.with_extension(format!("dl{}", std::process::id()));
        let mut f = tokio::fs::File::create(&tmp).await?;
        let mut stream = self.store.get(key).await.with_context(|| format!("get {key}"))?.into_stream();
        while let Some(chunk) = stream.next().await {
            f.write_all(&chunk?).await?;
        }
        f.flush().await?;
        f.sync_all().await?;
        drop(f);
        tokio::fs::rename(&tmp, dest).await?;
        Ok(())
    }

    async fn upload(&self, src: &Path, key: &ObjPath) -> anyhow::Result<()> {
        let mut f = tokio::fs::File::open(src).await?;
        let mut w = object_store::buffered::BufWriter::new(self.store.clone(), key.clone());
        tokio::io::copy(&mut f, &mut w).await?;
        w.shutdown().await.with_context(|| format!("put {key}"))?;
        Ok(())
    }

    fn local_packs(path: &Path) -> HashSet<String> {
        std::fs::read_dir(path.join("objects/pack"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter_map(|n| n.strip_suffix(".idx").map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Runs a mutating git operation under the repo's cluster-wide lock and commits the result
    /// to object storage + Postgres atomically. `op` gets the synced bare repo path.
    pub async fn write<T, F, Fut>(&self, repo_id: i64, op: F) -> anyhow::Result<(T, RefUpdates)>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        let mut conn = self.db.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock($1)").bind(repo_id).execute(&mut *conn).await?;
        let result = self.write_locked(repo_id, op).await;
        let unlock = sqlx::query("SELECT pg_advisory_unlock($1)").bind(repo_id).execute(&mut *conn).await;
        if unlock.is_err() {
            // Never return a connection that might still hold the lock to the pool.
            conn.detach();
        }
        result
    }

    async fn write_locked<T, F, Fut>(&self, repo_id: i64, op: F) -> anyhow::Result<(T, RefUpdates)>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        let m = self.manifest(repo_id).await?;
        let lock = self.local_lock(repo_id);
        let _g = lock.lock().await;
        let path = self.sync_locked(repo_id, &m).await?;
        let version_file = path.join("forge-version");
        // Until committed, the local cache may diverge from the manifest.
        let _ = tokio::fs::remove_file(&version_file).await;
        let before = parse_refs(&m.refs);
        let manifest_packs: HashSet<String> = m.packs.iter().cloned().collect();
        // Packs not in the manifest (left by a failed write) must not satisfy object lookups,
        // or `repack -d` could drop loose objects that only exist in an unuploaded pack.
        for stale in Self::local_packs(&path).difference(&manifest_packs) {
            for ext in ["idx", "pack", "rev", "bitmap"] {
                let _ = tokio::fs::remove_file(path.join("objects/pack").join(format!("{stale}.{ext}"))).await;
            }
        }

        let out = match op(path.clone()).await {
            Ok(v) => v,
            Err(e) => {
                let _ = self.write_refs(&path, &m.head, &m.refs).await;
                let _ = write_atomic(&version_file, m.version.to_string().as_bytes()).await;
                return Err(e);
            }
        };

        // Pack any loose objects the op created (API commits, merges).
        self.git.run(&path, &["repack", "-d", "-q"]).await?;
        let head = self
            .git
            .run(&path, &["symbolic-ref", "HEAD"])
            .await
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .unwrap_or(m.head.clone());
        let refs_text = String::from_utf8(
            self.git.run(&path, &["for-each-ref", "--format=%(objectname) %(refname)"]).await?,
        )?;
        let after = parse_refs(&refs_text);

        let local_after = Self::local_packs(&path);
        let mut new_packs: Vec<String> = local_after.difference(&manifest_packs).cloned().collect();
        new_packs.sort();
        for p in &new_packs {
            for ext in ["pack", "idx"] {
                let file = format!("{p}.{ext}");
                self.upload(&path.join("objects/pack").join(&file), &pack_key(repo_id, &file)).await?;
            }
        }
        // Manifest packs the op removed (full repack) drop out; new packs are appended.
        let mut packs: Vec<String> = m.packs.iter().filter(|p| local_after.contains(*p)).cloned().collect();
        packs.extend(new_packs.iter().cloned());
        let new_version: i64 = sqlx::query_scalar(
            "UPDATE repo_storage SET version = version + 1, head = $2, refs = $3, packs = $4, updated_at = now()
              WHERE repo_id = $1 AND version = $5 RETURNING version",
        )
        .bind(repo_id)
        .bind(&head)
        .bind(&refs_text)
        .bind(&packs)
        .bind(m.version)
        .fetch_optional(&self.db)
        .await?
        .context("repo_storage changed while locked (lock lost?)")?;
        // Normalize the local cache to the committed state (packed refs only).
        self.write_refs(&path, &head, &refs_text).await?;
        write_atomic(&version_file, new_version.to_string().as_bytes()).await?;
        Ok((out, RefUpdates { before, after }))
    }

    /// Repacks a repository into a single pack. Old pack objects are deleted later by gc once
    /// no manifest references them.
    pub async fn repack(&self, repo_id: i64) -> anyhow::Result<()> {
        let git = self.git.clone();
        self.write(repo_id, |path| async move {
            git.run(&path, &["repack", "-a", "-d", "-q"]).await?;
            Ok(())
        })
        .await?;
        Ok(())
    }
}

async fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", rand_suffix()));
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

fn rand_suffix() -> String {
    crate::auth::random_string(8)
}
