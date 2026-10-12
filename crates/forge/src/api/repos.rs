//! Repositories: create/get/update/delete and listings.
use axum::extract::{OriginalUri, Path, Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use crate::extract::Json;
use serde::Deserialize;
use serde_json::Value;

use super::{created, no_content, PageParams, Paged};
use crate::auth::{AuthUser, MaybeUser};
use crate::db::{self, Perm, Repo, RepoCtx, User};
use crate::error::{ApiError, ApiResult};
use crate::git::cmd::Identity;
use crate::git::plumbing::{self, Change};
use crate::json;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/repos", get(list_mine).post(create_for_user))
        .route("/users/{username}/repos", get(list_for_user))
        .route("/orgs/{org}/repos", get(list_for_org).post(create_for_org))
        .route("/repositories/{id}", get(get_by_id))
        .route("/repos/{owner}/{repo}", get(get_repo).patch(update_repo).delete(delete_repo))
        .route("/repos/{owner}/{repo}/topics", get(get_topics).put(put_topics))
        .route("/repos/{owner}/{repo}/languages", get(languages))
        .route("/admin/organizations", post(create_org))
}

pub fn valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && !name.ends_with(".git")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[derive(Deserialize, Default)]
pub struct CreateRepo {
    pub name: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub private: Option<bool>,
    pub visibility: Option<String>,
    pub has_issues: Option<bool>,
    pub has_projects: Option<bool>,
    pub has_wiki: Option<bool>,
    pub has_discussions: Option<bool>,
    pub is_template: Option<bool>,
    pub auto_init: Option<bool>,
    pub default_branch: Option<String>,
    pub allow_squash_merge: Option<bool>,
    pub allow_merge_commit: Option<bool>,
    pub allow_rebase_merge: Option<bool>,
    pub allow_auto_merge: Option<bool>,
    pub delete_branch_on_merge: Option<bool>,
    pub gitignore_template: Option<String>,
    pub license_template: Option<String>,
}

/// Creates a repository owned by `owner` (user or org). Shared by REST and GraphQL.
pub async fn create_repo(state: &AppState, owner: &User, creator: &User, req: CreateRepo) -> ApiResult<RepoCtx> {
    if !valid_repo_name(&req.name) {
        return Err(ApiError::validation("Repository", "name", "custom"));
    }
    let private = req.private.unwrap_or(false) || matches!(req.visibility.as_deref(), Some("private" | "internal"));
    let branch = req.default_branch.clone().unwrap_or_else(|| "main".into());
    // Written into HEAD and used in ref names: must be a plausible branch name.
    if branch.is_empty()
        || branch.len() > 255
        || branch.starts_with('-')
        || branch.contains("..")
        || branch.chars().any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
    {
        return Err(ApiError::validation("Repository", "default_branch", "invalid"));
    }
    let repo: Option<Repo> = sqlx::query_as(
        "INSERT INTO repositories (owner_id, name, description, homepage, private, visibility, default_branch,
            has_issues, has_projects, has_wiki, has_discussions, is_template,
            allow_squash_merge, allow_merge_commit, allow_rebase_merge, allow_auto_merge, delete_branch_on_merge)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(owner.id)
    .bind(&req.name)
    .bind(req.description.as_ref().filter(|d| !d.is_empty()))
    .bind(req.homepage.as_ref().filter(|d| !d.is_empty()))
    .bind(private)
    .bind(if private { "private" } else { "public" })
    .bind(&branch)
    .bind(req.has_issues.unwrap_or(true))
    .bind(req.has_projects.unwrap_or(true))
    .bind(req.has_wiki.unwrap_or(true))
    .bind(req.has_discussions.unwrap_or(false))
    .bind(req.is_template.unwrap_or(false))
    .bind(req.allow_squash_merge.unwrap_or(true))
    .bind(req.allow_merge_commit.unwrap_or(true))
    .bind(req.allow_rebase_merge.unwrap_or(true))
    .bind(req.allow_auto_merge.unwrap_or(false))
    .bind(req.delete_branch_on_merge.unwrap_or(false))
    .fetch_optional(&state.db)
    .await?;
    let Some(repo) = repo else {
        let mut e = ApiError::validation("Repository", "name", "custom");
        e.message = "Repository creation failed.".into();
        e.errors = Some(serde_json::json!([{
            "resource": "Repository", "code": "custom", "field": "name",
            "message": "name already exists on this account"
        }]));
        return Err(e);
    };
    state.git.init_repo(repo.id, &branch).await?;
    if req.auto_init.unwrap_or(false) {
        let mut files = vec![Change::Put {
            path: "README.md".into(),
            content: format!(
                "# {}\n{}",
                repo.name,
                req.description.as_deref().map(|d| format!("{d}\n")).unwrap_or_default()
            )
            .into_bytes(),
            mode: "100644".into(),
        }];
        if let Some(t) = req.gitignore_template.as_deref() {
            let content = super::misc::gitignore_template(t).map(|(_, c)| c.to_string()).unwrap_or_else(|| gitignore_for(t));
            files.push(Change::Put { path: ".gitignore".into(), content: content.into_bytes(), mode: "100644".into() });
        }
        if let Some(text) = req.license_template.as_deref().and_then(|k| {
            let year = chrono::Datelike::year(&chrono::Utc::now());
            let holder = creator.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| owner.login.clone());
            super::misc::license_text(k, year, &holder)
        }) {
            files.push(Change::Put { path: "LICENSE".into(), content: text.into_bytes(), mode: "100644".into() });
        }
        let who = identity_for(creator);
        let git = state.git.git.clone();
        let refname = format!("refs/heads/{branch}");
        state
            .git
            .write(repo.id, |path| async move {
                let sha = plumbing::commit_changes(&git, &path, None, files, "Initial commit", &who, &who).await?;
                plumbing::update_ref(&git, &path, &refname, &sha, None).await
            })
            .await?;
        sqlx::query("UPDATE repositories SET pushed_at = now() WHERE id = $1").bind(repo.id).execute(&state.db).await?;
    }
    super::labels::on_repo_created(state, repo.id, creator).await?;
    let repo = db::repo_by_id(&state.db, repo.id).await?;
    Ok(RepoCtx { repo, owner: owner.clone(), perm: Perm::Admin })
}

