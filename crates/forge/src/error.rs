use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

pub const DOCS_URL: &str = "https://docs.github.com/rest";

/// An error rendered in GitHub's REST error shape.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    pub errors: Option<Value>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiError { status, message: message.into(), errors: None }
    }
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not Found")
    }
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Requires authentication")
    }
    pub fn bad_credentials() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Bad credentials")
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, msg)
    }
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, msg)
    }
    /// 422 with a single validation error entry.
    pub fn validation(resource: &str, field: &str, code: &str) -> Self {
        ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: "Validation Failed".into(),
            errors: Some(json!([{ "resource": resource, "field": field, "code": code }])),
        }
    }
    /// 422 with a custom message (GitHub uses this for e.g. "Reference already exists").
    pub fn unprocessable(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, msg)
    }
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(error = %err, "internal error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "message": self.message, "documentation_url": DOCS_URL });
        if let Some(errors) = self.errors {
            body["errors"] = errors;
        }
        body["status"] = json!(self.status.as_u16().to_string());
        (self.status, axum::Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => ApiError::not_found(),
            other => ApiError::internal(other),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::internal(format!("{e:#}"))
    }
}

impl From<object_store::Error> for ApiError {
    fn from(e: object_store::Error) -> Self {
        ApiError::internal(e)
    }
}
