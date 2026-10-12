//! API root, meta, rate_limit, octocat/zen and similar global endpoints.
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use crate::extract::Json;
use serde_json::{json, Value};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/meta", get(meta))
        .route("/rate_limit", get(rate_limit))
        .route("/zen", get(|| async { "Keep it logically awesome." }))
        .route("/octocat", get(|| async { "forge" }))
        .route("/versions", get(|| async { Json(json!(["2022-11-28"])) }))
}

pub async fn root_handler(s: State<AppState>) -> Json<Value> {
    root(s).await
}

async fn root(State(s): State<AppState>) -> Json<Value> {
    let a = s.api_url();
    Json(json!({
        "current_user_url": format!("{a}/user"),
        "current_user_authorizations_html_url": format!("{}/settings/connections/applications{{/client_id}}", s.base_url()),
        "authorizations_url": format!("{a}/authorizations"),
        "code_search_url": format!("{a}/search/code?q={{query}}{{&page,per_page,sort,order}}"),
        "commit_search_url": format!("{a}/search/commits?q={{query}}{{&page,per_page,sort,order}}"),
        "emails_url": format!("{a}/user/emails"),
        "emojis_url": format!("{a}/emojis"),
        "events_url": format!("{a}/events"),
        "feeds_url": format!("{a}/feeds"),
        "followers_url": format!("{a}/user/followers"),
        "following_url": format!("{a}/user/following{{/target}}"),
        "gists_url": format!("{a}/gists{{/gist_id}}"),
        "hub_url": format!("{a}/hub"),
        "issue_search_url": format!("{a}/search/issues?q={{query}}{{&page,per_page,sort,order}}"),
        "issues_url": format!("{a}/issues"),
        "keys_url": format!("{a}/user/keys"),
        "label_search_url": format!("{a}/search/labels?q={{query}}&repository_id={{repository_id}}{{&page,per_page}}"),
        "notifications_url": format!("{a}/notifications"),
        "organization_url": format!("{a}/orgs/{{org}}"),
        "organization_repositories_url": format!("{a}/orgs/{{org}}/repos{{?type,page,per_page,sort}}"),
        "organization_teams_url": format!("{a}/orgs/{{org}}/teams"),
        "public_gists_url": format!("{a}/gists/public"),
        "rate_limit_url": format!("{a}/rate_limit"),
        "repository_url": format!("{a}/repos/{{owner}}/{{repo}}"),
        "repository_search_url": format!("{a}/search/repositories?q={{query}}{{&page,per_page,sort,order}}"),
        "current_user_repositories_url": format!("{a}/user/repos{{?type,page,per_page,sort}}"),
        "starred_url": format!("{a}/user/starred{{/owner}}{{/repo}}"),
        "starred_gists_url": format!("{a}/gists/starred"),
        "topic_search_url": format!("{a}/search/topics?q={{query}}{{&page,per_page}}"),
        "user_url": format!("{a}/users/{{user}}"),
        "user_organizations_url": format!("{a}/user/orgs"),
        "user_repositories_url": format!("{a}/users/{{user}}/repos{{?type,page,per_page,sort}}"),
        "user_search_url": format!("{a}/search/users?q={{query}}{{&page,per_page,sort,order}}"),
    }))
}

async fn meta() -> Json<Value> {
    Json(json!({
        "verifiable_password_authentication": true,
        "installed_version": "3.14.0",
        "ssh_key_fingerprints": {},
        "hooks": [], "web": [], "api": [], "git": [], "packages": [], "pages": [], "importer": [], "actions": [], "dependabot": [],
        "domains": {}
    }))
}

async fn rate_limit() -> Json<Value> {
    let reset = chrono::Utc::now().timestamp() + 3600;
    let r = json!({"limit": 5000, "used": 0, "remaining": 5000, "reset": reset});
    Json(json!({
        "resources": {
            "core": r, "search": {"limit": 30, "used": 0, "remaining": 30, "reset": reset},
            "graphql": r, "integration_manifest": r, "source_import": r, "code_scanning_upload": r,
            "actions_runner_registration": r, "scim": r
        },
        "rate": r
    }))
}
