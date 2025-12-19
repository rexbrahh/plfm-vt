//! Volume API endpoints.
//!
//! Volumes are org-scoped resources that can be attached to env/process types.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{
    event_types, AggregateType, JobStatus, RestoreJobCreatedPayload,
    RestoreJobStatusChangedPayload, SnapshotCreatedPayload, VolumeCreatedPayload,
    VolumeDeletedPayload,
};
use plfm_id::{OrgId, RestoreJobId, SnapshotId, VolumeId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Volume routes.
///
/// /v1/orgs/{org_id}/volumes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_volumes))
        .route("/", post(create_volume))
        .route("/:volume_id", get(get_volume))
        .route("/:volume_id", delete(delete_volume))
        .route("/:volume_id/snapshots", post(create_snapshot))
        .route("/:volume_id/snapshots", get(list_snapshots))
        .route("/:volume_id/restore", post(restore_volume))
}

// =============================================================================
// Request/Response Types (OpenAPI parity)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListVolumesQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListSnapshotsQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub ok: bool,
}

#[derive(Debug, Serialize)]
pub struct VolumeAttachmentResponse {
    pub id: String,
    pub volume_id: String,
    pub env_id: String,
    pub process_type: String,
    pub mount_path: String,
    #[serde(default)]
    pub read_only: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct VolumeResponse {
    pub id: String,
    pub org_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub size_bytes: i64,
    pub filesystem: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub attachments: Vec<VolumeAttachmentResponse>,
}

#[derive(Debug, Serialize)]
pub struct ListVolumesResponse {
    pub items: Vec<VolumeResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateVolumeRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub size_bytes: i64,
    #[serde(default = "default_filesystem")]
    pub filesystem: String,
    #[serde(default = "default_backup_enabled")]
    pub backup_enabled: bool,
}

fn default_filesystem() -> String {
    "ext4".to_string()
}

fn default_backup_enabled() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct SnapshotResponse {
    pub id: String,
    pub volume_id: String,
    pub created_at: DateTime<Utc>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListSnapshotsResponse {
    pub items: Vec<SnapshotResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSnapshotRequest {
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RestoreVolumeRequest {
    pub snapshot_id: String,
    pub new_volume_name: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// List volumes (org scoped).
///
/// GET /v1/orgs/{org_id}/volumes
async fn list_volumes(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Query(query): Query<ListVolumesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query.cursor.as_deref();

    let rows = sqlx::query_as::<_, VolumeRow>(
        r#"
        SELECT
            volume_id,
            org_id,
            name,
            size_bytes,
            filesystem,
            backup_enabled,
            created_at,
            updated_at
        FROM volumes_view
        WHERE org_id = $1
          AND NOT is_deleted
          AND ($2::TEXT IS NULL OR volume_id > $2)
        ORDER BY volume_id ASC
        LIMIT $3
        "#,
    )
    .bind(org_id.to_string())
    .bind(cursor)
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to list volumes");
        ApiError::internal("internal_error", "Failed to list volumes")
            .with_request_id(request_id.clone())
    })?;

    let volume_ids: Vec<String> = rows.iter().map(|r| r.volume_id.clone()).collect();
    let mut attachments =
        load_attachments_for_volumes(&state, &request_id, &org_id, &volume_ids).await?;

    let mut items: Vec<VolumeResponse> = Vec::with_capacity(rows.len());
    for row in rows {
        let attachments_for_volume = attachments.remove(&row.volume_id).unwrap_or_default();

        items.push(VolumeResponse {
            id: row.volume_id.clone(),
            org_id: row.org_id.clone(),
            name: row.name.clone(),
            size_bytes: row.size_bytes,
            filesystem: row.filesystem.clone(),
            created_at: row.created_at,
            updated_at: Some(row.updated_at),
            attachments: attachments_for_volume,
        });
    }

    let next_cursor = items
        .last()
        .filter(|_| items.len() as i64 == limit)
        .map(|v| v.id.clone());

    Ok(Json(ListVolumesResponse { items, next_cursor }))
}

/// Create volume.
///
/// POST /v1/orgs/{org_id}/volumes
async fn create_volume(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Json(mut req): Json<CreateVolumeRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "volumes.create";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    if req.size_bytes < 1_073_741_824 {
        return Err(ApiError::bad_request(
            "invalid_size_bytes",
            "size_bytes must be >= 1073741824 (1GiB)",
        )
        .with_request_id(request_id));
    }

    if let Some(name) = req.name.as_mut() {
        *name = name.trim().to_string();
        if name.is_empty() {
            return Err(
                ApiError::bad_request("invalid_name", "name cannot be empty")
                    .with_request_id(request_id),
            );
        }
    }

    if req.filesystem != "ext4" {
        return Err(
            ApiError::bad_request("invalid_filesystem", "filesystem must be 'ext4'")
                .with_request_id(request_id),
        );
    }

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            idempotency::request_hash(endpoint_name, &req).map(|hash| (key.to_string(), hash))
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

    let volume_id = VolumeId::new();
    let payload = VolumeCreatedPayload {
        volume_id,
        org_id,
        name: req.name.clone(),
        size_bytes: req.size_bytes,
        filesystem: req.filesystem.clone(),
        backup_enabled: req.backup_enabled,
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize volume payload");
        ApiError::internal("internal_error", "Failed to create volume")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::Volume,
        aggregate_id: volume_id.to_string(),
        aggregate_seq: 1,
        event_type: event_types::VOLUME_CREATED.to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, volume_id = %volume_id, "Failed to create volume");
        ApiError::internal("internal_error", "Failed to create volume")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "volumes",
            event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, VolumeRow>(
        r#"
        SELECT
            volume_id,
            org_id,
            name,
            size_bytes,
            filesystem,
            backup_enabled,
            created_at,
            updated_at
        FROM volumes_view
        WHERE org_id = $1 AND volume_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, volume_id = %volume_id, "Failed to load created volume");
        ApiError::internal("internal_error", "Failed to create volume")
            .with_request_id(request_id.clone())
    })?;

    let response = VolumeResponse {
        id: row.volume_id.clone(),
        org_id: row.org_id.clone(),
        name: row.name.clone(),
        size_bytes: row.size_bytes,
        filesystem: row.filesystem.clone(),
        created_at: row.created_at,
        updated_at: Some(row.updated_at),
        attachments: Vec::new(),
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create volume")
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

/// Get volume.
///
/// GET /v1/orgs/{org_id}/volumes/{volume_id}
async fn get_volume(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, volume_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let volume_id: VolumeId = volume_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_volume_id", "Invalid volume ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let row = sqlx::query_as::<_, VolumeRow>(
        r#"
        SELECT
            volume_id,
            org_id,
            name,
            size_bytes,
            filesystem,
            backup_enabled,
            created_at,
            updated_at
        FROM volumes_view
        WHERE org_id = $1 AND volume_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, volume_id = %volume_id, "Failed to load volume");
        ApiError::internal("internal_error", "Failed to load volume")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(
            ApiError::not_found("volume_not_found", "Volume not found").with_request_id(request_id)
        );
    };

    let volume_id_str = volume_id.to_string();
    let mut attachments = load_attachments_for_volumes(
        &state,
        &request_id,
        &org_id,
        std::slice::from_ref(&volume_id_str),
    )
    .await?;
    let attachments = attachments.remove(&volume_id_str).unwrap_or_default();

    Ok(Json(VolumeResponse {
        id: row.volume_id.clone(),
        org_id: row.org_id.clone(),
        name: row.name.clone(),
        size_bytes: row.size_bytes,
        filesystem: row.filesystem.clone(),
        created_at: row.created_at,
        updated_at: Some(row.updated_at),
        attachments,
    }))
}

/// Delete volume (idempotent for already-deleted volumes).
///
/// DELETE /v1/orgs/{org_id}/volumes/{volume_id}
async fn delete_volume(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, volume_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let volume_id: VolumeId = volume_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_volume_id", "Invalid volume ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    let row = sqlx::query_as::<_, VolumeDeleteRow>(
        r#"
        SELECT volume_id, org_id, is_deleted
        FROM volumes_view
        WHERE org_id = $1 AND volume_id = $2
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, volume_id = %volume_id, "Failed to load volume");
        ApiError::internal("internal_error", "Failed to delete volume")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(
            ApiError::not_found("volume_not_found", "Volume not found").with_request_id(request_id)
        );
    };

    let response = DeleteResponse { ok: true };
    if row.is_deleted {
        return Ok((StatusCode::OK, Json(response)).into_response());
    }

    let current_seq = state
        .db()
        .event_store()
        .get_latest_aggregate_seq(&AggregateType::Volume, &volume_id.to_string())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, volume_id = %volume_id, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to delete volume")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let payload = VolumeDeletedPayload { volume_id, org_id };
    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize volume delete payload");
        ApiError::internal("internal_error", "Failed to delete volume")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::Volume,
        aggregate_id: volume_id.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: event_types::VOLUME_DELETED.to_string(),
        event_version: 1,
        actor_type,
        actor_id,
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key,
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, volume_id = %volume_id, "Failed to delete volume");
        ApiError::internal("internal_error", "Failed to delete volume")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "volumes",
            event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    Ok((StatusCode::OK, Json(response)).into_response())
}

