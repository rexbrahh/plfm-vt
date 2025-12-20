//! Node supervisor - root supervisor for node agent actors.
//!
//! Per `docs/architecture/07-actors-and-supervision.md`, the NodeSupervisor:
//! - Is the root of the supervision tree
//! - Spawns and supervises all child actors
//! - Handles graceful shutdown
//!
//! ## Supervision Tree
//!
//! ```text
//! NodeSupervisor
//! ├── ControlPlaneStreamActor
//! ├── InstanceSupervisor (dynamic children)
//! │   └── InstanceActor(instance_id)
//! ├── ImageCacheSupervisor
//! │   └── ImagePullActor
//! └── [Future: VolumeSupervisor, OverlaySupervisor, etc.]
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use super::framework::{ActorHandle, RestartPolicy, Supervisor};
use super::image::{ImageMessage, ImagePullActor};
use super::instance::{DesiredInstanceState, InstanceActor, InstanceMessage};
use super::stream::{ControlPlaneStreamActor, StreamMessage};
use crate::client::InstancePlan;
use crate::config::Config;
use crate::runtime::Runtime;

// =============================================================================
// Node Supervisor
// =============================================================================

/// Root supervisor for the node agent.
pub struct NodeSupervisor<R: Runtime + Send + Sync + 'static> {
    /// Node configuration.
    config: Config,

    /// Runtime for VM operations.
    runtime: Arc<R>,

    /// Core supervisor for static actors.
    supervisor: Supervisor,

    /// Handle to the control plane stream actor.
    stream_handle: Option<ActorHandle<StreamMessage>>,

    /// Handle to the image pull actor.
    image_handle: Option<ActorHandle<ImageMessage>>,

    /// Instance actors by instance ID.
    instance_handles: HashMap<String, ActorHandle<InstanceMessage>>,

    /// Shutdown signal receiver.
    shutdown: watch::Receiver<bool>,

    /// Spec revision counter for message coalescing.
    spec_revision: u64,
}

