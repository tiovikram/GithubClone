//! Confinement of app credentials.
//!
//! [`middleware`] runs around every request (REST, uploads, GraphQL, git smart HTTP, web). For
//! an installation access token (`ghs_`) or a GitHub App user-to-server token (`ghu_`) it loads a
//! [`Scope`] (which repositories the token covers and with which permissions), classifies the
//! request into a GitHub App permission (`contents`, `issues`, `pull_requests`, ...) and
//!
//! * answers 403 "Resource not accessible by integration" when the token lacks that permission
//!   (or the request is outside what app tokens may do, e.g. `/api/forge`, `/user` for bots);
//! * otherwise runs the request inside a task-local scope. `db::permission` passes every
//!   computed permission through [`restrict`]: inside the scope the token's repositories get the
//!   permission level the request needs, other repositories at most public read; the bot user
//!   never has more than public read outside a scope.
//!
//! It also refuses writes to repositories locked by a migration (`crate::migration`).
use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Map, Value};

use crate::auth::AuthResult;
use crate::db::{Perm, Repo, User};
use crate::error::ApiError;
use crate::state::AppState;

pub const NOT_ACCESSIBLE: &str = "Resource not accessible by integration";

/// One account (and optionally a repository subset) a token covers.
#[derive(Debug, Clone)]
pub struct Coverage {
    pub target_id: i64,
    pub repos: Option<HashSet<i64>>,
}

#[derive(Debug, Clone)]
pub struct Scope {
    /// True for installation tokens (actor = the app's bot user), false for user-to-server.
    pub bot: bool,
    pub actor: User,
    pub app_id: i64,
    pub installation_id: Option<i64>,
    pub coverage: Vec<Coverage>,
    /// Token-level repository restriction (installation token `repository_ids`, scope-token).
    pub token_repos: Option<HashSet<i64>>,
    pub permissions: Map<String, Value>,
    /// The permission level this request may use on covered repositories.
    pub request_perm: Perm,
    pub token_hash: String,
}

tokio::task_local! {
    static CURRENT: Arc<Scope>;
}

/// The scope of the request being served, if it uses an app token.
pub fn current() -> Option<Arc<Scope>> {
    CURRENT.try_with(|s| s.clone()).ok()
}

/// The app acting through an installation token on this request (check runs are attributed to it).
pub fn current_app_id() -> Option<i64> {
    current().filter(|s| s.bot).map(|s| s.app_id)
}

/// Runs `f` inside `scope` (used by tests and background work done for a token).
pub async fn with_scope<F: std::future::Future>(scope: Scope, f: F) -> F::Output {
    CURRENT.scope(Arc::new(scope), f).await
}

impl Scope {
    pub fn covers(&self, repo: &Repo) -> bool {
        self.coverage.iter().any(|c| c.target_id == repo.owner_id && c.repos.as_ref().is_none_or(|s| s.contains(&repo.id)))
            && self.token_repos.as_ref().is_none_or(|s| s.contains(&repo.id))
    }
    pub fn level(&self, perm: &str) -> u8 {
        super::level_rank(self.permissions.get(perm).and_then(Value::as_str))
    }
}

/// Applies the current app scope to a permission computed by `db::permission`.
pub fn restrict(viewer: Option<&User>, repo: &Repo, p: Perm) -> Perm {
    let Some(u) = viewer else { return p };
    let base = if repo.private { Perm::None } else { Perm::Read };
    match current() {
        Some(sc) if sc.actor.id == u.id => {
            if !sc.covers(repo) {
                p.min(base)
            } else if sc.bot {
                sc.request_perm.max(base)
            } else {
                p.min(sc.request_perm.max(base))
            }
        }
        _ if u.kind == "Bot" => p.min(base),
        _ => p,
    }
}

/// The message used when a write is refused for an app token (instead of the PAT wording).
pub fn in_app_scope() -> bool {
    current().is_some()
}

// ---------------------------------------------------------------------------------------------
// Loading scopes

