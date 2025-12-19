//! SNI (Server Name Indication) extraction from TLS ClientHello.
//!
//! This module parses the initial bytes of a TLS connection to extract
//! the SNI hostname for routing decisions. Per spec:
//! - sniff_timeout_ms: 200ms default
//! - max_sniff_bytes: 8192 bytes default
//!
//! Reference: docs/specs/networking/ingress-l4.md

use std::io;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;
use tracing::{debug, warn};

/// Default timeout for SNI sniffing (200ms per spec).
pub const DEFAULT_SNIFF_TIMEOUT: Duration = Duration::from_millis(200);

/// Default maximum bytes to read when sniffing for SNI (8KB per spec).
pub const DEFAULT_MAX_SNIFF_BYTES: usize = 8192;

/// Result of SNI inspection.
#[derive(Debug, Clone)]
pub enum SniResult {
    /// Successfully extracted SNI hostname (normalized to lowercase).
    Found(String),
    /// TLS ClientHello present but no SNI extension.
    NoSni,
    /// Data is not a TLS ClientHello (first bytes don't match).
    NotTls,
    /// Timeout while waiting for enough data.
    Timeout,
    /// I/O error during read.
    IoError(String),
    /// ClientHello is malformed or incomplete within bounds.
    Malformed,
}

/// Configuration for SNI inspection.
#[derive(Debug, Clone)]
pub struct SniConfig {
    /// Maximum time to wait for SNI data.
    pub timeout: Duration,
    /// Maximum bytes to read.
    pub max_bytes: usize,
}

impl Default for SniConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_SNIFF_TIMEOUT,
            max_bytes: DEFAULT_MAX_SNIFF_BYTES,
        }
    }
}

/// SNI inspector for TLS ClientHello parsing.
pub struct SniInspector {
    config: SniConfig,
}

impl SniInspector {
    /// Create a new SNI inspector with default configuration.
    pub fn new() -> Self {
        Self {
            config: SniConfig::default(),
        }
    }

    /// Create a new SNI inspector with custom configuration.
    pub fn with_config(config: SniConfig) -> Self {
        Self { config }
    }

    /// Inspect a stream for SNI, reading into the provided buffer.
    ///
    /// Returns the SNI result and the number of bytes read into the buffer.
    /// The caller must forward these buffered bytes to the backend.
    pub async fn inspect<R: AsyncRead + Unpin>(
        &self,
        stream: &mut R,
        buffer: &mut Vec<u8>,
    ) -> (SniResult, usize) {
        buffer.clear();
        buffer.resize(self.config.max_bytes, 0);

        let read_result = timeout(self.config.timeout, self.read_client_hello(stream, buffer)).await;

        match read_result {
            Ok(Ok(bytes_read)) => {
                buffer.truncate(bytes_read);
                let result = parse_sni(&buffer[..bytes_read]);
                (result, bytes_read)
            }
            Ok(Err(e)) => {
                buffer.clear();
                (SniResult::IoError(e.to_string()), 0)
            }
            Err(_) => {
                // Timeout - we may have partial data
                warn!("SNI sniff timeout");
                (SniResult::Timeout, 0)
            }
        }
    }

    /// Read enough of the ClientHello to extract SNI.
    async fn read_client_hello<R: AsyncRead + Unpin>(
        &self,
        stream: &mut R,
        buffer: &mut [u8],
    ) -> io::Result<usize> {
        let mut total_read = 0;

        // Read TLS record header (5 bytes minimum)
        while total_read < 5 {
            let n = stream.read(&mut buffer[total_read..]).await?;
            if n == 0 {
                return Ok(total_read);
            }
            total_read += n;
        }

        // Check if this looks like a TLS ClientHello
        // Record type 0x16 = Handshake
        if buffer[0] != 0x16 {
            return Ok(total_read);
        }

        // TLS version (we accept 0x0301 through 0x0303)
        let version = u16::from_be_bytes([buffer[1], buffer[2]]);
        if !(0x0301..=0x0303).contains(&version) && version != 0x0300 {
            debug!(version = version, "Unexpected TLS version");
            // Still return what we have - might be valid
        }

        // Record length
        let record_len = u16::from_be_bytes([buffer[3], buffer[4]]) as usize;
        let target_len = (5 + record_len).min(self.config.max_bytes);

        // Read enough to parse the ClientHello
        while total_read < target_len {
            let n = stream.read(&mut buffer[total_read..target_len]).await?;
            if n == 0 {
                break;
            }
            total_read += n;
        }

        Ok(total_read)
    }
}

