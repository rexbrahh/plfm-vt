//! Control plane stream actor - maintains connection to control plane.
//!
//! Per `docs/specs/runtime/agent-actors.md`, the ControlPlaneStreamActor:
//! - Maintains the long-lived connection to the control plane
//! - Handles reconnection with exponential backoff
//! - Processes events from the control plane
//! - Sends heartbeats

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::framework::{Actor, ActorContext, ActorError, BackoffPolicy};
use crate::client::{ControlPlaneClient, HeartbeatRequest, NodePlan, NodeState};

// =============================================================================
// Messages
// =============================================================================

/// Messages handled by ControlPlaneStreamActor.
#[derive(Debug)]
pub enum StreamMessage {
    /// Connect or reconnect to the control plane.
    Connect { force: bool },

    /// Send a heartbeat.
    SendHeartbeat { tick_id: u64 },

    /// Received an event from the stream.
    StreamEvent { event_type: String, payload: String },

    /// Connection was lost.
    Disconnected { reason: String },
}

// =============================================================================
// Actor State
// =============================================================================

/// Connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected.
    Disconnected,
    /// Attempting to connect.
    Connecting,
    /// Connected and streaming.
    Connected,
    /// Waiting before reconnect attempt.
    BackoffWait,
}

/// Persisted state for recovery.
#[derive(Debug, Clone, Default)]
pub struct StreamActorState {
    /// Last event cursor position.
    pub last_event_cursor: u64,

    /// Last successful connection time.
    pub last_connected_at: Option<Instant>,

    pub last_cursor_event_id: i64,
    pub last_plan_id: Option<String>,

    pub next_reconnect_at: Option<Instant>,

    /// Consecutive connection failures.
    pub consecutive_failures: u32,
}

// =============================================================================
// Control Plane Stream Actor
// =============================================================================

/// Actor maintaining connection to the control plane.
pub struct ControlPlaneStreamActor {
    /// Node ID.
    node_id: String,

    /// Control plane URL.
    control_plane_url: String,

    client: Arc<ControlPlaneClient>,

    plan_tx: mpsc::Sender<NodePlan>,

    instance_count: Arc<AtomicUsize>,

    /// Current connection state.
    state: ConnectionState,

    /// Persisted state.
    persisted: StreamActorState,

    /// Backoff policy for reconnection.
    backoff: BackoffPolicy,

    /// Last heartbeat time.
    last_heartbeat_at: Option<Instant>,

    /// Heartbeat interval.
    heartbeat_interval: Duration,
}

impl ControlPlaneStreamActor {
    /// Create a new stream actor.
    pub fn new(
        node_id: String,
        control_plane_url: String,
        client: Arc<ControlPlaneClient>,
        plan_tx: mpsc::Sender<NodePlan>,
        instance_count: Arc<AtomicUsize>,
        heartbeat_interval: Duration,
    ) -> Self {
        Self {
            node_id,
            control_plane_url,
            client,
            plan_tx,
            instance_count,
            state: ConnectionState::Disconnected,
            persisted: StreamActorState::default(),
            backoff: BackoffPolicy::default(),
            last_heartbeat_at: None,
            heartbeat_interval,
        }
    }

    /// Get the current connection state.
    pub fn connection_state(&self) -> ConnectionState {
        self.state
    }

    /// Get the last event cursor.
    pub fn last_event_cursor(&self) -> u64 {
        self.persisted.last_event_cursor
    }

    // -------------------------------------------------------------------------
    // Message Handlers
    // -------------------------------------------------------------------------

    async fn handle_connect(&mut self, force: bool) -> Result<(), ActorError> {
        if self.state == ConnectionState::Connected && !force {
            debug!("Already connected, ignoring connect request");
            return Ok(());
        }

        info!(
            node_id = %self.node_id,
            url = %self.control_plane_url,
            cursor = self.persisted.last_event_cursor,
            "Connecting to control plane"
        );

        self.state = ConnectionState::Connecting;

        self.state = ConnectionState::Connected;
        self.persisted.last_connected_at = Some(Instant::now());
        self.persisted.consecutive_failures = 0;
        self.persisted.next_reconnect_at = None;

        info!(
            node_id = %self.node_id,
            "Connected to control plane"
        );

        if let Err(e) = self.fetch_and_publish_plan().await {
            self.handle_disconnected(format!("plan fetch failed: {e}"))
                .await?;
        }

        Ok(())
    }

    async fn fetch_and_publish_plan(&mut self) -> Result<(), ActorError> {
        let plan = self
            .client
            .fetch_plan()
            .await
            .map_err(|e| ActorError::Transient(e.to_string()))?;

        if plan.cursor_event_id < self.persisted.last_cursor_event_id {
            return Ok(());
        }

        if plan.cursor_event_id == self.persisted.last_cursor_event_id
            && self.persisted.last_plan_id.as_deref() == Some(plan.plan_id.as_str())
        {
            return Ok(());
        }

        let cursor_event_id = plan.cursor_event_id;
        let plan_id = plan.plan_id.clone();

        if let Err(e) = self.plan_tx.try_send(plan) {
            warn!(error = %e, "Failed to send plan update");
            return Ok(());
        }

        self.persisted.last_cursor_event_id = cursor_event_id;
        self.persisted.last_plan_id = Some(plan_id);

        Ok(())
    }

