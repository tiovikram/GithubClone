use std::sync::Arc;

use object_store::ObjectStore;
use sqlx::PgPool;

use crate::config::Config;
use crate::git::GitStore;

/// Shared application state. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub cfg: Arc<Config>,
    pub store: Arc<dyn ObjectStore>,
    pub git: Arc<GitStore>,
}

impl AppState {
    pub fn api_url(&self) -> String {
        self.cfg.api_url()
    }
    pub fn base_url(&self) -> &str {
        &self.cfg.base_url
    }
}

/// Builds the object store from `FORGE_STORAGE` (`file:///path` or `s3://bucket[/prefix]`).
pub fn build_object_store(spec: &str) -> anyhow::Result<Arc<dyn ObjectStore>> {
    if let Some(path) = spec.strip_prefix("file://") {
        std::fs::create_dir_all(path)?;
        let fs = object_store::local::LocalFileSystem::new_with_prefix(path)?;
        return Ok(Arc::new(fs));
    }
    if let Some(rest) = spec.strip_prefix("s3://") {
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        let s3 = object_store::aws::AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .with_virtual_hosted_style_request(false)
            .build()?;
        if prefix.is_empty() {
            return Ok(Arc::new(s3));
        }
        let prefixed = object_store::prefix::PrefixStore::new(s3, prefix.trim_end_matches('/'));
        return Ok(Arc::new(prefixed));
    }
    anyhow::bail!("unsupported FORGE_STORAGE {spec:?}: use file:///path or s3://bucket/prefix")
}
