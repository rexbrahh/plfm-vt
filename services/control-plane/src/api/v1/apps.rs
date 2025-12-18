//! Application API endpoints.
//!
//! Provides CRUD operations for applications within organizations.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType};
use plfm_id::{AppId, OrgId, RequestId};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create app routes.
///
/// Apps are nested under orgs: /v1/orgs/{org_id}/apps
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_app))
        .route("/", get(list_apps))
        .route("/{app_id}", get(get_app))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new application.
#[derive(Debug, Deserialize)]
pub struct CreateAppRequest {
    /// Application name (unique within org).
    pub name: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Response for a single application.
#[derive(Debug, Serialize)]
pub struct AppResponse {
    /// Application ID.
    pub id: String,

    /// Organization ID.
    pub org_id: String,

    /// Application name.
    pub name: String,

    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the app was created.
    pub created_at: DateTime<Utc>,

    /// When the app was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing applications.
#[derive(Debug, Serialize)]
pub struct ListAppsResponse {
    /// List of applications.
    pub items: Vec<AppResponse>,

    /// Total count (for pagination).
    pub total: i64,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new application.
///
/// POST /v1/orgs/{org_id}/apps
async fn create_app(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
    Json(req): Json<CreateAppRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate org_id format
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Validate org exists
    let org_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM orgs_view WHERE org_id = $1)",
    )
    .bind(org_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check org existence");
        ApiError::internal("internal_error", "Failed to verify organization")
            .with_request_id(request_id.to_string())
    })?;

    if !org_exists {
        return Err(ApiError::not_found(
            "org_not_found",
            format!("Organization {} not found", org_id),
        )
        .with_request_id(request_id.to_string()));
    }

    // Validate name
    if req.name.is_empty() {
        return Err(ApiError::bad_request("invalid_name", "Application name cannot be empty")
            .with_request_id(request_id.to_string()));
    }

    if req.name.len() > 100 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Application name cannot exceed 100 characters",
        )
        .with_request_id(request_id.to_string()));
    }

    // Check for duplicate name within org
    let name_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM apps_view WHERE org_id = $1 AND name = $2 AND NOT is_deleted)",
    )
    .bind(org_id.to_string())
    .bind(&req.name)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check app name uniqueness");
        ApiError::internal("internal_error", "Failed to verify application name")
            .with_request_id(request_id.to_string())
    })?;

    if name_exists {
        return Err(ApiError::conflict(
            "app_name_exists",
            format!("Application '{}' already exists in this organization", req.name),
        )
        .with_request_id(request_id.to_string()));
    }

    let app_id = AppId::new();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::App,
        aggregate_id: app_id.to_string(),
        aggregate_seq: 1,
        event_type: "app.created".to_string(),
        event_version: 1,
        actor_type: ActorType::System, // TODO: Extract from auth context
        actor_id: "system".to_string(),
        org_id: Some(org_id.clone()),
        request_id: request_id.to_string(),
        idempotency_key: None,
        app_id: Some(app_id.clone()),
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "name": req.name,
            "description": req.description
        }),
    };

    // Append the event
    let event_store = state.db().event_store();
    event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create app");
        ApiError::internal("internal_error", "Failed to create application")
            .with_request_id(request_id.to_string())
    })?;

    let now = Utc::now();
    let response = AppResponse {
        id: app_id.to_string(),
        org_id: org_id.to_string(),
        name: req.name,
        description: req.description,
        resource_version: 1,
        created_at: now,
        updated_at: now,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// List applications in an organization.
///
/// GET /v1/orgs/{org_id}/apps
async fn list_apps(
    State(state): State<AppState>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate org_id format
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Query the apps_view table
    let rows = sqlx::query_as::<_, AppRow>(
        r#"
        SELECT app_id, org_id, name, description, resource_version, created_at, updated_at
        FROM apps_view
        WHERE org_id = $1 AND NOT is_deleted
        ORDER BY created_at DESC
        LIMIT 100
        "#,
    )
    .bind(org_id.to_string())
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list apps");
        ApiError::internal("internal_error", "Failed to list applications")
            .with_request_id(request_id.to_string())
    })?;

    let items: Vec<AppResponse> = rows.into_iter().map(AppResponse::from).collect();
    let total = items.len() as i64;

    Ok(Json(ListAppsResponse { items, total }))
}

/// Get a single application by ID.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}
async fn get_app(
    State(state): State<AppState>,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate org_id format
    let _org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Validate app_id format
    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Query the apps_view table
    let row = sqlx::query_as::<_, AppRow>(
        r#"
        SELECT app_id, org_id, name, description, resource_version, created_at, updated_at
        FROM apps_view
        WHERE app_id = $1 AND NOT is_deleted
        "#,
    )
    .bind(&app_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, app_id = %app_id, "Failed to get app");
        ApiError::internal("internal_error", "Failed to get application")
            .with_request_id(request_id.to_string())
    })?;

    match row {
        Some(row) => Ok(Json(AppResponse::from(row))),
        None => Err(ApiError::not_found(
            "app_not_found",
            format!("Application {} not found", app_id),
        )
        .with_request_id(request_id.to_string())),
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Row from apps_view table.
struct AppRow {
    app_id: String,
    org_id: String,
    name: String,
    description: Option<String>,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AppRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            app_id: row.try_get("app_id")?,
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl From<AppRow> for AppResponse {
    fn from(row: AppRow) -> Self {
        Self {
            id: row.app_id,
            org_id: row.org_id,
            name: row.name,
            description: row.description,
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
    fn test_create_app_request_deserialization() {
        let json = r#"{"name": "my-app", "description": "A test app"}"#;
        let req: CreateAppRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "my-app");
        assert_eq!(req.description, Some("A test app".to_string()));
    }

    #[test]
    fn test_create_app_request_without_description() {
        let json = r#"{"name": "my-app"}"#;
        let req: CreateAppRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "my-app");
        assert_eq!(req.description, None);
    }

    #[test]
    fn test_app_response_serialization() {
        let response = AppResponse {
            id: "app_123".to_string(),
            org_id: "org_456".to_string(),
            name: "Test App".to_string(),
            description: Some("A test".to_string()),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"app_123\""));
        assert!(json.contains("\"org_id\":\"org_456\""));
    }
}
