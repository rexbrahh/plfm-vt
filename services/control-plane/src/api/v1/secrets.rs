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
use plfm_secrets_format::Secrets;
use sha2::{Digest, Sha256};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::secrets as secrets_crypto;
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

    let (format, data_hash, plaintext_bytes) =
        validate_and_canonicalize_secrets(&req, &request_id)?;

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

        store_secret_material(
            &state,
            &org_id_typed,
            &app_id_typed,
            &env_id_typed,
            &bundle_id,
            &version_id,
            actor_type,
            &actor_id,
            &format,
            &data_hash,
            &plaintext_bytes,
            &request_id,
        )
        .await?;

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

        store_secret_material(
            &state,
            &org_id_typed,
            &app_id_typed,
            &env_id_typed,
            &bundle_id,
            &version_id,
            actor_type,
            &actor_id,
            &format,
            &data_hash,
            &plaintext_bytes,
            &request_id,
        )
        .await?;

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
            crate::api::projection_wait_timeout(),
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

fn validate_and_canonicalize_secrets(
    req: &PutSecretsRequest,
    request_id: &str,
) -> Result<(String, String, Vec<u8>), ApiError> {
    match req {
        PutSecretsRequest::EnvFile(env_file) => {
            if env_file.format != "platform_env_v1" {
                return Err(ApiError::bad_request(
                    "invalid_secrets_format",
                    "format must be 'platform_env_v1'",
                )
                .with_request_id(request_id.to_string()));
            }

            let secrets = Secrets::parse(&env_file.data).map_err(|e| {
                ApiError::bad_request("invalid_secrets_format", e.to_string())
                    .with_request_id(request_id.to_string())
            })?;

            let canonical = secrets.serialize();
            let data_hash = secrets.data_hash();
            let bytes = canonical.into_bytes();
            let max_len = 1_048_576usize; // 1 MiB guardrail for v1
            if bytes.len() > max_len {
                return Err(ApiError::bad_request(
                    "secrets_too_large",
                    "secrets data is too large",
                )
                .with_request_id(request_id.to_string()));
            }

            Ok((env_file.format.clone(), data_hash, bytes))
        }
        PutSecretsRequest::Map(map) => {
            if map.values.len() > 10_000 {
                return Err(
                    ApiError::bad_request("secrets_too_large", "Too many secret keys")
                        .with_request_id(request_id.to_string()),
                );
            }

            let secrets =
                Secrets::try_from_iter(map.values.iter().map(|(k, v)| (k, v))).map_err(|e| {
                    ApiError::bad_request("invalid_secrets_format", e.to_string())
                        .with_request_id(request_id.to_string())
                })?;

            let canonical = secrets.serialize();
            let data_hash = secrets.data_hash();
            let bytes = canonical.into_bytes();
            let max_len = 1_048_576usize; // 1 MiB guardrail for v1
            if bytes.len() > max_len {
                return Err(ApiError::bad_request(
                    "secrets_too_large",
                    "secrets data is too large",
                )
                .with_request_id(request_id.to_string()));
            }

            Ok(("platform_env_v1".to_string(), data_hash, bytes))
        }
    }
}

fn secrets_aad(
    org_id: &OrgId,
    env_id: &EnvId,
    bundle_id: &SecretBundleId,
    version_id: &SecretVersionId,
    data_hash: &str,
) -> String {
    format!(
        "trc-secrets-v1|org:{org_id}|env:{env_id}|bundle:{bundle_id}|version:{version_id}|hash:{data_hash}"
    )
}

async fn store_secret_material(
    state: &AppState,
    org_id: &OrgId,
    app_id: &AppId,
    env_id: &EnvId,
    bundle_id: &SecretBundleId,
    version_id: &SecretVersionId,
    actor_type: plfm_events::ActorType,
    actor_id: &str,
    format: &str,
    data_hash: &str,
    plaintext: &[u8],
    request_id: &str,
) -> Result<(), ApiError> {
    let aad = secrets_aad(org_id, env_id, bundle_id, version_id, data_hash);
    let encrypted = secrets_crypto::encrypt(plaintext, aad.as_bytes()).map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            env_id = %env_id,
            "Failed to encrypt secrets"
        );
        ApiError::internal("secrets_encryption_failed", "Failed to encrypt secrets")
            .with_request_id(request_id.to_string())
    })?;

    let material_id = format!("sm_{}", plfm_id::RequestId::new());

    sqlx::query(
        r#"
        INSERT INTO secret_material (
            material_id, cipher, nonce, ciphertext, master_key_id,
            wrapped_data_key, wrapped_data_key_nonce, plaintext_size_bytes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(&material_id)
    .bind(&encrypted.cipher)
    .bind(&encrypted.nonce)
    .bind(&encrypted.ciphertext)
    .bind(&encrypted.master_key_id)
    .bind(&encrypted.wrapped_data_key)
    .bind(&encrypted.wrapped_data_key_nonce)
    .bind(encrypted.plaintext_size_bytes)
    .execute(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            "Failed to store secret material"
        );
        ApiError::internal("internal_error", "Failed to set secrets")
            .with_request_id(request_id.to_string())
    })?;

    sqlx::query(
        r#"
        INSERT INTO secret_versions (
            version_id, bundle_id, org_id, app_id, env_id, data_hash,
            format, material_id, created_by_actor_id, created_by_actor_type
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(version_id.to_string())
    .bind(bundle_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .bind(data_hash)
    .bind(format)
    .bind(&material_id)
    .bind(actor_id)
    .bind(actor_type.to_string())
    .execute(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            "Failed to store secret version"
        );
        ApiError::internal("internal_error", "Failed to set secrets")
            .with_request_id(request_id.to_string())
    })?;

    Ok(())
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
