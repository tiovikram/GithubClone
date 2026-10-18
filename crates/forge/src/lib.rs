#![recursion_limit = "512"]
//! forge: a GitHub-compatible code forge. See docs/ARCHITECTURE.md.
pub mod activity;
pub mod advsec;
pub mod api;
pub mod apps;
pub mod auth;
pub mod config;
pub mod db;
pub mod enterprise;
pub mod error;
pub mod events;
pub mod extract;
pub mod forge_api;
pub mod git;
pub mod graphql;
pub mod hooks;
pub mod ids;
pub mod jobs;
pub mod migration;
pub mod json;
pub mod oauth_device;
pub mod orgs;
pub mod packages;
pub mod ssh;
pub mod pulls;
pub mod runner;
pub mod security;
pub mod protection;
pub mod rules;
pub mod signed_url;
pub mod state;
pub mod web;
pub mod webhooks;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::{middleware, Router};
use sqlx::postgres::PgPoolOptions;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::state::AppState;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Connects to Postgres and object storage and builds shared state.
pub async fn build_state(cfg: Config) -> anyhow::Result<AppState> {
    let db = PgPoolOptions::new()
        .max_connections(std::env::var("FORGE_DB_POOL").ok().and_then(|v| v.parse().ok()).unwrap_or(32))
        .acquire_timeout(Duration::from_secs(10))
        .connect(&cfg.database_url)
        .await?;
    if !cfg.skip_migrations {
        migrate(&db).await?;
    }
    ssh::register_config(&cfg);
    let store = state::build_object_store(&cfg.storage)?;
    let git = Arc::new(git::GitStore::new(db.clone(), store.clone(), cfg.cache_dir.clone(), cfg.git_bin.clone()));
    Ok(AppState { db, cfg: Arc::new(cfg), store, git })
}

/// Runs migrations under an advisory lock so concurrently starting pods don't race.
pub async fn migrate(db: &sqlx::PgPool) -> anyhow::Result<()> {
    let mut conn = db.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(7262620)").execute(&mut *conn).await?;
    let res = MIGRATOR.run(&mut *conn).await;
    sqlx::query("SELECT pg_advisory_unlock(7262620)").execute(&mut *conn).await?;
    res?;
    Ok(())
}

pub fn app(state: AppState) -> Router {
    let api = api::routes()
        .layer(middleware::from_fn_with_state(state.clone(), security::archived_read_only))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(api::github_headers));
    let graphql = graphql::routes().layer(middleware::from_fn(api::github_headers));
    Router::new()
        .route("/api/v3/", get(api::meta::root_handler))
        .nest("/api/v3", api)
        .nest("/api/graphql", graphql)
        .nest("/api/forge", forge_api::routes().merge(oauth_device::forge_routes()).merge(api::apps_forge::forge_routes()).merge(advsec::forge_routes()))
        .merge(oauth_device::routes())
        .nest(
            "/api/uploads",
            api::releases::upload_routes()
                .layer(middleware::from_fn_with_state(state.clone(), security::archived_read_only))
                .layer(middleware::from_fn(api::github_headers)),
        )
        .merge(api::releases::download_routes())
        .merge(api::gists::raw_routes())
        .merge(api::gist_git::routes())
        .route("/avatars/{login}", get(web::avatar))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .merge(git::http::routes())
        .merge(runner::protocol::routes())
        .merge(api::actions::download_routes())
        .merge(api::runner_runners::download_routes())
        .merge(api::branches::web_routes())
        .merge(api::repos_pages::web_routes())
        .merge(api::activity_extra::web_routes())
        .merge(packages::routes())
        .fallback(web::spa)
        .layer(middleware::from_fn_with_state(state.clone(), apps::scope::middleware))
        .layer(middleware::from_fn_with_state(state.clone(), api::keys::dot_keys_middleware))
        .layer(middleware::from_fn_with_state(state.clone(), enterprise::maintenance_guard))
        .layer(middleware::from_fn(security::nosniff))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn readyz(axum::extract::State(s): axum::extract::State<AppState>) -> axum::http::StatusCode {
    match sqlx::query("SELECT 1").execute(&s.db).await {
        Ok(_) => axum::http::StatusCode::OK,
        Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}
