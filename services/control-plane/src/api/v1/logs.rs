//! Logs API endpoints.
//!
//! For Developer Preview, this provides a stable API surface and a placeholder
//! streaming transport. Full log aggregation is implemented in the
//! observability stack (future work).

use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use chrono::{DateTime, Utc};
use futures_core::Stream;
use plfm_id::{AppId, EnvId, OrgId};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::state::AppState;

/// Query parameters for log queries.
#[derive(Debug, Deserialize)]
pub struct QueryLogsParams {
    pub process_type: Option<String>,
    pub instance_id: Option<String>,
    /// RFC3339 timestamp (inclusive).
    pub since: Option<String>,
    /// RFC3339 timestamp (inclusive).
    pub until: Option<String>,
    pub tail_lines: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct LogLine {
    pub ts: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_type: Option<String>,
    pub line: String,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub items: Vec<LogLine>,
}

/// Query logs (bounded window).
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/logs
pub async fn query_logs(
    State(_state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Query(query): Query<QueryLogsParams>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;
    let env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    if let Some(since) = query.since.as_deref() {
        DateTime::parse_from_rfc3339(since).map_err(|_| {
            ApiError::bad_request(
                "invalid_since",
                "Invalid 'since' timestamp (expected RFC3339)",
            )
            .with_request_id(request_id.clone())
        })?;
    }

    if let Some(until) = query.until.as_deref() {
        DateTime::parse_from_rfc3339(until).map_err(|_| {
            ApiError::bad_request(
                "invalid_until",
                "Invalid 'until' timestamp (expected RFC3339)",
            )
            .with_request_id(request_id.clone())
        })?;
    }

    let _tail_lines = query.tail_lines.unwrap_or(200).clamp(1, 10_000);

    tracing::debug!(
        request_id = %request_id,
        org_id = %org_id,
        app_id = %app_id,
        env_id = %env_id,
        process_type = ?query.process_type,
        instance_id = ?query.instance_id,
        "Log query requested (not yet implemented)"
    );

    Ok(Json(LogsResponse { items: Vec::new() }))
}

/// Stream logs (server-sent events).
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/logs/stream
pub async fn stream_logs(
    State(_state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Query(query): Query<QueryLogsParams>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;
    let env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    tracing::info!(
        request_id = %request_id,
        org_id = %org_id,
        app_id = %app_id,
        env_id = %env_id,
        process_type = ?query.process_type,
        instance_id = ?query.instance_id,
        "Log stream opened (placeholder)"
    );

    let placeholder = LogLine {
        ts: Utc::now(),
        instance_id: query.instance_id.clone(),
        process_type: query.process_type.clone(),
        line: "Log streaming is not yet wired; this is a placeholder stream.".to_string(),
    };

    let data = serde_json::to_string(&placeholder).map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id,
            app_id = %app_id,
            env_id = %env_id,
            "Failed to serialize log line"
        );
        ApiError::internal("internal_error", "Failed to stream logs")
            .with_request_id(request_id.clone())
    })?;

    let stream = OneEventThenPending::new(Event::default().event("log").data(data));

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

struct OneEventThenPending {
    item: Option<Result<Event, Infallible>>,
}

impl OneEventThenPending {
    fn new(event: Event) -> Self {
        Self {
            item: Some(Ok(event)),
        }
    }
}

impl Stream for OneEventThenPending {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(item) = self.item.take() {
            return Poll::Ready(Some(item));
        }

        Poll::Pending
    }
}
