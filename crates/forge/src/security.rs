//! Security helpers shared across areas: outbound-request (SSRF) address filtering for webhooks,
//! login throttling, and response hardening for user-controlled content. See docs/security.md.
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
// Login throttling

/// Failed password attempts allowed per (login, client address) within `WINDOW`.
const MAX_FAILURES_PER_LOGIN: u32 = 10;
/// Failed password attempts allowed per client address (any login) within `WINDOW`.
const MAX_FAILURES_PER_CLIENT: u32 = 100;
const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Bound on tracked keys; expired entries are purged when it is reached.
const MAX_KEYS: usize = 100_000;

struct Counter {
    failures: u32,
    since: Instant,
}

#[derive(Default)]
struct Throttle {
    keys: HashMap<String, Counter>,
}

static THROTTLE: Mutex<Option<Throttle>> = Mutex::new(None);

fn keys(login: &str, client: Option<IpAddr>) -> (String, String) {
    let c = client.map(|ip| ip.to_string()).unwrap_or_else(|| "-".into());
    (format!("l:{}|{c}", login.to_lowercase()), format!("c:{c}"))
}

fn over(t: &Throttle, key: &str, max: u32) -> bool {
    t.keys.get(key).is_some_and(|c| c.since.elapsed() < WINDOW && c.failures >= max)
}

/// True if password attempts for `login` from `client` are currently throttled.
pub fn login_throttled(login: &str, client: Option<IpAddr>) -> bool {
    let (lk, ck) = keys(login, client);
    let g = THROTTLE.lock().unwrap();
    let Some(t) = g.as_ref() else { return false };
    over(t, &lk, MAX_FAILURES_PER_LOGIN) || (client.is_some() && over(t, &ck, MAX_FAILURES_PER_CLIENT))
}

/// Records the outcome of a password check.
pub fn record_login(login: &str, client: Option<IpAddr>, success: bool) {
    let (lk, ck) = keys(login, client);
    let mut g = THROTTLE.lock().unwrap();
    let t = g.get_or_insert_with(Throttle::default);
    if success {
        t.keys.remove(&lk);
        return;
    }
    if t.keys.len() >= MAX_KEYS {
        t.keys.retain(|_, c| c.since.elapsed() < WINDOW);
        if t.keys.len() >= MAX_KEYS {
            // Still full: drop the oldest half rather than grow without bound.
            let mut ages: Vec<Instant> = t.keys.values().map(|c| c.since).collect();
            ages.sort();
            let cutoff = ages[ages.len() / 2];
            t.keys.retain(|_, c| c.since > cutoff);
        }
    }
    let keys: &[String] = if client.is_some() { &[lk, ck] } else { std::slice::from_ref(&lk) };
    for k in keys {
        let c = t.keys.entry(k.clone()).or_insert(Counter { failures: 0, since: Instant::now() });
        if c.since.elapsed() >= WINDOW {
            *c = Counter { failures: 0, since: Instant::now() };
        }
        c.failures += 1;
    }
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

    #[test]
    fn throttles_after_failures() {
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let login = format!("throttle-{}", crate::auth::random_string(6));
        for _ in 0..MAX_FAILURES_PER_LOGIN {
            assert!(!login_throttled(&login, Some(ip)));
            record_login(&login, Some(ip), false);
        }
        assert!(login_throttled(&login, Some(ip)));
        // Other clients are unaffected.
        assert!(!login_throttled(&login, Some("203.0.113.10".parse().unwrap())));
    }
}
