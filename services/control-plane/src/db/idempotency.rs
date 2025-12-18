//! Idempotency record storage.
//!
//! Idempotency records store command responses for deduplication.
//! If a command is retried with the same idempotency key, we return the stored response.
//! If the key is reused with a different request, we return 409 Conflict.

use chrono::{DateTime, Utc};
use sqlx::{postgres::PgPool, postgres::PgRow, Row};

use super::DbError;

/// An idempotency record.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub org_id: String,
    pub actor_id: String,
    pub endpoint_name: String,
    pub idempotency_key: String,
    pub request_hash: String,
    pub response_status_code: i32,
    pub response_body: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for IdempotencyRecord {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            org_id: row.try_get("org_id")?,
            actor_id: row.try_get("actor_id")?,
            endpoint_name: row.try_get("endpoint_name")?,
            idempotency_key: row.try_get("idempotency_key")?,
            request_hash: row.try_get("request_hash")?,
            response_status_code: row.try_get("response_status_code")?,
            response_body: row.try_get("response_body")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

/// Result of checking for an existing idempotency record.
#[derive(Debug)]
pub enum IdempotencyCheck {
    /// No existing record found - proceed with the request.
    NotFound,
    /// Found matching record - return the cached response.
    Found(IdempotencyRecord),
    /// Found record with different request hash - conflict.
    Conflict,
}

/// Store for managing idempotency records.
#[derive(Clone)]
pub struct IdempotencyStore {
    pool: PgPool,
}

impl IdempotencyStore {
    /// Create a new idempotency store.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Check for an existing idempotency record.
    ///
    /// Returns:
    /// - `NotFound` if no record exists
    /// - `Found(record)` if a matching record exists (same request_hash)
    /// - `Conflict` if a record exists with a different request_hash
    pub async fn check(
        &self,
        org_id: &str,
        actor_id: &str,
        endpoint_name: &str,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<IdempotencyCheck, DbError> {
        let record = sqlx::query_as::<_, IdempotencyRecord>(
            r#"
            SELECT 
                org_id,
                actor_id,
                endpoint_name,
                idempotency_key,
                request_hash,
                response_status_code,
                response_body,
                created_at
            FROM idempotency_records
            WHERE org_id = $1 
              AND actor_id = $2 
              AND endpoint_name = $3 
              AND idempotency_key = $4
            "#,
        )
        .bind(org_id)
        .bind(actor_id)
        .bind(endpoint_name)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::Query)?;

        match record {
            None => Ok(IdempotencyCheck::NotFound),
            Some(r) => {
                if r.request_hash == request_hash {
                    Ok(IdempotencyCheck::Found(r))
                } else {
                    Ok(IdempotencyCheck::Conflict)
                }
            }
        }
    }

    /// Store a new idempotency record.
    ///
    /// This should be called after successfully processing a request.
    pub async fn store(
        &self,
        org_id: &str,
        actor_id: &str,
        endpoint_name: &str,
        idempotency_key: &str,
        request_hash: &str,
        response_status_code: i32,
        response_body: Option<serde_json::Value>,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO idempotency_records (
                org_id,
                actor_id,
                endpoint_name,
                idempotency_key,
                request_hash,
                response_status_code,
                response_body
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (org_id, actor_id, endpoint_name, idempotency_key) 
            DO NOTHING
            "#,
        )
        .bind(org_id)
        .bind(actor_id)
        .bind(endpoint_name)
        .bind(idempotency_key)
        .bind(request_hash)
        .bind(response_status_code)
        .bind(response_body)
        .execute(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(())
    }

    /// Delete expired idempotency records.
    ///
    /// Records older than the specified duration are deleted.
    /// The spec requires minimum 24 hour retention.
    pub async fn cleanup_expired(&self, max_age_hours: i32) -> Result<u64, DbError> {
        let result = sqlx::query(
            r#"
            DELETE FROM idempotency_records
            WHERE created_at < now() - ($1 || ' hours')::interval
            "#,
        )
        .bind(max_age_hours)
        .execute(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idempotency_check_variants() {
        // Just verify the enum compiles
        let check = IdempotencyCheck::NotFound;
        assert!(matches!(check, IdempotencyCheck::NotFound));
    }
}
