//! GraphQL endpoint (/api/graphql): the subset of GitHub's schema used by gh.
//! Spec: docs/research/gh-protocol.md (§4.6 leniency, §5 SDL, §5.2 minimal GHES profile).
//!
//! GitHub-compatibility notes:
//! * Always HTTP 200 for executed queries; errors carry GitHub's top-level `type`
//!   (`NOT_FOUND`, `FORBIDDEN`, `UNPROCESSABLE`), lifted from async-graphql error extensions.
//! * Validation is lenient like GitHub's (overlapping fields of different types in disjoint
//!   fragments, ID/String variable interchange, undeclared variables): we run async-graphql in
//!   `ValidationMode::Fast` and fill absent nullable variables with `null`.
//! * Feature-gated fields from §5.2 are deliberately absent so gh takes the simplest GHES path.
pub mod connection;
pub mod enums;
pub mod ghcompat;
pub mod gists;
pub mod issue;
pub mod loaders;
pub mod mutation;
pub mod projects;
pub mod pull;
pub mod query;
pub mod repo;
pub mod search;
pub mod rulesets;
pub mod services;
pub mod tree;
pub mod user;

use std::sync::OnceLock;

use async_graphql::dataloader::DataLoader;
use async_graphql::parser::types::DocumentOperations;
use async_graphql::{EmptySubscription, ErrorExtensions, Schema, ValidationMode};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};

use crate::auth::MaybeUser;
use crate::db::User;
use crate::error::ApiError;
use crate::state::AppState;

pub use loaders::Ld;

/// The mutation root: the original subset plus the gh-compatibility additions.
#[derive(async_graphql::MergedObject)]
#[graphql(name = "Mutation")]
pub struct MutationRoot(mutation::Mutation, ghcompat::GhMutation, projects::ProjectsMutation);

pub type ForgeSchema = Schema<query::Query, MutationRoot, EmptySubscription>;

/// The schema is immutable; build it once.
pub fn schema() -> &'static ForgeSchema {
    static SCHEMA: OnceLock<ForgeSchema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        Schema::build(query::Query, MutationRoot(mutation::Mutation, ghcompat::GhMutation, projects::ProjectsMutation), EmptySubscription)
            .validation_mode(ValidationMode::Fast)
            .limit_recursive_depth(64)
            // Bound query shape (aliases multiply work): gh's deepest queries (and
            // introspection) stay far below these.
            .limit_depth(32)
            .limit_complexity(5_000)
            .finish()
    })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/", post(handler))
}

/// Per-request context: who is asking.
#[derive(Clone)]
pub struct Viewer(pub User);

/// GitHub-style GraphQL error with a top-level `type`.
pub fn gql_err(kind: &'static str, msg: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(msg.into()).extend_with(|_, e| e.set("type", kind))
}

pub fn not_found(msg: impl Into<String>) -> async_graphql::Error {
    gql_err("NOT_FOUND", msg)
}

/// Converts REST-layer errors into GraphQL errors with GitHub's error types.
pub fn api_err(e: ApiError) -> async_graphql::Error {
    let kind = match e.status.as_u16() {
        404 => "NOT_FOUND",
        403 | 401 => "FORBIDDEN",
        422 | 409 | 400 => "UNPROCESSABLE",
        _ => "INTERNAL",
    };
    let mut msg = e.message.clone();
    if let Some(errs) = e.errors.as_ref().and_then(|v| v.as_array()) {
        let details: Vec<String> = errs
            .iter()
            .filter_map(|x| x.get("message").and_then(|m| m.as_str()).map(str::to_string))
            .collect();
        if !details.is_empty() {
            msg = details.join(", ");
        }
    }
    gql_err(kind, msg)
}

pub type GResult<T> = async_graphql::Result<T>;

/// Executes one GraphQL request body for `viewer` and returns GitHub-shaped JSON.
pub async fn execute(state: &AppState, viewer: User, body: &[u8]) -> Result<Value, ApiError> {
    let raw: Value = serde_json::from_slice(body).map_err(|_| ApiError::bad_request("Problems parsing JSON"))?;
    let mut req: async_graphql::Request =
        serde_json::from_value(raw).map_err(|_| ApiError::bad_request("Problems parsing JSON"))?;
    fill_absent_variables(&mut req);
    if let Some(errors) = ghcompat::undefined_mutation_fields(&req) {
        return Ok(errors);
    }
    let loader = DataLoader::new(Ld::new(state.clone(), Some(viewer.clone())), tokio::spawn);
    let req = req.data(state.clone()).data(Viewer(viewer)).data(loader);
    let resp = schema().execute(req).await;
    let mut v = serde_json::to_value(&resp).map_err(ApiError::internal)?;
    postprocess(&mut v);
    Ok(v)
}

/// GitHub treats declared-but-absent nullable variables as null; async-graphql would report
/// "Variable X is not defined". Insert explicit nulls (defaults still win when present).
fn fill_absent_variables(req: &mut async_graphql::Request) {
    let Ok(doc) = async_graphql::parser::parse_query(&req.query) else { return };
    let ops: Vec<_> = match &doc.operations {
        DocumentOperations::Single(op) => vec![op],
        DocumentOperations::Multiple(m) => match &req.operation_name {
            Some(n) => m.iter().filter(|(k, _)| k.as_str() == n).map(|(_, v)| v).collect(),
            None => m.values().collect(),
        },
    };
    for op in ops {
        for def in &op.node.variable_definitions {
            let name = def.node.name.node.clone();
            if !req.variables.contains_key(&name) && def.node.default_value.is_none() {
                req.variables.insert(name, async_graphql::Value::Null);
            }
        }
    }
}

/// Moves `extensions.type` to the top level of each error (GitHub's shape) and drops empty
/// `data` keys GitHub wouldn't send.
fn postprocess(v: &mut Value) {
    let Some(errors) = v.get_mut("errors").and_then(|e| e.as_array_mut()) else { return };
    for err in errors {
        let Some(obj) = err.as_object_mut() else { continue };
        let ty = obj
            .get_mut("extensions")
            .and_then(|x| x.as_object_mut())
            .and_then(|x| x.remove("type"));
        if obj.get("extensions").and_then(|x| x.as_object()).is_some_and(|x| x.is_empty()) {
            obj.remove("extensions");
        }
        if let Some(t) = ty {
            obj.insert("type".into(), t);
        }
    }
}

async fn handler(State(s): State<AppState>, MaybeUser(u): MaybeUser, body: Bytes) -> Response {
    let Some(user) = u else {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({
                "message": "This endpoint requires you to be authenticated.",
                "documentation_url": "https://docs.github.com/graphql/guides/forming-calls-with-graphql#authenticating-with-graphql"
            })),
        )
            .into_response();
    };
    match execute(&s, user, &body).await {
        Ok(v) => (StatusCode::OK, axum::Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}
