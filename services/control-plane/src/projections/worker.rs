//! Background projection worker.
//!
//! The worker continuously polls the event log and applies events to projections.
//! It runs in a loop:
//! 1. Query events after the minimum checkpoint across all projections
//! 2. For each event, dispatch to the appropriate handler
//! 3. Update checkpoints atomically with view updates
//! 4. Sleep if no new events, then repeat
//!
//! The worker handles restarts gracefully by resuming from persisted checkpoints.

use std::collections::HashMap;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;
use tokio::time::sleep;
use tracing::{debug, error, info, instrument, warn};

use crate::db::{EventStore, ProjectionStore};

use super::{ProjectionError, ProjectionRegistry, ProjectionResult};

/// Configuration for the projection worker.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Maximum number of events to fetch per batch.
    pub batch_size: i32,

    /// How long to sleep when no events are available.
    pub poll_interval: Duration,

    /// How often to log progress (in events processed).
    pub log_interval: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            poll_interval: Duration::from_millis(100),
            log_interval: 1000,
        }
    }
}

/// Background worker that processes events and updates projections.
pub struct ProjectionWorker {
    pool: PgPool,
    event_store: EventStore,
    projection_store: ProjectionStore,
    registry: ProjectionRegistry,
    config: WorkerConfig,
}

impl ProjectionWorker {
    /// Create a new projection worker.
    pub fn new(pool: PgPool, config: WorkerConfig) -> Self {
        Self {
            event_store: EventStore::new(pool.clone()),
            projection_store: ProjectionStore::new(pool.clone()),
            pool,
            registry: ProjectionRegistry::new(),
            config,
        }
    }

    /// Run the worker until the shutdown signal is received.
    ///
    /// # Arguments
    ///
    /// * `shutdown` - A watch receiver that signals when to shutdown.
    ///
    /// The worker will exit gracefully when the shutdown channel receives a value.
    #[instrument(skip(self, shutdown), name = "projection_worker")]
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> ProjectionResult<()> {
        info!("Starting projection worker");

        // Load initial checkpoints
        let mut checkpoints = self.load_checkpoints().await?;
        let min_checkpoint = self.min_checkpoint(&checkpoints);
        info!(
            min_checkpoint = min_checkpoint,
            projections = checkpoints.len(),
            "Loaded projection checkpoints"
        );

        let mut events_processed: u64 = 0;
        let mut last_log_count: u64 = 0;

        loop {
            // Check for shutdown signal
            if *shutdown.borrow() {
                info!(
                    events_processed = events_processed,
                    "Shutdown signal received, stopping projection worker"
                );
                break;
            }

            // Calculate minimum checkpoint to query from
            let min_checkpoint = self.min_checkpoint(&checkpoints);

            // Fetch next batch of events
            let events = self
                .event_store
                .query_after_cursor(min_checkpoint, self.config.batch_size)
                .await?;

            if events.is_empty() {
                // No events, wait and retry
                tokio::select! {
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!("Shutdown signal received during poll wait");
                            break;
                        }
                    }
                    _ = sleep(self.config.poll_interval) => {}
                }
                continue;
            }

            debug!(count = events.len(), "Processing event batch");

            // Process each event
            for event in events {
                if self.registry.handler_for(&event.event_type).is_none() {
                    warn!(
                        event_id = event.event_id,
                        event_type = %event.event_type,
                        "No projection handler registered for event type"
                    );
                }

                // Skip events that all projections have already processed
                let projections_needing_event: Vec<_> = self
                    .registry
                    .handlers()
                    .iter()
                    .filter(|h| {
                        let checkpoint = checkpoints.get(h.name()).copied().unwrap_or(0);
                        checkpoint < event.event_id
                            && h.event_types().contains(&event.event_type.as_str())
                    })
                    .collect();

                if projections_needing_event.is_empty() {
                    // No projection needs this event, update all checkpoints past it
                    for handler in self.registry.handlers() {
                        let checkpoint = checkpoints.entry(handler.name().to_string()).or_insert(0);
                        if *checkpoint < event.event_id {
                            *checkpoint = event.event_id;
                        }
                    }
                    continue;
                }

                // Process in a transaction
                let mut tx = self.pool.begin().await?;

                for handler in projections_needing_event {
                    let current_checkpoint = checkpoints.get(handler.name()).copied().unwrap_or(0);
                    if current_checkpoint >= event.event_id {
                        continue;
                    }

                    match handler.apply(&mut tx, &event).await {
                        Ok(()) => {
                            // Update checkpoint
                            if let Err(err) = ProjectionStore::update_checkpoint_in_tx(
                                &mut tx,
                                handler.name(),
                                event.event_id,
                            )
                            .await
                            {
                                error!(
                                    error = %err,
                                    event_id = event.event_id,
                                    projection = handler.name(),
                                    "Failed to update projection checkpoint"
                                );
                                return Err(ProjectionError::Database(err));
                            }

                            checkpoints.insert(handler.name().to_string(), event.event_id);
                        }
                        Err(e) => {
                            error!(
                                error = %e,
                                event_id = event.event_id,
                                event_type = %event.event_type,
                                projection = handler.name(),
                                "Failed to apply event, rolling back"
                            );
                            // Rollback happens automatically when tx is dropped
                            return Err(e);
                        }
                    }
                }

                tx.commit().await?;
                events_processed += 1;

                // Log progress periodically
                if events_processed - last_log_count >= self.config.log_interval {
                    info!(
                        events_processed = events_processed,
                        latest_event_id = event.event_id,
                        "Projection worker progress"
                    );
                    last_log_count = events_processed;
                }
            }
        }

        info!(
            events_processed = events_processed,
            "Projection worker stopped"
        );
        Ok(())
    }

    /// Load checkpoints for all projections.
    async fn load_checkpoints(&self) -> ProjectionResult<HashMap<String, i64>> {
        let mut checkpoints = HashMap::new();

        for name in self.registry.projection_names() {
            match self.projection_store.get_checkpoint(name).await {
                Ok(cp) => {
                    checkpoints.insert(name.to_string(), cp.last_applied_event_id);
                }
                Err(crate::db::DbError::ProjectionNotFound(_)) => {
                    // Projection checkpoint doesn't exist, start from 0
                    warn!(
                        projection = name,
                        "Projection checkpoint not found, starting from 0"
                    );
                    checkpoints.insert(name.to_string(), 0);
                }
                Err(e) => {
                    return Err(ProjectionError::Database(e));
                }
            }
        }

        Ok(checkpoints)
    }

    /// Get the minimum checkpoint across all projections.
    fn min_checkpoint(&self, checkpoints: &HashMap<String, i64>) -> i64 {
        checkpoints.values().copied().min().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_config_defaults() {
        let config = WorkerConfig::default();
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.poll_interval, Duration::from_millis(100));
    }
}
