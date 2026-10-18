//! Branch protection sub-resources (`required_status_checks/contexts`, `restrictions`
//! users/teams/apps), ruleset history (`/rulesets/{id}/history`), rule suites (ruleset
//! evaluations of pushes, recorded by [`on_refs_changed`]) and push-rule bypass requests.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::repo_extras::{load_protection, load_users, prot_ctx, save_protection, users_list};
use super::rulesets::{self, Level, RulesetRow};
use super::{no_content, PageParams, Paged};
use crate::auth::AuthUser;
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;
use crate::webhooks::ZERO_SHA;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/rulesets/rule-suites", get(repo_suites))
        .route("/repos/{owner}/{repo}/rulesets/rule-suites/{id}", get(repo_suite))
        .route("/repos/{owner}/{repo}/rulesets/{id}/history", get(history))
        .route("/repos/{owner}/{repo}/rulesets/{id}/history/{version}", get(version))
        .route("/orgs/{org}/rulesets/rule-suites", get(org_suites))
        .route("/orgs/{org}/rulesets/rule-suites/{id}", get(org_suite))
        .route("/orgs/{org}/rulesets/{id}/history", get(org_history))
        .route("/orgs/{org}/rulesets/{id}/history/{version}", get(org_version))
        .route("/repos/{owner}/{repo}/bypass-requests/push-rules", get(list_bypass))
        .route("/repos/{owner}/{repo}/bypass-requests/push-rules/{number}", get(get_bypass))
}

/// Branch-protection sub-routes, mounted behind `ref_dispatch` (branch names may contain `/`).
pub fn protection_routes() -> Router<AppState> {
    let p = "/repos/{owner}/{repo}/branches/{branch}/protection";
    Router::new()
        .route(
            &format!("{p}/required_status_checks/contexts"),
            get(get_contexts).post(add_contexts).put(set_contexts).delete(remove_contexts),
        )
        .route(&format!("{p}/restrictions"), get(get_restrictions).delete(delete_restrictions))
        .route(&format!("{p}/restrictions/users"), get(get_r_users).post(add_r_users).put(set_r_users).delete(rm_r_users))
        .route(&format!("{p}/restrictions/teams"), get(get_r_teams).post(add_r_teams).put(set_r_teams).delete(rm_r_teams))
        .route(&format!("{p}/restrictions/apps"), get(get_r_apps).post(add_r_apps).put(set_r_apps).delete(rm_r_apps))
}

// ---------------------------------------------------------------------------------------------
// Status check contexts

/// `{contexts: [...]}`, `[...]` or a bare string (all accepted by GitHub).
fn string_list(body: &Value, key: &str) -> ApiResult<Vec<String>> {
    let arr = match body {
        Value::Array(a) => a.clone(),
        Value::Object(o) => match o.get(key) {
            Some(Value::Array(a)) => a.clone(),
            _ => return Err(ApiError::validation("Branch", key, "missing_field")),
        },
        Value::String(s) => vec![Value::String(s.clone())],
        _ => return Err(ApiError::validation("Branch", key, "invalid")),
    };
    arr.into_iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| ApiError::validation("Branch", key, "invalid"))).collect()
}

fn current_contexts(sc: &Value) -> Vec<String> {
    let mut out: Vec<String> = sc.get("contexts").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
    for c in sc.get("checks").and_then(Value::as_array).into_iter().flatten() {
        if let Some(x) = c.get("context").and_then(Value::as_str) {
            if !out.iter().any(|o| o == x) {
                out.push(x.to_string());
            }
        }
    }
    out
}

pub enum Op {
    Get,
    Add(Vec<String>),
    Set(Vec<String>),
    Remove(Vec<String>),
}

fn apply(cur: Vec<String>, op: Op) -> Vec<String> {
    match op {
        Op::Get => cur,
        Op::Add(add) => {
            let mut v = cur;
            for a in add {
                if !v.contains(&a) {
                    v.push(a);
                }
            }
            v
        }
        Op::Set(set) => {
            let mut v: Vec<String> = Vec::new();
            for a in set {
                if !v.contains(&a) {
                    v.push(a);
                }
            }
            v
        }
        Op::Remove(rm) => cur.into_iter().filter(|c| !rm.contains(c)).collect(),
    }
}

