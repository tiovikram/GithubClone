//! Repository extras: template generation, branch protection (stored, not enforced),
//! contributors, license detection, activity events, notifications, invitations.
use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use object_store::path::Path as ObjPath;
#[allow(unused_imports)]
use object_store::ObjectStoreExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{created, no_content, PageParams, Paged};
use crate::activity::EventRow;
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::extract::Json;
use crate::state::AppState;

/// Branch protection routes, mounted behind `ref_dispatch` (branch names may contain `/`).
pub fn protection_routes() -> Router<AppState> {
    let prot = "/repos/{owner}/{repo}/branches/{branch}/protection";
    Router::new()
        .route(prot, get(get_protection).put(put_protection).delete(delete_protection))
        .route(&format!("{prot}/enforce_admins"), get(get_enforce_admins).post(set_enforce_admins).delete(del_enforce_admins))
        .route(&format!("{prot}/required_signatures"), get(get_signatures).post(set_signatures).delete(del_signatures))
        .route(
            &format!("{prot}/required_pull_request_reviews"),
            get(get_pr_reviews).patch(patch_pr_reviews).delete(del_pr_reviews),
        )
        .route(
            &format!("{prot}/required_status_checks"),
            get(get_status_checks).patch(patch_status_checks).delete(del_status_checks),
        )
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/repos/{template_owner}/{template_repo}/generate", post(generate))
        .route("/repos/{owner}/{repo}/license", get(repo_license))
        .route("/repos/{owner}/{repo}/events", get(repo_events))
        .route("/networks/{owner}/{repo}/events", get(repo_events))
        .route("/events", get(public_events))
        .route("/users/{username}/events", get(user_events))
        .route("/users/{username}/events/public", get(user_public_events))
        .route("/users/{username}/received_events", get(received_events))
        .route("/users/{username}/received_events/public", get(received_events))
        .route("/orgs/{org}/events", get(org_events))
        .route("/notifications", get(empty_list).put(mark_read))
        .route("/notifications/threads/{thread_id}", get(not_found).patch(not_found).delete(not_found))
        .route("/notifications/threads/{thread_id}/subscription", get(not_found).put(not_found).delete(not_found))
        .route("/repos/{owner}/{repo}/notifications", get(repo_notifications).put(repo_mark_read))
        .route("/repos/{owner}/{repo}/invitations/{invitation_id}", axum::routing::patch(not_found).delete(not_found))
        .route("/user/repository_invitations", get(empty_list))
        .route("/user/repository_invitations/{invitation_id}", axum::routing::patch(not_found).delete(not_found))
}

async fn not_found() -> ApiError {
    ApiError::not_found()
}

async fn empty_list(_u: AuthUser) -> Json<Value> {
    Json(json!([]))
}

// ---------------------------------------------------------------------------------------------
// Template generation

#[derive(Deserialize)]
pub struct GenerateReq {
    pub owner: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub include_all_branches: Option<bool>,
    pub private: Option<bool>,
}

