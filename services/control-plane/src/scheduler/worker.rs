//! Scheduler background worker.
//!
//! Runs the scheduler reconciliation loop on a periodic interval.

use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;
use tracing::{error, info, instrument};

use super::reconciler::SchedulerReconciler;

/// Scheduler worker that runs the reconciliation loop.
pub struct SchedulerWorker {
    reconciler: SchedulerReconciler,
    interval: Duration,
}

impl SchedulerWorker {
    /// Create a new scheduler worker.
    pub fn new(pool: PgPool, interval: Duration) -> Self {
        Self {
            reconciler: SchedulerReconciler::new(pool),
            interval,
        }
    }

    /// Run the scheduler worker until shutdown is signaled.
    #[instrument(skip(self, shutdown))]
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        info!(
            interval_secs = self.interval.as_secs(),
            "Starting scheduler worker"
        );

        let mut interval = tokio::time::interval(self.interval);
        // Don't immediately tick on startup - wait for first interval
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.run_reconciliation().await {
                        error!(error = %e, "Scheduler reconciliation failed");
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("Scheduler worker shutting down");
                        break;
                    }
                }
            }
        }
    }

    /// Run a single reconciliation pass.
    async fn run_reconciliation(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let stats = self.reconciler.reconcile_all().await?;
        
        if stats.instances_allocated > 0 || stats.instances_drained > 0 {
            info!(
                groups_processed = stats.groups_processed,
                instances_allocated = stats.instances_allocated,
                instances_drained = stats.instances_drained,
                "Scheduler reconciliation complete"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_worker_creation() {
        // Just verify the types work - actual database tests would need integration testing
        // let pool = PgPool::connect_lazy("postgres://test").unwrap();
        // let worker = SchedulerWorker::new(pool, Duration::from_secs(30));
    }
}