fn gitignore_for(template: &str) -> String {
    match template.to_ascii_lowercase().as_str() {
        "rust" => "/target\nCargo.lock\n**/*.rs.bk\n".into(),
        "node" => "node_modules/\nnpm-debug.log*\ndist/\n.env\n".into(),
        "python" => "__pycache__/\n*.py[cod]\n.venv/\ndist/\n*.egg-info/\n".into(),
        "go" => "*.exe\n*.test\n*.out\nvendor/\n".into(),
        _ => String::new(),
    }
}

/// Commit identity for a forge user.
pub fn identity_for(u: &User) -> Identity {
    Identity {
        name: u.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| u.login.clone()),
        email: u.email.clone().unwrap_or_else(|| format!("{}+{}@users.noreply.forge", u.id, u.login)),
        when: None,
    }
}

async fn create_for_user(State(s): State<AppState>, AuthUser(u): AuthUser, Json(req): Json<CreateRepo>) -> ApiResult<Response> {
    let ctx = create_repo(&s, &u, &u, req).await?;
    Ok(created(json::repo::full(&s.api_url(), s.base_url(), &ctx, None, None)))
}

async fn create_for_org(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path(org): Path<String>,
    Json(req): Json<CreateRepo>,
) -> ApiResult<Response> {
    let o = db::user_by_login(&s.db, &org).await?;
    if o.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    let member: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)")
        .bind(o.id)
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !member && !u.site_admin {
        return Err(ApiError::forbidden("You need admin access to the organization before adding a repository to it."));
    }
    let ctx = create_repo(&s, &o, &u, req).await?;
    Ok(created(json::repo::full(&s.api_url(), s.base_url(), &ctx, None, None)))
}

#[derive(Deserialize)]
struct CreateOrg {
    login: String,
    admin: Option<String>,
    profile_name: Option<String>,
}

