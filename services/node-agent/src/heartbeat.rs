//! Heartbeat loop for reporting node status to the control plane.
//!
//! The node agent sends periodic heartbeats to the control plane to:
//! - Indicate the node is alive and healthy
//! - Report current resource availability
//! - Report instance counts

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::client::{ControlPlaneClient, HeartbeatRequest, NodeState};
use crate::config::Config;
use crate::instance::InstanceManager;

/// Run the heartbeat loop until shutdown.
pub async fn run_heartbeat_loop(
    config: Config,
    instance_manager: Arc<InstanceManager>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let client = ControlPlaneClient::new(&config);
    let interval = Duration::from_secs(config.heartbeat_interval_secs);

    info!(
        node_id = %config.node_id,
        interval_secs = config.heartbeat_interval_secs,
        "Starting heartbeat loop"
    );

    let mut consecutive_failures = 0u32;
    let mut interval_timer = tokio::time::interval(interval);

    loop {
        tokio::select! {
            _ = interval_timer.tick() => {
                let instance_count = instance_manager.instance_count().await;

                let request = HeartbeatRequest {
                    state: NodeState::Active,
                    // TODO: Actually measure available resources
                    available_cpu_cores: 8,
                    available_memory_bytes: 16 * 1024 * 1024 * 1024, // 16 GiB
                    instance_count,
                };

                match client.send_heartbeat(&request).await {
                    Ok(response) => {
                        consecutive_failures = 0;
                        debug!(
                            accepted = response.accepted,
                            next_interval = response.next_heartbeat_secs,
                            instance_count,
                            "Heartbeat acknowledged"
                        );
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        if consecutive_failures <= 3 {
                            warn!(
                                error = %e,
                                consecutive_failures,
                                "Heartbeat failed"
                            );
                        } else {
                            error!(
                                error = %e,
                                consecutive_failures,
                                "Heartbeat failed repeatedly"
                            );
                        }
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("Heartbeat loop shutting down");
                    break;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heartbeat_request_serialization() {
        let request = HeartbeatRequest {
            state: NodeState::Active,
            available_cpu_cores: 8,
            available_memory_bytes: 16 * 1024 * 1024 * 1024,
            instance_count: 5,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"state\":\"active\""));
        assert!(json.contains("\"instance_count\":5"));
    }
}
