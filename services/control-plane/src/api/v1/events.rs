//! Events API endpoints.
//!
//! Provides org-scoped event querying for debugging and introspection.

use std::{collections::VecDeque, convert::Infallible, sync::OnceLock, time::Duration};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header::CONTENT_TYPE, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::stream::unfold;
use plfm_id::OrgId;
use plfm_proto::FILE_DESCRIPTOR_SET;
use prost_reflect::{DescriptorPool, DynamicMessage};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::api::authz;

const STREAM_BATCH_LIMIT: i64 = 200;
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(500);

use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::db::EventRow;
use crate::state::AppState;

/// Query parameters for listing events.
#[derive(Debug, Deserialize)]
pub struct ListEventsQuery {
    /// Return events with event_id > after_event_id.
    pub after_event_id: Option<i64>,
    /// Max number of events to return.
    pub limit: Option<i64>,
    /// Filter by exact event type.
    pub event_type: Option<String>,
    /// Filter by app_id.
    pub app_id: Option<String>,
    /// Filter by env_id.
    pub env_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StreamEventsQuery {
    pub after_event_id: Option<i64>,
    pub limit: Option<i64>,
    pub event_type: Option<String>,
    pub app_id: Option<String>,
    pub env_id: Option<String>,
    pub poll_ms: Option<u64>,
}

/// Response event shape (subset + payload).
#[derive(Debug, Serialize)]
pub struct EventResponse {
    pub event_id: i64,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub event_version: i32,
    pub actor_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_seq: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// Response for listing events.
#[derive(Debug, Serialize)]
pub struct EventsResponse {
    pub items: Vec<EventResponse>,
    pub next_after_event_id: i64,
}

#[derive(Debug, Serialize)]
struct EventStreamLine {
    pub ts: DateTime<Utc>,
    pub seq: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

struct EventStreamState {
    state: AppState,
    org_id: OrgId,
    org_id_str: String,
    event_type: Option<String>,
    app_id: Option<String>,
    env_id: Option<String>,
    limit: i64,
    poll_interval: Duration,
    last_id: i64,
    buffer: VecDeque<EventRow>,
}

/// Query or tail org-scoped events (debugging).
///
/// GET /v1/orgs/{org_id}/events
pub async fn list_events(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let after_event_id = query.after_event_id.unwrap_or(0).max(0);
    let limit: i32 = query.limit.unwrap_or(50).clamp(1, 200) as i32;

    let event_store = state.db().event_store();
    let org_id_str = org_id.to_string();
    let mut rows = if let Some(event_type) = query.event_type.as_deref() {
        let fetch_limit = limit.saturating_mul(10).clamp(1, 2000);
        event_store
            .query_by_type_after_cursor(event_type, after_event_id, fetch_limit)
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    request_id = %request_id,
                    org_id = %org_id,
                    event_type = %event_type,
                    "Failed to query events"
                );
                ApiError::internal("internal_error", "Failed to query events")
                    .with_request_id(request_id.clone())
            })?
            .into_iter()
            .filter(|row| row.org_id.as_deref() == Some(org_id_str.as_str()))
            .take(limit as usize)
            .collect::<Vec<_>>()
    } else {
        event_store
            .query_by_org_after_cursor(&org_id, after_event_id, limit)
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    request_id = %request_id,
                    org_id = %org_id,
                    "Failed to query events"
                );
                ApiError::internal("internal_error", "Failed to query events")
                    .with_request_id(request_id.clone())
            })?
    };

    if let Some(app_id) = query.app_id.as_deref() {
        rows.retain(|row| row.app_id.as_deref() == Some(app_id));
    }
    if let Some(env_id) = query.env_id.as_deref() {
        rows.retain(|row| row.env_id.as_deref() == Some(env_id));
    }

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let payload = event_payload_json(&row);
        items.push(EventResponse {
            event_id: row.event_id,
            occurred_at: row.occurred_at,
            event_type: row.event_type,
            event_version: row.event_version,
            actor_type: row.actor_type,
            aggregate_type: Some(row.aggregate_type),
            aggregate_id: Some(row.aggregate_id),
            aggregate_seq: Some(row.aggregate_seq),
            actor_id: Some(row.actor_id),
            request_id: row.request_id,
            idempotency_key: row.idempotency_key,
            correlation_id: row.correlation_id,
            causation_id: row.causation_id,
            payload,
        });
    }

    let next_after_event_id = items.last().map(|e| e.event_id).unwrap_or(after_event_id);

    Ok(Json(EventsResponse {
        items,
        next_after_event_id,
    }))
}

