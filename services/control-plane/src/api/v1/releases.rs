//! Release API endpoints.
//!
//! Provides operations for creating and querying releases.
//! Releases are immutable artifacts that capture an app's image and manifest.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::AggregateType;
use plfm_id::{AppId, OrgId, ReleaseId};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create release routes.
///
/// Releases are nested under apps: /v1/orgs/{org_id}/apps/{app_id}/releases
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_release))
        .route("/", get(list_releases))
        .route("/:release_id", get(get_release))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new release.
#[derive(Debug, Deserialize, Serialize)]
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

    /// Next cursor (null if no more results).
    pub next_cursor: Option<String>,
}

/// Query parameters for listing releases.
#[derive(Debug, Deserialize)]
pub struct ListReleasesQuery {
    /// Max number of items to return.
    pub limit: Option<i64>,
    /// Cursor (exclusive). Interpreted as a release_id.
    pub cursor: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new release.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/releases
async fn create_release(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id)): Path<(String, String)>,
    Json(req): Json<CreateReleaseRequest>,
) -> Result<Response, ApiError> {
    let RequestContext {
        request_id,
        idempotency_key,
        actor_type,
        actor_id,
    } = ctx;
    let endpoint_name = "releases.create";

    // Validate org_id format
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    // Validate app_id format
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    // Validate required fields
    if req.image_ref.is_empty() {
        return Err(
            ApiError::bad_request("invalid_image_ref", "Image reference cannot be empty")
                .with_request_id(request_id.clone()),
        );
    }

    if req.image_digest.is_empty() {
        return Err(
            ApiError::bad_request("invalid_image_digest", "Image digest cannot be empty")
                .with_request_id(request_id.clone()),
        );
    }

    if !req.image_digest.starts_with("sha256:") {
        return Err(ApiError::bad_request(
            "invalid_image_digest",
            "Image digest must start with 'sha256:'",
        )
        .with_request_id(request_id.clone()));
    }

    if req.manifest_hash.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_manifest_hash",
            "Manifest hash cannot be empty",
        )
        .with_request_id(request_id.clone()));
    }

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "app_id": app_id.to_string(),
                "body": &req
            });
            idempotency::request_hash(endpoint_name, &hash_input)
                .map(|hash| (key.to_string(), hash))
        })
        .transpose()
        .map_err(|e| e.with_request_id(request_id.clone()))?;

    if let Some((key, hash)) = request_hash.as_ref() {
        if let Some((status, body)) = idempotency::check(
            &state,
            &org_scope,
            &actor_id,
            endpoint_name,
            key,
            hash,
            &request_id,
        )
        .await?
        {
            return Ok(
                (status, Json(body.unwrap_or_else(|| serde_json::json!({})))).into_response(),
            );
        }
    }

    // Validate app exists and belongs to org
    let app_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM apps_view WHERE app_id = $1 AND org_id = $2 AND NOT is_deleted)",
    )
    .bind(app_id.to_string())
    .bind(org_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check app existence");
        ApiError::internal("internal_error", "Failed to verify application")
            .with_request_id(request_id.clone())
    })?;

    if !app_exists {
        return Err(ApiError::not_found(
            "app_not_found",
            format!(
                "Application {} not found in organization {}",
                app_id, org_id
            ),
        )
        .with_request_id(request_id.clone()));
    }

    let release_id = ReleaseId::new();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Release,
        aggregate_id: release_id.to_string(),
        aggregate_seq: 1,
        event_type: "release.created".to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: Some(app_id),
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
    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create release");
        ApiError::internal("internal_error", "Failed to create release")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "releases",
            event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, ReleaseRow>(
        r#"
        SELECT release_id, org_id, app_id, image_ref, index_or_manifest_digest,
               manifest_schema_version, manifest_hash, resource_version, created_at
        FROM releases_view
        WHERE release_id = $1 AND org_id = $2 AND app_id = $3
        "#,
    )
    .bind(release_id.to_string())
    .bind(org_scope.clone())
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load release");
        ApiError::internal("internal_error", "Failed to load release")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Release was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = ReleaseResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create release")
                .with_request_id(request_id.clone())
        })?;

        let _ = idempotency::store(
            &state,
            idempotency::StoreIdempotencyParams {
                org_scope: &org_scope,
                actor_id: &actor_id,
                endpoint_name,
                idempotency_key: &key,
                request_hash: &hash,
                status: StatusCode::OK,
                body: Some(body),
            },
            &request_id,
        )
        .await;
    }

    Ok((StatusCode::OK, Json(response)).into_response())
}

/// List releases for an application.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/releases
async fn list_releases(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id)): Path<(String, String)>,
    Query(query): Query<ListReleasesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate org_id format
    let _org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    // Validate app_id format
    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match query.cursor.as_deref() {
        Some(raw) => {
            let _: ReleaseId = raw.parse().map_err(|_| {
                ApiError::bad_request("invalid_cursor", "Invalid cursor format")
                    .with_request_id(request_id.clone())
            })?;
            Some(raw.to_string())
        }
        None => None,
    };

    // Query the releases_view table (stable ordering by release_id)
    let rows = sqlx::query_as::<_, ReleaseRow>(
        r#"
        SELECT release_id, org_id, app_id, image_ref, index_or_manifest_digest,
               manifest_schema_version, manifest_hash, resource_version, created_at
        FROM releases_view
        WHERE org_id = $1 AND app_id = $2
          AND ($3::TEXT IS NULL OR release_id > $3)
        ORDER BY release_id ASC
        LIMIT $4
        "#,
    )
    .bind(&org_id)
    .bind(&app_id)
    .bind(cursor.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list releases");
        ApiError::internal("internal_error", "Failed to list releases")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<ReleaseResponse> = rows.into_iter().map(ReleaseResponse::from).collect();
    let next_cursor = if items.len() == limit as usize {
        items.last().map(|r| r.id.clone())
    } else {
        None
    };

    Ok(Json(ListReleasesResponse { items, next_cursor }))
}

/// Get a single release by ID.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/releases/{release_id}
async fn get_release(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, release_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate IDs
    let _org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let _release_id: ReleaseId = release_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_release_id", "Invalid release ID format")
            .with_request_id(request_id.clone())
    })?;

    // Query the releases_view table
    let row = sqlx::query_as::<_, ReleaseRow>(
        r#"
        SELECT release_id, org_id, app_id, image_ref, index_or_manifest_digest,
               manifest_schema_version, manifest_hash, resource_version, created_at
        FROM releases_view
        WHERE org_id = $1 AND app_id = $2 AND release_id = $3
        "#,
    )
    .bind(&org_id)
    .bind(&app_id)
    .bind(&release_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, release_id = %release_id, "Failed to get release");
        ApiError::internal("internal_error", "Failed to get release")
            .with_request_id(request_id.clone())
    })?;

    match row {
        Some(row) => Ok(Json(ReleaseResponse::from(row))),
        None => Err(ApiError::not_found(
            "release_not_found",
            format!("Release {} not found", release_id),
        )
        .with_request_id(request_id)),
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
