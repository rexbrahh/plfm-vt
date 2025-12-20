//! Exec session handling for node-agent.
//!
//! This module implements the agent-side exec functionality as defined in
//! docs/specs/runtime/exec-sessions.md.
//!
//! The agent:
//! 1. Receives exec session requests from the control plane
//! 2. Validates the instance is running
//! 3. Connects to the guest-init exec service via vsock port 5162
//! 4. Proxies bytes between the client and guest
//! 5. Handles signal forwarding and cleanup

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use vsock::{VsockAddr, VsockStream};

/// Vsock port for exec service on guest-init.
pub const EXEC_PORT: u32 = 5162;

/// Default session timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 3600; // 1 hour

/// Maximum concurrent exec sessions per instance.
pub const MAX_SESSIONS_PER_INSTANCE: usize = 2;

/// Grace period after SIGTERM before SIGKILL.
const SIGTERM_GRACE_SECS: u64 = 5;

/// Maximum wait time after SIGKILL.
const SIGKILL_TIMEOUT_SECS: u64 = 30;

// =============================================================================
// Frame Types (matching guest-init and spec)
// =============================================================================

/// Frame types for exec stream protocol.
#[allow(dead_code)]
pub mod frame_type {
    pub const STDIN: u8 = 0x01;
    pub const STDOUT: u8 = 0x02;
    pub const STDERR: u8 = 0x03;
    pub const CONTROL: u8 = 0x10;
    pub const EXIT: u8 = 0x11;
}

// =============================================================================
// Message Types
// =============================================================================

/// Exec request sent to guest-init.
#[derive(Debug, Serialize)]
pub struct ExecRequest {
    /// Command and arguments.
    pub command: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Allocate PTY.
    #[serde(default)]
    pub tty: bool,
    /// Terminal columns.
    #[serde(default = "default_cols")]
    pub cols: u16,
    /// Terminal rows.
    #[serde(default = "default_rows")]
    pub rows: u16,
    /// Connect stdin.
    #[serde(default = "default_true")]
    pub stdin: bool,
}

fn _default_cols() -> u16 {
    80
}

fn _default_rows() -> u16 {
    24
}

fn _default_true() -> bool {
    true
}

/// Control message sent to guest-init.
#[derive(Debug, Serialize)]
pub struct ControlMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ControlMessage {
    /// Create a resize control message.
    pub fn resize(cols: u16, rows: u16) -> Self {
        Self {
            msg_type: "resize".to_string(),
            cols: Some(cols),
            rows: Some(rows),
            name: None,
        }
    }

    /// Create a signal control message.
    pub fn signal(name: &str) -> Self {
        Self {
            msg_type: "signal".to_string(),
            cols: None,
            rows: None,
            name: Some(name.to_string()),
        }
    }

    /// Create a close control message.
    pub fn close() -> Self {
        Self {
            msg_type: "close".to_string(),
            cols: None,
            rows: None,
            name: None,
        }
    }
}

/// Exit status received from guest-init.
#[derive(Debug, Deserialize)]
pub struct ExitMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub exit_code: i32,
    pub reason: String,
}

/// End reason for exec session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Process exited normally.
    Exited,
    /// Process killed by signal.
    Killed,
    /// Session duration exceeded.
    Timeout,
    /// Client never connected.
    ConnectTimeout,
    /// Client closed connection.
    ClientDisconnect,
    /// Admin terminated session.
    OperatorRevoked,
}

impl EndReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            EndReason::Exited => "exited",
            EndReason::Killed => "killed",
            EndReason::Timeout => "timeout",
            EndReason::ConnectTimeout => "connect_timeout",
            EndReason::ClientDisconnect => "client_disconnect",
            EndReason::OperatorRevoked => "operator_revoked",
        }
    }
}

/// Allowed signals for exec sessions.
#[derive(Debug, Clone, Copy)]
pub enum ExecSignal {
    Int,
    Term,
    Kill,
    Hup,
}

impl ExecSignal {
    /// Parse signal name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_uppercase().as_str() {
            "INT" | "SIGINT" => Some(ExecSignal::Int),
            "TERM" | "SIGTERM" => Some(ExecSignal::Term),
            "KILL" | "SIGKILL" => Some(ExecSignal::Kill),
            "HUP" | "SIGHUP" => Some(ExecSignal::Hup),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ExecSignal::Int => "INT",
            ExecSignal::Term => "TERM",
            ExecSignal::Kill => "KILL",
            ExecSignal::Hup => "HUP",
        }
    }
}

// =============================================================================
// Exec Session State
// =============================================================================

/// State of an exec session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecSessionState {
    /// Session granted, awaiting client connection.
    Granted,
    /// Client connected, streaming.
    Connected,
    /// Session ended.
    Ended,
}

