//! Secrets API endpoints.
//!
//! v1 treats secrets as an env-scoped "secret bundle" with version metadata.
//! The event log MUST NOT contain raw secret material; only version IDs and hashes.

use std::collections::BTreeMap;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{event_types, AggregateType};
use plfm_id::{AppId, EnvId, OrgId, SecretBundleId, SecretVersionId};
use sha2::{Digest, Sha256};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Secrets routes.
///
/// /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(get_secrets_metadata))
        .route("/", put(put_secrets))
}

// =============================================================================
// Request/Response Types (OpenAPI parity)
// =============================================================================

#[derive(Debug, serde::Serialize)]
pub struct SecretsMetadataResponse {
    pub env_id: String,
    pub bundle_id: String,
    pub current_version_id: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum PutSecretsRequest {
    EnvFile(PutSecretsEnvFileRequest),
    Map(PutSecretsMapRequest),
}

#[derive(Debug, serde::Deserialize)]
pub struct PutSecretsEnvFileRequest {
    pub format: String,
    pub data: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct PutSecretsMapRequest {
    pub values: BTreeMap<String, String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Get secrets metadata for an environment.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets
async fn get_secrets_metadata(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id_typed: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id_typed: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;
    let env_id_typed: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id_typed, &ctx).await?;

    let env_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM envs_view
            WHERE env_id = $1 AND org_id = $2 AND app_id = $3 AND NOT is_deleted
        )
        "#,
    )
    .bind(env_id_typed.to_string())
    .bind(org_id_typed.to_string())
    .bind(app_id_typed.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id_typed,
            app_id = %app_id_typed,
            env_id = %env_id_typed,
            "Failed to check env existence"
        );
        ApiError::internal("internal_error", "Failed to load secrets metadata")
            .with_request_id(request_id.clone())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id_typed),
        )
        .with_request_id(request_id));
    }

    let row = sqlx::query_as::<_, SecretBundleRow>(
        r#"
        SELECT bundle_id, current_version_id, updated_at
        FROM secret_bundles_view
        WHERE org_id = $1 AND app_id = $2 AND env_id = $3
        "#,
    )
    .bind(org_id_typed.to_string())
    .bind(app_id_typed.to_string())
    .bind(env_id_typed.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id_typed,
            app_id = %app_id_typed,
            env_id = %env_id_typed,
            "Failed to load secret bundle metadata"
        );
        ApiError::internal("internal_error", "Failed to load secrets metadata")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(ApiError::not_found(
            "secrets_not_configured",
            "Secrets have not been configured for this environment",
        )
        .with_request_id(request_id));
    };

    let Some(current_version_id) = row.current_version_id else {
        return Err(ApiError::not_found(
            "secrets_not_configured",
            "Secrets have not been configured for this environment",
        )
        .with_request_id(request_id));
    };

    Ok(Json(SecretsMetadataResponse {
        env_id: env_id_typed.to_string(),
        bundle_id: row.bundle_id,
        current_version_id,
        updated_at: row.updated_at,
    }))
}

