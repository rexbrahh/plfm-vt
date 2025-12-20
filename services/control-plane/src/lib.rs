//! plfm-vt control plane library.
//!
//! This crate primarily ships a `control-plane` binary, but we expose a small
//! library surface to enable integration testing and reuse.

pub mod api;
pub mod config;
pub mod db;
pub mod projections;
pub mod scheduler;
pub mod secrets;
pub mod state;
