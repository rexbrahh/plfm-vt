//! Exec service for `plfm exec`.
//!
//! Listens on vsock port 5162 for exec requests from the host agent
//! and spawns processes with optional PTY support.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use nix::pty::{openpty, OpenptyResult, Winsize};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};
use vsock::{VsockAddr, VsockListener, VsockStream};

/// Guest CID for listening (always 3 in Firecracker).
const GUEST_CID: u32 = 3;

/// Frame types for exec stream protocol.
#[allow(dead_code)] // Protocol constants for stream framing
mod frame_type {
    pub const STDIN: u8 = 0x01;
    pub const STDOUT: u8 = 0x02;
    pub const STDERR: u8 = 0x03;
    pub const CONTROL: u8 = 0x10;
    pub const EXIT: u8 = 0x11;
}

/// Exec request from host agent.
#[derive(Debug, Deserialize)]
struct ExecRequest {
    /// Command and arguments.
    command: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    env: HashMap<String, String>,
    /// Allocate PTY.
    #[serde(default)]
    tty: bool,
    /// Terminal columns.
    #[serde(default = "default_cols")]
    cols: u16,
    /// Terminal rows.
    #[serde(default = "default_rows")]
    rows: u16,
    /// Connect stdin.
    #[serde(default = "default_true")]
    stdin: bool,
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

fn default_true() -> bool {
    true
}

/// Control message.
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Used for PTY resize handling
struct ControlMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    #[serde(default)]
    name: Option<String>,
}

/// Exit status message.
#[derive(Debug, Serialize)]
struct ExitMessage {
    #[serde(rename = "type")]
    msg_type: String,
    exit_code: i32,
    reason: String,
}

impl ExitMessage {
    fn new(exit_code: i32, reason: &str) -> Self {
        Self {
            msg_type: "exit".to_string(),
            exit_code,
            reason: reason.to_string(),
        }
    }
}

/// Run the exec service on the specified vsock port.
pub async fn run_exec_service(port: u32) -> Result<()> {
    let addr = VsockAddr::new(GUEST_CID, port);

    // Note: vsock crate uses blocking I/O, so we spawn blocking tasks
    let listener = VsockListener::bind(&addr)
        .map_err(|e| anyhow::anyhow!("failed to bind exec service on port {}: {}", port, e))?;

    info!(port = port, "exec service listening");

    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                info!(peer_cid = peer.cid(), "exec connection accepted");

                // Handle connection in a blocking task
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = handle_exec_connection(stream) {
                        error!(error = %e, "exec session failed");
                    }
                });
            }
            Err(e) => {
                warn!(error = %e, "accept failed");
            }
        }
    }
}

/// Handle a single exec connection.
fn handle_exec_connection(mut stream: VsockStream) -> Result<()> {
    // Read the exec request (first line is JSON)
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf)?;

    if n == 0 {
        return Ok(());
    }

    // Find newline delimiter
    let request_end = buf[..n].iter().position(|&b| b == b'\n').unwrap_or(n);
    let request_json = std::str::from_utf8(&buf[..request_end])?;

    let request: ExecRequest =
        serde_json::from_str(request_json).context("invalid exec request JSON")?;

    debug!(
        command = ?request.command,
        tty = request.tty,
        "exec request received"
    );

    if request.command.is_empty() {
        send_exit(&mut stream, 1, "empty command")?;
        return Ok(());
    }

    // Execute the command
    let exit_code = if request.tty {
        execute_with_pty(&mut stream, &request)?
    } else {
        execute_with_pipes(&mut stream, &request)?
    };

    send_exit(&mut stream, exit_code, "exited")?;

    Ok(())
}

