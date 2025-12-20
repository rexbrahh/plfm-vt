//! PROXY Protocol v2 header generation.
//!
//! This module generates PROXY protocol v2 headers for prepending to
//! upstream connections when enabled per-route.
//!
//! Wire format (from HAProxy PROXY protocol spec):
//! - 12 bytes signature
//! - 1 byte version and command
//! - 1 byte address family and transport protocol
//! - 2 bytes address length
//! - variable: addresses and ports
//!
//! Reference: docs/specs/networking/proxy-protocol-v2.md

use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// PROXY protocol v2 signature (12 bytes).
const PROXY_V2_SIGNATURE: [u8; 12] = [
    0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
];

/// Version 2 with PROXY command (high nibble = version, low nibble = command).
const VERSION_COMMAND_PROXY: u8 = 0x21;

/// Version 2 with LOCAL command (for health checks, etc.).
#[allow(dead_code)]
const VERSION_COMMAND_LOCAL: u8 = 0x20;

/// Address family: AF_INET (IPv4).
const AF_INET: u8 = 0x10;

/// Address family: AF_INET6 (IPv6).
const AF_INET6: u8 = 0x20;

/// Transport protocol: STREAM (TCP).
const TRANSPORT_STREAM: u8 = 0x01;

/// Length of IPv4 address block (4 + 4 + 2 + 2 = 12 bytes).
const IPV4_ADDR_LEN: u16 = 12;

/// Length of IPv6 address block (16 + 16 + 2 + 2 = 36 bytes).
const IPV6_ADDR_LEN: u16 = 36;

/// PROXY protocol v2 header generator.
#[derive(Debug, Clone)]
pub struct ProxyProtocolV2 {
    /// Source (client) address.
    pub src_addr: SocketAddr,
    /// Destination (edge listener) address.
    pub dst_addr: SocketAddr,
}

impl ProxyProtocolV2 {
    /// Create a new PROXY v2 header for the given connection.
    ///
    /// # Arguments
    /// * `src_addr` - Original client source address and port
    /// * `dst_addr` - Destination address as observed at edge listener
    pub fn new(src_addr: SocketAddr, dst_addr: SocketAddr) -> Self {
        Self { src_addr, dst_addr }
    }

