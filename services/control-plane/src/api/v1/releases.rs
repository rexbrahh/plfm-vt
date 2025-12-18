//! Release API endpoints.
//!
//! Provides operations for creating and querying releases.
//! Releases are immutable artifacts that capture an app's image and manifest.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType};
use plfm_id::{AppId, OrgId, ReleaseId, RequestId};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create release routes.
///
/// Releases are nested under apps: /v1/orgs/{org_id}/apps/{app_id}/releases
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_release))
        .route("/", get(list_releases))
        .route("/{release_id}", get(get_release))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new release.
#[derive(Debug, Deserialize)]
pub struct CreateReleaseRequest {
    /// OCI image reference (e.g., "registry.example.com/app:v1.0").
    pub image_ref: String,

    /// Image digest (sha256:...).
    pub image_digest: String,

    /// Manifest schema version.
    #[serde(default = "default_manifest_version")]
    pub manifest_schema_version: i32,

    /// Hash of the manifest content.
    pub manifest_hash: String,
}

fn default_manifest_version() -> i32 {
    1
}

/// Response for a single release.
#[derive(Debug, Serialize)]
pub struct ReleaseResponse {
    /// Release ID.
    pub id: String,

    /// Organization ID.
    pub org_id: String,

    /// Application ID.
    pub app_id: String,

    /// OCI image reference.
    pub image_ref: String,

    /// Image digest.
    pub image_digest: String,

    /// Manifest schema version.
    pub manifest_schema_version: i32,

    /// Manifest hash.
    pub manifest_hash: String,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the release was created.
    pub created_at: DateTime<Utc>,
}

/// Response for listing releases.
#[derive(Debug, Serialize)]
pub struct ListReleasesResponse {
    /// List of releases.
    pub items: Vec<ReleaseResponse>,

    /// Total count (for pagination).
    pub total: i64,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new release.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/releases
async fn create_release(
    State(state): State<AppState>,
    Path((org_id, app_id)): Path<(String, String)>,
    Json(req): Json<CreateReleaseRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate org_id format
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Validate app_id format
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Validate app exists and belongs to org
    let app_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM apps_view WHERE app_id = $1 AND org_id = $2 AND NOT is_deleted)",
    )
    .bind(app_id.to_string())
    .bind(org_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check app existence");
        ApiError::internal("internal_error", "Failed to verify application")
            .with_request_id(request_id.to_string())
    })?;

    if !app_exists {
        return Err(ApiError::not_found(
            "app_not_found",
            format!("Application {} not found in organization {}", app_id, org_id),
        )
        .with_request_id(request_id.to_string()));
    }

    // Validate required fields
    if req.image_ref.is_empty() {
        return Err(ApiError::bad_request("invalid_image_ref", "Image reference cannot be empty")
            .with_request_id(request_id.to_string()));
    }

    if req.image_digest.is_empty() {
        return Err(ApiError::bad_request("invalid_image_digest", "Image digest cannot be empty")
            .with_request_id(request_id.to_string()));
    }

    if !req.image_digest.starts_with("sha256:") {
        return Err(ApiError::bad_request(
            "invalid_image_digest",
            "Image digest must start with 'sha256:'",
        )
        .with_request_id(request_id.to_string()));
    }

    if req.manifest_hash.is_empty() {
        return Err(ApiError::bad_request("invalid_manifest_hash", "Manifest hash cannot be empty")
            .with_request_id(request_id.to_string()));
    }

    let release_id = ReleaseId::new();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Release,
        aggregate_id: release_id.to_string(),
        aggregate_seq: 1,
        event_type: "release.created".to_string(),
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
            "image_ref": req.image_ref,
            "image_digest": req.image_digest,
            "manifest_schema_version": req.manifest_schema_version,
            "manifest_hash": req.manifest_hash
        }),
    };

    // Append the event
    let event_store = state.db().event_store();
    event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create release");
        ApiError::internal("internal_error", "Failed to create release")
            .with_request_id(request_id.to_string())
    })?;

    let now = Utc::now();
    let response = ReleaseResponse {
        id: release_id.to_string(),
        org_id: org_id.to_string(),
        app_id: app_id.to_string(),
        image_ref: req.image_ref,
        image_digest: req.image_digest,
        manifest_schema_version: req.manifest_schema_version,
        manifest_hash: req.manifest_hash,
        resource_version: 1,
        created_at: now,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// List releases for an application.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/releases
async fn list_releases(
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

    // Query the releases_view table
    let rows = sqlx::query_as::<_, ReleaseRow>(
        r#"
        SELECT release_id, org_id, app_id, image_ref, index_or_manifest_digest,
               manifest_schema_version, manifest_hash, resource_version, created_at
        FROM releases_view
        WHERE app_id = $1
        ORDER BY created_at DESC
        LIMIT 100
        "#,
    )
    .bind(&app_id)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list releases");
        ApiError::internal("internal_error", "Failed to list releases")
            .with_request_id(request_id.to_string())
    })?;

    let items: Vec<ReleaseResponse> = rows.into_iter().map(ReleaseResponse::from).collect();
    let total = items.len() as i64;

    Ok(Json(ListReleasesResponse { items, total }))
}

