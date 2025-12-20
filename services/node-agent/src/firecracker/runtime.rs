//! Firecracker runtime implementation.
//!
//! This module provides the full Firecracker runtime for production use,
//! implementing the `Runtime` trait for microVM lifecycle management.
//!
//! Reference: docs/specs/runtime/firecracker-boot.md

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::client::{ControlPlaneClient, InstancePlan, WorkloadLogEntry};
use crate::image::{parse_image_ref, ImagePuller};
use crate::network::{create_tap, TapConfig, TapDevice};
use crate::runtime::{Runtime, VmHandle};

use super::api::FirecrackerClient;
use super::config::{
    generate_mac_address, BootSource, DriveConfig, MachineConfig, NetworkInterface, VsockConfig,
};
use super::jailer::SandboxManager;

/// Default timeout for Firecracker API operations.
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for VM boot.
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const LOG_BATCH_SIZE: usize = 100;
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const MAX_LOG_LINE_BYTES: usize = 16 * 1024;
const DEFAULT_SCRATCH_DISK_BYTES: u64 = 1024 * 1024 * 1024;
const GUEST_CID_START: u64 = 3;

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
    /// Scratch disk size in bytes.
    pub scratch_disk_bytes: u64,
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
            scratch_disk_bytes: DEFAULT_SCRATCH_DISK_BYTES,
        }
    }
}

/// State of a running Firecracker instance.
struct InstanceState {
    /// Instance ID.
    #[allow(dead_code)]
    instance_id: String,
    /// Boot ID.
    #[allow(dead_code)]
    boot_id: String,
    /// Firecracker process handle.
    process: Child,
    /// API client for this instance.
    client: FirecrackerClient,
    /// Socket path.
    #[allow(dead_code)]
    socket_path: PathBuf,
    /// Guest CID for vsock.
    guest_cid: u32,
    /// Image digest for cache release.
    image_digest: String,
    /// Scratch disk path for cleanup.
    scratch_path: PathBuf,
    /// TAP device for networking.
    tap_device: Option<TapDevice>,
    /// Sandbox manager (if using jailer).
    sandbox: Option<SandboxManager>,
}

/// Firecracker runtime for production use.
pub struct FirecrackerRuntime {
    config: FirecrackerRuntimeConfig,
    instances: RwLock<HashMap<String, InstanceState>>,
    boot_counter: AtomicU64,
    guest_cid_counter: AtomicU64,
    image_puller: Arc<ImagePuller>,
    control_plane: Option<Arc<ControlPlaneClient>>,
}

impl FirecrackerRuntime {
    /// Create a new Firecracker runtime.
    pub fn new(
        config: FirecrackerRuntimeConfig,
        image_puller: Arc<ImagePuller>,
        control_plane: Option<Arc<ControlPlaneClient>>,
    ) -> Self {
        Self {
            config,
            instances: RwLock::new(HashMap::new()),
            boot_counter: AtomicU64::new(0),
            guest_cid_counter: AtomicU64::new(GUEST_CID_START),
            image_puller,
            control_plane,
        }
    }

    /// Generate a new boot ID.
    fn next_boot_id(&self) -> String {
        let counter = self.boot_counter.fetch_add(1, Ordering::SeqCst);
        format!("boot_{:016x}", counter)
    }

