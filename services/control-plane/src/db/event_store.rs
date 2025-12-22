//! Event store for append-only event log operations.
//!
//! The event store provides:
//! - Append events with optimistic concurrency control
//! - Query events by cursor (for projections)
//! - Query events by aggregate (for loading aggregate state)
//! - Query events by org (for tenant-scoped reads)

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use plfm_events::{event_types, ActorType, AggregateType};
use plfm_id::{AppId, EnvId, EventId, OrgId};
use plfm_proto::FILE_DESCRIPTOR_SET;
use prost_012::Message;
use prost_reflect::{
    DescriptorPool, DeserializeOptions, DynamicMessage, EnumDescriptor, FieldDescriptor, Kind,
    MessageDescriptor,
};
use sqlx::{postgres::PgPool, postgres::PgRow, Row};

use super::DbError;

/// A row from the events table.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub event_id: i64,
    pub occurred_at: DateTime<Utc>,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub aggregate_seq: i32,
    pub event_type: String,
    pub event_version: i32,
    pub actor_type: String,
    pub actor_id: String,
    pub org_id: Option<String>,
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub app_id: Option<String>,
    pub env_id: Option<String>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<i64>,
    pub payload: serde_json::Value,
    pub payload_type_url: Option<String>,
    pub payload_bytes: Option<Vec<u8>>,
    pub payload_schema_version: Option<i32>,
    pub traceparent: Option<String>,
    pub tags: Option<serde_json::Value>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for EventRow {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            event_id: row.try_get("event_id")?,
            occurred_at: row.try_get("occurred_at")?,
            aggregate_type: row.try_get("aggregate_type")?,
            aggregate_id: row.try_get("aggregate_id")?,
            aggregate_seq: row.try_get("aggregate_seq")?,
            event_type: row.try_get("event_type")?,
            event_version: row.try_get("event_version")?,
            actor_type: row.try_get("actor_type")?,
            actor_id: row.try_get("actor_id")?,
            org_id: row.try_get("org_id")?,
            request_id: row.try_get("request_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            correlation_id: row.try_get("correlation_id")?,
            causation_id: row.try_get("causation_id")?,
            payload: row.try_get("payload")?,
            payload_type_url: row.try_get("payload_type_url").ok(),
            payload_bytes: row.try_get("payload_bytes").ok(),
            payload_schema_version: row.try_get("payload_schema_version").ok(),
            traceparent: row.try_get("traceparent").ok(),
            tags: row.try_get("tags").ok(),
        })
    }
}

/// Input for appending a new event.
#[derive(Debug, Clone, Default)]
pub struct AppendEvent {
    pub aggregate_type: AggregateType,
    pub aggregate_id: String,
    pub aggregate_seq: i32,
    pub event_type: String,
    pub event_version: i32,
    pub actor_type: ActorType,
    pub actor_id: String,
    pub org_id: Option<OrgId>,
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub app_id: Option<AppId>,
    pub env_id: Option<EnvId>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<EventId>,
    pub payload: serde_json::Value,
    #[doc(hidden)]
    pub payload_type_url: Option<String>,
    #[doc(hidden)]
    pub payload_bytes: Option<Vec<u8>>,
    #[doc(hidden)]
    pub payload_schema_version: Option<i32>,
    #[doc(hidden)]
    pub traceparent: Option<String>,
    #[doc(hidden)]
    pub tags: Option<serde_json::Value>,
}

/// Event store for managing the append-only event log.
#[derive(Clone)]
pub struct EventStore {
    pool: PgPool,
}

