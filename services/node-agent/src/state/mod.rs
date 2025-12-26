//! Local state persistence for node agent.
//!
//! This module provides SQLite-based storage for:
//! - Node state (plan version, event cursor)
//! - Instance records (phase, spec revision, boot ID, socket paths)
//!
//! The state store enables the agent to recover after restarts
//! and track which instances are running.

mod store;

pub use store::{
    BootStatusRecord, InstancePhase, InstanceRecord, NodeState, StateStore, StateStoreError,
};
