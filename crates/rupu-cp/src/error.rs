use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);

impl ApiError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, msg.into())
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }

    /// 409 — the request conflicts with the run's current state (e.g.
    /// approving/rejecting a run that is no longer `awaiting_approval`).
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, msg.into())
    }

    /// 501 — this deployment cannot service the request because an optional
    /// adapter is not installed (e.g. launching runs from a read-only deploy
    /// with no `RunLauncher`).
    pub fn not_available(msg: impl Into<String>) -> Self {
        Self(StatusCode::NOT_IMPLEMENTED, msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": self.1 }));
        (self.0, body).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
