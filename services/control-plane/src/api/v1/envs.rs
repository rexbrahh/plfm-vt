//! Environment API endpoints.
//!
//! Provides CRUD operations for environments within applications.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType};
use plfm_id::{AppId, EnvId, OrgId, RequestId};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create env routes.
///
/// Envs are nested under apps: /v1/apps/{app_id}/envs
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_env))
        .route("/", get(list_envs))
        .route("/{env_id}", get(get_env))
}

/// Create env scale routes.
///
/// Scale is nested under orgs/apps/envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale
pub fn scale_routes() -> Router<AppState> {
    Router::new().route("/", post(set_scale))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new environment.
#[derive(Debug, Deserialize)]
pub struct CreateEnvRequest {
    /// Environment name (unique within app, e.g., "production", "staging").
    pub name: String,
}

/// Response for a single environment.
#[derive(Debug, Serialize)]
pub struct EnvResponse {
    /// Environment ID.
    pub id: String,

    /// Application ID.
    pub app_id: String,

    /// Organization ID.
    pub org_id: String,

    /// Environment name.
    pub name: String,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the env was created.
    pub created_at: DateTime<Utc>,

    /// When the env was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing environments.
#[derive(Debug, Serialize)]
pub struct ListEnvsResponse {
    /// List of environments.
    pub items: Vec<EnvResponse>,

    /// Total count (for pagination).
    pub total: i64,
}

/// Request to set scale for an environment.
#[derive(Debug, Deserialize)]
pub struct SetScaleRequest {
    /// Process type to count mapping.
    pub process_counts: std::collections::HashMap<String, i32>,
}

/// Response for setting scale.
#[derive(Debug, Serialize)]
pub struct SetScaleResponse {
    /// Whether the scale was set successfully.
    pub success: bool,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new environment.
///
/// POST /v1/apps/{app_id}/envs
async fn create_env(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
    Json(req): Json<CreateEnvRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate app_id format
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Get app and verify it exists
    let app_row = sqlx::query_as::<_, AppInfoRow>(
        "SELECT app_id, org_id FROM apps_view WHERE app_id = $1 AND NOT is_deleted",
    )
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check app existence");
        ApiError::internal("internal_error", "Failed to verify application")
            .with_request_id(request_id.to_string())
    })?;

    let app_row = app_row.ok_or_else(|| {
        ApiError::not_found("app_not_found", format!("Application {} not found", app_id))
            .with_request_id(request_id.to_string())
    })?;

    let org_id: OrgId = app_row.org_id.parse().map_err(|_| {
        ApiError::internal("internal_error", "Invalid org_id in database")
            .with_request_id(request_id.to_string())
    })?;

    // Validate name
    if req.name.is_empty() {
        return Err(
            ApiError::bad_request("invalid_name", "Environment name cannot be empty")
                .with_request_id(request_id.to_string()),
        );
    }

    if req.name.len() > 50 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Environment name cannot exceed 50 characters",
        )
        .with_request_id(request_id.to_string()));
    }

    // Validate name format (lowercase alphanumeric and hyphens)
    if !req
        .name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Environment name must contain only lowercase letters, numbers, and hyphens",
        )
        .with_request_id(request_id.to_string()));
    }

    // Check for duplicate name within app
    let name_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM envs_view WHERE app_id = $1 AND name = $2 AND NOT is_deleted)",
    )
    .bind(app_id.to_string())
    .bind(&req.name)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check env name uniqueness");
        ApiError::internal("internal_error", "Failed to verify environment name")
            .with_request_id(request_id.to_string())
    })?;

    if name_exists {
        return Err(ApiError::conflict(
            "env_name_exists",
            format!(
                "Environment '{}' already exists in this application",
                req.name
            ),
        )
        .with_request_id(request_id.to_string()));
    }

    let env_id = EnvId::new();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Env,
        aggregate_id: env_id.to_string(),
        aggregate_seq: 1,
        event_type: "env.created".to_string(),
        event_version: 1,
        actor_type: ActorType::System, // TODO: Extract from auth context
        actor_id: "system".to_string(),
        org_id: Some(org_id.clone()),
        request_id: request_id.to_string(),
        idempotency_key: None,
        app_id: Some(app_id.clone()),
        env_id: Some(env_id.clone()),
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "name": req.name
        }),
    };

    // Append the event
    let event_store = state.db().event_store();
    event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create env");
        ApiError::internal("internal_error", "Failed to create environment")
            .with_request_id(request_id.to_string())
    })?;

    let now = Utc::now();
    let response = EnvResponse {
        id: env_id.to_string(),
        app_id: app_id.to_string(),
        org_id: org_id.to_string(),
        name: req.name,
        resource_version: 1,
        created_at: now,
        updated_at: now,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// List environments in an application.
///
/// GET /v1/apps/{app_id}/envs
async fn list_envs(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate app_id format
    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Query the envs_view table
    let rows = sqlx::query_as::<_, EnvRow>(
        r#"
        SELECT env_id, app_id, org_id, name, resource_version, created_at, updated_at
        FROM envs_view
        WHERE app_id = $1 AND NOT is_deleted
        ORDER BY created_at DESC
        LIMIT 100
        "#,
    )
    .bind(&app_id)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list envs");
        ApiError::internal("internal_error", "Failed to list environments")
            .with_request_id(request_id.to_string())
    })?;

    let items: Vec<EnvResponse> = rows.into_iter().map(EnvResponse::from).collect();
    let total = items.len() as i64;

    Ok(Json(ListEnvsResponse { items, total }))
}