    async fn maybe_reconnect(&mut self) -> Result<(), ActorError> {
        if self.state == ConnectionState::Connecting {
            return Ok(());
        }

        if self.state == ConnectionState::BackoffWait {
            if let Some(next) = self.persisted.next_reconnect_at {
                if Instant::now() < next {
                    return Ok(());
                }
            }
        }

        if self.state != ConnectionState::Connected {
            self.handle_connect(true).await?;
        }

        Ok(())
    }

    async fn handle_heartbeat(&mut self, _tick_id: u64) -> Result<(), ActorError> {
        if self.state != ConnectionState::Connected {
            self.maybe_reconnect().await?;
            return Ok(());
        }

        if let Some(last) = self.last_heartbeat_at {
            if last.elapsed() < self.heartbeat_interval {
                return Ok(());
            }
        }

        let instance_count = self.instance_count.load(Ordering::Relaxed) as i32;
        let request = HeartbeatRequest {
            state: NodeState::Active,
            available_cpu_cores: 8,
            available_memory_bytes: 16 * 1024 * 1024 * 1024,
            instance_count,
        };

        debug!(node_id = %self.node_id, "Sending heartbeat");

        if let Err(e) = self.client.send_heartbeat(&request).await {
            self.handle_disconnected(format!("heartbeat failed: {e}"))
                .await?;
            return Ok(());
        }

        self.last_heartbeat_at = Some(Instant::now());

        if let Err(e) = self.fetch_and_publish_plan().await {
            warn!(error = %e, "Plan fetch failed");
        }

        Ok(())
    }

    fn handle_stream_event(&mut self, event_type: String, _payload: String) {
        debug!(
            event_type = %event_type,
            cursor = self.persisted.last_event_cursor,
            "Received stream event"
        );

        // Update cursor
        self.persisted.last_event_cursor += 1;

        // TODO: Dispatch event to appropriate handler
    }

    async fn handle_disconnected(&mut self, reason: String) -> Result<(), ActorError> {
        warn!(
            node_id = %self.node_id,
            reason = %reason,
            "Disconnected from control plane"
        );

        self.state = ConnectionState::BackoffWait;
        self.persisted.consecutive_failures += 1;

        let delay = self.backoff.delay(self.persisted.consecutive_failures);
        self.persisted.next_reconnect_at = Some(Instant::now() + delay);

        info!(
            attempt = self.persisted.consecutive_failures,
            delay_ms = delay.as_millis(),
            "Scheduling reconnect"
        );

        Ok(())
    }
}

#[async_trait]
impl Actor for ControlPlaneStreamActor {
    type Message = StreamMessage;

    fn name(&self) -> &str {
        "control_plane_stream"
    }

    async fn handle(
        &mut self,
        msg: StreamMessage,
        _ctx: &mut ActorContext,
    ) -> Result<bool, ActorError> {
        match msg {
            StreamMessage::Connect { force } => {
                self.handle_connect(force).await?;
            }

            StreamMessage::SendHeartbeat { tick_id } => {
                self.handle_heartbeat(tick_id).await?;
            }

            StreamMessage::StreamEvent {
                event_type,
                payload,
            } => {
                self.handle_stream_event(event_type, payload);
            }

            StreamMessage::Disconnected { reason } => {
                self.handle_disconnected(reason).await?;
            }
        }

        Ok(true)
    }

    async fn on_start(&mut self, _ctx: &mut ActorContext) -> Result<(), ActorError> {
        info!(
            node_id = %self.node_id,
            url = %self.control_plane_url,
            "ControlPlaneStreamActor starting"
        );

        // Initiate connection
        self.handle_connect(false).await?;

        Ok(())
    }

    async fn on_stop(&mut self, _ctx: &mut ActorContext) {
        info!(
            node_id = %self.node_id,
            state = ?self.state,
            cursor = self.persisted.last_event_cursor,
            "ControlPlaneStreamActor stopping"
        );

        self.state = ConnectionState::Disconnected;
    }

    fn on_crash(&mut self, error: &ActorError) {
        warn!(
            node_id = %self.node_id,
            error = %error,
            "ControlPlaneStreamActor crashed"
        );
        self.state = ConnectionState::Disconnected;
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_actor_state_default() {
        let state = StreamActorState::default();
        assert_eq!(state.last_event_cursor, 0);
        assert!(state.last_connected_at.is_none());
        assert_eq!(state.last_cursor_event_id, 0);
        assert!(state.last_plan_id.is_none());
        assert!(state.next_reconnect_at.is_none());
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn test_connection_state() {
        assert_ne!(ConnectionState::Connected, ConnectionState::Disconnected);
        assert_eq!(ConnectionState::Connecting, ConnectionState::Connecting);
    }

    #[test]
    fn test_control_plane_stream_actor_new() {
        let config = crate::config::Config {
            node_id: plfm_id::NodeId::new(),
            control_plane_url: "https://api.example.com".to_string(),
            data_dir: "/tmp/test".to_string(),
            heartbeat_interval_secs: 30,
            log_level: "info".to_string(),
            exec_listen_addr: "127.0.0.1:0".parse().unwrap(),
        };
        let client = std::sync::Arc::new(crate::client::ControlPlaneClient::new(&config));
        let (plan_tx, _plan_rx) = tokio::sync::mpsc::channel(4);
        let instance_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let actor = ControlPlaneStreamActor::new(
            config.node_id.to_string(),
            config.control_plane_url.clone(),
            client,
            plan_tx,
            instance_count,
            std::time::Duration::from_secs(config.heartbeat_interval_secs),
        );
        assert_eq!(actor.connection_state(), ConnectionState::Disconnected);
        assert_eq!(actor.last_event_cursor(), 0);
    }
}
