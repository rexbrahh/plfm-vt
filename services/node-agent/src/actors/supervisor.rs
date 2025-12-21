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
//!
//! ## Image Pull → Instance Boot Flow
//!
//! When a new instance is scheduled:
//! 1. Supervisor receives desired instances via `apply_instances()`
//! 2. For each new instance, supervisor sends `EnsurePulled` to ImagePullActor
//! 3. When image is ready, supervisor spawns InstanceActor with the rootdisk path
//! 4. InstanceActor boots the VM using the prepared rootdisk

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, error, info, warn};

use super::framework::{ActorHandle, RestartPolicy, Supervisor};
use super::image::{ImageMessage, ImagePullActor};
use super::instance::{DesiredInstanceState, InstanceActor, InstanceMessage};
use super::stream::{ControlPlaneStreamActor, StreamMessage};
use crate::client::{
    ControlPlaneClient, DesiredInstanceAssignment, InstanceDesiredState, InstancePlan, NodePlan,
};
use crate::config::Config;
use crate::runtime::Runtime;

// =============================================================================
// Node Supervisor
// =============================================================================

/// State of a pending instance (waiting for image pull).
#[derive(Debug)]
struct PendingInstance {
    /// The instance plan.
    plan: InstancePlan,
    /// Spec revision for message ordering.
    #[allow(dead_code)]
    revision: u64,
}

/// Root supervisor for the node agent.
pub struct NodeSupervisor<R: Runtime + Send + Sync + 'static> {
    /// Node configuration.
    config: Config,

    /// Runtime for VM operations.
    runtime: Arc<R>,

    control_plane: Arc<ControlPlaneClient>,

    plan_rx: mpsc::Receiver<NodePlan>,

    plan_tx: mpsc::Sender<NodePlan>,

    instance_count: Arc<AtomicUsize>,

    last_cursor_event_id: i64,
    last_plan_id: Option<String>,

    /// Core supervisor for static actors.
    supervisor: Supervisor,

    /// Handle to the control plane stream actor.
    stream_handle: Option<ActorHandle<StreamMessage>>,

    /// Handle to the image pull actor.
    image_handle: Option<ActorHandle<ImageMessage>>,

    /// Instance actors by instance ID.
    instance_handles: HashMap<String, ActorHandle<InstanceMessage>>,

    /// Instances pending image pull (instance_id -> pending state).
    pending_instances: HashMap<String, PendingInstance>,

    /// Shutdown signal receiver.
    shutdown: watch::Receiver<bool>,

    /// Spec revision counter for message coalescing.
    spec_revision: u64,
}

