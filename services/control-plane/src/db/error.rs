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

    /// Migration directory not found in the current environment.
    #[error("migration directory not found; tried {tried}. Last error: {last_error}. Run from repo root or services/control-plane.")]
    MigrationDirNotFound { tried: String, last_error: String },

    /// Aggregate sequence conflict (optimistic concurrency).
    #[error("aggregate sequence conflict: expected {expected}, got {actual}")]
    SequenceConflict {
        aggregate_id: String,
        expected: i32,
        actual: i32,
    },

    /// Projection checkpoint not found.
    #[error("projection not found: {0}")]
    ProjectionNotFound(String),

    /// Projection lagged beyond a caller-specified timeout.
    #[error("projection '{projection_name}' did not reach event_id {expected} (at {actual}) within timeout")]
    ProjectionTimeout {
        projection_name: String,
        expected: i64,
        actual: i64,
    },

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
