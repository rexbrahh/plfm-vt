//! Firecracker VM configuration structures.
//!
//! These structures map to the Firecracker API configuration objects
//! for machine configuration, boot source, drives, network interfaces, and vsock.
//!
//! Reference: https://github.com/firecracker-microvm/firecracker/blob/main/src/api_server/swagger/firecracker.yaml

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Machine configuration for the microVM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    /// Number of vCPUs (1-32).
    pub vcpu_count: u8,
    /// Memory size in MiB.
    pub mem_size_mib: u32,
    /// Enable simultaneous multithreading (hyperthreading).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smt: Option<bool>,
    /// Enable CPU template for migration compatibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_template: Option<String>,
    /// Track dirty pages for incremental snapshots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_dirty_pages: Option<bool>,
}

impl MachineConfig {
    /// Create a new machine configuration.
    pub fn new(vcpu_count: u8, mem_size_mib: u32) -> Self {
        Self {
            vcpu_count,
            mem_size_mib,
            smt: Some(false),
            cpu_template: None,
            track_dirty_pages: None,
        }
    }
}

/// Boot source configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootSource {
    /// Path to the kernel image.
    pub kernel_image_path: PathBuf,
    /// Kernel boot arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_args: Option<String>,
    /// Path to initrd (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initrd_path: Option<PathBuf>,
}

impl BootSource {
    /// Create a new boot source configuration.
    pub fn new(kernel_image_path: PathBuf) -> Self {
        Self {
            kernel_image_path,
            boot_args: Some(default_boot_args()),
            initrd_path: None,
        }
    }

    /// Set kernel boot arguments.
    pub fn with_boot_args(mut self, args: &str) -> Self {
        self.boot_args = Some(args.to_string());
        self
    }

    /// Set initrd path.
    pub fn with_initrd(mut self, path: PathBuf) -> Self {
        self.initrd_path = Some(path);
        self
    }
}

/// Default kernel boot arguments per spec.
fn default_boot_args() -> String {
    "console=ttyS0 reboot=k panic=1 pci=off ipv6.disable=0".to_string()
}

/// Block device (drive) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveConfig {
    /// Unique drive identifier.
    pub drive_id: String,
    /// Path to the drive image file.
    pub path_on_host: PathBuf,
    /// Whether this is the root device.
    pub is_root_device: bool,
    /// Whether the drive is read-only.
    pub is_read_only: bool,
    /// Optional rate limiter configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limiter: Option<RateLimiter>,
    /// Cache type (Unsafe, Writeback).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_type: Option<String>,
    /// I/O engine (Sync, Async).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_engine: Option<String>,
}

impl DriveConfig {
    /// Create a new drive configuration.
    pub fn new(drive_id: &str, path_on_host: PathBuf, is_root_device: bool) -> Self {
        Self {
            drive_id: drive_id.to_string(),
            path_on_host,
            is_root_device,
            is_read_only: false,
            rate_limiter: None,
            cache_type: None,
            io_engine: None,
        }
    }

    /// Create the root disk (vda) configuration.
    pub fn root_disk(path: PathBuf) -> Self {
        Self {
            drive_id: "rootfs".to_string(),
            path_on_host: path,
            is_root_device: true,
            is_read_only: true, // Root disk is read-only per spec
            rate_limiter: None,
            cache_type: None,
            io_engine: None,
        }
    }

    /// Create a scratch disk (vdb) configuration.
    pub fn scratch_disk(path: PathBuf) -> Self {
        Self {
            drive_id: "scratch".to_string(),
            path_on_host: path,
            is_root_device: false,
            is_read_only: false,
            rate_limiter: None,
            cache_type: None,
            io_engine: None,
        }
    }

    /// Set read-only flag.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.is_read_only = read_only;
        self
    }
}

/// Rate limiter configuration for drives or network interfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimiter {
    /// Bandwidth limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<TokenBucket>,
    /// Operations limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops: Option<TokenBucket>,
}

/// Token bucket configuration for rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBucket {
    /// Bucket size (one-time burst).
    pub size: u64,
    /// Refill time in milliseconds.
    pub refill_time: u64,
    /// Number of tokens added per refill.
    pub one_time_burst: Option<u64>,
}

