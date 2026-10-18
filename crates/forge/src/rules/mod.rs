//! Ruleset enforcement (repository and organization rulesets, GHES 3.22 rule types).
//!
//! Rulesets are stored by `api::rulesets`; this module evaluates them against ref updates and
//! decides whether the update may happen:
//! - git pushes (HTTP and SSH): the branch-protection `pre-receive` hook (`protection.rs`) hands
//!   the pushed ref updates to the server through a per-push directory ([`hook`]), and forge
//!   evaluates them in-process with git's quarantine environment (the pushed objects are not in
//!   the repository yet). Violations reject the push with GitHub's `GH013` message.
//! - API writes (contents, git refs, merges, pull request merges): [`enforce_api`] is called
//!   inside the git write, after the new commit exists and before the ref moves.
//!
//! Enforcement modes: `active` rulesets block, `evaluate` ones are only recorded, `disabled`
//! ones are ignored. Bypass actors (repository roles, teams, users, apps, deploy keys,
//! organization admins, enterprise owners) with `bypass_mode` `always` bypass everywhere,
//! `pull_request` only when merging a pull request, and `exempt` skip the ruleset entirely
//! (no evaluation recorded). Every evaluated ref update is recorded as a rule suite with
//! `pass` / `fail` / `bypass`. Push rulesets with bypass actors use delegated bypass: a blocked
//! pusher gets a push bypass request ([`bypass`]) that a bypass actor can approve; pushing the
//! same commit again then succeeds.
pub mod bypass;
pub mod hook;
pub mod sig;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;

use axum::http::StatusCode;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use crate::api::rulesets::{self, Level, RulesetRow};
use crate::db::{Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::git::read::CommitInfo;
use crate::pulls::{MergeMethod, Pull};
use crate::state::AppState;
use crate::webhooks::ZERO_SHA;

/// Who is updating refs.
#[derive(Debug, Clone)]
pub struct Actor {
    pub user: Option<User>,
    pub perm: Perm,
    /// The GitHub App acting through an installation token (e.g. `github-actions`).
    pub app_id: Option<i64>,
    /// The push authenticated with a deploy key.
    pub deploy_key: bool,
}

impl Actor {
    /// The caller of the current request (app scope from the request's token).
    pub fn new(ctx: &RepoCtx, user: &User) -> Actor {
        Actor { user: Some(user.clone()), perm: ctx.perm, app_id: crate::apps::scope::current_app_id(), deploy_key: false }
    }
}

/// How the refs are being changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// `git push`.
    Push,
    /// Git refs API (`POST/PATCH/DELETE /git/refs`): commits are whatever the caller made.
    RefApi,
    /// forge writes a new commit itself (contents API, `POST /merges`); such commits count as
    /// signed, like GitHub's web-flow signed commits.
    Commit,
    /// Merging a pull request (method unknown when only computing the merge state).
    Merge(Option<MergeMethod>),
    /// Syncing a fork branch with its upstream (`merge-upstream`).
    Upstream,
}

impl Via {
    fn is_merge(self) -> bool {
        matches!(self, Via::Merge(_))
    }
    fn forge_commits(self) -> bool {
        matches!(self, Via::Commit | Via::Merge(_) | Via::Upstream)
    }
}

/// One ref update (zero sha = created / deleted).
#[derive(Debug, Clone)]
pub struct RefUpdate {
    pub refname: String,
    pub old: String,
    pub new: String,
}

impl RefUpdate {
    pub fn new(refname: &str, old: Option<&str>, new: Option<&str>) -> RefUpdate {
        RefUpdate { refname: refname.to_string(), old: old.unwrap_or(ZERO_SHA).to_string(), new: new.unwrap_or(ZERO_SHA).to_string() }
    }
    fn created(&self) -> bool {
        self.old == ZERO_SHA
    }
    fn deleted(&self) -> bool {
        self.new == ZERO_SHA
    }
}

/// Read access to the repository being written (with git's quarantine environment for pushes).
#[derive(Debug, Clone)]
pub struct GitView {
    pub bin: String,
    pub path: PathBuf,
    pub env: Vec<(String, String)>,
}

impl GitView {
    pub fn new(s: &AppState, path: &std::path::Path) -> GitView {
        GitView { bin: s.cfg.git_bin.clone(), path: path.to_path_buf(), env: vec![] }
    }

    async fn run(&self, args: &[&str], input: Option<&[u8]>) -> anyhow::Result<(bool, Vec<u8>)> {
        let mut c = tokio::process::Command::new(&self.bin);
        c.current_dir(&self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .args(args)
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = c.spawn()?;
        if let Some(data) = input {
            let mut stdin = child.stdin.take().expect("stdin");
            let data = data.to_vec();
            tokio::spawn(async move {
                let _ = stdin.write_all(&data).await;
                let _ = stdin.shutdown().await;
            });
        }
        let out = child.wait_with_output().await?;
        Ok((out.status.success(), out.stdout))
    }

    async fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.run(&["merge-base", "--is-ancestor", a, b], None).await.map(|(ok, _)| ok).unwrap_or(true)
    }

