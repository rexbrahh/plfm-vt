//! M3 Integration tests: Status transition reporting.
//!
//! Tests verify the control loop behavior specified in M3:
//! - Instances report status only on transitions (not every tick)
//! - Successful boot results in Ready status
//! - Failed boot results in Failed status with reason

use plfm_node_agent::client::{
    FailureReason, InstancePlan, InstanceStatus, WorkloadImage, WorkloadNetwork, WorkloadResources,
};
use plfm_node_agent::instance::InstanceState;

fn test_plan(instance_id: &str) -> InstancePlan {
    InstancePlan {
        spec_version: "v1".to_string(),
        org_id: "org_test".to_string(),
        app_id: "app_test".to_string(),
        env_id: "env_test".to_string(),
        process_type: "web".to_string(),
        instance_id: instance_id.to_string(),
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
fn test_initial_status_needs_report() {
    let plan = test_plan("inst_001");
    let state = InstanceState::from_plan(plan);

    assert!(state.needs_status_report(), "Initial status should need reporting");
    assert_eq!(state.status, InstanceStatus::Booting);
}

#[test]
fn test_status_not_reported_twice() {
    let plan = test_plan("inst_001");
    let mut state = InstanceState::from_plan(plan);

    assert!(state.needs_status_report());

    state.mark_status_reported();
    assert!(!state.needs_status_report(), "Same status should not be reported twice");
}

#[test]
fn test_status_transition_triggers_report() {
    let plan = test_plan("inst_001");
    let mut state = InstanceState::from_plan(plan);

    state.mark_status_reported();
    assert!(!state.needs_status_report());

    state.status = InstanceStatus::Ready;
    assert!(state.needs_status_report(), "Transition to Ready should trigger report");

    state.mark_status_reported();
    assert!(!state.needs_status_report());
}

#[test]
fn test_failure_transition_triggers_report() {
    let plan = test_plan("inst_001");
    let mut state = InstanceState::from_plan(plan);

    state.mark_status_reported();

    state.status = InstanceStatus::Failed;
    state.reason_code = Some(FailureReason::FirecrackerStartFailed);
    state.error_message = Some("Boot timeout".to_string());
    assert!(state.needs_status_report(), "Transition to Failed should trigger report");

    let report = state.to_status_report();
    assert_eq!(report.status, InstanceStatus::Failed);
    assert_eq!(report.reason_code, Some(FailureReason::FirecrackerStartFailed));
    assert_eq!(report.error_message, Some("Boot timeout".to_string()));
}

#[test]
fn test_multiple_transitions() {
    let plan = test_plan("inst_001");
    let mut state = InstanceState::from_plan(plan);

    state.mark_status_reported();
    state.status = InstanceStatus::Ready;
    assert!(state.needs_status_report());
    state.mark_status_reported();

    state.status = InstanceStatus::Draining;
    assert!(state.needs_status_report());
    state.mark_status_reported();

    state.status = InstanceStatus::Stopped;
    assert!(state.needs_status_report());
    state.mark_status_reported();

    assert!(!state.needs_status_report());
}
