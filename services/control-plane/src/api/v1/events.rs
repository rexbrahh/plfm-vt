//! Events API endpoints.
//!
//! Provides org-scoped event querying for debugging and introspection.

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use plfm_id::OrgId;
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
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
            payload: Some(row.payload),
        });
    }

    let next_after_event_id = items.last().map(|e| e.event_id).unwrap_or(after_event_id);

    Ok(Json(EventsResponse {
        items,
        next_after_event_id,
    }))
}
