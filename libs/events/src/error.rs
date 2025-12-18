//! Error types for event handling.

use thiserror::Error;

/// Errors that can occur when handling events.
#[derive(Debug, Error, Clone)]
pub enum EventError {
    /// The event type is unknown.
    #[error("unknown event type: {0}")]
    UnknownEventType(String),

    /// The event version is not supported.
    #[error("unsupported event version: {event_type} v{version}")]
    UnsupportedVersion { event_type: String, version: i32 },

    /// The event payload is invalid.
    #[error("invalid event payload: {0}")]
    InvalidPayload(String),

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// The aggregate sequence is invalid.
    #[error("invalid aggregate sequence: expected {expected}, got {actual}")]
    InvalidSequence { expected: i32, actual: i32 },
}

impl From<serde_json::Error> for EventError {
    fn from(err: serde_json::Error) -> Self {
        EventError::Serialization(err.to_string())
    }
}
