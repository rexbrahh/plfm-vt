//! VM runtime interface and mock implementation.
//!
//! The runtime interface abstracts VM lifecycle operations:
//! - Starting/stopping Firecracker microVMs
//! - Health checks
//!
//! A mock implementation is provided for testing and development.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, info};

use crate::client::InstancePlan;

/// Handle to a running VM.
#[derive(Debug, Clone)]
pub struct VmHandle {
    /// Boot ID (unique per boot).
    pub boot_id: String,

    /// Instance ID.
    pub instance_id: String,
}

/// VM runtime interface.
#[async_trait]
pub trait Runtime: Send + Sync {
    /// Start a VM for the given instance plan.
    async fn start_vm(&self, plan: &InstancePlan) -> Result<VmHandle>;

    /// Stop a running VM.
    async fn stop_vm(&self, handle: &VmHandle) -> Result<()>;

    /// Check if a VM is healthy.
    async fn check_vm_health(&self, handle: &VmHandle) -> Result<bool>;
}

/// Mock runtime for testing and development.
pub struct MockRuntime {
    /// Counter for generating boot IDs.
    boot_counter: AtomicU64,

    /// Whether VMs should "fail" to start.
    fail_starts: bool,
}

impl MockRuntime {
    /// Create a new mock runtime.
    pub fn new() -> Self {
        Self {
            boot_counter: AtomicU64::new(0),
            fail_starts: false,
        }
    }

    /// Create a mock runtime that fails all starts.
    #[allow(dead_code)]
    pub fn failing() -> Self {
        Self {
            boot_counter: AtomicU64::new(0),
            fail_starts: true,
        }
    }

    /// Generate a new boot ID.
    fn next_boot_id(&self) -> String {
        let counter = self.boot_counter.fetch_add(1, Ordering::SeqCst);
        format!("boot_{:016x}", counter)
    }
}

impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runtime for MockRuntime {
    async fn start_vm(&self, plan: &InstancePlan) -> Result<VmHandle> {
        if self.fail_starts {
            anyhow::bail!("Mock runtime configured to fail");
        }

        info!(
            instance_id = %plan.instance_id,
            image = %plan.image,
            cpu = plan.resources.cpu,
            memory_mb = plan.resources.memory_bytes / (1024 * 1024),
            "[MOCK] Starting VM"
        );

        // Simulate some startup delay
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let boot_id = self.next_boot_id();
        debug!(
            instance_id = %plan.instance_id,
            boot_id = %boot_id,
            "[MOCK] VM started"
        );

        Ok(VmHandle {
            boot_id,
            instance_id: plan.instance_id.clone(),
        })
    }

    async fn stop_vm(&self, handle: &VmHandle) -> Result<()> {
        info!(
            instance_id = %handle.instance_id,
            boot_id = %handle.boot_id,
            "[MOCK] Stopping VM"
        );

        // Simulate some shutdown delay
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        debug!(
            instance_id = %handle.instance_id,
            "[MOCK] VM stopped"
        );

        Ok(())
    }

    async fn check_vm_health(&self, handle: &VmHandle) -> Result<bool> {
        debug!(
            instance_id = %handle.instance_id,
            boot_id = %handle.boot_id,
            "[MOCK] Checking VM health"
        );

        // Mock always returns healthy
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_plan() -> InstancePlan {
        InstancePlan {
            instance_id: "inst_test".to_string(),
            app_id: "app_test".to_string(),
            env_id: "env_test".to_string(),
            process_type: "web".to_string(),
            release_id: "rel_test".to_string(),
            deploy_id: "dep_test".to_string(),
            image: "test:latest".to_string(),
            resources: crate::client::InstanceResources {
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
    async fn test_mock_runtime_start() {
        let runtime = MockRuntime::new();
        let plan = test_plan();

        let handle = runtime.start_vm(&plan).await.unwrap();
        assert_eq!(handle.instance_id, "inst_test");
        assert!(handle.boot_id.starts_with("boot_"));
    }

    #[tokio::test]
    async fn test_mock_runtime_stop() {
        let runtime = MockRuntime::new();
        let plan = test_plan();

        let handle = runtime.start_vm(&plan).await.unwrap();
        runtime.stop_vm(&handle).await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_runtime_health() {
        let runtime = MockRuntime::new();
        let plan = test_plan();

        let handle = runtime.start_vm(&plan).await.unwrap();
        let healthy = runtime.check_vm_health(&handle).await.unwrap();
        assert!(healthy);
    }

    #[tokio::test]
    async fn test_mock_runtime_failing() {
        let runtime = MockRuntime::failing();
        let plan = test_plan();

        let result = runtime.start_vm(&plan).await;
        assert!(result.is_err());
    }
}
