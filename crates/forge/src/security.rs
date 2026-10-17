//! Security helpers shared across areas: outbound-request (SSRF) address filtering for webhooks,
//! login throttling, and response hardening for user-controlled content. See docs/security.md.
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};

use axum::http::{header, HeaderValue};
use axum::response::Response;

// ---------------------------------------------------------------------------------------------
// SSRF: outbound webhook destinations

static ALLOW_PRIVATE: AtomicBool = AtomicBool::new(false);

/// Lets webhooks reach loopback/private/link-local addresses (tests, or
/// `FORGE_WEBHOOK_ALLOW_PRIVATE=1` for deployments that deliver to in-cluster services).
pub fn set_webhook_allow_private(allow: bool) {
    ALLOW_PRIVATE.store(allow, Ordering::Relaxed);
}

pub fn webhook_allow_private() -> bool {
    ALLOW_PRIVATE.load(Ordering::Relaxed) || std::env::var("FORGE_WEBHOOK_ALLOW_PRIVATE").is_ok_and(|v| v == "1")
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0 // "this network"
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64/10 CGNAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0/24 IETF protocol assignments
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18/15 benchmarking
        || o[0] >= 240) // reserved
}

/// True for addresses that are routable on the public Internet (webhook targets).
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let seg = v6.segments();
            // IPv4-compatible (::a.b.c.d) and NAT64 (64:ff9b::/96) embed an IPv4 address.
            if seg[..6] == [0, 0, 0, 0, 0, 0] || seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let v4 = Ipv4Addr::new((seg[6] >> 8) as u8, seg[6] as u8, (seg[7] >> 8) as u8, seg[7] as u8);
                return public_v4(v4);
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (seg[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
                || seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
        }
    }
}

/// True when `host` (a URL host) is an IP literal or name that must never be a webhook target.
pub fn blocked_literal_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Ipv4(ip) => !is_public_ip(IpAddr::V4(*ip)),
        url::Host::Ipv6(ip) => !is_public_ip(IpAddr::V6(*ip)),
        url::Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
    }
}

/// DNS resolver for the webhook client: drops non-public addresses at connect time, so a public
/// name that resolves (or is rebound) to an internal address can't be used to reach it.
pub struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let allow = webhook_allow_private();
            let ok: Vec<SocketAddr> = addrs.into_iter().filter(|a| allow || is_public_ip(a.ip())).collect();
            if ok.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{host} does not resolve to a public address"),
                )) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(ok.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Git transport

/// Global options for `git receive-pack` (HTTP and SSH): `refs/pull/*` is managed by forge
/// (pull request heads/merges) and, like on GitHub, cannot be created, moved or deleted by a
/// push. Hidden refs are also left out of the push advertisement.
pub const RECEIVE_PACK_CONFIG: [&str; 2] = ["-c", "receive.hideRefs=refs/pull"];

/// Upper bound on a push (bytes received by receive-pack after any gzip decoding):
/// `FORGE_MAX_PUSH_BYTES`, default 2 GiB like GitHub.
pub fn max_push_bytes() -> u64 {
    std::env::var("FORGE_MAX_PUSH_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(2 << 30)
}

/// Upper bound on a release asset upload: `FORGE_MAX_ASSET_BYTES`, default 2 GiB like GitHub.
pub fn max_asset_bytes() -> u64 {
    std::env::var("FORGE_MAX_ASSET_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(2 << 30)
}

/// Upper bound on an upload-pack (fetch negotiation) request body after gzip decoding. Real
/// clients send kilobytes; this bounds decompression bombs.
pub const MAX_UPLOAD_PACK_REQUEST: u64 = 64 << 20;

// ---------------------------------------------------------------------------------------------
// Diff output

static MAX_DIFF_OVERRIDE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Overrides `FORGE_MAX_DIFF_BYTES` for this process (tests); 0 restores the env/default.
pub fn set_max_diff_bytes(n: usize) {
    MAX_DIFF_OVERRIDE.store(n, Ordering::Relaxed);
}

/// Upper bound on a generated diff/patch held in memory: `FORGE_MAX_DIFF_BYTES`, default 10 MiB.
pub fn max_diff_bytes() -> usize {
    match MAX_DIFF_OVERRIDE.load(Ordering::Relaxed) {
        0 => std::env::var("FORGE_MAX_DIFF_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(10 << 20),
        n => n,
    }
}

/// A diff exceeded `max_diff_bytes()` (git was stopped; nothing past the limit was buffered).
#[derive(Debug)]
pub struct DiffTooLarge(pub usize);

impl std::fmt::Display for DiffTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "diff exceeds {} bytes", self.0)
    }
}

impl std::error::Error for DiffTooLarge {}

/// GitHub's answer for a `.diff`/`.patch` media type it won't render (406, code `too_large`).
pub fn diff_too_large(resource: &str, max: usize) -> crate::error::ApiError {
    let mut e = crate::error::ApiError::new(
        axum::http::StatusCode::NOT_ACCEPTABLE,
        format!(
            "Sorry, the diff exceeded the maximum size ({max} bytes). Consider using the JSON API (files) or locally cloning the repository instead."
        ),
    );
    e.errors = Some(serde_json::json!([{ "resource": resource, "field": "diff", "code": "too_large" }]));
    e
}

/// Maps a git error from a `.diff`/`.patch` rendering: `DiffTooLarge` becomes GitHub's 406.
pub fn diff_api_error(e: anyhow::Error, resource: &str) -> crate::error::ApiError {
    match e.downcast_ref::<DiffTooLarge>() {
        Some(t) => diff_too_large(resource, t.0),
        None => crate::error::ApiError::internal(format!("{e:#}")),
    }
}

/// Runs git and returns at most `max` bytes of stdout as `(bytes, truncated)`. When the output
/// is longer, git is killed as soon as the limit is reached, so memory stays bounded.
pub async fn git_output_capped(git: &crate::git::cmd::GitCmd, dir: &std::path::Path, args: &[&str], max: usize) -> anyhow::Result<(Vec<u8>, bool)> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    let mut child = git.command(dir).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(64 << 10).read_to_end(&mut buf).await;
        buf
    });
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut out = Vec::new();
    (&mut stdout).take(max as u64 + 1).read_to_end(&mut out).await?;
    if out.len() > max {
        let _ = child.kill().await;
        err_task.abort();
        out.truncate(max);
        return Ok((out, true));
    }
    let status = child.wait().await?;
    let err = err_task.await.unwrap_or_default();
    if !status.success() {
        anyhow::bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&err).trim());
    }
    Ok((out, false))
}

