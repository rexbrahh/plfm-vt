//! Idempotency helpers for retry-safe write endpoints.

use axum::http::StatusCode;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api::error::ApiError;
use crate::db::{IdempotencyCheck, StoreIdempotencyRecord};
use crate::state::AppState;

pub const IDEMPOTENCY_SCOPE_GLOBAL: &str = "_global";

fn canonicalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();

            let mut ordered = serde_json::Map::new();
            for key in keys {
                if let Some(mut value) = map.remove(&key) {
                    canonicalize_json(&mut value);
                    ordered.insert(key, value);
                }
            }

            *map = ordered;
        }
        serde_json::Value::Array(items) => {
            for item in items {
                canonicalize_json(item);
            }
        }
        _ => {}
    }
}

pub fn request_hash(endpoint_name: &str, request: &impl Serialize) -> Result<String, ApiError> {
    let mut value = serde_json::to_value(request).map_err(|e| {
        ApiError::internal(
            "internal_error",
            format!("Failed to serialize request body: {}", e),
        )
    })?;

    canonicalize_json(&mut value);
    let canonical = serde_json::to_string(&value).map_err(|e| {
        ApiError::internal(
            "internal_error",
            format!("Failed to serialize canonical request body: {}", e),
        )
    })?;

    let mut hasher = Sha256::new();
    hasher.update(endpoint_name.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

pub async fn check(
    state: &AppState,
    org_scope: &str,
    actor_id: &str,
    endpoint_name: &str,
    idempotency_key: &str,
    request_hash: &str,
    request_id: &str,
) -> Result<Option<(StatusCode, Option<serde_json::Value>)>, ApiError> {
    let store = state.db().idempotency_store();
    let check = store
        .check(
            org_scope,
            actor_id,
            endpoint_name,
            idempotency_key,
            request_hash,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                request_id = %request_id,
                "Failed to check idempotency record"
            );
            ApiError::internal("internal_error", "Failed to process request")
                .with_request_id(request_id.to_string())
        })?;

    match check {
        IdempotencyCheck::NotFound => Ok(None),
        IdempotencyCheck::Found(record) => {
            let status =
                StatusCode::from_u16(record.response_status_code as u16).unwrap_or(StatusCode::OK);
            Ok(Some((status, record.response_body)))
        }
        IdempotencyCheck::Conflict => Err(ApiError::conflict(
            "idempotency_key_conflict",
            "Idempotency-Key was already used with a different request",
        )
        .with_request_id(request_id.to_string())),
    }
}

pub async fn store(
    state: &AppState,
    params: StoreIdempotencyParams<'_>,
    request_id: &str,
) -> Result<(), ApiError> {
    let store = state.db().idempotency_store();
    store
        .store(StoreIdempotencyRecord {
            org_id: params.org_scope.to_string(),
            actor_id: params.actor_id.to_string(),
            endpoint_name: params.endpoint_name.to_string(),
            idempotency_key: params.idempotency_key.to_string(),
            request_hash: params.request_hash.to_string(),
            response_status_code: params.status.as_u16() as i32,
            response_body: params.body,
        })
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                request_id = %request_id,
                "Failed to store idempotency record"
            );
            ApiError::internal("internal_error", "Failed to process request")
                .with_request_id(request_id.to_string())
        })
}

pub struct StoreIdempotencyParams<'a> {
    pub org_scope: &'a str,
    pub actor_id: &'a str,
    pub endpoint_name: &'a str,
    pub idempotency_key: &'a str,
    pub request_hash: &'a str,
    pub status: StatusCode,
    pub body: Option<serde_json::Value>,
}