/// Creates a repository from a template by copying the template's packs (server-side object
/// copy) and its default branch (or all branches) into a new storage manifest.
pub async fn generate_from_template(s: &AppState, tpl: &RepoCtx, actor: &User, req: GenerateReq) -> ApiResult<RepoCtx> {
    if !tpl.repo.is_template {
        let mut e = ApiError::unprocessable("Validation Failed");
        e.errors = Some(json!([{"resource": "Repository", "code": "custom", "field": "template_repo",
            "message": format!("{} is not a template repository", tpl.full_name())}]));
        return Err(e);
    }
    let name = req.name.clone().filter(|n| !n.is_empty()).ok_or_else(|| ApiError::validation("Repository", "name", "missing_field"))?;
    let owner = match req.owner.as_deref().filter(|o| !o.is_empty()) {
        Some(o) => db::user_by_login(&s.db, o).await?,
        None => actor.clone(),
    };
    if owner.id != actor.id && !actor.site_admin {
        let role = super::orgs_extra::org_role(&s.db, owner.id, actor.id).await?;
        if owner.kind != "Organization" || role.is_none() {
            return Err(ApiError::forbidden("You do not have permission to create repositories for this owner."));
        }
    }
    let manifest = s.git.manifest(tpl.repo.id).await?;
    let ctx = super::repos::create_repo(
        s,
        &owner,
        actor,
        super::repos::CreateRepo {
            name,
            description: req.description.clone(),
            private: Some(req.private.unwrap_or(false)),
            default_branch: Some(tpl.repo.default_branch.clone()),
            ..Default::default()
        },
    )
    .await?;
    let new_id = ctx.repo.id;
    let res: anyhow::Result<()> = async {
        // Packs are immutable and content-addressed: a server-side copy is a consistent snapshot.
        // Retry with a fresh manifest if gc removed a pack meanwhile.
        let mut m = manifest;
        for attempt in 0..3 {
            let mut ok = true;
            for pack in &m.packs {
                for ext in ["pack", "idx"] {
                    let from = ObjPath::from(format!("repos/{}/packs/{pack}.{ext}", tpl.repo.id));
                    let to = ObjPath::from(format!("repos/{new_id}/packs/{pack}.{ext}"));
                    if let Err(e) = s.store.copy(&from, &to).await {
                        if attempt == 2 {
                            return Err(e.into());
                        }
                        ok = false;
                    }
                }
            }
            if ok {
                break;
            }
            m = s.git.manifest(tpl.repo.id).await?;
        }
        let refs = crate::git::parse_refs(&m.refs);
        let default_ref = format!("refs/heads/{}", tpl.repo.default_branch);
        let all = req.include_all_branches.unwrap_or(false);
        let mut text = String::new();
        for (name, sha) in &refs {
            if (all && name.starts_with("refs/heads/")) || *name == default_ref {
                text.push_str(&format!("{sha} {name}\n"));
            }
        }
        sqlx::query("UPDATE repo_storage SET version = version + 1, head = $2, refs = $3, packs = $4, updated_at = now() WHERE repo_id = $1")
            .bind(new_id)
            .bind(&default_ref)
            .bind(&text)
            .bind(&m.packs)
            .execute(&s.db)
            .await?;
        sqlx::query("INSERT INTO repo_templates (repo_id, template_id) VALUES ($1, $2)").bind(new_id).bind(tpl.repo.id).execute(&s.db).await?;
        sqlx::query("UPDATE repositories SET pushed_at = now() WHERE id = $1").bind(new_id).execute(&s.db).await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        sqlx::query("DELETE FROM repositories WHERE id = $1").bind(new_id).execute(&s.db).await?;
        let _ = s.git.delete_repo(new_id).await;
        return Err(e.into());
    }
    let repo = db::repo_by_id(&s.db, new_id).await?;
    let ctx = RepoCtx { repo, owner: ctx.owner, perm: Perm::Admin };
    let sender = crate::json::user::simple(&s.api_url(), s.base_url(), actor);
    let _ = crate::activity::record(s, "CreateEvent", Some(actor.id), Some(&ctx), json!({
        "ref": null, "ref_type": "repository", "master_branch": ctx.repo.default_branch,
        "description": ctx.repo.description, "pusher_type": "user",
    }))
    .await;
    let _ = sender;
    Ok(ctx)
}

async fn generate(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(req): Json<GenerateReq>,
) -> ApiResult<Response> {
    let tpl = db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    let ctx = generate_from_template(&s, &tpl, &u, req).await?;
    let mut v = super::repos::full_json(&s, Some(&u), &ctx).await?;
    v["template_repository"] = crate::json::repo::repo(&s.api_url(), s.base_url(), &tpl);
    Ok(created(v))
}

// ---------------------------------------------------------------------------------------------
// Branch protection (stored configuration; enforcement is not implemented)

/// Stored protection config for a branch, if protected. Other areas can use this to report
/// `protected` on branches or to enforce rules.
pub async fn branch_protection(db: &sqlx::PgPool, repo_id: i64, branch: &str) -> ApiResult<Option<Value>> {
    Ok(sqlx::query_scalar("SELECT config FROM branch_protections WHERE repo_id = $1 AND branch = $2")
        .bind(repo_id)
        .bind(branch)
        .fetch_optional(db)
        .await?)
}

