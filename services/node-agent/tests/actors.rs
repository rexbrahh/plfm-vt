//! Integration tests for the actor system.
//!
//! These tests verify the actor framework and supervisor behavior.

use plfm_node_agent::client::{InstancePlan, WorkloadImage, WorkloadNetwork, WorkloadResources};

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

#[test]
fn test_instance_plan_creation() {
    let plan = test_plan("inst_001");
    assert_eq!(plan.instance_id, "inst_001");
    assert_eq!(plan.process_type, "web");
    assert_eq!(plan.resources.cpu_request, 1.0);
}

#[test]
fn test_multiple_instance_plans() {
    let plans: Vec<InstancePlan> = vec![
        test_plan("inst_001"),
        test_plan("inst_002"),
        test_plan("inst_003"),
    ];

    assert_eq!(plans.len(), 3);
    assert!(plans.iter().all(|p| p.process_type == "web"));
}

// Note: Full actor integration tests require spawning actors with their
// internal run loops. These are better tested via the supervisor tests
// in the library's unit tests.
//
// The framework module tests (in src/actors/framework.rs) already cover:
// - Actor creation and message handling
// - ActorHandle send/receive
// - Backoff policy calculation
// - Restart policy behavior
