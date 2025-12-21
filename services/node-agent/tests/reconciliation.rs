//! Integration tests for the reconciliation flow.
//!
//! These tests verify the full flow from receiving a plan to booting instances:
//! 1. Supervisor receives desired instances
//! 2. ImagePullActor pulls images
//! 3. InstanceActor boots VMs
//!
//! Uses MockRuntime to simulate Firecracker operations.

use std::sync::Arc;
use std::time::Duration;

use plfm_id::NodeId;
use plfm_node_agent::actors::supervisor::NodeSupervisor;
use plfm_node_agent::client::{
    ControlPlaneClient, DesiredInstanceAssignment, InstanceDesiredState, InstancePlan,
    WorkloadImage, WorkloadNetwork, WorkloadResources,
};
use plfm_node_agent::config::Config;
use plfm_node_agent::runtime::MockRuntime;
use tokio::sync::watch;

fn test_config() -> Config {
    Config {
        node_id: NodeId::new(),
        control_plane_url: "http://localhost:8080".to_string(),
        data_dir: "/tmp/node-agent-test".to_string(),
        heartbeat_interval_secs: 30,
        log_level: "debug".to_string(),
        exec_listen_addr: "127.0.0.1:0".parse().unwrap(),
    }
}

fn test_control_plane(config: &Config) -> Arc<ControlPlaneClient> {
    Arc::new(ControlPlaneClient::new(config))
}

fn test_plan(id: &str, image: &str) -> InstancePlan {
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
            image_ref: Some(image.to_string()),
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

fn test_assignment(id: &str, image: &str) -> DesiredInstanceAssignment {
    DesiredInstanceAssignment {
        assignment_id: format!("assign-{id}"),
        node_id: "node-test".to_string(),
        instance_id: id.to_string(),
        generation: 1,
        desired_state: InstanceDesiredState::Running,
        drain_grace_seconds: None,
        workload: Some(test_plan(id, image)),
    }
}

#[tokio::test]
async fn test_supervisor_lifecycle() {
    let config = test_config();
    let runtime = Arc::new(MockRuntime::new());
    let control_plane = test_control_plane(&config);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
    supervisor.start();

    // Verify static actors are running
    assert!(supervisor.stream_handle().is_some());
    assert!(supervisor.image_handle().is_some());
    assert_eq!(supervisor.instance_count(), 0);

    // Shutdown
    shutdown_tx.send(true).unwrap();
}

#[tokio::test]
async fn test_apply_single_instance() {
    let config = test_config();
    let runtime = Arc::new(MockRuntime::new());
    let control_plane = test_control_plane(&config);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
    supervisor.start();

    // Apply one instance
    let assignments = vec![test_assignment("inst_001", "ghcr.io/test/app:v1")];
    supervisor.apply_instances(assignments).await;

    // Instance should be pending (waiting for image pull)
    assert_eq!(supervisor.pending_count(), 1);

    // Give time for async image pull to complete
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn test_apply_multiple_instances() {
    let config = test_config();
    let runtime = Arc::new(MockRuntime::new());
    let control_plane = test_control_plane(&config);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
    supervisor.start();

    // Apply multiple instances with same image (should deduplicate pulls)
    let assignments = vec![
        test_assignment("inst_001", "ghcr.io/test/app:v1"),
        test_assignment("inst_002", "ghcr.io/test/app:v1"),
        test_assignment("inst_003", "ghcr.io/test/worker:v1"),
    ];
    supervisor.apply_instances(assignments).await;

    // All instances should be pending
    assert_eq!(supervisor.pending_count(), 3);
}

#[tokio::test]
async fn test_scale_up_and_down() {
    let config = test_config();
    let runtime = Arc::new(MockRuntime::new());
    let control_plane = test_control_plane(&config);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create supervisor without image actor (direct spawn)
    let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
    // Don't call start() - this bypasses image pull

    // Scale up to 3 instances
    let assignments = vec![
        test_assignment("inst_001", "test:v1"),
        test_assignment("inst_002", "test:v1"),
        test_assignment("inst_003", "test:v1"),
    ];
    supervisor.apply_instances(assignments).await;
    assert_eq!(supervisor.instance_count(), 3);

    // Scale down to 1
    let assignments = vec![test_assignment("inst_001", "test:v1")];
    supervisor.apply_instances(assignments).await;

    // Give actors time to process stop messages
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(supervisor.instance_count(), 1);

    // Scale to 0
    supervisor.apply_instances(Vec::new()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(supervisor.instance_count(), 0);
}

#[tokio::test]
async fn test_update_instance_spec() {
    let config = test_config();
    let runtime = Arc::new(MockRuntime::new());
    let control_plane = test_control_plane(&config);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
    // Don't call start() - direct spawn

    // Create instance
    let assignments = vec![test_assignment("inst_001", "test:v1")];
    supervisor.apply_instances(assignments).await;
    assert_eq!(supervisor.instance_count(), 1);

    // Update to new version (should trigger restart)
    let assignments = vec![test_assignment("inst_001", "test:v2")];
    supervisor.apply_instances(assignments).await;

    // Instance should still exist
    assert_eq!(supervisor.instance_count(), 1);
}

#[tokio::test]
async fn test_instance_with_digest() {
    let config = test_config();
    let runtime = Arc::new(MockRuntime::new());
    let control_plane = test_control_plane(&config);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
    supervisor.start();

    // Apply instance with digest in image ref
    let image_with_digest =
        "ghcr.io/test/app@sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
    let assignments = vec![test_assignment("inst_001", image_with_digest)];
    supervisor.apply_instances(assignments).await;

    // Should be pending for image pull
    assert_eq!(supervisor.pending_count(), 1);
}

#[tokio::test]
async fn test_concurrent_apply() {
    let config = test_config();
    let runtime = Arc::new(MockRuntime::new());
    let control_plane = test_control_plane(&config);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut supervisor = NodeSupervisor::new(config, runtime, control_plane, shutdown_rx);
    // Don't call start() - direct spawn

    // Rapidly apply different sets
    supervisor
        .apply_instances(vec![test_assignment("inst_001", "test:v1")])
        .await;
    supervisor
        .apply_instances(vec![
            test_assignment("inst_001", "test:v1"),
            test_assignment("inst_002", "test:v1"),
        ])
        .await;
    supervisor
        .apply_instances(vec![test_assignment("inst_002", "test:v1")])
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Only inst_002 should remain
    assert_eq!(supervisor.instance_count(), 1);
}
