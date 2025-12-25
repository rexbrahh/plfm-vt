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

use crate::client::{
    ControlPlaneClient, DesiredInstanceAssignment, FailureReason, InstanceDesiredState,
    InstancePlan, InstanceStatus, InstanceStatusReport,
};
use crate::runtime::{Runtime, VmHandle};
use crate::vsock::{ConfigStore, PendingConfig};

/// Tracks a single instance's state.
#[derive(Debug, Clone)]
pub struct InstanceState {
    pub plan: InstancePlan,
    pub status: InstanceStatus,
    pub last_reported_status: Option<InstanceStatus>,
    pub boot_id: Option<String>,
    pub vm_handle: Option<VmHandle>,
    pub reason_code: Option<FailureReason>,
    pub error_message: Option<String>,
    pub exit_code: Option<i32>,
}

impl InstanceState {
    pub fn from_plan(plan: InstancePlan) -> Self {
        Self {
            plan,
            status: InstanceStatus::Booting,
            last_reported_status: None,
            boot_id: None,
            vm_handle: None,
            reason_code: None,
            error_message: None,
            exit_code: None,
        }
    }

    /// Check if status needs to be reported (transition detection).
    pub fn needs_status_report(&self) -> bool {
        self.last_reported_status.as_ref() != Some(&self.status)
    }

    /// Mark current status as reported.
    pub fn mark_status_reported(&mut self) {
        self.last_reported_status = Some(self.status);
    }