    /// Commits reachable from `new` and not from any existing ref (newest first, at most 1000).
    async fn new_commits(&self, new: &str) -> anyhow::Result<Vec<CommitInfo>> {
        let (ok, out) = self.run(&["rev-list", "--max-count=1000", new, "--not", "--all"], None).await?;
        if !ok {
            return Ok(vec![]);
        }
        let shas: Vec<String> = String::from_utf8_lossy(&out).lines().map(str::to_string).filter(|l| crate::git::read::is_sha(l)).collect();
        if shas.is_empty() {
            return Ok(vec![]);
        }
        let input = shas.join("\n") + "\n";
        let (_, raw) = self.run(&["cat-file", "--batch"], Some(input.as_bytes())).await?;
        let mut out = Vec::new();
        let mut rest: &[u8] = &raw;
        while let Some(nl) = rest.iter().position(|&b| b == b'\n') {
            let header = String::from_utf8_lossy(&rest[..nl]).to_string();
            rest = &rest[nl + 1..];
            let f: Vec<&str> = header.split(' ').collect();
            if f.len() != 3 {
                continue;
            }
            let size: usize = f[2].parse().unwrap_or(0);
            if rest.len() < size {
                break;
            }
            if f[1] == "commit" {
                out.push(crate::git::read::parse_commit(f[0], &rest[..size]));
            }
            rest = &rest[(size + 1).min(rest.len())..];
        }
        Ok(out)
    }

    /// Files changed by each commit relative to its first parent: (path, new blob or None for a
    /// deletion).
    async fn changed_files(&self, commits: &[CommitInfo]) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        let mut seen = HashSet::new();
        for c in commits {
            let args: Vec<&str> = match c.parents.first() {
                Some(p) => vec!["diff-tree", "-r", "-z", "--no-renames", p.as_str(), c.sha.as_str()],
                None => vec!["diff-tree", "-r", "-z", "--no-renames", "--root", "--no-commit-id", c.sha.as_str()],
            };
            let (ok, raw) = self.run(&args, None).await?;
            if !ok {
                continue;
            }
            let mut parts = raw.split(|&b| b == 0).filter(|p| !p.is_empty());
            while let Some(meta) = parts.next() {
                let meta = String::from_utf8_lossy(meta).to_string();
                if !meta.starts_with(':') {
                    continue;
                }
                let Some(path) = parts.next() else { break };
                let path = String::from_utf8_lossy(path).to_string();
                let f: Vec<&str> = meta.split(' ').collect();
                if f.len() < 5 {
                    continue;
                }
                let blob = (!f[4].starts_with('D') && f[1] != "160000").then(|| f[3].to_string());
                if seen.insert((path.clone(), blob.clone())) {
                    out.push((path, blob));
                }
            }
        }
        Ok(out)
    }

    async fn blob_sizes(&self, blobs: &[String]) -> anyhow::Result<HashMap<String, u64>> {
        if blobs.is_empty() {
            return Ok(HashMap::new());
        }
        let input = blobs.join("\n") + "\n";
        let (_, out) = self.run(&["cat-file", "--batch-check=%(objectname) %(objectsize)"], Some(input.as_bytes())).await?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|l| {
                let (a, b) = l.split_once(' ')?;
                Some((a.to_string(), b.parse().ok()?))
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------------------------
// Which rulesets apply

fn glob(pat: &[u8], s: &[u8]) -> bool {
    match pat.first() {
        None => s.is_empty(),
        Some(b'*') if pat.get(1) == Some(&b'*') => {
            let rest = if pat.get(2) == Some(&b'/') { &pat[3..] } else { &pat[2..] };
            (0..=s.len()).any(|i| glob(rest, &s[i..])) || glob(&pat[2..], s)
        }
        Some(b'*') => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| glob(&pat[1..], &s[i..])),
        Some(b'?') => s.first().is_some_and(|&c| c != b'/') && glob(&pat[1..], &s[1..]),
        Some(c) => s.first() == Some(c) && glob(&pat[1..], &s[1..]),
    }
}

/// fnmatch-style path match: `*` within a segment, `**` across segments; a pattern without `/`
/// also matches the file name in any directory.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    let p = pattern.trim_start_matches('/');
    if glob(p.as_bytes(), path.as_bytes()) {
        return true;
    }
    if !p.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        if glob(p.as_bytes(), name.as_bytes()) {
            return true;
        }
    }
    // `dir/` or `dir` restricts everything below it.
    let dir = p.trim_end_matches('/');
    path.starts_with(&format!("{dir}/"))
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn name_matches(pattern: &str, name: &str) -> bool {
    pattern == "~ALL" || glob(pattern.to_lowercase().as_bytes(), name.to_lowercase().as_bytes())
}

/// Whether an organization ruleset's repository condition selects `ctx`'s repository.
fn repo_condition(r: &RulesetRow, ctx: &RepoCtx, props: &HashMap<String, Value>) -> bool {
    let Some(c) = r.conditions.as_ref() else { return true };
    if let Some(rn) = c.get("repository_name") {
        let name = &ctx.repo.name;
        return str_list(rn.get("include")).iter().any(|p| name_matches(p, name)) && !str_list(rn.get("exclude")).iter().any(|p| name_matches(p, name));
    }
    if let Some(ri) = c.get("repository_id") {
        return ri.get("repository_ids").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x.as_i64() == Some(ctx.repo.id)));
    }
    if let Some(rp) = c.get("repository_property") {
        let matches = |spec: &Value| -> bool {
            let name = spec.get("name").and_then(Value::as_str).unwrap_or("");
            let wanted = str_list(spec.get("property_values"));
            let have: Vec<String> = if spec.get("source").and_then(Value::as_str) == Some("system") || name == "visibility" && !props.contains_key(name) {
                match name {
                    "visibility" => vec![ctx.repo.visibility.clone()],
                    "fork" => vec![ctx.repo.fork_parent_id.is_some().to_string()],
                    _ => vec![],
                }
            } else {
                match props.get(name) {
                    Some(Value::String(s)) => vec![s.clone()],
                    Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                    Some(Value::Bool(b)) => vec![b.to_string()],
                    _ => vec![],
                }
            };
            have.iter().any(|h| wanted.iter().any(|w| w.eq_ignore_ascii_case(h)))
        };
        let inc = rp.get("include").and_then(Value::as_array).cloned().unwrap_or_default();
        let exc = rp.get("exclude").and_then(Value::as_array).cloned().unwrap_or_default();
        return inc.iter().all(matches) && !exc.iter().any(matches);
    }
    true
}

