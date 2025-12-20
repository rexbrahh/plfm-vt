//! Exec API endpoints.
//!
//! Provides an audited exec grant for a specific instance.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{Duration, Utc};
use plfm_events::{event_types, AggregateType, ExecSessionGrantedPayload};
use plfm_id::{AppId, EnvId, ExecSessionId, InstanceId, OrgId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::api::tokens;
use crate::db::AppendEvent;
use crate::state::AppState;

const MAX_SESSIONS_PER_ENV: i64 = 10;
const MAX_SESSIONS_PER_INSTANCE: i64 = 2;

/// Exec routes.
///
/// /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances/{instance_id}/exec
pub fn routes() -> Router<AppState> {
    Router::new().route("/", post(create_exec_grant))
}

// =============================================================================
// Request/Response Types (OpenAPI parity)
// =============================================================================

fn default_tty() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ExecGrantRequest {
    pub command: Vec<String>,
    #[serde(default = "default_tty")]
    pub tty: bool,
}

#[derive(Debug, Serialize)]
pub struct ExecGrantResponse {
    pub session_id: String,
    pub connect_url: String,
    pub session_token: String,
    pub expires_in_seconds: i64,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create an exec session grant.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances/{instance_id}/exec
async fn create_exec_grant(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id, instance_id)): Path<(String, String, String, String)>,
    Json(req): Json<ExecGrantRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "exec.grant";

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

    let instance_id: InstanceId = instance_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_instance_id", "Invalid instance ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_admin(role, &request_id)?;

    validate_exec_command(&req.command, &request_id)?;

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "app_id": app_id.to_string(),
                "env_id": env_id.to_string(),
                "instance_id": instance_id.to_string(),
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

    let instance = sqlx::query_as::<_, InstanceForExecRow>(
        r#"
        SELECT d.desired_state, s.status as reported_status
        FROM instances_desired_view d
        LEFT JOIN instances_status_view s ON d.instance_id = s.instance_id
        WHERE d.instance_id = $1
          AND d.org_id = $2
          AND d.app_id = $3
          AND d.env_id = $4
        "#,
    )
    .bind(instance_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            instance_id = %instance_id,
            "Failed to load instance for exec"
        );
        ApiError::internal("internal_error", "Failed to create exec grant")
            .with_request_id(request_id.clone())
    })?;

    let Some(instance) = instance else {
        return Err(
            ApiError::not_found("instance_not_found", "Instance not found")
                .with_request_id(request_id),
        );
    };

    let effective_status = match instance.desired_state.as_str() {
        "stopped" => "stopped",
        "draining" => "draining",
        _ => instance.reported_status.as_deref().unwrap_or("booting"),
    };

    if effective_status != "ready" {
        return Err(ApiError::bad_request(
            "instance_not_ready",
            "Exec is only allowed for instances in ready state",
        )
        .with_request_id(request_id));
    }

    let expires_in_seconds: i64 = 60;
    let expires_at = Utc::now() + Duration::seconds(expires_in_seconds);
    let exec_session_id = ExecSessionId::new();

    // Returned to the client only; never stored in events.
    let session_token = format!("exec_tok_{}", Uuid::new_v4());
    let connect_url = format!("/v1/exec-sessions/{}/connect", exec_session_id);

    enforce_exec_concurrency_limits(&state, &env_id, &instance_id, &request_id).await?;

    let payload = ExecSessionGrantedPayload {
        exec_session_id,
        org_id,
        app_id,
        env_id,
        instance_id,
        requested_command: req.command.clone(),
        tty: req.tty,
        expires_at: expires_at.to_rfc3339(),
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            "Failed to serialize exec_session.granted payload"
        );
        ApiError::internal("internal_error", "Failed to create exec grant")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::ExecSession,
        aggregate_id: exec_session_id.to_string(),
        aggregate_seq: 1,
        event_type: event_types::EXEC_SESSION_GRANTED.to_string(),
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
        tracing::error!(
            error = %e,
            request_id = %request_id,
            exec_session_id = %exec_session_id,
            "Failed to append exec_session.granted event"
        );
        ApiError::internal("internal_error", "Failed to create exec grant")
            .with_request_id(request_id.clone())
    })?;

    store_exec_token(
        &state,
        &exec_session_id,
        &session_token,
        expires_at,
        &request_id,
    )
    .await?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "exec_sessions",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let response = ExecGrantResponse {
        session_id: exec_session_id.to_string(),
        connect_url,
        session_token,
        expires_in_seconds,
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create exec grant")
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

async fn enforce_exec_concurrency_limits(
    state: &AppState,
    env_id: &EnvId,
    instance_id: &InstanceId,
    request_id: &str,
) -> Result<(), ApiError> {
    let env_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM exec_sessions_view
        WHERE env_id = $1
          AND status IN ('granted', 'connected')
          AND expires_at > now()
        "#,
    )
    .bind(env_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check exec env limits");
        ApiError::internal("internal_error", "Failed to create exec grant")
            .with_request_id(request_id.to_string())
    })?;

    if env_count >= MAX_SESSIONS_PER_ENV {
        return Err(ApiError::too_many_requests(
            "exec_rate_limited",
            "Too many exec sessions for this environment",
        )
        .with_request_id(request_id.to_string()));
    }

    let instance_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM exec_sessions_view
        WHERE instance_id = $1
          AND status IN ('granted', 'connected')
          AND expires_at > now()
        "#,
    )
    .bind(instance_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            "Failed to check exec instance limits"
        );
        ApiError::internal("internal_error", "Failed to create exec grant")
            .with_request_id(request_id.to_string())
    })?;

    if instance_count >= MAX_SESSIONS_PER_INSTANCE {
        return Err(ApiError::too_many_requests(
            "exec_rate_limited",
            "Too many exec sessions for this instance",
        )
        .with_request_id(request_id.to_string()));
    }

    Ok(())
}

