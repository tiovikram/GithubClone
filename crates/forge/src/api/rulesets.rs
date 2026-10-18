//! Repository and organization rulesets:
//! /repos/{o}/{r}/rulesets[/{id}], /orgs/{org}/rulesets[/{id}] and
//! /repos/{o}/{r}/rules/branches/{branch} (the active rules that apply to a branch).
//! Enforcement (pushes and API writes) lives in `crate::rules`.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{owner}/{repo}/rulesets", get(list_repo).post(create_repo))
        .route("/repos/{owner}/{repo}/rulesets/{id}", get(get_repo).put(update_repo).delete(delete_repo))
        .route("/repos/{owner}/{repo}/rules/branches/{*branch}", get(branch_rules))
        .route("/orgs/{org}/rulesets", get(list_org).post(create_org))
        .route("/orgs/{org}/rulesets/{id}", get(get_org).put(update_org).delete(delete_org))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RulesetRow {
    pub id: i64,
    pub repo_id: Option<i64>,
    pub org_id: Option<i64>,
    pub name: String,
    pub target: String,
    pub enforcement: String,
    pub conditions: Option<Value>,
    pub rules: Value,
    pub bypass_actors: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Where a ruleset lives: a repository (`owner/name`) or an organization (`login`).
#[derive(Debug, Clone)]
pub enum Level {
    Repo(RepoCtx),
    Org(User),
}

impl Level {
    fn source(&self) -> (&'static str, String) {
        match self {
            Level::Repo(c) => ("Repository", c.full_name()),
            Level::Org(o) => ("Organization", o.login.clone()),
        }
    }
}

/// The source of a ruleset row as seen from `level` (a repo's list may include org rulesets).
pub fn source_of(row: &RulesetRow, level: &Level) -> (&'static str, String) {
    match (level, row.org_id) {
        (Level::Repo(c), Some(_)) => ("Organization", c.owner.login.clone()),
        _ => level.source(),
    }
}

pub fn ruleset_json(s: &AppState, row: &RulesetRow, level: &Level, full: bool) -> Value {
    let (source_type, source) = source_of(row, level);
    let (self_href, html) = match source_type {
        "Repository" => (format!("{}/repos/{source}/rulesets/{}", s.api_url(), row.id), format!("{}/{source}/rules/{}", s.base_url(), row.id)),
        _ => (
            format!("{}/orgs/{source}/rulesets/{}", s.api_url(), row.id),
            format!("{}/organizations/{source}/settings/rules/{}", s.base_url(), row.id),
        ),
    };
    let mut v = json!({
        "id": row.id,
        "name": row.name,
        "target": row.target,
        "source_type": source_type,
        "source": source,
        "enforcement": row.enforcement,
        "node_id": crate::ids::encode("RepositoryRuleset", row.id),
        "_links": { "self": { "href": self_href }, "html": { "href": html } },
        "created_at": crate::json::ts(row.created_at),
        "updated_at": crate::json::ts(row.updated_at),
    });
    if full {
        let o = v.as_object_mut().expect("object");
        o.insert("conditions".into(), row.conditions.clone().unwrap_or(Value::Null));
        o.insert("rules".into(), row.rules.clone());
        o.insert("bypass_actors".into(), row.bypass_actors.clone());
        if matches!(level, Level::Repo(_)) {
            o.insert("current_user_can_bypass".into(), json!("never"));
        }
    }
    v
}

#[derive(Debug, Default, Deserialize)]
pub struct RulesetInput {
    pub name: Option<String>,
    pub target: Option<String>,
    pub enforcement: Option<String>,
    pub conditions: Option<Value>,
    pub rules: Option<Value>,
    pub bypass_actors: Option<Value>,
}

fn invalid(field: &str, message: &str) -> ApiError {
    let mut e = ApiError::validation("Ruleset", field, "invalid");
    e.message = format!("Invalid request.\n\n{message}");
    e.errors = Some(json!([{ "resource": "Ruleset", "code": "invalid", "field": field, "message": message }]));
    e
}

fn validate(input: &RulesetInput, creating: bool) -> ApiResult<()> {
    if creating && input.name.as_deref().is_none_or(|n| n.trim().is_empty()) {
        return Err(ApiError::validation("Ruleset", "name", "missing_field"));
    }
    if creating && input.enforcement.is_none() {
        return Err(ApiError::validation("Ruleset", "enforcement", "missing_field"));
    }
    if let Some(e) = &input.enforcement {
        if !["disabled", "active", "evaluate"].contains(&e.as_str()) {
            return Err(invalid("enforcement", "enforcement must be one of disabled, active, evaluate"));
        }
    }
    if let Some(t) = &input.target {
        if !["branch", "tag", "push"].contains(&t.as_str()) {
            return Err(invalid("target", "target must be one of branch, tag, push"));
        }
    }
    if let Some(r) = &input.rules {
        let ok = r.as_array().is_some_and(|a| a.iter().all(|x| x.get("type").and_then(Value::as_str).is_some()));
        if !ok {
            return Err(invalid("rules", "rules must be an array of objects with a type"));
        }
    }
    if let Some(c) = &input.conditions {
        if !c.is_object() && !c.is_null() {
            return Err(invalid("conditions", "conditions must be an object"));
        }
    }
    if let Some(b) = &input.bypass_actors {
        if !b.is_array() {
            return Err(invalid("bypass_actors", "bypass_actors must be an array"));
        }
    }
    Ok(())
}

/// Repo admin, or org admin for org rulesets.
async fn require_admin(s: &AppState, level: &Level, actor: &User) -> ApiResult<()> {
    match level {
        Level::Repo(c) => c.require(Perm::Admin),
        Level::Org(o) => {
            let admin: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2 AND role = 'admin')")
                .bind(o.id)
                .bind(actor.id)
                .fetch_one(&s.db)
                .await?;
            if admin {
                Ok(())
            } else {
                Err(ApiError::not_found())
            }
        }
    }
}

