//! Instance actor - manages a single microVM instance lifecycle.
//!
//! Per `docs/specs/runtime/agent-actors.md`, the InstanceActor:
//! - Owns the full lifecycle of a single microVM
//! - Processes messages sequentially (no internal concurrency)
//! - Emits events for every state transition
//!
//! ## State Machine
//!
//! ```text
//! allocated -> preparing -> booting -> ready -> draining -> stopped -> garbage_collected
//!                  |           |         |
//!                  +------> failed <-----+
//! ```

use std::time::Instant;

use async_trait::async_trait;
use tracing::{debug, error, info, warn};

use super::framework::{Actor, ActorContext, ActorError};
use crate::client::InstancePlan;
use crate::runtime::{Runtime, VmHandle};

// =============================================================================
// Messages
// =============================================================================

/// Messages handled by InstanceActor.
#[derive(Debug)]
pub enum InstanceMessage {
    /// Apply new desired state.
    ApplyDesired {
        spec_revision: u64,
        spec: InstancePlan,
        desired_state: DesiredInstanceState,
    },

    /// Periodic tick for health checks and timeout handling.
    Tick { tick_id: u64 },

    /// Execute a command in the instance.
    ExecRequest {
        session_id: String,
        command: Vec<String>,
        grant_token: String,
    },

    /// Stop the instance.
    Stop { reason: StopReason },
}

/// Desired instance state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredInstanceState {
    Running,
    Draining,
    Stopped,
}

/// Reason for stopping an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    NodeShutdown,
    AdminKill,
    Eviction,
    ScaleDown,
    ReleaseUpdate,
}

// =============================================================================
// Actor State
// =============================================================================

/// Current phase of the instance lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstancePhase {
    /// Preparing resources (fetching image, creating directories).
    Preparing,
    /// Starting the Firecracker microVM.
    Booting,
    /// VM is running and healthy.
    Ready,
    /// Draining connections before shutdown.
    Draining,
    /// VM has stopped.
    Stopped,
    /// Instance has failed.
    Failed,
}

/// Persisted state for recovery.
#[derive(Debug, Clone)]
pub struct InstanceActorState {
    /// Instance ID.
    pub instance_id: String,

    /// Last applied spec revision.
    pub last_applied_spec_revision: u64,

    /// Current phase.
    pub phase: InstancePhase,

    /// Firecracker socket path.
    pub firecracker_socket_path: Option<String>,

    /// TAP device name.
    pub tap_device_name: Option<String>,

    /// Root disk path.
    pub root_disk_path: Option<String>,

    /// Overlay IP address.
    pub overlay_ip: Option<String>,

    /// Boot start time (for measuring boot duration).
    pub boot_started_at: Option<Instant>,

    /// Last health check time.
    pub last_health_check_at: Option<Instant>,

    /// Error message if failed.
    pub error_message: Option<String>,
}

impl InstanceActorState {
    /// Create initial state for a new instance.
    pub fn new(instance_id: String) -> Self {
        Self {
            instance_id,
            last_applied_spec_revision: 0,
            phase: InstancePhase::Preparing,
            firecracker_socket_path: None,
            tap_device_name: None,
            root_disk_path: None,
            overlay_ip: None,
            boot_started_at: None,
            last_health_check_at: None,
            error_message: None,
        }
    }
}

// =============================================================================
// Instance Actor
// =============================================================================

/// Actor managing a single microVM instance.
pub struct InstanceActor<R: Runtime + Send + Sync + 'static> {
    /// Instance ID (actor key).
    instance_id: String,

    /// Runtime for VM operations.
    runtime: std::sync::Arc<R>,

    /// Current actor state.
    state: InstanceActorState,

    /// VM handle if running.
    vm_handle: Option<VmHandle>,

    /// Current spec.
    current_spec: Option<InstancePlan>,
}