/// Reads or changes the required status check contexts of a protected branch.
pub async fn status_contexts(s: &AppState, u: &User, (o, r, b): (&str, &str, &str), op: Op) -> ApiResult<Vec<String>> {
    let ctx = prot_ctx(s, u, o, r, b).await?;
    let mut cfg = load_protection(s, &ctx, b).await?;
    let Some(sc) = cfg.get("required_status_checks").filter(|v| v.is_object()).cloned() else {
        return Err(ApiError::not_found());
    };
    let changing = !matches!(op, Op::Get);
    let next = apply(current_contexts(&sc), op);
    if changing {
        let old_checks: Vec<Value> = sc.get("checks").and_then(Value::as_array).cloned().unwrap_or_default();
        let checks: Vec<Value> = next
            .iter()
            .map(|c| old_checks.iter().find(|x| x.get("context").and_then(Value::as_str) == Some(c)).cloned().unwrap_or_else(|| json!({"context": c, "app_id": null})))
            .collect();
        let mut sc = sc;
        sc["contexts"] = json!(next);
        sc["checks"] = Value::Array(checks);
        cfg["required_status_checks"] = sc;
        save_protection(s, &ctx, b, &cfg).await?;
    }
    Ok(next)
}

type BranchPath = Path<(String, String, String)>;

async fn get_contexts(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath) -> ApiResult<Json<Value>> {
    Ok(Json(json!(status_contexts(&s, &u, (&o, &r, &b), Op::Get).await?)))
}
async fn add_contexts(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let l = string_list(&body, "contexts")?;
    Ok(Json(json!(status_contexts(&s, &u, (&o, &r, &b), Op::Add(l)).await?)))
}
async fn set_contexts(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let l = string_list(&body, "contexts")?;
    Ok(Json(json!(status_contexts(&s, &u, (&o, &r, &b), Op::Set(l)).await?)))
}
async fn remove_contexts(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    let l = string_list(&body, "contexts")?;
    Ok(Json(json!(status_contexts(&s, &u, (&o, &r, &b), Op::Remove(l)).await?)))
}

// ---------------------------------------------------------------------------------------------
// Push restrictions

async fn load_teams(s: &AppState, ctx: &RepoCtx, slugs: Option<&Value>) -> ApiResult<Vec<super::orgs_extra::TeamRow>> {
    let names: Vec<String> = slugs.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|x| x.to_lowercase()).collect()).unwrap_or_default();
    if names.is_empty() {
        return Ok(vec![]);
    }
    Ok(sqlx::query_as("SELECT * FROM teams WHERE org_id = $1 AND lower(slug) = ANY($2) ORDER BY id").bind(ctx.owner.id).bind(&names).fetch_all(&s.db).await?)
}

/// `branch-restriction-policy` for a stored `restrictions` object.
pub async fn restrictions_json(s: &AppState, ctx: &RepoCtx, prot_url: &str, r: &Value) -> ApiResult<Value> {
    let users = load_users(s, r.get("users")).await?;
    let teams = load_teams(s, ctx, r.get("teams")).await?;
    let rurl = format!("{prot_url}/restrictions");
    Ok(json!({
        "url": rurl, "users_url": format!("{rurl}/users"), "teams_url": format!("{rurl}/teams"), "apps_url": format!("{rurl}/apps"),
        "users": users_list(s, &users),
        "teams": teams.iter().map(|t| super::orgs_extra::team_json(s, &ctx.owner, t, None)).collect::<Vec<_>>(),
        "apps": [],
    }))
}

fn prot_url(s: &AppState, ctx: &RepoCtx, b: &str) -> String {
    format!("{}/repos/{}/branches/{}/protection", s.api_url(), ctx.full_name(), crate::json::release::seg(b))
}