/// Extracts a bearer-ish credential: `token X`, `Bearer X` or the password of HTTP Basic.
pub fn credential(headers: &axum::http::HeaderMap) -> Option<String> {
    let h = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, cred) = h.split_once(' ')?;
    let cred = cred.trim();
    match scheme.to_ascii_lowercase().as_str() {
        "token" | "bearer" => Some(cred.to_string()),
        "basic" => {
            let decoded = base64::engine::general_purpose::STANDARD.decode(cred).ok()?;
            let s = String::from_utf8(decoded).ok()?;
            s.split_once(':').map(|(_, p)| p.to_string())
        }
        _ => None,
    }
}

#[derive(sqlx::FromRow)]
struct InstTokenRow {
    installation_id: i64,
    repository_ids: Option<Vec<i64>>,
    permissions: Value,
    app_id: i64,
    target_id: Option<i64>,
    repository_selection: String,
    bot_user_id: Option<i64>,
}

/// The bot user of a valid installation token (used by `auth::user_for_token`).
pub async fn bot_for_installation_token(db: &sqlx::PgPool, token: &str) -> Result<Option<User>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT u.* FROM app_installation_tokens t JOIN app_installations i ON i.id = t.installation_id
           JOIN apps a ON a.id = i.app_id JOIN users u ON u.id = a.bot_user_id
          WHERE t.token_hash = $1 AND t.expires_at > now() AND i.suspended_at IS NULL",
    )
    .bind(crate::auth::sha256_hex(token))
    .fetch_optional(db)
    .await?)
}

async fn selected_repos(db: &sqlx::PgPool, installation_id: i64) -> sqlx::Result<HashSet<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT repo_id FROM app_installation_repos WHERE installation_id = $1").bind(installation_id).fetch_all(db).await?;
    Ok(ids.into_iter().collect())
}

/// Loads the scope of an installation access token.
pub async fn installation_scope(db: &sqlx::PgPool, token: &str) -> anyhow::Result<Option<Scope>> {
    let hash = crate::auth::sha256_hex(token);
    let row: Option<InstTokenRow> = sqlx::query_as(
        "SELECT t.installation_id, t.repository_ids, t.permissions, i.app_id, i.target_id, i.repository_selection, a.bot_user_id
           FROM app_installation_tokens t JOIN app_installations i ON i.id = t.installation_id JOIN apps a ON a.id = i.app_id
          WHERE t.token_hash = $1 AND t.expires_at > now() AND i.suspended_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let Some(bot_id) = r.bot_user_id else { return Ok(None) };
    let actor = crate::db::user_by_id(db, bot_id).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let coverage = match r.target_id {
        Some(t) => vec![Coverage {
            target_id: t,
            repos: if r.repository_selection == "all" { None } else { Some(selected_repos(db, r.installation_id).await?) },
        }],
        None => vec![],
    };
    Ok(Some(Scope {
        bot: true,
        actor,
        app_id: r.app_id,
        installation_id: Some(r.installation_id),
        coverage,
        token_repos: r.repository_ids.map(|v| v.into_iter().collect()),
        permissions: r.permissions.as_object().cloned().unwrap_or_default(),
        request_perm: Perm::None,
        token_hash: hash,
    }))
}

#[derive(sqlx::FromRow)]
struct UserTokenRow {
    user_id: i64,
    app_id: i64,
    app_permissions: Value,
    target_id: Option<i64>,
    repository_ids: Option<Vec<i64>>,
    permissions: Option<Value>,
}

/// Loads the scope of a GitHub App user-to-server token (`ghu_`): the repositories of the app's
/// installations, with the app's permissions (narrowed by scope-token restrictions).
pub async fn user_token_scope(db: &sqlx::PgPool, token: &str) -> anyhow::Result<Option<Scope>> {
    let hash = crate::auth::sha256_hex(token);
    let row: Option<UserTokenRow> = sqlx::query_as(
        "SELECT oa.user_id, a.id AS app_id, a.permissions AS app_permissions, oa.target_id, oa.repository_ids, oa.permissions
           FROM oauth_authorizations oa JOIN tokens t ON t.id = oa.token_id JOIN apps a ON a.id = oa.app_id
          WHERE oa.hashed_token = $1 AND a.kind = 'app' AND (t.expires_at IS NULL OR t.expires_at > now())",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let actor = crate::db::user_by_id(db, r.user_id).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let insts: Vec<(i64, Option<i64>, String)> = sqlx::query_as(
        "SELECT id, target_id, repository_selection FROM app_installations WHERE app_id = $1 AND suspended_at IS NULL AND target_id IS NOT NULL",
    )
    .bind(r.app_id)
    .fetch_all(db)
    .await?;
    let mut coverage = Vec::new();
    for (id, target, sel) in insts {
        let Some(t) = target else { continue };
        if r.target_id.is_some_and(|x| x != t) {
            continue;
        }
        coverage.push(Coverage { target_id: t, repos: if sel == "all" { None } else { Some(selected_repos(db, id).await?) } });
    }
    let permissions = r.permissions.as_ref().and_then(Value::as_object).cloned().or_else(|| r.app_permissions.as_object().cloned()).unwrap_or_default();
    Ok(Some(Scope {
        bot: false,
        actor,
        app_id: r.app_id,
        installation_id: None,
        coverage,
        token_repos: r.repository_ids.map(|v| v.into_iter().collect()),
        permissions,
        request_perm: Perm::None,
        token_hash: hash,
    }))
}

// ---------------------------------------------------------------------------------------------
// Request classification

/// Permissions whose operations need repository admin rights in forge's permission model.
const ADMIN_PERMS: &[&str] = &[
    "administration", "repository_hooks", "secrets", "environments", "pages", "vulnerability_alerts",
    "repository_custom_properties", "dependabot_secrets", "security_events", "secret_scanning_alerts",
];

/// What a request touches.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A repository operation needing one of `perms` (`write` = write level).
    Repo { owner: String, repo: String, perms: &'static [&'static str], write: bool, git: bool },
    /// Any other API request, already decided.
    Allow,
    Deny,
    /// GraphQL: queries only.
    Graphql,
}

