//! Volume attachment API endpoints.
//!
//! Attachments bind an org-owned volume to an env/process type at a mount path.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{
    event_types, AggregateType, VolumeAttachmentCreatedPayload, VolumeAttachmentDeletedPayload,
};
use plfm_id::{AppId, EnvId, OrgId, VolumeAttachmentId, VolumeId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Volume attachment routes.
///
/// /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_attachment))
        .route("/:attachment_id", delete(delete_attachment))
}

// =============================================================================
// Request/Response Types (OpenAPI parity)
// =============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateVolumeAttachmentRequest {
    pub volume_id: String,
    pub process_type: String,
    pub mount_path: String,
    #[serde(default)]
    pub read_only: bool,
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
pub struct DeleteResponse {
    pub ok: bool,
}

// =============================================================================
// Handlers
// =============================================================================

/// Attach a volume to an env/process type.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments
async fn create_attachment(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Json(mut req): Json<CreateVolumeAttachmentRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "volume_attachments.create";

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

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    req.process_type = req.process_type.trim().to_string();
    if req.process_type.is_empty() {
        return Err(
            ApiError::bad_request("invalid_process_type", "process_type cannot be empty")
                .with_request_id(request_id),
        );
    }

    req.mount_path = req.mount_path.trim().to_string();
    validate_mount_path(&req.mount_path, &request_id)?;

    let volume_id: VolumeId = req.volume_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_volume_id", "Invalid volume ID format")
            .with_request_id(request_id.clone())
    })?;

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "app_id": app_id.to_string(),
                "env_id": env_id.to_string(),
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

    // Validate env exists (scoped to org/app).
    let env_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM envs_view
            WHERE env_id = $1 AND org_id = $2 AND app_id = $3 AND NOT is_deleted
        )
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, app_id = %app_id, env_id = %env_id, "Failed to check env existence");
        ApiError::internal("internal_error", "Failed to create volume attachment")
            .with_request_id(request_id.clone())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id),
        )
        .with_request_id(request_id.clone()));
    }

    // Validate volume exists and is owned by org.
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
        ApiError::internal("internal_error", "Failed to create volume attachment")
            .with_request_id(request_id.clone())
    })?;

    if !volume_exists {
        return Err(ApiError::not_found("volume_not_found", "Volume not found")
            .with_request_id(request_id.clone()));
    }

    // Enforce uniqueness of (env_id, process_type, mount_path).
    let attachment_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM volume_attachments_view
            WHERE org_id = $1
              AND env_id = $2
              AND process_type = $3
              AND mount_path = $4
              AND NOT is_deleted
        )
        "#,
    )
    .bind(org_id.to_string())
    .bind(env_id.to_string())
    .bind(&req.process_type)
    .bind(&req.mount_path)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, env_id = %env_id, "Failed to check attachment uniqueness");
        ApiError::internal("internal_error", "Failed to create volume attachment")
            .with_request_id(request_id.clone())
    })?;

    if attachment_exists {
        return Err(ApiError::conflict(
            "attachment_exists",
            "An attachment already exists for this mount_path and process_type",
        )
        .with_request_id(request_id.clone()));
    }

    let attachment_id = VolumeAttachmentId::new();
    let payload = VolumeAttachmentCreatedPayload {
        attachment_id,
        org_id,
        volume_id,
        app_id,
        env_id,
        process_type: req.process_type.clone(),
        mount_path: req.mount_path.clone(),
        read_only: req.read_only,
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize attachment payload");
        ApiError::internal("internal_error", "Failed to create volume attachment")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::VolumeAttachment,
        aggregate_id: attachment_id.to_string(),
        aggregate_seq: 1,
        event_type: event_types::VOLUME_ATTACHMENT_CREATED.to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: Some(app_id),
        env_id: Some(env_id),
        correlation_id: None,
        causation_id: None,
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, attachment_id = %attachment_id, "Failed to create volume attachment");
        ApiError::internal("internal_error", "Failed to create volume attachment")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "volume_attachments",
            event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, AttachmentRow>(
        r#"
        SELECT attachment_id, volume_id, env_id, process_type, mount_path, read_only, created_at
        FROM volume_attachments_view
        WHERE org_id = $1 AND attachment_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(org_id.to_string())
    .bind(attachment_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, attachment_id = %attachment_id, "Failed to load created attachment");
        ApiError::internal("internal_error", "Failed to create volume attachment")
            .with_request_id(request_id.clone())
    })?;

    let response = VolumeAttachmentResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create volume attachment")
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

