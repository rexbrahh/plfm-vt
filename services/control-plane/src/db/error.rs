//! Database error types.

use thiserror::Error;

/// Database operation errors.
#[derive(Debug, Error)]
pub enum DbError {
    /// Failed to connect to the database.
    #[error("failed to connect to database: {0}")]
    Connect(#[source] sqlx::Error),

    /// Failed to execute a query.
    #[error("query failed: {0}")]
    Query(#[source] sqlx::Error),

    /// Failed to run migrations.
    #[error("migration failed: {0}")]
    Migration(#[source] sqlx::migrate::MigrateError),

    /// Aggregate sequence conflict (optimistic concurrency).
    #[error("aggregate sequence conflict: expected {expected}, got {actual}")]
    SequenceConflict {
        aggregate_id: String,
        expected: i32,
        actual: i32,
    },

    /// Event not found.
    #[error("event not found: {0}")]
    EventNotFound(i64),

    /// Projection checkpoint not found.
    #[error("projection not found: {0}")]
    ProjectionNotFound(String),

    /// Idempotency key conflict.
    #[error("idempotency key reused with different request")]
    IdempotencyConflict {
        org_id: String,
        idempotency_key: String,
    },

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl DbError {
    /// Check if this is a retryable error.
    pub fn is_retryable(&self) -> bool {
        match self {
            DbError::Connect(_) => true,
            DbError::Query(e) => is_retryable_sqlx_error(e),
            _ => false,
        }
    }
}

fn is_retryable_sqlx_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Io(_) => true,
        sqlx::Error::PoolTimedOut => true,
        sqlx::Error::PoolClosed => false,
        sqlx::Error::Database(db_err) => {
            // Postgres error codes that are retryable
            if let Some(code) = db_err.code() {
                matches!(
                    code.as_ref(),
                    "40001" | // serialization_failure
                    "40P01" | // deadlock_detected
                    "57P01" | // admin_shutdown
                    "57P02" | // crash_shutdown
                    "57P03"   // cannot_connect_now
                )
            } else {
                false
            }
        }
        _ => false,
    }
}