/// Rulesets (repository + owning organization) that are not disabled and target `ctx`'s
/// repository.
pub async fn repo_rulesets(s: &AppState, ctx: &RepoCtx) -> ApiResult<Vec<RulesetRow>> {
    let all = rulesets::list_rulesets(s, &Level::Repo(ctx.clone()), true, None).await?;
    let all: Vec<RulesetRow> = all.into_iter().filter(|r| r.enforcement != "disabled").collect();
    if all.is_empty() {
        return Ok(all);
    }
    let props: HashMap<String, Value> = if all.iter().any(|r| r.org_id.is_some() || r.enterprise) {
        sqlx::query_as::<_, (String, Value)>("SELECT property_name, value FROM repo_custom_property_values WHERE repo_id = $1")
            .bind(ctx.repo.id)
            .fetch_all(&s.db)
            .await?
            .into_iter()
            .collect()
    } else {
        HashMap::new()
    };
    Ok(all
        .into_iter()
        .filter(|r| (r.org_id.is_none() && !r.enterprise) || (repo_condition(r, ctx, &props) && (!r.enterprise || org_condition(r, ctx))))
        .collect())
}

/// Whether an enterprise ruleset's `organization_name` condition selects `ctx`'s owner (no
/// condition selects every organization).
fn org_condition(r: &RulesetRow, ctx: &RepoCtx) -> bool {
    let Some(on) = r.conditions.as_ref().and_then(|c| c.get("organization_name")) else { return true };
    let name = &ctx.owner.login;
    str_list(on.get("include")).iter().any(|p| name_matches(p, name)) && !str_list(on.get("exclude")).iter().any(|p| name_matches(p, name))
}

/// Whether ruleset `r` applies to an update of `refname`.
pub fn targets_ref(r: &RulesetRow, refname: &str, default_branch: &str) -> bool {
    match r.target.as_str() {
        "branch" => refname.starts_with("refs/heads/") && rulesets::applies_to(r, refname, default_branch),
        "tag" => refname.starts_with("refs/tags/") && rulesets::applies_to(r, refname, default_branch),
        "push" => refname.starts_with("refs/heads/") || refname.starts_with("refs/tags/"),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Bypass

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BypassMode {
    Never,
    PullRequest,
    Always,
    Exempt,
}

/// Facts about the actor used to match bypass actors (loaded once per evaluation).
#[derive(Debug, Default)]
struct ActorFacts {
    team_ids: HashSet<i64>,
    org_admin: bool,
}

async fn actor_facts(s: &AppState, ctx: &RepoCtx, actor: &Actor) -> ActorFacts {
    let Some(u) = &actor.user else { return ActorFacts::default() };
    let team_ids: Vec<i64> = sqlx::query_scalar("SELECT tm.team_id FROM team_members tm JOIN teams t ON t.id = tm.team_id WHERE tm.user_id = $1 AND t.org_id = $2")
        .bind(u.id)
        .bind(ctx.owner.id)
        .fetch_all(&s.db)
        .await
        .unwrap_or_default();
    let org_admin = ctx.owner.kind == "Organization"
        && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2 AND role = 'admin')")
            .bind(ctx.owner.id)
            .bind(u.id)
            .fetch_one(&s.db)
            .await
            .unwrap_or(false);
    ActorFacts { team_ids: team_ids.into_iter().collect(), org_admin }
}

/// GitHub's base repository role ids (bypass actor `RepositoryRole`).
fn role_perm(id: i64) -> Option<Perm> {
    match id {
        1 => Some(Perm::Read),
        2 => Some(Perm::Maintain),
        3 => Some(Perm::Triage),
        4 => Some(Perm::Write),
        5 => Some(Perm::Admin),
        _ => None,
    }
}

fn mode_of(entry: &Value) -> BypassMode {
    match entry.get("bypass_mode").and_then(Value::as_str).unwrap_or("always") {
        "pull_request" => BypassMode::PullRequest,
        "exempt" => BypassMode::Exempt,
        _ => BypassMode::Always,
    }
}

fn entry_matches(entry: &Value, ctx: &RepoCtx, actor: &Actor, facts: &ActorFacts) -> bool {
    let id = entry.get("actor_id").and_then(Value::as_i64);
    match entry.get("actor_type").and_then(Value::as_str).unwrap_or("") {
        "DeployKey" => actor.deploy_key,
        _ if actor.deploy_key => false,
        "Integration" => actor.app_id.is_some() && actor.app_id == id,
        "OrganizationAdmin" => facts.org_admin || (ctx.owner.kind == "Organization" && actor.user.as_ref().is_some_and(|u| u.site_admin)),
        "EnterpriseOwner" => actor.user.as_ref().is_some_and(|u| u.site_admin),
        "RepositoryRole" => id.and_then(role_perm).is_some_and(|p| actor.perm >= p),
        "Team" => id.is_some_and(|t| facts.team_ids.contains(&t)),
        "User" => actor.user.as_ref().is_some_and(|u| Some(u.id) == id),
        _ => false,
    }
}

fn bypass_mode_with(r: &RulesetRow, ctx: &RepoCtx, actor: &Actor, facts: &ActorFacts) -> BypassMode {
    r.bypass_actors
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| entry_matches(e, ctx, actor, facts))
        .map(mode_of)
        .max()
        .unwrap_or(BypassMode::Never)
}