/// `git_output_capped` with `max_diff_bytes()`, failing with `DiffTooLarge` when exceeded.
pub async fn git_diff_capped(git: &crate::git::cmd::GitCmd, dir: &std::path::Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let max = max_diff_bytes();
    let (out, truncated) = git_output_capped(git, dir, args, max).await?;
    if truncated {
        return Err(DiffTooLarge(max).into());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Login throttling (shared by all replicas through Postgres, table `login_throttle`)

/// Failed password attempts allowed per (login, client address) within `WINDOW_SECS`.
const MAX_FAILURES_PER_LOGIN: i32 = 10;
/// Failed password attempts allowed per client address (any login) within `WINDOW_SECS`.
const MAX_FAILURES_PER_CLIENT: i32 = 100;
const WINDOW_SECS: i32 = 15 * 60;
/// Throttle keys embed the attempted login, which is attacker-chosen: bound its length.
const MAX_KEY_LOGIN: usize = 256;

fn keys(login: &str, client: Option<IpAddr>) -> (String, String) {
    let c = client.map(|ip| ip.to_string()).unwrap_or_else(|| "-".into());
    let login: String = login.to_lowercase().chars().take(MAX_KEY_LOGIN).collect();
    (format!("l:{login}|{c}"), format!("c:{c}"))
}

/// True if password attempts for `login` from `client` are currently throttled. One indexed
/// read per attempt.
pub async fn login_throttled(db: &sqlx::PgPool, login: &str, client: Option<IpAddr>) -> Result<bool, sqlx::Error> {
    let (lk, ck) = keys(login, client);
    let ck = client.map(|_| ck);
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM login_throttle
                        WHERE window_start > now() - make_interval(secs => $5)
                          AND ((key = $1 AND count >= $3) OR (key = $2 AND count >= $4)))",
    )
    .bind(lk)
    .bind(ck)
    .bind(MAX_FAILURES_PER_LOGIN)
    .bind(MAX_FAILURES_PER_CLIENT)
    .bind(WINDOW_SECS as f64)
    .fetch_one(db)
    .await
}