    async fn allocate_guest_cid(&self) -> u32 {
        loop {
            let cid = self
                .guest_cid_counter
                .fetch_add(1, Ordering::SeqCst)
                .max(GUEST_CID_START) as u32;
            let instances = self.instances.read().await;
            if instances.values().all(|state| state.guest_cid != cid) {
                return cid;
            }
        }
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

    fn scratch_path(&self, instance_id: &str) -> PathBuf {
        self.instance_dir(instance_id).join("scratch.ext4")
    }

    fn vsock_path(&self, instance_id: &str) -> PathBuf {
        self.instance_dir(instance_id).join("vsock.sock")
    }

    fn volume_path(&self, volume_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("volumes")
            .join(format!("{volume_id}.ext4"))
    }

    /// Start Firecracker process (without jailer).
    async fn start_firecracker_direct(&self, instance_id: &str) -> Result<(Child, PathBuf)> {
        let instance_dir = self.instance_dir(instance_id);
        std::fs::create_dir_all(&instance_dir)?;

        let socket_path = self.socket_path(instance_id);

        // Remove stale socket if exists
        if socket_path.exists() {
            std::fs::remove_file(&socket_path).ok();
        }

        let log_path = instance_dir.join("firecracker.log");
        let metrics_path = instance_dir.join("firecracker.metrics");

        let child = Command::new(&self.config.firecracker_path)
            .arg("--api-sock")
            .arg(&socket_path)
            .arg("--id")
            .arg(instance_id)
            .arg("--log-path")
            .arg(&log_path)
            .arg("--metrics-path")
            .arg(&metrics_path)
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
    ///
    /// Returns the TAP device that was created for this VM, if networking was configured.
    async fn configure_and_boot(
        &self,
        client: &FirecrackerClient,
        plan: &InstancePlan,
        root_disk_path: &PathBuf,
        scratch_path: &PathBuf,
        guest_cid: u32,
    ) -> Result<Option<TapDevice>> {
        let instance_id = &plan.instance_id;

        // Convert plan resources to Firecracker config
        let vcpu_count = (plan.resources.cpu.ceil() as u8).max(1);
        let mem_size_mib = (plan.resources.memory_bytes / (1024 * 1024)) as u32;

        let machine = MachineConfig::new(vcpu_count, mem_size_mib.max(128));

        // Configure machine
        client.put_machine_config(&machine).await?;

        // Configure boot source
        let mut boot_source = BootSource::new(self.config.kernel_path.clone());
        if let Some(initrd) = &self.config.initrd_path {
            boot_source = boot_source.with_initrd(initrd.clone());
        }
        client.put_boot_source(&boot_source).await?;

        // Configure root and scratch drives
        let root_drive = DriveConfig::root_disk(root_disk_path.clone());
        client.put_drive(&root_drive).await?;

        let scratch_drive = DriveConfig::scratch_disk(scratch_path.clone());
        client.put_drive(&scratch_drive).await?;

        // Configure volume drives (sorted by volume_id for deterministic mapping)
        let mut volumes = plan.volumes.clone();
        volumes.sort_by(|a, b| a.volume_id.cmp(&b.volume_id));

        for (idx, volume) in volumes.iter().enumerate() {
            let path = self.volume_path(&volume.volume_id);
            if !path.exists() {
                return Err(anyhow!(
                    "volume device missing for {} at {}",
                    volume.volume_id,
                    path.display()
                ));
            }

            let drive_id = format!("vol-{}", idx);
            let drive = DriveConfig::new(&drive_id, path, false).read_only(volume.read_only);
            client.put_drive(&drive).await?;
        }

        let vsock = VsockConfig::new(guest_cid, self.vsock_path(instance_id));
        client.put_vsock(&vsock).await?;

        // Configure networking if overlay_ipv6 is provided
        let tap_device = if !plan.overlay_ipv6.is_empty() {
            let tap_config = TapConfig::new(instance_id, &plan.overlay_ipv6);
            let tap_device = create_tap(&tap_config).map_err(|e| {
                error!(instance_id = %instance_id, error = %e, "Failed to create TAP device");
                anyhow!("Failed to create TAP device: {}", e)
            })?;

            // Configure network interface in Firecracker
            let mac = generate_mac_address(instance_id);
            let net_iface = NetworkInterface::new("eth0", tap_device.name()).with_mac(&mac);

            client.put_network_interface(&net_iface).await.map_err(|e| {
                error!(instance_id = %instance_id, error = %e, "Failed to configure network interface");
                // TAP will be cleaned up when tap_device is dropped
                anyhow!("Failed to configure network interface: {}", e)
            })?;

            info!(
                instance_id = %instance_id,
                tap = %tap_device.name(),
                mac = %mac,
                overlay_ipv6 = %plan.overlay_ipv6,
                "Network configured"
            );

            Some(tap_device)
        } else {
            warn!(instance_id = %instance_id, "No overlay_ipv6 provided, skipping network configuration");
            None
        };

        // Start the instance
        client.start_instance().await?;

        info!(instance_id = %instance_id, "VM started successfully");
        Ok(tap_device)
    }

    fn spawn_log_pipeline(
        &self,
        instance_id: &str,
        stdout: Option<tokio::process::ChildStdout>,
        stderr: Option<tokio::process::ChildStderr>,
    ) {
        if stdout.is_none() && stderr.is_none() {
            return;
        }

        let Some(control_plane) = self.control_plane.clone() else {
            if let Some(stdout) = stdout {
                tokio::spawn(drain_stream(stdout));
            }
            if let Some(stderr) = stderr {
                tokio::spawn(drain_stream(stderr));
            }
            return;
        };

        let (tx, rx) = mpsc::channel(LOG_BATCH_SIZE * 2);
        tokio::spawn(run_log_shipper(rx, control_plane));

        let instance_id = instance_id.to_string();
        if let Some(stdout) = stdout {
            let tx_clone = tx.clone();
            tokio::spawn(run_log_reader(
                stdout,
                "stdout",
                instance_id.clone(),
                tx_clone,
            ));
        }
        if let Some(stderr) = stderr {
            tokio::spawn(run_log_reader(stderr, "stderr", instance_id, tx));
        }
    }
}

#[async_trait]
impl Runtime for FirecrackerRuntime {
    async fn start_vm(&self, plan: &InstancePlan) -> Result<VmHandle> {
        let instance_id = &plan.instance_id;
        info!(instance_id = %instance_id, "Starting Firecracker VM");

        let boot_id = self.next_boot_id();
        let guest_cid = self.allocate_guest_cid().await;

        let (registry, repo, reference) = parse_image_ref(&plan.image)
            .map_err(|e| anyhow!("Invalid image reference {}: {}", plan.image, e))?;
        let pull_result = self
            .image_puller
            .ensure_image(&plan.image, &registry, &repo, &reference)
            .await
            .map_err(|e| anyhow!("Failed to pull image: {}", e))?;
        let root_disk_path = pull_result.root_disk_path.clone();
        let image_digest = pull_result.digest.clone();

        // Start Firecracker process
        let (mut process, socket_path) = self.start_firecracker_direct(instance_id).await?;

        let scratch_path = self.scratch_path(instance_id);
        if let Err(e) = ensure_scratch_disk(&scratch_path, self.config.scratch_disk_bytes) {
            let _ = process.kill().await;
            self.image_puller.release_image(&image_digest).await;
            return Err(e);
        }

        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        self.spawn_log_pipeline(instance_id, stdout, stderr);

        // Create API client
        let client = FirecrackerClient::new(&socket_path);

        // Configure and boot (this also creates the TAP device if needed)
        let tap_device = match self
            .configure_and_boot(&client, plan, &root_disk_path, &scratch_path, guest_cid)
            .await
        {
            Ok(tap) => tap,
            Err(e) => {
                error!(instance_id = %instance_id, error = %e, "Failed to configure VM");
                // Kill the process on failure
                let _ = process.kill().await;
                let _ = fs::remove_file(&scratch_path);
                self.image_puller.release_image(&image_digest).await;
                return Err(e);
            }
        };

        // Store instance state
        let state = InstanceState {
            instance_id: instance_id.clone(),
            boot_id: boot_id.clone(),
            process,
            client,
            socket_path,
            guest_cid,
            image_digest,
            scratch_path,
            tap_device,
            sandbox: None,
        };

        self.instances
            .write()
            .await
            .insert(instance_id.clone(), state);

        Ok(VmHandle {
            boot_id,
            instance_id: instance_id.clone(),
            guest_cid,
        })
    }

    async fn stop_vm(&self, handle: &VmHandle) -> Result<()> {
        let instance_id = &handle.instance_id;
        info!(instance_id = %instance_id, "Stopping Firecracker VM");

        let mut instances = self.instances.write().await;
        let state = instances
            .remove(instance_id)
            .ok_or_else(|| anyhow!("Instance not found: {}", instance_id))?;

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

        // Clean up TAP device if present
        if let Some(tap) = state.tap_device {
            if let Err(e) = tap.cleanup() {
                warn!(instance_id = %instance_id, error = %e, "Failed to cleanup TAP device");
            }
        }

        // Clean up sandbox if present
        if let Some(sandbox) = state.sandbox {
            if let Err(e) = sandbox.cleanup() {
                warn!(instance_id = %instance_id, error = %e, "Failed to cleanup sandbox");
            }
        }

        self.image_puller.release_image(&state.image_digest).await;

        // Clean up instance directory
        let instance_dir = self.instance_dir(instance_id);
        if instance_dir.exists() {
            std::fs::remove_dir_all(&instance_dir).ok();
        }

        Ok(())
    }

    async fn check_vm_health(&self, handle: &VmHandle) -> Result<bool> {
        let instances = self.instances.read().await;
        let state = instances
            .get(&handle.instance_id)
            .ok_or_else(|| anyhow!("Instance not found: {}", handle.instance_id))?;

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

fn ensure_scratch_disk(path: &PathBuf, size: u64) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = fs::File::create(path)?;
    file.set_len(size)?;
    drop(file);

    let status = std::process::Command::new("mkfs.ext4")
        .args(["-F", "-q"])
        .arg(path)
        .status()
        .map_err(|e| anyhow!("mkfs.ext4 failed: {e}"))?;

    if !status.success() {
        return Err(anyhow!("mkfs.ext4 failed"));
    }

    Ok(())
}

async fn run_log_reader<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    stream: &'static str,
    instance_id: String,
    sender: mpsc::Sender<WorkloadLogEntry>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let (line, truncated) = normalize_log_line(&line);
        let entry = WorkloadLogEntry {
            ts: Utc::now(),
            instance_id: instance_id.clone(),
            stream: stream.to_string(),
            line,
            truncated,
        };

        if sender.send(entry).await.is_err() {
            break;
        }
    }
}

async fn drain_stream<R: tokio::io::AsyncRead + Unpin>(reader: R) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(_)) = lines.next_line().await {}
}

