//! Vsock config delivery to guest-init.
//!
//! This module handles the vsock-based configuration handshake with guest-init
//! per docs/specs/runtime/guest-init.md.
//!
//! Protocol flow:
//! 1. Guest-init connects to host on vsock port 5161
//! 2. Guest sends hello message
//! 3. Host sends config message
//! 4. Guest sends ack message
//! 5. Guest sends status updates as boot progresses

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use vsock::{VsockAddr, VsockListener, VsockStream, VMADDR_CID_HOST};

use crate::client::InstancePlan;

/// Vsock port for config handshake.
pub const CONFIG_PORT: u32 = 5161;

/// Current protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Config version string.
pub const CONFIG_VERSION: &str = "v1";

// =============================================================================
// Message Types
// =============================================================================

/// Hello message from guest-init.
#[derive(Debug, Deserialize)]
pub struct HelloMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub guest_init_version: String,
    pub guest_init_protocol: u32,
    pub instance_id: String,
    pub boot_id: String,
}

/// Config message sent to guest-init.
#[derive(Debug, Serialize)]
pub struct ConfigMessage {
    #[serde(rename = "type")]
    msg_type: String,
    config_version: String,
    instance_id: String,
    generation: u64,
    workload: WorkloadConfig,
    network: NetworkConfig,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mounts: Vec<MountConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secrets: Option<SecretsConfig>,
    exec: ExecConfig,
}

/// Workload configuration for guest-init.
#[derive(Debug, Serialize)]
pub struct WorkloadConfig {
    argv: Vec<String>,
    cwd: String,
    env: HashMap<String, String>,
    uid: u32,
    gid: u32,
    stdin: bool,
    tty: bool,
}

/// Network configuration for guest-init.
#[derive(Debug, Serialize)]
pub struct NetworkConfig {
    overlay_ipv6: String,
    gateway_ipv6: String,
    prefix_len: u8,
    mtu: u32,
    dns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hostname: Option<String>,
}

/// Mount configuration for guest-init.
#[derive(Debug, Serialize)]
pub struct MountConfig {
    kind: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<String>,
    mountpoint: String,
    fs_type: String,
    mode: String,
}

/// Secrets configuration for guest-init.
#[derive(Debug, Serialize)]
pub struct SecretsConfig {
    required: bool,
    path: String,
    mode: String,
    owner_uid: u32,
    owner_gid: u32,
    format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle_version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}

/// Exec service configuration.
#[derive(Debug, Serialize)]
pub struct ExecConfig {
    vsock_port: u32,
    enabled: bool,
}

/// Ack message from guest-init.
#[derive(Debug, Deserialize)]
pub struct AckMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub config_version: String,
    pub generation: u64,
}

/// Status message from guest-init.
#[derive(Debug, Deserialize)]
pub struct StatusMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub state: String,
    pub timestamp: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

// =============================================================================
// Instance Config Store
// =============================================================================

/// Pending config for an instance awaiting handshake.
#[derive(Debug, Clone)]
pub struct PendingConfig {
    /// The instance plan.
    pub plan: InstancePlan,
    /// Overlay IPv6 address assigned to this instance.
    pub overlay_ipv6: String,
    /// Gateway IPv6 address.
    pub gateway_ipv6: String,
    /// Config generation number.
    pub generation: u64,
    /// Secrets data (decrypted, dotenv format).
    pub secrets_data: Option<String>,
}

/// Store for pending instance configurations.
pub struct ConfigStore {
    configs: RwLock<HashMap<String, PendingConfig>>,
}

impl ConfigStore {
    /// Create a new config store.
    pub fn new() -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
        }
    }

    /// Add a pending config for an instance.
    pub async fn add(&self, instance_id: &str, config: PendingConfig) {
        let mut configs = self.configs.write().await;
        configs.insert(instance_id.to_string(), config);
    }

    /// Get and remove a pending config.
    pub async fn take(&self, instance_id: &str) -> Option<PendingConfig> {
        let mut configs = self.configs.write().await;
        configs.remove(instance_id)
    }

    /// Remove a pending config without returning it.
    pub async fn remove(&self, instance_id: &str) {
        let mut configs = self.configs.write().await;
        configs.remove(instance_id);
    }
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Config Delivery Service
// =============================================================================

/// Config delivery service that handles vsock connections from guest-init.
pub struct ConfigDeliveryService {
    config_store: Arc<ConfigStore>,
}

impl ConfigDeliveryService {
    /// Create a new config delivery service.
    pub fn new(config_store: Arc<ConfigStore>) -> Self {
        Self { config_store }
    }

