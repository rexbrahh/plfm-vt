//! plfm-vt Node Agent Library
//!
//! The node agent runs on each bare-metal host and manages workload lifecycle.
//! It receives desired state from the control plane and converges the node
//! to match that state by booting/stopping Firecracker microVMs.
//!
//! ## Architecture
//!
//! The node agent uses an actor-based supervision tree for fault isolation:
//!
//! ```text
//! NodeSupervisor
//! ├── ControlPlaneStreamActor  (connection to control plane)
//! ├── ImagePullActor           (deduped image pulls)
//! └── InstanceActor(id)        (per-instance VM lifecycle)
//! ```
//!
//! See `docs/architecture/07-actors-and-supervision.md` for details.
//!
//! ## Modules
//!
//! - `actors`: Actor framework and implementations
//! - `firecracker`: Firecracker microVM runtime implementation
//! - `image`: OCI image fetching and root disk building
//! - `state`: Local SQLite state persistence

pub mod actors;
pub mod client;
pub mod exec;
pub mod exec_gateway;
pub mod firecracker;
pub mod image;
pub mod network;
pub mod state;
pub mod vsock;

// Internal modules exposed for integration tests
pub mod config;
pub mod heartbeat;
pub mod instance;
pub mod reconciler;
pub mod runtime;

// Re-export commonly used types
pub use client::{ControlPlaneClient, InstancePlan, InstanceResources};
pub use instance::{InstanceManager, InstanceState};
pub use runtime::MockRuntime;
