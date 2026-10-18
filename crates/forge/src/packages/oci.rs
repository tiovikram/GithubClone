//! OCI distribution-spec registry (GitHub's `ghcr.io`), served at `/v2/` on the forge host.
//! Images are `{host}/{owner}/{name}` (`name` may contain `/`). Works with stock `docker`,
//! `oras`, `crane`, `skopeo`, `buildx`.
//!
//! Auth: `GET /v2/` without credentials answers 401 with
//! `WWW-Authenticate: Bearer realm="{base}/v2/token",service="{host}"`. The token endpoint takes
//! HTTP Basic (login + PAT or password) and returns a short-lived registry token. Every endpoint
//! also accepts a PAT directly (`Bearer`/`token`/Basic). Tokens carry only the identity: the
//! package permissions (see `packages`) are checked on every request.
//!
//! Blobs are content-addressed in object storage. Upload sessions live in Postgres and their
//! chunks are separate objects, so a chunked upload can hop between pods.
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use bytes::Bytes;
use futures::StreamExt;
use hmac::{Hmac, KeyInit, Mac};
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use object_store::path::Path as ObjPath;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::{PackageRow, PkgPerm};
use crate::auth::{self, AuthResult};
use crate::db::{self, User};
use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v2", any(dispatch))
        .route("/v2/", any(dispatch))
        .route("/v2/{*rest}", any(dispatch))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(axum::middleware::map_response(|mut res: Response| async move {
            res.headers_mut().insert("docker-distribution-api-version", HeaderValue::from_static("registry/2.0"));
            res
        }))
}

pub const MT_OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const MT_OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
pub const MT_DOCKER_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";
pub const MT_DOCKER_LIST: &str = "application/vnd.docker.distribution.manifest.list.v2+json";
const MT_OCI_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
const MT_DOCKER_CONFIG: &str = "application/vnd.docker.container.image.v1+json";

const MAX_MANIFEST: usize = 4 << 20;
const TOKEN_TTL: i64 = 3600;

pub fn max_blob_bytes() -> u64 {
    std::env::var("FORGE_PACKAGES_MAX_BLOB_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(10 << 30)
}

// ---------------------------------------------------------------------------------------------
// Errors

/// A distribution-spec error (`{"errors":[{code, message, detail}]}`).
#[derive(Debug)]
pub struct RegErr {
    status: StatusCode,
    code: &'static str,
    message: String,
    detail: Value,
    challenge: Option<String>,
}

type RegResult<T> = Result<T, RegErr>;

impl RegErr {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        RegErr { status, code, message: message.into(), detail: Value::Null, challenge: None }
    }
    fn detail(mut self, d: Value) -> Self {
        self.detail = d;
        self
    }
    fn name_unknown() -> Self {
        Self::new(StatusCode::NOT_FOUND, "NAME_UNKNOWN", "repository name not known to registry")
    }
    fn blob_unknown() -> Self {
        Self::new(StatusCode::NOT_FOUND, "BLOB_UNKNOWN", "blob unknown to registry")
    }
    fn manifest_unknown() -> Self {
        Self::new(StatusCode::NOT_FOUND, "MANIFEST_UNKNOWN", "manifest unknown")
    }
    fn upload_unknown() -> Self {
        Self::new(StatusCode::NOT_FOUND, "BLOB_UPLOAD_UNKNOWN", "blob upload unknown to registry")
    }
    fn digest_invalid(msg: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "DIGEST_INVALID", msg)
    }
    fn denied() -> Self {
        Self::new(StatusCode::FORBIDDEN, "DENIED", "requested access to the resource is denied")
    }
    fn unauthorized(state: &AppState, scope: Option<String>) -> Self {
        let mut e = Self::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "authentication required");
        e.challenge = Some(challenge(state, scope.as_deref()));
        e
    }
}

impl From<ApiError> for RegErr {
    fn from(e: ApiError) -> Self {
        match e.status {
            StatusCode::NOT_FOUND => RegErr::name_unknown(),
            StatusCode::FORBIDDEN => RegErr::denied(),
            StatusCode::UNAUTHORIZED => RegErr::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", e.message),
            s if s.is_server_error() => RegErr::new(s, "UNKNOWN", "internal server error"),
            s => RegErr::new(s, "UNKNOWN", e.message),
        }
    }
}

impl From<sqlx::Error> for RegErr {
    fn from(e: sqlx::Error) -> Self {
        ApiError::from(e).into()
    }
}

impl From<object_store::Error> for RegErr {
    fn from(e: object_store::Error) -> Self {
        ApiError::internal(e).into()
    }
}

impl IntoResponse for RegErr {
    fn into_response(self) -> Response {
        let body = json!({ "errors": [{ "code": self.code, "message": self.message, "detail": self.detail }] });
        let mut res = (self.status, axum::Json(body)).into_response();
        if let Some(c) = self.challenge.and_then(|c| HeaderValue::from_str(&c).ok()) {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, c);
        }
        res
    }
}

fn challenge(state: &AppState, scope: Option<&str>) -> String {
    let mut c = format!("Bearer realm=\"{}/v2/token\",service=\"{}\"", state.base_url(), state.cfg.host());
    if let Some(s) = scope {
        c.push_str(&format!(",scope=\"{s}\""));
    }
    c
}

// ---------------------------------------------------------------------------------------------
// Registry tokens

fn token_mac(secret: &str, uid: i64, exp: i64) -> String {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("any key length");
    m.update(format!("registry\0{uid}\0{exp}").as_bytes());
    hex::encode(&m.finalize().into_bytes()[..20])
}

/// A registry bearer token for `uid` (0 = anonymous).
pub async fn issue_token(state: &AppState, uid: i64) -> anyhow::Result<String> {
    let secret = crate::signed_url::server_secret(&state.db, "registry").await?;
    let exp = chrono::Utc::now().timestamp() + TOKEN_TTL;
    Ok(format!("fgr_{uid}.{exp}.{}", token_mac(&secret, uid, exp)))
}

