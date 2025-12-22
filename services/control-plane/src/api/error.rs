use axum::{
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    pub code: String,
    pub request_id: String,
    pub retryable: bool,
    pub retry_after_seconds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<FieldError>>,
}

#[derive(Debug, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl ProblemDetails {
    fn new(status: StatusCode, code: impl Into<String>, detail: impl Into<String>) -> Self {
        let code = code.into();
        let title = status
            .canonical_reason()
            .unwrap_or("Unknown Error")
            .to_string();
        Self {
            r#type: format!("https://plfm.dev/problems/{code}"),
            title,
            status: status.as_u16(),
            detail: detail.into(),
            instance: None,
            code,
            request_id: "unknown".to_string(),
            retryable: false,
            retry_after_seconds: 0,
            details: None,
        }
    }

    fn set_request_id(&mut self, request_id: impl Into<String>) {
        let request_id = request_id.into();
        self.request_id = request_id.clone();
        if self.instance.is_none() {
            self.instance = Some(request_id);
        }
    }

    fn set_retryable(&mut self, retryable: bool) {
        self.retryable = retryable;
    }

    fn set_retry_after_seconds(&mut self, seconds: u32) {
        self.retry_after_seconds = seconds;
        if seconds > 0 {
            self.retryable = true;
        }
    }

    fn set_details(&mut self, details: Vec<FieldError>) {
        self.details = Some(details);
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub problem: Box<ProblemDetails>,
}

impl ApiError {
    pub fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::BAD_REQUEST;
        let problem = Box::new(ProblemDetails::new(status, code, message));
        Self { status, problem }
    }

    pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::NOT_FOUND;
        let problem = Box::new(ProblemDetails::new(status, code, message));
        Self { status, problem }
    }

    pub fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::INTERNAL_SERVER_ERROR;
        let problem = Box::new(ProblemDetails::new(status, code, message));
        Self { status, problem }
    }

    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::CONFLICT;
        let problem = Box::new(ProblemDetails::new(status, code, message));
        Self { status, problem }
    }

    pub fn gateway_timeout(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::GATEWAY_TIMEOUT;
        let problem = Box::new(ProblemDetails::new(status, code, message));
        Self { status, problem }
    }

    pub fn unauthorized(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::UNAUTHORIZED;
        let problem = Box::new(ProblemDetails::new(status, code, message));
        Self { status, problem }
    }

    pub fn forbidden(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::FORBIDDEN;
        let problem = Box::new(ProblemDetails::new(status, code, message));
        Self { status, problem }
    }

    pub fn too_many_requests(code: impl Into<String>, message: impl Into<String>) -> Self {
        let status = StatusCode::TOO_MANY_REQUESTS;
        let mut problem = Box::new(ProblemDetails::new(status, code, message));
        problem.set_retryable(true);
        Self { status, problem }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.problem.set_request_id(request_id);
        self
    }

    pub fn with_details(mut self, details: Vec<FieldError>) -> Self {
        self.problem.set_details(details);
        self
    }

    pub fn with_retry_after_seconds(mut self, seconds: u32) -> Self {
        self.problem.set_retry_after_seconds(seconds);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.problem)).into_response();
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}
