//! Unencoded branch/ref names containing `/` in URL paths (`/branches/feature/x/protection`,
//! `/commits/release/1.0/status`), like GitHub.
//!
//! matchit (axum's router) cannot register a catch-all next to sibling parameter routes, so the
//! per-ref routes of several modules are mounted on an inner router, and the public router has a
//! single catch-all per prefix. The dispatcher resolves the longest existing branch/ref whose
//! remaining suffix is a known sub-resource, re-encodes the ref as one path segment (`%2F`) and
//! hands the request to the inner router, so the existing handlers see a normal `{branch}` /
//! `{ref}` parameter. Already-encoded forms (`feature%2Fx`) keep working unchanged.
use axum::extract::{OriginalUri, Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use percent_encoding::percent_decode_str;
use tower::ServiceExt;

use crate::json::git::enc_component;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/branches/{*rest}", any(dispatch_branches))
        .route("/repos/{owner}/{repo}/commits/{*rest}", any(dispatch_commits))
}

/// The routes whose `{branch}` / `{ref}` segment may contain `/` when unencoded.
fn inner_routes() -> Router<AppState> {
    Router::new()
        .merge(super::branches::branch_ref_routes())
        .merge(super::repo_extras::protection_routes())
        .merge(super::commits::commit_ref_routes())
        .merge(super::statuses::commit_ref_routes())
        .merge(super::commit_comments::commit_ref_routes())
        .merge(super::pulls::commit_ref_routes())
        .fallback(|| async { crate::error::ApiError::not_found() })
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Branches,
    Commits,
}

impl Kind {
    fn segment(self) -> &'static str {
        match self {
            Kind::Branches => "branches",
            Kind::Commits => "commits",
        }
    }

    /// Whether `suffix` (raw path segments after the ref) names a sub-resource.
    fn suffix_ok(self, suffix: &[&str]) -> bool {
        let Some(first) = suffix.first() else { return true };
        match self {
            Kind::Branches => matches!(*first, "rename" | "protection"),
            Kind::Commits => {
                suffix.len() == 1
                    && matches!(*first, "status" | "statuses" | "check-runs" | "check-suites" | "comments" | "pulls" | "branches-where-head")
            }
        }
    }
}

async fn dispatch_branches(State(s): State<AppState>, req: Request) -> Response {
    dispatch(s, req, Kind::Branches).await
}

async fn dispatch_commits(State(s): State<AppState>, req: Request) -> Response {
    dispatch(s, req, Kind::Commits).await
}

fn decode(seg: &str) -> String {
    percent_decode_str(seg).decode_utf8_lossy().into_owned()
}

/// Ref names (without `refs/heads/` / `refs/tags/`) of the repository, for longest-match
/// resolution. Visibility is checked later by the handler itself (routing never reveals more
/// than the handler's own 404).
async fn ref_names(s: &AppState, owner: &str, repo: &str, kind: Kind) -> Vec<String> {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT r.id FROM repositories r JOIN users u ON u.id = r.owner_id WHERE lower(u.login) = lower($1) AND lower(r.name) = lower($2)",
    )
    .bind(owner)
    .bind(repo)
    .fetch_optional(&s.db)
    .await
    .ok()
    .flatten();
    let Some(id) = id else { return vec![] };
    let Ok((_, refs)) = s.git.refs(id).await else { return vec![] };
    refs.keys()
        .filter_map(|k| {
            k.strip_prefix("refs/heads/").or_else(|| if kind == Kind::Commits { k.strip_prefix("refs/tags/") } else { None })
        })
        .map(str::to_string)
        .collect()
}

/// Splits the segments after `/branches/` or `/commits/` into (ref, suffix): the longest ref that
/// exists with a known suffix, else the longest candidate with a known suffix.
fn split<'a>(rest: &[&'a str], kind: Kind, exists: impl Fn(&str) -> bool) -> (String, Vec<&'a str>) {
    let n = rest.len();
    for k in (1..=n).rev() {
        let name = rest[..k].iter().map(|x| decode(x)).collect::<Vec<_>>().join("/");
        if kind.suffix_ok(&rest[k..]) && exists(&name) {
            return (name, rest[k..].to_vec());
        }
    }
    for k in (1..n).rev() {
        if kind.suffix_ok(&rest[k..]) {
            let name = rest[..k].iter().map(|x| decode(x)).collect::<Vec<_>>().join("/");
            return (name, rest[k..].to_vec());
        }
    }
    (rest.iter().map(|x| decode(x)).collect::<Vec<_>>().join("/"), vec![])
}

fn looks_like_sha(r: &str) -> bool {
    (4..=40).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn dispatch(s: AppState, req: Request, kind: Kind) -> Response {
    let path = req.uri().path().to_string();
    let segs: Vec<&str> = path.split('/').collect();
    // ["", "repos", owner, repo, kind, rest...]
    if segs.len() < 6 || segs[4] != kind.segment() {
        return crate::error::ApiError::not_found().into_response();
    }
    let (owner, repo) = (decode(segs[2]), decode(segs[3]));
    let mut rest: Vec<&str> = segs[5..].to_vec();
    if rest.len() > 1 && rest.last() == Some(&"") {
        rest.pop();
    }
    let (name, suffix) = if rest.len() == 1 {
        (decode(rest[0]), vec![])
    } else {
        let names = ref_names(&s, &owner, &repo, kind).await;
        split(&rest, kind, |r| names.iter().any(|n| n == r) || (kind == Kind::Commits && (looks_like_sha(r) || r == "HEAD")))
    };
    let mut new_path = format!("/repos/{}/{}/{}/{}", segs[2], segs[3], kind.segment(), enc_component(&name));
    for x in &suffix {
        new_path.push('/');
        new_path.push_str(x);
    }
    if let Some(q) = req.uri().query() {
        new_path.push('?');
        new_path.push_str(q);
    }
    let (parts, body) = req.into_parts();
    let Ok(uri) = new_path.parse::<axum::http::Uri>() else {
        return crate::error::ApiError::not_found().into_response();
    };
    let mut inner = Request::new(body);
    *inner.method_mut() = parts.method;
    *inner.uri_mut() = uri;
    *inner.version_mut() = parts.version;
    *inner.headers_mut() = parts.headers;
    if let Some(o) = parts.extensions.get::<OriginalUri>() {
        inner.extensions_mut().insert(o.clone());
    }
    match inner_routes().with_state(s).oneshot(inner).await {
        Ok(r) => r,
        Err(e) => match e {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_longest_existing_ref() {
        let names = ["feature/x", "feature"];
        let ex = |r: &str| names.contains(&r);
        let (n, s) = split(&["feature", "x", "protection", "enforce_admins"], Kind::Branches, ex);
        assert_eq!((n.as_str(), s), ("feature/x", vec!["protection", "enforce_admins"]));
        let (n, s) = split(&["feature", "x"], Kind::Branches, ex);
        assert_eq!((n.as_str(), s), ("feature/x", vec![]));
        let (n, s) = split(&["feature", "rename"], Kind::Branches, ex);
        assert_eq!((n.as_str(), s), ("feature", vec!["rename"]));
        let (n, s) = split(&["nope", "y", "status"], Kind::Commits, ex);
        assert_eq!((n.as_str(), s), ("nope/y", vec!["status"]));
        let (n, s) = split(&["feature%2Fx", "status"], Kind::Commits, ex);
        assert_eq!((n.as_str(), s), ("feature/x", vec!["status"]));
    }
}