/// The user id a registry token stands for (`Some(0)` = anonymous), if valid.
pub async fn verify_token(state: &AppState, token: &str) -> Option<i64> {
    let rest = token.strip_prefix("fgr_")?;
    let mut it = rest.splitn(3, '.');
    let uid: i64 = it.next()?.parse().ok()?;
    let exp: i64 = it.next()?.parse().ok()?;
    let sig = it.next()?;
    if exp < chrono::Utc::now().timestamp() {
        return None;
    }
    let secret = crate::signed_url::server_secret(&state.db, "registry").await.ok()?;
    let want = token_mac(&secret, uid, exp);
    (want.len() == sig.len() && want.bytes().zip(sig.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0).then_some(uid)
}

/// Authenticates a registry (or npm) request: registry tokens, PATs, Basic login:password/PAT.
/// Invalid credentials are `Err(())`.
pub async fn registry_user(state: &AppState, parts: &mut axum::http::request::Parts) -> Result<Option<User>, ()> {
    let bearer = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.trim().split_once(' ').filter(|(s, _)| s.eq_ignore_ascii_case("bearer")).map(|(_, t)| t.trim().to_string()));
    if let Some(t) = bearer.filter(|t| t.starts_with("fgr_")) {
        return match verify_token(state, &t).await {
            Some(0) => Ok(None),
            Some(uid) => db::user_by_id(&state.db, uid).await.map(Some).map_err(|_| ()),
            None => Err(()),
        };
    }
    match auth::authenticate(parts, state).await {
        Ok(AuthResult::User(u)) => Ok(Some(u)),
        Ok(AuthResult::Anonymous) => Ok(None),
        Ok(AuthResult::Invalid) | Err(_) => Err(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Request routing

enum Route {
    Base,
    Token,
    Catalog,
    Tags(String),
    Manifest(String, String),
    Blob(String, String),
    Uploads(String),
    Upload(String, String),
    Referrers(String, String),
}

fn parse_route(rest: &str) -> Option<Route> {
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        return Some(Route::Base);
    }
    if rest == "token" || rest == "token/" {
        return Some(Route::Token);
    }
    if rest == "_catalog" {
        return Some(Route::Catalog);
    }
    if let Some(name) = rest.strip_suffix("/tags/list") {
        return Some(Route::Tags(name.to_string()));
    }
    if let Some(name) = rest.strip_suffix("/blobs/uploads").or_else(|| rest.strip_suffix("/blobs/uploads/")) {
        return Some(Route::Uploads(name.to_string()));
    }
    if let Some((name, id)) = rest.rsplit_once("/blobs/uploads/") {
        return Some(Route::Upload(name.to_string(), id.to_string()));
    }
    if let Some((name, r)) = rest.rsplit_once("/manifests/") {
        return Some(Route::Manifest(name.to_string(), r.to_string()));
    }
    if let Some((name, d)) = rest.rsplit_once("/blobs/") {
        return Some(Route::Blob(name.to_string(), d.to_string()));
    }
    if let Some((name, d)) = rest.rsplit_once("/referrers/") {
        return Some(Route::Referrers(name.to_string(), d.to_string()));
    }
    None
}

/// OCI repository name grammar (lowercase path components separated by `/`).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.split('/').all(|c| {
            !c.is_empty()
                && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-')
                && c.as_bytes()[0].is_ascii_alphanumeric()
                && c.as_bytes()[c.len() - 1].is_ascii_alphanumeric()
        })
}

fn valid_tag(t: &str) -> bool {
    let b = t.as_bytes();
    !b.is_empty() && b.len() <= 128 && (b[0].is_ascii_alphanumeric() || b[0] == b'_') && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

fn query_map(uri: &axum::http::Uri) -> std::collections::HashMap<String, String> {
    uri.query().map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default()
}

async fn dispatch(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let rest = path.strip_prefix("/v2").unwrap_or("");
    let Some(route) = parse_route(rest) else {
        return RegErr::new(StatusCode::NOT_FOUND, "NAME_INVALID", "invalid repository name").into_response();
    };
    let user = match registry_user(&state, &mut parts).await {
        Ok(u) => u,
        Err(()) => {
            let mut e = RegErr::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "authentication required: bad credentials");
            if !matches!(route, Route::Token) {
                e.challenge = Some(challenge(&state, None));
            }
            return e.into_response();
        }
    };
    let q = query_map(&parts.uri);
    let m = parts.method.clone();
    let res = match route {
        Route::Base => base(&state, user.as_ref()),
        Route::Token => token(&state, user.as_ref(), &m, body).await,
        Route::Catalog => catalog(&state, user.as_ref(), &q).await,
        Route::Tags(name) if m == Method::GET => tags_list(&state, user.as_ref(), &name, &q).await,
        Route::Manifest(name, r) => match m {
            Method::GET | Method::HEAD => get_manifest(&state, user.as_ref(), &name, &r, m == Method::HEAD).await,
            Method::PUT => put_manifest(&state, user.as_ref(), &name, &r, &parts.headers, body).await,
            Method::DELETE => delete_manifest(&state, user.as_ref(), &name, &r).await,
            _ => Err(unsupported()),
        },
        Route::Blob(name, d) => match m {
            Method::GET | Method::HEAD => get_blob(&state, user.as_ref(), &name, &d, &parts.headers, m == Method::HEAD).await,
            Method::DELETE => Err(RegErr::new(StatusCode::METHOD_NOT_ALLOWED, "UNSUPPORTED", "blobs are deleted by garbage collection once no manifest references them")),
            _ => Err(unsupported()),
        },
        Route::Uploads(name) if m == Method::POST => start_upload(&state, user.as_ref(), &name, &q, body).await,
        Route::Upload(name, id) => match m {
            Method::GET => upload_status(&state, user.as_ref(), &name, &id).await,
            Method::PATCH => patch_upload(&state, user.as_ref(), &name, &id, &parts.headers, body).await,
            Method::PUT => put_upload(&state, user.as_ref(), &name, &id, &q, body).await,
            Method::DELETE => cancel_upload(&state, user.as_ref(), &name, &id).await,
            _ => Err(unsupported()),
        },
        Route::Referrers(name, d) if m == Method::GET => referrers(&state, user.as_ref(), &name, &d, &q).await,
        _ => Err(unsupported()),
    };
    res.unwrap_or_else(|e| e.into_response())
}