impl<R: Runtime + Send + Sync + 'static> InstanceActor<R> {
    /// Create a new instance actor.
    pub fn new(instance_id: String, runtime: std::sync::Arc<R>) -> Self {
        Self {
            instance_id: instance_id.clone(),
            runtime,
            state: InstanceActorState::new(instance_id),
            vm_handle: None,
            current_spec: None,
        }
    }

    /// Create from recovered state.
    pub fn from_state(state: InstanceActorState, runtime: std::sync::Arc<R>) -> Self {
        Self {
            instance_id: state.instance_id.clone(),
            runtime,
            state,
            vm_handle: None, // Will be recovered in on_start
            current_spec: None,
        }
    }

    /// Get current phase.
    pub fn phase(&self) -> InstancePhase {
        self.state.phase
    }

    /// Get current state for persistence.
    pub fn get_state(&self) -> &InstanceActorState {
        &self.state
    }

    // -------------------------------------------------------------------------
    // Message Handlers
    // -------------------------------------------------------------------------

    async fn handle_apply_desired(
        &mut self,
        spec_revision: u64,
        spec: InstancePlan,
        desired_state: DesiredInstanceState,
    ) -> Result<(), ActorError> {
        // Check if this is a newer revision
        if spec_revision <= self.state.last_applied_spec_revision {
            debug!(
                instance_id = %self.instance_id,
                spec_revision,
                last_revision = self.state.last_applied_spec_revision,
                "Ignoring stale spec revision"
            );
            return Ok(());
        }

        info!(
            instance_id = %self.instance_id,
            spec_revision,
            desired_state = ?desired_state,
            phase = ?self.state.phase,
            "Applying desired state"
        );

        match (self.state.phase, desired_state) {
            // Start from preparing/failed
            (InstancePhase::Preparing | InstancePhase::Failed, DesiredInstanceState::Running) => {
                self.start_instance(&spec).await?;
            }

            // Already running, check for spec changes
            (InstancePhase::Ready, DesiredInstanceState::Running) => {
                if self.needs_restart(&spec) {
                    info!(
                        instance_id = %self.instance_id,
                        "Spec changed, restarting instance"
                    );
                    self.stop_instance(StopReason::ReleaseUpdate).await?;
                    self.start_instance(&spec).await?;
                }
            }

            // Start draining
            (InstancePhase::Ready, DesiredInstanceState::Draining) => {
                self.start_draining().await?;
            }

            // Stop immediately
            (_, DesiredInstanceState::Stopped) => {
                self.stop_instance(StopReason::ScaleDown).await?;
            }

            // Already draining, wait for completion
            (InstancePhase::Draining, DesiredInstanceState::Draining) => {
                debug!(instance_id = %self.instance_id, "Already draining");
            }

            // Already stopped
            (InstancePhase::Stopped, _) => {
                debug!(instance_id = %self.instance_id, "Already stopped");
            }

            // Booting, wait for boot to complete
            (InstancePhase::Booting, DesiredInstanceState::Running) => {
                debug!(instance_id = %self.instance_id, "Still booting");
            }

            _ => {
                debug!(
                    instance_id = %self.instance_id,
                    phase = ?self.state.phase,
                    desired = ?desired_state,
                    "No action needed for state transition"
                );
            }
        }

        self.state.last_applied_spec_revision = spec_revision;
        self.current_spec = Some(spec);

        Ok(())
    }

    async fn handle_tick(&mut self, _tick_id: u64) -> Result<(), ActorError> {
        match self.state.phase {
            InstancePhase::Ready => {
                // Health check
                if let Some(handle) = &self.vm_handle {
                    match self.runtime.check_vm_health(handle).await {
                        Ok(true) => {
                            self.state.last_health_check_at = Some(Instant::now());
                        }
                        Ok(false) => {
                            warn!(instance_id = %self.instance_id, "Health check failed");
                            self.transition_to_failed("Health check failed".to_string());
                        }
                        Err(e) => {
                            warn!(
                                instance_id = %self.instance_id,
                                error = %e,
                                "Error during health check"
                            );
                        }
                    }
                }
            }

            InstancePhase::Draining => {
                // Check drain timeout
                // TODO: implement drain timeout handling
            }

            InstancePhase::Booting => {
                // Check boot timeout
                if let Some(started) = self.state.boot_started_at {
                    if started.elapsed() > std::time::Duration::from_secs(60) {
                        warn!(instance_id = %self.instance_id, "Boot timeout");
                        self.transition_to_failed("Boot timeout".to_string());
                    }
                }
            }

            _ => {}
        }

        Ok(())
    }

    async fn handle_stop(&mut self, reason: StopReason) -> Result<(), ActorError> {
        info!(
            instance_id = %self.instance_id,
            reason = ?reason,
            "Stop requested"
        );

        self.stop_instance(reason).await
    }

    // -------------------------------------------------------------------------
    // Internal Operations
    // -------------------------------------------------------------------------

    async fn start_instance(&mut self, spec: &InstancePlan) -> Result<(), ActorError> {
        info!(
            instance_id = %self.instance_id,
            image = %spec.image,
            "Starting instance"
        );

        self.state.phase = InstancePhase::Booting;
        self.state.boot_started_at = Some(Instant::now());

        // TODO: Prepare resources (image, directories, networking)
        // For now, go straight to starting the VM

        match self.runtime.start_vm(spec).await {
            Ok(handle) => {
                let boot_duration = self.state.boot_started_at.map(|t| t.elapsed());
                info!(
                    instance_id = %self.instance_id,
                    boot_id = %handle.boot_id,
                    boot_duration_ms = ?boot_duration.map(|d| d.as_millis()),
                    "Instance started successfully"
                );

                self.vm_handle = Some(handle);
                self.state.phase = InstancePhase::Ready;
                self.state.last_health_check_at = Some(Instant::now());

                Ok(())
            }
            Err(e) => {
                error!(
                    instance_id = %self.instance_id,
                    error = %e,
                    "Failed to start instance"
                );
                self.transition_to_failed(e.to_string());
                Err(ActorError::Transient(e.to_string()))
            }
        }
    }

    async fn stop_instance(&mut self, reason: StopReason) -> Result<(), ActorError> {
        if let Some(handle) = self.vm_handle.take() {
            info!(
                instance_id = %self.instance_id,
                boot_id = %handle.boot_id,
                reason = ?reason,
                "Stopping VM"
            );

            if let Err(e) = self.runtime.stop_vm(&handle).await {
                warn!(
                    instance_id = %self.instance_id,
                    error = %e,
                    "Error stopping VM"
                );
            }
        }

        self.state.phase = InstancePhase::Stopped;
        info!(instance_id = %self.instance_id, "Instance stopped");

        Ok(())
    }

    async fn start_draining(&mut self) -> Result<(), ActorError> {
        info!(instance_id = %self.instance_id, "Starting drain");
        self.state.phase = InstancePhase::Draining;

        // TODO: Notify load balancer to stop sending traffic
        // TODO: Wait for in-flight requests to complete
        // For now, just transition directly to stopped after a short delay

        Ok(())
    }

    fn needs_restart(&self, new_spec: &InstancePlan) -> bool {
        if let Some(current) = &self.current_spec {
            // Restart if image or release changed
            current.image != new_spec.image || current.release_id != new_spec.release_id
        } else {
            false
        }
    }

    fn transition_to_failed(&mut self, error_message: String) {
        self.state.phase = InstancePhase::Failed;
        self.state.error_message = Some(error_message);
        self.vm_handle = None;
    }
}