fn level_ids(level: &Level) -> (Option<i64>, Option<i64>) {
    match level {
        Level::Repo(c) => (Some(c.repo.id), None),
        Level::Org(o) => (None, Some(o.id)),
    }
}

pub async fn create_ruleset(s: &AppState, level: &Level, actor: &User, input: RulesetInput) -> ApiResult<RulesetRow> {
    require_admin(s, level, actor).await?;
    validate(&input, true)?;
    let (repo_id, org_id) = level_ids(level);
    let row: RulesetRow = sqlx::query_as(
        "INSERT INTO rulesets (repo_id, org_id, name, target, enforcement, conditions, rules, bypass_actors)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *",
    )
    .bind(repo_id)
    .bind(org_id)
    .bind(input.name.unwrap_or_default().trim())
    .bind(input.target.as_deref().unwrap_or("branch"))
    .bind(input.enforcement.as_deref().unwrap_or("disabled"))
    .bind(input.conditions.filter(|c| !c.is_null()))
    .bind(input.rules.unwrap_or_else(|| json!([])))
    .bind(input.bypass_actors.unwrap_or_else(|| json!([])))
    .fetch_one(&s.db)
    .await?;
    // ruleset history (orgs area)
    super::orgs_settings::record_ruleset_version(&s.db, row.id, actor, ruleset_json(s, &row, level, true)).await;
    Ok(row)
}

/// Rulesets visible at `level`. For a repository, `includes_parents` adds the owning
/// organization's rulesets.
pub async fn list_rulesets(s: &AppState, level: &Level, includes_parents: bool, targets: Option<&[String]>) -> ApiResult<Vec<RulesetRow>> {
    let rows: Vec<RulesetRow> = match level {
        Level::Repo(c) => {
            let org = (includes_parents && c.owner.kind == "Organization").then_some(c.owner.id);
            sqlx::query_as("SELECT * FROM rulesets WHERE repo_id = $1 OR ($2::bigint IS NOT NULL AND org_id = $2) ORDER BY id")
                .bind(c.repo.id)
                .bind(org)
                .fetch_all(&s.db)
                .await?
        }
        Level::Org(o) => sqlx::query_as("SELECT * FROM rulesets WHERE org_id = $1 ORDER BY id").bind(o.id).fetch_all(&s.db).await?,
    };
    Ok(rows.into_iter().filter(|r| targets.is_none_or(|t| t.iter().any(|x| x == &r.target))).collect())
}

pub async fn get_ruleset(s: &AppState, level: &Level, id: i64) -> ApiResult<RulesetRow> {
    list_rulesets(s, level, true, None).await?.into_iter().find(|r| r.id == id).ok_or_else(ApiError::not_found)
}

/// Rows owned directly by `level` (not inherited) can be changed there.
async fn own_ruleset(s: &AppState, level: &Level, id: i64) -> ApiResult<RulesetRow> {
    let r = get_ruleset(s, level, id).await?;
    if level_ids(level) != (r.repo_id, r.org_id) {
        return Err(ApiError::not_found());
    }
    Ok(r)
}