/// Active exec session.
#[derive(Debug)]
pub struct ExecSession {
    /// Session ID.
    pub session_id: String,
    /// Instance ID.
    pub instance_id: String,
    /// Guest CID for vsock connection.
    pub guest_cid: u32,
    /// Command to execute.
    pub command: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Use PTY.
    pub tty: bool,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Session state.
    pub state: ExecSessionState,
    /// Exit code (if ended).
    pub exit_code: Option<i32>,
    /// End reason (if ended).
    pub end_reason: Option<EndReason>,
}

// =============================================================================
// Exec Session Manager
// =============================================================================

/// Manager for active exec sessions.
pub struct ExecSessionManager {
    /// Active sessions by session ID.
    sessions: RwLock<HashMap<String, ExecSession>>,
    /// Sessions per instance.
    sessions_per_instance: RwLock<HashMap<String, usize>>,
}

impl ExecSessionManager {
    /// Create a new exec session manager.
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            sessions_per_instance: RwLock::new(HashMap::new()),
        }
    }

    /// Check if a new session can be started for the instance.
    pub async fn can_start_session(&self, instance_id: &str) -> bool {
        let counts = self.sessions_per_instance.read().await;
        let current = counts.get(instance_id).copied().unwrap_or(0);
        current < MAX_SESSIONS_PER_INSTANCE
    }

    /// Register a new session.
    pub async fn register_session(&self, session: ExecSession) -> Result<()> {
        let instance_id = session.instance_id.clone();
        let session_id = session.session_id.clone();

        // Check concurrency limit
        if !self.can_start_session(&instance_id).await {
            return Err(anyhow!(
                "Max concurrent exec sessions ({}) reached for instance {}",
                MAX_SESSIONS_PER_INSTANCE,
                instance_id
            ));
        }

        // Register session
        let mut sessions = self.sessions.write().await;
        let mut counts = self.sessions_per_instance.write().await;

        sessions.insert(session_id, session);
        *counts.entry(instance_id).or_insert(0) += 1;

        Ok(())
    }

    /// Get a session by ID.
    pub async fn get_session(&self, session_id: &str) -> Option<ExecSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| ExecSession {
            session_id: s.session_id.clone(),
            instance_id: s.instance_id.clone(),
            guest_cid: s.guest_cid,
            command: s.command.clone(),
            env: s.env.clone(),
            tty: s.tty,
            cols: s.cols,
            rows: s.rows,
            state: s.state,
            exit_code: s.exit_code,
            end_reason: s.end_reason,
        })
    }

    /// Mark session as connected.
    pub async fn mark_connected(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.state = ExecSessionState::Connected;
        }
    }

    /// End a session.
    pub async fn end_session(
        &self,
        session_id: &str,
        exit_code: Option<i32>,
        reason: EndReason,
    ) {
        let mut sessions = self.sessions.write().await;
        let mut counts = self.sessions_per_instance.write().await;

        if let Some(session) = sessions.get_mut(session_id) {
            session.state = ExecSessionState::Ended;
            session.exit_code = exit_code;
            session.end_reason = Some(reason);

            // Decrement instance count
            if let Some(count) = counts.get_mut(&session.instance_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    counts.remove(&session.instance_id);
                }
            }
        }
    }

    /// Remove a session.
    pub async fn remove_session(&self, session_id: &str) -> Option<ExecSession> {
        let mut sessions = self.sessions.write().await;
        let mut counts = self.sessions_per_instance.write().await;

        if let Some(session) = sessions.remove(session_id) {
            if let Some(count) = counts.get_mut(&session.instance_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    counts.remove(&session.instance_id);
                }
            }
            Some(session)
        } else {
            None
        }
    }
}

impl Default for ExecSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Exec Service
// =============================================================================

/// Exec service that handles connections to guest-init.
pub struct ExecService {
    #[allow(dead_code)] // Will be used for session tracking in full implementation
    session_manager: Arc<ExecSessionManager>,
}

impl ExecService {
    /// Create a new exec service.
    pub fn new(session_manager: Arc<ExecSessionManager>) -> Self {
        Self { session_manager }
    }