fn unsupported() -> RegErr {
    RegErr::new(StatusCode::METHOD_NOT_ALLOWED, "UNSUPPORTED", "the operation is unsupported")
}

fn base(state: &AppState, user: Option<&User>) -> RegResult<Response> {
    if user.is_none() {
        return Err(RegErr::unauthorized(state, None));
    }
    Ok((StatusCode::OK, axum::Json(json!({}))).into_response())
}

/// `GET /v2/token`: exchanges Basic credentials (or none, for anonymous pulls) for a registry
/// token (Docker token spec). `POST /v2/token` is the OAuth2 flavour docker uses after
/// `docker login`: `grant_type=password` (username + password/PAT) or `grant_type=refresh_token`
/// (a PAT used as identity token).
async fn token(state: &AppState, user: Option<&User>, m: &Method, body: Body) -> RegResult<Response> {
    let mut who = user.cloned();
    if *m == Method::POST {
        let data = axum::body::to_bytes(body, 1 << 16).await.map_err(|_| RegErr::new(StatusCode::BAD_REQUEST, "UNSUPPORTED", "request too large"))?;
        let form: std::collections::HashMap<String, String> = url::form_urlencoded::parse(&data).into_owned().collect();
        let bad = || RegErr::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "authentication required: bad credentials");
        match form.get("grant_type").map(String::as_str) {
            Some("password") => {
                let login = form.get("username").cloned().unwrap_or_default();
                let secret = form.get("password").cloned().unwrap_or_default();
                let u = match auth::user_for_token(state, &secret).await? {
                    Some(u) => Some(u),
                    None if !secret.is_empty() => auth::user_for_password(state, &login, &secret, None).await?,
                    None => None,
                };
                who = Some(u.ok_or_else(bad)?);
            }
            Some("refresh_token") => {
                let t = form.get("refresh_token").cloned().unwrap_or_default();
                who = Some(auth::user_for_token(state, &t).await?.ok_or_else(bad)?);
            }
            _ if who.is_some() => {}
            _ => return Err(RegErr::new(StatusCode::BAD_REQUEST, "UNSUPPORTED", "unsupported grant_type")),
        }
    }
    let uid = who.as_ref().map(|u| u.id).unwrap_or(0);
    let t = issue_token(state, uid).await.map_err(ApiError::from)?;
    let body = json!({ "token": t, "access_token": t, "expires_in": TOKEN_TTL, "issued_at": crate::json::ts(chrono::Utc::now()) });
    let mut res = (StatusCode::OK, axum::Json(body)).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

// ---------------------------------------------------------------------------------------------
// Name resolution and access

/// A resolved `owner/name` and what the caller may do with it.
struct Target {
    full: String,
    owner: User,
    name: String,
    pkg: Option<PackageRow>,
    perm: PkgPerm,
    can_create: bool,
}

impl Target {
    fn can_push(&self) -> bool {
        match &self.pkg {
            Some(_) => self.perm >= PkgPerm::Write,
            None => self.can_create,
        }
    }
    fn can_pull(&self) -> bool {
        match &self.pkg {
            Some(_) => self.perm >= PkgPerm::Read,
            // Blobs pushed before the first manifest are visible to whoever may push.
            None => self.can_create,
        }
    }
}

async fn resolve(state: &AppState, user: Option<&User>, full: &str) -> RegResult<Target> {
    if !valid_name(full) {
        return Err(RegErr::new(StatusCode::BAD_REQUEST, "NAME_INVALID", "invalid repository name"));
    }
    let Some((owner_login, name)) = full.split_once('/') else {
        return Err(RegErr::new(StatusCode::BAD_REQUEST, "NAME_INVALID", "repository name must be <owner>/<name>"));
    };
    let owner = db::user_by_login(&state.db, owner_login).await.map_err(|_| RegErr::name_unknown())?;
    let pkg = super::find_active(state, owner.id, "container", name).await?;
    let perm = match &pkg {
        Some(p) => super::package_perm(state, user, p, &owner).await?,
        None => PkgPerm::None,
    };
    let can_create = match user {
        Some(u) => pkg.is_none() && super::can_create(state, u, &owner).await?,
        None => false,
    };
    Ok(Target { full: full.to_string(), owner, name: name.to_string(), pkg, perm, can_create })
}

fn scope(full: &str, actions: &str) -> String {
    format!("repository:{full}:{actions}")
}

async fn pull_target(state: &AppState, user: Option<&User>, full: &str) -> RegResult<Target> {
    let t = resolve(state, user, full).await?;
    if t.can_pull() {
        return Ok(t);
    }
    if user.is_none() {
        return Err(RegErr::unauthorized(state, Some(scope(full, "pull"))));
    }
    Err(RegErr::name_unknown())
}

async fn push_target(state: &AppState, user: Option<&User>, full: &str) -> RegResult<(Target, User)> {
    let t = resolve(state, user, full).await?;
    let Some(u) = user else {
        return Err(RegErr::unauthorized(state, Some(scope(full, "pull,push"))));
    };
    if t.can_push() {
        return Ok((t, u.clone()));
    }
    if t.pkg.is_some() && t.perm >= PkgPerm::Read || t.pkg.is_none() {
        return Err(RegErr::denied());
    }
    Err(RegErr::name_unknown())
}

