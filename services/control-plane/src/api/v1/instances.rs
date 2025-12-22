//! Instance API endpoints.
//!
//! Provides endpoints for instance status reporting and querying.
//! These are primarily used by node-agents to report status.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create instance routes.
///
/// Instance status is reported by node-agents.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_instances))
        .route("/{instance_id}", get(get_instance))
        .route("/{instance_id}/status", post(report_status))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to report instance status.
#[derive(Debug, Deserialize)]
pub struct ReportStatusRequest {
    /// Current status.
    pub status: String,

    /// Optional boot ID.
    #[serde(default)]
    pub boot_id: Option<String>,

    /// Optional error message.
    #[serde(default)]
    pub error_message: Option<String>,

    /// Optional exit code.
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// Response for status report.
#[derive(Debug, Serialize)]
pub struct ReportStatusResponse {
    /// Whether the status was accepted.
    pub accepted: bool,
}

/// Response for a single instance.
#[derive(Debug, Serialize)]
pub struct InstanceResponse {
    /// Instance ID.
    pub id: String,

    /// Organization ID.
    pub org_id: String,

    /// App ID.
    pub app_id: String,

    /// Env ID.
    pub env_id: String,

    /// Process type.
    pub process_type: String,

    /// Node ID where the instance is running.
    pub node_id: String,

    /// Desired state (running, draining, stopped).
    pub desired_state: String,

    /// Current status (booting, ready, draining, stopped, failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

    /// Release ID.
    pub release_id: String,

    /// Overlay IPv6 address (for ingress routing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_ipv6: Option<String>,

    /// When the instance was created.
    pub created_at: DateTime<Utc>,

    /// When the instance was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing instances.
#[derive(Debug, Serialize)]
pub struct ListInstancesResponse {
    /// List of instances.
    pub items: Vec<InstanceResponse>,

    /// Next cursor (null if no more results).
    pub next_cursor: Option<String>,
}

/// Query parameters for listing instances.
#[derive(Debug, Deserialize)]
pub struct ListInstancesQuery {
    /// Max number of items to return.
    pub limit: Option<i64>,
    /// Cursor (exclusive). Interpreted as an instance_id.
    pub cursor: Option<String>,
    /// Filter by env_id.
    pub env_id: Option<String>,
    /// Filter by node_id.
    pub node_id: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// List all instances (optionally filtered by env or node).
///
/// GET /v1/instances
async fn list_instances(
    State(state): State<AppState>,
    ctx: RequestContext,
    Query(query): Query<ListInstancesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    if ctx.actor_type != ActorType::System {
        return Err(ApiError::forbidden(
            "forbidden",
            "This endpoint is only available to system actors",
        )
        .with_request_id(request_id));
    }

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query.cursor;

    // Query instances from the desired view, joined with status view
    let rows = sqlx::query_as::<_, InstanceRow>(
        r#"
        SELECT
            d.instance_id, d.org_id, d.app_id, d.env_id, d.process_type,
            d.node_id, d.desired_state, d.release_id, d.overlay_ipv6,
            d.created_at, d.updated_at,
            s.status
        FROM instances_desired_view d
        LEFT JOIN instances_status_view s ON d.instance_id = s.instance_id
        WHERE d.desired_state != 'stopped'
          AND ($1::text IS NULL OR d.instance_id > $1)
          AND ($2::text IS NULL OR d.env_id = $2)
          AND ($3::text IS NULL OR d.node_id = $3)
        ORDER BY d.instance_id ASC
        LIMIT $4
        "#,
    )
    .bind(cursor.as_deref())
    .bind(query.env_id.as_deref())
    .bind(query.node_id.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list instances");
        ApiError::internal("internal_error", "Failed to list instances")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<InstanceResponse> = rows.into_iter().map(InstanceResponse::from).collect();
    let next_cursor = if items.len() == limit as usize {
        items.last().map(|item| item.id.clone())
    } else {
        None
    };

    Ok(Json(ListInstancesResponse { items, next_cursor }))
}

/// Get a single instance by ID.
///
/// GET /v1/instances/{instance_id}
async fn get_instance(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(instance_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    if ctx.actor_type != ActorType::System {
        return Err(ApiError::forbidden(
            "forbidden",
            "This endpoint is only available to system actors",
        )
        .with_request_id(request_id));
    }

    let row = sqlx::query_as::<_, InstanceRow>(
        r#"
        SELECT
            d.instance_id, d.org_id, d.app_id, d.env_id, d.process_type,
            d.node_id, d.desired_state, d.release_id, d.overlay_ipv6,
            d.created_at, d.updated_at,
            s.status
        FROM instances_desired_view d
        LEFT JOIN instances_status_view s ON d.instance_id = s.instance_id
        WHERE d.instance_id = $1
        "#,
    )
    .bind(&instance_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get instance");
        ApiError::internal("internal_error", "Failed to get instance")
            .with_request_id(request_id.clone())
    })?;

    match row {
        Some(row) => Ok(Json(InstanceResponse::from(row))),
        None => Err(ApiError::not_found(
            "instance_not_found",
            format!("Instance {} not found", instance_id),
        )
        .with_request_id(request_id)),
    }
}

/// Report instance status (called by node-agent).
///
/// POST /v1/instances/{instance_id}/status
async fn report_status(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(instance_id): Path<String>,
    Json(req): Json<ReportStatusRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    if ctx.actor_type != ActorType::System {
        return Err(ApiError::forbidden(
            "forbidden",
            "This endpoint is only available to system actors",
        )
        .with_request_id(request_id));
    }

    // Validate status
    let valid_statuses = ["booting", "ready", "draining", "stopped", "failed"];
    if !valid_statuses.contains(&req.status.as_str()) {
        return Err(ApiError::bad_request(
            "invalid_status",
            format!("Status must be one of: {:?}", valid_statuses),
        )
        .with_request_id(request_id.clone()));
    }

    // Get current status if exists
    let _current_status = sqlx::query_scalar::<_, Option<String>>(
        "SELECT status FROM instances_status_view WHERE instance_id = $1",
    )
    .bind(&instance_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to get current status");
        ApiError::internal("internal_error", "Failed to process status")
            .with_request_id(request_id.clone())
    })?
    .flatten();

    // Get instance details for the event
    let instance_info = sqlx::query_as::<_, InstanceInfoRow>(
        "SELECT org_id, app_id, env_id, node_id FROM instances_desired_view WHERE instance_id = $1",
    )
    .bind(&instance_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to get instance info");
        ApiError::internal("internal_error", "Failed to process status")
            .with_request_id(request_id.clone())
    })?;