pub async fn update_ruleset(s: &AppState, level: &Level, actor: &User, id: i64, input: RulesetInput) -> ApiResult<RulesetRow> {
    require_admin(s, level, actor).await?;
    validate(&input, false)?;
    let cur = own_ruleset(s, level, id).await?;
    let row: RulesetRow = sqlx::query_as(
        "UPDATE rulesets SET name = $2, target = $3, enforcement = $4, conditions = $5, rules = $6, bypass_actors = $7,
                updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(input.name.filter(|n| !n.trim().is_empty()).unwrap_or(cur.name))
    .bind(input.target.unwrap_or(cur.target))
    .bind(input.enforcement.unwrap_or(cur.enforcement))
    .bind(if input.conditions.is_some() { input.conditions.filter(|c| !c.is_null()) } else { cur.conditions })
    .bind(input.rules.unwrap_or(cur.rules))
    .bind(input.bypass_actors.unwrap_or(cur.bypass_actors))
    .fetch_one(&s.db)
    .await?;
    super::orgs_settings::record_ruleset_version(&s.db, row.id, actor, ruleset_json(s, &row, level, true)).await;
    Ok(row)
}

pub async fn delete_ruleset(s: &AppState, level: &Level, actor: &User, id: i64) -> ApiResult<()> {
    require_admin(s, level, actor).await?;
    own_ruleset(s, level, id).await?;
    sqlx::query("DELETE FROM rulesets WHERE id = $1").bind(id).execute(&s.db).await?;
    Ok(())
}

/// Glob match for ruleset ref patterns: `*` matches within one path segment, `**` anything.
fn glob(pat: &[u8], s: &[u8]) -> bool {
    match pat.first() {
        None => s.is_empty(),
        Some(b'*') if pat.get(1) == Some(&b'*') => (0..=s.len()).any(|i| glob(&pat[2..], &s[i..])),
        Some(b'*') => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| glob(&pat[1..], &s[i..])),
        Some(c) => s.first() == Some(c) && glob(&pat[1..], &s[1..]),
    }
}

fn pattern_matches(pattern: &str, full_ref: &str, default_branch: &str) -> bool {
    match pattern {
        "~ALL" => true,
        "~DEFAULT_BRANCH" => full_ref == format!("refs/heads/{default_branch}"),
        p if p.starts_with("refs/") => glob(p.as_bytes(), full_ref.as_bytes()),
        p => {
            let short = full_ref.strip_prefix("refs/heads/").or_else(|| full_ref.strip_prefix("refs/tags/")).unwrap_or(full_ref);
            glob(p.as_bytes(), short.as_bytes())
        }
    }
}