/// Create snapshot for a volume.
///
/// POST /v1/orgs/{org_id}/volumes/{volume_id}/snapshots
async fn create_snapshot(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, volume_id)): Path<(String, String)>,
    maybe_body: Option<Json<CreateSnapshotRequest>>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "snapshots.create";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let volume_id: VolumeId = volume_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_volume_id", "Invalid volume ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    let note = maybe_body
        .and_then(|Json(b)| b.note)
        .map(|n| n.trim().to_string());

    // Validate volume exists.
    let volume_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM volumes_view
            WHERE org_id = $1 AND volume_id = $2 AND NOT is_deleted
        )
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, volume_id = %volume_id, "Failed to check volume existence");
        ApiError::internal("internal_error", "Failed to create snapshot")
            .with_request_id(request_id.clone())
    })?;

    if !volume_exists {
        return Err(
            ApiError::not_found("volume_not_found", "Volume not found").with_request_id(request_id)
        );
    }

    let org_scope = org_id.to_string();
    let hash_input = serde_json::json!({
        "volume_id": volume_id.to_string(),
        "note": note.as_deref(),
    });
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
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

    let snapshot_id = plfm_id::SnapshotId::new();
    let payload = SnapshotCreatedPayload {
        snapshot_id,
        org_id,
        volume_id,
        status: JobStatus::Queued,
        note,
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize snapshot payload");
        ApiError::internal("internal_error", "Failed to create snapshot")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::Snapshot,
        aggregate_id: snapshot_id.to_string(),
        aggregate_seq: 1,
        event_type: event_types::SNAPSHOT_CREATED.to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, snapshot_id = %snapshot_id, "Failed to create snapshot");
        ApiError::internal("internal_error", "Failed to create snapshot")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "snapshots",
            event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, SnapshotRow>(
        r#"
        SELECT snapshot_id, volume_id, created_at, status, size_bytes
        FROM snapshots_view
        WHERE org_id = $1 AND snapshot_id = $2
        "#,
    )
    .bind(org_id.to_string())
    .bind(snapshot_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, snapshot_id = %snapshot_id, "Failed to load created snapshot");
        ApiError::internal("internal_error", "Failed to create snapshot")
            .with_request_id(request_id.clone())
    })?;

    let response = SnapshotResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create snapshot")
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

