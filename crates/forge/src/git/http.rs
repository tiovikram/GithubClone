//! Git smart HTTP (protocol v0/v1/v2) backed by `git upload-pack/receive-pack --stateless-rpc`.
use std::process::Stdio;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use futures::TryStreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::auth::{authenticate, AuthResult};
use crate::db::{self, Perm, RepoCtx};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{owner}/{repo}/info/refs", get(info_refs))
        .route("/{owner}/{repo}/git-upload-pack", post(upload_pack))
        .route("/{owner}/{repo}/git-receive-pack", post(receive_pack))
}

#[derive(Deserialize)]
struct InfoRefsQuery {
    service: Option<String>,
}

pub(crate) fn plain(status: StatusCode, msg: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(format!("{msg}\n")))
        .unwrap()
}

fn auth_challenge() -> Response<Body> {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, "Basic realm=\"forge\"")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("Authentication required\n"))
        .unwrap()
}

/// Resolves the repo and checks access. Anonymous callers get a Basic challenge so stock git
/// prompts for (or supplies) credentials.
async fn authorize(
    state: &AppState,
    parts: &mut axum::http::request::Parts,
    owner: &str,
    repo: &str,
    need: Perm,
) -> Result<RepoCtx, Response<Body>> {
    let auth = authenticate(parts, state).await.map_err(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "auth error"))?;
    let user = match auth {
        AuthResult::User(u) => Some(u),
        AuthResult::Anonymous => None,
        AuthResult::Invalid => return Err(auth_challenge()),
    };
    match db::load_repo(&state.db, user.as_ref(), owner, repo).await {
        Ok(ctx) if ctx.perm >= need => Ok(ctx),
        Ok(_) | Err(_) if user.is_none() => Err(auth_challenge()),
        Ok(_) => Err(plain(StatusCode::FORBIDDEN, "Permission denied")),
        Err(_) => Err(plain(StatusCode::NOT_FOUND, "Repository not found")),
    }
}

fn pkt_line(s: &str) -> Vec<u8> {
    format!("{:04x}{s}", s.len() + 4).into_bytes()
}

fn protocol_env(headers: &HeaderMap) -> Option<String> {
    headers.get("git-protocol").and_then(|v| v.to_str().ok()).map(str::to_string)
}

async fn info_refs(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<InfoRefsQuery>,
    req: axum::extract::Request,
) -> Response<Body> {
    let (mut parts, _) = req.into_parts();
    let service = match q.service.as_deref() {
        Some(s @ ("git-upload-pack" | "git-receive-pack")) => s.to_string(),
        _ => return plain(StatusCode::FORBIDDEN, "dumb HTTP protocol is not supported"),
    };
    let need = if service == "git-receive-pack" { Perm::Write } else { Perm::Read };
    let ctx = match authorize(&state, &mut parts, &owner, &repo, need).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let path = match state.git.sync(ctx.repo.id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "sync failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable");
        }
    };
    advertise(&state, &path, &service, &parts.headers).await
}

