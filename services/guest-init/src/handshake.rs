//! vsock config handshake with host agent.
//!
//! Protocol:
//! 1. Guest connects to host on vsock port 5161
//! 2. Guest sends hello message
//! 3. Host sends config message
//! 4. Guest sends ack message
//! 5. Guest sends status updates as boot progresses

use std::io::{BufRead, BufReader, Write};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use uuid::Uuid;
use vsock::{VsockAddr, VsockStream};

use crate::config::{AckMessage, ConfigMessage, GuestConfig, HelloMessage, StatusMessage};
use crate::error::InitError;
use crate::{PROTOCOL_VERSION, VERSION};

/// Host CID for vsock (always 2 per virtio-vsock spec).
const HOST_CID: u32 = 2;

/// Connection timeout in seconds.
#[allow(dead_code)] // Reserved for future timeout implementation
const CONNECT_TIMEOUT_SECS: u64 = 5;

/// Global connection for status reporting.
static VSOCK_CONN: OnceLock<std::sync::Mutex<VsockStream>> = OnceLock::new();

/// Read expected instance ID from kernel cmdline.
fn read_instance_id_from_cmdline() -> Option<String> {
    let cmdline = std::fs::read_to_string("/proc/cmdline").ok()?;
    for part in cmdline.split_whitespace() {
        if let Some(id) = part.strip_prefix("platform.instance_id=") {
            return Some(id.to_string());
        }
    }
    None
}

/// Generate a unique boot ID.
fn generate_boot_id() -> String {
    Uuid::new_v4().to_string()
}

/// Perform the config handshake with the host agent.
pub async fn perform_handshake(port: u32) -> Result<GuestConfig> {
    // Read instance ID from kernel cmdline
    let instance_id = read_instance_id_from_cmdline().unwrap_or_else(|| "unknown".to_string());
    let boot_id = generate_boot_id();

    info!(
        instance_id = %instance_id,
        boot_id = %boot_id,
        host_cid = HOST_CID,
        port = port,
        "connecting to host agent"
    );

    // Connect to host agent
    let addr = VsockAddr::new(HOST_CID, port);
    let mut stream = VsockStream::connect(&addr)
        .map_err(|e| InitError::HandshakeFailed(format!("failed to connect to host: {}", e)))?;

    info!("connected to host agent");

    // Send hello
    let hello = HelloMessage::new(&instance_id, &boot_id, VERSION, PROTOCOL_VERSION);
    send_message(&mut stream, &hello)?;
    debug!("sent hello");

    // Receive config
    let config = receive_config(&mut stream)?;
    info!(
        config_version = %config.config_version,
        generation = config.generation,
        "received config"
    );

    // Send ack
    let ack = AckMessage::new(&config.config_version, config.generation);
    send_message(&mut stream, &ack)?;
    debug!("sent ack");

    // Store connection for status reporting
    let _ = VSOCK_CONN.set(std::sync::Mutex::new(stream));

    Ok(config)
}

/// Send a JSON message over vsock (NDJSON format).
fn send_message<T: serde::Serialize>(stream: &mut VsockStream, msg: &T) -> Result<()> {
    let json = serde_json::to_string(msg).context("failed to serialize message")?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

/// Receive config message from host.
fn receive_config(stream: &mut VsockStream) -> Result<GuestConfig> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    reader
        .read_line(&mut line)
        .context("failed to read config from host")?;

    if line.is_empty() {
        return Err(InitError::HandshakeFailed("host closed connection".to_string()).into());
    }

    let msg: ConfigMessage = serde_json::from_str(&line)
        .map_err(|e| InitError::ConfigParseFailed(format!("invalid config JSON: {}", e)))?;

    if msg.msg_type != "config" {
        return Err(InitError::ConfigParseFailed(format!(
            "expected 'config' message, got '{}'",
            msg.msg_type
        ))
        .into());
    }

    Ok(msg.config)
}

/// Report status to host agent.
pub async fn report_status(state: &str) -> Result<()> {
    let Some(conn) = VSOCK_CONN.get() else {
        warn!("no vsock connection for status report");
        return Ok(());
    };

    let status = StatusMessage::new(state);

    if let Ok(mut stream) = conn.lock() {
        if let Err(e) = send_message(&mut stream, &status) {
            warn!(error = %e, state = state, "failed to send status");
        } else {
            debug!(state = state, "status reported");
        }
    }

    Ok(())
}

/// Report failure to host agent.
#[allow(dead_code)] // Called from error handling paths
pub async fn report_failure(reason: &str, detail: &str) -> Result<()> {
    let Some(conn) = VSOCK_CONN.get() else {
        warn!("no vsock connection for failure report");
        return Ok(());
    };

    let status = StatusMessage::with_failure("failed", reason, detail);

    if let Ok(mut stream) = conn.lock() {
        if let Err(e) = send_message(&mut stream, &status) {
            warn!(error = %e, reason = reason, "failed to send failure status");
        } else {
            info!(reason = reason, "failure reported to host");
        }
    }

    Ok(())
}

/// Report workload exit to host agent.
pub async fn report_exit(exit_code: i32) -> Result<()> {
    let Some(conn) = VSOCK_CONN.get() else {
        warn!("no vsock connection for exit report");
        return Ok(());
    };

    let status = StatusMessage::with_exit(exit_code);

    if let Ok(mut stream) = conn.lock() {
        if let Err(e) = send_message(&mut stream, &status) {
            warn!(error = %e, exit_code = exit_code, "failed to send exit status");
        } else {
            info!(exit_code = exit_code, "exit reported to host");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_boot_id() {
        let id1 = generate_boot_id();
        let id2 = generate_boot_id();
        assert_ne!(id1, id2);
        // Should be valid UUID
        assert!(Uuid::parse_str(&id1).is_ok());
    }
}