/// Whether `digest` is part of repository `t` (pushed into it, or referenced by a version).
async fn blob_in_repo(state: &AppState, t: &Target, digest: &str) -> RegResult<bool> {
    let pkg_id = t.pkg.as_ref().map(|p| p.id).unwrap_or(0);
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM package_blobs b WHERE b.digest = $3 AND (
            EXISTS (SELECT 1 FROM package_repo_blobs WHERE owner_id = $1 AND name = $2 AND digest = $3)
            OR EXISTS (SELECT 1 FROM package_version_blobs vb JOIN package_versions v ON v.id = vb.version_id
                        WHERE v.package_id = $4 AND vb.digest = $3)))",
    )
    .bind(t.owner.id)
    .bind(&t.name)
    .bind(digest)
    .bind(pkg_id)
    .fetch_one(&state.db)
    .await?)
}

async fn link_blob(state: &AppState, t: &Target, digest: &str) -> RegResult<()> {
    sqlx::query(
        "INSERT INTO package_repo_blobs (owner_id, name, digest) VALUES ($1, $2, $3)
         ON CONFLICT (owner_id, name, digest) DO UPDATE SET last_seen_at = now()",
    )
    .bind(t.owner.id)
    .bind(&t.name)
    .bind(digest)
    .execute(&state.db)
    .await?;
    sqlx::query("UPDATE package_blobs SET last_seen_at = now() WHERE digest = $1").bind(digest).execute(&state.db).await?;
    Ok(())
}

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or(HeaderValue::from_static(""))
}

// ---------------------------------------------------------------------------------------------
// Blobs

async fn get_blob(state: &AppState, user: Option<&User>, full: &str, digest: &str, headers: &HeaderMap, head: bool) -> RegResult<Response> {
    let t = pull_target(state, user, full).await?;
    if super::digest_hex(digest).is_none() {
        return Err(RegErr::digest_invalid("invalid digest"));
    }
    if !blob_in_repo(state, &t, digest).await? {
        return Err(RegErr::blob_unknown());
    }
    let size = super::blob_size(state, digest).await?.ok_or_else(RegErr::blob_unknown)?;
    super::touch_blobs(state, &[digest.to_string()]).await?;
    sqlx::query("UPDATE package_repo_blobs SET last_seen_at = now() WHERE owner_id = $1 AND name = $2 AND digest = $3 AND last_seen_at < now() - interval '1 minute'")
        .bind(t.owner.id)
        .bind(&t.name)
        .bind(digest)
        .execute(&state.db)
        .await?;
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|r| parse_range(r, size as u64));
    let mut res = if head {
        let mut r = Response::new(Body::empty());
        r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(size as u64));
        r
    } else if let Some((start, end)) = range {
        let opts = object_store::GetOptions { range: Some(object_store::GetRange::Bounded(start..end + 1)), ..Default::default() };
        let got = state.store.get_opts(&super::blob_key(digest), opts).await.map_err(|_| RegErr::blob_unknown())?;
        let mut r = Response::new(Body::from_stream(got.into_stream()));
        *r.status_mut() = StatusCode::PARTIAL_CONTENT;
        r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(end - start + 1));
        r.headers_mut().insert(header::CONTENT_RANGE, hv(&format!("bytes {start}-{end}/{size}")));
        r
    } else {
        let got = state.store.get(&super::blob_key(digest)).await.map_err(|_| RegErr::blob_unknown())?;
        let mut r = Response::new(Body::from_stream(got.into_stream()));
        r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(size as u64));
        r
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert("docker-content-digest", hv(digest));
    h.insert(header::ETAG, hv(&format!("\"{digest}\"")));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=31536000"));
    crate::security::harden_user_content(&mut res);
    Ok(res)
}

/// `bytes=a-b` / `bytes=a-` (single range) -> inclusive (start, end).
fn parse_range(r: &str, size: u64) -> Option<(u64, u64)> {
    let spec = r.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let start: u64 = a.trim().parse().ok()?;
    let end: u64 = if b.trim().is_empty() { size - 1 } else { b.trim().parse::<u64>().ok()?.min(size - 1) };
    (start <= end).then_some((start, end))
}

// ---------------------------------------------------------------------------------------------
// Uploads

#[derive(sqlx::FromRow)]
struct UploadRow {
    id: String,
    owner_id: i64,
    name: String,
    user_id: Option<i64>,
    size: i64,
    parts: Vec<String>,
    single_digest: Option<String>,
}

fn upload_location(full: &str, id: &str) -> String {
    format!("/v2/{full}/blobs/uploads/{id}")
}

fn range_value(size: i64) -> String {
    format!("0-{}", (size - 1).max(0))
}

fn upload_response(status: StatusCode, full: &str, u: &UploadRow) -> Response {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::LOCATION, hv(&upload_location(full, &u.id)));
    h.insert(header::RANGE, hv(&range_value(u.size)));
    h.insert("docker-upload-uuid", hv(&u.id));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(0u64));
    res
}

fn blob_created(full: &str, digest: &str) -> Response {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::CREATED;
    let h = res.headers_mut();
    h.insert(header::LOCATION, hv(&format!("/v2/{full}/blobs/{digest}")));
    h.insert("docker-content-digest", hv(digest));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(0u64));
    res
}

/// Streams a request body into one object; returns (size, sha256 digest).
async fn stream_to_object(state: &AppState, key: &ObjPath, body: Body, limit: u64) -> RegResult<(i64, String)> {
    let mut w = object_store::buffered::BufWriter::new(state.store.clone(), key.clone());
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = w.abort().await;
                return Err(RegErr::new(StatusCode::BAD_REQUEST, "BLOB_UPLOAD_INVALID", format!("upload interrupted: {e}")));
            }
        };
        size += chunk.len() as u64;
        if size > limit {
            let _ = w.abort().await;
            return Err(RegErr::new(StatusCode::PAYLOAD_TOO_LARGE, "SIZE_INVALID", "blob exceeds the maximum size"));
        }
        hasher.update(&chunk);
        if let Err(e) = w.write_all(&chunk).await {
            let _ = w.abort().await;
            return Err(ApiError::internal(e).into());
        }
    }
    w.shutdown().await.map_err(ApiError::internal)?;
    Ok((size as i64, format!("sha256:{}", hex::encode(hasher.finalize()))))
}

