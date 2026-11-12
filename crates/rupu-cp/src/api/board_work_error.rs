use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rupu_fleet::work_queue::WorkError;
use serde_json::json;

pub(super) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub(super) struct Error {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub retry: &'static str,
    pub request_id: Option<String>,
}
impl Error {
    pub fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        retry: &'static str,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry,
            request_id: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "work_invalid_input",
            message,
            "none",
        )
    }
    pub fn owner_changed() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "work_owner_changed",
            "Board owner changed; refresh before writing",
            "refresh",
        )
    }
    pub fn cursor() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "work_cursor_changed",
            "Cursor is invalid or its source changed; reload without a cursor",
            "refresh",
        )
    }
    pub fn missing(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "work_not_found", message, "none")
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "work_unsupported",
            message,
            "none",
        )
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "work_persistence",
            message,
            "none",
        )
    }
    pub fn with_request(mut self, request_id: &str) -> Self {
        self.request_id = Some(request_id.to_owned());
        self
    }
    pub fn reconciliation(request_id: &str, message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "work_reconcile",
            message,
            "same_request",
        )
        .with_request(request_id)
    }
}
impl From<WorkError> for Error {
    fn from(error: WorkError) -> Self {
        use StatusCode as S;
        use WorkError as W;
        let status = match &error {
            W::InvalidInput(_) => S::BAD_REQUEST,
            W::Denied(_) => S::FORBIDDEN,
            W::NotFound { .. } => S::NOT_FOUND,
            W::OwnerChanged
            | W::ReadOnly(_)
            | W::RevisionConflict { .. }
            | W::AttemptConflict
            | W::IdempotencyConflict { .. }
            | W::TransitionConflict { .. }
            | W::CursorConflict
            | W::Fleet(rupu_fleet::FleetError::Refused(_)) => S::CONFLICT,
            W::Capacity { .. } => S::PAYLOAD_TOO_LARGE,
            W::Busy => S::SERVICE_UNAVAILABLE,
            W::UnsupportedSchema { .. } => S::NOT_IMPLEMENTED,
            W::Corrupt(_) | W::Persistence(_) | W::Reconciliation { .. } | W::Fleet(_) => {
                S::INTERNAL_SERVER_ERROR
            }
        };
        let code = error.code();
        let retry = error.retry();
        let request_id = error.request_id().map(str::to_owned);
        Self {
            status,
            code,
            retry,
            message: error.to_string(),
            request_id,
        }
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let mut body = json!({"code": self.code, "message": self.message, "retry": self.retry});
        if let Some(id) = self.request_id {
            body["request_id"] = json!(id);
        }
        (self.status, Json(json!({"error": body}))).into_response()
    }
}