/// Network interface configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInterface {
    /// Unique interface identifier.
    pub iface_id: String,
    /// Host device name (tap device).
    pub host_dev_name: String,
    /// Guest MAC address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_mac: Option<String>,
    /// Rate limiter for receive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_rate_limiter: Option<RateLimiter>,
    /// Rate limiter for transmit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_rate_limiter: Option<RateLimiter>,
}

impl NetworkInterface {
    /// Create a new network interface configuration.
    pub fn new(iface_id: &str, host_dev_name: &str) -> Self {
        Self {
            iface_id: iface_id.to_string(),
            host_dev_name: host_dev_name.to_string(),
            guest_mac: None,
            rx_rate_limiter: None,
            tx_rate_limiter: None,
        }
    }

    /// Set guest MAC address.
    pub fn with_mac(mut self, mac: &str) -> Self {
        self.guest_mac = Some(mac.to_string());
        self
    }
}

/// Generate a deterministic MAC address from instance ID.
///
/// Uses the locally administered bit (bit 1 of first byte) and unicast (bit 0 = 0).
/// Format: AA:XX:XX:XX:XX:XX where AA has bit 1 set.
pub fn generate_mac_address(instance_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    instance_id.hash(&mut hasher);
    let hash = hasher.finish();

    // Locally administered (bit 1 = 1), unicast (bit 0 = 0)
    let first_byte = ((hash >> 40) as u8 & 0xFC) | 0x02;

    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        first_byte,
        (hash >> 32) as u8,
        (hash >> 24) as u8,
        (hash >> 16) as u8,
        (hash >> 8) as u8,
        hash as u8,
    )
}

/// Vsock device configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VsockConfig {
    /// Vsock context ID (CID) for the guest.
    pub guest_cid: u32,
    /// Path to the Unix domain socket on the host.
    pub uds_path: PathBuf,
}

impl VsockConfig {
    /// Create a new vsock configuration.
    pub fn new(guest_cid: u32, uds_path: PathBuf) -> Self {
        Self {
            guest_cid,
            uds_path,
        }
    }
}

/// Full VM configuration combining all components.
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Instance identifier.
    pub instance_id: String,
    /// Machine configuration.
    pub machine: MachineConfig,
    /// Boot source.
    pub boot_source: BootSource,
    /// Block devices.
    pub drives: Vec<DriveConfig>,
    /// Network interfaces.
    pub network_interfaces: Vec<NetworkInterface>,
    /// Vsock device.
    pub vsock: Option<VsockConfig>,
}

impl VmConfig {
    /// Create a new VM configuration.
    pub fn new(instance_id: &str, machine: MachineConfig, boot_source: BootSource) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            machine,
            boot_source,
            drives: Vec::new(),
            network_interfaces: Vec::new(),
            vsock: None,
        }
    }

    /// Add a drive.
    pub fn add_drive(mut self, drive: DriveConfig) -> Self {
        self.drives.push(drive);
        self
    }

    /// Add a network interface.
    pub fn add_network(mut self, iface: NetworkInterface) -> Self {
        self.network_interfaces.push(iface);
        self
    }

    /// Set vsock configuration.
    pub fn with_vsock(mut self, vsock: VsockConfig) -> Self {
        self.vsock = Some(vsock);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_machine_config() {
        let config = MachineConfig::new(2, 512);
        assert_eq!(config.vcpu_count, 2);
        assert_eq!(config.mem_size_mib, 512);
    }

    #[test]
    fn test_generate_mac_address() {
        let mac1 = generate_mac_address("instance-1");
        let mac2 = generate_mac_address("instance-2");
        let mac1_again = generate_mac_address("instance-1");

        // MAC should be deterministic
        assert_eq!(mac1, mac1_again);
        // Different instances should have different MACs
        assert_ne!(mac1, mac2);
        // Check format (6 groups of 2 hex digits)
        assert_eq!(mac1.len(), 17);
        assert!(mac1.chars().filter(|&c| c == ':').count() == 5);
    }

    #[test]
    fn test_drive_config() {
        let root = DriveConfig::root_disk("/path/to/rootfs.ext4".into());
        assert!(root.is_root_device);
        assert!(root.is_read_only);

        let scratch = DriveConfig::scratch_disk("/path/to/scratch.ext4".into());
        assert!(!scratch.is_root_device);
        assert!(!scratch.is_read_only);
    }
}