/// Execute command with PTY.
fn execute_with_pty(stream: &mut VsockStream, request: &ExecRequest) -> Result<i32> {
    let winsize = Winsize {
        ws_row: request.rows,
        ws_col: request.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // Open PTY
    let OpenptyResult { master, slave } = openpty(Some(&winsize), None)?;

    // Get the raw FDs before we move slave into the closure
    let slave_fd = slave.as_raw_fd();
    let master_fd = master.as_raw_fd();

    // Fork and exec
    let program = &request.command[0];
    let args: Vec<&str> = request.command.iter().map(|s| s.as_str()).collect();

    let child = unsafe {
        Command::new(program)
            .args(&args[1..])
            .envs(&request.env)
            .stdin(Stdio::from_raw_fd(slave_fd))
            .stdout(Stdio::from_raw_fd(slave_fd))
            .stderr(Stdio::from_raw_fd(slave_fd))
            .pre_exec(move || {
                // Create new session and set controlling terminal
                libc::setsid();
                libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0);
                Ok(())
            })
            .spawn()
    };

    // Drop slave - will close the fd
    drop(slave);

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            drop(master);
            return Err(e.into());
        }
    };

    // Use blocking I/O for vsock stream
    // Read/write loop - take ownership of master fd
    let mut master_file = unsafe { std::fs::File::from_raw_fd(master_fd) };
    // Prevent master OwnedFd from closing the fd since we transferred it
    std::mem::forget(master);

    // Simple polling loop (in production, use proper async or epoll)
    let mut buf = [0u8; 4096];
    loop {
        // Check if child exited
        if let Some(status) = child.try_wait()? {
            // Drain any remaining output
            while let Ok(n) = master_file.read(&mut buf) {
                if n == 0 {
                    break;
                }
                send_frame(stream, frame_type::STDOUT, &buf[..n])?;
            }
            return Ok(status.code().unwrap_or(128));
        }

        // Read from PTY master, write to stream
        // Note: This is simplified; real implementation needs non-blocking I/O
        if let Ok(n) = master_file.read(&mut buf) {
            if n > 0 {
                send_frame(stream, frame_type::STDOUT, &buf[..n])?;
            }
        }

        // Read from stream, write to PTY
        // TODO: Handle control messages for resize
    }
}

/// Execute command with separate stdin/stdout/stderr pipes.
fn execute_with_pipes(stream: &mut VsockStream, request: &ExecRequest) -> Result<i32> {
    let program = &request.command[0];
    let args: Vec<&str> = request.command.iter().map(|s| s.as_str()).collect();

    let mut child = Command::new(program)
        .args(&args[1..])
        .envs(&request.env)
        .stdin(if request.stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Get handles
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    // Simple output forwarding (simplified implementation)
    let mut buf = [0u8; 4096];

    loop {
        // Check if child exited
        if let Some(status) = child.try_wait()? {
            // Drain remaining output
            if let Some(ref mut out) = stdout {
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    send_frame(stream, frame_type::STDOUT, &buf[..n])?;
                }
            }
            if let Some(ref mut err) = stderr {
                while let Ok(n) = err.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    send_frame(stream, frame_type::STDERR, &buf[..n])?;
                }
            }
            return Ok(status.code().unwrap_or(128));
        }

        // Forward stdout
        if let Some(ref mut out) = stdout {
            if let Ok(n) = out.read(&mut buf) {
                if n > 0 {
                    send_frame(stream, frame_type::STDOUT, &buf[..n])?;
                }
            }
        }

        // Forward stderr
        if let Some(ref mut err) = stderr {
            if let Ok(n) = err.read(&mut buf) {
                if n > 0 {
                    send_frame(stream, frame_type::STDERR, &buf[..n])?;
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Send a frame over the stream.
fn send_frame(stream: &mut VsockStream, frame_type: u8, data: &[u8]) -> Result<()> {
    let mut frame = Vec::with_capacity(1 + data.len());
    frame.push(frame_type);
    frame.extend_from_slice(data);
    stream.write_all(&frame)?;
    stream.flush()?;
    Ok(())
}

/// Send exit status.
fn send_exit(stream: &mut VsockStream, exit_code: i32, reason: &str) -> Result<()> {
    let msg = ExitMessage::new(exit_code, reason);
    let json = serde_json::to_string(&msg)?;
    send_frame(stream, frame_type::EXIT, json.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_request_deserialization() {
        let json = r#"{
            "command": ["sh", "-c", "echo hello"],
            "tty": true,
            "cols": 120,
            "rows": 40
        }"#;

        let request: ExecRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.command, vec!["sh", "-c", "echo hello"]);
        assert!(request.tty);
        assert_eq!(request.cols, 120);
        assert_eq!(request.rows, 40);
    }

    #[test]
    fn test_exit_message_serialization() {
        let msg = ExitMessage::new(0, "exited");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"exit_code\":0"));
        assert!(json.contains("\"reason\":\"exited\""));
    }
}