    pub fn to_status_report(&self) -> InstanceStatusReport {
        InstanceStatusReport {
            instance_id: self.plan.instance_id.clone(),
            status: self.status,
            boot_id: self.boot_id.clone(),
            reason_code: self.reason_code,
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

    last_cursor_event_id: RwLock<i64>,
    last_plan_id: RwLock<Option<String>>,

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
            last_cursor_event_id: RwLock::new(0),
            last_plan_id: RwLock::new(None),
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

    pub async fn last_cursor_event_id(&self) -> i64 {
        *self.last_cursor_event_id.read().await
    }

    /// Apply a new plan, converging the local state to match.
    pub async fn apply_plan(
        &self,
        cursor_event_id: i64,
        plan_id: String,
        desired_instances: Vec<DesiredInstanceAssignment>,
    ) {
        let last_cursor = *self.last_cursor_event_id.read().await;
        if cursor_event_id < last_cursor {
            return;
        }

        if cursor_event_id == last_cursor {
            let last_plan_id = self.last_plan_id.read().await.clone();
            if last_plan_id.as_deref() == Some(plan_id.as_str()) {
                return;
            }
        }

        info!(
            cursor_event_id,
            instance_count = desired_instances.len(),
            "Applying new plan"
        );

        let desired_ids: std::collections::HashSet<_> = desired_instances
            .iter()
            .map(|assignment| assignment.instance_id.clone())
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
        for assignment in desired_instances {
            match assignment.desired_state {
                InstanceDesiredState::Running => {
                    if let Some(plan) = assignment.workload {
                        self.ensure_instance(plan).await;
                    } else {
                        warn!(
                            instance_id = %assignment.instance_id,
                            "Missing workload spec for running instance"
                        );
                    }
                }
                InstanceDesiredState::Draining | InstanceDesiredState::Stopped => {
                    self.stop_instance(&assignment.instance_id).await;
                }
            }
        }

        *self.last_cursor_event_id.write().await = cursor_event_id;
        *self.last_plan_id.write().await = Some(plan_id);
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
                let image_changed = existing.plan.image.resolved_digest
                    != plan.image.resolved_digest
                    || existing.plan.image.image_ref != plan.image.image_ref;

                if existing.plan.release_id != plan.release_id || image_changed {
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
        let env_var_count = plan.env_vars.as_ref().map(|m| m.len()).unwrap_or(0);
        let mount_count = plan.mounts.as_ref().map(|m| m.len()).unwrap_or(0);
        let read_only_mount_count = plan
            .mounts
            .as_ref()
            .map(|m| m.iter().filter(|mount| mount.read_only).count())
            .unwrap_or(0);
        let non_empty_volume_ids = plan
            .mounts
            .as_ref()
            .map(|m| m.iter().filter(|mount| !mount.volume_id.is_empty()).count())
            .unwrap_or(0);
        let total_mount_path_chars: usize = plan
            .mounts
            .as_ref()
            .map(|m| m.iter().map(|mount| mount.mount_path.len()).sum())
            .unwrap_or(0);
        let image_label = plan
            .image
            .image_ref
            .as_deref()
            .unwrap_or(&plan.image.resolved_digest);

        info!(
            instance_id = %instance_id,
            app_id = %plan.app_id,
            env_id = %plan.env_id,
            release_id = %plan.release_id,
            manifest_hash = %plan.manifest_hash,
            image = %image_label,
            env_var_count,
            mount_count,
            read_only_mount_count,
            non_empty_volume_ids,
            total_mount_path_chars,
            "Starting instance"
        );

        // Create initial state
        let mut state = InstanceState::from_plan(plan.clone());

        let secret_version_id = plan
            .secrets
            .as_ref()
            .and_then(|secrets| secrets.secret_version_id.as_deref());
        let secrets_data = match secret_version_id {
            Some(version_id) => match self.control_plane.fetch_secret_material(version_id).await {
                Ok(payload) => Some(payload.data),
                Err(e) => {
                    state.status = InstanceStatus::Failed;
                    state.reason_code = Some(FailureReason::SecretsInjectionFailed);
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
        let overlay_ipv6 = if plan.network.overlay_ipv6.is_empty() {
            "fd00::1".to_string()
        } else {
            plan.network.overlay_ipv6.clone()
        };
        let gateway_ipv6 = if plan.network.gateway_ipv6.is_empty() {
            "fe80::1".to_string()
        } else {
            plan.network.gateway_ipv6.clone()
        };

        let pending = PendingConfig {
            plan: plan.clone(),
            overlay_ipv6,
            gateway_ipv6,
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
                state.reason_code = Some(FailureReason::FirecrackerStartFailed);
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

    /// Get status reports for instances with status transitions (not yet reported).
    pub async fn get_pending_status_reports(&self) -> Vec<InstanceStatusReport> {
        let instances = self.instances.read().await;
        instances
            .values()
            .filter(|i| i.needs_status_report())
            .map(|i| i.to_status_report())
            .collect()
    }

    /// Mark an instance's current status as reported.
    pub async fn mark_status_reported(&self, instance_id: &str) {
        let mut instances = self.instances.write().await;
        if let Some(instance) = instances.get_mut(instance_id) {
            instance.mark_status_reported();
        }
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
                                instance.reason_code = Some(FailureReason::HealthcheckFailed);
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

    fn test_plan() -> InstancePlan {
        InstancePlan {
            spec_version: "v1".to_string(),
            org_id: "org_test".to_string(),
            app_id: "app_456".to_string(),
            env_id: "env_789".to_string(),
            process_type: "web".to_string(),
            instance_id: "inst_123".to_string(),
            generation: 1,
            release_id: "rel_abc".to_string(),
            image: crate::client::WorkloadImage {
                image_ref: Some("ghcr.io/org/app:v1".to_string()),
                digest: "sha256:manifest".to_string(),
                index_digest: None,
                resolved_digest: "sha256:resolved".to_string(),
                os: "linux".to_string(),
                arch: "amd64".to_string(),
            },
            manifest_hash: "hash_abc".to_string(),
            command: vec!["./start".to_string()],
            workdir: None,
            env_vars: None,
            resources: crate::client::WorkloadResources {
                cpu_request: 1.0,
                memory_limit_bytes: 512 * 1024 * 1024,
                ephemeral_disk_bytes: None,
                vcpu_count: None,
                cpu_weight: None,
            },
            network: crate::client::WorkloadNetwork {
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

    #[test]
    fn test_instance_state_from_plan() {
        let plan = test_plan();
        let state = InstanceState::from_plan(plan);
        assert_eq!(state.status, InstanceStatus::Booting);
        assert!(state.boot_id.is_none());
    }

    #[test]
    fn test_instance_state_to_status_report() {
        let plan = test_plan();
        let mut state = InstanceState::from_plan(plan);
        state.status = InstanceStatus::Ready;
        state.boot_id = Some("boot_abc".to_string());

        let report = state.to_status_report();
        assert_eq!(report.instance_id, "inst_123");
        assert_eq!(report.status, InstanceStatus::Ready);
        assert_eq!(report.boot_id, Some("boot_abc".to_string()));
    }

    #[test]
    fn test_needs_status_report_initial() {
        let state = InstanceState::from_plan(test_plan());
        assert!(state.needs_status_report());
    }

    #[test]
    fn test_needs_status_report_after_mark() {
        let mut state = InstanceState::from_plan(test_plan());
        state.mark_status_reported();
        assert!(!state.needs_status_report());
    }

    #[test]
    fn test_needs_status_report_after_transition() {
        let mut state = InstanceState::from_plan(test_plan());
        state.mark_status_reported();
        state.status = InstanceStatus::Ready;
        assert!(state.needs_status_report());
    }
}
