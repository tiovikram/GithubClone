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
    /// `FORGE_SSH_LISTEN`: start the embedded git-over-SSH server on this address.
    pub ssh_listen: Option<String>,
    /// `FORGE_SSH_HOST_KEY`: OpenSSH private key file; unset = shared key stored in Postgres.
    pub ssh_host_key: Option<PathBuf>,
    /// `FORGE_SSH_URL_HOST` (`host` or `host:port`) and `FORGE_SSH_URL_PORT`: what `ssh_url`
    /// in API responses points at. Default: base URL host, port of `FORGE_SSH_LISTEN`.
    pub ssh_url_host: Option<String>,
    pub ssh_url_port: Option<u16>,
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
            ssh_listen: env("FORGE_SSH_LISTEN"),
            ssh_host_key: env("FORGE_SSH_HOST_KEY").map(PathBuf::from),
            ssh_url_host: env("FORGE_SSH_URL_HOST"),
            ssh_url_port: env("FORGE_SSH_URL_PORT").and_then(|v| v.parse().ok()),
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

    /// Prefix of SSH clone URLs (`git@host:` on port 22, else `ssh://git@host:port/`), or
    /// `None` when SSH is not configured.
    pub fn ssh_url_prefix(&self) -> Option<String> {
        if self.ssh_listen.is_none() && self.ssh_url_host.is_none() {
            return None;
        }
        let base_host = url::Url::parse(&self.base_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "localhost".into());
        let (host, host_port) = match self.ssh_url_host.as_deref() {
            Some(h) => match h.rsplit_once(':') {
                Some((h, p)) if !h.contains(':') || h.starts_with('[') => (h.to_string(), p.parse::<u16>().ok()),
                _ => (h.to_string(), None),
            },
            None => (base_host, None),
        };
        let listen_port = self
            .ssh_listen
            .as_deref()
            .and_then(|l| l.rsplit_once(':'))
            .and_then(|(_, p)| p.parse::<u16>().ok());
        let port = self.ssh_url_port.or(host_port).or(listen_port).unwrap_or(22);
        Some(if port == 22 { format!("git@{host}:") } else { format!("ssh://git@{host}:{port}/") })
    }
}