    /// Generate the PROXY v2 header bytes.
    ///
    /// Returns the header as a Vec<u8> that should be written to the
    /// upstream connection before any application data.
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        // Determine address family based on source address
        // Both addresses should be the same family; if not, we'll use the source family
        match (self.src_addr.ip(), self.dst_addr.ip()) {
            (IpAddr::V4(src_ip), IpAddr::V4(dst_ip)) => self.encode_v4(src_ip, dst_ip),
            (IpAddr::V6(src_ip), IpAddr::V6(dst_ip)) => self.encode_v6(src_ip, dst_ip),
            (IpAddr::V4(src_ip), IpAddr::V6(dst_ip)) => {
                // Mixed: map v4 to v6 for destination
                let dst_v4 = extract_v4_from_v6(dst_ip).unwrap_or(Ipv4Addr::UNSPECIFIED);
                self.encode_v4(src_ip, dst_v4)
            }
            (IpAddr::V6(src_ip), IpAddr::V4(dst_ip)) => {
                // Mixed: map v4 to v6 for source
                let src_v4 = extract_v4_from_v6(src_ip).unwrap_or(Ipv4Addr::UNSPECIFIED);
                self.encode_v4(src_v4, dst_ip)
            }
        }
    }

    /// Encode IPv4 PROXY v2 header.
    fn encode_v4(&self, src_ip: Ipv4Addr, dst_ip: Ipv4Addr) -> io::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(16 + 12); // header + addresses

        // Signature (12 bytes)
        buf.write_all(&PROXY_V2_SIGNATURE)?;

        // Version and command
        buf.push(VERSION_COMMAND_PROXY);

        // Address family and protocol: AF_INET + STREAM
        buf.push(AF_INET | TRANSPORT_STREAM);

        // Address length
        buf.write_all(&IPV4_ADDR_LEN.to_be_bytes())?;

        // Source address (4 bytes)
        buf.write_all(&src_ip.octets())?;

        // Destination address (4 bytes)
        buf.write_all(&dst_ip.octets())?;

        // Source port (2 bytes)
        buf.write_all(&self.src_addr.port().to_be_bytes())?;

        // Destination port (2 bytes)
        buf.write_all(&self.dst_addr.port().to_be_bytes())?;

        Ok(buf)
    }

    /// Encode IPv6 PROXY v2 header.
    fn encode_v6(&self, src_ip: Ipv6Addr, dst_ip: Ipv6Addr) -> io::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(16 + 36); // header + addresses

        // Signature (12 bytes)
        buf.write_all(&PROXY_V2_SIGNATURE)?;

        // Version and command
        buf.push(VERSION_COMMAND_PROXY);

        // Address family and protocol: AF_INET6 + STREAM
        buf.push(AF_INET6 | TRANSPORT_STREAM);

        // Address length
        buf.write_all(&IPV6_ADDR_LEN.to_be_bytes())?;

        // Source address (16 bytes)
        buf.write_all(&src_ip.octets())?;

        // Destination address (16 bytes)
        buf.write_all(&dst_ip.octets())?;

        // Source port (2 bytes)
        buf.write_all(&self.src_addr.port().to_be_bytes())?;

        // Destination port (2 bytes)
        buf.write_all(&self.dst_addr.port().to_be_bytes())?;

        Ok(buf)
    }

    /// Get the expected header size for a given address family.
    pub fn header_size(is_ipv6: bool) -> usize {
        if is_ipv6 {
            16 + 36 // signature + header + IPv6 addresses
        } else {
            16 + 12 // signature + header + IPv4 addresses
        }
    }
}

/// Extract IPv4 from an IPv6 address if it's a mapped or compatible address.
fn extract_v4_from_v6(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    // Check for IPv4-mapped (::ffff:a.b.c.d) or IPv4-compatible (::a.b.c.d)
    let octets = addr.octets();

    // IPv4-mapped: first 10 bytes zero, bytes 10-11 are 0xff
    if octets[..10].iter().all(|&b| b == 0) && octets[10] == 0xff && octets[11] == 0xff {
        return Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }

    // IPv4-compatible: first 12 bytes zero (deprecated but still handled)
    if octets[..12].iter().all(|&b| b == 0) {
        return Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }

    None
}