fn is_write(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The permission(s) that grant an operation on `/repos/{o}/{r}/<rest...>`.
pub fn repo_permission(method: &Method, rest: &[&str]) -> (&'static [&'static str], bool) {
    let w = is_write(method);
    let seg = |i: usize| rest.get(i).copied().unwrap_or("");
    let p: &'static [&'static str] = match seg(0) {
        "" => if w { &["administration"] } else { &["metadata"] },
        "issues" | "labels" | "milestones" | "assignees" => &["issues", "pull_requests"],
        "pulls" => {
            if w && matches!(seg(2), "merge" | "update-branch") {
                &["contents"]
            } else {
                &["pull_requests"]
            }
        }
        "commits" => match seg(2) {
            "check-runs" | "check-suites" => &["checks"],
            "status" | "statuses" => &["statuses"],
            "pulls" => &["pull_requests"],
            _ => &["contents"],
        },
        "branches" if seg(2) == "protection" => &["administration"],
        "contents" | "git" | "branches" | "compare" | "merges" | "tarball" | "zipball" | "readme" | "releases" | "tags"
        | "merge-upstream" | "dispatches" | "archive" | "comments" | "rename" => &["contents"],
        "statuses" => &["statuses"],
        "check-runs" | "check-suites" => &["checks"],
        "deployments" => &["deployments"],
        "environments" => if w { &["environments"] } else { &["actions", "deployments", "environments"] },
        "hooks" => &["repository_hooks"],
        "actions" => match seg(1) {
            "secrets" => &["secrets"],
            _ => &["actions"],
        },
        "pages" => &["pages"],
        "projects" => &["repository_projects"],
        "code-scanning" => &["security_events"],
        "secret-scanning" => &["secret_scanning_alerts"],
        "dependabot" => if seg(1) == "secrets" { &["dependabot_secrets"] } else { &["vulnerability_alerts"] },
        "properties" => &["repository_custom_properties"],
        "collaborators" | "teams" | "topics" | "forks" if !w => &["metadata"],
        "subscription" | "stargazers" | "watchers" | "events" | "activity" | "stats" | "traffic" | "languages"
        | "contributors" | "license" | "community" | "codeowners" | "installation" | "subscribers"
            if !w => &["metadata"],
        _ => if w { &["administration"] } else { &["metadata"] },
    };
    (p, w)
}

