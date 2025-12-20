//! Networking utilities for the platform.
//!
//! This library provides helpers for:
//! - IPAM (IP Address Management) for IPv6 overlay addresses
//! - WireGuard peer configuration
//! - MTU and network interface configuration
//! - Guest networking setup

use std::net::Ipv6Addr;
use std::str::FromStr;

use thiserror::Error;

/// Networking errors.
#[derive(Debug, Error)]
pub enum NetworkError {
    /// Invalid IP address.
    #[error("invalid IP address: {0}")]
    InvalidAddress(String),

    /// Invalid CIDR prefix.
    #[error("invalid CIDR prefix: {0}")]
    InvalidPrefix(String),

    /// Address pool exhausted.
    #[error("address pool exhausted: {0}")]
    PoolExhausted(String),

    /// Invalid MTU value.
    #[error("invalid MTU: {value} (must be between {min} and {max})")]
    InvalidMtu { value: u16, min: u16, max: u16 },

    /// Invalid WireGuard key.
    #[error("invalid WireGuard key: {0}")]
    InvalidKey(String),

    /// Configuration error.
    #[error("configuration error: {0}")]
    Config(String),
}

// ============================================================================
// IPAM (IP Address Management)
// ============================================================================

/// IPv6 prefix for IPAM allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv6Prefix {
    /// Base address of the prefix.
    pub address: Ipv6Addr,

    /// Prefix length (e.g., 64 for /64).
    pub prefix_len: u8,
}

impl Ipv6Prefix {
    /// Create a new prefix.
    pub fn new(address: Ipv6Addr, prefix_len: u8) -> Result<Self, NetworkError> {
        if prefix_len > 128 {
            return Err(NetworkError::InvalidPrefix(format!(
                "prefix length {} exceeds 128",
                prefix_len
            )));
        }

        // Mask the address to the prefix
        let masked = mask_ipv6(address, prefix_len);

        Ok(Self {
            address: masked,
            prefix_len,
        })
    }

    /// Parse from CIDR notation (e.g., "2001:db8::/32").
    pub fn from_cidr(s: &str) -> Result<Self, NetworkError> {
        let Some((addr_str, prefix_str)) = s.split_once('/') else {
            return Err(NetworkError::InvalidPrefix(format!(
                "missing '/' in CIDR: {}",
                s
            )));
        };

        let address = Ipv6Addr::from_str(addr_str)
            .map_err(|_| NetworkError::InvalidAddress(addr_str.to_string()))?;

        let prefix_len = prefix_str
            .parse::<u8>()
            .map_err(|_| NetworkError::InvalidPrefix(prefix_str.to_string()))?;

        Self::new(address, prefix_len)
    }

    /// Check if an address is within this prefix.
    pub fn contains(&self, addr: Ipv6Addr) -> bool {
        let masked = mask_ipv6(addr, self.prefix_len);
        masked == self.address
    }

    /// Calculate the number of addresses in this prefix.
    pub fn size(&self) -> u128 {
        if self.prefix_len >= 128 {
            1
        } else {
            1u128 << (128 - self.prefix_len)
        }
    }
}

impl std::fmt::Display for Ipv6Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix_len)
    }
}

/// Mask an IPv6 address to a prefix length.
fn mask_ipv6(addr: Ipv6Addr, prefix_len: u8) -> Ipv6Addr {
    let bits = u128::from_be_bytes(addr.octets());
    let mask = if prefix_len == 0 {
        0
    } else if prefix_len >= 128 {
        u128::MAX
    } else {
        u128::MAX << (128 - prefix_len)
    };
    Ipv6Addr::from((bits & mask).to_be_bytes())
}

/// Sequential IPv6 address allocator.
#[derive(Debug)]
pub struct Ipv6Allocator {
    /// Prefix to allocate from.
    prefix: Ipv6Prefix,

    /// Next address offset to allocate.
    next_offset: u128,

    /// Maximum offset (exclusive).
    max_offset: u128,
}

impl Ipv6Allocator {
    /// Create a new allocator for a prefix.
    pub fn new(prefix: Ipv6Prefix) -> Self {
        let max_offset = prefix.size();
        Self {
            prefix,
            next_offset: 1, // Skip the network address (::0)
            max_offset,
        }
    }