/// GHES admin endpoint; forge lets any authenticated user create an org they administer.
async fn create_org(State(s): State<AppState>, AuthUser(u): AuthUser, Json(req): Json<CreateOrg>) -> ApiResult<Response> {
    if !crate::forge_api::valid_login(&req.login) {
        return Err(ApiError::validation("Organization", "login", "invalid"));
    }
    let admin = match req.admin.as_deref() {
        Some(a) => db::user_by_login(&s.db, a).await?,
        None => u.clone(),
    };
    let org: Option<User> = sqlx::query_as(
        "INSERT INTO users (login, kind, name) VALUES ($1, 'Organization', $2) ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(&req.login)
    .bind(&req.profile_name)
    .fetch_optional(&s.db)
    .await?;
    let org = org.ok_or_else(|| ApiError::validation("Organization", "login", "already_exists"))?;
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(org.id)
        .bind(admin.id)
        .execute(&s.db)
        .await?;
    Ok(created(json::user::org_simple(&s.api_url(), s.base_url(), &org)))
}

/// Builds full-repository JSON including parent/source for forks.
pub async fn full_json(s: &AppState, viewer: Option<&User>, ctx: &RepoCtx) -> ApiResult<Value> {
    let mut parent = None;
    let mut source = None;
    if let Some(pid) = ctx.repo.fork_parent_id {
        if let Ok(p) = db::load_repo_by_id(&s.db, viewer, pid).await {
            let mut root = p.clone();
            while let Some(next) = root.repo.fork_parent_id {
                match db::load_repo_by_id(&s.db, viewer, next).await {
                    Ok(r) => root = r,
                    Err(_) => break,
                }
            }
            parent = Some(p);
            source = Some(root);
        }
    }
    let mut v = json::repo::full(&s.api_url(), s.base_url(), ctx, parent.as_ref(), source.as_ref());
    v["subscribers_count"] = serde_json::json!(subscribers_count(s, ctx.repo.id).await?);
    Ok(v)
}

/// Number of users watching (subscribed to) a repository.
pub async fn subscribers_count(s: &AppState, repo_id: i64) -> ApiResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM watches WHERE repo_id = $1 AND subscribed AND NOT ignored")
        .bind(repo_id)
        .fetch_one(&s.db)
        .await?)
}

async fn get_repo(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    Ok(Json(full_json(&s, u.as_ref(), &ctx).await?))
}

async fn get_by_id(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo_by_id(&s.db, u.as_ref(), id).await?;
    Ok(Json(full_json(&s, u.as_ref(), &ctx).await?))
}

#[derive(Deserialize)]
struct UpdateRepo {
    name: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    private: Option<bool>,
    visibility: Option<String>,
    has_issues: Option<bool>,
    has_projects: Option<bool>,
    has_wiki: Option<bool>,
    has_discussions: Option<bool>,
    is_template: Option<bool>,
    default_branch: Option<String>,
    allow_squash_merge: Option<bool>,
    allow_merge_commit: Option<bool>,
    allow_rebase_merge: Option<bool>,
    allow_auto_merge: Option<bool>,
    delete_branch_on_merge: Option<bool>,
    archived: Option<bool>,
}

async fn update_repo(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(p): Json<UpdateRepo>,
) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    if let Some(n) = &p.name {
        if !valid_repo_name(n) {
            return Err(ApiError::validation("Repository", "name", "custom"));
        }
    }
    if let Some(b) = &p.default_branch {
        if b != &ctx.repo.default_branch {
            let (_, refs) = s.git.refs(ctx.repo.id).await?;
            if !refs.contains_key(&format!("refs/heads/{b}")) && !refs.is_empty() {
                return Err(ApiError::validation("Repository", "default_branch", "invalid"));
            }
            crate::events::set_default_branch(&s, ctx.repo.id, b).await?;
        }
    }
    let private = p.private.or(p.visibility.as_deref().map(|v| v != "public"));
    let r: Repo = sqlx::query_as(
        "UPDATE repositories SET name = COALESCE($2, name), description = COALESCE($3, description),
            homepage = COALESCE($4, homepage), private = COALESCE($5, private),
            visibility = CASE WHEN COALESCE($5, private) THEN 'private' ELSE 'public' END,
            has_issues = COALESCE($6, has_issues), has_projects = COALESCE($7, has_projects),
            has_wiki = COALESCE($8, has_wiki), has_discussions = COALESCE($9, has_discussions),
            is_template = COALESCE($10, is_template), allow_squash_merge = COALESCE($11, allow_squash_merge),
            allow_merge_commit = COALESCE($12, allow_merge_commit), allow_rebase_merge = COALESCE($13, allow_rebase_merge),
            allow_auto_merge = COALESCE($14, allow_auto_merge), delete_branch_on_merge = COALESCE($15, delete_branch_on_merge),
            archived = COALESCE($16, archived), updated_at = now()
          WHERE id = $1 RETURNING *",
    )
    .bind(ctx.repo.id)
    .bind(&p.name)
    .bind(&p.description)
    .bind(&p.homepage)
    .bind(private)
    .bind(p.has_issues)
    .bind(p.has_projects)
    .bind(p.has_wiki)
    .bind(p.has_discussions)
    .bind(p.is_template)
    .bind(p.allow_squash_merge)
    .bind(p.allow_merge_commit)
    .bind(p.allow_rebase_merge)
    .bind(p.allow_auto_merge)
    .bind(p.delete_branch_on_merge)
    .bind(p.archived)
    .fetch_one(&s.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::validation("Repository", "name", "already_exists"),
        e => e.into(),
    })?;
    let ctx = RepoCtx { repo: r, ..ctx };
    Ok(Json(full_json(&s, u.as_ref(), &ctx).await?))
}

