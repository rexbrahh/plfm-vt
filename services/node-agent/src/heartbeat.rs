//! Heartbeat loop for reporting node status to the control plane.
//!
//! The node agent sends periodic heartbeats to the control plane to:
//! - Indicate the node is alive and healthy
//! - Report current resource availability
//! - Receive updated desired state (node plan)

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::config::Config;

/// Heartbeat request sent to the control plane.
#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    /// Node identifier.
    pub node_id: String,

    /// Timestamp of this heartbeat.
    pub timestamp: String,

    /// Current node state.
    pub state: NodeState,

    /// Resource availability.
    pub resources: ResourceStatus,

    /// Number of running instances.
    pub instance_count: i32,
}

/// Current node state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Active,
    Draining,
    Disabled,
}

/// Resource availability on this node.
#[derive(Debug, Serialize)]
pub struct ResourceStatus {
    /// Available CPU cores.
    pub available_cpu_cores: i32,

    /// Available memory in bytes.
    pub available_memory_bytes: i64,

    /// Available disk space in bytes.
    pub available_disk_bytes: i64,
}

/// Heartbeat response from the control plane.
#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    /// Acknowledged.
    pub ok: bool,

    /// Optional message.
    #[serde(default)]
    pub message: Option<String>,

    /// Whether there are plan updates available.
    #[serde(default)]
    pub plan_updates_available: bool,
}

/// Run the heartbeat loop indefinitely.
pub async fn run_heartbeat_loop(config: Config) -> Result<()> {
    let client = reqwest::Client::new();
    let heartbeat_url = format!("{}/internal/nodes/heartbeat", config.control_plane_url);
    let interval = Duration::from_secs(config.heartbeat_interval_secs);

    info!(
        node_id = %config.node_id,
        interval_secs = config.heartbeat_interval_secs,
        "Starting heartbeat loop"
    );

    let mut consecutive_failures = 0u32;

    loop {
        let request = HeartbeatRequest {
            node_id: config.node_id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            state: NodeState::Active,
            resources: ResourceStatus {
                // TODO: Actually measure available resources
                available_cpu_cores: 8,
                available_memory_bytes: 16 * 1024 * 1024 * 1024, // 16 GiB
                available_disk_bytes: 100 * 1024 * 1024 * 1024,  // 100 GiB
            },
            instance_count: 0, // TODO: Count actual running instances
        };

        match send_heartbeat(&client, &heartbeat_url, &request).await {
            Ok(response) => {
                consecutive_failures = 0;
                debug!(
                    ok = response.ok,
                    plan_updates = response.plan_updates_available,
                    "Heartbeat acknowledged"
                );

                if response.plan_updates_available {
                    info!("Plan updates available, fetching...");
                    // TODO: Fetch and apply new plan
                }
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

        tokio::time::sleep(interval).await;
    }
}

/// Send a single heartbeat request.
async fn send_heartbeat(
    client: &reqwest::Client,
    url: &str,
    request: &HeartbeatRequest,
) -> Result<HeartbeatResponse> {
    let response = client
        .post(url)
        .json(request)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("heartbeat failed with status: {}", response.status());
    }

    let body: HeartbeatResponse = response.json().await?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heartbeat_request_serialization() {
        let request = HeartbeatRequest {
            node_id: "node_01HV4Z2WQXKJNM8GPQY6VBKC3D".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            state: NodeState::Active,
            resources: ResourceStatus {
                available_cpu_cores: 8,
                available_memory_bytes: 16 * 1024 * 1024 * 1024,
                available_disk_bytes: 100 * 1024 * 1024 * 1024,
            },
            instance_count: 5,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"node_id\""));
        assert!(json.contains("\"active\"")); // state should be snake_case
    }

    #[test]
    fn test_heartbeat_response_deserialization() {
        let json = r#"{"ok": true, "plan_updates_available": true}"#;
        let response: HeartbeatResponse = serde_json::from_str(json).unwrap();
        assert!(response.ok);
        assert!(response.plan_updates_available);
    }
}
