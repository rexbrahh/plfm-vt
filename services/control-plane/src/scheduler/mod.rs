//! Scheduler module for instance allocation and placement.
//!
//! The scheduler is responsible for:
//! - Computing desired instances from env scale and release settings
//! - Allocating instances to nodes based on capacity and constraints
//! - Managing rolling updates and rollbacks
//! - Emitting instance.allocated and instance.desired_state_changed events
//!
//! See: docs/specs/scheduler/reconciliation-loop.md
//! See: docs/specs/scheduler/placement.md

mod reconciler;
mod worker;

#[allow(unused_imports)]
pub use reconciler::SchedulerReconciler;
pub use worker::SchedulerWorker;