async fn delete_repo(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Response> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    if ctx.perm < Perm::Admin {
        return Err(ApiError::forbidden("Must have admin rights to Repository."));
    }
    sqlx::query("DELETE FROM repositories WHERE id = $1").bind(ctx.repo.id).execute(&s.db).await?;
    s.git.delete_repo(ctx.repo.id).await?;
    if let Some(parent) = ctx.repo.fork_parent_id {
        sqlx::query("UPDATE repositories SET forks_count = GREATEST(forks_count - 1, 0) WHERE id = $1")
            .bind(parent)
            .execute(&s.db)
            .await?;
    }
    Ok(no_content())
}

#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(rename = "type")]
    kind: Option<String>,
    visibility: Option<String>,
    affiliation: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

impl ListQuery {
    fn pages(&self) -> PageParams {
        PageParams { page: self.page, per_page: self.per_page }
    }
    fn order(&self) -> String {
        let col = match self.sort.as_deref() {
            Some("created") => "r.created_at",
            Some("pushed") => "r.pushed_at",
            Some("full_name") => "lower(o.login || '/' || r.name)",
            _ => "r.updated_at",
        };
        let default_dir = if self.sort.as_deref() == Some("full_name") { "ASC" } else { "DESC" };
        let dir = match self.direction.as_deref() {
            Some("asc") => "ASC",
            Some("desc") => "DESC",
            _ => default_dir,
        };
        format!("{col} {dir} NULLS LAST, r.id {dir}")
    }
}

/// Rows of (repo, owner) for listing, already permission-filtered by the SQL.
async fn list_rows(s: &AppState, where_sql: &str, binds: Vec<i64>, viewer: Option<i64>, q: &ListQuery) -> ApiResult<(Vec<(Repo, User)>, i64)> {
    let p = q.pages();
    let vis = match q.visibility.as_deref() {
        Some("public") => " AND NOT r.private",
        Some("private") => " AND r.private",
        _ => "",
    };
    // Repos visible to the viewer: public, or owner/collaborator/org member.
    let visible = "(NOT r.private OR r.owner_id = $1 OR EXISTS(SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = $1)
                    OR EXISTS(SELECT 1 FROM org_members m WHERE m.org_id = r.owner_id AND m.user_id = $1)
                    OR EXISTS(SELECT 1 FROM users a WHERE a.id = $1 AND a.site_admin))";
    let sql_base = format!("FROM repositories r JOIN users o ON o.id = r.owner_id WHERE {visible} AND ({where_sql}){vis}");
    let sql = format!("SELECT r.id {sql_base} ORDER BY {} LIMIT {} OFFSET {}", q.order(), p.limit(), p.offset());
    let count_sql = format!("SELECT count(*) {sql_base}");
    let mut qa = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql)).bind(viewer.unwrap_or(-1));
    let mut qc = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql)).bind(viewer.unwrap_or(-1));
    for b in &binds {
        qa = qa.bind(*b);
        qc = qc.bind(*b);
    }
    let ids = qa.fetch_all(&s.db).await?;
    let total = qc.fetch_one(&s.db).await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let r = db::repo_by_id(&s.db, id).await?;
        let o = db::user_by_id(&s.db, r.owner_id).await?;
        out.push((r, o));
    }
    Ok((out, total))
}

async fn to_json(s: &AppState, viewer: Option<&User>, rows: Vec<(Repo, User)>) -> ApiResult<Vec<Value>> {
    let mut items = Vec::with_capacity(rows.len());
    for (repo, owner) in rows {
        let perm = db::permission(&s.db, viewer, &repo, &owner).await?;
        let ctx = RepoCtx { repo, owner, perm };
        items.push(json::repo::repo(&s.api_url(), s.base_url(), &ctx));
    }
    Ok(items)
}

