//! Organization API endpoints.
//!
//! Provides CRUD operations for organizations.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType};
use plfm_id::{OrgId, RequestId};
use serde::{Deserialize, Serialize};

use crate::db::AppendEvent;
use crate::state::AppState;

/// Create org routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_org))
        .route("/", get(list_orgs))
        .route("/{org_id}", get(get_org))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new organization.
#[derive(Debug, Deserialize)]
pub struct CreateOrgRequest {
    /// Organization name.
    pub name: String,
}

/// Response for a single organization.
#[derive(Debug, Serialize)]
pub struct OrgResponse {
    /// Organization ID.
    pub id: String,

    /// Organization name.
    pub name: String,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the org was created.
    pub created_at: DateTime<Utc>,

    /// When the org was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing organizations.
#[derive(Debug, Serialize)]
pub struct ListOrgsResponse {
    /// List of organizations.
    pub items: Vec<OrgResponse>,

    /// Total count (for pagination).
    pub total: i64,
}

/// Error response.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// Error code.
    pub code: String,

    /// Human-readable message.
    pub message: String,

    /// Request ID for correlation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new organization.
///
/// POST /v1/orgs
async fn create_org(
    State(state): State<AppState>,
    Json(req): Json<CreateOrgRequest>,
) -> impl IntoResponse {
    let request_id = RequestId::new();
    let org_id = OrgId::new();

    // Validate name
    if req.name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: "invalid_name".to_string(),
                message: "Organization name cannot be empty".to_string(),
                request_id: Some(request_id.to_string()),
            }),
        )
            .into_response();
    }

    if req.name.len() > 100 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: "invalid_name".to_string(),
                message: "Organization name cannot exceed 100 characters".to_string(),
                request_id: Some(request_id.to_string()),
            }),
        )
            .into_response();
    }

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Org,
        aggregate_id: org_id.to_string(),
        aggregate_seq: 1, // First event for this org
        event_type: "org.created".to_string(),
        event_version: 1,
        actor_type: ActorType::System, // TODO: Extract from auth context
        actor_id: "system".to_string(), // TODO: Extract from auth context
        org_id: Some(org_id.clone()),
        request_id: request_id.to_string(),
        idempotency_key: None, // TODO: Extract from header
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "name": req.name
        }),
    };

    // Append the event
    let event_store = state.db().event_store();
    match event_store.append(event).await {
        Ok(_event_id) => {
            let now = Utc::now();
            let response = OrgResponse {
                id: org_id.to_string(),
                name: req.name,
                resource_version: 1,
                created_at: now,
                updated_at: now,
            };

            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, request_id = %request_id, "Failed to create org");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: "internal_error".to_string(),
                    message: "Failed to create organization".to_string(),
                    request_id: Some(request_id.to_string()),
                }),
            )
                .into_response()
        }
    }
}

/// List organizations.
///
/// GET /v1/orgs
async fn list_orgs(State(state): State<AppState>) -> impl IntoResponse {
    let request_id = RequestId::new();

    // Query the orgs_view table
    let result = sqlx::query_as::<_, OrgRow>(
        r#"
        SELECT org_id, name, resource_version, created_at, updated_at
        FROM orgs_view
        ORDER BY created_at DESC
        LIMIT 100
        "#,
    )
    .fetch_all(state.db().pool())
    .await;

    match result {
        Ok(rows) => {
            let items: Vec<OrgResponse> = rows
                .into_iter()
                .map(|row| OrgResponse {
                    id: row.org_id,
                    name: row.name,
                    resource_version: row.resource_version,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                })
                .collect();

            let total = items.len() as i64;

            Json(ListOrgsResponse { items, total }).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, request_id = %request_id, "Failed to list orgs");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: "internal_error".to_string(),
                    message: "Failed to list organizations".to_string(),
                    request_id: Some(request_id.to_string()),
                }),
            )
                .into_response()
        }
    }
}

/// Get a single organization by ID.
///
/// GET /v1/orgs/{org_id}
async fn get_org(State(state): State<AppState>, Path(org_id): Path<String>) -> impl IntoResponse {
    let request_id = RequestId::new();

    // Validate org_id format
    if org_id.parse::<OrgId>().is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: "invalid_org_id".to_string(),
                message: "Invalid organization ID format".to_string(),
                request_id: Some(request_id.to_string()),
            }),
        )
            .into_response();
    }

    // Query the orgs_view table
    let result = sqlx::query_as::<_, OrgRow>(
        r#"
        SELECT org_id, name, resource_version, created_at, updated_at
        FROM orgs_view
        WHERE org_id = $1
        "#,
    )
    .bind(&org_id)
    .fetch_optional(state.db().pool())
    .await;

    match result {
        Ok(Some(row)) => {
            let response = OrgResponse {
                id: row.org_id,
                name: row.name,
                resource_version: row.resource_version,
                created_at: row.created_at,
                updated_at: row.updated_at,
            };
            Json(response).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                code: "org_not_found".to_string(),
                message: format!("Organization {} not found", org_id),
                request_id: Some(request_id.to_string()),
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to get org");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    code: "internal_error".to_string(),
                    message: "Failed to get organization".to_string(),
                    request_id: Some(request_id.to_string()),
                }),
            )
                .into_response()
        }
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Row from orgs_view table.
struct OrgRow {
    org_id: String,
    name: String,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for OrgRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_org_request_deserialization() {
        let json = r#"{"name": "Acme Corp"}"#;
        let req: CreateOrgRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Acme Corp");
    }

    #[test]
    fn test_org_response_serialization() {
        let response = OrgResponse {
            id: "org_123".to_string(),
            name: "Test Org".to_string(),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"org_123\""));
    }
}
