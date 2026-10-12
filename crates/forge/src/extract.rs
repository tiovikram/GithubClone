//! A `Json` extractor/response that behaves like GitHub: the request body is parsed as JSON
//! regardless of Content-Type, an empty body counts as `{}`, and errors use GitHub's shape.
use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::ApiError;

pub struct Json<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = ApiError;
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, e.body_text()))?;
        let slice: &[u8] = if bytes.iter().all(|b| b.is_ascii_whitespace()) { b"{}" } else { &bytes };
        serde_json::from_slice(slice).map(Json).map_err(|e| {
            let msg = e.to_string();
            // Type/shape errors on valid JSON are validation failures (422) on GitHub.
            if e.is_data() {
                let mut err = ApiError::unprocessable("Invalid request.");
                err.message = format!("Invalid request.\n\n{msg}");
                err
            } else {
                ApiError::bad_request("Problems parsing JSON")
            }
        })
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