impl EventStore {
    /// Create a new event store.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Append a single event to the log.
    ///
    /// Returns the assigned event_id.
    ///
    /// # Errors
    ///
    /// Returns `DbError::SequenceConflict` if the aggregate_seq already exists
    /// for this aggregate (optimistic concurrency violation).
    pub async fn append(&self, event: AppendEvent) -> Result<EventId, DbError> {
        let mut event = event;
        populate_protobuf_payload(&mut event)?;
        let result = sqlx::query(
            r#"
            INSERT INTO events (
                aggregate_type,
                aggregate_id,
                aggregate_seq,
                event_type,
                event_version,
                actor_type,
                actor_id,
                org_id,
                request_id,
                idempotency_key,
                app_id,
                env_id,
                correlation_id,
                causation_id,
                payload,
                payload_type_url,
                payload_bytes,
                payload_schema_version,
                traceparent,
                tags
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
            RETURNING event_id
            "#,
        )
        .bind(event.aggregate_type.to_string())
        .bind(&event.aggregate_id)
        .bind(event.aggregate_seq)
        .bind(&event.event_type)
        .bind(event.event_version)
        .bind(event.actor_type.to_string())
        .bind(&event.actor_id)
        .bind(event.org_id.as_ref().map(|id| id.to_string()))
        .bind(&event.request_id)
        .bind(&event.idempotency_key)
        .bind(event.app_id.as_ref().map(|id| id.to_string()))
        .bind(event.env_id.as_ref().map(|id| id.to_string()))
        .bind(&event.correlation_id)
        .bind(event.causation_id.map(|id| id.value()))
        .bind(&event.payload)
        .bind(&event.payload_type_url)
        .bind(&event.payload_bytes)
        .bind(event.payload_schema_version)
        .bind(&event.traceparent)
        .bind(&event.tags)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.code().as_deref() == Some("23505") {
                    return DbError::SequenceConflict {
                        aggregate_id: event.aggregate_id.clone(),
                        expected: event.aggregate_seq,
                        actual: event.aggregate_seq,
                    };
                }
            }
            DbError::Query(e)
        })?;

        let event_id: i64 = result.get("event_id");
        Ok(EventId::new(event_id))
    }

    /// Append multiple events atomically.
    ///
    /// All events must be for the same transaction context.
    /// Returns the assigned event_ids.
    pub async fn append_batch(&self, events: Vec<AppendEvent>) -> Result<Vec<EventId>, DbError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        let mut events = events;
        for event in &mut events {
            populate_protobuf_payload(event)?;
        }

        let mut tx = self.pool.begin().await.map_err(DbError::Query)?;
        let mut event_ids = Vec::with_capacity(events.len());

        for event in events {
            let result = sqlx::query(
                r#"
                INSERT INTO events (
                    aggregate_type,
                    aggregate_id,
                    aggregate_seq,
                    event_type,
                    event_version,
                    actor_type,
                    actor_id,
                    org_id,
                    request_id,
                    idempotency_key,
                    app_id,
                    env_id,
                    correlation_id,
                    causation_id,
                    payload,
                    payload_type_url,
                    payload_bytes,
                    payload_schema_version,
                    traceparent,
                    tags
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
                RETURNING event_id
                "#,
            )
            .bind(event.aggregate_type.to_string())
            .bind(&event.aggregate_id)
            .bind(event.aggregate_seq)
            .bind(&event.event_type)
            .bind(event.event_version)
            .bind(event.actor_type.to_string())
            .bind(&event.actor_id)
            .bind(event.org_id.as_ref().map(|id| id.to_string()))
            .bind(&event.request_id)
            .bind(&event.idempotency_key)
            .bind(event.app_id.as_ref().map(|id| id.to_string()))
            .bind(event.env_id.as_ref().map(|id| id.to_string()))
            .bind(&event.correlation_id)
            .bind(event.causation_id.map(|id| id.value()))
            .bind(&event.payload)
            .bind(&event.payload_type_url)
            .bind(&event.payload_bytes)
            .bind(event.payload_schema_version)
            .bind(&event.traceparent)
            .bind(&event.tags)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(ref db_err) = e {
                    if db_err.code().as_deref() == Some("23505") {
                        return DbError::SequenceConflict {
                            aggregate_id: event.aggregate_id.clone(),
                            expected: event.aggregate_seq,
                            actual: event.aggregate_seq,
                        };
                    }
                }
                DbError::Query(e)
            })?;

            let event_id: i64 = result.get("event_id");
            event_ids.push(EventId::new(event_id));
        }

        tx.commit().await.map_err(DbError::Query)?;
        Ok(event_ids)
    }

    /// Query events after a given cursor.
    ///
    /// Returns events in ascending event_id order.
    /// This is the primary interface for projections.
    pub async fn query_after_cursor(
        &self,
        after_event_id: i64,
        limit: i32,
    ) -> Result<Vec<EventRow>, DbError> {
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT
                event_id,
                occurred_at,
                aggregate_type,
                aggregate_id,
                aggregate_seq,
                event_type,
                event_version,
                actor_type,
                actor_id,
                org_id,
                request_id,
                idempotency_key,
                app_id,
                env_id,
                correlation_id,
                causation_id,
                payload,
                payload_type_url,
                payload_bytes,
                payload_schema_version,
                traceparent,
                tags
            FROM events
            WHERE event_id > $1
            ORDER BY event_id ASC
            LIMIT $2
            "#,
        )
        .bind(after_event_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(rows)
    }

    /// Query events for a specific aggregate.
    ///
    /// Returns events in ascending aggregate_seq order.
    pub async fn query_by_aggregate(
        &self,
        aggregate_type: &AggregateType,
        aggregate_id: &str,
    ) -> Result<Vec<EventRow>, DbError> {
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT
                event_id,
                occurred_at,
                aggregate_type,
                aggregate_id,
                aggregate_seq,
                event_type,
                event_version,
                actor_type,
                actor_id,
                org_id,
                request_id,
                idempotency_key,
                app_id,
                env_id,
                correlation_id,
                causation_id,
                payload,
                payload_type_url,
                payload_bytes,
                payload_schema_version,
                traceparent,
                tags
            FROM events
            WHERE aggregate_type = $1 AND aggregate_id = $2
            ORDER BY aggregate_seq ASC
            "#,
        )
        .bind(aggregate_type.to_string())
        .bind(aggregate_id)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(rows)
    }

    /// Get the latest aggregate sequence number.
    ///
    /// Returns None if no events exist for the aggregate.
    pub async fn get_latest_aggregate_seq(
        &self,
        aggregate_type: &AggregateType,
        aggregate_id: &str,
    ) -> Result<Option<i32>, DbError> {
        let result = sqlx::query(
            r#"
            SELECT MAX(aggregate_seq) as max_seq
            FROM events
            WHERE aggregate_type = $1 AND aggregate_id = $2
            "#,
        )
        .bind(aggregate_type.to_string())
        .bind(aggregate_id)
        .fetch_one(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(result.get("max_seq"))
    }

    /// Query events for an organization after a cursor.
    ///
    /// Used for org-scoped streaming/audit.
    pub async fn query_by_org_after_cursor(
        &self,
        org_id: &OrgId,
        after_event_id: i64,
        limit: i32,
    ) -> Result<Vec<EventRow>, DbError> {
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT
                event_id,
                occurred_at,
                aggregate_type,
                aggregate_id,
                aggregate_seq,
                event_type,
                event_version,
                actor_type,
                actor_id,
                org_id,
                request_id,
                idempotency_key,
                app_id,
                env_id,
                correlation_id,
                causation_id,
                payload,
                payload_type_url,
                payload_bytes,
                payload_schema_version,
                traceparent,
                tags
            FROM events
            WHERE org_id = $1 AND event_id > $2
            ORDER BY event_id ASC
            LIMIT $3
            "#,
        )
        .bind(org_id.to_string())
        .bind(after_event_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(rows)
    }

    /// Query events by type after a cursor.
    ///
    /// Used for type-filtered streaming.
    pub async fn query_by_type_after_cursor(
        &self,
        event_type: &str,
        after_event_id: i64,
        limit: i32,
    ) -> Result<Vec<EventRow>, DbError> {
        let rows = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT
                event_id,
                occurred_at,
                aggregate_type,
                aggregate_id,
                aggregate_seq,
                event_type,
                event_version,
                actor_type,
                actor_id,
                org_id,
                request_id,
                idempotency_key,
                app_id,
                env_id,
                correlation_id,
                causation_id,
                payload,
                payload_type_url,
                payload_bytes,
                payload_schema_version,
                traceparent,
                tags
            FROM events
            WHERE event_type = $1 AND event_id > $2
            ORDER BY event_id ASC
            LIMIT $3
            "#,
        )
        .bind(event_type)
        .bind(after_event_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(rows)
    }

    /// Get the current max event_id.
    ///
    /// Returns 0 if no events exist.
    pub async fn get_max_event_id(&self) -> Result<i64, DbError> {
        let result = sqlx::query("SELECT COALESCE(MAX(event_id), 0) as max_id FROM events")
            .fetch_one(&self.pool)
            .await
            .map_err(DbError::Query)?;

        Ok(result.get("max_id"))
    }
}