/// The bypass mode `actor` has for ruleset `r` on `ctx`'s repository.
pub async fn bypass_mode(s: &AppState, ctx: &RepoCtx, actor: &Actor, r: &RulesetRow) -> BypassMode {
    let facts = actor_facts(s, ctx, actor).await;
    bypass_mode_with(r, ctx, actor, &facts)
}

/// `current_user_can_bypass` of a ruleset for `user` (`always`, `pull_requests_only`, `never`,
/// `exempt`).
pub async fn current_user_can_bypass(s: &AppState, ctx: &RepoCtx, user: Option<&User>, r: &RulesetRow) -> &'static str {
    let Some(u) = user else { return "never" };
    match bypass_mode(s, ctx, &Actor::new(ctx, u), r).await {
        BypassMode::Never => "never",
        BypassMode::PullRequest => "pull_requests_only",
        BypassMode::Always => "always",
        BypassMode::Exempt => "exempt",
    }
}

// ---------------------------------------------------------------------------------------------
// Rule evaluation

/// Lazily computed facts about one ref update.
struct Facts<'a> {
    view: Option<&'a GitView>,
    upd: &'a RefUpdate,
    commits: Option<Vec<CommitInfo>>,
    files: Option<Vec<(String, Option<String>)>>,
    sizes: Option<HashMap<String, u64>>,
}

impl<'a> Facts<'a> {
    async fn commits(&mut self) -> &[CommitInfo] {
        if self.commits.is_none() {
            let c = match (self.view, self.upd.deleted()) {
                (Some(v), false) => v.new_commits(&self.upd.new).await.unwrap_or_default(),
                _ => vec![],
            };
            self.commits = Some(c);
        }
        self.commits.as_deref().unwrap_or_default()
    }

    async fn files(&mut self) -> Vec<(String, Option<String>)> {
        if self.files.is_none() {
            let commits = self.commits().await.to_vec();
            let f = match self.view {
                Some(v) => v.changed_files(&commits).await.unwrap_or_default(),
                None => vec![],
            };
            self.files = Some(f);
        }
        self.files.clone().unwrap_or_default()
    }

    async fn sizes(&mut self) -> HashMap<String, u64> {
        if self.sizes.is_none() {
            let blobs: Vec<String> = self.files().await.into_iter().filter_map(|(_, b)| b).collect::<HashSet<_>>().into_iter().collect();
            let m = match self.view {
                Some(v) => v.blob_sizes(&blobs).await.unwrap_or_default(),
                None => HashMap::new(),
            };
            self.sizes = Some(m);
        }
        self.sizes.clone().unwrap_or_default()
    }
}

fn pattern_ok(params: &Value, value: &str) -> bool {
    let pattern = params.get("pattern").and_then(Value::as_str).unwrap_or("");
    let negate = params.get("negate").and_then(Value::as_bool).unwrap_or(false);
    let hit = match params.get("operator").and_then(Value::as_str).unwrap_or("") {
        "starts_with" => value.starts_with(pattern),
        "ends_with" => value.ends_with(pattern),
        "contains" => value.contains(pattern),
        "regex" => regex::Regex::new(pattern).map(|re| re.is_match(value)).unwrap_or(false),
        _ => true,
    };
    hit != negate
}

fn pattern_message(subject: &str, params: &Value) -> String {
    let pattern = params.get("pattern").and_then(Value::as_str).unwrap_or("");
    let negate = params.get("negate").and_then(Value::as_bool).unwrap_or(false);
    let verb = match params.get("operator").and_then(Value::as_str).unwrap_or("") {
        "starts_with" => "start with",
        "ends_with" => "end with",
        "contains" => "contain",
        _ => "match",
    };
    let not = if negate { " not" } else { "" };
    format!("{subject} must{not} {verb} a given pattern: {pattern}")
}

fn found(n: usize, items: &[String]) -> String {
    let shown: Vec<&String> = items.iter().take(10).collect();
    let noun = if n == 1 { "violation" } else { "violations" };
    let mut out = format!("\n  Found {n} {noun}:\n");
    for i in shown {
        out.push_str(&format!("\n  - {i}"));
    }
    out
}

fn checks_of(params: &Value) -> Vec<(String, Option<i64>)> {
    params
        .get("required_status_checks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c.get("context")?.as_str()?.to_string(), c.get("integration_id").and_then(Value::as_i64))))
        .collect()
}

/// Required checks that have not succeeded on `sha` (commit statuses or check runs).
async fn missing_checks(s: &AppState, repo_id: i64, sha: &str, checks: &[(String, Option<i64>)]) -> ApiResult<Vec<String>> {
    if checks.is_empty() {
        return Ok(vec![]);
    }
    let contexts: Vec<String> = checks.iter().map(|c| c.0.clone()).collect();
    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT context FROM (SELECT DISTINCT ON (context) context, state FROM commit_statuses
           WHERE repo_id = $1 AND sha = $2 AND context = ANY($3) ORDER BY context, id DESC) s WHERE state = 'success'",
    )
    .bind(repo_id)
    .bind(sha)
    .bind(&contexts)
    .fetch_all(&s.db)
    .await?;
    let runs: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, app_id FROM (SELECT DISTINCT ON (cr.name, cs.app_id) cr.name, cs.app_id, cr.status, cr.conclusion
            FROM check_runs cr JOIN check_suites cs ON cs.id = cr.suite_id
           WHERE cr.repo_id = $1 AND cr.head_sha = $2 AND cr.name = ANY($3) ORDER BY cr.name, cs.app_id, cr.id DESC) x
          WHERE status = 'completed' AND conclusion IN ('success', 'neutral', 'skipped')",
    )
    .bind(repo_id)
    .bind(sha)
    .bind(&contexts)
    .fetch_all(&s.db)
    .await?;
    Ok(checks
        .iter()
        .filter(|(c, app)| {
            let by_status = app.is_none() && statuses.contains(c);
            let by_run = runs.iter().any(|(n, a)| n == c && app.is_none_or(|x| x == *a));
            !(by_status || by_run)
        })
        .map(|(c, _)| c.clone())
        .collect())
}