async fn restrictions_cfg(s: &AppState, u: &User, (o, r, b): (&str, &str, &str)) -> ApiResult<(RepoCtx, Value, Value)> {
    let ctx = prot_ctx(s, u, o, r, b).await?;
    let cfg = load_protection(s, &ctx, b).await?;
    let Some(rs) = cfg.get("restrictions").filter(|v| v.is_object()).cloned() else {
        return Err(ApiError::not_found());
    };
    Ok((ctx, cfg, rs))
}

async fn get_restrictions(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath) -> ApiResult<Json<Value>> {
    let (ctx, _, rs) = restrictions_cfg(&s, &u, (&o, &r, &b)).await?;
    Ok(Json(restrictions_json(&s, &ctx, &prot_url(&s, &ctx, &b), &rs).await?))
}

async fn delete_restrictions(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath) -> ApiResult<Response> {
    let (ctx, mut cfg, _) = restrictions_cfg(&s, &u, (&o, &r, &b)).await?;
    cfg.as_object_mut().map(|m| m.remove("restrictions"));
    save_protection(&s, &ctx, &b, &cfg).await?;
    Ok(no_content())
}

#[derive(Clone, Copy)]
enum Kind {
    Users,
    Teams,
    Apps,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Users => "users",
            Kind::Teams => "teams",
            Kind::Apps => "apps",
        }
    }
}

/// Validates that every name exists (users/teams with push access; apps installed), returning
/// canonical names. GitHub answers 422 for unknown entries.
async fn validate_names(s: &AppState, ctx: &RepoCtx, kind: Kind, names: &[String]) -> ApiResult<Vec<String>> {
    let mut out = Vec::new();
    for n in names {
        let ok = match kind {
            Kind::Users => match db::user_by_login(&s.db, n).await {
                Ok(user) if db::permission(&s.db, Some(&user), &ctx.repo, &ctx.owner).await? >= Perm::Write => Some(user.login),
                _ => None,
            },
            Kind::Teams => load_teams(s, ctx, Some(&json!([n]))).await?.into_iter().next().map(|t| t.slug),
            // forge has no GitHub Apps installed on repositories (apps area).
            Kind::Apps => None,
        };
        match ok {
            Some(c) => {
                if !out.contains(&c) {
                    out.push(c)
                }
            }
            None => {
                let mut e = ApiError::unprocessable("Validation Failed");
                e.errors = Some(json!([{"resource": "Branch", "field": kind.key(), "code": "invalid", "value": n,
                    "message": format!("{n} must exist and have push access to the repository")}]));
                return Err(e);
            }
        }
    }
    Ok(out)
}

async fn restriction_list(s: &AppState, u: &User, (o, r, b): (&str, &str, &str), kind: Kind, op: Op) -> ApiResult<Value> {
    let (ctx, mut cfg, mut rs) = restrictions_cfg(s, u, (o, r, b)).await?;
    let changing = !matches!(op, Op::Get);
    if changing && ctx.owner.kind != "Organization" {
        let mut e = ApiError::unprocessable("Validation Failed");
        e.errors = Some(json!([{"resource": "Branch", "field": kind.key(), "code": "invalid",
            "message": "Only organization repositories can have users and team restrictions"}]));
        return Err(e);
    }
    let op = match op {
        Op::Add(l) => Op::Add(validate_names(s, &ctx, kind, &l).await?),
        Op::Set(l) => Op::Set(validate_names(s, &ctx, kind, &l).await?),
        other => other,
    };
    let cur: Vec<String> = rs.get(kind.key()).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
    let next = apply(cur, op);
    if changing {
        rs[kind.key()] = json!(next);
        cfg["restrictions"] = rs.clone();
        save_protection(s, &ctx, b, &cfg).await?;
    }
    Ok(match kind {
        Kind::Users => json!(users_list(s, &load_users(s, Some(&json!(next))).await?)),
        Kind::Teams => json!(load_teams(s, &ctx, Some(&json!(next))).await?.iter().map(|t| super::orgs_extra::team_json(s, &ctx.owner, t, None)).collect::<Vec<_>>()),
        Kind::Apps => json!([]),
    })
}