fn populate_protobuf_payload(event: &mut AppendEvent) -> Result<(), DbError> {
    if event.payload_bytes.is_some() && event.payload_type_url.is_some() {
        return Ok(());
    }

    let type_url = event
        .payload_type_url
        .clone()
        .or_else(|| payload_type_url_for_event(&event.event_type).map(str::to_string))
        .ok_or_else(|| {
            DbError::InvalidPayload(format!(
                "missing payload type url for event_type {}",
                event.event_type
            ))
        })?;

    let payload_bytes = encode_payload_bytes(&type_url, &event.payload)?;

    event.payload_type_url = Some(type_url);
    event.payload_bytes = Some(payload_bytes);
    if event.payload_schema_version.is_none() {
        event.payload_schema_version = Some(1);
    }

    Ok(())
}

fn payload_type_url_for_event(event_type: &str) -> Option<&'static str> {
    match event_type {
        event_types::ORG_CREATED => Some("type.googleapis.com/plfm.events.v1.OrgCreatedPayload"),
        event_types::ORG_UPDATED => Some("type.googleapis.com/plfm.events.v1.OrgUpdatedPayload"),
        event_types::ORG_MEMBER_ADDED => {
            Some("type.googleapis.com/plfm.events.v1.OrgMemberAddedPayload")
        }
        event_types::ORG_MEMBER_ROLE_UPDATED => {
            Some("type.googleapis.com/plfm.events.v1.OrgMemberRoleUpdatedPayload")
        }
        event_types::ORG_MEMBER_REMOVED => {
            Some("type.googleapis.com/plfm.events.v1.OrgMemberRemovedPayload")
        }
        event_types::SERVICE_PRINCIPAL_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.ServicePrincipalCreatedPayload")
        }
        event_types::SERVICE_PRINCIPAL_SCOPES_UPDATED => {
            Some("type.googleapis.com/plfm.events.v1.ServicePrincipalScopesUpdatedPayload")
        }
        event_types::SERVICE_PRINCIPAL_SECRET_ROTATED => {
            Some("type.googleapis.com/plfm.events.v1.ServicePrincipalSecretRotatedPayload")
        }
        event_types::SERVICE_PRINCIPAL_DELETED => {
            Some("type.googleapis.com/plfm.events.v1.ServicePrincipalDeletedPayload")
        }
        event_types::PROJECT_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.ProjectCreatedPayload")
        }
        event_types::PROJECT_UPDATED => {
            Some("type.googleapis.com/plfm.events.v1.ProjectUpdatedPayload")
        }
        event_types::PROJECT_DELETED => {
            Some("type.googleapis.com/plfm.events.v1.ProjectDeletedPayload")
        }
        event_types::APP_CREATED => Some("type.googleapis.com/plfm.events.v1.AppCreatedPayload"),
        event_types::APP_UPDATED => Some("type.googleapis.com/plfm.events.v1.AppUpdatedPayload"),
        event_types::APP_DELETED => Some("type.googleapis.com/plfm.events.v1.AppDeletedPayload"),
        event_types::ENV_CREATED => Some("type.googleapis.com/plfm.events.v1.EnvCreatedPayload"),
        event_types::ENV_UPDATED => Some("type.googleapis.com/plfm.events.v1.EnvUpdatedPayload"),
        event_types::ENV_DELETED => Some("type.googleapis.com/plfm.events.v1.EnvDeletedPayload"),
        event_types::ENV_SCALE_SET => Some("type.googleapis.com/plfm.events.v1.EnvScaleSetPayload"),
        event_types::ENV_DESIRED_RELEASE_SET => {
            Some("type.googleapis.com/plfm.events.v1.EnvDesiredReleaseSetPayload")
        }
        event_types::ENV_IPV4_ADDON_ENABLED => {
            Some("type.googleapis.com/plfm.events.v1.EnvIpv4AddonEnabledPayload")
        }
        event_types::ENV_IPV4_ADDON_DISABLED => {
            Some("type.googleapis.com/plfm.events.v1.EnvIpv4AddonDisabledPayload")
        }
        event_types::RELEASE_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.ReleaseCreatedPayload")
        }
        event_types::DEPLOY_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.DeployCreatedPayload")
        }
        event_types::DEPLOY_STATUS_CHANGED => {
            Some("type.googleapis.com/plfm.events.v1.DeployStatusChangedPayload")
        }
        event_types::ROUTE_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.RouteCreatedPayload")
        }
        event_types::ROUTE_UPDATED => {
            Some("type.googleapis.com/plfm.events.v1.RouteUpdatedPayload")
        }
        event_types::ROUTE_DELETED => {
            Some("type.googleapis.com/plfm.events.v1.RouteDeletedPayload")
        }
        event_types::SECRET_BUNDLE_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.SecretBundleCreatedPayload")
        }
        event_types::SECRET_BUNDLE_VERSION_SET => {
            Some("type.googleapis.com/plfm.events.v1.SecretBundleVersionSetPayload")
        }
        event_types::VOLUME_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.VolumeCreatedPayload")
        }
        event_types::VOLUME_DELETED => {
            Some("type.googleapis.com/plfm.events.v1.VolumeDeletedPayload")
        }
        event_types::VOLUME_ATTACHMENT_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.VolumeAttachmentCreatedPayload")
        }
        event_types::VOLUME_ATTACHMENT_DELETED => {
            Some("type.googleapis.com/plfm.events.v1.VolumeAttachmentDeletedPayload")
        }
        event_types::SNAPSHOT_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.SnapshotCreatedPayload")
        }
        event_types::SNAPSHOT_STATUS_CHANGED => {
            Some("type.googleapis.com/plfm.events.v1.SnapshotStatusChangedPayload")
        }
        event_types::RESTORE_JOB_CREATED => {
            Some("type.googleapis.com/plfm.events.v1.RestoreJobCreatedPayload")
        }
        event_types::RESTORE_JOB_STATUS_CHANGED => {
            Some("type.googleapis.com/plfm.events.v1.RestoreJobStatusChangedPayload")
        }
        event_types::INSTANCE_ALLOCATED => {
            Some("type.googleapis.com/plfm.events.v1.InstanceAllocatedPayload")
        }
        event_types::INSTANCE_DESIRED_STATE_CHANGED => {
            Some("type.googleapis.com/plfm.events.v1.InstanceDesiredStateChangedPayload")
        }
        event_types::INSTANCE_STATUS_CHANGED => {
            Some("type.googleapis.com/plfm.events.v1.InstanceStatusChangedPayload")
        }
        event_types::NODE_ENROLLED => {
            Some("type.googleapis.com/plfm.events.v1.NodeEnrolledPayload")
        }
        event_types::NODE_STATE_CHANGED => {
            Some("type.googleapis.com/plfm.events.v1.NodeStateChangedPayload")
        }
        event_types::NODE_CAPACITY_UPDATED => {
            Some("type.googleapis.com/plfm.events.v1.NodeCapacityUpdatedPayload")
        }
        event_types::EXEC_SESSION_GRANTED => {
            Some("type.googleapis.com/plfm.events.v1.ExecSessionGrantedPayload")
        }
        event_types::EXEC_SESSION_CONNECTED => {
            Some("type.googleapis.com/plfm.events.v1.ExecSessionConnectedPayload")
        }
        event_types::EXEC_SESSION_ENDED => {
            Some("type.googleapis.com/plfm.events.v1.ExecSessionEndedPayload")
        }
        _ => None,
    }
}

