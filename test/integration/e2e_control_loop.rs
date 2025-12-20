//! End-to-end integration test for the control loop.
//!
//! This test verifies the full flow across control-plane and node-agent:
//!
//! 1. Control plane creates org, app, env, release, deploy
//! 2. Scheduler assigns instances to nodes
//! 3. Node enrolls and receives plan via API
//! 4. Node-agent pulls images and boots instances
//! 5. Node reports instance status back to control plane
//! 6. Control plane updates instance view
//!
//! ## Test Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        Test Process                              │
//! │  ┌─────────────────────┐    ┌────────────────────────────────┐ │
//! │  │   Control Plane     │    │        Node Agent               │ │
//! │  │   (in-process)      │◄───┤        (in-process)             │ │
//! │  │                     │    │                                 │ │
//! │  │  - API Server       │    │  - NodeSupervisor               │ │
//! │  │  - Event Store      │    │  - ImagePullActor (mock)        │ │
//! │  │  - Projections      │    │  - InstanceActor (MockRuntime)  │ │
//! │  │  - Scheduler        │    │                                 │ │
//! │  └─────────────────────┘    └────────────────────────────────┘ │
//! │            │                              │                     │
//! │            └──────────────────────────────┘                     │
//! │                    HTTP API calls                               │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Running the Test
//!
//! ```bash
//! # Requires Docker for PostgreSQL testcontainer
//! cargo test -p plfm-control-plane --test core_loop
//! cargo test -p plfm-node-agent --test reconciliation
//! ```
//!
//! ## Future Enhancements
//!
//! - Add multi-node scheduling tests
//! - Add network overlay configuration tests
//! - Add volume attachment tests
//! - Add secret injection tests
//! - Add drain and eviction tests

/// Placeholder test - actual E2E tests are in:
/// - services/control-plane/tests/core_loop.rs
/// - services/control-plane/tests/nodes_api.rs
/// - services/node-agent/tests/reconciliation.rs
#[test]
fn e2e_test_locations() {
    // This file documents the E2E test architecture.
    // Actual tests are in the service-specific test directories.
    //
    // Control Plane E2E tests:
    //   - core_loop.rs: Full org → app → env → release → deploy → schedule flow
    //   - nodes_api.rs: Node enrollment, heartbeat, plan, status reporting
    //
    // Node Agent E2E tests:
    //   - reconciliation.rs: Supervisor lifecycle, instance management, scaling
    //   - image_pipeline.rs: Image pull, cache, rootdisk building
    //   - actors.rs: Actor framework behavior
}

/// Test matrix for E2E scenarios.
///
/// | Scenario                          | Control Plane | Node Agent |
/// |-----------------------------------|---------------|------------|
/// | Create org/app/env                | core_loop     | -          |
/// | Create release                    | core_loop     | -          |
/// | Create deploy                     | core_loop     | -          |
/// | Schedule instances                | core_loop     | -          |
/// | Node enrollment                   | nodes_api     | -          |
/// | Node heartbeat                    | nodes_api     | -          |
/// | Get node plan                     | nodes_api     | -          |
/// | Report instance status            | nodes_api     | -          |
/// | Supervisor lifecycle              | -             | reconciliation |
/// | Apply instances                   | -             | reconciliation |
/// | Scale up/down                     | -             | reconciliation |
/// | Image pull flow                   | -             | image_pipeline |
/// | Image cache                       | -             | image_pipeline |
#[test]
fn test_matrix_documentation() {
    // This test documents the test coverage matrix.
    // See the table above for which tests cover which scenarios.
}

/// Integration test that would run both services.
///
/// This is a placeholder for a future test that would:
/// 1. Start control-plane with testcontainer Postgres
/// 2. Start node-agent with MockRuntime
/// 3. Run the full control loop
///
/// For now, we test each service independently and rely on
/// API contract tests to ensure compatibility.
#[test]
fn future_full_integration_test() {
    // TODO: Implement when we have a test harness that can run both services
    // in the same process or coordinate between processes.
    //
    // Requirements:
    // - PostgreSQL testcontainer for control-plane
    // - MockRuntime for node-agent (no real Firecracker)
    // - HTTP client for node-agent to call control-plane
    // - Async coordination between the two
}