/// Detach a volume attachment (idempotent for already-deleted attachments).
///
/// DELETE /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments/{attachment_id}
async fn delete_attachment(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id, attachment_id)): Path<(String, String, String, String)>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();

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
    let attachment_id: VolumeAttachmentId = attachment_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_attachment_id", "Invalid attachment ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    let row = sqlx::query_as::<_, AttachmentDeleteRow>(
        r#"
        SELECT attachment_id, app_id, env_id, volume_id, process_type, is_deleted
        FROM volume_attachments_view
        WHERE org_id = $1 AND attachment_id = $2
        "#,
    )
    .bind(org_id.to_string())
    .bind(attachment_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, attachment_id = %attachment_id, "Failed to load attachment");
        ApiError::internal("internal_error", "Failed to delete attachment")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(
            ApiError::not_found("attachment_not_found", "Attachment not found")
                .with_request_id(request_id),
        );
    };

    if row.app_id != app_id.to_string() || row.env_id != env_id.to_string() {
        return Err(
            ApiError::not_found("attachment_not_found", "Attachment not found")
                .with_request_id(request_id),
        );
    }

    let response = DeleteResponse { ok: true };
    if row.is_deleted {
        return Ok((StatusCode::OK, Json(response)).into_response());
    }

    let current_seq = state
        .db()
        .event_store()
        .get_latest_aggregate_seq(&AggregateType::VolumeAttachment, &attachment_id.to_string())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, attachment_id = %attachment_id, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to delete attachment")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let payload = VolumeAttachmentDeletedPayload {
        attachment_id,
        org_id,
        volume_id: row.volume_id.parse().map_err(|_| {
            ApiError::internal("internal_error", "Corrupt attachment state")
                .with_request_id(request_id.clone())
        })?,
        env_id,
        process_type: row.process_type.clone(),
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize attachment delete payload");
        ApiError::internal("internal_error", "Failed to delete attachment")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::VolumeAttachment,
        aggregate_id: attachment_id.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: event_types::VOLUME_ATTACHMENT_DELETED.to_string(),
        event_version: 1,
        actor_type,
        actor_id,
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key,
        app_id: Some(app_id),
        env_id: Some(env_id),
        correlation_id: None,
        causation_id: None,
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, attachment_id = %attachment_id, "Failed to delete attachment");
        ApiError::internal("internal_error", "Failed to delete attachment")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "volume_attachments",
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

// =============================================================================
// Helpers
// =============================================================================

fn validate_mount_path(mount_path: &str, request_id: &str) -> Result<(), ApiError> {
    if !mount_path.starts_with('/') {
        return Err(ApiError::bad_request(
            "invalid_mount_path",
            "mount_path must be an absolute path",
        )
        .with_request_id(request_id.to_string()));
    }

    if mount_path == "/" {
        return Err(
            ApiError::bad_request("invalid_mount_path", "mount_path cannot be '/'")
                .with_request_id(request_id.to_string()),
        );
    }

    let reserved = ["/proc", "/sys", "/dev", "/run", "/etc"];
    for prefix in reserved {
        if mount_path == prefix || mount_path.starts_with(&format!("{prefix}/")) {
            return Err(ApiError::bad_request(
                "invalid_mount_path",
                "mount_path is under a reserved system path",
            )
            .with_request_id(request_id.to_string()));
        }
    }

    if mount_path.contains("/../") || mount_path.ends_with("/..") || mount_path == ".." {
        return Err(ApiError::bad_request(
            "invalid_mount_path",
            "mount_path must not contain '..'",
        )
        .with_request_id(request_id.to_string()));
    }

    Ok(())
}

// =============================================================================
// DB Row Types
// =============================================================================

#[derive(Debug)]
struct AttachmentRow {
    attachment_id: String,
    volume_id: String,
    env_id: String,
    process_type: String,
    mount_path: String,
    read_only: bool,
    created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AttachmentRow {
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

impl From<AttachmentRow> for VolumeAttachmentResponse {
    fn from(row: AttachmentRow) -> Self {
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
struct AttachmentDeleteRow {
    #[allow(dead_code)]
    attachment_id: String,
    app_id: String,
    env_id: String,
    volume_id: String,
    process_type: String,
    is_deleted: bool,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AttachmentDeleteRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            attachment_id: row.try_get("attachment_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            volume_id: row.try_get("volume_id")?,
            process_type: row.try_get("process_type")?,
            is_deleted: row.try_get("is_deleted")?,
        })
    }
}