async fn new_upload(state: &AppState, t: &Target, u: &User) -> RegResult<UploadRow> {
    Ok(sqlx::query_as("INSERT INTO package_uploads (id, owner_id, name, user_id) VALUES ($1, $2, $3, $4) RETURNING *")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(t.owner.id)
        .bind(&t.name)
        .bind(u.id)
        .fetch_one(&state.db)
        .await?)
}

async fn start_upload(state: &AppState, user: Option<&User>, full: &str, q: &std::collections::HashMap<String, String>, body: Body) -> RegResult<Response> {
    let (t, u) = push_target(state, user, full).await?;
    // Cross-repository mount.
    if let Some(d) = q.get("mount") {
        if super::digest_hex(d).is_some() {
            let mounted = match q.get("from") {
                Some(from) if from != full => match resolve(state, user, from).await {
                    Ok(src) if src.can_pull() => blob_in_repo(state, &src, d).await?,
                    _ => false,
                },
                _ => blob_in_repo(state, &t, d).await?,
            };
            if mounted {
                link_blob(state, &t, d).await?;
                return Ok(blob_created(full, d));
            }
        }
    }
    let up = new_upload(state, &t, &u).await?;
    // Monolithic upload: POST ?digest= with the whole blob as the body.
    if let Some(d) = q.get("digest") {
        let up = append_part(state, up, body, None).await?;
        return finish_upload(state, &t, up, d).await;
    }
    Ok(upload_response(StatusCode::ACCEPTED, full, &up))
}

async fn load_upload(state: &AppState, t: &Target, u: &User, id: &str) -> RegResult<UploadRow> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| RegErr::upload_unknown())?.to_string();
    let up: UploadRow = sqlx::query_as("SELECT * FROM package_uploads WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(RegErr::upload_unknown)?;
    if up.owner_id != t.owner.id || up.name != t.name || up.user_id != Some(u.id) {
        return Err(RegErr::upload_unknown());
    }
    Ok(up)
}

/// Appends `body` as the next chunk. `start` (from Content-Range) must equal the current size.
async fn append_part(state: &AppState, up: UploadRow, body: Body, start: Option<i64>) -> RegResult<UploadRow> {
    if start.is_some_and(|s| s != up.size) {
        return Err(RegErr::new(StatusCode::RANGE_NOT_SATISFIABLE, "BLOB_UPLOAD_INVALID", "chunk out of order"));
    }
    let n = up.parts.len();
    let key = format!("packages/uploads/{}/{n:06}-{}", up.id, crate::auth::random_string(6));
    let limit = max_blob_bytes().saturating_sub(up.size as u64);
    let (len, digest) = stream_to_object(state, &ObjPath::from(key.clone()), body, limit).await?;
    if len == 0 {
        let _ = state.store.delete(&ObjPath::from(key)).await;
        return Ok(up);
    }
    let first = n == 0;
    let row: Option<UploadRow> = sqlx::query_as(
        "UPDATE package_uploads SET parts = array_append(parts, $2), size = size + $3,
            single_digest = CASE WHEN $5 THEN $4 ELSE NULL END, updated_at = now()
          WHERE id = $1 AND cardinality(parts) = $6 RETURNING *",
    )
    .bind(up.id)
    .bind(&key)
    .bind(len)
    .bind(&digest)
    .bind(first)
    .bind(n as i32)
    .fetch_optional(&state.db)
    .await?;
    match row {
        Some(r) => Ok(r),
        None => {
            let _ = state.store.delete(&ObjPath::from(key)).await;
            Err(RegErr::new(StatusCode::RANGE_NOT_SATISFIABLE, "BLOB_UPLOAD_INVALID", "concurrent chunk upload"))
        }
    }
}

async fn delete_upload_objects(state: &AppState, up: &UploadRow) {
    for p in &up.parts {
        let _ = state.store.delete(&ObjPath::from(p.as_str())).await;
    }
    let _ = state.store.delete(&ObjPath::from(format!("packages/uploads/{}/assembled", up.id))).await;
    let _ = sqlx::query("DELETE FROM package_uploads WHERE id = $1").bind(&up.id).execute(&state.db).await;
}

/// Verifies the uploaded content against `digest`, moves it into the blob store, links it to the
/// repository and ends the session.
async fn finish_upload(state: &AppState, t: &Target, up: UploadRow, digest: &str) -> RegResult<Response> {
    if super::digest_hex(digest).is_none() {
        delete_upload_objects(state, &up).await;
        return Err(RegErr::digest_invalid("unsupported or invalid digest"));
    }
    // The content as one object, and its digest.
    let (source, computed): (Option<ObjPath>, String) = match (up.parts.len(), &up.single_digest) {
        (0, _) => (None, super::sha256_digest(b"")),
        (1, Some(d)) => (Some(ObjPath::from(up.parts[0].as_str())), d.clone()),
        _ => {
            let key = ObjPath::from(format!("packages/uploads/{}/assembled", up.id));
            let mut w = object_store::buffered::BufWriter::new(state.store.clone(), key.clone());
            let mut hasher = Sha256::new();
            for p in &up.parts {
                let got = state.store.get(&ObjPath::from(p.as_str())).await?;
                let mut s = got.into_stream();
                while let Some(chunk) = s.next().await {
                    let chunk = chunk?;
                    hasher.update(&chunk);
                    w.write_all(&chunk).await.map_err(ApiError::internal)?;
                }
            }
            w.shutdown().await.map_err(ApiError::internal)?;
            (Some(key), format!("sha256:{}", hex::encode(hasher.finalize())))
        }
    };
    if computed != digest {
        delete_upload_objects(state, &up).await;
        return Err(RegErr::digest_invalid("provided digest did not match uploaded content").detail(json!({ "computed": computed })));
    }
    if !super::blob_stored(state, digest).await? {
        let dest = super::blob_key(digest);
        match &source {
            Some(src) => state.store.copy(src, &dest).await?,
            None => {
                state.store.put(&dest, Bytes::new().into()).await?;
            }
        }
        sqlx::query("INSERT INTO package_blobs (digest, size) VALUES ($1, $2) ON CONFLICT (digest) DO UPDATE SET last_seen_at = now()")
            .bind(digest)
            .bind(up.size)
            .execute(&state.db)
            .await?;
    }
    link_blob(state, t, digest).await?;
    delete_upload_objects(state, &up).await;
    Ok(blob_created(&t.full, digest))
}