impl Default for SniInspector {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse SNI from a TLS ClientHello buffer.
///
/// TLS Record structure:
/// - byte 0: record type (0x16 = Handshake)
/// - bytes 1-2: version
/// - bytes 3-4: record length
/// - bytes 5+: handshake message
///
/// Handshake ClientHello:
/// - byte 0: handshake type (0x01 = ClientHello)
/// - bytes 1-3: length
/// - bytes 4-5: client version
/// - bytes 6-37: random (32 bytes)
/// - byte 38: session ID length
/// - variable: session ID
/// - 2 bytes: cipher suites length
/// - variable: cipher suites
/// - 1 byte: compression methods length
/// - variable: compression methods
/// - 2 bytes: extensions length
/// - variable: extensions
fn parse_sni(data: &[u8]) -> SniResult {
    // Minimum: 5 (record header) + 1 (handshake type) + 3 (length) = 9 bytes
    if data.len() < 9 {
        return SniResult::Malformed;
    }

    // Check record type
    if data[0] != 0x16 {
        return SniResult::NotTls;
    }

    // Skip record header (5 bytes)
    let handshake = &data[5..];

    // Check handshake type (0x01 = ClientHello)
    if handshake.is_empty() || handshake[0] != 0x01 {
        return SniResult::NotTls;
    }

    // Parse handshake length
    if handshake.len() < 4 {
        return SniResult::Malformed;
    }
    let handshake_len =
        ((handshake[1] as usize) << 16) | ((handshake[2] as usize) << 8) | (handshake[3] as usize);

    // Sanity check
    if handshake.len() < 4 + handshake_len.min(handshake.len() - 4) {
        // We may have incomplete data, try to parse what we have
    }

    let client_hello = &handshake[4..];
    if client_hello.len() < 34 {
        return SniResult::Malformed;
    }

    // Skip version (2) + random (32) = 34 bytes
    let mut pos = 34;

    // Session ID
    if pos >= client_hello.len() {
        return SniResult::Malformed;
    }
    let session_id_len = client_hello[pos] as usize;
    pos += 1 + session_id_len;

    // Cipher suites
    if pos + 2 > client_hello.len() {
        return SniResult::Malformed;
    }
    let cipher_suites_len =
        u16::from_be_bytes([client_hello[pos], client_hello[pos + 1]]) as usize;
    pos += 2 + cipher_suites_len;

    // Compression methods
    if pos >= client_hello.len() {
        return SniResult::Malformed;
    }
    let compression_len = client_hello[pos] as usize;
    pos += 1 + compression_len;

    // Extensions
    if pos + 2 > client_hello.len() {
        // No extensions
        return SniResult::NoSni;
    }
    let extensions_len = u16::from_be_bytes([client_hello[pos], client_hello[pos + 1]]) as usize;
    pos += 2;

    let extensions_end = (pos + extensions_len).min(client_hello.len());

    // Parse extensions to find SNI (type 0x0000)
    while pos + 4 <= extensions_end {
        let ext_type = u16::from_be_bytes([client_hello[pos], client_hello[pos + 1]]);
        let ext_len = u16::from_be_bytes([client_hello[pos + 2], client_hello[pos + 3]]) as usize;
        pos += 4;

        if ext_type == 0x0000 {
            // SNI extension
            return parse_sni_extension(&client_hello[pos..(pos + ext_len).min(client_hello.len())]);
        }

        pos += ext_len;
    }

    SniResult::NoSni
}

/// Parse the SNI extension value.
///
/// SNI extension structure:
/// - 2 bytes: list length
/// - for each entry:
///   - 1 byte: name type (0 = hostname)
///   - 2 bytes: name length
///   - variable: name
fn parse_sni_extension(data: &[u8]) -> SniResult {
    if data.len() < 2 {
        return SniResult::Malformed;
    }

    let list_len = u16::from_be_bytes([data[0], data[1]]) as usize;
    if data.len() < 2 + list_len {
        return SniResult::Malformed;
    }

    let mut pos = 2;
    while pos + 3 <= 2 + list_len {
        let name_type = data[pos];
        let name_len = u16::from_be_bytes([data[pos + 1], data[pos + 2]]) as usize;
        pos += 3;

        if name_type == 0 {
            // Host name
            if pos + name_len > data.len() {
                return SniResult::Malformed;
            }

            match std::str::from_utf8(&data[pos..pos + name_len]) {
                Ok(hostname) => {
                    // Normalize: lowercase, trim trailing dot
                    let normalized = hostname.to_lowercase().trim_end_matches('.').to_string();
                    return SniResult::Found(normalized);
                }
                Err(_) => {
                    return SniResult::Malformed;
                }
            }
        }

        pos += name_len;
    }

    SniResult::NoSni
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal TLS 1.2 ClientHello with SNI "example.com"
    const EXAMPLE_CLIENT_HELLO: &[u8] = &[
        // TLS record header
        0x16, // Handshake
        0x03, 0x01, // TLS 1.0 (for compatibility)
        0x00, 0x5f, // Record length: 95 bytes
        // Handshake header
        0x01, // ClientHello
        0x00, 0x00, 0x5b, // Length: 91 bytes
        // Client version
        0x03, 0x03, // TLS 1.2
        // Random (32 bytes)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, // Session ID length
        0x00, // Cipher suites length
        0x00, 0x02, // Cipher suite
        0x00, 0x2f, // TLS_RSA_WITH_AES_128_CBC_SHA
        // Compression methods
        0x01, 0x00, // null compression
        // Extensions length
        0x00, 0x28, // 40 bytes
        // SNI extension
        0x00, 0x00, // type: SNI
        0x00, 0x10, // length: 16 bytes
        0x00, 0x0e, // list length: 14 bytes
        0x00, // name type: hostname
        0x00, 0x0b, // name length: 11 bytes
        b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c', b'o', b'm',
        // Padding to match declared length
        0x00, 0x15, // padding extension
        0x00, 0x10, // length
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ];

    #[test]
    fn test_parse_sni_found() {
        let result = parse_sni(EXAMPLE_CLIENT_HELLO);
        match result {
            SniResult::Found(hostname) => assert_eq!(hostname, "example.com"),
            other => panic!("Expected Found, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_not_tls() {
        let http_request = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let result = parse_sni(http_request);
        assert!(matches!(result, SniResult::NotTls));
    }

    #[test]
    fn test_parse_too_short() {
        let result = parse_sni(&[0x16, 0x03, 0x01]);
        assert!(matches!(result, SniResult::Malformed));
    }

    #[test]
    fn test_normalize_trailing_dot() {
        // Test the normalize function directly
        let hostname = "EXAMPLE.COM.";
        let normalized = hostname.to_lowercase().trim_end_matches('.').to_string();
        assert_eq!(normalized, "example.com");
    }
}
