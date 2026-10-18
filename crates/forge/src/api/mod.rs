//! REST API mounted at /api/v3. One module per area; each exposes `routes()`.
pub mod keys;
pub mod branches;
pub mod checks;
pub mod review_reactions;
pub mod commits;
pub mod contents;
pub mod cross_compare;
pub mod forks;
pub mod git_data;
pub mod gitapi;
pub mod meta;
pub mod pulls;
pub mod actions;
pub mod actions_secrets;
pub mod runner_misc;
pub mod runner_runners;
pub mod apps_core;
pub mod apps_enterprise;
pub mod apps_forge;
pub mod apps_migrations;
pub mod apps_oauth;
pub mod actions_settings;
pub mod commit_comments;
pub mod deployments;
pub mod discussions;
pub mod gists;
pub mod gist_git;
pub mod misc;
pub mod orgs_extra;
pub mod orgs_props;
pub mod orgs_registries;
pub mod orgs_roles;
pub mod orgs_settings;
pub mod orgs_teams;
pub mod plat;
pub mod projects_v2;
pub mod releases;
pub mod repo_extras;
pub mod social;
pub mod repos;
pub mod reviews;
pub mod statuses;
pub mod collaborators;
pub mod issue_comments;
pub mod issue_events;
pub mod issue_ops;
pub mod repo_ops;
pub mod issues;
pub mod labels;
pub mod reactions;
pub mod ref_dispatch;
pub mod search;
pub mod search_git;
pub mod stars;
pub mod users;
pub mod autolinks;
pub mod rulesets;
pub mod signing_keys;
pub mod webhooks;
pub mod ent_actions;
pub mod ent_admin;
pub mod ent_enterprise;
pub mod ent_manage;
pub mod ent_prereceive;
pub mod ent_scim;
pub mod ent_teams;
// repos build-out area
pub mod repos_env;
pub mod repos_settings;
pub mod repos_rules;
pub mod repos_stats;
pub mod repos_pages;
pub mod users_extra;
pub mod issues_extra;
pub mod gist_comments;
pub mod notifications;
pub mod invitations;
pub mod activity_extra;
pub mod dependency_graph;
pub mod events_emit;
// packages build-out area
pub mod packages;
pub mod projects_v2_rest;

use axum::extract::{OriginalUri, Request};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(meta::routes())
        .merge(users::routes())
        .merge(repos::routes())
        .merge(keys::routes())
        .merge(pulls::routes())
        .merge(reviews::routes())
        .merge(git_data::routes())
        .merge(contents::routes())
        .merge(commits::routes())
        .merge(branches::routes())
        .merge(forks::routes())
        .merge(statuses::routes())
        .merge(issues::routes())
        .merge(issue_comments::routes())
        .merge(issue_events::routes())
        .merge(labels::routes())
        .merge(reactions::routes())
        .merge(search::routes())
        .merge(search_git::routes())
        .merge(stars::routes())
        .merge(collaborators::routes())
        .merge(releases::routes())
        .merge(webhooks::routes())
        .merge(social::routes())
        .merge(orgs_extra::routes())
        .merge(orgs_teams::routes())
        .merge(orgs_settings::routes())
        .merge(orgs_roles::routes())
        .merge(orgs_props::routes())
        .merge(orgs_registries::routes())
        .merge(repo_extras::routes())
        .merge(commit_comments::routes())
        .merge(deployments::routes())
        .merge(gists::routes())
        .merge(misc::routes())
        .merge(autolinks::routes())
        .merge(rulesets::routes())
        .merge(signing_keys::routes())
        .merge(actions::routes())
        .merge(actions_settings::routes())
        .merge(actions_settings::enterprise_routes())
        .merge(checks::routes())
        .merge(review_reactions::routes())
        .merge(actions_secrets::routes())
        .merge(runner_runners::routes())
        .merge(runner_misc::routes())
        .merge(ref_dispatch::routes())
        .merge(apps_core::routes())
        .merge(apps_oauth::routes())
        .merge(apps_enterprise::routes())
        .merge(apps_migrations::routes())
        .merge(ent_admin::routes())
        .merge(ent_prereceive::routes())
        .merge(ent_actions::routes())
        .merge(ent_enterprise::routes())
        .merge(ent_teams::routes())
        .merge(ent_scim::routes())
        .merge(ent_manage::routes())
        // repos build-out area
        .merge(repos_env::routes())
        .merge(repos_settings::routes())
        .merge(repos_rules::routes())
        .merge(repos_stats::routes())
        .merge(repos_pages::routes())
        .merge(users_extra::routes())
        .merge(issues_extra::routes())
        .merge(gist_comments::routes())
        .merge(notifications::routes())
        .merge(invitations::routes())
        .merge(activity_extra::routes())
        .merge(dependency_graph::routes())
        // packages build-out area
        .merge(packages::routes())
        .merge(projects_v2_rest::routes())
        // advsec area
        .merge(crate::advsec::routes())
        .fallback(|| async { crate::error::ApiError::not_found() })
}

