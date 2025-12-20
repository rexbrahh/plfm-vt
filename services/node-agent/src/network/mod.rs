//! Network setup for microVM instances.
//!
//! This module handles TAP device creation and configuration for Firecracker VMs.
//! Each microVM gets a dedicated TAP device for its eth0 interface.
//!
//! Architecture:
//! - TAP device per instance (e.g., `tap-inst_abc123`)
//! - IPv6 link-local gateway on host side (fe80::1)
//! - Proxy NDP or routing for instance overlay IPv6
//! - MTU matching overlay (1420 default)

#![allow(dead_code)]

mod tap;

pub use tap::{TapConfig, TapDevice, TapError};