fn enabled(cfg: &Value, k: &str) -> bool {
    cfg.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn users_list(s: &AppState, users: &[User]) -> Vec<Value> {
    users.iter().map(|u| crate::json::user::simple(&s.api_url(), s.base_url(), u)).collect()
}

async fn load_users(s: &AppState, logins: Option<&Value>) -> ApiResult<Vec<User>> {
    let names: Vec<String> = logins
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|x| x.to_lowercase()).collect())
        .unwrap_or_default();
    if names.is_empty() {
        return Ok(vec![]);
    }
    Ok(sqlx::query_as("SELECT * FROM users WHERE lower(login) = ANY($1) ORDER BY id").bind(&names).fetch_all(&s.db).await?)
}

async fn status_checks_json(url: &str, sc: &Value) -> Value {
    let contexts: Vec<String> = sc
        .get("contexts")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let mut checks: Vec<Value> = sc
        .get("checks")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| c.get("context").and_then(Value::as_str).map(|ctx| json!({"context": ctx, "app_id": c.get("app_id").cloned().unwrap_or(Value::Null)})))
                .collect()
        })
        .unwrap_or_default();
    if checks.is_empty() {
        checks = contexts.iter().map(|c| json!({"context": c, "app_id": null})).collect();
    }
    let all_contexts: Vec<Value> = if contexts.is_empty() { checks.iter().map(|c| c["context"].clone()).collect() } else { contexts.iter().map(|c| json!(c)).collect() };
    json!({
        "url": url,
        "strict": sc.get("strict").and_then(Value::as_bool).unwrap_or(false),
        "contexts": all_contexts,
        "checks": checks,
        "contexts_url": format!("{url}/contexts"),
    })
}

async fn reviews_json(s: &AppState, url: &str, r: &Value) -> ApiResult<Value> {
    let mut v = json!({
        "url": url,
        "dismiss_stale_reviews": r.get("dismiss_stale_reviews").and_then(Value::as_bool).unwrap_or(false),
        "require_code_owner_reviews": r.get("require_code_owner_reviews").and_then(Value::as_bool).unwrap_or(false),
        "required_approving_review_count": r.get("required_approving_review_count").and_then(Value::as_i64).unwrap_or(1),
        "require_last_push_approval": r.get("require_last_push_approval").and_then(Value::as_bool).unwrap_or(false),
    });
    if let Some(d) = r.get("dismissal_restrictions").filter(|d| d.is_object()) {
        let users = load_users(s, d.get("users")).await?;
        v["dismissal_restrictions"] = json!({
            "url": format!("{url}/dismissal_restrictions"),
            "users_url": format!("{url}/dismissal_restrictions/users"),
            "teams_url": format!("{url}/dismissal_restrictions/teams"),
            "users": users_list(s, &users), "teams": [], "apps": [],
        });
    }
    if let Some(b) = r.get("bypass_pull_request_allowances").filter(|d| d.is_object()) {
        let users = load_users(s, b.get("users")).await?;
        v["bypass_pull_request_allowances"] = json!({ "users": users_list(s, &users), "teams": [], "apps": [] });
    }
    Ok(v)
}

/// Renders the `protected-branch` shape from the stored config.
pub async fn protection_json(s: &AppState, ctx: &RepoCtx, branch: &str, cfg: &Value) -> ApiResult<Value> {
    let url = format!("{}/repos/{}/branches/{}/protection", s.api_url(), ctx.full_name(), crate::json::release::seg(branch));
    let mut v = json!({
        "url": url,
        "required_signatures": { "url": format!("{url}/required_signatures"), "enabled": enabled(cfg, "required_signatures") },
        "enforce_admins": { "url": format!("{url}/enforce_admins"), "enabled": enabled(cfg, "enforce_admins") },
        "required_linear_history": { "enabled": enabled(cfg, "required_linear_history") },
        "allow_force_pushes": { "enabled": enabled(cfg, "allow_force_pushes") },
        "allow_deletions": { "enabled": enabled(cfg, "allow_deletions") },
        "block_creations": { "enabled": enabled(cfg, "block_creations") },
        "required_conversation_resolution": { "enabled": enabled(cfg, "required_conversation_resolution") },
        "lock_branch": { "enabled": enabled(cfg, "lock_branch") },
        "allow_fork_syncing": { "enabled": enabled(cfg, "allow_fork_syncing") },
    });
    if let Some(sc) = cfg.get("required_status_checks").filter(|x| x.is_object()) {
        v["required_status_checks"] = status_checks_json(&format!("{url}/required_status_checks"), sc).await;
    }
    if let Some(r) = cfg.get("required_pull_request_reviews").filter(|x| x.is_object()) {
        v["required_pull_request_reviews"] = reviews_json(s, &format!("{url}/required_pull_request_reviews"), r).await?;
    }
    if let Some(r) = cfg.get("restrictions").filter(|x| x.is_object()) {
        let users = load_users(s, r.get("users")).await?;
        let rurl = format!("{url}/restrictions");
        v["restrictions"] = json!({
            "url": rurl, "users_url": format!("{rurl}/users"), "teams_url": format!("{rurl}/teams"), "apps_url": format!("{rurl}/apps"),
            "users": users_list(s, &users), "teams": [], "apps": [],
        });
    }
    Ok(v)
}

