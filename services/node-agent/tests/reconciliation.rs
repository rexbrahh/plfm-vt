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
use plfm_node_agent::client::{ControlPlaneClient, InstancePlan, InstanceResources};
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
        instance_id: id.to_string(),
        app_id: "app_test".to_string(),
        env_id: "env_test".to_string(),
        process_type: "web".to_string(),
        release_id: "rel_test".to_string(),
        deploy_id: "dep_test".to_string(),
        image: image.to_string(),
        command: vec!["./start".to_string()],
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
    let plans = vec![test_plan("inst_001", "ghcr.io/test/app:v1")];
    supervisor.apply_instances(plans).await;

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
    let plans = vec![
        test_plan("inst_001", "ghcr.io/test/app:v1"),
        test_plan("inst_002", "ghcr.io/test/app:v1"),
        test_plan("inst_003", "ghcr.io/test/worker:v1"),
    ];
    supervisor.apply_instances(plans).await;

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
    let plans = vec![
        test_plan("inst_001", "test:v1"),
        test_plan("inst_002", "test:v1"),
        test_plan("inst_003", "test:v1"),
    ];
    supervisor.apply_instances(plans).await;
    assert_eq!(supervisor.instance_count(), 3);

    // Scale down to 1
    let plans = vec![test_plan("inst_001", "test:v1")];
    supervisor.apply_instances(plans).await;

    // Give actors time to process stop messages
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(supervisor.instance_count(), 1);

    // Scale to 0
    supervisor.apply_instances(vec![]).await;
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
    let plans = vec![test_plan("inst_001", "test:v1")];
    supervisor.apply_instances(plans).await;
    assert_eq!(supervisor.instance_count(), 1);

    // Update to new version (should trigger restart)
    let plans = vec![test_plan("inst_001", "test:v2")];
    supervisor.apply_instances(plans).await;

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
    let plans = vec![test_plan("inst_001", image_with_digest)];
    supervisor.apply_instances(plans).await;

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
        .apply_instances(vec![test_plan("inst_001", "test:v1")])
        .await;
    supervisor
        .apply_instances(vec![
            test_plan("inst_001", "test:v1"),
            test_plan("inst_002", "test:v1"),
        ])
        .await;
    supervisor
        .apply_instances(vec![test_plan("inst_002", "test:v1")])
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Only inst_002 should remain
    assert_eq!(supervisor.instance_count(), 1);
}