macro_rules! restriction_handlers {
    ($kind:expr, $get:ident, $add:ident, $set:ident, $rm:ident) => {
        async fn $get(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath) -> ApiResult<Json<Value>> {
            Ok(Json(restriction_list(&s, &u, (&o, &r, &b), $kind, Op::Get).await?))
        }
        async fn $add(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
            let l = string_list(&body, $kind.key())?;
            Ok(Json(restriction_list(&s, &u, (&o, &r, &b), $kind, Op::Add(l)).await?))
        }
        async fn $set(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
            let l = string_list(&body, $kind.key())?;
            Ok(Json(restriction_list(&s, &u, (&o, &r, &b), $kind, Op::Set(l)).await?))
        }
        async fn $rm(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): BranchPath, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
            let l = string_list(&body, $kind.key())?;
            Ok(Json(restriction_list(&s, &u, (&o, &r, &b), $kind, Op::Remove(l)).await?))
        }
    };
}

restriction_handlers!(Kind::Users, get_r_users, add_r_users, set_r_users, rm_r_users);
restriction_handlers!(Kind::Teams, get_r_teams, add_r_teams, set_r_teams, rm_r_teams);
restriction_handlers!(Kind::Apps, get_r_apps, add_r_apps, set_r_apps, rm_r_apps);

