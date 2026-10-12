//! GraphQL endpoint (/api/graphql): the subset of GitHub's schema used by gh.
//! Spec: docs/research/gh-protocol.md.
use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};

use crate::auth::MaybeUser;
use crate::db::User;
use crate::state::AppState;

pub struct Query;

#[Object]
impl Query {
    /// Placeholder until the GraphQL module is implemented.
    async fn viewer(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        let u = ctx.data_opt::<User>().ok_or("This endpoint requires you to be authenticated.")?;
        Ok(u.login.clone())
    }
}

pub type ForgeSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub fn routes() -> Router<AppState> {
    Router::new().route("/", post(handler))
}

async fn handler(State(s): State<AppState>, MaybeUser(u): MaybeUser, Json(req): Json<async_graphql::Request>) -> Response {
    let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
    let mut req = req.data(s);
    if let Some(u) = u {
        req = req.data(u);
    }
    Json(schema.execute(req).await).into_response()
}
