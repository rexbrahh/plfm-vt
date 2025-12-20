//! TAP device creation and management.
//!
//! Creates and configures TAP devices for Firecracker microVMs.
//! Each instance gets a dedicated TAP device that Firecracker uses
//! for its virtio-net interface (eth0 inside the guest).
//!
//! Host-side setup:
//! - TAP device named `tap-{instance_id_suffix}`
//! - Link-local IPv6 address (fe80::1) as gateway
//! - MTU matching overlay network
//! - Proxy NDP enabled for instance overlay address
//!
//! Reference: docs/specs/runtime/networking-inside-vm.md

use std::process::Command;

use anyhow::{Context, Result};
use thiserror::Error;
use tracing::{debug, info, warn};

/// TAP device configuration.
#[derive(Debug, Clone)]
pub struct TapConfig {
    /// Instance ID (used for naming).
    pub instance_id: String,
    /// Instance overlay IPv6 address.
    pub overlay_ipv6: String,
    /// Gateway IPv6 address (link-local, typically fe80::1).
    pub gateway_ipv6: String,
    /// MTU (default 1420).
    pub mtu: u32,
}

impl TapConfig {
    /// Create a new TAP configuration.
    pub fn new(instance_id: &str, overlay_ipv6: &str) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            overlay_ipv6: overlay_ipv6.to_string(),
            gateway_ipv6: "fe80::1".to_string(),
            mtu: 1420,
        }
    }

    /// Set custom MTU.
    pub fn with_mtu(mut self, mtu: u32) -> Self {
        self.mtu = mtu;
        self
    }

    /// Get the TAP device name.
    pub fn tap_name(&self) -> String {
        // Use last 8 chars of instance_id for short unique name
        // TAP names are limited to 15 chars (IFNAMSIZ - 1)
        let suffix = if self.instance_id.len() > 8 {
            &self.instance_id[self.instance_id.len() - 8..]
        } else {
            &self.instance_id
        };
        format!("tap-{}", suffix)
    }
}

/// Errors from TAP device operations.
#[derive(Debug, Error)]
pub enum TapError {
    #[error("failed to create TAP device: {0}")]
    CreateFailed(String),

    #[error("failed to configure TAP device: {0}")]
    ConfigFailed(String),

    #[error("failed to add route: {0}")]
    RouteFailed(String),

    #[error("failed to delete TAP device: {0}")]
    DeleteFailed(String),

    #[error("command execution failed: {0}")]
    CommandFailed(#[from] std::io::Error),
}

/// Handle to a created TAP device.
#[derive(Debug)]
pub struct TapDevice {
    /// TAP device name.
    name: String,
    /// Instance ID.
    instance_id: String,
    /// Overlay IPv6 for routing cleanup.
    overlay_ipv6: String,
}

impl TapDevice {
    /// Get the TAP device name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the instance ID.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Clean up the TAP device (delete it).
    pub fn cleanup(&self) -> Result<(), TapError> {
        delete_tap(&self.name, &self.overlay_ipv6)
    }
}

impl Drop for TapDevice {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            warn!(
                tap = %self.name,
                error = %e,
                "Failed to cleanup TAP device on drop"
            );
        }
    }
}