    let instance_info = match instance_info {
        Some(info) => info,
        None => {
            return Err(ApiError::not_found(
                "instance_not_found",
                format!("Instance {} not found", instance_id),
            )
            .with_request_id(request_id.clone()));
        }
    };

    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::Instance, &instance_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to process status")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let org_id = instance_info.org_id.parse().map_err(|_| {
        ApiError::internal("internal_error", "Invalid org_id in instances_desired_view")
            .with_request_id(request_id.clone())
    })?;
    let app_id = instance_info.app_id.parse().map_err(|_| {
        ApiError::internal("internal_error", "Invalid app_id in instances_desired_view")
            .with_request_id(request_id.clone())
    })?;
    let env_id = instance_info.env_id.parse().map_err(|_| {
        ApiError::internal("internal_error", "Invalid env_id in instances_desired_view")
            .with_request_id(request_id.clone())
    })?;

    // Create the status changed event
    let event = AppendEvent {
        aggregate_type: AggregateType::Instance,
        aggregate_id: instance_id.clone(),
        aggregate_seq: current_seq + 1,
        event_type: "instance.status_changed".to_string(),
        event_version: 1,
        actor_type: ActorType::ServicePrincipal, // Node agent
        actor_id: "node-agent".to_string(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: None,
        app_id: Some(app_id),
        env_id: Some(env_id),
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "instance_id": instance_id,
            "node_id": instance_info.node_id,
            "status": req.status,
            "boot_id": req.boot_id,
            "exit_code": req.exit_code,
            "reason_code": if req.status == "failed" { req.error_message.as_ref().map(|_| "unknown_error") } else { None },
            "reason_detail": req.error_message,
            "reported_at": chrono::Utc::now().to_rfc3339(),
        }),
        ..Default::default()
    };

    // Append the event
    event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to record status");
        ApiError::internal("internal_error", "Failed to record status")
            .with_request_id(request_id.clone())
    })?;

    tracing::debug!(
        instance_id = %instance_id,
        status = %req.status,
        "Instance status reported"
    );

    Ok((
        StatusCode::OK,
        Json(ReportStatusResponse { accepted: true }),
    ))
}

// =============================================================================
// Database Row Types
// =============================================================================

struct InstanceRow {
    instance_id: String,
    org_id: String,
    app_id: String,
    env_id: String,
    process_type: String,
    node_id: String,
    desired_state: String,
    release_id: String,
    overlay_ipv6: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            instance_id: row.try_get("instance_id")?,
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            process_type: row.try_get("process_type")?,
            node_id: row.try_get("node_id")?,
            desired_state: row.try_get("desired_state")?,
            release_id: row.try_get("release_id")?,
            overlay_ipv6: row.try_get("overlay_ipv6")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            status: row.try_get("status")?,
        })
    }
}

impl From<InstanceRow> for InstanceResponse {
    fn from(row: InstanceRow) -> Self {
        Self {
            id: row.instance_id,
            org_id: row.org_id,
            app_id: row.app_id,
            env_id: row.env_id,
            process_type: row.process_type,
            node_id: row.node_id,
            desired_state: row.desired_state,
            status: row.status,
            release_id: row.release_id,
            overlay_ipv6: row.overlay_ipv6,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

struct InstanceInfoRow {
    org_id: String,
    app_id: String,
    env_id: String,
    node_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceInfoRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            node_id: row.try_get("node_id")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_status_request_deserialization() {
        let json = r#"{"status": "ready", "boot_id": "boot_123"}"#;
        let req: ReportStatusRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.status, "ready");
        assert_eq!(req.boot_id, Some("boot_123".to_string()));
    }

    #[test]
    fn test_report_status_response_serialization() {
        let response = ReportStatusResponse { accepted: true };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"accepted\":true"));
    }
}