/// Records the outcome of a password check: a success clears the (login, client) counter, a
/// failure bumps both counters in one atomic upsert (an expired window restarts at 1).
pub async fn record_login(db: &sqlx::PgPool, login: &str, client: Option<IpAddr>, success: bool) -> Result<(), sqlx::Error> {
    let (lk, ck) = keys(login, client);
    if success {
        sqlx::query("DELETE FROM login_throttle WHERE key = $1").bind(lk).execute(db).await?;
        return Ok(());
    }
    let mut ks = vec![lk];
    if client.is_some() {
        ks.push(ck);
    }
    // Same lock order in every transaction (no deadlocks between concurrent failures).
    ks.sort();
    sqlx::query(
        "INSERT INTO login_throttle AS t (key, window_start, count)
         SELECT k, now(), 1 FROM unnest($1::text[]) k
         ON CONFLICT (key) DO UPDATE SET
           count = CASE WHEN t.window_start <= now() - make_interval(secs => $2) THEN 1 ELSE t.count + 1 END,
           window_start = CASE WHEN t.window_start <= now() - make_interval(secs => $2) THEN now() ELSE t.window_start END",
    )
    .bind(&ks)
    .bind(WINDOW_SECS as f64)
    .execute(db)
    .await?;
    // Occasional pruning keeps the table at roughly the keys of the last window.
    if rand::random::<u8>() < 4 {
        sqlx::query("DELETE FROM login_throttle WHERE window_start <= now() - make_interval(secs => $1)")
            .bind(WINDOW_SECS as f64)
            .execute(db)
            .await?;
    }
    Ok(())
}

/// Client address of a request: the peer address when the server was started with connect
/// info, or — only with `FORGE_TRUST_X_FORWARDED_FOR=1` (behind an ingress that appends it) —
/// the last `X-Forwarded-For` entry.
pub fn client_ip(parts: &axum::http::request::Parts) -> Option<IpAddr> {
    if std::env::var("FORGE_TRUST_X_FORWARDED_FOR").is_ok_and(|v| v == "1") {
        let fwd = parts
            .headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .filter_map(|s| s.trim().parse::<IpAddr>().ok())
            .last();
        if fwd.is_some() {
            return fwd;
        }
    }
    parts.extensions.get::<axum::extract::ConnectInfo<SocketAddr>>().map(|c| c.0.ip())
}

// ---------------------------------------------------------------------------------------------
// Archived repositories

/// Sub-resources of `/repos/{o}/{r}` that stay writable on an archived repository (GitHub:
/// settings/unarchive, deletion, subscriptions, forking, admin configuration).
const ARCHIVE_WRITABLE: &[&str] = &["", "subscription", "forks", "hooks", "keys", "collaborators", "invitations", "transfer", "notifications"];

/// Middleware for the REST API (mounted inside `/api/v3` and `/api/uploads`): archived
/// repositories are read-only. Mutating requests under `/repos/{o}/{r}/...` get GitHub's 403
/// unless the sub-resource is in `ARCHIVE_WRITABLE`. Callers that can't see the repo fall through
/// to the handler (404), so the check never reveals a private repository.
pub async fn archived_read_only(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use axum::http::Method;
    use axum::response::IntoResponse;
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let segs: Vec<String> = req.uri().path().trim_start_matches('/').split('/').map(str::to_string).collect();
    if segs.len() < 3 || segs[0] != "repos" || ARCHIVE_WRITABLE.contains(&segs.get(3).map(String::as_str).unwrap_or("")) {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let viewer = match crate::auth::authenticate(&mut parts, &state).await {
        Ok(crate::auth::AuthResult::User(u)) => Some(u),
        _ => None,
    };
    if let Ok(ctx) = crate::db::load_repo(&state.db, viewer.as_ref(), &segs[1], &segs[2]).await {
        if ctx.repo.archived {
            return crate::error::ApiError::forbidden("Repository was archived so is read-only.").into_response();
        }
    }
    next.run(axum::extract::Request::from_parts(parts, body)).await
}

// ---------------------------------------------------------------------------------------------
// Serving user content

/// Headers for responses whose body is user-controlled bytes (raw files, release assets, gist
/// raw): never sniff, never run scripts or load anything if a browser renders it anyway.
pub fn harden_user_content(res: &mut Response) {
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"));
}

/// Middleware: `X-Content-Type-Options: nosniff` on every response (plus a restrictive CSP
/// on non-HTML API/raw responses is set per route via `harden_user_content`).
pub async fn nosniff(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().entry(header::X_CONTENT_TYPE_OPTIONS).or_insert(HeaderValue::from_static("nosniff"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "::", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "::ffff:169.254.169.254", "64:ff9b::a9fe:a9fe", "::7f00:1"] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} must be blocked");
        }
        for ip in ["8.8.8.8", "140.82.112.3", "2606:4700::1111", "::ffff:8.8.8.8"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip} must be allowed");
        }
    }
}
