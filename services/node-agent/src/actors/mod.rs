//! Actor framework for node agent.
//!
//! This module provides a lightweight actor framework tailored for the node agent's
//! reconciliation patterns. It implements the supervision tree design from
//! `docs/architecture/07-actors-and-supervision.md`.
//!
//! ## Design Principles
//!
//! - **One actor per resource**: Each actor owns the mutable state and side effects
//!   for a single resource (instance, volume attachment, etc.)
//! - **Message coalescing**: Multiple updates to the same resource are coalesced
//!   into a single reconciliation pass
//! - **Crash isolation**: Actor crashes are contained and don't affect siblings
//! - **Supervised restart**: Failed actors are restarted with exponential backoff
//!
//! ## Actor Types
//!
//! - `InstanceActor`: Manages a single microVM instance lifecycle
//! - `ImagePullActor`: Deduplicates and manages image pulls
//! - `ControlPlaneStreamActor`: Maintains connection to control plane
//! - `NodeSupervisor`: Root supervisor that manages all child actors

mod framework;
mod instance;
mod image;
mod stream;
mod supervisor;

pub use framework::{
    Actor, ActorContext, ActorError, ActorHandle, ActorRef, BackoffPolicy, Message,
    RestartPolicy, Supervisor,
};
pub use instance::{InstanceActor, InstanceActorState, InstanceMessage};
pub use image::{ImagePullActor, ImageMessage};
pub use stream::{ControlPlaneStreamActor, StreamMessage};
pub use supervisor::NodeSupervisor;
