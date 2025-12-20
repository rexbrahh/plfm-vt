//! Exec command - Interactive command execution in running instances.
//!
//! Per docs/specs/runtime/exec-sessions.md, exec sessions:
//! 1. Create an exec grant (session_id + connect_url + token)
//! 2. Connect via WebSocket with binary frame protocol
//! 3. Stream stdin/stdout/stderr with frame types:
//!    - 0x01: stdin (client -> server)
//!    - 0x02: stdout (server -> client)
//!    - 0x03: stderr (server -> client)
//!    - 0x10: JSON control message (bidirectional)
//!    - 0x11: exit status JSON (server -> client)

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::output::{print_info, print_single, print_success, OutputFormat};

use super::CommandContext;

// =============================================================================
// Frame Types (per exec-sessions.md spec)
// =============================================================================

const FRAME_STDIN: u8 = 0x01;
const FRAME_STDOUT: u8 = 0x02;
const FRAME_STDERR: u8 = 0x03;
const FRAME_CONTROL: u8 = 0x10;
const FRAME_EXIT: u8 = 0x11;

// =============================================================================
// CLI Exit Codes (per exec-sessions.md spec)
// =============================================================================

/// Exit code for auth failure.
pub const EXIT_AUTH_FAILURE: i32 = 10;
/// Exit code for instance not running.
pub const EXIT_INSTANCE_NOT_RUNNING: i32 = 20;
/// Exit code for connect timeout.
pub const EXIT_CONNECT_TIMEOUT: i32 = 30;
/// Exit code for server error.
pub const EXIT_SERVER_ERROR: i32 = 40;
/// Exit code for rate limited.
pub const EXIT_RATE_LIMITED: i32 = 50;

// =============================================================================
// Command Definition
// =============================================================================

/// Execute a command in a running instance.
///
/// Creates an exec session, connects via WebSocket, and streams I/O.
#[derive(Debug, Args)]
pub struct ExecCommand {
    /// Instance ID to exec into.
    pub instance: String,

    /// Allocate a pseudo-terminal (PTY).
    #[arg(long, short = 't', default_value_t = true)]
    pub tty: bool,

    /// Disable PTY allocation (pipe mode).
    #[arg(long, short = 'T', conflicts_with = "tty")]
    pub no_tty: bool,

    /// Only create a session grant without connecting (for external tools).
    #[arg(long)]
    pub grant_only: bool,

    /// Print the session token in table mode (sensitive).
    #[arg(long)]
    pub show_token: bool,

    /// Environment variables to set (KEY=VALUE format).
    #[arg(long = "set-env", short = 'e', value_name = "KEY=VALUE")]
    pub env_vars: Vec<String>,

    /// Command to run (after `--`).
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Serialize)]
struct ExecGrantRequest {
    command: Vec<String>,
    tty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cols: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExecGrantResponse {
    session_id: String,
    connect_url: String,
    session_token: String,
    expires_in_seconds: i64,
}

// =============================================================================
// Control Messages
// =============================================================================

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlMessage {
    Resize { cols: u16, rows: u16 },
    Signal { name: String },
    Close,
}

#[derive(Debug, Deserialize)]
struct ExitMessage {
    exit_code: i32,
    reason: String,
}

// =============================================================================
// Implementation
// =============================================================================

impl ExecCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        let client = ctx.client()?;
        let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
        let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
        let env_id =
            crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

        let use_tty = !self.no_tty && self.tty;

        // Get terminal size if TTY mode
        let (cols, rows) = if use_tty {
            terminal::size().unwrap_or((80, 24))
        } else {
            (80, 24)
        };

        // Parse environment variables
        let env = if self.env_vars.is_empty() {
            None
        } else {
            let mut map = std::collections::HashMap::new();
            for var in &self.env_vars {
                if let Some((key, value)) = var.split_once('=') {
                    map.insert(key.to_string(), value.to_string());
                } else {
                    anyhow::bail!("Invalid environment variable format: {}. Expected KEY=VALUE", var);
                }
            }
            Some(map)
        };