async fn run_log_shipper(
    mut receiver: mpsc::Receiver<WorkloadLogEntry>,
    control_plane: Arc<ControlPlaneClient>,
) {
    let mut buffer: Vec<WorkloadLogEntry> = Vec::with_capacity(LOG_BATCH_SIZE);
    let mut ticker = tokio::time::interval(LOG_FLUSH_INTERVAL);

    loop {
        tokio::select! {
            Some(entry) = receiver.recv() => {
                buffer.push(entry);
                if buffer.len() >= LOG_BATCH_SIZE {
                    flush_log_batch(&mut buffer, &control_plane).await;
                }
            }
            _ = ticker.tick() => {
                if !buffer.is_empty() {
                    flush_log_batch(&mut buffer, &control_plane).await;
                }
            }
            else => break,
        }
    }

    if !buffer.is_empty() {
        flush_log_batch(&mut buffer, &control_plane).await;
    }
}

async fn flush_log_batch(buffer: &mut Vec<WorkloadLogEntry>, control_plane: &ControlPlaneClient) {
    let batch = std::mem::take(buffer);
    if let Err(e) = control_plane.send_workload_logs(batch).await {
        warn!(error = %e, "Failed to ship workload logs");
    }
}

fn normalize_log_line(line: &str) -> (String, bool) {
    if line.as_bytes().len() <= MAX_LOG_LINE_BYTES {
        return (line.to_string(), false);
    }

    let limit = MAX_LOG_LINE_BYTES.saturating_sub(3);
    let mut end = 0;
    for (idx, ch) in line.char_indices() {
        let next = idx + ch.len_utf8();
        if next > limit {
            break;
        }
        end = next;
    }

    let mut trimmed = line[..end].to_string();
    trimmed.push_str("...");
    (trimmed, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{ImageCache, ImageCacheConfig, ImagePuller, ImagePullerConfig};

    fn test_image_puller() -> Arc<ImagePuller> {
        let cache = Arc::new(ImageCache::new(ImageCacheConfig::default()));
        let puller = ImagePuller::new(ImagePullerConfig::default(), cache).unwrap();
        Arc::new(puller)
    }

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
        let runtime = FirecrackerRuntime::new(config, test_image_puller(), None);

        let path = runtime.socket_path("inst-123");
        assert!(path.to_string_lossy().contains("inst-123"));
        assert!(path.to_string_lossy().contains("firecracker.socket"));
    }

    #[test]
    fn test_boot_id_generation() {
        let config = FirecrackerRuntimeConfig::default();
        let runtime = FirecrackerRuntime::new(config, test_image_puller(), None);

        let id1 = runtime.next_boot_id();
        let id2 = runtime.next_boot_id();

        assert!(id1.starts_with("boot_"));
        assert!(id2.starts_with("boot_"));
        assert_ne!(id1, id2);
    }
}
