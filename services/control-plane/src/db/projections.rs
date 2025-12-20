//! Projection checkpoint management.
//!
//! Each projection maintains a durable checkpoint of the last applied event_id.
//! On startup, projections resume from (last_applied_event_id + 1).

use chrono::{DateTime, Utc};
use sqlx::{postgres::PgPool, postgres::PgRow, Row};
use tokio::time::{sleep, Duration, Instant};

use super::DbError;

/// A projection checkpoint record.
#[derive(Debug, Clone)]
pub struct ProjectionCheckpoint {
    pub projection_name: String,
    pub last_applied_event_id: i64,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, PgRow> for ProjectionCheckpoint {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            projection_name: row.try_get("projection_name")?,
            last_applied_event_id: row.try_get("last_applied_event_id")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Store for managing projection checkpoints.
#[derive(Clone)]
pub struct ProjectionStore {
    pool: PgPool,
}

impl ProjectionStore {
    /// Create a new projection store.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get the checkpoint for a projection.
    pub async fn get_checkpoint(
        &self,
        projection_name: &str,
    ) -> Result<ProjectionCheckpoint, DbError> {
        let checkpoint = sqlx::query_as::<_, ProjectionCheckpoint>(
            r#"
            SELECT projection_name, last_applied_event_id, updated_at
            FROM projection_checkpoints
            WHERE projection_name = $1
            "#,
        )
        .bind(projection_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::Query)?
        .ok_or_else(|| DbError::ProjectionNotFound(projection_name.to_string()))?;

        Ok(checkpoint)
    }

    /// Update the checkpoint for a projection.
    ///
    /// This should be called after successfully applying events.
    pub async fn update_checkpoint(
        &self,
        projection_name: &str,
        last_applied_event_id: i64,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO projection_checkpoints (projection_name, last_applied_event_id, updated_at)
            VALUES ($1, $2, now())
            ON CONFLICT (projection_name)
            DO UPDATE SET last_applied_event_id = EXCLUDED.last_applied_event_id, updated_at = now()
            "#,
        )
        .bind(projection_name)
        .bind(last_applied_event_id)
        .execute(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(())
    }

    /// Ensure a projection checkpoint row exists without lowering progress.
    pub async fn ensure_checkpoint(&self, projection_name: &str) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO projection_checkpoints (projection_name, last_applied_event_id, updated_at)
            VALUES ($1, 0, now())
            ON CONFLICT (projection_name) DO NOTHING
            "#,
        )
        .bind(projection_name)
        .execute(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(())
    }

    /// Update checkpoint atomically with view updates.
    ///
    /// This is used when applying events within a transaction.
    pub async fn update_checkpoint_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        projection_name: &str,
        last_applied_event_id: i64,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO projection_checkpoints (projection_name, last_applied_event_id, updated_at)
            VALUES ($1, $2, now())
            ON CONFLICT (projection_name)
            DO UPDATE SET last_applied_event_id = EXCLUDED.last_applied_event_id, updated_at = now()
            "#,
        )
        .bind(projection_name)
        .bind(last_applied_event_id)
        .execute(&mut **tx)
        .await
        .map_err(DbError::Query)?;

        Ok(())
    }

    /// Get all projection checkpoints.
    pub async fn list_checkpoints(&self) -> Result<Vec<ProjectionCheckpoint>, DbError> {
        let checkpoints = sqlx::query_as::<_, ProjectionCheckpoint>(
            r#"
            SELECT projection_name, last_applied_event_id, updated_at
            FROM projection_checkpoints
            ORDER BY projection_name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(checkpoints)
    }

    /// Reset a projection checkpoint to 0.
    ///
    /// Used when rebuilding a projection from scratch.
    pub async fn reset_checkpoint(&self, projection_name: &str) -> Result<(), DbError> {
        sqlx::query(
            r#"
            UPDATE projection_checkpoints
            SET last_applied_event_id = 0, updated_at = now()
            WHERE projection_name = $1
            "#,
        )
        .bind(projection_name)
        .execute(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(())
    }

    /// Calculate lag for all projections.
    ///
    /// Returns projection name and lag (max_event_id - last_applied_event_id).
    pub async fn calculate_lag(&self) -> Result<Vec<(String, i64)>, DbError> {
        let rows = sqlx::query(
            r#"
            SELECT
                p.projection_name,
                COALESCE((SELECT MAX(event_id) FROM events), 0) - p.last_applied_event_id as lag
            FROM projection_checkpoints p
            ORDER BY lag DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Query)?;

        let result = rows
            .iter()
            .map(|row| {
                let name: String = row.get("projection_name");
                let lag: i64 = row.get("lag");
                (name, lag)
            })
            .collect();

        Ok(result)
    }

    /// Wait until a projection checkpoint has reached at least `min_event_id`.
    pub async fn wait_for_checkpoint(
        &self,
        projection_name: &str,
        min_event_id: i64,
        timeout: Duration,
    ) -> Result<(), DbError> {
        let deadline = Instant::now() + timeout;

        loop {
            let checkpoint = match self.get_checkpoint(projection_name).await {
                Ok(cp) => cp,
                Err(DbError::ProjectionNotFound(_)) => {
                    // Ensure the checkpoint row exists so other readers can observe progress.
                    self.ensure_checkpoint(projection_name).await?;
                    ProjectionCheckpoint {
                        projection_name: projection_name.to_string(),
                        last_applied_event_id: 0,
                        updated_at: Utc::now(),
                    }
                }
                Err(e) => return Err(e),
            };
            if checkpoint.last_applied_event_id >= min_event_id {
                return Ok(());
            }

            if Instant::now() >= deadline {
                return Err(DbError::ProjectionTimeout {
                    projection_name: projection_name.to_string(),
                    expected: min_event_id,
                    actual: checkpoint.last_applied_event_id,
                });
            }

            sleep(Duration::from_millis(25)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_structure() {
        // Just verify the struct compiles
        let checkpoint = ProjectionCheckpoint {
            projection_name: "orgs".to_string(),
            last_applied_event_id: 100,
            updated_at: Utc::now(),
        };
        assert_eq!(checkpoint.projection_name, "orgs");
    }
}