/// Set scale for an environment.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale
async fn set_scale(
    State(state): State<AppState>,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Json(req): Json<SetScaleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate IDs
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.to_string())
    })?;

    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.to_string())
    })?;

    let env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Verify env exists
    let env_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM envs_view WHERE env_id = $1 AND NOT is_deleted)",
    )
    .bind(env_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check env existence");
        ApiError::internal("internal_error", "Failed to verify environment")
            .with_request_id(request_id.to_string())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id),
        )
        .with_request_id(request_id.to_string()));
    }

    // Validate process counts
    for (process_type, count) in &req.process_counts {
        if process_type.is_empty() {
            return Err(ApiError::bad_request(
                "invalid_process_type",
                "Process type cannot be empty",
            )
            .with_request_id(request_id.to_string()));
        }
        if *count < 0 {
            return Err(ApiError::bad_request(
                "invalid_count",
                format!("Count for '{}' must be non-negative", process_type),
            )
            .with_request_id(request_id.to_string()));
        }
    }

    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::Env, &env_id.to_string())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to set scale")
                .with_request_id(request_id.to_string())
        })?
        .unwrap_or(0);

    // Convert process_counts map to scales array format expected by projection
    let scales: Vec<serde_json::Value> = req
        .process_counts
        .iter()
        .map(|(process_type, count)| {
            serde_json::json!({
                "process_type": process_type,
                "desired": count
            })
        })
        .collect();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Env,
        aggregate_id: env_id.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: "env.scale_set".to_string(),
        event_version: 1,
        actor_type: ActorType::System, // TODO: Extract from auth context
        actor_id: "system".to_string(),
        org_id: Some(org_id.clone()),
        request_id: request_id.to_string(),
        idempotency_key: None,
        app_id: Some(app_id.clone()),
        env_id: Some(env_id.clone()),
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "env_id": env_id.to_string(),
            "org_id": org_id.to_string(),
            "app_id": app_id.to_string(),
            "scales": scales
        }),
    };

    // Append the event
    event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to set scale");
        ApiError::internal("internal_error", "Failed to set scale")
            .with_request_id(request_id.to_string())
    })?;

    tracing::info!(
        env_id = %env_id,
        process_counts = ?req.process_counts,
        "Scale set for environment"
    );

    Ok((StatusCode::OK, Json(SetScaleResponse { success: true })))
}

/// Get a single environment by ID.
///
/// GET /v1/apps/{app_id}/envs/{env_id}
async fn get_env(
    State(state): State<AppState>,
    Path((app_id, env_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate app_id format
    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Validate env_id format
    let _env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Query the envs_view table
    let row = sqlx::query_as::<_, EnvRow>(
        r#"
        SELECT env_id, app_id, org_id, name, resource_version, created_at, updated_at
        FROM envs_view
        WHERE env_id = $1 AND NOT is_deleted
        "#,
    )
    .bind(&env_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, env_id = %env_id, "Failed to get env");
        ApiError::internal("internal_error", "Failed to get environment")
            .with_request_id(request_id.to_string())
    })?;

    match row {
        Some(row) => Ok(Json(EnvResponse::from(row))),
        None => Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id),
        )
        .with_request_id(request_id.to_string())),
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Minimal app info for lookups.
struct AppInfoRow {
    #[allow(dead_code)]
    app_id: String,
    org_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AppInfoRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            app_id: row.try_get("app_id")?,
            org_id: row.try_get("org_id")?,
        })
    }
}

/// Row from envs_view table.
struct EnvRow {
    env_id: String,
    app_id: String,
    org_id: String,
    name: String,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for EnvRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            env_id: row.try_get("env_id")?,
            app_id: row.try_get("app_id")?,
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl From<EnvRow> for EnvResponse {
    fn from(row: EnvRow) -> Self {
        Self {
            id: row.env_id,
            app_id: row.app_id,
            org_id: row.org_id,
            name: row.name,
            resource_version: row.resource_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_env_request_deserialization() {
        let json = r#"{"name": "production"}"#;
        let req: CreateEnvRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "production");
    }

    #[test]
    fn test_env_response_serialization() {
        let response = EnvResponse {
            id: "env_123".to_string(),
            app_id: "app_456".to_string(),
            org_id: "org_789".to_string(),
            name: "staging".to_string(),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"env_123\""));
        assert!(json.contains("\"app_id\":\"app_456\""));
        assert!(json.contains("\"name\":\"staging\""));
    }
}