/// Parse a PROXY v2 header from a buffer.
///
/// Returns the parsed header and the number of bytes consumed,
/// or None if the buffer doesn't contain a valid header.
#[allow(dead_code)]
pub fn parse_proxy_v2(data: &[u8]) -> Option<(ProxyProtocolV2, usize)> {
    // Minimum header size: 16 bytes (signature + version/command + family + length)
    if data.len() < 16 {
        return None;
    }

    // Check signature
    if data[..12] != PROXY_V2_SIGNATURE {
        return None;
    }

    let version_command = data[12];
    let family_protocol = data[13];
    let addr_len = u16::from_be_bytes([data[14], data[15]]) as usize;

    // Verify we have enough data
    if data.len() < 16 + addr_len {
        return None;
    }

    // Only handle PROXY command
    if version_command != VERSION_COMMAND_PROXY {
        return None;
    }

    let (src_addr, dst_addr) = match family_protocol {
        x if x == (AF_INET | TRANSPORT_STREAM) => {
            // IPv4
            if addr_len < 12 {
                return None;
            }
            let src_ip = Ipv4Addr::new(data[16], data[17], data[18], data[19]);
            let dst_ip = Ipv4Addr::new(data[20], data[21], data[22], data[23]);
            let src_port = u16::from_be_bytes([data[24], data[25]]);
            let dst_port = u16::from_be_bytes([data[26], data[27]]);
            (
                SocketAddr::new(IpAddr::V4(src_ip), src_port),
                SocketAddr::new(IpAddr::V4(dst_ip), dst_port),
            )
        }
        x if x == (AF_INET6 | TRANSPORT_STREAM) => {
            // IPv6
            if addr_len < 36 {
                return None;
            }
            let src_octets: [u8; 16] = data[16..32].try_into().ok()?;
            let dst_octets: [u8; 16] = data[32..48].try_into().ok()?;
            let src_ip = Ipv6Addr::from(src_octets);
            let dst_ip = Ipv6Addr::from(dst_octets);
            let src_port = u16::from_be_bytes([data[48], data[49]]);
            let dst_port = u16::from_be_bytes([data[50], data[51]]);
            (
                SocketAddr::new(IpAddr::V6(src_ip), src_port),
                SocketAddr::new(IpAddr::V6(dst_ip), dst_port),
            )
        }
        _ => return None,
    };

    Some((ProxyProtocolV2::new(src_addr, dst_addr), 16 + addr_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_ipv4() {
        let header = ProxyProtocolV2::new(
            "192.168.1.1:12345".parse().unwrap(),
            "10.0.0.1:443".parse().unwrap(),
        );

        let encoded = header.encode().unwrap();

        // Should be 28 bytes: 12 signature + 4 header + 12 addresses
        assert_eq!(encoded.len(), 28);

        // Check signature
        assert_eq!(&encoded[..12], &PROXY_V2_SIGNATURE);

        // Check version/command
        assert_eq!(encoded[12], VERSION_COMMAND_PROXY);

        // Check family/protocol
        assert_eq!(encoded[13], AF_INET | TRANSPORT_STREAM);

        // Check length
        assert_eq!(u16::from_be_bytes([encoded[14], encoded[15]]), 12);

        // Parse it back
        let (parsed, consumed) = parse_proxy_v2(&encoded).unwrap();
        assert_eq!(consumed, 28);
        assert_eq!(parsed.src_addr, header.src_addr);
        assert_eq!(parsed.dst_addr, header.dst_addr);
    }

    #[test]
    fn test_encode_ipv6() {
        let header = ProxyProtocolV2::new(
            "[2001:db8::1]:12345".parse().unwrap(),
            "[2001:db8::2]:443".parse().unwrap(),
        );

        let encoded = header.encode().unwrap();

        // Should be 52 bytes: 12 signature + 4 header + 36 addresses
        assert_eq!(encoded.len(), 52);

        // Check signature
        assert_eq!(&encoded[..12], &PROXY_V2_SIGNATURE);

        // Check family/protocol
        assert_eq!(encoded[13], AF_INET6 | TRANSPORT_STREAM);

        // Check length
        assert_eq!(u16::from_be_bytes([encoded[14], encoded[15]]), 36);

        // Parse it back
        let (parsed, consumed) = parse_proxy_v2(&encoded).unwrap();
        assert_eq!(consumed, 52);
        assert_eq!(parsed.src_addr, header.src_addr);
        assert_eq!(parsed.dst_addr, header.dst_addr);
    }

    #[test]
    fn test_header_size() {
        assert_eq!(ProxyProtocolV2::header_size(false), 28);
        assert_eq!(ProxyProtocolV2::header_size(true), 52);
    }

    #[test]
    fn test_extract_v4_from_v6() {
        // IPv4-mapped
        let mapped: Ipv6Addr = "::ffff:192.168.1.1".parse().unwrap();
        assert_eq!(
            extract_v4_from_v6(mapped),
            Some(Ipv4Addr::new(192, 168, 1, 1))
        );

        // Regular IPv6
        let regular: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert_eq!(extract_v4_from_v6(regular), None);
    }

    #[test]
    fn test_parse_invalid() {
        // Too short
        assert!(parse_proxy_v2(&[0; 10]).is_none());

        // Invalid signature
        let mut bad_sig = vec![0; 28];
        bad_sig[14] = 0;
        bad_sig[15] = 12;
        assert!(parse_proxy_v2(&bad_sig).is_none());
    }
}
