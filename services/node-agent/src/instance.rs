//! Instance manager for tracking and managing local instances.
//!
//! The instance manager:
//! - Tracks desired state (from plan) vs actual state (from VM runtime)
//! - Triggers VM lifecycle operations to converge state
//! - Reports status changes back to the control plane

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::client::{ControlPlaneClient, InstancePlan, InstanceStatus, InstanceStatusReport};
use crate::runtime::{Runtime, VmHandle};
use crate::vsock::{ConfigStore, PendingConfig};

/// Tracks a single instance's state.
#[derive(Debug, Clone)]
pub struct InstanceState {
    /// The plan for this instance.
    pub plan: InstancePlan,

    /// Current status.
    pub status: InstanceStatus,

    /// Boot ID (if running).
    pub boot_id: Option<String>,

    /// VM handle (if running).
    pub vm_handle: Option<VmHandle>,

    /// Error message (if failed).
    pub error_message: Option<String>,

    /// Exit code (if stopped).
    pub exit_code: Option<i32>,
}

impl InstanceState {
    /// Create a new instance state from a plan.
    pub fn from_plan(plan: InstancePlan) -> Self {
        Self {
            plan,
            status: InstanceStatus::Booting,
            boot_id: None,
            vm_handle: None,
            error_message: None,
            exit_code: None,
        }
    }

    /// Convert to a status report.
    pub fn to_status_report(&self) -> InstanceStatusReport {
        InstanceStatusReport {
            instance_id: self.plan.instance_id.clone(),
            status: self.status,
            boot_id: self.boot_id.clone(),
            error_message: self.error_message.clone(),
            exit_code: self.exit_code,
        }
    }
}

/// Instance manager.
pub struct InstanceManager {
    /// Runtime for VM lifecycle operations.
    runtime: Arc<dyn Runtime>,

    /// Current instances by instance_id.
    instances: RwLock<HashMap<String, InstanceState>>,

    /// Last applied plan version.
    last_plan_version: RwLock<i64>,

    /// Node overlay IPv6 address (/128).
    node_overlay_ipv6: RwLock<Option<String>>,

    /// Config store for guest-init handshake.
    config_store: Arc<ConfigStore>,

    /// Control plane client (for secrets/logs).
    control_plane: Arc<ControlPlaneClient>,

    /// Config generation counter.
    config_generation: AtomicU64,
}

impl InstanceManager {
    /// Create a new instance manager.
    pub fn new(
        runtime: Arc<dyn Runtime>,
        config_store: Arc<ConfigStore>,
        control_plane: Arc<ControlPlaneClient>,
    ) -> Self {
        Self {
            runtime,
            instances: RwLock::new(HashMap::new()),
            last_plan_version: RwLock::new(0),
            node_overlay_ipv6: RwLock::new(None),
            config_store,
            control_plane,
            config_generation: AtomicU64::new(1),
        }
    }

    /// Get the current instance count.
    pub async fn instance_count(&self) -> i32 {
        let instances = self.instances.read().await;
        instances
            .values()
            .filter(|i| matches!(i.status, InstanceStatus::Booting | InstanceStatus::Ready))
            .count() as i32
    }

    /// Get the last applied plan version.
    pub async fn last_plan_version(&self) -> i64 {
        *self.last_plan_version.read().await
    }

    pub async fn set_node_overlay_ipv6(&self, overlay_ipv6: Option<String>) {
        let mut current = self.node_overlay_ipv6.write().await;
        if *current == overlay_ipv6 {
            return;
        }
        *current = overlay_ipv6.clone();
        match overlay_ipv6 {
            Some(value) => info!(node_overlay_ipv6 = %value, "Updated node overlay IPv6"),
            None => info!("Cleared node overlay IPv6"),
        }
    }

    /// Apply a new plan, converging the local state to match.
    pub async fn apply_plan(&self, plan_version: i64, desired_instances: Vec<InstancePlan>) {
        let last_version = *self.last_plan_version.read().await;
        if plan_version <= last_version {
            debug!(
                plan_version,
                last_version, "Plan version not newer, skipping"
            );
            return;
        }

        info!(
            plan_version,
            instance_count = desired_instances.len(),
            "Applying new plan"
        );

        let desired_ids: std::collections::HashSet<_> = desired_instances
            .iter()
            .map(|i| i.instance_id.clone())
            .collect();

        // Find instances to stop (in current state but not in desired)
        let instances_to_stop: Vec<String> = {
            let instances = self.instances.read().await;
            instances
                .keys()
                .filter(|id| !desired_ids.contains(*id))
                .cloned()
                .collect()
        };

        // Stop instances that are no longer desired
        for instance_id in instances_to_stop {
            self.stop_instance(&instance_id).await;
        }

        // Start or update instances
        for plan in desired_instances {
            self.ensure_instance(plan).await;
        }

        // Update plan version
        *self.last_plan_version.write().await = plan_version;
    }

