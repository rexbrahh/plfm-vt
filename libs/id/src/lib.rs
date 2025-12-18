//! # plfm-id
//!
//! Stable ID types, parsing, and validation for the plfm-vt platform.
//!
//! ## Design Principles
//!
//! - IDs are stable and system-generated; names are user-controlled labels
//! - All IDs have a canonical string representation with strict parsing
//! - IDs support roundtrip serialization (parse → format → parse)
//! - IDs are typed to prevent mixing different resource types
//!
//! ## ID Format
//!
//! All resource IDs use a prefixed format: `{prefix}_{ulid}`
//!
//! Examples:
//! - `org_01HV4Z2WQXKJNM8GPQY6VBKC3D`
//! - `app_01HV4Z3MXNKPQR9HSTZ7WCLD4E`
//! - `inst_01HV4Z4NYPLTRS0JTUA8XDME5F`
//!
//! This format provides:
//! - Type safety (prefix indicates resource type)
//! - Sortability (ULID is time-ordered)
//! - Uniqueness (ULID has 80 bits of randomness)
//! - Human readability (clear prefixes)

mod error;
mod macros;
mod types;

pub use error::IdError;
pub use types::*;

/// Re-export ulid for consumers that need raw ULID operations
pub use ulid::Ulid;
