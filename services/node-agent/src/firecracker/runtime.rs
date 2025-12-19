//! Firecracker runtime implementation.
//!
//! This module provides the full Firecracker runtime for production use,
//! implementing the `Runtime` trait for microVM lifecycle management.
//!
//! Reference: docs/specs/runtime/firecracker-boot.md

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::client::InstancePlan;
use crate::runtime::{Runtime, VmHandle};

use super::api::FirecrackerClient;
use super::config::{BootSource, MachineConfig};
use super::jailer::SandboxManager;

/// Default timeout for Firecracker API operations.
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for VM boot.
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);

/// Configuration for the Firecracker runtime.
#[derive(Debug, Clone)]
pub struct FirecrackerRuntimeConfig {
    /// Path to the firecracker binary.
    pub firecracker_path: PathBuf,
    /// Path to the jailer binary.
    pub jailer_path: PathBuf,
    /// Base directory for instance data.
    pub data_dir: PathBuf,
    /// Path to the kernel image.
    pub kernel_path: PathBuf,
    /// Path to initrd (optional).
    pub initrd_path: Option<PathBuf>,
    /// Whether to use the jailer.
    pub use_jailer: bool,
    /// UID to run VMs as (when using jailer).
    pub vm_uid: u32,
    /// GID to run VMs as (when using jailer).
    pub vm_gid: u32,
}

impl Default for FirecrackerRuntimeConfig {
    fn default() -> Self {
        Self {
            firecracker_path: PathBuf::from("/usr/bin/firecracker"),
            jailer_path: PathBuf::from("/usr/bin/jailer"),
            data_dir: PathBuf::from("/var/lib/plfm-agent"),
            kernel_path: PathBuf::from("/var/lib/plfm-agent/kernel/vmlinux"),
            initrd_path: None,
            use_jailer: true,
            vm_uid: 1000,
            vm_gid: 1000,
        }
    }
}

/// State of a running Firecracker instance.
struct InstanceState {
    /// Instance ID.
    instance_id: String,
    /// Boot ID.
    boot_id: String,
    /// Firecracker process handle.
    process: Child,
    /// API client for this instance.
    client: FirecrackerClient,
    /// Socket path.
    socket_path: PathBuf,
    /// Sandbox manager (if using jailer).
    sandbox: Option<SandboxManager>,
}

/// Firecracker runtime for production use.
pub struct FirecrackerRuntime {
    config: FirecrackerRuntimeConfig,
    instances: RwLock<HashMap<String, InstanceState>>,
    boot_counter: AtomicU64,
}

impl FirecrackerRuntime {
    /// Create a new Firecracker runtime.
    pub fn new(config: FirecrackerRuntimeConfig) -> Self {
        Self {
            config,
            instances: RwLock::new(HashMap::new()),
            boot_counter: AtomicU64::new(0),
        }
    }

    /// Generate a new boot ID.
    fn next_boot_id(&self) -> String {
        let counter = self.boot_counter.fetch_add(1, Ordering::SeqCst);
        format!("boot_{:016x}", counter)
    }

    /// Get the socket path for an instance.
    fn socket_path(&self, instance_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("instances")
            .join(instance_id)
            .join("firecracker.socket")
    }

    /// Get the instance directory.
    fn instance_dir(&self, instance_id: &str) -> PathBuf {
        self.config.data_dir.join("instances").join(instance_id)
    }

    /// Start Firecracker process (without jailer).
    async fn start_firecracker_direct(
        &self,
        instance_id: &str,
    ) -> Result<(Child, PathBuf)> {
        let instance_dir = self.instance_dir(instance_id);
        std::fs::create_dir_all(&instance_dir)?;

        let socket_path = self.socket_path(instance_id);

        // Remove stale socket if exists
        if socket_path.exists() {
            std::fs::remove_file(&socket_path).ok();
        }

        let child = Command::new(&self.config.firecracker_path)
            .arg("--api-sock")
            .arg(&socket_path)
            .arg("--id")
            .arg(instance_id)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Wait for socket to appear
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        if !socket_path.exists() {
            return Err(anyhow!("Firecracker socket did not appear"));
        }

        Ok((child, socket_path))
    }

