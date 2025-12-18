//! Error types for ID parsing and validation.

use thiserror::Error;

/// Errors that can occur when parsing or validating IDs.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IdError {
    /// The ID string is empty.
    #[error("ID cannot be empty")]
    Empty,

    /// The ID is missing the required prefix.
    #[error("ID missing prefix: expected '{expected}', got '{actual}'")]
    MissingPrefix { expected: &'static str, actual: String },

    /// The ID has an invalid prefix.
    #[error("invalid ID prefix: expected '{expected}', got '{actual}'")]
    InvalidPrefix {
        expected: &'static str,
        actual: String,
    },

    /// The ID is missing the underscore separator.
    #[error("ID missing underscore separator")]
    MissingSeparator,

    /// The ULID portion of the ID is invalid.
    #[error("invalid ULID: {0}")]
    InvalidUlid(String),

    /// The ID format is invalid.
    #[error("invalid ID format: {message}")]
    InvalidFormat { message: String },
}

impl IdError {
    /// Returns true if this error indicates the input was empty.
    pub fn is_empty(&self) -> bool {
        matches!(self, IdError::Empty)
    }

    /// Returns true if this error indicates a prefix mismatch.
    pub fn is_prefix_error(&self) -> bool {
        matches!(self, IdError::MissingPrefix { .. } | IdError::InvalidPrefix { .. })
    }
}