    /// Allocate the next available address.
    pub fn allocate(&mut self) -> Result<Ipv6Addr, NetworkError> {
        if self.next_offset >= self.max_offset {
            return Err(NetworkError::PoolExhausted(self.prefix.to_string()));
        }

        let base = u128::from_be_bytes(self.prefix.address.octets());
        let addr = base + self.next_offset;
        self.next_offset += 1;

        Ok(Ipv6Addr::from(addr.to_be_bytes()))
    }

    /// Allocate a specific address (for recovery/import).
    ///
    /// Does not advance the internal counter.
    pub fn allocate_specific(&self, addr: Ipv6Addr) -> Result<Ipv6Addr, NetworkError> {
        if !self.prefix.contains(addr) {
            return Err(NetworkError::InvalidAddress(format!(
                "{} is not in prefix {}",
                addr, self.prefix
            )));
        }
        Ok(addr)
    }

    /// Get the prefix being allocated from.
    pub fn prefix(&self) -> &Ipv6Prefix {
        &self.prefix
    }

    /// Get remaining addresses.
    pub fn remaining(&self) -> u128 {
        self.max_offset.saturating_sub(self.next_offset)
    }
}

// ============================================================================
// WireGuard Configuration
// ============================================================================

/// Default WireGuard port.
pub const WIREGUARD_DEFAULT_PORT: u16 = 51820;

/// Default WireGuard MTU.
pub const WIREGUARD_DEFAULT_MTU: u16 = 1420;

/// Default persistent keepalive interval (seconds).
pub const WIREGUARD_DEFAULT_KEEPALIVE: u16 = 25;

/// WireGuard public key (base64-encoded, 32 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WgPublicKey(String);

impl WgPublicKey {
    /// Create from a base64-encoded string.
    pub fn from_base64(s: &str) -> Result<Self, NetworkError> {
        // Validate base64 format and length
        let decoded = base64_decode(s)
            .map_err(|_| NetworkError::InvalidKey(format!("invalid base64: {}", s)))?;

        if decoded.len() != 32 {
            return Err(NetworkError::InvalidKey(format!(
                "key must be 32 bytes, got {}",
                decoded.len()
            )));
        }

        Ok(Self(s.to_string()))
    }

    /// Get the base64-encoded key.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WgPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Simple base64 decoder (standard alphabet).
fn base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;

    for c in s.bytes() {
        let val = ALPHABET.iter().position(|&x| x == c).ok_or(())?;
        buf = (buf << 6) | (val as u32);
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}

/// WireGuard peer configuration.
#[derive(Debug, Clone)]
pub struct WgPeer {
    /// Peer's public key.
    pub public_key: WgPublicKey,

    /// Endpoint address and port (if known).
    pub endpoint: Option<String>,

    /// Allowed IPs for this peer.
    pub allowed_ips: Vec<String>,

    /// Persistent keepalive interval (seconds, 0 = disabled).
    pub persistent_keepalive: u16,
}

impl WgPeer {
    /// Create a new peer with minimal configuration.
    pub fn new(public_key: WgPublicKey, allowed_ips: Vec<String>) -> Self {
        Self {
            public_key,
            endpoint: None,
            allowed_ips,
            persistent_keepalive: WIREGUARD_DEFAULT_KEEPALIVE,
        }
    }

    /// Set the endpoint.
    pub fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = Some(endpoint);
        self
    }

    /// Set the persistent keepalive.
    pub fn with_keepalive(mut self, seconds: u16) -> Self {
        self.persistent_keepalive = seconds;
        self
    }
}

/// WireGuard interface configuration.
#[derive(Debug, Clone)]
pub struct WgInterface {
    /// Interface name (e.g., "wg0").
    pub name: String,

    /// Listen port.
    pub listen_port: u16,

    /// MTU.
    pub mtu: u16,

    /// Assigned addresses (CIDR notation).
    pub addresses: Vec<String>,

    /// Configured peers.
    pub peers: Vec<WgPeer>,
}

impl WgInterface {
    /// Create a new interface with defaults.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            listen_port: WIREGUARD_DEFAULT_PORT,
            mtu: WIREGUARD_DEFAULT_MTU,
            addresses: Vec::new(),
            peers: Vec::new(),
        }
    }

    /// Add an address.
    pub fn add_address(&mut self, address: &str) {
        self.addresses.push(address.to_string());
    }

    /// Add a peer.
    pub fn add_peer(&mut self, peer: WgPeer) {
        self.peers.push(peer);
    }

    /// Find a peer by public key.
    pub fn find_peer(&self, key: &WgPublicKey) -> Option<&WgPeer> {
        self.peers.iter().find(|p| &p.public_key == key)
    }

    /// Remove a peer by public key.
    pub fn remove_peer(&mut self, key: &WgPublicKey) -> Option<WgPeer> {
        let idx = self.peers.iter().position(|p| &p.public_key == key)?;
        Some(self.peers.remove(idx))
    }
}