/// Create and configure a TAP device for an instance.
///
/// This sets up:
/// 1. TAP device with specified name
/// 2. MTU configuration
/// 3. Link-local IPv6 gateway address
/// 4. Route for instance overlay IPv6
/// 5. Proxy NDP (if available)
pub fn create_tap(config: &TapConfig) -> Result<TapDevice, TapError> {
    let tap_name = config.tap_name();

    info!(
        tap = %tap_name,
        instance_id = %config.instance_id,
        overlay_ipv6 = %config.overlay_ipv6,
        mtu = config.mtu,
        "Creating TAP device"
    );

    // Create TAP device
    run_ip(&["tuntap", "add", "dev", &tap_name, "mode", "tap"])
        .map_err(|e| TapError::CreateFailed(e.to_string()))?;

    // Set MTU
    run_ip(&["link", "set", "dev", &tap_name, "mtu", &config.mtu.to_string()])
        .map_err(|e| {
            // Try to clean up on failure
            let _ = run_ip(&["link", "delete", &tap_name]);
            TapError::ConfigFailed(format!("MTU: {}", e))
        })?;

    // Bring interface up
    run_ip(&["link", "set", "dev", &tap_name, "up"])
        .map_err(|e| {
            let _ = run_ip(&["link", "delete", &tap_name]);
            TapError::ConfigFailed(format!("bring up: {}", e))
        })?;

    // Add link-local IPv6 address (gateway from guest's perspective)
    // fe80::1/64 on the tap interface
    run_ip(&["-6", "addr", "add", &format!("{}/64", config.gateway_ipv6), "dev", &tap_name])
        .map_err(|e| {
            let _ = run_ip(&["link", "delete", &tap_name]);
            TapError::ConfigFailed(format!("gateway address: {}", e))
        })?;

    // Add route for instance overlay IPv6 via this TAP
    // This tells the host to send traffic for the instance through this TAP
    run_ip(&["-6", "route", "add", &format!("{}/128", config.overlay_ipv6), "dev", &tap_name])
        .map_err(|e| {
            let _ = run_ip(&["link", "delete", &tap_name]);
            TapError::RouteFailed(e.to_string())
        })?;

    // Enable proxy NDP for the instance address (so host responds to NDP on behalf of VM)
    // This may fail on some systems, so we just warn
    if let Err(e) = enable_proxy_ndp(&tap_name, &config.overlay_ipv6) {
        warn!(
            tap = %tap_name,
            error = %e,
            "Failed to enable proxy NDP (may not be critical)"
        );
    }

    // Enable IPv6 forwarding for this interface
    if let Err(e) = enable_ipv6_forwarding(&tap_name) {
        warn!(
            tap = %tap_name,
            error = %e,
            "Failed to enable IPv6 forwarding"
        );
    }

    debug!(tap = %tap_name, "TAP device created and configured");

    Ok(TapDevice {
        name: tap_name,
        instance_id: config.instance_id.clone(),
        overlay_ipv6: config.overlay_ipv6.clone(),
    })
}

/// Delete a TAP device and clean up routes.
fn delete_tap(tap_name: &str, overlay_ipv6: &str) -> Result<(), TapError> {
    info!(tap = %tap_name, "Deleting TAP device");

    // Remove route first (ignore errors as it may not exist)
    let _ = run_ip(&["-6", "route", "del", &format!("{}/128", overlay_ipv6), "dev", tap_name]);

    // Remove proxy NDP entry (ignore errors)
    let _ = run_ip(&["-6", "neigh", "del", "proxy", overlay_ipv6, "dev", tap_name]);

    // Delete the TAP device
    run_ip(&["link", "delete", tap_name])
        .map_err(|e| TapError::DeleteFailed(e.to_string()))?;

    debug!(tap = %tap_name, "TAP device deleted");

    Ok(())
}

/// Run an `ip` command and return result.
fn run_ip(args: &[&str]) -> Result<()> {
    let output = Command::new("ip")
        .args(args)
        .output()
        .context("failed to execute ip command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ip {} failed: {}", args.join(" "), stderr.trim());
    }

    Ok(())
}

/// Enable proxy NDP for an address on an interface.
fn enable_proxy_ndp(iface: &str, ipv6: &str) -> Result<()> {
    // Add proxy NDP entry
    run_ip(&["-6", "neigh", "add", "proxy", ipv6, "dev", iface])?;
    Ok(())
}

/// Enable IPv6 forwarding for an interface.
fn enable_ipv6_forwarding(iface: &str) -> Result<()> {
    // Write to sysctl
    let path = format!("/proc/sys/net/ipv6/conf/{}/forwarding", iface);
    std::fs::write(&path, "1").context("failed to enable IPv6 forwarding")?;
    Ok(())
}

/// Check if a TAP device exists.
#[allow(dead_code)]
pub fn tap_exists(tap_name: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{}", tap_name)).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tap_name_generation() {
        let config = TapConfig::new("inst_01JEXAMPLE123", "fd00::1234");
        let name = config.tap_name();
        
        // Should be tap- prefix + last 8 chars
        assert!(name.starts_with("tap-"));
        assert!(name.len() <= 15); // IFNAMSIZ limit
        // "inst_01JEXAMPLE123" has 18 chars, last 8 = "AMPLE123"
        assert_eq!(name, "tap-AMPLE123");
    }

    #[test]
    fn test_tap_name_short_instance_id() {
        let config = TapConfig::new("inst_1", "fd00::1234");
        let name = config.tap_name();
        
        assert_eq!(name, "tap-inst_1");
    }

    #[test]
    fn test_tap_config_builder() {
        let config = TapConfig::new("inst_test", "fd00::abcd")
            .with_mtu(9000);
        
        assert_eq!(config.mtu, 9000);
        assert_eq!(config.gateway_ipv6, "fe80::1");
        assert_eq!(config.overlay_ipv6, "fd00::abcd");
    }

    #[test]
    fn test_default_gateway() {
        let config = TapConfig::new("inst_test", "fd00::1234");
        assert_eq!(config.gateway_ipv6, "fe80::1");
    }
}