async fn prot_ctx(s: &AppState, u: &User, owner: &str, repo: &str, branch: &str) -> ApiResult<RepoCtx> {
    let ctx = db::load_repo(&s.db, Some(u), owner, repo).await?;
    ctx.require(Perm::Admin)?;
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    if !refs.contains_key(&format!("refs/heads/{branch}")) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "Branch not found"));
    }
    Ok(ctx)
}

async fn load_protection(s: &AppState, ctx: &RepoCtx, branch: &str) -> ApiResult<Value> {
    branch_protection(&s.db, ctx.repo.id, branch)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Branch not protected"))
}

async fn save_protection(s: &AppState, ctx: &RepoCtx, branch: &str, cfg: &Value) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO branch_protections (repo_id, branch, config) VALUES ($1,$2,$3)
         ON CONFLICT (repo_id, branch) DO UPDATE SET config = $3, updated_at = now()",
    )
    .bind(ctx.repo.id)
    .bind(branch)
    .bind(cfg)
    .execute(&s.db)
    .await?;
    Ok(())
}

async fn get_protection(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, branch)): Path<(String, String, String)>) -> ApiResult<Json<Value>> {
    let ctx = prot_ctx(&s, &u, &owner, &repo, &branch).await?;
    let cfg = load_protection(&s, &ctx, &branch).await?;
    let mut v = protection_json(&s, &ctx, &branch, &cfg).await?;
    v["name"] = json!(branch);
    v["enabled"] = json!(true);
    v["protection_url"] = v["url"].clone();
    Ok(Json(v))
}

async fn put_protection(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path((owner, repo, branch)): Path<(String, String, String)>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let ctx = prot_ctx(&s, &u, &owner, &repo, &branch).await?;
    let Some(obj) = body.as_object() else {
        return Err(ApiError::unprocessable("Invalid request."));
    };
    for k in ["required_status_checks", "enforce_admins", "required_pull_request_reviews", "restrictions"] {
        if !obj.contains_key(k) {
            let mut e = ApiError::unprocessable("Invalid request.");
            e.message = format!("Invalid request.\n\n\"{k}\" wasn't supplied.");
            return Err(e);
        }
    }
    let mut cfg = Map::new();
    for (k, v) in obj {
        match k.as_str() {
            "enforce_admins" | "required_linear_history" | "allow_force_pushes" | "allow_deletions" | "block_creations"
            | "required_conversation_resolution" | "lock_branch" | "allow_fork_syncing" => {
                cfg.insert(k.clone(), json!(v.as_bool().unwrap_or(false)));
            }
            "required_status_checks" | "required_pull_request_reviews" | "restrictions" => {
                if v.is_object() {
                    cfg.insert(k.clone(), v.clone());
                }
            }
            _ => {}
        }
    }
    // Keep required_signatures (managed by its own endpoint).
    if let Some(old) = branch_protection(&s.db, ctx.repo.id, &branch).await? {
        if let Some(sig) = old.get("required_signatures") {
            cfg.insert("required_signatures".into(), sig.clone());
        }
    }
    let cfg = Value::Object(cfg);
    save_protection(&s, &ctx, &branch, &cfg).await?;
    Ok(Json(protection_json(&s, &ctx, &branch, &cfg).await?))
}

