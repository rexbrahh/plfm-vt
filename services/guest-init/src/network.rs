//! Network configuration inside the guest.
//!
//! Configures the overlay network interface with IPv6 address, routes, and DNS.

use std::fs;
use std::net::Ipv6Addr;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{debug, info};

use crate::config::NetworkConfig;
use crate::error::InitError;

/// Network interface name (first virtio-net device).
const INTERFACE: &str = "eth0";

/// Configure networking inside the guest.
pub async fn configure(config: &NetworkConfig) -> Result<()> {
    // Validate IPv6 addresses
    let _overlay_addr: Ipv6Addr = config.overlay_ipv6.parse().map_err(|e| {
        InitError::NetConfigFailed(format!("invalid overlay_ipv6 '{}': {}", config.overlay_ipv6, e))
    })?;

    let _gateway_addr: Ipv6Addr = config.gateway_ipv6.parse().map_err(|e| {
        InitError::NetConfigFailed(format!("invalid gateway_ipv6 '{}': {}", config.gateway_ipv6, e))
    })?;

    // Set MTU
    run_ip(&["link", "set", "dev", INTERFACE, "mtu", &config.mtu.to_string()])?;
    debug!(mtu = config.mtu, "MTU set");

    // Bring interface up
    run_ip(&["link", "set", "dev", INTERFACE, "up"])?;
    debug!("interface up");

    // Add IPv6 address
    let addr_with_prefix = format!("{}/{}", config.overlay_ipv6, config.prefix_len);
    run_ip(&["-6", "addr", "add", &addr_with_prefix, "dev", INTERFACE])?;
    info!(address = %addr_with_prefix, "IPv6 address configured");

    // Add default route
    let gateway_str = config.gateway_ipv6.clone();
    run_ip(&["-6", "route", "replace", "default", "via", &gateway_str, "dev", INTERFACE])?;
    info!(gateway = %gateway_str, "default route configured");

    // Configure DNS
    if !config.dns.is_empty() {
        configure_dns(&config.dns)?;
        info!(servers = ?config.dns, "DNS configured");
    }

    // Set hostname
    if let Some(hostname) = &config.hostname {
        set_hostname(hostname)?;
        info!(hostname = %hostname, "hostname set");
    }

    Ok(())
}

/// Run an `ip` command.
fn run_ip(args: &[&str]) -> Result<()> {
    let output = Command::new("ip")
        .args(args)
        .output()
        .context("failed to execute ip command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(InitError::NetConfigFailed(format!(
            "ip {} failed: {}",
            args.join(" "),
            stderr.trim()
        ))
        .into());
    }

    Ok(())
}

/// Configure DNS by writing /etc/resolv.conf.
fn configure_dns(servers: &[String]) -> Result<()> {
    let mut content = String::new();
    for server in servers {
        content.push_str(&format!("nameserver {}\n", server));
    }

    // Atomic write
    let tmp_path = "/etc/resolv.conf.tmp";
    fs::write(tmp_path, &content).context("failed to write /etc/resolv.conf.tmp")?;
    fs::rename(tmp_path, "/etc/resolv.conf").context("failed to rename resolv.conf")?;

    Ok(())
}

/// Set the system hostname.
fn set_hostname(hostname: &str) -> Result<()> {
    // Use sethostname syscall via nix
    nix::unistd::sethostname(hostname).map_err(|e| {
        InitError::NetConfigFailed(format!("sethostname failed: {}", e))
    })?;

    // Also write /etc/hostname for persistence
    let _ = fs::write("/etc/hostname", format!("{}\n", hostname));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv6_parsing() {
        let addr: Ipv6Addr = "fd00::1234".parse().unwrap();
        // Verify it parsed correctly by checking segments
        assert_eq!(addr.segments()[0], 0xfd00);
        assert_eq!(addr.segments()[7], 0x1234);
    }

    #[test]
    fn test_dns_content() {
        let servers = vec!["fd00::53".to_string(), "8.8.8.8".to_string()];
        let mut content = String::new();
        for server in &servers {
            content.push_str(&format!("nameserver {}\n", server));
        }
        assert!(content.contains("nameserver fd00::53"));
        assert!(content.contains("nameserver 8.8.8.8"));
    }
}