        let path = format!(
            "/v1/orgs/{}/apps/{}/envs/{}/instances/{}/exec",
            org_id, app_id, env_id, self.instance
        );

        let request = ExecGrantRequest {
            command: self.command.clone(),
            tty: use_tty,
            cols: if use_tty { Some(cols) } else { None },
            rows: if use_tty { Some(rows) } else { None },
            env,
        };

        let idempotency_key = match ctx.idempotency_key.as_deref() {
            Some(key) => key.to_string(),
            None => crate::idempotency::default_idempotency_key("exec.grant", &path, &request)?,
        };

        let response: ExecGrantResponse = client
            .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
            .await
            .map_err(|e| {
                // Map API errors to appropriate exit codes
                let msg = e.to_string();
                if msg.contains("instance_not_ready") {
                    std::process::exit(EXIT_INSTANCE_NOT_RUNNING);
                } else if msg.contains("unauthorized") || msg.contains("forbidden") {
                    std::process::exit(EXIT_AUTH_FAILURE);
                } else if msg.contains("rate_limit") || msg.contains("429") {
                    std::process::exit(EXIT_RATE_LIMITED);
                }
                e
            })?;

        // If grant-only mode, just print the grant and exit
        if self.grant_only {
            return self.print_grant_only(&response, &ctx);
        }

        // Connect and stream
        let exit_code = self.connect_and_stream(&response, &ctx, use_tty).await?;

