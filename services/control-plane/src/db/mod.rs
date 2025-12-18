//! Database layer for the control plane.
//!
//! This module provides:
//! - Connection pool management
//! - Event store operations (append, query)
//! - Projection checkpoint management
//! - Idempotency record storage
//!
//! The database layer uses SQLx with Postgres.

mod error;
mod event_store;
mod idempotency;
mod projections;

pub use error::DbError;
pub use event_store::{AppendEvent, EventRow, EventStore};
#[allow(unused_imports)]
pub use idempotency::{IdempotencyCheck, IdempotencyRecord, IdempotencyStore};
#[allow(unused_imports)]
pub use projections::{ProjectionCheckpoint, ProjectionStore};

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;
use tracing::info;

/// Database configuration.
#[derive(Debug, Clone)]
pub struct DbConfig {
    /// Database connection URL.
    pub database_url: String,

    /// Maximum number of connections in the pool.
    pub max_connections: u32,

    /// Minimum number of idle connections.
    pub min_connections: u32,

    /// Connection acquire timeout.
    pub acquire_timeout: Duration,

    /// Idle connection timeout.
    pub idle_timeout: Duration,

    /// Maximum lifetime of a connection.
    pub max_lifetime: Duration,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            database_url: "postgres://localhost/plfm".to_string(),
            max_connections: 10,
            min_connections: 1,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(600),
            max_lifetime: Duration::from_secs(1800),
        }
    }
}

impl DbConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://localhost/plfm".to_string());

        let max_connections = std::env::var("DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let min_connections = std::env::var("DB_MIN_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        Self {
            database_url,
            max_connections,
            min_connections,
            ..Default::default()
        }
    }
}

/// Database connection pool wrapper.
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Create a new database connection pool.
    pub async fn connect(config: &DbConfig) -> Result<Self, DbError> {
        info!(
            max_connections = config.max_connections,
            min_connections = config.min_connections,
            "Connecting to database"
        );

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(config.acquire_timeout)
            .idle_timeout(Some(config.idle_timeout))
            .max_lifetime(Some(config.max_lifetime))
            .connect(&config.database_url)
            .await
            .map_err(DbError::Connect)?;

        info!("Database connection pool established");

        Ok(Self { pool })
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Check if the database is reachable.
    pub async fn health_check(&self) -> Result<(), DbError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(DbError::Query)?;
        Ok(())
    }

    /// Run pending migrations.
    ///
    /// Note: In production, migrations should be run via a separate migration tool
    /// or as part of deployment. This method uses runtime migration loading.
    pub async fn run_migrations(&self) -> Result<(), DbError> {
        info!("Running database migrations");
        
        // Use runtime migration loading instead of compile-time macro
        // to avoid requiring a database connection during compilation.
        let migrator = sqlx::migrate::Migrator::new(std::path::Path::new("./migrations"))
            .await
            .map_err(DbError::Migration)?;
        
        migrator.run(&self.pool).await.map_err(DbError::Migration)?;
        
        info!("Database migrations complete");
        Ok(())
    }

    /// Get an event store handle.
    pub fn event_store(&self) -> EventStore {
        EventStore::new(self.pool.clone())
    }

    /// Get a projection store handle.
    pub fn projection_store(&self) -> ProjectionStore {
        ProjectionStore::new(self.pool.clone())
    }

    /// Get an idempotency store handle.
    pub fn idempotency_store(&self) -> IdempotencyStore {
        IdempotencyStore::new(self.pool.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_config_defaults() {
        let config = DbConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 1);
    }
}