fn encode_payload_bytes(type_url: &str, payload: &serde_json::Value) -> Result<Vec<u8>, DbError> {
    let pool = descriptor_pool()?;
    let message_name = type_url.rsplit('/').next().unwrap_or(type_url);
    let descriptor = pool
        .get_message_by_name(message_name)
        .ok_or_else(|| DbError::InvalidPayload(format!("unknown payload type {message_name}")))?;

    let canonical = canonicalize_payload_json(&descriptor, payload)?;
    let json_bytes = serde_json::to_vec(&canonical)?;
    let mut deserializer = serde_json::Deserializer::from_slice(&json_bytes);
    let options = DeserializeOptions::new().deny_unknown_fields(false);
    let message = DynamicMessage::deserialize_with_options(descriptor, &mut deserializer, &options)
        .map_err(|e| DbError::InvalidPayload(format!("payload decode failed: {e}")))?;
    deserializer
        .end()
        .map_err(|e| DbError::InvalidPayload(format!("payload decode failed: {e}")))?;

    Ok(message.encode_to_vec())
}

fn descriptor_pool() -> Result<&'static DescriptorPool, DbError> {
    static DESCRIPTORS: OnceLock<Option<DescriptorPool>> = OnceLock::new();
    DESCRIPTORS
        .get_or_init(|| DescriptorPool::decode(FILE_DESCRIPTOR_SET).ok())
        .as_ref()
        .ok_or_else(|| DbError::InvalidPayload("descriptor pool unavailable".to_string()))
}