        std::process::exit(exit_code);
    }

    /// Print grant-only output (for external tools).
    fn print_grant_only(&self, response: &ExecGrantResponse, ctx: &CommandContext) -> Result<()> {
        match ctx.format {
            OutputFormat::Json => print_single(response, ctx.format),
            OutputFormat::Table => {
                print_success(&format!(
                    "Created exec grant session {} (expires in {}s)",
                    response.session_id, response.expires_in_seconds
                ));
                print_info(&format!("connect_url: {}", response.connect_url));
                if self.show_token {
                    print_info(&format!("session_token: {}", response.session_token));
                } else {
                    print_info(
                        "session_token is sensitive; use --show-token or --format json to print it",
                    );
                }
            }
        }
        Ok(())
    }

    /// Connect to the exec session via WebSocket and stream I/O.
    async fn connect_and_stream(
        &self,
        grant: &ExecGrantResponse,
        ctx: &CommandContext,
        use_tty: bool,
    ) -> Result<i32> {
        // Build WebSocket URL
        let base_url = ctx.config.api_url.trim_end_matches('/');
        let ws_url = if base_url.starts_with("https://") {
            format!(
                "wss://{}{}?token={}",
                &base_url[8..],
                grant.connect_url,
                grant.session_token
            )
        } else if base_url.starts_with("http://") {
            format!(
                "ws://{}{}?token={}",
                &base_url[7..],
                grant.connect_url,
                grant.session_token
            )
        } else {
            anyhow::bail!("Invalid API URL format: {}", base_url);
        };

        // Connect with timeout
        let connect_timeout = std::time::Duration::from_secs(30);
        let ws_result = tokio::time::timeout(
            connect_timeout,
            tokio_tungstenite::connect_async(&ws_url),
        )
        .await;

        let (ws_stream, _) = match ws_result {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                anyhow::bail!("Failed to connect to exec session: {}", e);
            }
            Err(_) => {
                // Timeout elapsed
                eprintln!("Connection timeout after {} seconds", connect_timeout.as_secs());
                std::process::exit(EXIT_CONNECT_TIMEOUT);
            }
        };

        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Set up terminal if TTY mode
        let _raw_guard = if use_tty {
            enable_raw_mode().ok();
            Some(RawModeGuard)
        } else {
            None
        };

        // Channel for sending messages to WebSocket
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);

        // Track if we should exit
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        // Set up signal handler for Ctrl+C
        let tx_signal = tx.clone();
        let running_signal = running.clone();
        let _ = ctrlc::set_handler(move || {
            // Send SIGINT via control message
            let msg = ControlMessage::Signal {
                name: "INT".to_string(),
            };
            if let Ok(json) = serde_json::to_vec(&msg) {
                let mut frame = vec![FRAME_CONTROL];
                frame.extend(json);
                let _ = tx_signal.blocking_send(frame);
            }
            running_signal.store(false, Ordering::SeqCst);
        });

        // Spawn stdin reader task
        let tx_stdin = tx.clone();
        let running_stdin = running.clone();
        let stdin_handle = if use_tty {
            Some(tokio::task::spawn_blocking(move || {
                let mut stdin = io::stdin();
                let mut buf = [0u8; 1024];
                while running_stdin.load(Ordering::SeqCst) {
                    match stdin.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let mut frame = vec![FRAME_STDIN];
                            frame.extend_from_slice(&buf[..n]);
                            if tx_stdin.blocking_send(frame).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            }))
        } else {
            // Pipe mode: read stdin asynchronously
            let tx_pipe = tx.clone();
            Some(tokio::spawn(async move {
                let mut stdin = tokio::io::stdin();
                let mut buf = [0u8; 4096];
                loop {
                    match stdin.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let mut frame = vec![FRAME_STDIN];
                            frame.extend_from_slice(&buf[..n]);
                            if tx_pipe.send(frame).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }))
        };

        // Spawn terminal resize watcher (TTY mode only)
        let resize_handle = if use_tty {
            let tx_resize = tx.clone();
            let running_resize = running.clone();
            Some(tokio::spawn(async move {
                let mut last_size = terminal::size().unwrap_or((80, 24));
                while running_resize.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    if let Ok(size) = terminal::size() {
                        if size != last_size {
                            last_size = size;
                            let msg = ControlMessage::Resize {
                                cols: size.0,
                                rows: size.1,
                            };
                            if let Ok(json) = serde_json::to_vec(&msg) {
                                let mut frame = vec![FRAME_CONTROL];
                                frame.extend(json);
                                let _ = tx_resize.send(frame).await;
                            }
                        }
                    }
                }
            }))
        } else {
            None
        };

        // Main event loop
        let mut exit_code = 0i32;
        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();

        loop {
            tokio::select! {
                // Handle outgoing messages
                Some(frame) = rx.recv() => {
                    if ws_write.send(Message::Binary(frame)).await.is_err() {
                        break;
                    }
                }

                // Handle incoming WebSocket messages
                msg = ws_read.next() => {
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            if data.is_empty() {
                                continue;
                            }
                            let frame_type = data[0];
                            let payload = &data[1..];

                            match frame_type {
                                FRAME_STDOUT => {
                                    stdout.write_all(payload).await?;
                                    stdout.flush().await?;
                                }
                                FRAME_STDERR => {
                                    stderr.write_all(payload).await?;
                                    stderr.flush().await?;
                                }
                                FRAME_EXIT => {
                                    if let Ok(exit_msg) = serde_json::from_slice::<ExitMessage>(payload) {
                                        exit_code = exit_msg.exit_code;
                                        if exit_msg.reason != "exited" && exit_msg.reason != "killed" {
                                            // Log non-normal exit reasons
                                            eprintln!("\r\n[exec session ended: {}]", exit_msg.reason);
                                        }
                                    }
                                    running_clone.store(false, Ordering::SeqCst);
                                    break;
                                }
                                FRAME_CONTROL => {
                                    // Server-side control messages (future expansion)
                                }
                                _ => {
                                    // Unknown frame type, ignore
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            running_clone.store(false, Ordering::SeqCst);
                            break;
                        }
                        Some(Err(e)) => {
                            eprintln!("\r\n[connection error: {}]", e);
                            exit_code = EXIT_SERVER_ERROR;
                            break;
                        }
                        None => {
                            // Stream ended
                            break;
                        }
                        _ => {
                            // Ignore text/ping/pong frames
                        }
                    }
                }
            }
        }

        // Cleanup
        running.store(false, Ordering::SeqCst);

        // Send close message
        let close_msg = ControlMessage::Close;
        if let Ok(json) = serde_json::to_vec(&close_msg) {
            let mut frame = vec![FRAME_CONTROL];
            frame.extend(json);
            let _ = ws_write.send(Message::Binary(frame)).await;
        }
        let _ = ws_write.close().await;

        // Wait for background tasks
        if let Some(handle) = stdin_handle {
            let _ = tokio::time::timeout(std::time::Duration::from_millis(100), handle).await;
        }
        if let Some(handle) = resize_handle {
            handle.abort();
        }

        Ok(exit_code)
    }
}