#[async_trait]
impl<R: Runtime + Send + Sync + 'static> Actor for InstanceActor<R> {
    type Message = InstanceMessage;

    fn name(&self) -> &str {
        "instance"
    }

    async fn handle(
        &mut self,
        msg: InstanceMessage,
        _ctx: &mut ActorContext,
    ) -> Result<bool, ActorError> {
        match msg {
            InstanceMessage::ApplyDesired {
                spec_revision,
                spec,
                desired_state,
            } => {
                self.handle_apply_desired(spec_revision, spec, desired_state)
                    .await?;
            }

            InstanceMessage::Tick { tick_id } => {
                self.handle_tick(tick_id).await?;
            }

            InstanceMessage::ExecRequest {
                session_id,
                command,
                grant_token: _,
            } => {
                // TODO: Forward to exec handler
                info!(
                    instance_id = %self.instance_id,
                    session_id = %session_id,
                    command = ?command,
                    "Exec request received (not implemented)"
                );
            }

            InstanceMessage::Stop { reason } => {
                self.handle_stop(reason).await?;
                return Ok(false); // Signal actor to stop
            }
        }

        Ok(true)
    }

    async fn on_start(&mut self, _ctx: &mut ActorContext) -> Result<(), ActorError> {
        info!(
            instance_id = %self.instance_id,
            phase = ?self.state.phase,
            "InstanceActor starting"
        );

        // Recovery: check if VM is still running
        if self.state.phase == InstancePhase::Ready || self.state.phase == InstancePhase::Booting {
            // TODO: Check Firecracker socket to see if VM is still running
            // For now, assume we need to restart
            info!(
                instance_id = %self.instance_id,
                "Recovering from previous state - would check VM status"
            );
        }

        Ok(())
    }

    async fn on_stop(&mut self, _ctx: &mut ActorContext) {
        info!(instance_id = %self.instance_id, "InstanceActor stopping");

        // Ensure VM is stopped
        if self.vm_handle.is_some() {
            let _ = self.stop_instance(StopReason::NodeShutdown).await;
        }
    }

    fn on_crash(&mut self, error: &ActorError) {
        warn!(
            instance_id = %self.instance_id,
            error = %error,
            "InstanceActor crashed"
        );
        self.state.error_message = Some(error.to_string());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_actor_state_new() {
        let state = InstanceActorState::new("inst_123".to_string());
        assert_eq!(state.instance_id, "inst_123");
        assert_eq!(state.phase, InstancePhase::Preparing);
        assert_eq!(state.last_applied_spec_revision, 0);
    }

    #[test]
    fn test_desired_instance_state() {
        assert_ne!(DesiredInstanceState::Running, DesiredInstanceState::Stopped);
        assert_eq!(DesiredInstanceState::Draining, DesiredInstanceState::Draining);
    }

    #[test]
    fn test_stop_reason() {
        let reason = StopReason::ScaleDown;
        assert_eq!(reason, StopReason::ScaleDown);
    }
}
