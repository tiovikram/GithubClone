use std::path::PathBuf;

/// Environment-only configuration (see docs/ARCHITECTURE.md).
#[derive(Clone, Debug)]
pub struct Config {
    pub listen: String,
    /// External base URL without trailing slash, e.g. `http://localhost:3000`.
    pub base_url: String,
    pub database_url: String,
    pub storage: String,
    pub cache_dir: PathBuf,
    pub git_bin: String,
    pub skip_migrations: bool,
}

impl Config {
    pub fn from_env() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Config {
            listen: env("FORGE_LISTEN").unwrap_or_else(|| "0.0.0.0:3000".into()),
            base_url: env("FORGE_BASE_URL")
                .unwrap_or_else(|| "http://localhost:3000".into())
                .trim_end_matches('/')
                .to_string(),
            database_url: env("DATABASE_URL")
                .unwrap_or_else(|| "postgres://forge:forge@localhost/forge".into()),
            storage: env("FORGE_STORAGE")
                .unwrap_or_else(|| format!("file://{}", cwd.join("data/objects").display())),
            cache_dir: env("FORGE_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| cwd.join("data/cache")),
            git_bin: env("FORGE_GIT_BIN").unwrap_or_else(|| "git".into()),
            skip_migrations: env("FORGE_SKIP_MIGRATIONS").is_some_and(|v| v == "1"),
        }
    }

    /// `{base}/api/v3`
    pub fn api_url(&self) -> String {
        format!("{}/api/v3", self.base_url)
    }

    /// Host (and port) part of the base URL, e.g. `localhost:3000`.
    pub fn host(&self) -> String {
        url::Url::parse(&self.base_url)
            .ok()
            .and_then(|u| {
                u.host_str()
                    .map(|h| match u.port() {
                        Some(p) => format!("{h}:{p}"),
                        None => h.to_string(),
                    })
            })
            .unwrap_or_else(|| "localhost".into())
    }
}
