use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// API error carrying the wire contract's `{"error": "...", "code": "..."}` envelope.
///
/// The `code` strings are part of the client contract (the Go reference sends
/// `AUTH`/`VALIDATION`/`LOCKED`/`QUOTA_EXCEEDED`/`SERVER`); never rename them.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing token")]
    MissingToken,
    #[error("invalid token")]
    InvalidToken,
    #[error("forbidden")]
    Forbidden,
    #[error("bad request")]
    Validation,
    #[error("too many failures, device locked")]
    Locked { retry_after_secs: u64 },
    #[error("storage quota exceeded")]
    QuotaExceeded,
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn server<E: Into<anyhow::Error>>(e: E) -> Self {
        ApiError::Server(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            ApiError::MissingToken => (StatusCode::UNAUTHORIZED, "AUTH", self.to_string()),
            ApiError::InvalidToken => (StatusCode::UNAUTHORIZED, "AUTH", self.to_string()),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "AUTH", self.to_string()),
            ApiError::Validation => (StatusCode::BAD_REQUEST, "VALIDATION", self.to_string()),
            ApiError::Locked { retry_after_secs: _ } => (
                StatusCode::TOO_MANY_REQUESTS,
                "LOCKED",
                "too many failures, device locked".into(),
            ),
            ApiError::QuotaExceeded => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "QUOTA_EXCEEDED",
                self.to_string(),
            ),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "SERVER", "not found".into()),
            ApiError::Server(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "SERVER",
                "internal server error".into(),
            ),
        };
        let mut resp = (status, axum::Json(json!({ "error": message, "code": code }))).into_response();
        if let ApiError::Locked { retry_after_secs } = self {
            resp.headers_mut().insert(
                "Retry-After",
                retry_after_secs.max(1).to_string().parse().unwrap(),
            );
        }
        resp
    }
}