pub async fn stream_events(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Query(query): Query<StreamEventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let after_event_id = query.after_event_id.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(STREAM_BATCH_LIMIT)
        .clamp(1, STREAM_BATCH_LIMIT);
    let poll_ms = query
        .poll_ms
        .unwrap_or(STREAM_POLL_INTERVAL.as_millis() as u64)
        .max(100);
    let poll_interval = Duration::from_millis(poll_ms);

    let stream_state = EventStreamState {
        state: state.clone(),
        org_id,
        org_id_str: org_id.to_string(),
        event_type: query.event_type.clone(),
        app_id: query.app_id.clone(),
        env_id: query.env_id.clone(),
        limit,
        poll_interval,
        last_id: after_event_id,
        buffer: VecDeque::new(),
    };

    let stream = unfold(stream_state, move |mut st| {
        let request_id = request_id.clone();
        async move {
            loop {
                if let Some(row) = st.buffer.pop_front() {
                    let payload = event_payload_json(&row);
                    let line = EventStreamLine {
                        ts: row.occurred_at,
                        seq: row.event_id,
                        event_type: row.event_type,
                        aggregate_type: Some(row.aggregate_type),
                        aggregate_id: Some(row.aggregate_id),
                        app_id: row.app_id,
                        env_id: row.env_id,
                        payload,
                    };

                    let data = match serde_json::to_string(&line) {
                        Ok(data) => data,
                        Err(e) => {
                            tracing::error!(error = ?e, "Failed to serialize event stream line");
                            continue;
                        }
                    };

                    let payload = Bytes::from(format!("{data}\n"));
                    return Some((Ok::<Bytes, Infallible>(payload), st));
                }

                let event_store = st.state.db().event_store();
                let rows = if let Some(event_type) = st.event_type.as_deref() {
                    let fetch_limit = (st.limit.saturating_mul(10)).clamp(1, 2000) as i32;
                    event_store
                        .query_by_type_after_cursor(event_type, st.last_id, fetch_limit)
                        .await
                } else {
                    event_store
                        .query_by_org_after_cursor(&st.org_id, st.last_id, st.limit as i32)
                        .await
                };

                match rows {
                    Ok(rows) => {
                        if rows.is_empty() {
                            sleep(st.poll_interval).await;
                            continue;
                        }

                        if let Some(last) = rows.last() {
                            st.last_id = last.event_id;
                        }

                        let mut filtered = rows;
                        if st.event_type.is_some() {
                            filtered.retain(|row| {
                                row.org_id.as_deref() == Some(st.org_id_str.as_str())
                            });
                        }
                        if let Some(app_id) = st.app_id.as_deref() {
                            filtered.retain(|row| row.app_id.as_deref() == Some(app_id));
                        }
                        if let Some(env_id) = st.env_id.as_deref() {
                            filtered.retain(|row| row.env_id.as_deref() == Some(env_id));
                        }

                        if filtered.is_empty() {
                            continue;
                        }

                        st.buffer = VecDeque::from(filtered);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, request_id = %request_id, "Failed to stream events");
                        sleep(st.poll_interval).await;
                    }
                }
            }
        }
    });

    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    Ok(response)
}

