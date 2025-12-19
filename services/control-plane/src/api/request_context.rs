//! Request-scoped context extracted from HTTP requests.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use plfm_events::ActorType;
use plfm_id::RequestId;

use crate::api::error::ApiError;

pub const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub actor_type: ActorType,
    pub actor_id: String,
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for RequestContext
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let request_id = header_string(&parts.headers, "x-request-id")
            .unwrap_or_else(|| RequestId::new().to_string());

        let idempotency_key = header_string(&parts.headers, IDEMPOTENCY_KEY_HEADER);
        if let Some(key) = &idempotency_key {
            if !(8..=128).contains(&key.len()) {
                return Err(ApiError::bad_request(
                    "invalid_idempotency_key",
                    "Idempotency-Key must be between 8 and 128 characters",
                )
                .with_request_id(request_id));
            }
        }

        Ok(Self {
            request_id,
            idempotency_key,
            actor_type: ActorType::System,
            actor_id: "system".to_string(),
        })
    }
}