/// RAII guard to restore terminal mode on drop.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_message_serialization() {
        let resize = ControlMessage::Resize { cols: 120, rows: 40 };
        let json = serde_json::to_string(&resize).unwrap();
        assert!(json.contains("\"type\":\"resize\""));
        assert!(json.contains("\"cols\":120"));
        assert!(json.contains("\"rows\":40"));

        let signal = ControlMessage::Signal {
            name: "INT".to_string(),
        };
        let json = serde_json::to_string(&signal).unwrap();
        assert!(json.contains("\"type\":\"signal\""));
        assert!(json.contains("\"name\":\"INT\""));

        let close = ControlMessage::Close;
        let json = serde_json::to_string(&close).unwrap();
        assert!(json.contains("\"type\":\"close\""));
    }

    #[test]
    fn test_exit_message_deserialization() {
        let json = r#"{"exit_code": 0, "reason": "exited"}"#;
        let msg: ExitMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.exit_code, 0);
        assert_eq!(msg.reason, "exited");

        let json = r#"{"exit_code": 137, "reason": "killed"}"#;
        let msg: ExitMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.exit_code, 137);
        assert_eq!(msg.reason, "killed");
    }

    #[test]
    fn test_frame_constants() {
        assert_eq!(FRAME_STDIN, 0x01);
        assert_eq!(FRAME_STDOUT, 0x02);
        assert_eq!(FRAME_STDERR, 0x03);
        assert_eq!(FRAME_CONTROL, 0x10);
        assert_eq!(FRAME_EXIT, 0x11);
    }

    #[test]
    fn test_exec_grant_request_serialization() {
        let req = ExecGrantRequest {
            command: vec!["sh".to_string(), "-c".to_string(), "uptime".to_string()],
            tty: true,
            cols: Some(120),
            rows: Some(40),
            env: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"command\":[\"sh\",\"-c\",\"uptime\"]"));
        assert!(json.contains("\"tty\":true"));
        assert!(json.contains("\"cols\":120"));
        assert!(json.contains("\"rows\":40"));
        // env should be omitted when None
        assert!(!json.contains("\"env\""));
    }

    #[test]
    fn test_exec_grant_request_with_env() {
        let mut env = std::collections::HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("BAZ".to_string(), "qux".to_string());

        let req = ExecGrantRequest {
            command: vec!["printenv".to_string()],
            tty: false,
            cols: None,
            rows: None,
            env: Some(env),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"env\":{"));
        assert!(json.contains("\"FOO\":\"bar\""));
    }

    #[test]
    fn test_exit_codes() {
        assert_eq!(EXIT_AUTH_FAILURE, 10);
        assert_eq!(EXIT_INSTANCE_NOT_RUNNING, 20);
        assert_eq!(EXIT_CONNECT_TIMEOUT, 30);
        assert_eq!(EXIT_SERVER_ERROR, 40);
        assert_eq!(EXIT_RATE_LIMITED, 50);
    }
}