/// Set secrets for an environment (creates a new version).
///
/// PUT /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets
async fn put_secrets(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Json(req): Json<PutSecretsRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "secrets.put";

    let org_id_typed: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id_typed: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;
    let env_id_typed: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id_typed, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    let (format, data_hash) = validate_and_hash_secrets(&req, &request_id)?;

    let org_scope = org_id_typed.to_string();
    let request_hash = idempotency_key.as_deref().map(|key| {
        let mut hasher = Sha256::new();
        hasher.update(endpoint_name.as_bytes());
        hasher.update(b"\n");
        hasher.update(org_id_typed.to_string().as_bytes());
        hasher.update(b"\n");
        hasher.update(app_id_typed.to_string().as_bytes());
        hasher.update(b"\n");
        hasher.update(env_id_typed.to_string().as_bytes());
        hasher.update(b"\n");
        hasher.update(format.as_bytes());
        hasher.update(b"\n");
        hasher.update(data_hash.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        (key.to_string(), hash)
    });

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
    .bind(env_id_typed.to_string())
    .bind(org_id_typed.to_string())
    .bind(app_id_typed.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id_typed,
            app_id = %app_id_typed,
            env_id = %env_id_typed,
            "Failed to check env existence"
        );
        ApiError::internal("internal_error", "Failed to set secrets")
            .with_request_id(request_id.clone())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id_typed),
        )
        .with_request_id(request_id.clone()));
    }

    let existing = sqlx::query_as::<_, SecretBundleExistingRow>(
        r#"
        SELECT bundle_id
        FROM secret_bundles_view
        WHERE org_id = $1 AND app_id = $2 AND env_id = $3
        "#,
    )
    .bind(org_id_typed.to_string())
    .bind(app_id_typed.to_string())
    .bind(env_id_typed.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id_typed,
            app_id = %app_id_typed,
            env_id = %env_id_typed,
            "Failed to check existing secret bundle"
        );
        ApiError::internal("internal_error", "Failed to set secrets")
            .with_request_id(request_id.clone())
    })?;

    let now = Utc::now();
    let version_id = SecretVersionId::new();

    let (bundle_id, event_ids) = if let Some(existing) = existing {
        let bundle_id: SecretBundleId = existing.bundle_id.parse().map_err(|_| {
            ApiError::internal("internal_error", "Corrupt secret bundle state")
                .with_request_id(request_id.clone())
        })?;

        let current_seq = state
            .db()
            .event_store()
            .get_latest_aggregate_seq(&AggregateType::SecretBundle, &bundle_id.to_string())
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    request_id = %request_id,
                    bundle_id = %bundle_id,
                    "Failed to get aggregate sequence"
                );
                ApiError::internal("internal_error", "Failed to set secrets")
                    .with_request_id(request_id.clone())
            })?
            .unwrap_or(0);

        let payload = serde_json::json!({
            "bundle_id": bundle_id,
            "org_id": org_id_typed,
            "env_id": env_id_typed,
            "version_id": version_id,
            "format": &format,
            "data_hash": &data_hash,
            "updated_at": now.to_rfc3339(),
        });

        let event = AppendEvent {
            aggregate_type: AggregateType::SecretBundle,
            aggregate_id: bundle_id.to_string(),
            aggregate_seq: current_seq + 1,
            event_type: event_types::SECRET_BUNDLE_VERSION_SET.to_string(),
            event_version: 1,
            actor_type,
            actor_id: actor_id.clone(),
            org_id: Some(org_id_typed),
            request_id: request_id.clone(),
            idempotency_key: idempotency_key.clone(),
            app_id: Some(app_id_typed),
            env_id: Some(env_id_typed),
            correlation_id: None,
            causation_id: None,
            payload,
        };

        let event_id = state.db().event_store().append(event).await.map_err(|e| {
            tracing::error!(
                error = %e,
                request_id = %request_id,
                bundle_id = %bundle_id,
                "Failed to append secret bundle version_set event"
            );
            match e {
                crate::db::DbError::SequenceConflict { .. } => ApiError::conflict(
                    "version_conflict",
                    "Concurrent secrets update detected; retry",
                )
                .with_request_id(request_id.clone()),
                _ => ApiError::internal("internal_error", "Failed to set secrets")
                    .with_request_id(request_id.clone()),
            }
        })?;

        (bundle_id, vec![event_id])
    } else {
        let bundle_id = SecretBundleId::new();

        let created_payload = serde_json::json!({
            "bundle_id": bundle_id,
            "org_id": org_id_typed,
            "app_id": app_id_typed,
            "env_id": env_id_typed,
            "format": &format,
            "created_at": now.to_rfc3339(),
        });

        let version_payload = serde_json::json!({
            "bundle_id": bundle_id,
            "org_id": org_id_typed,
            "env_id": env_id_typed,
            "version_id": version_id,
            "format": &format,
            "data_hash": &data_hash,
            "updated_at": now.to_rfc3339(),
        });

        let events = vec![
            AppendEvent {
                aggregate_type: AggregateType::SecretBundle,
                aggregate_id: bundle_id.to_string(),
                aggregate_seq: 1,
                event_type: event_types::SECRET_BUNDLE_CREATED.to_string(),
                event_version: 1,
                actor_type,
                actor_id: actor_id.clone(),
                org_id: Some(org_id_typed),
                request_id: request_id.clone(),
                idempotency_key: idempotency_key.clone(),
                app_id: Some(app_id_typed),
                env_id: Some(env_id_typed),
                correlation_id: None,
                causation_id: None,
                payload: created_payload,
            },
            AppendEvent {
                aggregate_type: AggregateType::SecretBundle,
                aggregate_id: bundle_id.to_string(),
                aggregate_seq: 2,
                event_type: event_types::SECRET_BUNDLE_VERSION_SET.to_string(),
                event_version: 1,
                actor_type,
                actor_id: actor_id.clone(),
                org_id: Some(org_id_typed),
                request_id: request_id.clone(),
                idempotency_key: idempotency_key.clone(),
                app_id: Some(app_id_typed),
                env_id: Some(env_id_typed),
                correlation_id: None,
                causation_id: None,
                payload: version_payload,
            },
        ];

        let event_ids = state
            .db()
            .event_store()
            .append_batch(events)
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    request_id = %request_id,
                    bundle_id = %bundle_id,
                    "Failed to append secret bundle events"
                );
                match e {
                    crate::db::DbError::SequenceConflict { .. } => ApiError::conflict(
                        "version_conflict",
                        "Concurrent secrets update detected; retry",
                    )
                    .with_request_id(request_id.clone()),
                    _ => ApiError::internal("internal_error", "Failed to set secrets")
                        .with_request_id(request_id.clone()),
                }
            })?;

        (bundle_id, event_ids)
    };

    let last_event_id = event_ids
        .last()
        .copied()
        .ok_or_else(|| ApiError::internal("internal_error", "Failed to set secrets"))?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "secret_bundles",
            last_event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let updated = sqlx::query_as::<_, SecretBundleRow>(
        r#"
        SELECT bundle_id, current_version_id, updated_at
        FROM secret_bundles_view
        WHERE org_id = $1 AND app_id = $2 AND env_id = $3
        "#,
    )
    .bind(org_id_typed.to_string())
    .bind(app_id_typed.to_string())
    .bind(env_id_typed.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id_typed,
            app_id = %app_id_typed,
            env_id = %env_id_typed,
            "Failed to load updated secret bundle metadata"
        );
        ApiError::internal("internal_error", "Failed to set secrets")
            .with_request_id(request_id.clone())
    })?;

    let Some(current_version_id) = updated.current_version_id else {
        return Err(ApiError::gateway_timeout(
            "projection_timeout",
            "Secrets update not yet visible",
        )
        .with_request_id(request_id));
    };

    let response_body = SecretsMetadataResponse {
        env_id: env_id_typed.to_string(),
        bundle_id: bundle_id.to_string(),
        current_version_id,
        updated_at: updated.updated_at,
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response_body).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to set secrets")
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

    Ok((StatusCode::OK, Json(response_body)).into_response())
}