/// Classifies a request (full path, outside any nest).
pub fn classify(method: &Method, path: &str, query: Option<&str>, bot: bool) -> Target {
    let w = is_write(method);
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let repo_target = |o: &str, r: &str, rest: &[&str]| {
        let (perms, write) = repo_permission(method, rest);
        Target::Repo { owner: o.to_string(), repo: r.to_string(), perms, write, git: false }
    };
    match segs.as_slice() {
        ["api", "v3", rest @ ..] => match rest {
            ["repos", o, r, tail @ ..] => repo_target(o, r, tail),
            ["installation", ..] | ["app", ..] | ["app-manifests", ..] | ["applications", ..] | ["authorizations", ..] => Target::Allow,
            ["user", "installations", ..] if !bot => Target::Allow,
            ["user" | "users", ..] if bot && w => Target::Deny,
            ["user", ..] if bot => Target::Deny,
            ["user", tail @ ..] => {
                if !w {
                    return Target::Allow;
                }
                // user-to-server writes need the matching user permission
                let need: &str = match tail.first().copied().unwrap_or("") {
                    "" => "profile",
                    "keys" => "git_ssh_keys",
                    "gpg_keys" => "gpg_keys",
                    "emails" => "email_addresses",
                    "following" => "followers",
                    "starred" => "starring",
                    "repos" => "administration",
                    _ => return Target::Deny,
                };
                Target::Repo { owner: String::new(), repo: String::new(), perms: user_perm(need), write: true, git: false }
            }
            ["gists", ..] | ["admin", ..] | ["enterprises", ..] | ["setup", ..] | ["notifications", ..] if w || bot => Target::Deny,
            ["markdown", ..] => Target::Allow,
            _ if w => match rest {
                ["orgs", _, tail @ ..] => {
                    let need: &'static [&'static str] = match tail.first().copied().unwrap_or("") {
                        "members" | "memberships" | "teams" | "invitations" | "outside_collaborators" => &["members"],
                        "hooks" => &["organization_hooks"],
                        "projects" => &["organization_projects"],
                        "blocks" => &["organization_user_blocking"],
                        "actions" => &["organization_secrets", "organization_self_hosted_runners"],
                        _ => &["organization_administration"],
                    };
                    Target::Repo { owner: String::new(), repo: String::new(), perms: need, write: true, git: false }
                }
                _ => Target::Deny,
            },
            _ => Target::Allow,
        },
        ["api", "uploads", "repos", o, r, ..] => Target::Repo { owner: o.to_string(), repo: r.to_string(), perms: &["contents"], write: true, git: false },
        ["api", "graphql", ..] => Target::Graphql,
        ["api", ..] => Target::Deny,
        ["login", ..] => Target::Deny,
        [o, r, "info", "refs"] => {
            let push = query.is_some_and(|q| q.contains("service=git-receive-pack"));
            Target::Repo { owner: o.to_string(), repo: r.to_string(), perms: &["contents"], write: push, git: true }
        }
        [o, r, "git-upload-pack"] => Target::Repo { owner: o.to_string(), repo: r.to_string(), perms: &["contents"], write: false, git: true },
        [o, r, "git-receive-pack"] => Target::Repo { owner: o.to_string(), repo: r.to_string(), perms: &["contents"], write: true, git: true },
        [o, r, _, ..] if !w => Target::Repo { owner: o.to_string(), repo: r.to_string(), perms: &["contents"], write: false, git: false },
        _ if w => Target::Deny,
        _ => Target::Allow,
    }
}

fn user_perm(p: &str) -> &'static [&'static str] {
    match p {
        "profile" => &["profile"],
        "git_ssh_keys" => &["git_ssh_keys"],
        "gpg_keys" => &["gpg_keys"],
        "email_addresses" => &["email_addresses"],
        "followers" => &["followers"],
        "starring" => &["starring"],
        _ => &["administration"],
    }
}

/// The forge permission level a granted app permission maps to for this request.
fn perm_for(perms: &[&str], write: bool) -> Perm {
    if perms.first().is_some_and(|p| ADMIN_PERMS.contains(p)) {
        Perm::Admin
    } else if write {
        Perm::Write
    } else {
        Perm::Read
    }
}

fn granted(sc: &Scope, perms: &[&str], write: bool) -> bool {
    let need = if write { 2 } else { 1 };
    perms.iter().any(|p| *p == "metadata" && !write || sc.level(p) >= need)
}

fn deny(git: bool, msg: &str) -> Response {
    if git {
        (StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "text/plain")], format!("{msg}\n")).into_response()
    } else {
        ApiError::forbidden(msg).into_response()
    }
}