/// List snapshots for a volume.
///
/// GET /v1/orgs/{org_id}/volumes/{volume_id}/snapshots
async fn list_snapshots(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, volume_id)): Path<(String, String)>,
    Query(query): Query<ListSnapshotsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let volume_id: VolumeId = volume_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_volume_id", "Invalid volume ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    // 404 if volume doesn't exist.
    let volume_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM volumes_view
            WHERE org_id = $1 AND volume_id = $2 AND NOT is_deleted
        )
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, volume_id = %volume_id, "Failed to check volume existence");
        ApiError::internal("internal_error", "Failed to list snapshots")
            .with_request_id(request_id.clone())
    })?;

    if !volume_exists {
        return Err(
            ApiError::not_found("volume_not_found", "Volume not found").with_request_id(request_id)
        );
    }

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query.cursor.as_deref();

    let rows = sqlx::query_as::<_, SnapshotRow>(
        r#"
        SELECT snapshot_id, volume_id, created_at, status, size_bytes
        FROM snapshots_view
        WHERE org_id = $1
          AND volume_id = $2
          AND ($3::TEXT IS NULL OR snapshot_id > $3)
        ORDER BY snapshot_id ASC
        LIMIT $4
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_id.to_string())
    .bind(cursor)
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, volume_id = %volume_id, "Failed to list snapshots");
        ApiError::internal("internal_error", "Failed to list snapshots")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<SnapshotResponse> = rows.into_iter().map(SnapshotResponse::from).collect();
    let next_cursor = items
        .last()
        .filter(|_| items.len() as i64 == limit)
        .map(|s| s.id.clone());

    Ok(Json(ListSnapshotsResponse { items, next_cursor }))
}