async fn missing_deployments(s: &AppState, repo_id: i64, sha: &str, envs: &[String]) -> ApiResult<Vec<String>> {
    if envs.is_empty() {
        return Ok(vec![]);
    }
    let ok: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT d.environment FROM deployments d
           JOIN LATERAL (SELECT state FROM deployment_statuses ds WHERE ds.deployment_id = d.id ORDER BY ds.id DESC LIMIT 1) st ON TRUE
          WHERE d.repo_id = $1 AND d.sha = $2 AND d.environment = ANY($3) AND st.state = 'success'",
    )
    .bind(repo_id)
    .bind(sha)
    .bind(envs)
    .fetch_all(&s.db)
    .await?;
    Ok(envs.iter().filter(|e| !ok.contains(e)).cloned().collect())
}

/// What is being evaluated, shared by every rule of one ref update.
struct Subject<'a> {
    s: &'a AppState,
    ctx: &'a RepoCtx,
    via: Via,
    pull: Option<&'a Pull>,
}

/// Evaluates one rule. `None` = pass, `Some(details)` = fail.
async fn eval_rule(sub: &Subject<'_>, rule: &Value, f: &mut Facts<'_>) -> ApiResult<Option<String>> {
    let upd = f.upd;
    let (created, deleted) = (upd.created(), upd.deleted());
    let updated = !created && !deleted;
    let params = rule.get("parameters").cloned().unwrap_or(Value::Null);
    let is_branch = upd.refname.starts_with("refs/heads/");
    // The commit whose checks / deployments count: the PR head when merging.
    let check_sha = match (sub.via, sub.pull) {
        (Via::Merge(_), Some(p)) => p.pr.head_sha.clone(),
        _ => upd.new.clone(),
    };
    let ty = rule.get("type").and_then(Value::as_str).unwrap_or("");
    Ok(match ty {
        "creation" => created.then(|| "Cannot create ref due to creations being restricted.".into()),
        "update" => {
            let fetch_ok = sub.via == Via::Upstream && params.get("update_allows_fetch_and_merge").and_then(Value::as_bool).unwrap_or(false);
            (updated && !fetch_ok).then(|| "Cannot update this protected ref.".into())
        }
        "deletion" => deleted.then(|| "Cannot delete this protected ref.".into()),
        "non_fast_forward" => {
            if !updated || f.view.is_none() {
                None
            } else {
                let ff = f.view.expect("view").is_ancestor(&upd.old, &upd.new).await;
                (!ff).then(|| "Cannot force-push to this branch".into())
            }
        }
        "required_linear_history" => {
            if matches!(sub.via, Via::Merge(Some(MergeMethod::Merge))) {
                Some("This branch must not contain merge commits.".into())
            } else if deleted {
                None
            } else {
                let merges: Vec<String> = f.commits().await.iter().filter(|c| c.parents.len() > 1).map(|c| c.sha.clone()).collect();
                (!merges.is_empty()).then(|| format!("This branch must not contain merge commits.{}", found(merges.len(), &merges)))
            }
        }
        "required_signatures" => {
            if deleted || sub.via.forge_commits() {
                None
            } else {
                let commits = f.commits().await.to_vec();
                let bad = sig::unverified(sub.s, &commits).await;
                (!bad.is_empty()).then(|| format!("Commits must have verified signatures.{}", found(bad.len(), &bad)))
            }
        }
        "pull_request" => {
            if !is_branch || !updated {
                None
            } else if let (Via::Merge(method), Some(pull)) = (sub.via, sub.pull) {
                pull_request_violation(sub.s, sub.ctx, pull, &params, method).await?
            } else {
                Some("Changes must be made through a pull request.".into())
            }
        }
        "required_status_checks" => {
            let skip_create = created && params.get("do_not_enforce_on_create").and_then(Value::as_bool).unwrap_or(false);
            if deleted || skip_create {
                None
            } else {
                let checks = checks_of(&params);
                let missing = if matches!(sub.via, Via::Commit | Via::Upstream) { checks.iter().map(|c| c.0.clone()).collect() } else { missing_checks(sub.s, sub.ctx.repo.id, &check_sha, &checks).await? };
                if !missing.is_empty() {
                    Some(crate::protection::status_message(&missing, checks.len()))
                } else if let (Via::Merge(_), Some(pull), true) = (sub.via, sub.pull, params.get("strict_required_status_checks_policy").and_then(Value::as_bool).unwrap_or(false)) {
                    let behind = match f.view {
                        Some(v) if !created => !v.is_ancestor(&upd.old, &pull.pr.head_sha).await,
                        _ => false,
                    };
                    behind.then(|| "Head branch is out of date. Update the branch and try again.".into())
                } else {
                    None
                }
            }
        }
        "required_deployments" => {
            let envs = str_list(params.get("required_deployment_environments"));
            if deleted || envs.is_empty() {
                None
            } else {
                let missing = if matches!(sub.via, Via::Commit | Via::Upstream) { envs.clone() } else { missing_deployments(sub.s, sub.ctx.repo.id, &check_sha, &envs).await? };
                (!missing.is_empty()).then(|| format!("Changes must be successfully deployed to {} before they can be merged.", missing.join(", ")))
            }
        }
        "commit_message_pattern" | "commit_author_email_pattern" | "committer_email_pattern" => {
            if deleted {
                None
            } else {
                let (subject, pick): (&str, fn(&CommitInfo) -> String) = match ty {
                    "commit_message_pattern" => ("Commit message", |c| c.message.clone()),
                    "commit_author_email_pattern" => ("Commit author email", |c| c.author.email.clone()),
                    _ => ("Committer email", |c| c.committer.email.clone()),
                };
                let bad: Vec<String> = f.commits().await.iter().filter(|c| !pattern_ok(&params, &pick(c))).map(|c| c.sha.clone()).collect();
                (!bad.is_empty()).then(|| format!("{}{}", pattern_message(subject, &params), found(bad.len(), &bad)))
            }
        }
        "branch_name_pattern" | "tag_name_pattern" => {
            let short = upd.refname.strip_prefix("refs/heads/").or_else(|| upd.refname.strip_prefix("refs/tags/")).unwrap_or(&upd.refname);
            let subject = if ty == "branch_name_pattern" { "Branch name" } else { "Tag name" };
            (created && !pattern_ok(&params, short)).then(|| pattern_message(subject, &params))
        }
        "file_path_restriction" => {
            let pats = str_list(params.get("restricted_file_paths"));
            if deleted || pats.is_empty() {
                None
            } else {
                let bad: Vec<String> = f.files().await.into_iter().map(|(p, _)| p).filter(|p| pats.iter().any(|x| path_matches(x, p))).collect();
                (!bad.is_empty()).then(|| format!("Cannot push to restricted file paths.{}", found(bad.len(), &bad)))
            }
        }
        "max_file_path_length" => {
            let max = params.get("max_file_path_length").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
            if deleted {
                None
            } else {
                let bad: Vec<String> = f.files().await.into_iter().filter(|(p, b)| b.is_some() && p.chars().count() > max).map(|(p, _)| p).collect();
                (!bad.is_empty()).then(|| format!("File paths must be at most {max} characters long.{}", found(bad.len(), &bad)))
            }
        }
        "file_extension_restriction" => {
            let exts: Vec<String> = str_list(params.get("restricted_file_extensions")).iter().map(|e| e.trim_start_matches('*').to_lowercase()).filter(|e| !e.is_empty()).collect();
            if deleted || exts.is_empty() {
                None
            } else {
                let bad: Vec<String> = f
                    .files()
                    .await
                    .into_iter()
                    .filter(|(p, b)| {
                        let lp = p.to_lowercase();
                        b.is_some() && exts.iter().any(|e| if e.starts_with('.') { lp.ends_with(e.as_str()) } else { lp.ends_with(&format!(".{e}")) })
                    })
                    .map(|(p, _)| p)
                    .collect();
                (!bad.is_empty()).then(|| format!("Cannot push files with restricted file extensions.{}", found(bad.len(), &bad)))
            }
        }
        "max_file_size" => {
            let mb = params.get("max_file_size").and_then(Value::as_u64).unwrap_or(100);
            if deleted {
                None
            } else {
                let sizes = f.sizes().await;
                let limit = mb * 1024 * 1024;
                let bad: Vec<String> = f
                    .files()
                    .await
                    .into_iter()
                    .filter_map(|(p, b)| b.and_then(|b| sizes.get(&b).copied()).filter(|&n| n > limit).map(|_| p))
                    .collect();
                (!bad.is_empty()).then(|| format!("Files must be smaller than {mb} MB.{}", found(bad.len(), &bad)))
            }
        }
        "workflows" => {
            let skip_create = created && params.get("do_not_enforce_on_create").and_then(Value::as_bool).unwrap_or(false);
            if deleted || skip_create || !is_branch {
                None
            } else {
                let mut missing = Vec::new();
                for w in params.get("workflows").and_then(Value::as_array).into_iter().flatten() {
                    let path = w.get("path").and_then(Value::as_str).unwrap_or("");
                    let repo_id = w.get("repository_id").and_then(Value::as_i64).unwrap_or(sub.ctx.repo.id);
                    let ok: bool = sub.via != Via::Commit
                        && sqlx::query_scalar(
                            "SELECT EXISTS(SELECT 1 FROM actions_runs WHERE repo_id = $1 AND workflow_path = $2 AND head_sha = $3 AND conclusion = 'success')",
                        )
                        .bind(repo_id)
                        .bind(path)
                        .bind(&check_sha)
                        .fetch_one(&sub.s.db)
                        .await?;
                    if !ok {
                        missing.push(path.to_string());
                    }
                }
                (!missing.is_empty()).then(|| format!("Required workflows have not succeeded: {}", missing.join(", ")))
            }
        }
        "code_scanning" => {
            if !sub.via.is_merge() {
                None
            } else {
                let mut missing = Vec::new();
                for t in params.get("code_scanning_tools").and_then(Value::as_array).into_iter().flatten() {
                    let tool = t.get("tool").and_then(Value::as_str).unwrap_or("");
                    let ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM code_scanning_analyses WHERE repo_id = $1 AND tool_name = $2 AND commit_sha = $3)")
                        .bind(sub.ctx.repo.id)
                        .bind(tool)
                        .bind(&check_sha)
                        .fetch_one(&sub.s.db)
                        .await?;
                    if !ok {
                        missing.push(tool.to_string());
                    }
                }
                (!missing.is_empty()).then(|| format!("Code scanning results are required from: {}", missing.join(", ")))
            }
        }
        // merge_queue: forge has no merge queue (merges are not routed through one);
        // copilot_code_review: no Copilot. Both always pass.
        _ => None,
    })
}