pub const SCOPES: &str = "repo, read:org, gist, workflow, admin:org, user, delete_repo, admin:public_key, write:packages";

/// Adds GitHub's standard response headers to every API response.
pub async fn github_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let rid = crate::auth::random_string(16).to_uppercase();
    let set = |h: &mut HeaderMap, k: &'static str, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(HeaderName::from_static(k), v);
        }
    };
    set(h, "x-github-request-id", &rid);
    set(h, "x-github-enterprise-version", "3.14.0");
    set(h, "x-github-media-type", "github.v3; format=json");
    set(h, "x-github-api-version-selected", "2022-11-28");
    set(h, "x-oauth-scopes", SCOPES);
    set(h, "x-accepted-oauth-scopes", "repo");
    set(h, "x-ratelimit-limit", "5000");
    set(h, "x-ratelimit-remaining", "4999");
    set(h, "x-ratelimit-reset", &(chrono::Utc::now().timestamp() + 3600).to_string());
    set(h, "x-ratelimit-used", "1");
    set(h, "x-ratelimit-resource", "core");
    if !h.contains_key(header::CACHE_CONTROL) {
        set(h, "cache-control", "private, max-age=60, s-maxage=60");
    }
    res
}

/// `page`/`per_page` query parameters (GitHub defaults: 1 and 30, max 100).
#[derive(Debug, Deserialize, Clone, Copy, Default)]
pub struct PageParams {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl PageParams {
    pub fn page(&self) -> i64 {
        // Bounded so `offset()` can't overflow (a huge `page` used to wrap to a negative offset).
        self.page.unwrap_or(1).clamp(1, 1_000_000)
    }
    pub fn per_page(&self) -> i64 {
        self.per_page.unwrap_or(30).clamp(1, 100)
    }
    pub fn limit(&self) -> i64 {
        self.per_page()
    }
    pub fn offset(&self) -> i64 {
        (self.page() - 1) * self.per_page()
    }
}

/// A JSON list response with a GitHub `Link` header. `total` enables `last`; without it, `next`
/// is emitted when the page is full.
pub struct Paged {
    pub uri: Uri,
    pub base_url: String,
    pub params: PageParams,
    pub total: Option<i64>,
    pub items: Vec<Value>,
}

impl Paged {
    pub fn new(uri: &OriginalUri, state: &AppState, params: PageParams, total: Option<i64>, items: Vec<Value>) -> Self {
        Paged { uri: uri.0.clone(), base_url: state.base_url().to_string(), params, total, items }
    }
}

fn page_url(base: &str, uri: &Uri, page: i64, per_page: i64) -> String {
    let mut pairs: Vec<(String, String)> = uri
        .query()
        .map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect())
        .unwrap_or_default();
    pairs.retain(|(k, _)| k != "page" && k != "per_page");
    pairs.push(("per_page".into(), per_page.to_string()));
    pairs.push(("page".into(), page.to_string()));
    let q = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
    format!("{base}{}?{q}", uri.path())
}

impl IntoResponse for Paged {
    fn into_response(self) -> Response {
        let page = self.params.page();
        let per = self.params.per_page();
        let mut links = Vec::new();
        let last = self.total.map(|t| ((t + per - 1) / per).max(1));
        let has_next = match last {
            Some(l) => page < l,
            None => self.items.len() as i64 >= per,
        };
        if has_next {
            links.push(format!("<{}>; rel=\"next\"", page_url(&self.base_url, &self.uri, page + 1, per)));
            if let Some(l) = last {
                links.push(format!("<{}>; rel=\"last\"", page_url(&self.base_url, &self.uri, l, per)));
            }
        }
        if page > 1 {
            links.push(format!("<{}>; rel=\"first\"", page_url(&self.base_url, &self.uri, 1, per)));
            links.push(format!("<{}>; rel=\"prev\"", page_url(&self.base_url, &self.uri, page - 1, per)));
        }
        let mut res = (StatusCode::OK, Json(Value::Array(self.items))).into_response();
        if !links.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&links.join(", ")) {
                res.headers_mut().insert(header::LINK, v);
            }
        }
        res
    }
}

/// 201 Created with JSON body (and Location header when `url` is present).
pub fn created(v: Value) -> Response {
    let loc = v.get("url").and_then(|u| u.as_str()).map(str::to_string);
    let mut res = (StatusCode::CREATED, Json(v)).into_response();
    if let Some(l) = loc.and_then(|l| HeaderValue::from_str(&l).ok()) {
        res.headers_mut().insert(header::LOCATION, l);
    }
    res
}

pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}
