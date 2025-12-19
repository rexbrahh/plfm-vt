//! Environment-scoped instances API endpoints.
//!
//! Tenant-facing instance listing and inspection lives under an org/app/env path:
//! /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_id::{AppId, EnvId, InstanceId, OrgId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_instances))
        .route("/:instance_id", get(get_instance))
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListInstancesQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub process_type: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InstanceResponse {
    pub id: String,
    pub env_id: String,
    pub process_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<i32>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ListInstancesResponse {
    pub items: Vec<InstanceResponse>,
    pub next_cursor: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

async fn list_instances(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Query(query): Query<ListInstancesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id_typed: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let _: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id_typed, &ctx).await?;

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match query.cursor.as_deref() {
        Some(raw) => {
            let _: InstanceId = raw.parse().map_err(|_| {
                ApiError::bad_request("invalid_cursor", "Invalid cursor format")
                    .with_request_id(request_id.clone())
            })?;
            Some(raw.to_string())
        }
        None => None,
    };

    if let Some(status) = query.status.as_deref() {
        match status {
            "booting" | "ready" | "draining" | "stopped" | "failed" => {}
            _ => {
                return Err(
                    ApiError::bad_request("invalid_status", "Invalid status filter")
                        .with_request_id(request_id),
                );
            }
        }
    }

    let rows = sqlx::query_as::<_, InstanceRow>(
        r#"
        SELECT
            d.instance_id,
            d.env_id,
            d.process_type,
            d.node_id,
            d.generation,
            d.desired_state,
            d.created_at,
            d.updated_at,
            s.status as reported_status,
            s.reported_at,
            s.reason_code
        FROM instances_desired_view d
        LEFT JOIN instances_status_view s ON d.instance_id = s.instance_id
        WHERE d.org_id = $1
          AND d.app_id = $2
          AND d.env_id = $3
          AND ($4::TEXT IS NULL OR d.instance_id > $4)
          AND ($5::TEXT IS NULL OR d.process_type = $5)
          AND (
            $6::TEXT IS NULL OR (
                CASE
                    WHEN d.desired_state = 'stopped' THEN 'stopped'
                    WHEN d.desired_state = 'draining' THEN 'draining'
                    WHEN s.status IS NOT NULL THEN s.status
                    ELSE 'booting'
                END
            ) = $6
          )
        ORDER BY d.instance_id ASC
        LIMIT $7
        "#,
    )
    .bind(&org_id)
    .bind(&app_id)
    .bind(&env_id)
    .bind(cursor.as_deref())
    .bind(query.process_type.as_deref())
    .bind(query.status.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id_typed,
            app_id = %app_id,
            env_id = %env_id,
            "Failed to list instances"
        );
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

async fn get_instance(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id, instance_id)): Path<(String, String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id_typed: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let _: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let instance_id_typed: InstanceId = instance_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_instance_id", "Invalid instance ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id_typed, &ctx).await?;

    let row = sqlx::query_as::<_, InstanceRow>(
        r#"
        SELECT
            d.instance_id,
            d.env_id,
            d.process_type,
            d.node_id,
            d.generation,
            d.desired_state,
            d.created_at,
            d.updated_at,
            s.status as reported_status,
            s.reported_at,
            s.reason_code
        FROM instances_desired_view d
        LEFT JOIN instances_status_view s ON d.instance_id = s.instance_id
        WHERE d.instance_id = $1
          AND d.org_id = $2
          AND d.app_id = $3
          AND d.env_id = $4
        "#,
    )
    .bind(instance_id_typed.to_string())
    .bind(&org_id)
    .bind(&app_id)
    .bind(&env_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            instance_id = %instance_id_typed,
            "Failed to get instance"
        );
        ApiError::internal("internal_error", "Failed to get instance")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(
            ApiError::not_found("instance_not_found", "Instance not found")
                .with_request_id(request_id),
        );
    };

    Ok(Json(InstanceResponse::from(row)))
}