fn canonicalize_payload_json(
    descriptor: &MessageDescriptor,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, DbError> {
    let obj = match payload.as_object() {
        Some(obj) => obj,
        None if payload.is_null() => return Ok(serde_json::json!({})),
        None => {
            return Err(DbError::InvalidPayload(format!(
                "payload for {} must be an object",
                descriptor.full_name()
            )))
        }
    };

    let mut out = serde_json::Map::new();
    for (key, value) in obj {
        if value.is_null() {
            continue;
        }

        let Some(field) = resolve_field(descriptor, key) else {
            tracing::warn!(
                payload_type = %descriptor.full_name(),
                field = %key,
                "unknown payload field"
            );
            continue;
        };

        if let Some(normalized) = canonicalize_field_value(&field, value)? {
            out.insert(field.json_name().to_string(), normalized);
        }
    }

    Ok(serde_json::Value::Object(out))
}

fn resolve_field(descriptor: &MessageDescriptor, key: &str) -> Option<FieldDescriptor> {
    descriptor
        .get_field_by_json_name(key)
        .or_else(|| descriptor.get_field_by_name(key))
        .or_else(|| descriptor.get_field_by_json_name(&snake_to_lower_camel(key)))
}

fn canonicalize_field_value(
    field: &FieldDescriptor,
    value: &serde_json::Value,
) -> Result<Option<serde_json::Value>, DbError> {
    if value.is_null() {
        return Ok(None);
    }

    if field.is_list() {
        let Some(items) = value.as_array() else {
            return Err(DbError::InvalidPayload(format!(
                "field {} expects array",
                field.full_name()
            )));
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            if item.is_null() {
                continue;
            }
            out.push(canonicalize_value_by_kind(&field.kind(), item)?);
        }
        return Ok(Some(serde_json::Value::Array(out)));
    }

    if field.is_map() {
        let Some(items) = value.as_object() else {
            return Err(DbError::InvalidPayload(format!(
                "field {} expects object",
                field.full_name()
            )));
        };
        let Kind::Message(entry_desc) = field.kind() else {
            return Err(DbError::InvalidPayload(format!(
                "field {} is not a map entry",
                field.full_name()
            )));
        };
        let value_desc = entry_desc
            .get_field_by_name("value")
            .ok_or_else(|| DbError::InvalidPayload("map value missing".to_string()))?;
        let mut out = serde_json::Map::new();
        for (key, item) in items {
            let value = canonicalize_value_by_kind(&value_desc.kind(), item)?;
            out.insert(key.clone(), value);
        }
        return Ok(Some(serde_json::Value::Object(out)));
    }

    Ok(Some(canonicalize_value_by_kind(&field.kind(), value)?))
}

fn canonicalize_value_by_kind(
    kind: &Kind,
    value: &serde_json::Value,
) -> Result<serde_json::Value, DbError> {
    match kind {
        Kind::Bool => value
            .as_bool()
            .map(serde_json::Value::Bool)
            .ok_or_else(|| DbError::InvalidPayload("expected bool".to_string())),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => json_i32(value)
            .map(|v| serde_json::Value::Number(v.into()))
            .ok_or_else(|| DbError::InvalidPayload("expected int32".to_string())),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => json_i64(value)
            .map(|v| serde_json::Value::String(v.to_string()))
            .ok_or_else(|| DbError::InvalidPayload("expected int64".to_string())),
        Kind::Uint32 | Kind::Fixed32 => json_u32(value)
            .map(|v| serde_json::Value::Number(v.into()))
            .ok_or_else(|| DbError::InvalidPayload("expected uint32".to_string())),
        Kind::Uint64 | Kind::Fixed64 => json_u64(value)
            .map(|v| serde_json::Value::String(v.to_string()))
            .ok_or_else(|| DbError::InvalidPayload("expected uint64".to_string())),
        Kind::Float => {
            let value = json_f64(value)
                .ok_or_else(|| DbError::InvalidPayload("expected float".to_string()))?;
            let number = serde_json::Number::from_f64(value)
                .ok_or_else(|| DbError::InvalidPayload("invalid float".to_string()))?;
            Ok(serde_json::Value::Number(number))
        }
        Kind::Double => {
            let value = json_f64(value)
                .ok_or_else(|| DbError::InvalidPayload("expected double".to_string()))?;
            let number = serde_json::Number::from_f64(value)
                .ok_or_else(|| DbError::InvalidPayload("invalid double".to_string()))?;
            Ok(serde_json::Value::Number(number))
        }
        Kind::String => value
            .as_str()
            .map(|v| serde_json::Value::String(v.to_string()))
            .ok_or_else(|| DbError::InvalidPayload("expected string".to_string())),
        Kind::Bytes => value
            .as_str()
            .map(|v| serde_json::Value::String(v.to_string()))
            .ok_or_else(|| DbError::InvalidPayload("expected bytes".to_string())),
        Kind::Enum(enum_desc) => {
            canonicalize_enum_value(enum_desc, value).map(serde_json::Value::String)
        }
        Kind::Message(message_desc) => {
            if is_well_known_string_message(message_desc) {
                return value
                    .as_str()
                    .map(|v| serde_json::Value::String(v.to_string()))
                    .ok_or_else(|| DbError::InvalidPayload("expected string".to_string()));
            }
            let obj = canonicalize_payload_json(message_desc, value)?;
            Ok(obj)
        }
    }
}

fn canonicalize_enum_value(
    enum_desc: &EnumDescriptor,
    value: &serde_json::Value,
) -> Result<String, DbError> {
    if let Some(name) = value.as_str() {
        if let Some(found) = enum_desc.get_value_by_name(name) {
            return Ok(found.name().to_string());
        }
        let candidate = normalize_enum_candidate(name);
        if let Some(found) = enum_desc.get_value_by_name(&candidate) {
            return Ok(found.name().to_string());
        }
        let suffix = format!("_{candidate}");
        let matches = enum_desc
            .values()
            .filter(|value| value.name().ends_with(&suffix))
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            return Ok(matches[0].name().to_string());
        }
        return Err(DbError::InvalidPayload(format!(
            "unknown enum value {name} for {}",
            enum_desc.full_name()
        )));
    }

    if let Some(number) = json_i32(value) {
        if let Some(found) = enum_desc.get_value(number) {
            return Ok(found.name().to_string());
        }
        return Err(DbError::InvalidPayload(format!(
            "unknown enum value {number} for {}",
            enum_desc.full_name()
        )));
    }

    Err(DbError::InvalidPayload(format!(
        "invalid enum value for {}",
        enum_desc.full_name()
    )))
}