/// Whether a ruleset's `ref_name` condition selects `full_ref`. No condition selects nothing,
/// like GitHub (a branch ruleset without targets applies to no branch).
pub fn applies_to(row: &RulesetRow, full_ref: &str, default_branch: &str) -> bool {
    let Some(rn) = row.conditions.as_ref().and_then(|c| c.get("ref_name")) else { return false };
    let list = |k: &str| -> Vec<String> {
        rn.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    list("include").iter().any(|p| pattern_matches(p, full_ref, default_branch)) && !list("exclude").iter().any(|p| pattern_matches(p, full_ref, default_branch))
}

/// The rules of every active branch ruleset (repo and parent org) that applies to `branch`,
/// each annotated with its ruleset (`repository-rule-detailed`).
pub async fn rules_for_branch(s: &AppState, ctx: &RepoCtx, branch: &str) -> ApiResult<Vec<Value>> {
    let level = Level::Repo(ctx.clone());
    let full_ref = format!("refs/heads/{branch}");
    let mut out = Vec::new();
    for r in crate::rules::repo_rulesets(s, ctx).await? {
        if r.enforcement != "active" || r.target != "branch" || !applies_to(&r, &full_ref, &ctx.repo.default_branch) {
            continue;
        }
        let (source_type, source) = source_of(&r, &level);
        for rule in r.rules.as_array().into_iter().flatten() {
            let mut rule = rule.clone();
            if let Some(o) = rule.as_object_mut() {
                o.insert("ruleset_source_type".into(), json!(source_type));
                o.insert("ruleset_source".into(), json!(source));
                o.insert("ruleset_id".into(), json!(r.id));
            }
            out.push(rule);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Handlers

async fn repo_level(s: &AppState, u: Option<&User>, owner: &str, repo: &str) -> ApiResult<Level> {
    Ok(Level::Repo(db::load_repo(&s.db, u, owner, repo).await?))
}

async fn org_level(s: &AppState, org: &str) -> ApiResult<Level> {
    let o = db::user_by_login(&s.db, org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    Ok(Level::Org(o))
}

#[derive(Deserialize, Default)]
struct ListQuery {
    includes_parents: Option<bool>,
    targets: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

fn render_list(s: &AppState, uri: &OriginalUri, level: &Level, rows: Vec<RulesetRow>, q: &ListQuery) -> Paged {
    let p = PageParams { page: q.page, per_page: q.per_page };
    let total = rows.len() as i64;
    let items = rows.iter().skip(p.offset() as usize).take(p.limit() as usize).map(|r| ruleset_json(s, r, level, false)).collect();
    Paged::new(uri, s, p, Some(total), items)
}

fn targets(q: &ListQuery) -> Option<Vec<String>> {
    q.targets.as_ref().map(|t| t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
}

async fn list_repo(State(s): State<AppState>, uri: OriginalUri, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let level = repo_level(&s, u.as_ref(), &owner, &repo).await?;
    let rows = list_rulesets(&s, &level, q.includes_parents.unwrap_or(true), targets(&q).as_deref()).await?;
    Ok(render_list(&s, &uri, &level, rows, &q))
}

async fn create_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>, Json(b): Json<RulesetInput>) -> ApiResult<Response> {
    let level = repo_level(&s, Some(&u), &owner, &repo).await?;
    let row = create_ruleset(&s, &level, &u, b).await?;
    Ok(created(full_json(&s, &row, &level, Some(&u)).await))
}

async fn get_repo(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Json<Value>> {
    let level = repo_level(&s, u.as_ref(), &owner, &repo).await?;
    let row = get_ruleset(&s, &level, id).await?;
    Ok(Json(full_json(&s, &row, &level, u.as_ref()).await))
}

/// A ruleset with `current_user_can_bypass` computed for the viewer (repository level).
async fn full_json(s: &AppState, row: &RulesetRow, level: &Level, viewer: Option<&User>) -> Value {
    let mut v = ruleset_json(s, row, level, true);
    if let Level::Repo(ctx) = level {
        v["current_user_can_bypass"] = json!(crate::rules::current_user_can_bypass(s, ctx, viewer, row).await);
    }
    v
}

async fn update_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>, Json(b): Json<RulesetInput>) -> ApiResult<Json<Value>> {
    let level = repo_level(&s, Some(&u), &owner, &repo).await?;
    let row = update_ruleset(&s, &level, &u, id, b).await?;
    Ok(Json(full_json(&s, &row, &level, Some(&u)).await))
}

async fn delete_repo(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, id)): Path<(String, String, i64)>) -> ApiResult<Response> {
    let level = repo_level(&s, Some(&u), &owner, &repo).await?;
    delete_ruleset(&s, &level, &u, id).await?;
    Ok(no_content())
}

async fn branch_rules(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo, branch)): Path<(String, String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let rules = rules_for_branch(&s, &ctx, &branch).await?;
    let total = rules.len() as i64;
    let items = rules.into_iter().skip(p.offset() as usize).take(p.limit() as usize).collect();
    Ok(Paged::new(&uri, &s, p, Some(total), items))
}

async fn list_org(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Path(org): Path<String>, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let level = org_level(&s, &org).await?;
    require_admin(&s, &level, &u).await?;
    let rows = list_rulesets(&s, &level, true, targets(&q).as_deref()).await?;
    Ok(render_list(&s, &uri, &level, rows, &q))
}

async fn create_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path(org): Path<String>, Json(b): Json<RulesetInput>) -> ApiResult<Response> {
    let level = org_level(&s, &org).await?;
    let row = create_ruleset(&s, &level, &u, b).await?;
    Ok(created(ruleset_json(&s, &row, &level, true)))
}

async fn get_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Json<Value>> {
    let level = org_level(&s, &org).await?;
    require_admin(&s, &level, &u).await?;
    let row = get_ruleset(&s, &level, id).await?;
    Ok(Json(ruleset_json(&s, &row, &level, true)))
}

async fn update_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>, Json(b): Json<RulesetInput>) -> ApiResult<Json<Value>> {
    let level = org_level(&s, &org).await?;
    let row = update_ruleset(&s, &level, &u, id, b).await?;
    Ok(Json(ruleset_json(&s, &row, &level, true)))
}

async fn delete_org(State(s): State<AppState>, AuthUser(u): AuthUser, Path((org, id)): Path<(String, i64)>) -> ApiResult<Response> {
    let level = org_level(&s, &org).await?;
    delete_ruleset(&s, &level, &u, id).await?;
    Ok(no_content())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(pattern_matches("~DEFAULT_BRANCH", "refs/heads/main", "main"));
        assert!(!pattern_matches("~DEFAULT_BRANCH", "refs/heads/dev", "main"));
        assert!(pattern_matches("~ALL", "refs/heads/x", "main"));
        assert!(pattern_matches("refs/heads/release/*", "refs/heads/release/1.0", "main"));
        assert!(!pattern_matches("refs/heads/release/*", "refs/heads/release/1/2", "main"));
        assert!(pattern_matches("refs/heads/release/**", "refs/heads/release/1/2", "main"));
        assert!(pattern_matches("feat*", "refs/heads/feature", "main"));
    }
}