async fn list_mine(State(s): State<AppState>, uri: OriginalUri, AuthUser(u): AuthUser, Query(q): Query<ListQuery>) -> ApiResult<Paged> {
    let aff = q.affiliation.clone().unwrap_or_else(|| match q.kind.as_deref() {
        Some("owner") => "owner".into(),
        Some("member") => "collaborator,organization_member".into(),
        _ => "owner,collaborator,organization_member".into(),
    });
    let mut conds = Vec::new();
    if aff.contains("owner") {
        conds.push("r.owner_id = $2");
    }
    if aff.contains("collaborator") {
        conds.push("EXISTS(SELECT 1 FROM repo_collaborators c2 WHERE c2.repo_id = r.id AND c2.user_id = $2)");
    }
    if aff.contains("organization_member") {
        conds.push("EXISTS(SELECT 1 FROM org_members m2 WHERE m2.org_id = r.owner_id AND m2.user_id = $2)");
    }
    if conds.is_empty() {
        conds.push("FALSE");
    }
    let (rows, total) = list_rows(&s, &conds.join(" OR "), vec![u.id], Some(u.id), &q).await?;
    let items = to_json(&s, Some(&u), rows).await?;
    Ok(Paged::new(&uri, &s, q.pages(), Some(total), items))
}

async fn list_for_user(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(v): MaybeUser,
    Path(login): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let owner = db::user_by_login(&s.db, &login).await?;
    let cond = match q.kind.as_deref() {
        Some("member") => "EXISTS(SELECT 1 FROM repo_collaborators c2 WHERE c2.repo_id = r.id AND c2.user_id = $2)",
        Some("all") => "r.owner_id = $2 OR EXISTS(SELECT 1 FROM repo_collaborators c2 WHERE c2.repo_id = r.id AND c2.user_id = $2)",
        _ => "r.owner_id = $2",
    };
    let (rows, total) = list_rows(&s, cond, vec![owner.id], v.as_ref().map(|u| u.id), &q).await?;
    let items = to_json(&s, v.as_ref(), rows).await?;
    Ok(Paged::new(&uri, &s, q.pages(), Some(total), items))
}

async fn list_for_org(
    State(s): State<AppState>,
    uri: OriginalUri,
    MaybeUser(v): MaybeUser,
    Path(org): Path<String>,
    Query(mut q): Query<ListQuery>,
) -> ApiResult<Paged> {
    let owner = db::user_by_login(&s.db, &org).await?;
    if owner.kind != "Organization" {
        return Err(ApiError::not_found());
    }
    if q.sort.is_none() {
        q.sort = Some("created".into());
    }
    let cond = match q.kind.as_deref() {
        Some("public") => "r.owner_id = $2 AND NOT r.private",
        Some("private") => "r.owner_id = $2 AND r.private",
        Some("forks") => "r.owner_id = $2 AND r.fork_parent_id IS NOT NULL",
        Some("sources") => "r.owner_id = $2 AND r.fork_parent_id IS NULL",
        _ => "r.owner_id = $2",
    };
    let (rows, total) = list_rows(&s, cond, vec![owner.id], v.as_ref().map(|u| u.id), &q).await?;
    let items = to_json(&s, v.as_ref(), rows).await?;
    Ok(Paged::new(&uri, &s, q.pages(), Some(total), items))
}

async fn get_topics(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    Ok(Json(serde_json::json!({ "names": ctx.repo.topics })))
}

#[derive(Deserialize)]
struct Topics {
    names: Vec<String>,
}

async fn put_topics(
    State(s): State<AppState>,
    MaybeUser(u): MaybeUser,
    Path((owner, repo)): Path<(String, String)>,
    Json(t): Json<Topics>,
) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    ctx.require(Perm::Admin)?;
    let names: Vec<String> = t.names.iter().map(|n| n.to_lowercase()).collect();
    sqlx::query("UPDATE repositories SET topics = $2, updated_at = now() WHERE id = $1")
        .bind(ctx.repo.id)
        .bind(&names)
        .execute(&s.db)
        .await?;
    Ok(Json(serde_json::json!({ "names": names })))
}

async fn languages(State(s): State<AppState>, MaybeUser(u): MaybeUser, Path((owner, repo)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    let ctx = db::load_repo(&s.db, u.as_ref(), &owner, &repo).await?;
    let mut m = serde_json::Map::new();
    if let Some(l) = &ctx.repo.language {
        m.insert(l.clone(), Value::from(ctx.repo.size_kb * 1024));
    }
    Ok(Json(Value::Object(m)))
}
