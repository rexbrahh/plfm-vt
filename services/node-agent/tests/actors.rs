//! Integration tests for the actor system.
//!
//! These tests verify the actor framework and supervisor behavior.

use std::sync::Arc;

use plfm_node_agent::actors::{Actor, ActorContext, ActorError, ActorHandle};
use plfm_node_agent::client::{InstancePlan, InstanceResources};

use async_trait::async_trait;
use tokio::sync::mpsc;

/// Helper to create a test InstancePlan
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

#[test]
fn test_instance_plan_creation() {
    let plan = test_plan("inst_001");
    assert_eq!(plan.instance_id, "inst_001");
    assert_eq!(plan.process_type, "web");
    assert_eq!(plan.resources.cpu, 1.0);
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