    /// Run the config delivery service, listening for guest-init connections.
    pub async fn run(&self) -> Result<()> {
        let addr = VsockAddr::new(VMADDR_CID_HOST, CONFIG_PORT);

        let listener = VsockListener::bind(&addr).map_err(|e| {
            anyhow!(
                "Failed to bind vsock listener on port {}: {}",
                CONFIG_PORT,
                e
            )
        })?;

        info!(port = CONFIG_PORT, "Config delivery service listening");

        loop {
            match listener.accept() {
                Ok((stream, peer)) => {
                    let cid = peer.cid();
                    info!(cid = cid, "Guest connection accepted");

                    let config_store = Arc::clone(&self.config_store);
                    tokio::task::spawn_blocking(move || {
                        if let Err(e) = handle_connection(stream, config_store) {
                            error!(cid = cid, error = %e, "Connection handler failed");
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, "Accept failed");
                }
            }
        }
    }
}

/// Handle a single guest-init connection.
fn handle_connection(mut stream: VsockStream, config_store: Arc<ConfigStore>) -> Result<()> {
    // Read hello message
    let hello =
        read_message::<HelloMessage>(&mut stream).context("Failed to read hello message")?;

    if hello.msg_type != "hello" {
        return Err(anyhow!(
            "Expected 'hello' message, got '{}'",
            hello.msg_type
        ));
    }

    info!(
        instance_id = %hello.instance_id,
        boot_id = %hello.boot_id,
        guest_init_version = %hello.guest_init_version,
        protocol = hello.guest_init_protocol,
        "Received hello from guest-init"
    );

    // Check protocol version
    if hello.guest_init_protocol != PROTOCOL_VERSION {
        error!(
            expected = PROTOCOL_VERSION,
            got = hello.guest_init_protocol,
            "Protocol version mismatch"
        );
        return Err(anyhow!(
            "Protocol version mismatch: expected {}, got {}",
            PROTOCOL_VERSION,
            hello.guest_init_protocol
        ));
    }

    // Get pending config for this instance
    // Note: This is a blocking call in spawn_blocking context
    let pending = tokio::runtime::Handle::current().block_on(config_store.take(&hello.instance_id));

    let pending = match pending {
        Some(p) => p,
        None => {
            error!(instance_id = %hello.instance_id, "No pending config for instance");
            return Err(anyhow!(
                "No pending config for instance {}",
                hello.instance_id
            ));
        }
    };

    // Build config message
    let config_msg = build_config_message(&hello.instance_id, &pending);

    // Send config
    send_message(&mut stream, &config_msg).context("Failed to send config")?;
    debug!(instance_id = %hello.instance_id, "Sent config to guest-init");

    // Read ack
    let ack = read_message::<AckMessage>(&mut stream).context("Failed to read ack")?;

    if ack.msg_type != "ack" {
        return Err(anyhow!("Expected 'ack' message, got '{}'", ack.msg_type));
    }

    info!(
        instance_id = %hello.instance_id,
        generation = ack.generation,
        "Config ack received"
    );

    // Continue reading status messages
    loop {
        match read_message::<StatusMessage>(&mut stream) {
            Ok(status) => {
                if status.msg_type != "status" {
                    warn!(
                        instance_id = %hello.instance_id,
                        msg_type = %status.msg_type,
                        "Unexpected message type, ignoring"
                    );
                    continue;
                }

                info!(
                    instance_id = %hello.instance_id,
                    state = %status.state,
                    reason = ?status.reason,
                    exit_code = ?status.exit_code,
                    "Guest status update"
                );

                // Handle terminal states
                if status.state == "failed" || status.state == "exited" {
                    break;
                }
            }
            Err(e) => {
                debug!(
                    instance_id = %hello.instance_id,
                    error = %e,
                    "Connection closed or error reading status"
                );
                break;
            }
        }
    }

    Ok(())
}

/// Build a config message from the pending config.
fn build_config_message(instance_id: &str, pending: &PendingConfig) -> ConfigMessage {
    let plan = &pending.plan;

    // Convert env_vars from JSON to HashMap
    let env: HashMap<String, String> = match plan.env_vars.as_object() {
        Some(obj) => obj
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        None => HashMap::new(),
    };

    // Build workload config
    // Note: argv would come from the image or release config
    // For now, we use a placeholder
    let workload = WorkloadConfig {
        argv: vec!["./start".to_string()], // TODO: Get from release config
        cwd: "/app".to_string(),
        env,
        uid: 1000,
        gid: 1000,
        stdin: false,
        tty: false,
    };

    // Build network config
    let network = NetworkConfig {
        overlay_ipv6: pending.overlay_ipv6.clone(),
        gateway_ipv6: pending.gateway_ipv6.clone(),
        prefix_len: 128,
        mtu: 1420,
        dns: vec!["fd00::53".to_string()], // TODO: From platform config
        hostname: Some(format!("i-{}", instance_id)),
    };

    // Build mount configs from volumes
    let mounts: Vec<MountConfig> = plan
        .volumes
        .iter()
        .enumerate()
        .map(|(i, vol)| MountConfig {
            kind: "volume".to_string(),
            name: vol.volume_id.clone(),
            device: Some(format!("/dev/vd{}", (b'c' + i as u8) as char)), // vdc, vdd, etc.
            mountpoint: vol.mount_path.clone(),
            fs_type: "ext4".to_string(),
            mode: if vol.read_only { "ro" } else { "rw" }.to_string(),
        })
        .collect();

    // Build secrets config
    let secrets = pending.secrets_data.as_ref().map(|data| SecretsConfig {
        required: true,
        path: "/run/secrets/platform.env".to_string(),
        mode: "0400".to_string(),
        owner_uid: 0,
        owner_gid: 0,
        format: "dotenv".to_string(),
        bundle_version_id: None,
        data: Some(data.clone()),
    });

    // Exec config
    let exec = ExecConfig {
        vsock_port: 5162,
        enabled: true,
    };

    ConfigMessage {
        msg_type: "config".to_string(),
        config_version: CONFIG_VERSION.to_string(),
        instance_id: instance_id.to_string(),
        generation: pending.generation,
        workload,
        network,
        mounts,
        secrets,
        exec,
    }
}

/// Read a JSON message from the stream.
fn read_message<T: serde::de::DeserializeOwned>(stream: &mut VsockStream) -> Result<T> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    reader.read_line(&mut line).context("Failed to read line")?;

    if line.is_empty() {
        return Err(anyhow!("Connection closed"));
    }

    serde_json::from_str(&line).context("Failed to parse JSON message")
}

/// Send a JSON message to the stream.
fn send_message<T: serde::Serialize>(stream: &mut VsockStream, msg: &T) -> Result<()> {
    let json = serde_json::to_string(msg).context("Failed to serialize message")?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_deserialization() {
        let json = r#"{
            "type": "hello",
            "guest_init_version": "1.0.0",
            "guest_init_protocol": 1,
            "instance_id": "inst_123",
            "boot_id": "boot_456"
        }"#;

        let hello: HelloMessage = serde_json::from_str(json).unwrap();
        assert_eq!(hello.msg_type, "hello");
        assert_eq!(hello.instance_id, "inst_123");
        assert_eq!(hello.guest_init_protocol, 1);
    }