// ============================================================================
// MTU Configuration
// ============================================================================

/// Minimum MTU for IPv6.
pub const MTU_MIN_IPV6: u16 = 1280;

/// Maximum MTU for jumbo frames.
pub const MTU_MAX_JUMBO: u16 = 9000;

/// Default MTU for Ethernet.
pub const MTU_DEFAULT_ETHERNET: u16 = 1500;

/// Validate an MTU value.
pub fn validate_mtu(mtu: u16) -> Result<u16, NetworkError> {
    if !(MTU_MIN_IPV6..=MTU_MAX_JUMBO).contains(&mtu) {
        return Err(NetworkError::InvalidMtu {
            value: mtu,
            min: MTU_MIN_IPV6,
            max: MTU_MAX_JUMBO,
        });
    }
    Ok(mtu)
}

/// Calculate MTU for encapsulated traffic.
///
/// Returns the inner MTU given an outer MTU and overhead.
pub fn calculate_inner_mtu(outer_mtu: u16, overhead: u16) -> Result<u16, NetworkError> {
    let inner = outer_mtu.saturating_sub(overhead);
    validate_mtu(inner)
}

/// WireGuard encapsulation overhead.
pub const WIREGUARD_OVERHEAD: u16 = 80; // WG header + IPv6

// ============================================================================
// Guest Networking
// ============================================================================

/// Guest network configuration.
#[derive(Debug, Clone)]
pub struct GuestNetworkConfig {
    /// IPv6 address with prefix (e.g., "2001:db8::1/128").
    pub ipv6_address: String,

    /// Default gateway (link-local or routed).
    pub gateway: String,

    /// MTU for the guest interface.
    pub mtu: u16,

    /// DNS resolvers.
    pub dns_servers: Vec<String>,
}

impl GuestNetworkConfig {
    /// Create a new guest network configuration.
    pub fn new(ipv6_address: &str, gateway: &str, mtu: u16) -> Result<Self, NetworkError> {
        validate_mtu(mtu)?;

        Ok(Self {
            ipv6_address: ipv6_address.to_string(),
            gateway: gateway.to_string(),
            mtu,
            dns_servers: Vec::new(),
        })
    }

    /// Add a DNS server.
    pub fn add_dns(&mut self, server: &str) {
        self.dns_servers.push(server.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv6_prefix() {
        let prefix = Ipv6Prefix::from_cidr("2001:db8::/32").unwrap();
        assert_eq!(prefix.prefix_len, 32);

        let addr1: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let addr2: Ipv6Addr = "2001:db9::1".parse().unwrap();

        assert!(prefix.contains(addr1));
        assert!(!prefix.contains(addr2));
    }

    #[test]
    fn test_ipv6_allocator() {
        let prefix = Ipv6Prefix::from_cidr("2001:db8:1::/120").unwrap();
        let mut allocator = Ipv6Allocator::new(prefix);

        let addr1 = allocator.allocate().unwrap();
        let addr2 = allocator.allocate().unwrap();

        assert_ne!(addr1, addr2);
        assert!(addr1.to_string().starts_with("2001:db8:1::"));
    }

    #[test]
    fn test_wg_public_key() {
        // Valid 32-byte key in base64
        let valid = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        assert!(WgPublicKey::from_base64(valid).is_ok());

        // Invalid length
        let short = "AAAA";
        assert!(WgPublicKey::from_base64(short).is_err());
    }

    #[test]
    fn test_mtu_validation() {
        assert!(validate_mtu(1280).is_ok());
        assert!(validate_mtu(1500).is_ok());
        assert!(validate_mtu(9000).is_ok());

        assert!(validate_mtu(1279).is_err());
        assert!(validate_mtu(9001).is_err());
    }

    #[test]
    fn test_inner_mtu() {
        let inner = calculate_inner_mtu(1500, WIREGUARD_OVERHEAD).unwrap();
        assert_eq!(inner, 1420);
    }
}