impl<R: Runtime + Send + Sync + 'static> NodeSupervisor<R> {
    /// Create a new node supervisor.
    pub fn new(config: Config, runtime: Arc<R>, shutdown: watch::Receiver<bool>) -> Self {
        let supervisor = Supervisor::new(RestartPolicy::default(), shutdown.clone());

        Self {
            config,
            runtime,
            supervisor,
            stream_handle: None,
            image_handle: None,
            instance_handles: HashMap::new(),
            shutdown,
            spec_revision: 0,
        }
    }

    /// Start all static actors.
    pub fn start(&mut self) {
        info!(
            node_id = %self.config.node_id,
            "Starting node supervisor"
        );

        // Start control plane stream actor
        let stream_actor = ControlPlaneStreamActor::new(
            self.config.node_id.to_string(),
            self.config.control_plane_url.clone(),
        );
        self.stream_handle = Some(self.supervisor.spawn(stream_actor, 256));

        // Start image pull actor
        let image_actor = ImagePullActor::new(
            format!("{}/images", self.config.data_dir),
            10 * 1024 * 1024 * 1024, // 10 GB cache limit
        );
        self.image_handle = Some(self.supervisor.spawn(image_actor, 64));

        info!(
            running = self.supervisor.running_count(),
            "Static actors started"
        );
    }

    /// Apply a new set of desired instances.
    ///
    /// This is the main entry point for reconciliation - it compares desired
    /// vs current instances and spawns/stops actors as needed.
    pub async fn apply_instances(&mut self, desired: Vec<InstancePlan>) {
        self.spec_revision += 1;
        let revision = self.spec_revision;

        info!(
            revision,
            desired_count = desired.len(),
            current_count = self.instance_handles.len(),
            "Applying desired instances"
        );

        // Build set of desired instance IDs
        let desired_ids: std::collections::HashSet<_> =
            desired.iter().map(|p| p.instance_id.clone()).collect();

        // Find instances to stop
        let to_stop: Vec<String> = self
            .instance_handles
            .keys()
            .filter(|id| !desired_ids.contains(*id))
            .cloned()
            .collect();

        // Stop instances no longer desired
        for instance_id in to_stop {
            self.stop_instance(&instance_id).await;
        }

        // Ensure each desired instance exists
        for plan in desired {
            self.ensure_instance(plan, revision).await;
        }

        debug!(
            running_instances = self.instance_handles.len(),
            "Instance reconciliation complete"
        );
    }

    /// Ensure an instance actor exists and has the correct spec.
    async fn ensure_instance(&mut self, plan: InstancePlan, revision: u64) {
        let instance_id = plan.instance_id.clone();

        if let Some(handle) = self.instance_handles.get(&instance_id) {
            // Actor exists, send updated spec
            let msg = InstanceMessage::ApplyDesired {
                spec_revision: revision,
                spec: Box::new(plan.clone()),
                desired_state: DesiredInstanceState::Running,
            };

            if let Err(e) = handle.send(msg).await {
                warn!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to send spec update to instance actor"
                );
                // Remove dead actor
                self.instance_handles.remove(&instance_id);
                // Respawn
                self.spawn_instance(plan, revision);
            }
        } else {
            // Spawn new actor
            self.spawn_instance(plan, revision);
        }
    }

    /// Spawn a new instance actor.
    fn spawn_instance(&mut self, plan: InstancePlan, revision: u64) {
        let instance_id = plan.instance_id.clone();

        info!(instance_id = %instance_id, "Spawning instance actor");

        let actor = InstanceActor::new(instance_id.clone(), Arc::clone(&self.runtime));
        let handle = self.supervisor.spawn(actor, 16);

        // Send initial spec
        let msg = InstanceMessage::ApplyDesired {
            spec_revision: revision,
            spec: Box::new(plan),
            desired_state: DesiredInstanceState::Running,
        };

        // Use try_send since we just spawned
        if let Err(e) = handle.try_send(msg) {
            error!(
                instance_id = %instance_id,
                error = %e,
                "Failed to send initial spec to instance actor"
            );
        }

        self.instance_handles.insert(instance_id, handle);
    }

    /// Stop an instance actor.
    async fn stop_instance(&mut self, instance_id: &str) {
        if let Some(handle) = self.instance_handles.remove(instance_id) {
            info!(instance_id = %instance_id, "Stopping instance actor");

            let msg = InstanceMessage::Stop {
                reason: super::instance::StopReason::ScaleDown,
            };

            if let Err(e) = handle.send(msg).await {
                warn!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to send stop message to instance actor"
                );
            }
        }
    }

    /// Run the supervisor loop until shutdown.
    pub async fn run(&mut self) {
        info!("Node supervisor entering main loop");

        let mut check_interval = tokio::time::interval(Duration::from_secs(5));
        let mut tick_id = 0u64;

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.changed() => {
                    if *self.shutdown.borrow() {
                        info!("Node supervisor received shutdown signal");
                        break;
                    }
                }

                _ = check_interval.tick() => {
                    tick_id += 1;

                    // Check and restart any crashed actors
                    self.supervisor.check_and_restart().await;

                    // Send tick to all instance actors
                    for (instance_id, handle) in &self.instance_handles {
                        let msg = InstanceMessage::Tick { tick_id };
                        if let Err(e) = handle.try_send(msg) {
                            debug!(
                                instance_id = %instance_id,
                                error = %e,
                                "Failed to send tick to instance actor"
                            );
                        }
                    }

                    // Send heartbeat tick to stream actor
                    if let Some(handle) = &self.stream_handle {
                        let msg = StreamMessage::SendHeartbeat { tick_id };
                        let _ = handle.try_send(msg);
                    }

                    // Send GC tick to image actor
                    if let Some(handle) = &self.image_handle {
                        let msg = ImageMessage::GCCheck { tick_id };
                        let _ = handle.try_send(msg);
                    }

                    debug!(
                        tick_id,
                        running_actors = self.supervisor.running_count(),
                        degraded_actors = self.supervisor.degraded_count(),
                        instances = self.instance_handles.len(),
                        "Supervisor tick"
                    );
                }
            }
        }

        self.shutdown().await;
    }

    /// Gracefully shut down all actors.
    async fn shutdown(&mut self) {
        info!(
            instances = self.instance_handles.len(),
            "Shutting down node supervisor"
        );

        // Stop all instance actors first
        let instance_ids: Vec<_> = self.instance_handles.keys().cloned().collect();
        for instance_id in instance_ids {
            self.stop_instance(&instance_id).await;
        }

        // Give instances time to stop gracefully
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Stop all actors
        self.supervisor.stop_all().await;

        info!("Node supervisor shutdown complete");
    }

    /// Get the number of running instance actors.
    pub fn instance_count(&self) -> usize {
        self.instance_handles.len()
    }

    /// Get the handle to the image pull actor.
    pub fn image_handle(&self) -> Option<&ActorHandle<ImageMessage>> {
        self.image_handle.as_ref()
    }

    /// Get the handle to the stream actor.
    pub fn stream_handle(&self) -> Option<&ActorHandle<StreamMessage>> {
        self.stream_handle.as_ref()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::InstanceResources;
    use crate::runtime::MockRuntime;
    use plfm_id::NodeId;

    fn test_config() -> Config {
        Config {
            node_id: NodeId::new(),
            control_plane_url: "http://localhost:8080".to_string(),
            data_dir: "/tmp/test".to_string(),
            heartbeat_interval_secs: 30,
            log_level: "info".to_string(),
        }
    }

    fn test_plan(id: &str) -> InstancePlan {
        InstancePlan {
            instance_id: id.to_string(),
            app_id: "app_test".to_string(),
            env_id: "env_test".to_string(),
            process_type: "web".to_string(),
            release_id: "rel_test".to_string(),
            deploy_id: "dep_test".to_string(),
            image: "test:latest".to_string(),
            resources: InstanceResources {
                cpu: 1.0,
                memory_bytes: 512 * 1024 * 1024,
            },
            overlay_ipv6: "fd00::1".to_string(),
            secrets_version_id: None,
            env_vars: serde_json::json!({}),
            volumes: vec![],
        }
    }

    #[tokio::test]
    async fn test_node_supervisor_new() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);

        let supervisor = NodeSupervisor::new(config, runtime, shutdown_rx);
        assert_eq!(supervisor.instance_count(), 0);
    }

    #[tokio::test]
    async fn test_node_supervisor_start() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);

        let mut supervisor = NodeSupervisor::new(config, runtime, shutdown_rx);
        supervisor.start();

        assert!(supervisor.stream_handle().is_some());
        assert!(supervisor.image_handle().is_some());
    }

    #[tokio::test]
    async fn test_node_supervisor_apply_instances() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);

        let mut supervisor = NodeSupervisor::new(config, runtime, shutdown_rx);
        supervisor.start();

        // Apply some instances
        let plans = vec![test_plan("inst_1"), test_plan("inst_2")];
        supervisor.apply_instances(plans).await;

        assert_eq!(supervisor.instance_count(), 2);

        // Scale down to one
        let plans = vec![test_plan("inst_1")];
        supervisor.apply_instances(plans).await;

        // Give actors time to process
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(supervisor.instance_count(), 1);
    }
}
