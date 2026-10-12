//! Integration-test harness: boots forge on a random port against DATABASE_URL_TEST with
//! temporary object storage and cache. Tests create uniquely named users/repos so they can
//! share one database and run in parallel. Use #[tokio::test(flavor = "multi_thread")]: the git
//! helper blocks, and a single-threaded runtime would starve the in-process server.
#![allow(dead_code)]
use std::path::PathBuf;

use serde_json::{json, Value};

pub struct TestServer {
    pub base: String,
    pub api: String,
    pub dir: tempfile::TempDir,
    pub state: forge::state::AppState,
}

pub struct TestUser {
    pub login: String,
    pub token: String,
}

pub fn unique(prefix: &str) -> String {
    format!("{prefix}{}", forge::auth::random_string(8).to_lowercase())
}

impl TestServer {
    pub async fn start() -> TestServer {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let cfg = forge::config::Config {
            listen: format!("127.0.0.1:{port}"),
            base_url: base.clone(),
            database_url: std::env::var("DATABASE_URL_TEST")
                .unwrap_or_else(|_| "postgres://forge:forge@localhost/forge_test".into()),
            storage: format!("file://{}", dir.path().join("objects").display()),
            cache_dir: dir.path().join("cache"),
            git_bin: "git".into(),
            skip_migrations: false,
        };
        let state = forge::build_state(cfg).await.expect("build state");
        let app = forge::app(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        TestServer { api: format!("{base}/api/v3"), base, dir, state }
    }

    /// Creates a fresh user with a token.
    pub async fn user(&self) -> TestUser {
        let login = unique("u");
        let u = forge::forge_api::create_user(&self.state, &login, Some(&format!("{login}@example.com")), "password123", false)
            .await
            .map_err(|e| e.message)
            .unwrap();
        let (_, token) = forge::forge_api::create_token_for(&self.state, u.id, "test").await.map_err(|e| e.message).unwrap();
        TestUser { login, token }
    }

    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }

    /// Authenticated JSON request against /api/v3; returns (status, body).
    pub async fn call(&self, u: Option<&TestUser>, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let url = format!("{}{}", self.api, path);
        let mut req = self.client().request(method.parse().unwrap(), url);
        if let Some(u) = u {
            req = req.header("Authorization", format!("token {}", u.token));
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let text = res.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub async fn create_repo(&self, u: &TestUser, auto_init: bool) -> String {
        let name = unique("r");
        let (st, _) = self.call(Some(u), "POST", "/user/repos", Some(json!({"name": name, "auto_init": auto_init}))).await;
        assert_eq!(st, 201);
        name
    }

    /// Runs git with an isolated config (never touches the user's keychain/global config).
    pub fn git(&self, cwd: &std::path::Path, args: &[&str]) -> (bool, String) {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    pub fn clone_url(&self, u: &TestUser, repo: &str) -> String {
        format!("http://{}:{}@{}/{}/{}.git", u.login, u.token, self.base.trim_start_matches("http://"), u.login, repo)
    }

    pub fn work_dir(&self, name: &str) -> PathBuf {
        let p = self.dir.path().join("work").join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
