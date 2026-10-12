//! Read-only git access to gists (`git clone https://HOST/gist/ID.git`, used by `gh gist clone`).
//!
//! Gists live in Postgres (`gists`, `gist_history` snapshots). A pod materializes a bare
//! repository in its disposable cache from the history: one commit per snapshot, authored by the
//! snapshot's user at its timestamp, so every pod produces identical commit ids. The cache is
//! rebuilt when the gist has a newer snapshot. Pushing is not supported.
use std::path::{Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{Response, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use super::gists::{load_gist, GistRow, HistoryRow};
use crate::git::cmd::Identity;
use crate::git::http::{advertise, plain, serve_upload_pack};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    // `{login}` is the gist id here; the name matches `/gist/{login}/{gist_id}/raw/..`.
    Router::new()
        .route("/gist/{login}/info/refs", get(info_refs))
        .route("/gist/{login}/git-upload-pack", post(upload_pack))
        .route("/gist/{login}/git-receive-pack", post(receive_pack))
}

/// Builds (or reuses) the bare repository for `gist` and returns its path.
pub async fn materialize(s: &AppState, gist: &GistRow) -> anyhow::Result<PathBuf> {
    let history: Vec<HistoryRow> = sqlx::query_as("SELECT * FROM gist_history WHERE gist_id = $1 ORDER BY id").bind(gist.id).fetch_all(&s.db).await?;
    let stamp = history.last().map(|h| h.id).unwrap_or(0).to_string();
    let root = s.cfg.cache_dir.join("gists");
    tokio::fs::create_dir_all(&root).await?;
    let path = root.join(format!("{}.git", gist.id));
    let marker = path.join("forge-gist-version");
    if tokio::fs::read_to_string(&marker).await.ok().as_deref() == Some(stamp.as_str()) {
        return Ok(path);
    }
    let tmp = root.join(format!("{}.{}.tmp", gist.id, crate::auth::random_string(8)));
    let r = build(s, &tmp, &history).await;
    if let Err(e) = r {
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        return Err(e);
    }
    tokio::fs::write(tmp.join("forge-gist-version"), &stamp).await?;
    let _ = tokio::fs::remove_dir_all(&path).await;
    if tokio::fs::rename(&tmp, &path).await.is_err() {
        // Lost a race with a concurrent build of the same version: use theirs.
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
    Ok(path)
}

async fn build(s: &AppState, dir: &FsPath, history: &[HistoryRow]) -> anyhow::Result<()> {
    let git = &s.git.git;
    tokio::fs::create_dir_all(dir).await?;
    git.run(dir, &["init", "--bare", "-q", "--initial-branch=main", "."]).await?;
    let mut parent: Option<String> = None;
    for h in history {
        let files: std::collections::BTreeMap<String, String> = serde_json::from_value(h.files.clone()).unwrap_or_default();
        let mut listing = Vec::new();
        for (name, content) in &files {
            let blob = git.run_input(dir, &["hash-object", "-w", "--stdin"], content.as_bytes()).await?;
            let blob = String::from_utf8_lossy(&blob).trim().to_string();
            listing.extend_from_slice(format!("100644 blob {blob}\t{name}").as_bytes());
            listing.push(0);
        }
        let tree = git.run_input(dir, &["mktree", "-z"], &listing).await?;
        let tree = String::from_utf8_lossy(&tree).trim().to_string();
        let login: Option<String> = match h.user_id {
            Some(id) => sqlx::query_scalar("SELECT login FROM users WHERE id = $1").bind(id).fetch_optional(&s.db).await?,
            None => None,
        };
        let login = login.unwrap_or_else(|| "ghost".into());
        let who = Identity { email: format!("{login}@users.noreply.forge"), name: login, when: Some(h.committed_at.timestamp()) };
        let mut args = vec!["commit-tree", tree.as_str(), "-m", ""];
        if let Some(p) = &parent {
            args.extend(["-p", p.as_str()]);
        }
        let c = git.run_as(dir, &args, None, &who).await?;
        parent = Some(String::from_utf8_lossy(&c).trim().to_string());
    }
    if let Some(p) = parent {
        git.run(dir, &["update-ref", "refs/heads/main", &p]).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct InfoRefsQuery {
    service: Option<String>,
}

async fn repo_for(s: &AppState, id: &str) -> Result<PathBuf, Response<Body>> {
    let id = id.strip_suffix(".git").unwrap_or(id);
    let gist = load_gist(s, id).await.map_err(|_| plain(StatusCode::NOT_FOUND, "Repository not found"))?;
    materialize(s, &gist).await.map_err(|e| {
        tracing::error!(error = %e, "gist materialize failed");
        plain(StatusCode::INTERNAL_SERVER_ERROR, "git error")
    })
}

async fn info_refs(State(s): State<AppState>, Path(id): Path<String>, Query(q): Query<InfoRefsQuery>, req: Request) -> Response<Body> {
    match q.service.as_deref() {
        Some("git-upload-pack") => {}
        Some("git-receive-pack") => return plain(StatusCode::FORBIDDEN, "Pushing to gists is not supported"),
        _ => return plain(StatusCode::FORBIDDEN, "dumb HTTP protocol is not supported"),
    }
    match repo_for(&s, &id).await {
        Ok(path) => advertise(&s, &path, "git-upload-pack", req.headers()).await,
        Err(r) => r,
    }
}

async fn upload_pack(State(s): State<AppState>, Path(id): Path<String>, req: Request) -> Response<Body> {
    let (parts, body) = req.into_parts();
    match repo_for(&s, &id).await {
        Ok(path) => serve_upload_pack(&s, &path, &parts.headers, body),
        Err(r) => r,
    }
}

async fn receive_pack() -> Response<Body> {
    plain(StatusCode::FORBIDDEN, "Pushing to gists is not supported")
}