// =============================================================================
// Database Row Types
// =============================================================================

#[derive(Debug, Clone)]
struct InstanceRow {
    instance_id: String,
    env_id: String,
    process_type: String,
    node_id: String,
    generation: i32,
    desired_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    reported_status: Option<String>,
    reported_at: Option<DateTime<Utc>>,
    reason_code: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            instance_id: row.try_get("instance_id")?,
            env_id: row.try_get("env_id")?,
            process_type: row.try_get("process_type")?,
            node_id: row.try_get("node_id")?,
            generation: row.try_get("generation")?,
            desired_state: row.try_get("desired_state")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            reported_status: row.try_get("reported_status")?,
            reported_at: row.try_get("reported_at")?,
            reason_code: row.try_get("reason_code")?,
        })
    }
}

impl From<InstanceRow> for InstanceResponse {
    fn from(row: InstanceRow) -> Self {
        let status = match row.desired_state.as_str() {
            "stopped" => "stopped".to_string(),
            "draining" => "draining".to_string(),
            _ => row.reported_status.unwrap_or_else(|| "booting".to_string()),
        };

        let failure_reason = if status == "failed" {
            row.reason_code
        } else {
            None
        };

        let last_transition_at = row.reported_at.or(Some(row.updated_at));

        let node_id = match row.desired_state.as_str() {
            "stopped" => None,
            _ => Some(row.node_id),
        };

        let generation = match row.desired_state.as_str() {
            "stopped" => None,
            _ => Some(row.generation),
        };

        Self {
            id: row.instance_id,
            env_id: row.env_id,
            process_type: row.process_type,
            node_id,
            generation,
            status,
            last_transition_at,
            failure_reason,
            created_at: row.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_response_serialization() {
        let resp = InstanceResponse {
            id: "inst_123".to_string(),
            env_id: "env_123".to_string(),
            process_type: "web".to_string(),
            node_id: Some("node_1".to_string()),
            generation: Some(1),
            status: "booting".to_string(),
            last_transition_at: None,
            failure_reason: None,
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"id\":\"inst_123\""));
    }

    #[test]
    fn test_instance_row_status_mapping() {
        let now = Utc::now();

        let base = InstanceRow {
            instance_id: "inst_1".to_string(),
            env_id: "env_1".to_string(),
            process_type: "web".to_string(),
            node_id: "node_1".to_string(),
            generation: 1,
            desired_state: "running".to_string(),
            created_at: now,
            updated_at: now,
            reported_status: Some("ready".to_string()),
            reported_at: Some(now),
            reason_code: None,
        };

        let ready = InstanceResponse::from(base.clone());
        assert_eq!(ready.status, "ready");
        assert!(ready.failure_reason.is_none());

        let draining = InstanceResponse::from(InstanceRow {
            desired_state: "draining".to_string(),
            reported_status: Some("ready".to_string()),
            ..InstanceRow {
                instance_id: "inst_2".to_string(),
                ..base.clone()
            }
        });
        assert_eq!(draining.status, "draining");

        let booting = InstanceResponse::from(InstanceRow {
            instance_id: "inst_3".to_string(),
            reported_status: None,
            ..base.clone()
        });
        assert_eq!(booting.status, "booting");

        let failed = InstanceResponse::from(InstanceRow {
            instance_id: "inst_4".to_string(),
            reported_status: Some("failed".to_string()),
            reason_code: Some("crash_loop_backoff".to_string()),
            ..base.clone()
        });
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.failure_reason.as_deref(), Some("crash_loop_backoff"));

        let stopped = InstanceResponse::from(InstanceRow {
            instance_id: "inst_5".to_string(),
            desired_state: "stopped".to_string(),
            ..base
        });
        assert_eq!(stopped.status, "stopped");
        assert!(stopped.node_id.is_none());
        assert!(stopped.generation.is_none());
    }
}