fn normalize_enum_candidate(input: &str) -> String {
    let mut out = String::new();
    let mut prev_is_lower_or_digit = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if prev_is_lower_or_digit && ch.is_ascii_uppercase() {
                out.push('_');
            }
            out.push(ch.to_ascii_uppercase());
            prev_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else if !out.ends_with('_') {
            out.push('_');
            prev_is_lower_or_digit = false;
        }
    }
    out.trim_matches('_').to_string()
}

fn snake_to_lower_camel(input: &str) -> String {
    let mut parts = input.split('_');
    let Some(first) = parts.next() else {
        return String::new();
    };
    let mut out = String::from(first);
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first_char) = chars.next() {
            out.push(first_char.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    out
}

fn is_well_known_string_message(descriptor: &MessageDescriptor) -> bool {
    matches!(
        descriptor.full_name(),
        "google.protobuf.Timestamp" | "google.protobuf.Duration"
    )
}

fn json_i32(value: &serde_json::Value) -> Option<i32> {
    json_i64(value).and_then(|v| i32::try_from(v).ok())
}

fn json_i64(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn json_u32(value: &serde_json::Value) -> Option<u32> {
    json_u64(value).and_then(|v| u32::try_from(v).ok())
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plfm_proto::events::v1::OrgCreatedPayload;
    use prost::Message;

    #[test]
    fn test_append_event_construction() {
        let event = AppendEvent {
            aggregate_type: AggregateType::Org,
            aggregate_id: "org_123".to_string(),
            aggregate_seq: 1,
            event_type: "org.created".to_string(),
            event_version: 1,
            actor_type: ActorType::User,
            actor_id: "user_456".to_string(),
            org_id: None,
            request_id: "req_789".to_string(),
            idempotency_key: Some("idem_abc".to_string()),
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: serde_json::json!({"name": "Test Org"}),
            payload_type_url: None,
            payload_bytes: None,
            payload_schema_version: None,
            traceparent: None,
            tags: None,
        };

        assert_eq!(event.event_type, "org.created");
    }

    #[test]
    fn test_populate_protobuf_payload_sets_bytes() {
        let mut event = AppendEvent {
            aggregate_type: AggregateType::Org,
            aggregate_id: "org_123".to_string(),
            aggregate_seq: 1,
            event_type: event_types::ORG_CREATED.to_string(),
            event_version: 1,
            actor_type: ActorType::User,
            actor_id: "user_456".to_string(),
            org_id: None,
            request_id: "req_789".to_string(),
            idempotency_key: None,
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: serde_json::json!({
                "org_id": "org_123",
                "name": "Acme"
            }),
            payload_type_url: None,
            payload_bytes: None,
            payload_schema_version: None,
            traceparent: None,
            tags: None,
        };

        populate_protobuf_payload(&mut event).expect("payload bytes");

        assert_eq!(
            event.payload_type_url.as_deref(),
            Some("type.googleapis.com/plfm.events.v1.OrgCreatedPayload")
        );
        let bytes = event.payload_bytes.expect("payload bytes missing");
        let decoded = OrgCreatedPayload::decode(bytes.as_slice()).expect("decode");
        assert_eq!(decoded.org_id, "org_123");
        assert_eq!(decoded.name, "Acme");
    }
}