fn content_range_start(headers: &HeaderMap) -> Option<i64> {
    let v = headers.get(header::CONTENT_RANGE)?.to_str().ok()?;
    let v = v.trim().trim_start_matches("bytes").trim_start_matches([' ', '=']);
    v.split_once('-')?.0.trim().parse().ok()
}

async fn patch_upload(state: &AppState, user: Option<&User>, full: &str, id: &str, headers: &HeaderMap, body: Body) -> RegResult<Response> {
    let (t, u) = push_target(state, user, full).await?;
    let up = load_upload(state, &t, &u, id).await?;
    let start = content_range_start(headers);
    match append_part(state, up, body, start).await {
        Ok(up) => Ok(upload_response(StatusCode::ACCEPTED, full, &up)),
        Err(e) if e.status == StatusCode::RANGE_NOT_SATISFIABLE => {
            let cur = load_upload(state, &t, &u, id).await?;
            let mut res = e.into_response();
            res.headers_mut().insert(header::LOCATION, hv(&upload_location(full, &cur.id)));
            res.headers_mut().insert(header::RANGE, hv(&range_value(cur.size)));
            Ok(res)
        }
        Err(e) => Err(e),
    }
}

async fn put_upload(state: &AppState, user: Option<&User>, full: &str, id: &str, q: &std::collections::HashMap<String, String>, body: Body) -> RegResult<Response> {
    let (t, u) = push_target(state, user, full).await?;
    let up = load_upload(state, &t, &u, id).await?;
    let Some(d) = q.get("digest") else {
        return Err(RegErr::digest_invalid("digest parameter is required"));
    };
    let up = append_part(state, up, body, None).await?;
    finish_upload(state, &t, up, d).await
}

async fn upload_status(state: &AppState, user: Option<&User>, full: &str, id: &str) -> RegResult<Response> {
    let (t, u) = push_target(state, user, full).await?;
    let up = load_upload(state, &t, &u, id).await?;
    Ok(upload_response(StatusCode::NO_CONTENT, full, &up))
}

async fn cancel_upload(state: &AppState, user: Option<&User>, full: &str, id: &str) -> RegResult<Response> {
    let (t, u) = push_target(state, user, full).await?;
    let up = load_upload(state, &t, &u, id).await?;
    delete_upload_objects(state, &up).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------------------------
// Manifests

fn is_digest(r: &str) -> bool {
    r.contains(':')
}

/// The active version a reference (tag or digest) names.
async fn resolve_ref(state: &AppState, pkg: &PackageRow, r: &str) -> RegResult<Option<super::VersionRow>> {
    let sql = if is_digest(r) {
        "SELECT * FROM package_versions WHERE package_id = $1 AND name = $2 AND deleted_at IS NULL"
    } else {
        "SELECT v.* FROM package_tags t JOIN package_versions v ON v.id = t.version_id
          WHERE t.package_id = $1 AND t.tag = $2 AND v.deleted_at IS NULL"
    };
    Ok(sqlx::query_as(sql).bind(pkg.id).bind(r).fetch_optional(&state.db).await?)
}

async fn get_manifest(state: &AppState, user: Option<&User>, full: &str, r: &str, head: bool) -> RegResult<Response> {
    let t = pull_target(state, user, full).await?;
    let pkg = t.pkg.as_ref().ok_or_else(RegErr::manifest_unknown)?;
    let v = resolve_ref(state, pkg, r).await?.ok_or_else(RegErr::manifest_unknown)?;
    let mut res = if head {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from(super::read_blob(state, &v.digest).await?))
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, hv(&v.media_type));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(v.size as u64));
    h.insert("docker-content-digest", hv(&v.digest));
    h.insert(header::ETAG, hv(&format!("\"{}\"", v.digest)));
    Ok(res)
}

/// What a manifest references and says about itself.
#[derive(Default)]
struct ManifestInfo {
    blobs: Vec<String>,
    children: Vec<String>,
    config_digest: Option<String>,
    config_media_type: Option<String>,
    subject: Option<String>,
    artifact_type: Option<String>,
    annotations: serde_json::Map<String, Value>,
}

fn manifest_info(doc: &Value) -> ManifestInfo {
    let mut m = ManifestInfo::default();
    let digest_of = |v: &Value| v["digest"].as_str().map(str::to_string);
    if let Some(c) = doc.get("config") {
        m.config_digest = digest_of(c);
        m.config_media_type = c["mediaType"].as_str().map(str::to_string);
        m.blobs.extend(digest_of(c));
    }
    for key in ["layers", "blobs"] {
        for l in doc[key].as_array().into_iter().flatten() {
            // Foreign (non-distributable) layers live elsewhere.
            if l["urls"].as_array().is_some_and(|u| !u.is_empty()) {
                continue;
            }
            m.blobs.extend(digest_of(l));
        }
    }
    for c in doc["manifests"].as_array().into_iter().flatten() {
        m.children.extend(digest_of(c));
    }
    m.subject = doc.get("subject").and_then(digest_of);
    m.artifact_type = doc["artifactType"].as_str().map(str::to_string).or_else(|| {
        // OCI 1.1: an image manifest's artifact type defaults to its config media type when it is
        // not an image config.
        m.config_media_type.clone().filter(|t| t != MT_OCI_CONFIG && t != MT_DOCKER_CONFIG && !t.ends_with("empty.v1+json"))
    });
    if let Some(a) = doc["annotations"].as_object() {
        m.annotations = a.clone();
    }
    m
}

