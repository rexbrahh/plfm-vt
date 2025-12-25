//! Control plane API client for the node agent.
//!
//! Provides methods for communicating with the control plane:
//! - Fetching the current plan
//! - Reporting instance status

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
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

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Failed to fetch plan");
            anyhow::bail!("Failed to fetch plan: {} - {}", status, body);
        }

        let plan: NodePlan = response.json().await?;
        debug!(
            cursor_event_id = plan.cursor_event_id,
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

        let response = self.client.post(&url).json(status).send().await?;

        if !response.status().is_success() {
            let status_code = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status_code, body = %body, "Failed to report status");
            anyhow::bail!("Failed to report status: {} - {}", status_code, body);
        }

        Ok(())
    }

    /// Fetch decrypted secret material for a version.
    pub async fn fetch_secret_material(&self, version_id: &str) -> Result<SecretMaterialResponse> {
        let url = format!(
            "{}/v1/nodes/{}/secrets/{}",
            self.base_url, self.node_id, version_id
        );
        debug!(url = %url, "Fetching secret material");

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            let status_code = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status_code, body = %body, "Failed to fetch secret material");
            anyhow::bail!(
                "Failed to fetch secret material: {} - {}",
                status_code,
                body
            );
        }

        let payload: SecretMaterialResponse = response.json().await?;
        Ok(payload)
    }

    /// Send workload log entries to the control plane.
    pub async fn send_workload_logs(&self, entries: Vec<WorkloadLogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let url = format!("{}/v1/nodes/{}/logs", self.base_url, self.node_id);
        let request = WorkloadLogRequest { entries };

        let response = self.client.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status_code = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(status = %status_code, body = %body, "Failed to send workload logs");
            anyhow::bail!("Failed to send workload logs: {} - {}", status_code, body);
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
    pub spec_version: String,
    pub node_id: String,
    pub plan_id: String,
    pub created_at: DateTime<Utc>,
    pub cursor_event_id: i64,
    pub instances: Vec<DesiredInstanceAssignment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DesiredInstanceAssignment {
    pub assignment_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub generation: i32,
    pub desired_state: InstanceDesiredState,
    #[serde(default)]
    pub drain_grace_seconds: Option<i32>,
    #[serde(default)]
    pub workload: Option<InstancePlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceDesiredState {
    Running,
    Draining,
    Stopped,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstancePlan {
    pub spec_version: String,
    pub org_id: String,
    pub app_id: String,
    pub env_id: String,
    pub process_type: String,
    pub instance_id: String,
    pub generation: i32,
    pub release_id: String,
    pub image: WorkloadImage,
    pub manifest_hash: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env_vars: Option<HashMap<String, String>>,
    pub resources: WorkloadResources,
    pub network: WorkloadNetwork,
    #[serde(default)]
    pub mounts: Option<Vec<WorkloadMount>>,
    #[serde(default)]
    pub secrets: Option<WorkloadSecrets>,
    #[serde(default)]
    pub spec_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkloadImage {
    #[serde(rename = "ref")]
    pub image_ref: Option<String>,
    pub digest: String,
    #[serde(default)]
    pub index_digest: Option<String>,
    pub resolved_digest: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkloadResources {
    pub cpu_request: f64,
    pub memory_limit_bytes: i64,
    #[serde(default)]
    pub ephemeral_disk_bytes: Option<i64>,
    #[serde(default)]
    pub vcpu_count: Option<i32>,
    #[serde(default)]
    pub cpu_weight: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkloadNetwork {
    pub overlay_ipv6: String,
    pub gateway_ipv6: String,
    #[serde(default)]
    pub mtu: Option<i32>,
    #[serde(default)]
    pub dns: Option<Vec<String>>,
    #[serde(default)]
    pub ports: Option<Vec<WorkloadPort>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkloadPort {
    pub name: String,
    pub port: i32,
    pub protocol: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkloadMount {
    pub volume_id: String,
    pub mount_path: String,
    pub read_only: bool,
    pub filesystem: String,
    #[serde(default)]
    pub device_hint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkloadSecrets {
    pub required: bool,
    #[serde(default)]
    pub secret_version_id: Option<String>,
    pub mount_path: String,
    #[serde(default)]
    pub mode: Option<i32>,
    #[serde(default)]
    pub uid: Option<i32>,
    #[serde(default)]
    pub gid: Option<i32>,
}

/// Secret material response from the control plane.
#[derive(Debug, Clone, Deserialize)]
pub struct SecretMaterialResponse {
    pub version_id: String,
    pub format: String,
    pub data_hash: String,
    pub data: String,
}

/// Workload log entry sent by node agents.
#[derive(Debug, Clone, Serialize)]
pub struct WorkloadLogEntry {
    pub ts: DateTime<Utc>,
    pub instance_id: String,
    pub stream: String,
    pub line: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
struct WorkloadLogRequest {
    entries: Vec<WorkloadLogEntry>,
}

/// Instance status report sent to the control plane.
#[derive(Debug, Serialize)]
pub struct InstanceStatusReport {
    pub instance_id: String,
    pub status: InstanceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<FailureReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    ImagePullFailed,
    RootfsBuildFailed,
    FirecrackerStartFailed,
    NetworkSetupFailed,
    VolumeAttachFailed,
    SecretsMissing,
    SecretsInjectionFailed,
    HealthcheckFailed,
    OomKilled,
    CrashLoopBackoff,
    TerminatedByOperator,
    NodeDraining,
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
            "spec_version": "v1",
            "node_id": "node_123",
            "plan_id": "01HZYKX4MZ5ZQ2KQ2B70YH9F7T",
            "created_at": "2025-12-17T12:00:00Z",
            "cursor_event_id": 42,
            "instances": [
                {
                    "assignment_id": "assign_123",
                    "node_id": "node_123",
                    "instance_id": "inst_123",
                    "generation": 1,
                    "desired_state": "running",
                    "drain_grace_seconds": 10,
                    "workload": {
                        "spec_version": "v1",
                        "org_id": "org_123",
                        "app_id": "app_456",
                        "env_id": "env_789",
                        "process_type": "web",
                        "instance_id": "inst_123",
                        "generation": 1,
                        "release_id": "rel_abc",
                        "image": {
                            "ref": "ghcr.io/org/app:v1",
                            "digest": "sha256:manifest",
                            "resolved_digest": "sha256:resolved",
                            "os": "linux",
                            "arch": "amd64"
                        },
                        "manifest_hash": "hash_abc",
                        "command": ["./start"],
                        "env_vars": {"FOO": "bar"},
                        "resources": {
                            "cpu_request": 1.0,
                            "memory_limit_bytes": 536870912
                        },
                        "network": {
                            "overlay_ipv6": "fd00::1234",
                            "gateway_ipv6": "fd00::1",
                            "mtu": 1420,
                            "dns": ["fd00::53"]
                        },
                        "mounts": [],
                        "secrets": null,
                        "spec_hash": "spec_hash"
                    }
                }
            ]
        }"#;

        let plan: NodePlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.cursor_event_id, 42);
        assert_eq!(plan.plan_id, "01HZYKX4MZ5ZQ2KQ2B70YH9F7T");
        assert_eq!(plan.instances.len(), 1);
        assert_eq!(plan.instances[0].instance_id, "inst_123");
        assert_eq!(
            plan.instances[0].desired_state,
            InstanceDesiredState::Running
        );
        let workload = plan.instances[0].workload.as_ref().unwrap();
        assert_eq!(workload.process_type, "web");
        assert_eq!(workload.network.overlay_ipv6, "fd00::1234");
    }

    #[test]
    fn test_instance_status_serialization() {
        let report = InstanceStatusReport {
            instance_id: "inst_123".to_string(),
            status: InstanceStatus::Ready,
            boot_id: Some("boot_456".to_string()),
            reason_code: None,
            error_message: None,
            exit_code: None,
        };

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"status\":\"ready\""));
        assert!(json.contains("\"boot_id\":\"boot_456\""));
        assert!(!json.contains("error_message")); // Should be skipped
    }
}
