use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;
use tracing::{error, info, instrument, warn};

#[derive(Debug, Clone)]
pub struct CleanupWorkerConfig {
    pub interval: Duration,
    pub workload_log_retention_days: i32,
    pub ipv4_cooldown_grace_days: i32,
    pub idempotency_retention_days: i32,
}

impl Default for CleanupWorkerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3600),
            workload_log_retention_days: 7,
            ipv4_cooldown_grace_days: 1,
            idempotency_retention_days: 7,
        }
    }
}

pub struct CleanupWorker {
    pool: PgPool,
    config: CleanupWorkerConfig,
}

impl CleanupWorker {
    pub fn new(pool: PgPool, config: CleanupWorkerConfig) -> Self {
        Self { pool, config }
    }

    #[instrument(skip(self, shutdown))]
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        info!(
            interval_secs = self.config.interval.as_secs(),
            workload_log_retention_days = self.config.workload_log_retention_days,
            "Starting cleanup worker"
        );

        let mut interval = tokio::time::interval(self.config.interval);
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.run_cleanup().await;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("Cleanup worker shutting down");
                        break;
                    }
                }
            }
        }
    }

    async fn run_cleanup(&self) {
        let mut total_deleted = 0u64;

        match self.cleanup_workload_logs().await {
            Ok(count) => {
                if count > 0 {
                    info!(deleted = count, "Cleaned up old workload logs");
                }
                total_deleted += count;
            }
            Err(e) => {
                error!(error = %e, "Failed to cleanup workload logs");
            }
        }

        match self.cleanup_ipv4_cooldowns().await {
            Ok(count) => {
                if count > 0 {
                    info!(deleted = count, "Cleaned up expired IPv4 cooldowns");
                }
                total_deleted += count;
            }
            Err(e) => {
                warn!(error = %e, "Failed to cleanup IPv4 cooldowns");
            }
        }

        match self.cleanup_idempotency_records().await {
            Ok(count) => {
                if count > 0 {
                    info!(deleted = count, "Cleaned up old idempotency records");
                }
                total_deleted += count;
            }
            Err(e) => {
                warn!(error = %e, "Failed to cleanup idempotency records");
            }
        }

        if total_deleted > 0 {
            info!(total_deleted = total_deleted, "Cleanup pass complete");
        }
    }

    async fn cleanup_workload_logs(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM workload_logs
            WHERE ts < now() - make_interval(days => $1)
            "#,
        )
        .bind(self.config.workload_log_retention_days)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    async fn cleanup_ipv4_cooldowns(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM ipam_ipv4_allocations
            WHERE released_at IS NOT NULL
              AND cooldown_until < now() - make_interval(days => $1)
            "#,
        )
        .bind(self.config.ipv4_cooldown_grace_days)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    async fn cleanup_idempotency_records(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM idempotency_records
            WHERE created_at < now() - make_interval(days => $1)
            "#,
        )
        .bind(self.config.idempotency_retention_days)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = CleanupWorkerConfig::default();
        assert_eq!(config.workload_log_retention_days, 7);
        assert_eq!(config.interval.as_secs(), 3600);
    }
}
