//! Reconciliation loop for converging node state.
//!
//! The reconciler:
//! - Periodically fetches the plan from the control plane
//! - Applies the plan to the instance manager
//! - Reports status changes back to the control plane

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::client::ControlPlaneClient;
use crate::config::Config;
use crate::instance::InstanceManager;

/// Reconciliation loop configuration.
pub struct ReconcilerConfig {
    /// Interval between plan fetches.
    pub reconcile_interval: Duration,

    /// Interval between health checks.
    pub health_check_interval: Duration,
}

impl Default for ReconcilerConfig {
    fn default() -> Self {
        Self {
            reconcile_interval: Duration::from_secs(5),
            health_check_interval: Duration::from_secs(10),
        }
    }
}

/// Reconciler for converging node state.
pub struct Reconciler {
    /// Control plane client.
    client: ControlPlaneClient,

    /// Instance manager.
    instance_manager: Arc<InstanceManager>,

    /// Configuration.
    config: ReconcilerConfig,
}

impl Reconciler {
    /// Create a new reconciler.
    pub fn new(
        agent_config: &Config,
        instance_manager: Arc<InstanceManager>,
        config: ReconcilerConfig,
    ) -> Self {
        Self {
            client: ControlPlaneClient::new(agent_config),
            instance_manager,
            config,
        }
    }

    /// Run the reconciliation loop until shutdown.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        info!(
            reconcile_interval_secs = self.config.reconcile_interval.as_secs(),
            health_check_interval_secs = self.config.health_check_interval.as_secs(),
            "Starting reconciliation loop"
        );

        let mut reconcile_interval = tokio::time::interval(self.config.reconcile_interval);
        let mut health_check_interval = tokio::time::interval(self.config.health_check_interval);

        loop {
            tokio::select! {
                _ = reconcile_interval.tick() => {
                    if let Err(e) = self.reconcile().await {
                        error!(error = %e, "Reconciliation failed");
                    }
                }
                _ = health_check_interval.tick() => {
                    self.check_health().await;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("Reconciler shutting down");
                        break;
                    }
                }
            }
        }
    }

    /// Perform a single reconciliation pass.
    async fn reconcile(&self) -> anyhow::Result<()> {
        debug!("Starting reconciliation");

        // Fetch the current plan
        let plan = match self.client.fetch_plan().await {
            Ok(plan) => plan,
            Err(e) => {
                warn!(error = %e, "Failed to fetch plan, will retry");
                return Err(e);
            }
        };

        // Check if plan is newer
        let last_version = self.instance_manager.last_plan_version().await;
        if plan.plan_version <= last_version {
            debug!(
                plan_version = plan.plan_version,
                last_version, "Plan version not newer, skipping"
            );
            return Ok(());
        }

        // Apply the plan
        self.instance_manager
            .apply_plan(plan.plan_version, plan.instances)
            .await;

        // Report status for all instances
        self.report_all_status().await;

        Ok(())
    }

    /// Check health of all instances.
    async fn check_health(&self) {
        debug!("Checking instance health");
        self.instance_manager.check_health().await;
    }

    /// Report status for all instances.
    async fn report_all_status(&self) {
        let reports = self.instance_manager.get_status_reports().await;

        for report in reports {
            if let Err(e) = self.client.report_instance_status(&report).await {
                warn!(
                    instance_id = %report.instance_id,
                    error = %e,
                    "Failed to report instance status"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconciler_config_default() {
        let config = ReconcilerConfig::default();
        assert_eq!(config.reconcile_interval, Duration::from_secs(5));
        assert_eq!(config.health_check_interval, Duration::from_secs(10));
    }
}
