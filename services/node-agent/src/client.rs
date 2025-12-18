//! Control plane API client for the node agent.
//!
//! Provides methods for communicating with the control plane:
//! - Fetching the current plan
//! - Reporting instance status

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::config::Config;

/// Control plane API client.
pub struct ControlPlaneClient {
    client: reqwest::Client,
    base_url: String,
    node_id: String,
}

impl ControlPlaneClient {
    /// Create a new control plane client.
    pub fn new(config: &Config) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url: config.control_plane_url.clone(),
            node_id: config.node_id.to_string(),
        }
    }

    /// Fetch the current plan for this node.
    pub async fn fetch_plan(&self) -> Result<NodePlan> {
        let url = format!("{}/v1/nodes/{}/plan", self.base_url, self.node_id);
        debug!(url = %url, "Fetching node plan");

        let response = self
            .client
            .get(&url)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Failed to fetch plan");
            anyhow::bail!("Failed to fetch plan: {} - {}", status, body);
        }

        let plan: NodePlan = response.json().await?;
        debug!(
            plan_version = plan.plan_version,
            instance_count = plan.instances.len(),
            "Fetched node plan"
        );

        Ok(plan)
    }

    /// Report instance status to the control plane.
    pub async fn report_instance_status(&self, status: &InstanceStatusReport) -> Result<()> {
        let url = format!(
            "{}/v1/nodes/{}/instances/{}/status",
            self.base_url, self.node_id, status.instance_id
        );
        debug!(
            instance_id = %status.instance_id,
            status = %status.status,
            "Reporting instance status"
        );

        let response = self
            .client
            .post(&url)
            .json(status)
            .send()
            .await?;

        if !response.status().is_success() {
            let status_code = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status_code, body = %body, "Failed to report status");
            anyhow::bail!("Failed to report status: {} - {}", status_code, body);
        }

        Ok(())
    }

    /// Send heartbeat with current state.
    pub async fn send_heartbeat(&self, request: &HeartbeatRequest) -> Result<HeartbeatResponse> {
        let url = format!("{}/v1/nodes/{}/heartbeat", self.base_url, self.node_id);

        let response = self
            .client
            .post(&url)
            .json(request)
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Heartbeat failed with status: {}", response.status());
        }

        let body: HeartbeatResponse = response.json().await?;
        Ok(body)
    }
}

/// Node plan from the control plane.
#[derive(Debug, Clone, Deserialize)]
pub struct NodePlan {
    /// Plan version (monotonically increasing).
    pub plan_version: i64,

    /// Instances assigned to this node.
    pub instances: Vec<InstancePlan>,
}

/// Plan for a single instance.
#[derive(Debug, Clone, Deserialize)]
pub struct InstancePlan {
    /// Instance ID.
    pub instance_id: String,

    /// App ID.
    pub app_id: String,

    /// Env ID.
    pub env_id: String,

    /// Release ID to run.
    pub release_id: String,

    /// Deploy ID that triggered this instance.
    pub deploy_id: String,

    /// OCI image reference.
    pub image: String,

    /// Resource requests.
    pub resources: InstanceResources,

    /// Environment variables.
    #[serde(default)]
    pub env_vars: serde_json::Value,

    /// Volume mounts.
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
}

/// Resource requests for an instance.
#[derive(Debug, Clone, Deserialize)]
pub struct InstanceResources {
    /// CPU cores (can be fractional).
    pub cpu: f64,

    /// Memory in bytes.
    pub memory_bytes: i64,
}

/// Volume mount specification.
#[derive(Debug, Clone, Deserialize)]
pub struct VolumeMount {
    /// Volume ID.
    pub volume_id: String,

    /// Mount path inside the instance.
    pub mount_path: String,

    /// Whether the mount is read-only.
    pub read_only: bool,
}

/// Instance status report sent to the control plane.
#[derive(Debug, Serialize)]
pub struct InstanceStatusReport {
    /// Instance ID.
    pub instance_id: String,

    /// Current status.
    pub status: InstanceStatus,

    /// Optional boot ID (set when instance is running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,

    /// Optional error message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,

    /// Exit code (if stopped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Instance status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    /// Instance is booting.
    Booting,
    /// Instance is ready and serving.
    Ready,
    /// Instance is draining (preparing to stop).
    Draining,
    /// Instance has stopped.
    Stopped,
    /// Instance has failed.
    Failed,
}

impl std::fmt::Display for InstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceStatus::Booting => write!(f, "booting"),
            InstanceStatus::Ready => write!(f, "ready"),
            InstanceStatus::Draining => write!(f, "draining"),
            InstanceStatus::Stopped => write!(f, "stopped"),
            InstanceStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Heartbeat request.
#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    /// Current node state.
    pub state: NodeState,

    /// Available CPU cores.
    pub available_cpu_cores: i32,

    /// Available memory in bytes.
    pub available_memory_bytes: i64,

    /// Number of running instances.
    pub instance_count: i32,
}

/// Node state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Active,
    Draining,
    Disabled,
    Degraded,
    Offline,
}

/// Heartbeat response.
#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    /// Whether the heartbeat was accepted.
    pub accepted: bool,

    /// Next heartbeat interval in seconds.
    pub next_heartbeat_secs: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_plan_deserialization() {
        let json = r#"{
            "plan_version": 42,
            "instances": [
                {
                    "instance_id": "inst_123",
                    "app_id": "app_456",
                    "env_id": "env_789",
                    "release_id": "rel_abc",
                    "deploy_id": "dep_xyz",
                    "image": "ghcr.io/org/app:v1",
                    "resources": {
                        "cpu": 1.0,
                        "memory_bytes": 536870912
                    }
                }
            ]
        }"#;

        let plan: NodePlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.plan_version, 42);
        assert_eq!(plan.instances.len(), 1);
        assert_eq!(plan.instances[0].instance_id, "inst_123");
    }

    #[test]
    fn test_instance_status_serialization() {
        let report = InstanceStatusReport {
            instance_id: "inst_123".to_string(),
            status: InstanceStatus::Ready,
            boot_id: Some("boot_456".to_string()),
            error_message: None,
            exit_code: None,
        };

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"status\":\"ready\""));
        assert!(json.contains("\"boot_id\":\"boot_456\""));
        assert!(!json.contains("error_message")); // Should be skipped
    }
}