async fn delete_protection(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo, branch)): Path<(String, String, String)>) -> ApiResult<Response> {
    let ctx = prot_ctx(&s, &u, &owner, &repo, &branch).await?;
    load_protection(&s, &ctx, &branch).await?;
    sqlx::query("DELETE FROM branch_protections WHERE repo_id = $1 AND branch = $2").bind(ctx.repo.id).bind(&branch).execute(&s.db).await?;
    Ok(no_content())
}

/// Shared implementation for the boolean sub-resources (enforce_admins, required_signatures).
async fn flag(s: &AppState, u: &User, owner: &str, repo: &str, branch: &str, key: &str, set: Option<bool>) -> ApiResult<Response> {
    let ctx = prot_ctx(s, u, owner, repo, branch).await?;
    let mut cfg = load_protection(s, &ctx, branch).await?;
    if let Some(v) = set {
        cfg[key] = json!(v);
        save_protection(s, &ctx, branch, &cfg).await?;
        if !v {
            return Ok(no_content());
        }
    }
    let url = format!("{}/repos/{}/branches/{}/protection/{key}", s.api_url(), ctx.full_name(), crate::json::release::seg(branch));
    Ok(axum::Json(json!({ "url": url, "enabled": enabled(&cfg, key) })).into_response())
}

async fn get_enforce_admins(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    flag(&s, &u, &o, &r, &b, "enforce_admins", None).await
}
async fn set_enforce_admins(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    flag(&s, &u, &o, &r, &b, "enforce_admins", Some(true)).await
}
async fn del_enforce_admins(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    flag(&s, &u, &o, &r, &b, "enforce_admins", Some(false)).await
}
async fn get_signatures(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    flag(&s, &u, &o, &r, &b, "required_signatures", None).await
}
async fn set_signatures(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    flag(&s, &u, &o, &r, &b, "required_signatures", Some(true)).await
}
async fn del_signatures(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    flag(&s, &u, &o, &r, &b, "required_signatures", Some(false)).await
}

/// Shared implementation for object sub-resources (reviews, status checks).
async fn sub_object(
    s: &AppState,
    u: &User,
    (owner, repo, branch): (&str, &str, &str),
    key: &str,
    op: Option<Option<Value>>,
) -> ApiResult<Response> {
    let ctx = prot_ctx(s, u, owner, repo, branch).await?;
    let mut cfg = load_protection(s, &ctx, branch).await?;
    match op {
        Some(None) => {
            if cfg.get(key).is_none_or(|v| !v.is_object()) {
                return Err(ApiError::not_found());
            }
            cfg.as_object_mut().unwrap().remove(key);
            save_protection(s, &ctx, branch, &cfg).await?;
            return Ok(no_content());
        }
        Some(Some(patch)) => {
            let mut cur = cfg.get(key).cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
            if let (Some(c), Some(p)) = (cur.as_object_mut(), patch.as_object()) {
                for (k, v) in p {
                    c.insert(k.clone(), v.clone());
                }
            }
            cfg[key] = cur;
            save_protection(s, &ctx, branch, &cfg).await?;
        }
        None => {}
    }
    let v = protection_json(s, &ctx, branch, &cfg).await?;
    match v.get(key) {
        Some(x) => Ok(axum::Json(x.clone()).into_response()),
        None => Err(ApiError::not_found()),
    }
}

async fn get_pr_reviews(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    sub_object(&s, &u, (&o, &r, &b), "required_pull_request_reviews", None).await
}
async fn patch_pr_reviews(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>, Json(p): Json<Value>) -> ApiResult<Response> {
    sub_object(&s, &u, (&o, &r, &b), "required_pull_request_reviews", Some(Some(p))).await
}
async fn del_pr_reviews(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    sub_object(&s, &u, (&o, &r, &b), "required_pull_request_reviews", Some(None)).await
}
async fn get_status_checks(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    sub_object(&s, &u, (&o, &r, &b), "required_status_checks", None).await
}
async fn patch_status_checks(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>, Json(p): Json<Value>) -> ApiResult<Response> {
    sub_object(&s, &u, (&o, &r, &b), "required_status_checks", Some(Some(p))).await
}
async fn del_status_checks(State(s): State<AppState>, AuthUser(u): AuthUser, Path((o, r, b)): Path<(String, String, String)>) -> ApiResult<Response> {
    sub_object(&s, &u, (&o, &r, &b), "required_status_checks", Some(None)).await
}

// ---------------------------------------------------------------------------------------------
// Contributors