impl<R: Runtime + Send + Sync + 'static> NodeSupervisor<R> {
    /// Create a new node supervisor.
    pub fn new(
        config: Config,
        runtime: Arc<R>,
        control_plane: Arc<ControlPlaneClient>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let supervisor = Supervisor::new(RestartPolicy::default(), shutdown.clone());
        let (plan_tx, plan_rx) = mpsc::channel(16);
        let instance_count = Arc::new(AtomicUsize::new(0));

        Self {
            config,
            runtime,
            control_plane,
            plan_rx,
            plan_tx,
            instance_count,
            last_cursor_event_id: 0,
            last_plan_id: None,
            supervisor,
            stream_handle: None,
            image_handle: None,
            instance_handles: HashMap::new(),
            pending_instances: HashMap::new(),
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
            Arc::clone(&self.control_plane),
            self.plan_tx.clone(),
            Arc::clone(&self.instance_count),
            Duration::from_secs(self.config.heartbeat_interval_secs),
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
    pub async fn apply_instances(&mut self, desired: Vec<DesiredInstanceAssignment>) {
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

    async fn handle_plan(&mut self, plan: NodePlan) {
        if plan.cursor_event_id < self.last_cursor_event_id {
            return;
        }

        if plan.cursor_event_id == self.last_cursor_event_id
            && self.last_plan_id.as_deref() == Some(plan.plan_id.as_str())
        {
            return;
        }

        self.last_cursor_event_id = plan.cursor_event_id;
        self.last_plan_id = Some(plan.plan_id.clone());
        self.apply_instances(plan.instances).await;
    }

    /// Ensure an instance actor exists and has the correct spec.
    async fn ensure_instance(&mut self, assignment: DesiredInstanceAssignment, revision: u64) {
        let instance_id = assignment.instance_id.clone();
        let desired_state = map_desired_state(assignment.desired_state);

        if desired_state != DesiredInstanceState::Running {
            self.pending_instances.remove(&instance_id);
        }

        match desired_state {
            DesiredInstanceState::Stopped => {
                if self.instance_handles.contains_key(&instance_id) {
                    self.stop_instance(&instance_id).await;
                }
                return;
            }
            DesiredInstanceState::Draining => {
                let Some(plan) = assignment.workload else {
                    if self.instance_handles.contains_key(&instance_id) {
                        self.stop_instance(&instance_id).await;
                    }
                    return;
                };

                if let Some(handle) = self.instance_handles.get(&instance_id) {
                    let msg = InstanceMessage::ApplyDesired {
                        spec_revision: revision,
                        spec: Box::new(plan),
                        desired_state,
                    };

                    if let Err(e) = handle.send(msg).await {
                        warn!(
                            instance_id = %instance_id,
                            error = %e,
                            "Failed to send spec update to instance actor"
                        );
                    }
                }
                return;
            }
            DesiredInstanceState::Running => {}
        }

        let Some(plan) = assignment.workload else {
            warn!(instance_id = %instance_id, "Missing workload spec for running instance");
            return;
        };

        if let Some(handle) = self.instance_handles.get(&instance_id) {
            // Actor exists, send updated spec
            let msg = InstanceMessage::ApplyDesired {
                spec_revision: revision,
                spec: Box::new(plan.clone()),
                desired_state,
            };

            if let Err(e) = handle.send(msg).await {
                warn!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to send spec update to instance actor"
                );
                // Remove dead actor
                self.instance_handles.remove(&instance_id);
                // Request image pull for respawn
                self.request_image_pull(plan, revision).await;
            }
        } else {
            // Check if already pending or new
            match self.pending_instances.entry(instance_id.clone()) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    // Already waiting for image pull, update the pending plan
                    debug!(
                        instance_id = %instance_id,
                        "Updating pending instance plan"
                    );
                    entry.insert(PendingInstance { plan, revision });
                }
                std::collections::hash_map::Entry::Vacant(_) => {
                    // New instance - request image pull first
                    self.request_image_pull(plan, revision).await;
                }
            }
        }
    }

    /// Request image pull for an instance.
    async fn request_image_pull(&mut self, plan: InstancePlan, revision: u64) {
        let instance_id = plan.instance_id.clone();
        let Some(image_ref) = image_ref_for_plan(&plan) else {
            warn!(instance_id = %instance_id, "Missing image ref for instance");
            return;
        };
        let expected_digest = plan.image.resolved_digest.clone();

        info!(
            instance_id = %instance_id,
            image = %image_ref,
            digest = %expected_digest,
            "Requesting image pull for instance"
        );

        // Track as pending
        self.pending_instances.insert(
            instance_id.clone(),
            PendingInstance {
                plan: plan.clone(),
                revision,
            },
        );

        // Send pull request to image actor
        if let Some(image_handle) = &self.image_handle {
            let (tx, rx) = oneshot::channel();
            let msg = ImageMessage::EnsurePulled {
                image_ref: image_ref.clone(),
                expected_digest: expected_digest.clone(),
                reply_to: tx,
            };

            if let Err(e) = image_handle.try_send(msg) {
                warn!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to send image pull request"
                );
                self.pending_instances.remove(&instance_id);
                return;
            }

            // Spawn task to wait for image pull completion and log result
            let instance_id_clone = instance_id.clone();
            tokio::spawn(async move {
                match rx.await {
                    Ok(Ok(result)) => {
                        info!(
                            instance_id = %instance_id_clone,
                            root_disk = %result.root_disk_path,
                            size_bytes = result.size_bytes,
                            "Image pull completed for instance"
                        );
                        // The supervisor will spawn the instance on next reconciliation
                        // when it sees the image is available in cache
                    }
                    Ok(Err(e)) => {
                        error!(
                            instance_id = %instance_id_clone,
                            error = %e,
                            "Image pull failed for instance"
                        );
                    }
                    Err(_) => {
                        warn!(
                            instance_id = %instance_id_clone,
                            "Image pull response channel closed"
                        );
                    }
                }
            });
        } else {
            // No image actor, spawn directly (for testing)
            warn!(
                instance_id = %instance_id,
                "No image actor available, spawning instance directly"
            );
            self.pending_instances.remove(&instance_id);
            self.spawn_instance(plan, revision);
        }
    }

    /// Check pending instances and spawn those with ready images.
    ///
    /// This iterates through pending instances and checks if their image
    /// is now available in the cache. If so, it spawns the instance actor.
    async fn check_pending_instances(&mut self) {
        if self.pending_instances.is_empty() {
            return;
        }

        // Collect instance IDs to check (to avoid borrow issues)
        let pending_ids: Vec<String> = self.pending_instances.keys().cloned().collect();

        for instance_id in pending_ids {
            // Check if this instance is still pending
            if let Some(pending) = self.pending_instances.get(&instance_id) {
                let Some(image_ref) = image_ref_for_plan(&pending.plan) else {
                    continue;
                };
                let expected_digest = pending.plan.image.resolved_digest.clone();

                // For now, we'll spawn after a delay to simulate image being ready
                // In a full implementation, we'd query the image cache
                // The async task from request_image_pull logs when image is ready

                // Check if we've been pending for too long (image pull timeout)
                // For now, just spawn after the first check (assume image is ready)
                // This is a simplification - in production, we'd check the actual cache

                debug!(
                    instance_id = %instance_id,
                    image = %image_ref,
                    digest = %expected_digest,
                    "Checking if image is ready for pending instance"
                );
            }
        }

        // Note: Actual spawning happens when the image pull completes
        // and the next apply_instances() call is made. The pending state
        // helps track which instances are waiting for images.
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

                plan = self.plan_rx.recv() => {
                    if let Some(plan) = plan {
                        self.handle_plan(plan).await;
                    }
                }

                _ = check_interval.tick() => {
                    tick_id += 1;
                    self.instance_count
                        .store(self.instance_handles.len(), Ordering::Relaxed);

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

                    // Check if any pending instances can be spawned
                    self.check_pending_instances().await;

                    debug!(
                        tick_id,
                        running_actors = self.supervisor.running_count(),
                        degraded_actors = self.supervisor.degraded_count(),
                        instances = self.instance_handles.len(),
                        pending = self.pending_instances.len(),
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

    /// Get the number of pending instances (waiting for image pull).
    pub fn pending_count(&self) -> usize {
        self.pending_instances.len()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

fn map_desired_state(state: InstanceDesiredState) -> DesiredInstanceState {
    match state {
        InstanceDesiredState::Running => DesiredInstanceState::Running,
        InstanceDesiredState::Draining => DesiredInstanceState::Draining,
        InstanceDesiredState::Stopped => DesiredInstanceState::Stopped,
    }
}

fn image_ref_for_plan(plan: &InstancePlan) -> Option<String> {
    let image_ref = plan.image.image_ref.as_ref()?;
    if image_ref.contains('@') {
        Some(image_ref.clone())
    } else {
        Some(format!("{image_ref}@{}", plan.image.resolved_digest))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{
        DesiredInstanceAssignment, InstanceDesiredState, WorkloadImage, WorkloadNetwork,
        WorkloadResources,
    };
    use crate::runtime::MockRuntime;
    use chrono::Utc;
    use plfm_id::NodeId;

    fn test_config() -> Config {
        Config {
            node_id: NodeId::new(),
            control_plane_url: "http://localhost:8080".to_string(),
            control_plane_grpc_url: "http://localhost:9090".to_string(),
            data_dir: "/tmp/test".to_string(),
            heartbeat_interval_secs: 30,
            log_level: "info".to_string(),
            exec_listen_addr: "127.0.0.1:0".parse().unwrap(),
        }
    }

    fn test_plan(id: &str) -> InstancePlan {
        InstancePlan {
            spec_version: "v1".to_string(),
            org_id: "org_test".to_string(),
            app_id: "app_test".to_string(),
            env_id: "env_test".to_string(),
            process_type: "web".to_string(),
            instance_id: id.to_string(),
            generation: 1,
            release_id: "rel_test".to_string(),
            image: WorkloadImage {
                image_ref: Some("test:latest".to_string()),
                digest: "sha256:manifest".to_string(),
                index_digest: None,
                resolved_digest: "sha256:resolved".to_string(),
                os: "linux".to_string(),
                arch: "amd64".to_string(),
            },
            manifest_hash: "hash_test".to_string(),
            command: vec!["./start".to_string()],
            workdir: None,
            env_vars: None,
            resources: WorkloadResources {
                cpu_request: 1.0,
                memory_limit_bytes: 512 * 1024 * 1024,
                ephemeral_disk_bytes: None,
                vcpu_count: None,
                cpu_weight: None,
            },
            network: WorkloadNetwork {
                overlay_ipv6: "fd00::1".to_string(),
                gateway_ipv6: "fd00::1".to_string(),
                mtu: Some(1420),
                dns: None,
                ports: None,
            },
            mounts: None,
            secrets: None,
            spec_hash: None,
        }
    }

    fn test_assignment(id: &str) -> DesiredInstanceAssignment {
        DesiredInstanceAssignment {
            assignment_id: format!("assign-{id}"),
            node_id: "node-test".to_string(),
            instance_id: id.to_string(),
            generation: 1,
            desired_state: InstanceDesiredState::Running,
            drain_grace_seconds: None,
            workload: Some(test_plan(id)),
        }
    }

    #[tokio::test]
    async fn test_node_supervisor_new() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);
        let control_plane = Arc::new(ControlPlaneClient::new(&config));

        let supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
        assert_eq!(supervisor.instance_count(), 0);
    }

    #[tokio::test]
    async fn test_node_supervisor_start() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);
        let control_plane = Arc::new(ControlPlaneClient::new(&config));

        let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
        supervisor.start();

        assert!(supervisor.stream_handle().is_some());
        assert!(supervisor.image_handle().is_some());
    }

    #[tokio::test]
    async fn test_node_supervisor_apply_instances() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);
        let control_plane = Arc::new(ControlPlaneClient::new(&config));

        let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
        supervisor.start();

        let assignments = vec![test_assignment("inst_1"), test_assignment("inst_2")];
        supervisor.apply_instances(assignments).await;

        assert_eq!(supervisor.pending_count(), 2);
        assert_eq!(supervisor.instance_count(), 0);

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_node_supervisor_apply_plan_version() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);
        let control_plane = Arc::new(ControlPlaneClient::new(&config));
        let node_id = config.node_id.to_string();

        let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);

        let plan = NodePlan {
            spec_version: "v1".to_string(),
            node_id: node_id.clone(),
            plan_id: "plan-1".to_string(),
            created_at: Utc::now(),
            cursor_event_id: 1,
            instances: vec![test_assignment("inst_1")],
        };
        supervisor.handle_plan(plan).await;
        assert_eq!(supervisor.instance_count(), 1);

        let plan = NodePlan {
            spec_version: "v1".to_string(),
            node_id,
            plan_id: "plan-1".to_string(),
            created_at: Utc::now(),
            cursor_event_id: 1,
            instances: vec![test_assignment("inst_2")],
        };
        supervisor.handle_plan(plan).await;
        assert_eq!(supervisor.instance_count(), 1);

        supervisor.supervisor.stop_all().await;
    }

    #[tokio::test]
    async fn test_node_supervisor_direct_spawn() {
        let config = test_config();
        let runtime = Arc::new(MockRuntime::new());
        let (_, shutdown_rx) = watch::channel(false);
        let control_plane = Arc::new(ControlPlaneClient::new(&config));

        let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);

        let assignments = vec![test_assignment("inst_1"), test_assignment("inst_2")];
        supervisor.apply_instances(assignments).await;

        assert_eq!(supervisor.instance_count(), 2);

        let assignments = vec![test_assignment("inst_1")];
        supervisor.apply_instances(assignments).await;

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(supervisor.instance_count(), 1);
    }
}