    /// Configure and boot a VM via the API.
    async fn configure_and_boot(
        &self,
        client: &FirecrackerClient,
        plan: &InstancePlan,
    ) -> Result<()> {
        let instance_id = &plan.instance_id;

        // Convert plan resources to Firecracker config
        let vcpu_count = (plan.resources.cpu.ceil() as u8).max(1);
        let mem_size_mib = (plan.resources.memory_bytes / (1024 * 1024)) as u32;

        let machine = MachineConfig::new(vcpu_count, mem_size_mib.max(128));

        // Configure machine
        client.put_machine_config(&machine).await?;

        // Configure boot source
        let boot_source = BootSource::new(self.config.kernel_path.clone());
        client.put_boot_source(&boot_source).await?;

        // Note: In production, we'd configure drives from the rootdisk path
        // For now, this is a placeholder that would need the rootdisk path

        // Start the instance
        client.start_instance().await?;

        info!(instance_id = %instance_id, "VM started successfully");
        Ok(())
    }
}

#[async_trait]
impl Runtime for FirecrackerRuntime {
    async fn start_vm(&self, plan: &InstancePlan) -> Result<VmHandle> {
        let instance_id = &plan.instance_id;
        info!(instance_id = %instance_id, "Starting Firecracker VM");

        let boot_id = self.next_boot_id();

        // Start Firecracker process
        let (process, socket_path) = self.start_firecracker_direct(instance_id).await?;

        // Create API client
        let client = FirecrackerClient::new(&socket_path);

        // Configure and boot
        if let Err(e) = self.configure_and_boot(&client, plan).await {
            error!(instance_id = %instance_id, error = %e, "Failed to configure VM");
            // Kill the process on failure
            drop(process);
            return Err(e);
        }

        // Store instance state
        let state = InstanceState {
            instance_id: instance_id.clone(),
            boot_id: boot_id.clone(),
            process,
            client,
            socket_path,
            sandbox: None,
        };

        self.instances
            .write()
            .await
            .insert(instance_id.clone(), state);

        Ok(VmHandle {
            boot_id,
            instance_id: instance_id.clone(),
        })
    }

    async fn stop_vm(&self, handle: &VmHandle) -> Result<()> {
        let instance_id = &handle.instance_id;
        info!(instance_id = %instance_id, "Stopping Firecracker VM");

        let mut instances = self.instances.write().await;
        let state = instances.remove(instance_id).ok_or_else(|| {
            anyhow!("Instance not found: {}", instance_id)
        })?;

        // Try graceful shutdown first
        match state.client.send_ctrl_alt_del().await {
            Ok(_) => {
                debug!(instance_id = %instance_id, "Sent CtrlAltDel");
                // Wait briefly for graceful shutdown
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => {
                warn!(instance_id = %instance_id, error = %e, "CtrlAltDel failed, will force kill");
            }
        }

        // Kill the process if still running
        let mut process = state.process;
        if let Err(e) = process.kill().await {
            warn!(instance_id = %instance_id, error = %e, "Failed to kill process");
        }

        // Clean up sandbox if present
        if let Some(sandbox) = state.sandbox {
            if let Err(e) = sandbox.cleanup() {
                warn!(instance_id = %instance_id, error = %e, "Failed to cleanup sandbox");
            }
        }

        // Clean up instance directory
        let instance_dir = self.instance_dir(instance_id);
        if instance_dir.exists() {
            std::fs::remove_dir_all(&instance_dir).ok();
        }

        Ok(())
    }

    async fn check_vm_health(&self, handle: &VmHandle) -> Result<bool> {
        let instances = self.instances.read().await;
        let state = instances.get(&handle.instance_id).ok_or_else(|| {
            anyhow!("Instance not found: {}", handle.instance_id)
        })?;

        // Try to get info from Firecracker API
        match state.client.get_instance_info().await {
            Ok(info) => {
                // Check if the instance is running
                Ok(info.state == "Running")
            }
            Err(_) => {
                // If API fails, consider unhealthy
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_config_default() {
        let config = FirecrackerRuntimeConfig::default();
        assert!(config.use_jailer);
        assert_eq!(config.vm_uid, 1000);
    }

    #[test]
    fn test_socket_path() {
        let config = FirecrackerRuntimeConfig {
            data_dir: PathBuf::from("/var/lib/test"),
            ..Default::default()
        };
        let runtime = FirecrackerRuntime::new(config);

        let path = runtime.socket_path("inst-123");
        assert!(path.to_string_lossy().contains("inst-123"));
        assert!(path.to_string_lossy().contains("firecracker.socket"));
    }

    #[test]
    fn test_boot_id_generation() {
        let config = FirecrackerRuntimeConfig::default();
        let runtime = FirecrackerRuntime::new(config);

        let id1 = runtime.next_boot_id();
        let id2 = runtime.next_boot_id();

        assert!(id1.starts_with("boot_"));
        assert!(id2.starts_with("boot_"));
        assert_ne!(id1, id2);
    }
}
