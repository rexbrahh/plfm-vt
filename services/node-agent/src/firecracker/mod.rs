//! Firecracker microVM runtime implementation.
//!
//! This module provides the full Firecracker runtime for production use,
//! implementing the `Runtime` trait defined in the runtime module.
//!
//! ## Components
//!
//! - `api`: HTTP client for Firecracker's Unix socket API
//! - `config`: VM configuration structures (machine, boot, drives, network)
//! - `jailer`: Sandbox configuration and cgroup setup
//! - `runtime`: Full `Runtime` trait implementation
//!
//! ## Reference
//!
//! - Firecracker boot contract: `docs/specs/runtime/firecracker-boot.md`
//! - Limits and isolation: `docs/specs/runtime/limits-and-isolation.md`

#![allow(dead_code)]

mod api;
mod config;
mod jailer;
mod runtime;

pub use api::FirecrackerClient;
pub use config::{BootSource, DriveConfig, MachineConfig, NetworkInterface, VsockConfig};
pub use jailer::JailerConfig;
pub use runtime::FirecrackerRuntime;
