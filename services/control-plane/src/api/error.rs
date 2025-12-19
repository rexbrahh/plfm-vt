//! Common API error types and responses.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;

/// Standard error response for API errors.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// Error code (machine-readable).
    pub code: String,

    /// Human-readable error message.
    pub message: String,

    /// Request ID for correlation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// Field-level validation errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<FieldError>>,
}

/// Field-level validation error.
#[derive(Debug, Serialize)]
pub struct FieldError {
    /// Field name.
    pub field: String,

    /// Error message for this field.
    pub message: String,
}

impl ErrorResponse {
    /// Create a new error response.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            request_id: None,
            details: None,
        }
    }

    /// Add a request ID.
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

/// API error type that can be converted to a response.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub response: ErrorResponse,
}

impl ApiError {
    pub fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            response: ErrorResponse::new(code, message),
        }
    }

    pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            response: ErrorResponse::new(code, message),
        }
    }

    pub fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            response: ErrorResponse::new(code, message),
        }
    }

    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            response: ErrorResponse::new(code, message),
        }
    }

    pub fn gateway_timeout(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            response: ErrorResponse::new(code, message),
        }
    }

    pub fn unauthorized(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            response: ErrorResponse::new(code, message),
        }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.response = self.response.with_request_id(request_id);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.response)).into_response()
    }
}
