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
use plfm_id::RequestId;
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_seq: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
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
    Path(org_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.to_string())
    })?;

    let after_event_id = query.after_event_id.unwrap_or(0).max(0);
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as i64;

    let rows = sqlx::query_as::<_, crate::db::EventRow>(
        r#"
        SELECT
            event_id,
            occurred_at,
            aggregate_type,
            aggregate_id,
            aggregate_seq,
            event_type,
            event_version,
            actor_type,
            actor_id,
            org_id,
            request_id,
            idempotency_key,
            app_id,
            env_id,
            correlation_id,
            causation_id,
            payload
        FROM events
        WHERE org_id = $1
          AND event_id > $2
          AND ($3::TEXT IS NULL OR event_type = $3)
          AND ($4::TEXT IS NULL OR app_id = $4)
          AND ($5::TEXT IS NULL OR env_id = $5)
        ORDER BY event_id ASC
        LIMIT $6
        "#,
    )
    .bind(org_id.to_string())
    .bind(after_event_id)
    .bind(query.event_type.as_deref())
    .bind(query.app_id.as_deref())
    .bind(query.env_id.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id,
            "Failed to query events"
        );
        ApiError::internal("internal_error", "Failed to query events")
            .with_request_id(request_id.to_string())
    })?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(EventResponse {
            event_id: row.event_id,
            occurred_at: row.occurred_at,
            event_type: row.event_type,
            aggregate_type: Some(row.aggregate_type),
            aggregate_id: Some(row.aggregate_id),
            aggregate_seq: Some(row.aggregate_seq),
            actor_id: Some(row.actor_id),
            payload: Some(row.payload),
        });
    }

    let next_after_event_id = items.last().map(|e| e.event_id).unwrap_or(after_event_id);

    Ok(Json(EventsResponse {
        items,
        next_after_event_id,
    }))
}