    #[test]
    fn test_config_serialization() {
        let config = ConfigMessage {
            msg_type: "config".to_string(),
            config_version: "v1".to_string(),
            instance_id: "inst_123".to_string(),
            generation: 7,
            workload: WorkloadConfig {
                argv: vec!["./server".to_string()],
                cwd: "/app".to_string(),
                env: HashMap::new(),
                uid: 1000,
                gid: 1000,
                stdin: false,
                tty: false,
            },
            network: NetworkConfig {
                overlay_ipv6: "fd00::1234".to_string(),
                gateway_ipv6: "fd00::1".to_string(),
                prefix_len: 128,
                mtu: 1420,
                dns: vec!["fd00::53".to_string()],
                hostname: Some("i-inst_123".to_string()),
            },
            mounts: vec![],
            secrets: None,
            exec: ExecConfig {
                vsock_port: 5162,
                enabled: true,
            },
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"type\":\"config\""));
        assert!(json.contains("\"overlay_ipv6\":\"fd00::1234\""));
    }

    #[test]
    fn test_status_deserialization() {
        let json = r#"{
            "type": "status",
            "state": "ready",
            "timestamp": "2025-12-17T12:00:00Z"
        }"#;

        let status: StatusMessage = serde_json::from_str(json).unwrap();
        assert_eq!(status.state, "ready");
        assert!(status.reason.is_none());

        let json_failed = r#"{
            "type": "status",
            "state": "failed",
            "reason": "mount_failed",
            "detail": "ext4 error",
            "timestamp": "2025-12-17T12:00:00Z"
        }"#;

        let status_failed: StatusMessage = serde_json::from_str(json_failed).unwrap();
        assert_eq!(status_failed.state, "failed");
        assert_eq!(status_failed.reason, Some("mount_failed".to_string()));
    }

    #[tokio::test]
    async fn test_config_store() {
        let store = ConfigStore::new();

        let plan = InstancePlan {
            instance_id: "inst_test".to_string(),
            app_id: "app_test".to_string(),
            env_id: "env_test".to_string(),
            release_id: "rel_test".to_string(),
            deploy_id: "dep_test".to_string(),
            image: "test:latest".to_string(),
            resources: crate::client::InstanceResources {
                cpu: 1.0,
                memory_bytes: 512 * 1024 * 1024,
            },
            env_vars: serde_json::json!({"PORT": "8080"}),
            volumes: vec![],
        };

        let pending = PendingConfig {
            plan,
            overlay_ipv6: "fd00::1234".to_string(),
            gateway_ipv6: "fd00::1".to_string(),
            generation: 1,
            secrets_data: None,
        };

        store.add("inst_test", pending.clone()).await;

        let retrieved = store.take("inst_test").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().overlay_ipv6, "fd00::1234");

        // Should be removed now
        let again = store.take("inst_test").await;
        assert!(again.is_none());
    }
}