/// Whether a push restriction on `branch` blocks `user` (listed users, members of listed teams
/// and, unless `enforce_admins`, admins may push). For enforcement by git/API writes.
pub async fn push_restricted(s: &AppState, ctx: &RepoCtx, branch: &str, user: &User) -> ApiResult<bool> {
    let Some(cfg) = super::repo_extras::branch_protection(&s.db, ctx.repo.id, branch).await? else { return Ok(false) };
    let Some(rs) = cfg.get("restrictions").filter(|v| v.is_object()) else { return Ok(false) };
    let enforce_admins = cfg.get("enforce_admins").and_then(Value::as_bool).unwrap_or(false);
    if ctx.perm >= Perm::Admin && !enforce_admins {
        return Ok(false);
    }
    let users: Vec<String> = rs.get("users").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_lowercase)).collect();
    if users.contains(&user.login.to_lowercase()) {
        return Ok(false);
    }
    let teams: Vec<String> = rs.get("teams").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(str::to_lowercase)).collect();
    if !teams.is_empty() {
        let member: Option<i64> = sqlx::query_scalar(
            "SELECT 1::bigint FROM team_members m JOIN teams t ON t.id = m.team_id WHERE t.org_id = $1 AND lower(t.slug) = ANY($2) AND m.user_id = $3 LIMIT 1",
        )
        .bind(ctx.owner.id)
        .bind(&teams)
        .bind(user.id)
        .fetch_optional(&s.db)
        .await?;
        if member.is_some() {
            return Ok(false);
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Ruleset history

/// Records a new version of a ruleset (called by `api::rulesets` on create/update).
pub async fn record_version(s: &AppState, row: &RulesetRow, level: &Level, actor: &User) -> ApiResult<()> {
    let state = rulesets::ruleset_json(s, row, level, true);
    sqlx::query(
        "INSERT INTO ruleset_versions (ruleset_id, version_id, actor_id, actor_type, state)
         VALUES ($1, COALESCE((SELECT max(version_id) FROM ruleset_versions WHERE ruleset_id = $1), 0) + 1, $2, 'User', $3)",
    )
    .bind(row.id)
    .bind(actor.id)
    .bind(&state)
    .execute(&s.db)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct VersionRow {
    version_id: i64,
    actor_id: Option<i64>,
    actor_type: String,
    state: Value,
    updated_at: DateTime<Utc>,
}

fn version_json(v: &VersionRow, with_state: bool) -> Value {
    let mut out = json!({"version_id": v.version_id, "actor": {"id": v.actor_id, "type": v.actor_type}, "updated_at": crate::json::ts(v.updated_at)});
    if with_state {
        out["state"] = v.state.clone();
    }
    out
}

async fn versions_page(s: &AppState, uri: &OriginalUri, level: &Level, id: i64, p: PageParams) -> ApiResult<Paged> {
    let r = rulesets::get_ruleset(s, level, id).await?;
    let rows: Vec<VersionRow> = sqlx::query_as(
        "SELECT version_id, actor_id, actor_type, state, updated_at FROM ruleset_versions WHERE ruleset_id = $1 ORDER BY version_id DESC LIMIT $2 OFFSET $3",
    )
    .bind(r.id)
    .bind(p.limit())
    .bind(p.offset())
    .fetch_all(&s.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ruleset_versions WHERE ruleset_id = $1").bind(r.id).fetch_one(&s.db).await?;
    Ok(Paged::new(uri, s, p, Some(total), rows.iter().map(|v| version_json(v, false)).collect()))
}

async fn one_version(s: &AppState, level: &Level, id: i64, version: i64) -> ApiResult<Value> {
    let r = rulesets::get_ruleset(s, level, id).await?;
    let v: VersionRow = sqlx::query_as("SELECT version_id, actor_id, actor_type, state, updated_at FROM ruleset_versions WHERE ruleset_id = $1 AND version_id = $2")
        .bind(r.id)
        .bind(version)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(version_json(&v, true))
}

async fn repo_admin_level(s: &AppState, u: &User, o: &str, r: &str) -> ApiResult<Level> {
    let ctx = db::load_repo(&s.db, Some(u), o, r).await?;
    ctx.require(Perm::Admin)?;
    Ok(Level::Repo(ctx))
}

async fn org_admin_level(s: &AppState, u: &User, org: &str) -> ApiResult<Level> {
    let o = db::user_by_login(&s.db, org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    let role = super::orgs_extra::org_role(&s.db, o.id, u.id).await?;
    if role.as_deref() != Some("admin") && !u.site_admin {
        return Err(ApiError::forbidden("You must be an organization owner."));
    }
    Ok(Level::Org(o))
}

async fn history(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let level = repo_admin_level(&s, &u, &o, &r).await?;
    versions_page(&s, &uri, &level, id, p).await
}

async fn version(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id, v)): Path<(String, String, i64, i64)>) -> ApiResult<Json<Value>> {
    let level = repo_admin_level(&s, &u, &o, &r).await?;
    Ok(Json(one_version(&s, &level, id, v).await?))
}

async fn org_history(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let level = org_admin_level(&s, &u, &org).await?;
    versions_page(&s, &uri, &level, id, p).await
}

async fn org_version(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id, v)): Path<(String, i64, i64)>) -> ApiResult<Json<Value>> {
    let level = org_admin_level(&s, &u, &org).await?;
    Ok(Json(one_version(&s, &level, id, v).await?))
}

// ---------------------------------------------------------------------------------------------
// Rule suites

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SuiteRow {
    pub id: i64,
    pub repo_id: i64,
    pub org_id: Option<i64>,
    pub actor_id: Option<i64>,
    pub actor_name: String,
    pub before_sha: String,
    pub after_sha: String,
    #[sqlx(rename = "ref")]
    pub git_ref: String,
    pub result: String,
    pub evaluation_result: String,
    pub rule_evaluations: Value,
    pub pushed_at: DateTime<Utc>,
}

/// Evaluates one rule of a ruleset against a ref update. Returns (passed, details).
async fn evaluate_rule(s: &AppState, ctx: &RepoCtx, rule: &Value, refname: &str, old: &str, new: &str, pr_merge: bool) -> (bool, Option<String>) {
    let created = old == ZERO_SHA;
    let deleted = new == ZERO_SHA;
    match rule.get("type").and_then(Value::as_str).unwrap_or("") {
        "creation" => (!created, created.then(|| "Cannot create ref due to creations being restricted.".into())),
        "deletion" => (!deleted, deleted.then(|| "Cannot delete this ref.".into())),
        "update" => {
            let upd = !created && !deleted;
            (!upd, upd.then(|| "Cannot update this protected ref.".into()))
        }
        "non_fast_forward" if !created && !deleted => {
            let path = match s.git.sync(ctx.repo.id).await {
                Ok(p) => p,
                Err(_) => return (true, None),
            };
            let ff = crate::git::read::is_ancestor(&s.git.git, &path, old, new).await.unwrap_or(true);
            (ff, (!ff).then(|| "Cannot force-push to this ref.".into()))
        }
        "required_linear_history" if !created && !deleted => {
            let range = format!("{old}..{new}");
            let merges = super::plat::git_read(s, ctx.repo.id, &["rev-list", "--merges", "-n", "1", &range, "--"]).await.ok().flatten().unwrap_or_default();
            let ok = merges.is_empty();
            (ok, (!ok).then(|| "This branch must not contain merge commits.".into()))
        }
        "pull_request" if refname.starts_with("refs/heads/") && !created && !deleted => {
            (pr_merge, (!pr_merge).then(|| "Changes must be made through a pull request.".into()))
        }
        _ => (true, None),
    }
}

/// Records a rule suite for every ref update that active or evaluate-mode rulesets apply to.
/// forge does not block pushes on ruleset rules, so failing active rules are reported as
/// `bypass` (the push went through) and evaluate-mode failures as `evaluation_result: fail`.
pub async fn record_rule_suites(s: &AppState, ctx: &RepoCtx, actor: Option<&User>, changed: &[(String, String, String)]) -> anyhow::Result<()> {
    let level = Level::Repo(ctx.clone());
    let all = rulesets::list_rulesets(s, &level, true, None).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let all: Vec<RulesetRow> = all.into_iter().filter(|r| r.enforcement != "disabled").collect();
    if all.is_empty() {
        return Ok(());
    }
    for (refname, old, new) in changed {
        let target = if refname.starts_with("refs/heads/") {
            "branch"
        } else if refname.starts_with("refs/tags/") {
            "tag"
        } else {
            continue;
        };
        let applicable: Vec<&RulesetRow> =
            all.iter().filter(|r| r.target == target && rulesets::applies_to(r, refname, &ctx.repo.default_branch)).collect();
        if applicable.is_empty() {
            continue;
        }
        let pr_merge = super::repos_stats::is_pr_merge(s, ctx, refname, new).await;
        let mut evals = Vec::new();
        let (mut active_fail, mut eval_fail) = (false, false);
        for r in applicable {
            for rule in r.rules.as_array().into_iter().flatten() {
                let (ok, details) = evaluate_rule(s, ctx, rule, refname, old, new, pr_merge).await;
                if !ok {
                    if r.enforcement == "active" {
                        active_fail = true
                    } else {
                        eval_fail = true
                    }
                }
                let mut e = json!({
                    "rule_source": {"type": if r.org_id.is_some() { "organization" } else { "ruleset" }, "id": r.id, "name": r.name},
                    "enforcement": r.enforcement,
                    "result": if ok { "pass" } else { "fail" },
                    "rule_type": rule.get("type").and_then(Value::as_str).unwrap_or("unknown"),
                });
                if let Some(d) = details {
                    e["details"] = json!(d);
                }
                evals.push(e);
            }
        }
        let org_id = (ctx.owner.kind == "Organization").then_some(ctx.owner.id);
        sqlx::query(
            "INSERT INTO rule_suites (repo_id, org_id, actor_id, actor_name, before_sha, after_sha, ref, result, evaluation_result, rule_evaluations)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(ctx.repo.id)
        .bind(org_id)
        .bind(actor.map(|a| a.id))
        .bind(actor.map(|a| a.login.clone()).unwrap_or_default())
        .bind(old)
        .bind(new)
        .bind(refname)
        .bind(if active_fail { "bypass" } else { "pass" })
        .bind(if eval_fail { "fail" } else { "pass" })
        .bind(Value::Array(evals))
        .execute(&s.db)
        .await?;
    }
    Ok(())
}

fn suite_json(r: &SuiteRow, repo_name: &str, detail: bool) -> Value {
    let mut v = json!({
        "id": r.id, "actor_id": r.actor_id, "actor_name": r.actor_name,
        "before_sha": r.before_sha, "after_sha": r.after_sha, "ref": r.git_ref,
        "repository_id": r.repo_id, "repository_name": repo_name,
        "pushed_at": crate::json::ts(r.pushed_at),
        "result": r.result, "evaluation_result": r.evaluation_result,
    });
    if detail {
        v["rule_evaluations"] = r.rule_evaluations.clone();
    }
    v
}

#[derive(Deserialize)]
struct SuiteQuery {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    repository_name: Option<String>,
    time_period: Option<String>,
    actor_name: Option<String>,
    rule_suite_result: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

fn period(p: Option<&str>) -> ApiResult<&'static str> {
    Ok(match p.unwrap_or("day") {
        "hour" => "1 hour",
        "day" => "1 day",
        "week" => "7 days",
        "month" => "1 month",
        _ => return Err(ApiError::validation("RuleSuite", "time_period", "invalid")),
    })
}

async fn suites(s: &AppState, uri: &OriginalUri, repo_id: Option<i64>, org_id: Option<i64>, q: SuiteQuery) -> ApiResult<Paged> {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let interval = period(q.time_period.as_deref())?;
    let result = q.rule_suite_result.filter(|r| r != "all");
    let git_ref = q.git_ref.map(|r| if r.starts_with("refs/") { r } else { format!("refs/heads/{r}") });
    let sql = format!(
        "SELECT rs.*, o.login || '/' || r.name AS full FROM rule_suites rs JOIN repositories r ON r.id = rs.repo_id JOIN users o ON o.id = r.owner_id
          WHERE ($1::bigint IS NULL OR rs.repo_id = $1) AND ($2::bigint IS NULL OR rs.org_id = $2)
            AND ($3::text IS NULL OR rs.ref = $3) AND ($4::text IS NULL OR lower(rs.actor_name) = lower($4))
            AND ($5::text IS NULL OR rs.result = $5) AND ($6::text IS NULL OR lower(r.name) = lower($6))
            AND rs.pushed_at > now() - interval '{interval}'
          ORDER BY rs.id DESC LIMIT $7 OFFSET $8"
    );
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        suite: SuiteRow,
        full: String,
    }
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(repo_id)
        .bind(org_id)
        .bind(&git_ref)
        .bind(&q.actor_name)
        .bind(&result)
        .bind(&q.repository_name)
        .bind(p.limit())
        .bind(p.offset())
        .fetch_all(&s.db)
        .await?;
    let items = rows.iter().map(|r| suite_json(&r.suite, r.full.rsplit('/').next().unwrap_or(""), false)).collect();
    Ok(Paged::new(uri, s, p, None, items))
}

async fn one_suite(s: &AppState, repo_id: Option<i64>, org_id: Option<i64>, id: i64) -> ApiResult<Value> {
    let r: SuiteRow = sqlx::query_as("SELECT * FROM rule_suites WHERE id = $1 AND ($2::bigint IS NULL OR repo_id = $2) AND ($3::bigint IS NULL OR org_id = $3)")
        .bind(id)
        .bind(repo_id)
        .bind(org_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let repo = db::repo_by_id(&s.db, r.repo_id).await?;
    Ok(suite_json(&r, &repo.name, true))
}

async fn repo_suites(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Query(q): Query<SuiteQuery>) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Write)?;
    suites(&s, &uri, Some(ctx.repo.id), None, q).await
}

async fn repo_suite(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Write)?;
    Ok(Json(one_suite(&s, Some(ctx.repo.id), None, id).await?))
}

async fn org_suites(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<SuiteQuery>) -> ApiResult<Paged> {
    let Level::Org(o) = org_admin_level(&s, &u, &org).await? else { unreachable!() };
    suites(&s, &uri, None, Some(o.id), q).await
}

async fn org_suite(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let Level::Org(o) = org_admin_level(&s, &u, &org).await? else { unreachable!() };
    Ok(Json(one_suite(&s, None, Some(o.id), id).await?))
}

// ---------------------------------------------------------------------------------------------
// Push-rule bypass requests. Push rulesets (target `push`) are stored but not enforced, so no
// push is ever blocked and no delegated-bypass request is created: lists are empty.

async fn list_bypass(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path((o, r)): Path<(String, String)>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Write)?;
    Ok(Paged::new(&uri, &s, p, Some(0), vec![]))
}

async fn get_bypass(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, _n)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, Some(&u), &o, &r).await?;
    ctx.require(Perm::Write)?;
    Ok(ApiError::not_found().into_response())
}

