//! # plfm-events
//!
//! Event type definitions and serialization for the plfm-vt platform.
//!
//! ## Design Principles
//!
//! - Events are immutable records of validated state transitions
//! - Events never contain secret values (only version IDs and metadata)
//! - Every event belongs to exactly one aggregate
//! - Events are versioned for schema evolution
//!
//! ## Event Envelope
//!
//! All events share a common envelope with:
//! - Global ordering (`event_id`)
//! - Aggregate ordering (`aggregate_type`, `aggregate_id`, `aggregate_seq`)
//! - Audit context (`actor_type`, `actor_id`, `request_id`)
//! - Correlation (`correlation_id`, `causation_id`)
//!
//! ## Event Types
//!
//! Events are organized by aggregate:
//! - Organization events (`org.*`)
//! - Application events (`app.*`, `env.*`, `release.*`, `deploy.*`)
//! - Instance events (`instance.*`)
//! - Route events (`route.*`)
//! - Volume events (`volume.*`, `snapshot.*`)
//! - Secret events (`secret_bundle.*`)
//! - Node events (`node.*`)
//! - Session events (`exec_session.*`)

mod envelope;
mod error;
mod types;

pub use envelope::*;
pub use error::EventError;
pub use types::*;