/// Restore volume from snapshot (creates a new volume).
///
/// POST /v1/orgs/{org_id}/volumes/{volume_id}/restore
async fn restore_volume(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, volume_id)): Path<(String, String)>,
    Json(req): Json<RestoreVolumeRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "volumes.restore";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let volume_id: VolumeId = volume_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_volume_id", "Invalid volume ID format")
            .with_request_id(request_id.clone())
    })?;
    let snapshot_id: SnapshotId = req.snapshot_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_snapshot_id", "Invalid snapshot ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            idempotency::request_hash(endpoint_name, &req).map(|hash| (key.to_string(), hash))
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

    // Load source volume attributes.
    let source = sqlx::query_as::<_, VolumeRow>(
        r#"
        SELECT
            volume_id,
            org_id,
            name,
            size_bytes,
            filesystem,
            backup_enabled,
            created_at,
            updated_at
        FROM volumes_view
        WHERE org_id = $1 AND volume_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, volume_id = %volume_id, "Failed to load source volume");
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;

    let Some(source) = source else {
        return Err(
            ApiError::not_found("volume_not_found", "Volume not found").with_request_id(request_id)
        );
    };

    // Validate snapshot exists and matches the source volume.
    let snapshot = sqlx::query_as::<_, SnapshotRow>(
        r#"
        SELECT snapshot_id, volume_id, created_at, status, size_bytes
        FROM snapshots_view
        WHERE org_id = $1 AND snapshot_id = $2
        "#,
    )
    .bind(org_id.to_string())
    .bind(snapshot_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, snapshot_id = %snapshot_id, "Failed to load snapshot");
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;

    let Some(snapshot) = snapshot else {
        return Err(
            ApiError::not_found("snapshot_not_found", "Snapshot not found")
                .with_request_id(request_id),
        );
    };

    if snapshot.volume_id != volume_id.to_string() {
        return Err(
            ApiError::not_found("snapshot_not_found", "Snapshot not found")
                .with_request_id(request_id),
        );
    }

    let new_volume_id = VolumeId::new();
    let restore_id = RestoreJobId::new();
    let new_name = req.new_volume_name.as_ref().map(|s| s.trim().to_string());
    if let Some(name) = new_name.as_deref() {
        if name.is_empty() {
            return Err(ApiError::bad_request(
                "invalid_new_volume_name",
                "new_volume_name cannot be empty",
            )
            .with_request_id(request_id));
        }
    }

    let restore_created = RestoreJobCreatedPayload {
        restore_id,
        org_id,
        snapshot_id,
        source_volume_id: volume_id,
        new_volume_name: new_name.clone(),
        status: JobStatus::Queued,
    };

    let new_volume_created = VolumeCreatedPayload {
        volume_id: new_volume_id,
        org_id,
        name: new_name.clone(),
        size_bytes: source.size_bytes,
        filesystem: source.filesystem.clone(),
        backup_enabled: source.backup_enabled,
    };

    let restore_done = RestoreJobStatusChangedPayload {
        restore_id,
        org_id,
        status: JobStatus::Succeeded,
        new_volume_id: Some(new_volume_id),
        failed_reason: None,
    };

    let restore_created_payload = serde_json::to_value(&restore_created).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize restore payload");
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;
    let new_volume_payload = serde_json::to_value(&new_volume_created).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize volume payload");
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;
    let restore_done_payload = serde_json::to_value(&restore_done).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize restore payload");
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;

    let events = vec![
        AppendEvent {
            aggregate_type: AggregateType::RestoreJob,
            aggregate_id: restore_id.to_string(),
            aggregate_seq: 1,
            event_type: event_types::RESTORE_JOB_CREATED.to_string(),
            event_version: 1,
            actor_type,
            actor_id: actor_id.clone(),
            org_id: Some(org_id),
            request_id: request_id.clone(),
            idempotency_key: idempotency_key.clone(),
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: restore_created_payload,
        },
        AppendEvent {
            aggregate_type: AggregateType::Volume,
            aggregate_id: new_volume_id.to_string(),
            aggregate_seq: 1,
            event_type: event_types::VOLUME_CREATED.to_string(),
            event_version: 1,
            actor_type,
            actor_id: actor_id.clone(),
            org_id: Some(org_id),
            request_id: request_id.clone(),
            idempotency_key: idempotency_key.clone(),
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: new_volume_payload,
        },
        AppendEvent {
            aggregate_type: AggregateType::RestoreJob,
            aggregate_id: restore_id.to_string(),
            aggregate_seq: 2,
            event_type: event_types::RESTORE_JOB_STATUS_CHANGED.to_string(),
            event_version: 1,
            actor_type,
            actor_id: actor_id.clone(),
            org_id: Some(org_id),
            request_id: request_id.clone(),
            idempotency_key: idempotency_key.clone(),
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: restore_done_payload,
        },
    ];

    let event_ids = state.db().event_store().append_batch(events).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, restore_id = %restore_id, "Failed to append restore events");
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;

    // Wait for volumes projection to apply the new volume.created event (2nd event in batch).
    let volume_event_id = event_ids.get(1).copied().ok_or_else(|| {
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "volumes",
            volume_event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, VolumeRow>(
        r#"
        SELECT
            volume_id,
            org_id,
            name,
            size_bytes,
            filesystem,
            backup_enabled,
            created_at,
            updated_at
        FROM volumes_view
        WHERE org_id = $1 AND volume_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(org_id.to_string())
    .bind(new_volume_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, volume_id = %new_volume_id, "Failed to load restored volume");
        ApiError::internal("internal_error", "Failed to restore volume")
            .with_request_id(request_id.clone())
    })?;

    let response = VolumeResponse {
        id: row.volume_id.clone(),
        org_id: row.org_id.clone(),
        name: row.name.clone(),
        size_bytes: row.size_bytes,
        filesystem: row.filesystem.clone(),
        created_at: row.created_at,
        updated_at: Some(row.updated_at),
        attachments: Vec::new(),
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to restore volume")
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

// =============================================================================
// Helpers
// =============================================================================

use std::collections::HashMap;

async fn load_attachments_for_volumes(
    state: &AppState,
    request_id: &str,
    org_id: &OrgId,
    volume_ids: &[String],
) -> Result<HashMap<String, Vec<VolumeAttachmentResponse>>, ApiError> {
    if volume_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query_as::<_, VolumeAttachmentRow>(
        r#"
        SELECT
            attachment_id,
            volume_id,
            env_id,
            process_type,
            mount_path,
            read_only,
            created_at
        FROM volume_attachments_view
        WHERE org_id = $1
          AND NOT is_deleted
          AND volume_id = ANY($2::TEXT[])
        ORDER BY volume_id ASC, attachment_id ASC
        "#,
    )
    .bind(org_id.to_string())
    .bind(volume_ids)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to load volume attachments");
        ApiError::internal("internal_error", "Failed to load volumes")
            .with_request_id(request_id.to_string())
    })?;

    let mut map: HashMap<String, Vec<VolumeAttachmentResponse>> = HashMap::new();
    for row in rows {
        map.entry(row.volume_id.clone())
            .or_default()
            .push(VolumeAttachmentResponse::from(row));
    }

    Ok(map)
}