    /// Ensure an instance is running with the given plan.
    async fn ensure_instance(&self, plan: InstancePlan) {
        let instance_id = plan.instance_id.clone();

        // Check if instance already exists
        let existing = {
            let instances = self.instances.read().await;
            instances.get(&instance_id).cloned()
        };

        match existing {
            Some(existing) => {
                // Instance exists - check if it needs updating
                if existing.plan.release_id != plan.release_id || existing.plan.image != plan.image
                {
                    info!(
                        instance_id = %instance_id,
                        old_release = %existing.plan.release_id,
                        new_release = %plan.release_id,
                        "Instance needs update, recreating"
                    );
                    self.stop_instance(&instance_id).await;
                    self.start_instance(plan).await;
                } else {
                    debug!(instance_id = %instance_id, "Instance already running with correct config");
                }
            }
            None => {
                // New instance
                self.start_instance(plan).await;
            }
        }
    }

    /// Start a new instance.
    async fn start_instance(&self, plan: InstancePlan) {
        let instance_id = plan.instance_id.clone();
        let env_var_count = plan.env_vars.as_object().map(|m| m.len()).unwrap_or(0);
        let volume_count = plan.volumes.len();
        let read_only_volume_count = plan.volumes.iter().filter(|v| v.read_only).count();
        let non_empty_volume_ids = plan
            .volumes
            .iter()
            .filter(|v| !v.volume_id.is_empty())
            .count();
        let total_mount_path_chars: usize = plan.volumes.iter().map(|v| v.mount_path.len()).sum();

        info!(
            instance_id = %instance_id,
            app_id = %plan.app_id,
            env_id = %plan.env_id,
            deploy_id = %plan.deploy_id,
            image = %plan.image,
            env_var_count,
            volume_count,
            read_only_volume_count,
            non_empty_volume_ids,
            total_mount_path_chars,
            "Starting instance"
        );

        // Create initial state
        let mut state = InstanceState::from_plan(plan.clone());

        let secrets_data = match plan.secrets_version_id.as_deref() {
            Some(version_id) => match self.control_plane.fetch_secret_material(version_id).await {
                Ok(payload) => Some(payload.data),
                Err(e) => {
                    state.status = InstanceStatus::Failed;
                    state.error_message = Some(format!("Failed to fetch secrets: {e}"));
                    error!(instance_id = %instance_id, error = %e, "Failed to fetch secrets");
                    let mut instances = self.instances.write().await;
                    instances.insert(instance_id, state);
                    return;
                }
            },
            None => None,
        };

        let generation = self.config_generation.fetch_add(1, Ordering::SeqCst);
        let overlay_ipv6 = if plan.overlay_ipv6.is_empty() {
            "fd00::1".to_string()
        } else {
            plan.overlay_ipv6.clone()
        };

        let pending = PendingConfig {
            plan: plan.clone(),
            overlay_ipv6,
            gateway_ipv6: "fe80::1".to_string(),
            generation,
            secrets_data,
        };

        self.config_store.add(&instance_id, pending).await;

        // Try to start the VM
        match self.runtime.start_vm(&plan).await {
            Ok(handle) => {
                state.status = InstanceStatus::Ready;
                state.boot_id = Some(handle.boot_id.clone());
                state.vm_handle = Some(handle);
                info!(instance_id = %instance_id, "Instance started successfully");
            }
            Err(e) => {
                state.status = InstanceStatus::Failed;
                state.error_message = Some(e.to_string());
                error!(instance_id = %instance_id, error = %e, "Failed to start instance");
                self.config_store.remove(&instance_id).await;
            }
        }

        // Store state
        let mut instances = self.instances.write().await;
        instances.insert(instance_id, state);
    }