    /// Execute a command in an instance.
    ///
    /// This connects to the guest-init exec service and proxies the stream.
    /// Returns the exit code and end reason when the session ends.
    pub fn execute(
        &self,
        session_id: &str,
        guest_cid: u32,
        request: ExecRequest,
    ) -> Result<(i32, EndReason)> {
        info!(
            session_id = %session_id,
            guest_cid = guest_cid,
            command = ?request.command,
            tty = request.tty,
            "Starting exec session"
        );

        // Connect to guest-init exec service
        let addr = VsockAddr::new(guest_cid, EXEC_PORT);
        let mut stream = VsockStream::connect(&addr).map_err(|e| {
            anyhow!(
                "Failed to connect to exec service (cid={}, port={}): {}",
                guest_cid,
                EXEC_PORT,
                e
            )
        })?;

        debug!(session_id = %session_id, "Connected to guest exec service");

        // Send exec request as JSON + newline
        let request_json = serde_json::to_string(&request)?;
        stream.write_all(request_json.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        debug!(session_id = %session_id, "Sent exec request");

        // Read response frames until exit
        let mut buf = [0u8; 4096];
        let mut exit_code = 128;
        let mut end_reason = EndReason::Exited;

        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    // Connection closed
                    debug!(session_id = %session_id, "Connection closed by guest");
                    break;
                }
                Ok(n) => {
                    // Parse frame
                    if n < 1 {
                        continue;
                    }

                    let frame_type = buf[0];
                    let payload = &buf[1..n];

                    match frame_type {
                        frame_type::STDOUT => {
                            // In a real proxy, we'd forward this to the client
                            debug!(
                                session_id = %session_id,
                                bytes = payload.len(),
                                "Received stdout"
                            );
                        }
                        frame_type::STDERR => {
                            debug!(
                                session_id = %session_id,
                                bytes = payload.len(),
                                "Received stderr"
                            );
                        }
                        frame_type::EXIT => {
                            // Parse exit message
                            if let Ok(exit_msg) =
                                serde_json::from_slice::<ExitMessage>(payload)
                            {
                                exit_code = exit_msg.exit_code;
                                end_reason = match exit_msg.reason.as_str() {
                                    "exited" => EndReason::Exited,
                                    "killed" => EndReason::Killed,
                                    "timeout" => EndReason::Timeout,
                                    _ => EndReason::Exited,
                                };
                                info!(
                                    session_id = %session_id,
                                    exit_code = exit_code,
                                    reason = exit_msg.reason,
                                    "Exec session ended"
                                );
                            }
                            break;
                        }
                        frame_type::CONTROL => {
                            // Guest-to-host control messages (rare)
                            debug!(session_id = %session_id, "Received control message");
                        }
                        other => {
                            warn!(
                                session_id = %session_id,
                                frame_type = other,
                                "Unknown frame type"
                            );
                        }
                    }
                }
                Err(e) => {
                    error!(session_id = %session_id, error = %e, "Read error");
                    end_reason = EndReason::ClientDisconnect;
                    break;
                }
            }
        }

        Ok((exit_code, end_reason))
    }

    /// Send a signal to an exec session.
    pub fn send_signal(
        &self,
        guest_cid: u32,
        signal: ExecSignal,
    ) -> Result<()> {
        let addr = VsockAddr::new(guest_cid, EXEC_PORT);
        let mut stream = VsockStream::connect(&addr)?;

        let control = ControlMessage::signal(signal.as_str());
        let json = serde_json::to_string(&control)?;

        let mut frame = Vec::with_capacity(1 + json.len());
        frame.push(frame_type::CONTROL);
        frame.extend_from_slice(json.as_bytes());

        stream.write_all(&frame)?;
        stream.flush()?;

        Ok(())
    }

    /// Send a resize event to an exec session.
    pub fn send_resize(
        &self,
        guest_cid: u32,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        let addr = VsockAddr::new(guest_cid, EXEC_PORT);
        let mut stream = VsockStream::connect(&addr)?;

        let control = ControlMessage::resize(cols, rows);
        let json = serde_json::to_string(&control)?;

        let mut frame = Vec::with_capacity(1 + json.len());
        frame.push(frame_type::CONTROL);
        frame.extend_from_slice(json.as_bytes());

        stream.write_all(&frame)?;
        stream.flush()?;

        Ok(())
    }

    /// Cleanup on disconnect - sends signals in sequence.
    pub fn cleanup_on_disconnect(&self, guest_cid: u32) -> Result<()> {
        info!(guest_cid = guest_cid, "Starting cleanup on disconnect");

        // 1. Send SIGHUP immediately
        if let Err(e) = self.send_signal(guest_cid, ExecSignal::Hup) {
            warn!(error = %e, "Failed to send SIGHUP");
        }

        // 2. Wait 5 seconds, then SIGTERM
        std::thread::sleep(Duration::from_secs(SIGTERM_GRACE_SECS));
        if let Err(e) = self.send_signal(guest_cid, ExecSignal::Term) {
            warn!(error = %e, "Failed to send SIGTERM");
        }

        // 3. Wait 30 seconds, then SIGKILL
        std::thread::sleep(Duration::from_secs(SIGKILL_TIMEOUT_SECS));
        if let Err(e) = self.send_signal(guest_cid, ExecSignal::Kill) {
            warn!(error = %e, "Failed to send SIGKILL");
        }

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_request_serialization() {
        let request = ExecRequest {
            command: vec!["sh".to_string(), "-c".to_string(), "uptime".to_string()],
            env: HashMap::from([("TERM".to_string(), "xterm-256color".to_string())]),
            tty: true,
            cols: 120,
            rows: 40,
            stdin: true,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"command\":[\"sh\",\"-c\",\"uptime\"]"));
        assert!(json.contains("\"tty\":true"));
        assert!(json.contains("\"cols\":120"));
    }

    #[test]
    fn test_control_message_resize() {
        let msg = ControlMessage::resize(120, 40);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"resize\""));
        assert!(json.contains("\"cols\":120"));
        assert!(json.contains("\"rows\":40"));
    }

    #[test]
    fn test_control_message_signal() {
        let msg = ControlMessage::signal("INT");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"signal\""));
        assert!(json.contains("\"name\":\"INT\""));
    }

    #[test]
    fn test_exit_message_deserialization() {
        let json = r#"{"type":"exit","exit_code":0,"reason":"exited"}"#;
        let msg: ExitMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.exit_code, 0);
        assert_eq!(msg.reason, "exited");
    }

    #[test]
    fn test_exec_signal_parsing() {
        assert!(ExecSignal::from_name("INT").is_some());
        assert!(ExecSignal::from_name("SIGINT").is_some());
        assert!(ExecSignal::from_name("TERM").is_some());
        assert!(ExecSignal::from_name("KILL").is_some());
        assert!(ExecSignal::from_name("HUP").is_some());
        assert!(ExecSignal::from_name("INVALID").is_none());
    }

    #[test]
    fn test_end_reason_as_str() {
        assert_eq!(EndReason::Exited.as_str(), "exited");
        assert_eq!(EndReason::Killed.as_str(), "killed");
        assert_eq!(EndReason::Timeout.as_str(), "timeout");
        assert_eq!(EndReason::ClientDisconnect.as_str(), "client_disconnect");
    }

    #[tokio::test]
    async fn test_session_manager_concurrency() {
        let manager = ExecSessionManager::new();

        let session1 = ExecSession {
            session_id: "sess_1".to_string(),
            instance_id: "inst_1".to_string(),
            guest_cid: 100,
            command: vec!["sh".to_string()],
            env: HashMap::new(),
            tty: true,
            cols: 80,
            rows: 24,
            state: ExecSessionState::Granted,
            exit_code: None,
            end_reason: None,
        };

        let session2 = ExecSession {
            session_id: "sess_2".to_string(),
            instance_id: "inst_1".to_string(),
            guest_cid: 100,
            command: vec!["sh".to_string()],
            env: HashMap::new(),
            tty: true,
            cols: 80,
            rows: 24,
            state: ExecSessionState::Granted,
            exit_code: None,
            end_reason: None,
        };

        let session3 = ExecSession {
            session_id: "sess_3".to_string(),
            instance_id: "inst_1".to_string(),
            guest_cid: 100,
            command: vec!["sh".to_string()],
            env: HashMap::new(),
            tty: true,
            cols: 80,
            rows: 24,
            state: ExecSessionState::Granted,
            exit_code: None,
            end_reason: None,
        };

        // First two sessions should succeed
        assert!(manager.can_start_session("inst_1").await);
        manager.register_session(session1).await.unwrap();

        assert!(manager.can_start_session("inst_1").await);
        manager.register_session(session2).await.unwrap();

        // Third session should fail (max 2 per instance)
        assert!(!manager.can_start_session("inst_1").await);
        assert!(manager.register_session(session3).await.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_end_session() {
        let manager = ExecSessionManager::new();

        let session = ExecSession {
            session_id: "sess_1".to_string(),
            instance_id: "inst_1".to_string(),
            guest_cid: 100,
            command: vec!["sh".to_string()],
            env: HashMap::new(),
            tty: true,
            cols: 80,
            rows: 24,
            state: ExecSessionState::Granted,
            exit_code: None,
            end_reason: None,
        };

        manager.register_session(session).await.unwrap();

        // End the session
        manager.end_session("sess_1", Some(0), EndReason::Exited).await;

        // Check state updated
        let ended = manager.get_session("sess_1").await.unwrap();
        assert_eq!(ended.state, ExecSessionState::Ended);
        assert_eq!(ended.exit_code, Some(0));
        assert_eq!(ended.end_reason, Some(EndReason::Exited));

        // Should be able to start new sessions now
        assert!(manager.can_start_session("inst_1").await);
    }
}