// =============================================================================
// DB Row Types
// =============================================================================

#[derive(Debug)]
struct VolumeRow {
    volume_id: String,
    org_id: String,
    name: Option<String>,
    size_bytes: i64,
    filesystem: String,
    backup_enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for VolumeRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            volume_id: row.try_get("volume_id")?,
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            size_bytes: row.try_get("size_bytes")?,
            filesystem: row.try_get("filesystem")?,
            backup_enabled: row.try_get("backup_enabled")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

#[derive(Debug)]
struct VolumeDeleteRow {
    #[allow(dead_code)]
    volume_id: String,
    #[allow(dead_code)]
    org_id: String,
    is_deleted: bool,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for VolumeDeleteRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            volume_id: row.try_get("volume_id")?,
            org_id: row.try_get("org_id")?,
            is_deleted: row.try_get("is_deleted")?,
        })
    }
}

#[derive(Debug)]
struct VolumeAttachmentRow {
    attachment_id: String,
    volume_id: String,
    env_id: String,
    process_type: String,
    mount_path: String,
    read_only: bool,
    created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for VolumeAttachmentRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            attachment_id: row.try_get("attachment_id")?,
            volume_id: row.try_get("volume_id")?,
            env_id: row.try_get("env_id")?,
            process_type: row.try_get("process_type")?,
            mount_path: row.try_get("mount_path")?,
            read_only: row.try_get("read_only")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

impl From<VolumeAttachmentRow> for VolumeAttachmentResponse {
    fn from(row: VolumeAttachmentRow) -> Self {
        Self {
            id: row.attachment_id,
            volume_id: row.volume_id,
            env_id: row.env_id,
            process_type: row.process_type,
            mount_path: row.mount_path,
            read_only: row.read_only,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug)]
struct SnapshotRow {
    snapshot_id: String,
    volume_id: String,
    created_at: DateTime<Utc>,
    status: String,
    size_bytes: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for SnapshotRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            snapshot_id: row.try_get("snapshot_id")?,
            volume_id: row.try_get("volume_id")?,
            created_at: row.try_get("created_at")?,
            status: row.try_get("status")?,
            size_bytes: row.try_get("size_bytes")?,
        })
    }
}

impl From<SnapshotRow> for SnapshotResponse {
    fn from(row: SnapshotRow) -> Self {
        Self {
            id: row.snapshot_id,
            volume_id: row.volume_id,
            created_at: row.created_at,
            status: row.status,
            size_bytes: row.size_bytes,
        }
    }
}