async fn store_exec_token(
    state: &AppState,
    exec_session_id: &ExecSessionId,
    token: &str,
    expires_at: chrono::DateTime<Utc>,
    request_id: &str,
) -> Result<(), ApiError> {
    let token_hash = tokens::hash_token(token);
    sqlx::query(
        r#"
        INSERT INTO exec_session_tokens (exec_session_id, token_hash, expires_at)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(exec_session_id.to_string())
    .bind(token_hash)
    .bind(expires_at)
    .execute(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            exec_session_id = %exec_session_id,
            "Failed to store exec session token"
        );
        ApiError::internal("internal_error", "Failed to create exec grant")
            .with_request_id(request_id.to_string())
    })?;

    Ok(())
}

fn validate_exec_command(command: &[String], request_id: &str) -> Result<(), ApiError> {
    if command.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_command",
            "command must contain at least one element",
        )
        .with_request_id(request_id.to_string()));
    }

    if command.len() > 64 {
        return Err(ApiError::bad_request(
            "invalid_command",
            "command must contain at most 64 elements",
        )
        .with_request_id(request_id.to_string()));
    }

    let mut total_bytes: usize = 0;
    for (idx, part) in command.iter().enumerate() {
        let part = part.trim();
        if part.is_empty() {
            return Err(ApiError::bad_request(
                "invalid_command",
                format!("command[{idx}] cannot be empty"),
            )
            .with_request_id(request_id.to_string()));
        }

        if part.contains('\0') {
            return Err(ApiError::bad_request(
                "invalid_command",
                format!("command[{idx}] contains invalid NUL byte"),
            )
            .with_request_id(request_id.to_string()));
        }

        if part.len() > 1024 {
            return Err(ApiError::bad_request(
                "invalid_command",
                format!("command[{idx}] is too long (max 1024 chars)"),
            )
            .with_request_id(request_id.to_string()));
        }

        total_bytes = total_bytes.saturating_add(part.len());
        if total_bytes > 8192 {
            return Err(ApiError::bad_request(
                "invalid_command",
                "command is too long (max 8192 bytes total)",
            )
            .with_request_id(request_id.to_string()));
        }
    }

    Ok(())
}

// =============================================================================
// Database Row Types
// =============================================================================

struct InstanceForExecRow {
    desired_state: String,
    reported_status: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceForExecRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            desired_state: row.try_get("desired_state")?,
            reported_status: row.try_get("reported_status")?,
        })
    }
}