/// The `info/refs?service=...` response for the bare repository at `path` (also used for gists).
pub(crate) async fn advertise(state: &AppState, path: &std::path::Path, service: &str, headers: &HeaderMap) -> Response<Body> {
    let sub = service.trim_start_matches("git-");
    let mut cmd = state.git.git.command(path);
    cmd.args([sub, "--stateless-rpc", "--advertise-refs", "."]);
    let proto = protocol_env(headers);
    if let Some(p) = &proto {
        cmd.env("GIT_PROTOCOL", p);
    }
    let out = match cmd.output().await {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            tracing::error!(stderr = %String::from_utf8_lossy(&o.stderr), "advertise failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "git error");
        }
        Err(e) => {
            tracing::error!(error = %e, "spawn failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "git error");
        }
    };
    let mut body = Vec::new();
    // Protocol v2 responses start directly with the capability advertisement.
    if !proto.as_deref().unwrap_or("").contains("version=2") {
        body.extend(pkt_line(&format!("# service={service}\n")));
        body.extend(b"0000");
    }
    body.extend(out);
    Response::builder()
        .header(header::CONTENT_TYPE, format!("application/x-{service}-advertisement"))
        .header(header::CACHE_CONTROL, "no-cache, max-age=0, must-revalidate")
        .body(Body::from(body))
        .unwrap()
}

/// Request body as an AsyncRead, transparently gunzipping `Content-Encoding: gzip`.
fn body_reader(headers: &HeaderMap, body: Body) -> Box<dyn tokio::io::AsyncRead + Send + Unpin> {
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    let gz = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    if gz {
        Box::new(async_compression::tokio::bufread::GzipDecoder::new(tokio::io::BufReader::new(reader)))
    } else {
        Box::new(reader)
    }
}

async fn upload_pack(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Response<Body> {
    let (mut parts, body) = req.into_parts();
    let ctx = match authorize(&state, &mut parts, &owner, &repo, Perm::Read).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let path = match state.git.sync(ctx.repo.id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "sync failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "storage unavailable");
        }
    };
    serve_upload_pack(&state, &path, &parts.headers, body)
}

/// Streams `git upload-pack --stateless-rpc` for the bare repository at `path` (also used for gists).
pub(crate) fn serve_upload_pack(state: &AppState, path: &std::path::Path, headers: &HeaderMap, body: Body) -> Response<Body> {
    let mut cmd = state.git.git.command(path);
    cmd.args(["upload-pack", "--stateless-rpc", "."])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(p) = protocol_env(headers) {
        cmd.env("GIT_PROTOCOL", p);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "git error"),
    };
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = body_reader(headers, body);
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut reader, &mut stdin).await;
        let _ = stdin.shutdown().await;
        drop(stdin);
    });
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-git-upload-pack-result")
        .header(header::CACHE_CONTROL, "no-cache, max-age=0, must-revalidate")
        .body(Body::from_stream(ReaderStream::new(stdout)))
        .unwrap()
}

async fn receive_pack(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Response<Body> {
    let (mut parts, body) = req.into_parts();
    let ctx = match authorize(&state, &mut parts, &owner, &repo, Perm::Write).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if ctx.repo.archived {
        return plain(StatusCode::FORBIDDEN, "This repository was archived so it is read-only.");
    }
    let pusher = match crate::auth::authenticate(&mut parts, &state).await {
        Ok(AuthResult::User(u)) => u,
        _ => return auth_challenge(),
    };
    let proto = protocol_env(&parts.headers);
    let mut reader = body_reader(&parts.headers, body);
    let git = state.git.git.clone();
    let repo_id = ctx.repo.id;
    // The report-status output is buffered so the client only sees success after the new
    // packs and refs are durably committed.
    let result = state
        .git
        .write(repo_id, |path| async move {
            let mut cmd = git.command(&path);
            cmd.args(["receive-pack", "--stateless-rpc", "."])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(p) = proto {
                cmd.env("GIT_PROTOCOL", p);
            }
            let mut child = cmd.spawn()?;
            let mut stdin = child.stdin.take().unwrap();
            let feed = async move {
                tokio::io::copy(&mut reader, &mut stdin).await?;
                stdin.shutdown().await?;
                drop(stdin);
                Ok::<_, std::io::Error>(())
            };
            let (fed, out) = tokio::join!(feed, child.wait_with_output());
            let out = out?;
            fed?;
            if !out.status.success() {
                anyhow::bail!("receive-pack failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            Ok(out.stdout)
        })
        .await;
    match result {
        Ok((report, updates)) => {
            if let Err(e) = crate::events::on_push(&state, &ctx, &pusher, &updates).await {
                tracing::error!(error = %e, "post-push processing failed");
            }
            Response::builder()
                .header(header::CONTENT_TYPE, "application/x-git-receive-pack-result")
                .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, max-age=0, must-revalidate"))
                .body(Body::from(report))
                .unwrap()
        }
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "push failed");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "push failed")
        }
    }
}
