//! Event store for append-only event log operations.
//!
//! The event store provides:
//! - Append events with optimistic concurrency control
//! - Query events by cursor (for projections)
//! - Query events by aggregate (for loading aggregate state)
//! - Query events by org (for tenant-scoped reads)

use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType};
use plfm_id::{AppId, EnvId, EventId, OrgId};
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
        })
    }
}

/// Input for appending a new event.
#[derive(Debug, Clone)]
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
                payload
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
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
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            // Check for unique constraint violation on aggregate_seq
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.code().as_deref() == Some("23505") {
                    // unique_violation
                    return DbError::SequenceConflict {
                        aggregate_id: event.aggregate_id.clone(),
                        expected: event.aggregate_seq,
                        actual: event.aggregate_seq, // We don't know the actual
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
                    payload
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
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
                payload
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
                payload
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
                payload
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
                payload
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

#[cfg(test)]
mod tests {
    use super::*;

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
        };

        assert_eq!(event.event_type, "org.created");
    }
}