/// (user or (name,email), contributions) from `git shortlog` on the default branch.
pub async fn contributors_list(s: &AppState, ctx: &RepoCtx, anon: bool) -> ApiResult<Vec<Value>> {
    let (_, refs) = s.git.refs(ctx.repo.id).await?;
    let head = format!("refs/heads/{}", ctx.repo.default_branch);
    if !refs.contains_key(&head) {
        return Ok(vec![]);
    }
    let out = super::plat::git_read(s, ctx.repo.id, &["shortlog", "-sne", &head]).await?.unwrap_or_default();
    let text = String::from_utf8_lossy(&out);
    let mut entries: Vec<(i64, String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((n, rest)) = line.split_once('\t') else { continue };
        let n: i64 = n.trim().parse().unwrap_or(0);
        let (name, email) = match (rest.rfind('<'), rest.rfind('>')) {
            (Some(a), Some(b)) if a < b => (rest[..a].trim().to_string(), rest[a + 1..b].to_string()),
            _ => (rest.to_string(), String::new()),
        };
        entries.push((n, name, email));
    }
    let emails: Vec<String> = entries.iter().map(|e| e.2.clone()).collect();
    let users = super::plat::users_by_emails(&s.db, &emails).await?;
    let mut by_user: HashMap<i64, (User, i64)> = HashMap::new();
    let mut anons: Vec<(String, String, i64)> = Vec::new();
    for (n, name, email) in entries {
        match users.get(&email.to_lowercase()) {
            Some(u) => by_user.entry(u.id).or_insert_with(|| (u.clone(), 0)).1 += n,
            None => anons.push((name, email, n)),
        }
    }
    let api = s.api_url();
    let mut items: Vec<(i64, Value)> = by_user
        .into_values()
        .map(|(u, n)| {
            let mut v = crate::json::user::simple(&api, s.base_url(), &u);
            let o = v.as_object_mut().unwrap();
            o.retain(|_, val| !val.is_null());
            o.insert("contributions".into(), json!(n));
            (n, v)
        })
        .collect();
    if anon {
        items.extend(anons.into_iter().map(|(name, email, n)| (n, json!({"type": "Anonymous", "name": name, "email": email, "contributions": n}))));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(items.into_iter().map(|(_, v)| v).collect())
}

// ---------------------------------------------------------------------------------------------
// License detection

const LICENSE_NAMES: &[&str] = &["license", "license.md", "license.txt", "licence", "licence.md", "licence.txt", "copying", "copying.md", "copying.txt", "unlicense", "license-mit", "license-apache"];

/// Finds and identifies the license file at the root of `branch`.
/// Returns (path, blob sha, size, content, license-simple JSON).
pub async fn find_license(s: &AppState, ctx: &RepoCtx, refname: &str) -> ApiResult<Option<(String, String, i64, Vec<u8>, Value)>> {
    let Some(out) = super::plat::git_read(s, ctx.repo.id, &["ls-tree", "-l", refname]).await? else { return Ok(None) };
    let text = String::from_utf8_lossy(&out);
    let mut cands: Vec<(usize, String, String, i64)> = Vec::new();
    for line in text.lines() {
        // "<mode> blob <sha> <size>\t<path>"
        let Some((meta, path)) = line.split_once('\t') else { continue };
        let f: Vec<&str> = meta.split_whitespace().collect();
        if f.len() < 4 || f[1] != "blob" {
            continue;
        }
        if let Some(rank) = LICENSE_NAMES.iter().position(|n| *n == path.to_ascii_lowercase()) {
            cands.push((rank, path.to_string(), f[2].to_string(), f[3].parse().unwrap_or(0)));
        }
    }
    cands.sort();
    let Some((_, path, sha, size)) = cands.into_iter().next() else { return Ok(None) };
    let content = super::plat::git_read(s, ctx.repo.id, &["cat-file", "blob", &sha]).await?.unwrap_or_default();
    let api = s.api_url();
    let lic = match super::misc::detect_license(&String::from_utf8_lossy(&content)) {
        Some(l) => super::misc::license_simple(&api, l),
        None => super::misc::license_other(),
    };
    Ok(Some((path, sha, size, content, lic)))
}

#[derive(Deserialize)]
struct RefQuery {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
}

async fn repo_license(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(q): Query<RefQuery>,
) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let r = q.git_ref.clone().unwrap_or_else(|| ctx.repo.default_branch.clone());
    let (path, sha, size, content, lic) = find_license(&s, &ctx, &r).await?.ok_or_else(ApiError::not_found)?;
    let api = s.api_url();
    let base = s.base_url();
    let full = ctx.full_name();
    let enc = base64::engine::general_purpose::STANDARD.encode(&content);
    let wrapped: String = enc.as_bytes().chunks(60).map(|c| format!("{}\n", String::from_utf8_lossy(c))).collect();
    let self_url = format!("{api}/repos/{full}/contents/{path}?ref={r}");
    let git_url = format!("{api}/repos/{full}/git/blobs/{sha}");
    let html_url = format!("{base}/{full}/blob/{r}/{path}");
    Ok(Json(json!({
        "name": path, "path": path, "sha": sha, "size": size,
        "url": self_url, "html_url": html_url, "git_url": git_url,
        "download_url": format!("{base}/{full}/raw/{r}/{path}"),
        "type": "file", "content": wrapped, "encoding": "base64",
        "_links": { "self": self_url, "git": git_url, "html": html_url },
        "license": lic,
    })))
}