/// Decides an app-token request: Ok(request permission) or Err(response).
async fn decide(s: &AppState, sc: &Scope, target: &Target) -> Result<Perm, Response> {
    match target {
        Target::Allow => Ok(Perm::Read),
        Target::Graphql => Ok(Perm::Read),
        Target::Deny => Err(deny(false, NOT_ACCESSIBLE)),
        Target::Repo { owner, repo, perms, write, git } if owner.is_empty() => {
            // account-level permission (user or org endpoints)
            let _ = repo;
            if granted(sc, perms, *write) { Ok(Perm::Read) } else { Err(deny(*git, NOT_ACCESSIBLE)) }
        }
        Target::Repo { owner, repo, perms, write, git } => {
            let name = repo.strip_suffix(".git").unwrap_or(repo);
            let row: Option<Repo> = sqlx::query_as(
                "SELECT r.* FROM repositories r JOIN users u ON u.id = r.owner_id WHERE lower(u.login) = lower($1) AND lower(r.name) = lower($2)",
            )
            .bind(owner)
            .bind(name)
            .fetch_optional(&s.db)
            .await
            .map_err(|e| ApiError::from(e).into_response())?;
            let Some(r) = row else { return Ok(perm_for(perms, *write)) }; // handler answers 404
            if !sc.covers(&r) {
                // Other repositories: public reads only (private ones stay 404 through restrict()).
                return if *write && !r.private { Err(deny(*git, NOT_ACCESSIBLE)) } else { Ok(Perm::Read) };
            }
            if granted(sc, perms, *write) {
                Ok(perm_for(perms, *write))
            } else {
                Err(deny(*git, NOT_ACCESSIBLE))
            }
        }
    }
}

/// See the module docs. Mounted on the whole application router.
pub async fn middleware(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    if let Some(resp) = crate::migration::lock_guard(&s, &mut req, &method, &path, query.as_deref()).await {
        return resp;
    }
    let cred = credential(req.headers());
    let scope = match cred.as_deref() {
        Some(t) if t.starts_with("ghs_") => installation_scope(&s.db, t).await,
        Some(t) if t.starts_with("ghu_") => user_token_scope(&s.db, t).await,
        _ => return next.run(req).await,
    };
    let mut sc = match scope {
        Ok(Some(sc)) => sc,
        Ok(None) => return next.run(req).await, // invalid/expired: the auth layer answers 401
        Err(e) => return ApiError::internal(format!("{e:#}")).into_response(),
    };
    let target = classify(&method, &path, query.as_deref(), sc.bot);
    match decide(&s, &sc, &target).await {
        Ok(p) => sc.request_perm = p,
        Err(resp) => return resp,
    }
    req.extensions_mut().insert(AuthResult::User(sc.actor.clone()));
    CURRENT.scope(Arc::new(sc), next.run(req)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        let g = Method::GET;
        let p = Method::POST;
        assert_eq!(repo_permission(&p, &["issues"]), (&["issues", "pull_requests"][..], true));
        assert_eq!(repo_permission(&Method::PUT, &["pulls", "1", "merge"]), (&["contents"][..], true));
        assert_eq!(repo_permission(&g, &[]), (&["metadata"][..], false));
        assert_eq!(repo_permission(&Method::PATCH, &[]), (&["administration"][..], true));
        assert_eq!(repo_permission(&g, &["commits", "abc", "check-runs"]), (&["checks"][..], false));
        assert!(matches!(classify(&g, "/api/v3/user", None, true), Target::Deny));
        assert!(matches!(classify(&g, "/api/v3/user", None, false), Target::Allow));
        assert!(matches!(classify(&p, "/api/forge/tokens", None, true), Target::Deny));
        assert!(matches!(classify(&p, "/o/r.git/git-receive-pack", None, true), Target::Repo { write: true, git: true, .. }));
        assert!(matches!(classify(&g, "/o/r/info/refs", Some("service=git-upload-pack"), true), Target::Repo { write: false, git: true, .. }));
        assert!(matches!(classify(&g, "/api/v3/installation/repositories", None, true), Target::Allow));
        assert_eq!(perm_for(&["repository_hooks"], false), Perm::Admin);
        assert_eq!(perm_for(&["issues"], true), Perm::Write);
    }
}