    /// Stop an instance.
    async fn stop_instance(&self, instance_id: &str) {
        info!(instance_id = %instance_id, "Stopping instance");

        // Get the current state
        let state = {
            let instances = self.instances.read().await;
            instances.get(instance_id).cloned()
        };

        if let Some(mut state) = state {
            // Update status to draining
            state.status = InstanceStatus::Draining;
            {
                let mut instances = self.instances.write().await;
                instances.insert(instance_id.to_string(), state.clone());
            }

            // Stop the VM if it has a handle
            if let Some(handle) = &state.vm_handle {
                if let Err(e) = self.runtime.stop_vm(handle).await {
                    warn!(instance_id = %instance_id, error = %e, "Error stopping VM");
                }
            }

            // Update to stopped
            state.status = InstanceStatus::Stopped;
            state.vm_handle = None;

            let mut instances = self.instances.write().await;
            instances.insert(instance_id.to_string(), state);

            self.config_store.remove(instance_id).await;

            info!(instance_id = %instance_id, "Instance stopped");
        }
    }

    /// Get status reports for all instances.
    pub async fn get_status_reports(&self) -> Vec<InstanceStatusReport> {
        let instances = self.instances.read().await;
        instances.values().map(|i| i.to_status_report()).collect()
    }

    /// Get the guest CID for a running instance.
    pub async fn guest_cid_for_instance(&self, instance_id: &str) -> Option<u32> {
        let instances = self.instances.read().await;
        instances.get(instance_id).and_then(|instance| {
            if instance.status == InstanceStatus::Ready {
                instance.vm_handle.as_ref().map(|handle| handle.guest_cid)
            } else {
                None
            }
        })
    }

    /// Check and update instance health.
    pub async fn check_health(&self) {
        let instances: Vec<(String, InstanceState)> = {
            let instances = self.instances.read().await;
            instances
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

        for (instance_id, state) in instances {
            if let Some(handle) = &state.vm_handle {
                match self.runtime.check_vm_health(handle).await {
                    Ok(healthy) => {
                        if !healthy && state.status == InstanceStatus::Ready {
                            warn!(instance_id = %instance_id, "Instance health check failed");
                            let mut instances = self.instances.write().await;
                            if let Some(instance) = instances.get_mut(&instance_id) {
                                instance.status = InstanceStatus::Failed;
                                instance.error_message = Some("Health check failed".to_string());
                            }
                        }
                    }
                    Err(e) => {
                        warn!(instance_id = %instance_id, error = %e, "Error checking instance health");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_state_from_plan() {
        let plan = InstancePlan {
            instance_id: "inst_123".to_string(),
            app_id: "app_456".to_string(),
            env_id: "env_789".to_string(),
            process_type: "web".to_string(),
            release_id: "rel_abc".to_string(),
            deploy_id: "dep_xyz".to_string(),
            image: "ghcr.io/org/app:v1".to_string(),
            command: vec!["./start".to_string()],
            resources: crate::client::InstanceResources {
                cpu: 1.0,
                memory_bytes: 512 * 1024 * 1024,
            },
            overlay_ipv6: "fd00::1".to_string(),
            secrets_version_id: None,
            env_vars: serde_json::json!({}),
            volumes: vec![],
        };

        let state = InstanceState::from_plan(plan);
        assert_eq!(state.status, InstanceStatus::Booting);
        assert!(state.boot_id.is_none());
    }

    #[test]
    fn test_instance_state_to_status_report() {
        let plan = InstancePlan {
            instance_id: "inst_123".to_string(),
            app_id: "app_456".to_string(),
            env_id: "env_789".to_string(),
            process_type: "web".to_string(),
            release_id: "rel_abc".to_string(),
            deploy_id: "dep_xyz".to_string(),
            image: "ghcr.io/org/app:v1".to_string(),
            command: vec!["./start".to_string()],
            resources: crate::client::InstanceResources {
                cpu: 1.0,
                memory_bytes: 512 * 1024 * 1024,
            },
            overlay_ipv6: "fd00::1".to_string(),
            secrets_version_id: None,
            env_vars: serde_json::json!({}),
            volumes: vec![],
        };

        let mut state = InstanceState::from_plan(plan);
        state.status = InstanceStatus::Ready;
        state.boot_id = Some("boot_abc".to_string());

        let report = state.to_status_report();
        assert_eq!(report.instance_id, "inst_123");
        assert_eq!(report.status, InstanceStatus::Ready);
        assert_eq!(report.boot_id, Some("boot_abc".to_string()));
    }
}