// ---------------------------------------------------------------------------------------------
// Events

async fn events_page(s: &AppState, uri: &OriginalUri, where_sql: &str, id: i64, p: PageParams) -> ApiResult<Paged> {
    let sql = format!("SELECT * FROM activity_events WHERE {where_sql} ORDER BY id DESC LIMIT $2 OFFSET $3");
    let rows: Vec<EventRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(id).bind(p.limit()).bind(p.offset()).fetch_all(&s.db).await?;
    let items = crate::activity::render(s, &rows).await?;
    Ok(Paged::new(uri, s, p, None, items))
}

async fn repo_events(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    events_page(&s, &uri, "repo_id = $1", ctx.repo.id, p).await
}

async fn public_events(State(s): State<AppState>, uri: OriginalUri, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    events_page(&s, &uri, "public AND $1 = $1", 0, p).await
}

async fn user_events(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(v): MaybeUser,
    Path(login): Path<String>,
    Query(p): Query<PageParams>,
) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    if v.as_ref().is_some_and(|v| v.id == u.id) {
        events_page(&s, &uri, "actor_id = $1", u.id, p).await
    } else {
        events_page(&s, &uri, "actor_id = $1 AND public", u.id, p).await
    }
}

async fn user_public_events(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    events_page(&s, &uri, "actor_id = $1 AND public", u.id, p).await
}

async fn received_events(State(s): State<AppState>, uri: OriginalUri, Path(login): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let u = db::user_by_login(&s.db, &login).await?;
    events_page(
        &s,
        &uri,
        "public AND (actor_id IN (SELECT followee_id FROM follows WHERE follower_id = $1)
            OR repo_id IN (SELECT repo_id FROM watches WHERE user_id = $1)
            OR repo_id IN (SELECT repo_id FROM stars WHERE user_id = $1))",
        u.id,
        p,
    )
    .await
}

async fn org_events(State(s): State<AppState>, uri: OriginalUri, MaybeUser(v): MaybeUser, Path(org): Path<String>, Query(p): Query<PageParams>) -> ApiResult<Paged> {
    let o = db::user_by_login(&s.db, &org).await?;
    let member = match &v {
        Some(v) => super::orgs_extra::org_role(&s.db, o.id, v.id).await?.is_some() || v.site_admin,
        None => false,
    };
    if member {
        events_page(&s, &uri, "org_id = $1", o.id, p).await
    } else {
        events_page(&s, &uri, "org_id = $1 AND public", o.id, p).await
    }
}

// ---------------------------------------------------------------------------------------------
// Notifications & invitations (forge has no notification inbox yet: lists are empty)

async fn mark_read(_u: AuthUser) -> Response {
    StatusCode::RESET_CONTENT.into_response()
}

async fn repo_notifications(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    Ok(Json(json!([])))
}

async fn repo_mark_read(State(s): State<AppState>, AuthUser(u): AuthUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Response> {
    db::load_repo(&s.db, Some(&u), &owner, &repo).await?;
    Ok(StatusCode::RESET_CONTENT.into_response())
}