/// Get a single release by ID.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/releases/{release_id}
async fn get_release(
    State(state): State<AppState>,
    Path((org_id, app_id, release_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate IDs
    let _org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.to_string())
    })?;

    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.to_string())
    })?;

    let _release_id: ReleaseId = release_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_release_id", "Invalid release ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Query the releases_view table
    let row = sqlx::query_as::<_, ReleaseRow>(
        r#"
        SELECT release_id, org_id, app_id, image_ref, index_or_manifest_digest,
               manifest_schema_version, manifest_hash, resource_version, created_at
        FROM releases_view
        WHERE release_id = $1
        "#,
    )
    .bind(&release_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, release_id = %release_id, "Failed to get release");
        ApiError::internal("internal_error", "Failed to get release")
            .with_request_id(request_id.to_string())
    })?;

    match row {
        Some(row) => Ok(Json(ReleaseResponse::from(row))),
        None => Err(ApiError::not_found(
            "release_not_found",
            format!("Release {} not found", release_id),
        )
        .with_request_id(request_id.to_string())),
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Row from releases_view table.
struct ReleaseRow {
    release_id: String,
    org_id: String,
    app_id: String,
    image_ref: String,
    index_or_manifest_digest: String,
    manifest_schema_version: i32,
    manifest_hash: String,
    resource_version: i32,
    created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ReleaseRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            release_id: row.try_get("release_id")?,
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            image_ref: row.try_get("image_ref")?,
            index_or_manifest_digest: row.try_get("index_or_manifest_digest")?,
            manifest_schema_version: row.try_get("manifest_schema_version")?,
            manifest_hash: row.try_get("manifest_hash")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

impl From<ReleaseRow> for ReleaseResponse {
    fn from(row: ReleaseRow) -> Self {
        Self {
            id: row.release_id,
            org_id: row.org_id,
            app_id: row.app_id,
            image_ref: row.image_ref,
            image_digest: row.index_or_manifest_digest,
            manifest_schema_version: row.manifest_schema_version,
            manifest_hash: row.manifest_hash,
            resource_version: row.resource_version,
            created_at: row.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_release_request_deserialization() {
        let json = r#"{
            "image_ref": "registry.example.com/app:v1.0",
            "image_digest": "sha256:abc123",
            "manifest_hash": "def456"
        }"#;
        let req: CreateReleaseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.image_ref, "registry.example.com/app:v1.0");
        assert_eq!(req.image_digest, "sha256:abc123");
        assert_eq!(req.manifest_schema_version, 1); // default
        assert_eq!(req.manifest_hash, "def456");
    }

    #[test]
    fn test_release_response_serialization() {
        let response = ReleaseResponse {
            id: "rel_123".to_string(),
            org_id: "org_456".to_string(),
            app_id: "app_789".to_string(),
            image_ref: "registry.example.com/app:v1.0".to_string(),
            image_digest: "sha256:abc123".to_string(),
            manifest_schema_version: 1,
            manifest_hash: "def456".to_string(),
            resource_version: 1,
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"rel_123\""));
        assert!(json.contains("\"image_ref\":\"registry.example.com/app:v1.0\""));
    }
}
