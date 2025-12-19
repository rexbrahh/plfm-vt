//! Configuration types for guest init.
//!
//! These types match the config message format from the vsock protocol.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Complete guest configuration received from host agent.
#[derive(Debug, Clone, Deserialize)]
pub struct GuestConfig {
    /// Config format version.
    pub config_version: String,

    /// Instance ID.
    pub instance_id: String,

    /// Configuration generation number.
    pub generation: u64,

    /// Workload configuration.
    pub workload: WorkloadConfig,

    /// Network configuration.
    pub network: NetworkConfig,

    /// Volume mounts.
    #[serde(default)]
    pub mounts: Vec<MountConfig>,

    /// Secrets configuration.
    #[serde(default)]
    pub secrets: Option<SecretsConfig>,

    /// Exec service configuration.
    #[serde(default)]
    pub exec: ExecConfig,
}

/// Workload process configuration.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Fields used by deserialization
pub struct WorkloadConfig {
    /// Command and arguments.
    pub argv: Vec<String>,

    /// Working directory.
    pub cwd: String,

    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// User ID to run as.
    #[serde(default = "default_uid")]
    pub uid: u32,

    /// Group ID to run as.
    #[serde(default = "default_gid")]
    pub gid: u32,

    /// Whether stdin is connected.
    #[serde(default)]
    pub stdin: bool,

    /// Whether to allocate a TTY.
    #[serde(default)]
    pub tty: bool,
}

fn default_uid() -> u32 {
    1000
}

fn default_gid() -> u32 {
    1000
}

/// Network configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    /// Overlay IPv6 address.
    pub overlay_ipv6: String,

    /// Gateway IPv6 address.
    pub gateway_ipv6: String,

    /// Prefix length (typically 128).
    #[serde(default = "default_prefix_len")]
    pub prefix_len: u8,

    /// MTU.
    #[serde(default = "default_mtu")]
    pub mtu: u32,

    /// DNS servers.
    #[serde(default)]
    pub dns: Vec<String>,

    /// Hostname.
    #[serde(default)]
    pub hostname: Option<String>,
}

fn default_prefix_len() -> u8 {
    128
}

fn default_mtu() -> u32 {
    1420
}

/// Volume mount configuration.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Fields used by deserialization
pub struct MountConfig {
    /// Mount type (volume, tmpfs).
    pub kind: String,

    /// Volume name.
    pub name: String,

    /// Device path (e.g., /dev/vdc).
    #[serde(default)]
    pub device: Option<String>,

    /// Mount point inside guest.
    pub mountpoint: String,

    /// Filesystem type.
    #[serde(default = "default_fs_type")]
    pub fs_type: String,

    /// Mount mode (rw, ro).
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_fs_type() -> String {
    "ext4".to_string()
}

fn default_mode() -> String {
    "rw".to_string()
}

/// Secrets configuration.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Fields used by deserialization
pub struct SecretsConfig {
    /// Whether secrets are required.
    #[serde(default)]
    pub required: bool,

    /// Path to write secrets file.
    #[serde(default = "default_secrets_path")]
    pub path: String,

    /// File permissions (octal string).
    #[serde(default = "default_secrets_mode")]
    pub mode: String,

    /// Owner UID.
    #[serde(default)]
    pub owner_uid: u32,

    /// Owner GID.
    #[serde(default)]
    pub owner_gid: u32,

    /// Secrets format (dotenv).
    #[serde(default = "default_secrets_format")]
    pub format: String,

    /// Secret bundle version ID.
    #[serde(default)]
    pub bundle_version_id: Option<String>,

    /// Secrets data (if inline).
    #[serde(default)]
    pub data: Option<String>,
}

fn default_secrets_path() -> String {
    "/run/secrets/platform.env".to_string()
}

fn default_secrets_mode() -> String {
    "0400".to_string()
}

fn default_secrets_format() -> String {
    "dotenv".to_string()
}

/// Exec service configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ExecConfig {
    /// vsock port for exec service.
    #[serde(default = "default_exec_port")]
    pub vsock_port: u32,

    /// Whether exec service is enabled.
    #[serde(default = "default_exec_enabled")]
    pub enabled: bool,
}

fn default_exec_port() -> u32 {
    5162
}

fn default_exec_enabled() -> bool {
    true
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            vsock_port: default_exec_port(),
            enabled: default_exec_enabled(),
        }
    }
}

// =============================================================================
// Handshake Messages
// =============================================================================

/// Hello message sent from guest to host.
#[derive(Debug, Serialize)]
pub struct HelloMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub guest_init_version: String,
    pub guest_init_protocol: u32,
    pub instance_id: String,
    pub boot_id: String,
}

impl HelloMessage {
    pub fn new(instance_id: &str, boot_id: &str, version: &str, protocol: u32) -> Self {
        Self {
            msg_type: "hello".to_string(),
            guest_init_version: version.to_string(),
            guest_init_protocol: protocol,
            instance_id: instance_id.to_string(),
            boot_id: boot_id.to_string(),
        }
    }
}

/// Acknowledgment message sent from guest to host.
#[derive(Debug, Serialize)]
pub struct AckMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub config_version: String,
    pub generation: u64,
}

impl AckMessage {
    pub fn new(config_version: &str, generation: u64) -> Self {
        Self {
            msg_type: "ack".to_string(),
            config_version: config_version.to_string(),
            generation,
        }
    }
}

/// Status message sent from guest to host.
#[derive(Debug, Serialize)]
pub struct StatusMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub state: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl StatusMessage {
    pub fn new(state: &str) -> Self {
        Self {
            msg_type: "status".to_string(),
            state: state.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            reason: None,
            detail: None,
            exit_code: None,
        }
    }

    #[allow(dead_code)] // Used by error reporting paths
    pub fn with_failure(state: &str, reason: &str, detail: &str) -> Self {
        Self {
            msg_type: "status".to_string(),
            state: state.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            reason: Some(reason.to_string()),
            detail: Some(detail.to_string()),
            exit_code: None,
        }
    }

    pub fn with_exit(exit_code: i32) -> Self {
        Self {
            msg_type: "status".to_string(),
            state: "exited".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            reason: None,
            detail: None,
            exit_code: Some(exit_code),
        }
    }
}

/// Config message received from host.
#[derive(Debug, Deserialize)]
pub struct ConfigMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(flatten)]
    pub config: GuestConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_serialization() {
        let hello = HelloMessage::new("inst_123", "boot_456", "1.0.0", 1);
        let json = serde_json::to_string(&hello).unwrap();
        assert!(json.contains("\"type\":\"hello\""));
        assert!(json.contains("\"guest_init_version\":\"1.0.0\""));
    }

    #[test]
    fn test_config_deserialization() {
        let json = r#"{
            "type": "config",
            "config_version": "v1",
            "instance_id": "inst_123",
            "generation": 7,
            "workload": {
                "argv": ["./server"],
                "cwd": "/app"
            },
            "network": {
                "overlay_ipv6": "fd00::1234",
                "gateway_ipv6": "fd00::1"
            }
        }"#;

        let msg: ConfigMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.msg_type, "config");
        assert_eq!(msg.config.instance_id, "inst_123");
        assert_eq!(msg.config.workload.argv[0], "./server");
    }

    #[test]
    fn test_status_serialization() {
        let status = StatusMessage::new("ready");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"state\":\"ready\""));
        assert!(!json.contains("reason")); // should be skipped

        let failed = StatusMessage::with_failure("failed", "mount_failed", "ext4 error");
        let json = serde_json::to_string(&failed).unwrap();
        assert!(json.contains("\"reason\":\"mount_failed\""));
    }
}