// =============================================================================
// Helpers
// =============================================================================

fn validate_and_hash_secrets(
    req: &PutSecretsRequest,
    request_id: &str,
) -> Result<(String, String), ApiError> {
    match req {
        PutSecretsRequest::EnvFile(env_file) => {
            if env_file.format != "platform_env_v1" {
                return Err(ApiError::bad_request(
                    "invalid_secrets_format",
                    "format must be 'platform_env_v1'",
                )
                .with_request_id(request_id.to_string()));
            }

            let max_len = 1_048_576usize; // 1 MiB guardrail for v1
            if env_file.data.len() > max_len {
                return Err(ApiError::bad_request(
                    "secrets_too_large",
                    "secrets data is too large",
                )
                .with_request_id(request_id.to_string()));
            }

            let mut hasher = Sha256::new();
            hasher.update(env_file.data.as_bytes());
            Ok((env_file.format.clone(), format!("{:x}", hasher.finalize())))
        }
        PutSecretsRequest::Map(map) => {
            // Validate key names only (never log values).
            if map.values.len() > 10_000 {
                return Err(
                    ApiError::bad_request("secrets_too_large", "Too many secret keys")
                        .with_request_id(request_id.to_string()),
                );
            }

            let mut total_bytes: usize = 0;

            for (key, value) in &map.values {
                total_bytes = total_bytes
                    .saturating_add(key.len())
                    .saturating_add(value.len());

                let key = key.trim();
                if key.is_empty() {
                    return Err(ApiError::bad_request(
                        "invalid_secret_key",
                        "secret keys cannot be empty",
                    )
                    .with_request_id(request_id.to_string()));
                }
                if key.len() > 256 {
                    return Err(ApiError::bad_request(
                        "invalid_secret_key",
                        "secret keys must be <= 256 characters",
                    )
                    .with_request_id(request_id.to_string()));
                }
            }

            let max_len = 1_048_576usize; // 1 MiB guardrail for v1
            if total_bytes > max_len {
                return Err(ApiError::bad_request(
                    "secrets_too_large",
                    "secrets data is too large",
                )
                .with_request_id(request_id.to_string()));
            }

            let mut canonical = String::new();
            for (k, v) in &map.values {
                canonical.push_str(k);
                canonical.push('=');
                canonical.push_str(v);
                canonical.push('\n');
            }

            let mut hasher = Sha256::new();
            hasher.update(canonical.as_bytes());
            Ok((
                "platform_env_v1".to_string(),
                format!("{:x}", hasher.finalize()),
            ))
        }
    }
}

// =============================================================================
// DB Row Types
// =============================================================================

#[derive(Debug)]
struct SecretBundleRow {
    bundle_id: String,
    current_version_id: Option<String>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for SecretBundleRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            bundle_id: row.try_get("bundle_id")?,
            current_version_id: row.try_get("current_version_id")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

#[derive(Debug)]
struct SecretBundleExistingRow {
    bundle_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for SecretBundleExistingRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            bundle_id: row.try_get("bundle_id")?,
        })
    }
}