fn event_payload_json(row: &EventRow) -> Option<serde_json::Value> {
    if let (Some(type_url), Some(payload_bytes)) = (
        row.payload_type_url.as_deref(),
        row.payload_bytes.as_deref(),
    ) {
        if let Some(value) = decode_protobuf_payload(type_url, payload_bytes) {
            return Some(value);
        }
    }

    if row.payload.is_null() {
        None
    } else {
        Some(to_proto_json(row.payload.clone()))
    }
}

fn decode_protobuf_payload(type_url: &str, payload_bytes: &[u8]) -> Option<serde_json::Value> {
    let pool = descriptor_pool()?;
    let message_name = type_url.rsplit('/').next().unwrap_or(type_url);
    let descriptor = pool.get_message_by_name(message_name)?;

    let dynamic = match DynamicMessage::decode(descriptor, payload_bytes) {
        Ok(message) => message,
        Err(err) => {
            tracing::warn!(
                error = ?err,
                payload_type_url = %type_url,
                "Failed to decode protobuf payload"
            );
            return None;
        }
    };

    match serde_json::to_value(dynamic) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(
                error = ?err,
                payload_type_url = %type_url,
                "Failed to serialize protobuf payload to JSON"
            );
            None
        }
    }
}

fn descriptor_pool() -> Option<&'static DescriptorPool> {
    static DESCRIPTORS: OnceLock<Option<DescriptorPool>> = OnceLock::new();
    DESCRIPTORS
        .get_or_init(|| match DescriptorPool::decode(FILE_DESCRIPTOR_SET) {
            Ok(pool) => Some(pool),
            Err(err) => {
                tracing::error!(error = ?err, "Failed to decode protobuf descriptor set");
                None
            }
        })
        .as_ref()
}

fn to_proto_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(to_proto_json).collect())
        }
        serde_json::Value::Object(entries) => {
            let mut mapped = serde_json::Map::new();
            for (key, value) in entries {
                mapped.insert(snake_to_lower_camel(&key), to_proto_json(value));
            }
            serde_json::Value::Object(mapped)
        }
        other => other,
    }
}

fn snake_to_lower_camel(input: &str) -> String {
    let mut parts = input.split('_');
    let Some(first) = parts.next() else {
        return String::new();
    };
    let mut out = String::from(first);
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first_char) = chars.next() {
            out.push(first_char.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use plfm_proto::events::v1::OrgCreatedPayload;
    use prost::Message;

    #[test]
    fn snake_to_lower_camel_converts() {
        assert_eq!(snake_to_lower_camel("app_id"), "appId");
        assert_eq!(snake_to_lower_camel("env"), "env");
        assert_eq!(snake_to_lower_camel(""), "");
    }

    #[test]
    fn to_proto_json_maps_nested_keys() {
        let value = serde_json::json!({
            "app_id": "app_123",
            "env_id": "env_456",
            "nested": { "deploy_id": "dep_1" },
            "items": [
                { "event_id": 1 },
                { "event_id": 2 }
            ]
        });
        let mapped = to_proto_json(value);
        let expected = serde_json::json!({
            "appId": "app_123",
            "envId": "env_456",
            "nested": { "deployId": "dep_1" },
            "items": [
                { "eventId": 1 },
                { "eventId": 2 }
            ]
        });
        assert_eq!(mapped, expected);
    }

    #[test]
    fn decode_protobuf_payload_maps_fields() {
        let payload = OrgCreatedPayload {
            org_id: "org_123".to_string(),
            name: "Acme".to_string(),
        };
        let bytes = payload.encode_to_vec();
        let json = decode_protobuf_payload(
            "type.googleapis.com/plfm.events.v1.OrgCreatedPayload",
            &bytes,
        )
        .expect("decoded payload");
        let expected = serde_json::json!({
            "orgId": "org_123",
            "name": "Acme"
        });
        assert_eq!(json, expected);
    }
}