async fn pull_request_violation(s: &AppState, ctx: &RepoCtx, pull: &Pull, params: &Value, method: Option<MergeMethod>) -> ApiResult<Option<String>> {
    if let Some(m) = method {
        let allowed = str_list(params.get("allowed_merge_methods"));
        let name = match m {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        };
        if !allowed.is_empty() && !allowed.iter().any(|a| a == name) {
            return Ok(Some(format!("Merge method {name} is not allowed by a repository rule. Allowed methods: {}.", allowed.join(", "))));
        }
    }
    let need = params.get("required_approving_review_count").and_then(Value::as_i64).unwrap_or(0);
    if need > 0 {
        let dismiss = params.get("dismiss_stale_reviews_on_push").and_then(Value::as_bool).unwrap_or(false);
        let r = crate::protection::Rules { branch: pull.pr.base_ref.clone(), cfg: json!({"required_pull_request_reviews": {"dismiss_stale_reviews": dismiss}}) };
        if crate::protection::approval_count(s, ctx, pull, &r).await? < need {
            let noun = if need == 1 { "review is" } else { "reviews are" };
            return Ok(Some(format!("At least {need} approving {noun} required by reviewers with write access.")));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------------------------
// Evaluation of a set of ref updates

/// A failing rule.
#[derive(Debug, Clone)]
pub struct Violation {
    pub ruleset_id: i64,
    pub ruleset_name: String,
    pub rule_type: String,
    pub message: String,
    pub push_rule: bool,
}

/// The verdict for one ref update.
#[derive(Debug, Clone, Default)]
pub struct RefVerdict {
    pub refname: String,
    pub new: String,
    /// Active violations that block the update.
    pub blocked: Vec<Violation>,
    /// Active violations the actor bypassed.
    pub bypassed: Vec<Violation>,
    /// Push bypass request created (or reused) for the blocked push rules.
    pub bypass_request: Option<(i64, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub refs: Vec<RefVerdict>,
}

impl Outcome {
    pub fn rejected(&self) -> bool {
        self.refs.iter().any(|r| !r.blocked.is_empty())
    }

    /// GitHub's GH013 text for a push (lines for stderr; git prefixes them with `remote:`).
    pub fn push_message(&self, s: &AppState, ctx: &RepoCtx) -> String {
        let mut out = String::new();
        for r in &self.refs {
            let enc = percent_encoding::utf8_percent_encode(&r.refname, percent_encoding::NON_ALPHANUMERIC);
            if !r.blocked.is_empty() {
                out.push_str(&format!("error: GH013: Repository rule violations found for {}.\n", r.refname));
                out.push_str(&format!("Review all repository rules at {}/{}/rules?ref={enc}\n\n", s.base_url(), ctx.full_name()));
                for v in &r.blocked {
                    out.push_str(&format!("- {}\n\n", v.message));
                }
                if let Some((_, url)) = &r.bypass_request {
                    out.push_str(&format!("A push bypass request was created; once a bypass actor approves it, push again: {url}\n\n"));
                }
            } else if !r.bypassed.is_empty() {
                out.push_str(&format!("Bypassed rule violations for {}:\n\n", r.refname));
                for v in &r.bypassed {
                    out.push_str(&format!("- {}\n\n", v.message));
                }
            }
        }
        out
    }

    /// The API error for a rejected API write.
    pub fn api_error(&self, status: StatusCode) -> ApiError {
        let mut msg = String::from("Repository rule violations found\n\n");
        for r in &self.refs {
            for v in &r.blocked {
                msg.push_str(&v.message);
                msg.push_str("\n\n");
            }
        }
        if let Some((_, url)) = self.refs.iter().find_map(|r| r.bypass_request.clone()) {
            msg.push_str(&format!("A push bypass request was created: {url}\n\n"));
        }
        ApiError::new(status, msg)
    }
}

/// Evaluates every applicable ruleset against `updates`, records rule suites and push bypass
/// requests, and returns the verdict. `view` = None skips rules that need git (merge state
/// computation).
pub async fn evaluate(
    s: &AppState,
    ctx: &RepoCtx,
    actor: &Actor,
    view: Option<&GitView>,
    updates: &[RefUpdate],
    via: Via,
    pull: Option<&Pull>,
    record: bool,
) -> ApiResult<Outcome> {
    let sets = repo_rulesets(s, ctx).await?;
    let mut outcome = Outcome::default();
    if sets.is_empty() {
        return Ok(outcome);
    }
    let facts = actor_facts(s, ctx, actor).await;
    let sub = Subject { s, ctx, via, pull };
    for upd in updates {
        let applicable: Vec<&RulesetRow> = sets.iter().filter(|r| targets_ref(r, &upd.refname, &ctx.repo.default_branch)).collect();
        if applicable.is_empty() {
            continue;
        }
        let mut f = Facts { view, upd, commits: None, files: None, sizes: None };
        let mut verdict = RefVerdict { refname: upd.refname.clone(), new: upd.new.clone(), ..Default::default() };
        let mut evals = Vec::new();
        let (mut any_evaluate, mut eval_fail) = (false, false);
        let mut delegated: Vec<Violation> = Vec::new();
        for r in applicable {
            let mode = bypass_mode_with(r, ctx, actor, &facts);
            if mode == BypassMode::Exempt {
                continue;
            }
            let bypass = mode == BypassMode::Always || (mode == BypassMode::PullRequest && via.is_merge());
            let active = r.enforcement == "active";
            any_evaluate |= !active;
            let mut seen_types = HashSet::new();
            for rule in r.rules.as_array().into_iter().flatten() {
                let rule_type = rule.get("type").and_then(Value::as_str).unwrap_or("unknown").to_string();
                let failed = eval_rule(&sub, rule, &mut f).await?;
                let mut e = json!({
                    "rule_source": {"type": if r.enterprise { "Enterprise" } else if r.org_id.is_some() { "Organization" } else { "Repository" }, "id": r.id, "name": r.name},
                    "enforcement": r.enforcement,
                    "result": if failed.is_some() { "fail" } else { "pass" },
                    "rule_type": rule_type,
                    "details": failed,
                });
                if failed.is_none() {
                    e["details"] = Value::Null;
                }
                evals.push(e);
                let Some(message) = failed else { continue };
                if !active {
                    eval_fail = true;
                    continue;
                }
                let v = Violation { ruleset_id: r.id, ruleset_name: r.name.clone(), rule_type: rule_type.clone(), message, push_rule: r.target == "push" };
                if bypass {
                    verdict.bypassed.push(v);
                } else if r.target == "push" && r.bypass_actors.as_array().is_some_and(|a| !a.is_empty()) && !via.is_merge() {
                    if seen_types.insert(rule_type) {
                        delegated.push(v);
                    }
                } else {
                    verdict.blocked.push(v);
                }
            }
        }
        if !delegated.is_empty() {
            // Delegated bypass: an approved request for this commit lets the push through.
            match bypass::approved_for(s, ctx, actor, &upd.new, &delegated).await? {
                true => verdict.bypassed.extend(delegated),
                false => {
                    if record && !upd.deleted() {
                        verdict.bypass_request = Some(bypass::request_for(s, ctx, actor, upd, &delegated).await?);
                    }
                    verdict.blocked.extend(delegated);
                }
            }
        }
        if evals.is_empty() {
            continue;
        }
        let result = if !verdict.blocked.is_empty() {
            "fail"
        } else if !verdict.bypassed.is_empty() {
            "bypass"
        } else {
            "pass"
        };
        let evaluation_result = any_evaluate.then(|| if eval_fail || result == "fail" { "fail" } else { result });
        if record {
            record_suite(s, ctx, actor, upd, result, evaluation_result, evals).await?;
        }
        outcome.refs.push(verdict);
    }
    Ok(outcome)
}

async fn record_suite(s: &AppState, ctx: &RepoCtx, actor: &Actor, upd: &RefUpdate, result: &str, evaluation: Option<&str>, evals: Vec<Value>) -> ApiResult<()> {
    let org_id = (ctx.owner.kind == "Organization").then_some(ctx.owner.id);
    sqlx::query(
        "INSERT INTO rule_suites (repo_id, org_id, actor_id, actor_name, before_sha, after_sha, ref, result, evaluation_result, rule_evaluations)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(ctx.repo.id)
    .bind(org_id)
    .bind(actor.user.as_ref().map(|u| u.id))
    .bind(actor.user.as_ref().map(|u| u.login.clone()).unwrap_or_default())
    .bind(&upd.old)
    .bind(&upd.new)
    .bind(&upd.refname)
    .bind(result)
    .bind(evaluation)
    .bind(Value::Array(evals))
    .execute(&s.db)
    .await?;
    Ok(())
}

/// Enforces rulesets on an API write (call inside the git write, after creating the new
/// commits and before moving the ref). Returns the error to send when the update is blocked.
pub async fn enforce_api(
    s: &AppState,
    ctx: &RepoCtx,
    actor: &Actor,
    path: &std::path::Path,
    updates: &[RefUpdate],
    via: Via,
    pull: Option<&Pull>,
) -> ApiResult<()> {
    let view = GitView::new(s, path);
    let outcome = evaluate(s, ctx, actor, Some(&view), updates, via, pull, true).await?;
    if outcome.rejected() {
        let status = match via {
            Via::Commit => StatusCode::CONFLICT,
            Via::Merge(_) => StatusCode::METHOD_NOT_ALLOWED,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        return Err(outcome.api_error(status));
    }
    Ok(())
}

/// Why rulesets block merging `pull` right now (merge state; no bypass, nothing recorded).
pub async fn pull_merge_blocked(s: &AppState, ctx: &RepoCtx, pull: &Pull) -> ApiResult<bool> {
    let base = format!("refs/heads/{}", pull.pr.base_ref);
    let upd = RefUpdate { refname: base, old: pull.pr.base_sha.clone(), new: pull.pr.head_sha.clone() };
    let actor = Actor { user: None, perm: Perm::Write, app_id: None, deploy_key: false };
    let outcome = evaluate(s, ctx, &actor, None, &[upd], Via::Merge(None), Some(pull), false).await?;
    Ok(outcome.refs.iter().any(|r| r.blocked.iter().any(|v| matches!(v.rule_type.as_str(), "pull_request" | "required_status_checks" | "required_deployments" | "workflows" | "code_scanning"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert!(path_matches("*.env", "config/prod.env"));
        assert!(path_matches("secrets/**", "secrets/a/b.txt"));
        assert!(path_matches("secrets", "secrets/a.txt"));
        assert!(!path_matches("secrets/*", "secrets/a/b.txt"));
        assert!(path_matches("**/*.pem", "a/b/c.pem"));
        assert!(path_matches("**/*.pem", "c.pem"));
        assert!(!path_matches("docs/*.md", "src/a.md"));
    }

    #[test]
    fn patterns() {
        let p = json!({"operator": "starts_with", "pattern": "feat"});
        assert!(pattern_ok(&p, "feat: x"));
        assert!(!pattern_ok(&p, "fix: x"));
        let p = json!({"operator": "regex", "pattern": "^[A-Z]+-\\d+", "negate": true});
        assert!(!pattern_ok(&p, "ABC-12 do"));
        assert!(pattern_ok(&p, "nothing"));
        assert_eq!(pattern_message("Commit message", &json!({"operator": "contains", "pattern": "x"})), "Commit message must contain a given pattern: x");
    }
}