async fn put_manifest(state: &AppState, user: Option<&User>, full: &str, r: &str, headers: &HeaderMap, body: Body) -> RegResult<Response> {
    let (t, actor) = push_target(state, user, full).await?;
    let data = axum::body::to_bytes(body, MAX_MANIFEST)
        .await
        .map_err(|_| RegErr::new(StatusCode::PAYLOAD_TOO_LARGE, "MANIFEST_INVALID", "manifest too large"))?;
    let doc: Value = serde_json::from_slice(&data).map_err(|_| RegErr::new(StatusCode::BAD_REQUEST, "MANIFEST_INVALID", "manifest invalid: not JSON"))?;
    let header_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string())
        .filter(|v| !v.is_empty() && v != "application/json" && v != "application/octet-stream");
    let media_type = header_type.or_else(|| doc["mediaType"].as_str().map(str::to_string)).unwrap_or_else(|| {
        if doc.get("manifests").is_some() { MT_OCI_INDEX.to_string() } else { MT_OCI_MANIFEST.to_string() }
    });
    if media_type.starts_with("application/vnd.docker.distribution.manifest.v1") || doc["schemaVersion"] == 1 {
        return Err(RegErr::new(StatusCode::BAD_REQUEST, "MANIFEST_INVALID", "Docker image manifest schema 1 is not supported"));
    }
    if doc["schemaVersion"] != 2 {
        return Err(RegErr::new(StatusCode::BAD_REQUEST, "MANIFEST_INVALID", "manifest invalid: schemaVersion must be 2"));
    }
    let digest = super::sha256_digest(&data);
    if is_digest(r) {
        if r != digest {
            return Err(RegErr::digest_invalid("manifest digest does not match the reference"));
        }
    } else if !valid_tag(r) {
        return Err(RegErr::new(StatusCode::BAD_REQUEST, "TAG_INVALID", "manifest tag did not match URI"));
    }
    let info = manifest_info(&doc);
    for d in info.blobs.iter().chain(info.children.iter()) {
        if super::digest_hex(d).is_none() || !blob_in_repo(state, &t, d).await? {
            return Err(RegErr::new(StatusCode::BAD_REQUEST, "MANIFEST_BLOB_UNKNOWN", "blob unknown to registry").detail(json!({ "digest": d })));
        }
    }
    // Image labels from the config blob (source repository, description, license).
    let mut labels = serde_json::Map::new();
    if let (Some(cd), Some(ct)) = (&info.config_digest, &info.config_media_type) {
        if ct == MT_OCI_CONFIG || ct == MT_DOCKER_CONFIG {
            if super::blob_size(state, cd).await?.is_some_and(|s| s < 4 << 20) {
                if let Ok(cfg) = super::read_blob(state, cd).await.map(|b| serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null)) {
                    if let Some(l) = cfg["config"]["Labels"].as_object() {
                        labels = l.clone();
                    }
                }
            }
        }
    }
    for (k, v) in &info.annotations {
        labels.entry(k.clone()).or_insert(v.clone());
    }
    let label = |k: &str| labels.get(k).and_then(|v| v.as_str()).map(str::to_string);

    super::put_blob_bytes(state, data.clone()).await?;
    let (mut pkg, created) = match &t.pkg {
        Some(p) => (p.clone(), false),
        None => super::ensure_package(state, &actor, &t.owner, "container", &t.name).await?,
    };
    if pkg.repo_id.is_none() {
        if let Some(src) = label("org.opencontainers.image.source") {
            pkg = super::link_source(state, &actor, &pkg, &t.owner, &src, created).await?;
        }
    }
    let metadata = json!({
        "config_digest": info.config_digest,
        "artifact_type": info.artifact_type,
        "subject": info.subject,
        "annotations": Value::Object(info.annotations.clone()),
        "children": info.children,
    });
    let mut tx = state.db.begin().await?;
    let v: super::VersionRow = sqlx::query_as(
        "INSERT INTO package_versions (package_id, name, media_type, digest, size, metadata, description, license, creator_id)
         VALUES ($1, $2, $3, $2, $4, $5, $6, $7, $8)
         ON CONFLICT (package_id, name) DO UPDATE SET media_type = EXCLUDED.media_type, metadata = EXCLUDED.metadata,
            description = EXCLUDED.description, license = EXCLUDED.license, deleted_at = NULL, deleted_by = NULL, updated_at = now()
         RETURNING *",
    )
    .bind(pkg.id)
    .bind(&digest)
    .bind(&media_type)
    .bind(data.len() as i64)
    .bind(metadata)
    .bind(label("org.opencontainers.image.description"))
    .bind(label("org.opencontainers.image.licenses"))
    .bind(actor.id)
    .fetch_one(&mut *tx)
    .await?;
    let mut refs: Vec<String> = vec![digest.clone()];
    refs.extend(info.blobs.iter().cloned());
    refs.extend(info.children.iter().cloned());
    sqlx::query("INSERT INTO package_version_blobs (version_id, digest) SELECT $1, unnest($2::text[]) ON CONFLICT DO NOTHING")
        .bind(v.id)
        .bind(&refs)
        .execute(&mut *tx)
        .await?;
    if !is_digest(r) {
        sqlx::query(
            "INSERT INTO package_tags (package_id, tag, version_id) VALUES ($1, $2, $3)
             ON CONFLICT (package_id, tag) DO UPDATE SET version_id = EXCLUDED.version_id, updated_at = now()",
        )
        .bind(pkg.id)
        .bind(r)
        .bind(v.id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE packages SET updated_at = now() WHERE id = $1").bind(pkg.id).execute(&mut *tx).await?;
    tx.commit().await?;

    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::CREATED;
    let h = res.headers_mut();
    h.insert(header::LOCATION, hv(&format!("/v2/{full}/manifests/{digest}")));
    h.insert("docker-content-digest", hv(&digest));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(0u64));
    if let Some(s) = &info.subject {
        h.insert("oci-subject", hv(s));
    }
    Ok(res)
}

async fn delete_manifest(state: &AppState, user: Option<&User>, full: &str, r: &str) -> RegResult<Response> {
    let t = resolve(state, user, full).await?;
    let Some(u) = user else {
        return Err(RegErr::unauthorized(state, Some(scope(full, "delete"))));
    };
    let pkg = match &t.pkg {
        Some(p) if t.perm >= PkgPerm::Read => p.clone(),
        _ => return Err(RegErr::name_unknown()),
    };
    if is_digest(r) {
        if t.perm < PkgPerm::Admin {
            return Err(RegErr::denied());
        }
        let v = resolve_ref(state, &pkg, r).await?.ok_or_else(RegErr::manifest_unknown)?;
        super::delete_version(state, u, &pkg, t.perm, v.id).await?;
    } else {
        if t.perm < PkgPerm::Write {
            return Err(RegErr::denied());
        }
        let n = sqlx::query("DELETE FROM package_tags WHERE package_id = $1 AND tag = $2").bind(pkg.id).bind(r).execute(&state.db).await?;
        if n.rows_affected() == 0 {
            return Err(RegErr::manifest_unknown());
        }
    }
    Ok(StatusCode::ACCEPTED.into_response())
}

// ---------------------------------------------------------------------------------------------
// Listings

fn page_n(q: &std::collections::HashMap<String, String>) -> i64 {
    q.get("n").and_then(|n| n.parse().ok()).unwrap_or(1000).clamp(1, 10_000)
}

async fn tags_list(state: &AppState, user: Option<&User>, full: &str, q: &std::collections::HashMap<String, String>) -> RegResult<Response> {
    let t = pull_target(state, user, full).await?;
    let pkg = t.pkg.as_ref().ok_or_else(RegErr::name_unknown)?;
    let n = page_n(q);
    let last = q.get("last").cloned();
    let mut tags: Vec<String> = sqlx::query_scalar(
        "SELECT t.tag FROM package_tags t JOIN package_versions v ON v.id = t.version_id
          WHERE t.package_id = $1 AND v.deleted_at IS NULL AND ($2::text IS NULL OR t.tag > $2) ORDER BY t.tag LIMIT $3",
    )
    .bind(pkg.id)
    .bind(&last)
    .bind(n + 1)
    .fetch_all(&state.db)
    .await?;
    let more = tags.len() as i64 > n;
    tags.truncate(n as usize);
    let mut res = (StatusCode::OK, axum::Json(json!({ "name": full, "tags": tags }))).into_response();
    if more {
        if let Some(l) = tags.last() {
            let link = format!("</v2/{full}/tags/list?n={n}&last={}>; rel=\"next\"", super::enc(l));
            res.headers_mut().insert(header::LINK, hv(&link));
        }
    }
    Ok(res)
}

async fn catalog(state: &AppState, user: Option<&User>, q: &std::collections::HashMap<String, String>) -> RegResult<Response> {
    let n = page_n(q);
    let last = q.get("last").cloned().unwrap_or_default();
    let pkgs: Vec<PackageRow> = sqlx::query_as("SELECT * FROM packages WHERE package_type = 'container' AND deleted_at IS NULL ORDER BY id")
        .fetch_all(&state.db)
        .await?;
    let owners = crate::json::issue::users_by_ids(&state.db, &pkgs.iter().map(|p| p.owner_id).collect::<Vec<_>>()).await?;
    let mut names = Vec::new();
    for p in pkgs {
        let Some(owner) = owners.get(&p.owner_id) else { continue };
        if super::package_perm(state, user, &p, owner).await? >= PkgPerm::Read {
            names.push(format!("{}/{}", owner.login.to_lowercase(), p.name));
        }
    }
    names.sort();
    names.retain(|n| n.as_str() > last.as_str());
    let more = names.len() as i64 > n;
    names.truncate(n as usize);
    let mut res = (StatusCode::OK, axum::Json(json!({ "repositories": names }))).into_response();
    if more {
        if let Some(l) = names.last() {
            res.headers_mut().insert(header::LINK, hv(&format!("</v2/_catalog?n={n}&last={}>; rel=\"next\"", super::enc(l))));
        }
    }
    Ok(res)
}

/// OCI 1.1 referrers API: manifests whose `subject` is `digest`.
async fn referrers(state: &AppState, user: Option<&User>, full: &str, digest: &str, q: &std::collections::HashMap<String, String>) -> RegResult<Response> {
    let t = pull_target(state, user, full).await?;
    if super::digest_hex(digest).is_none() {
        return Err(RegErr::digest_invalid("invalid digest"));
    }
    let rows: Vec<super::VersionRow> = match &t.pkg {
        Some(p) => sqlx::query_as("SELECT * FROM package_versions WHERE package_id = $1 AND deleted_at IS NULL AND metadata->>'subject' = $2 ORDER BY id")
            .bind(p.id)
            .bind(digest)
            .fetch_all(&state.db)
            .await?,
        None => Vec::new(),
    };
    let filter = q.get("artifactType");
    let manifests: Vec<Value> = rows
        .iter()
        .filter(|v| filter.is_none_or(|f| v.metadata["artifact_type"].as_str() == Some(f.as_str())))
        .map(|v| {
            let mut d = json!({ "mediaType": v.media_type, "digest": v.digest, "size": v.size });
            if let Some(a) = v.metadata["artifact_type"].as_str() {
                d["artifactType"] = json!(a);
            }
            if v.metadata["annotations"].as_object().is_some_and(|a| !a.is_empty()) {
                d["annotations"] = v.metadata["annotations"].clone();
            }
            d
        })
        .collect();
    let body = json!({ "schemaVersion": 2, "mediaType": MT_OCI_INDEX, "manifests": manifests });
    let mut res = (StatusCode::OK, axum::Json(body)).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(MT_OCI_INDEX));
    if filter.is_some() {
        res.headers_mut().insert("oci-filters-applied", HeaderValue::from_static("artifactType"));
    }
    Ok(res)
}
