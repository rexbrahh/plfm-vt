//! Platform guest init - PID 1 for Firecracker microVMs.
//!
//! This binary runs as PID 1 inside each microVM and is responsible for:
//! - Config handshake with host agent over vsock
//! - Network configuration inside the guest
//! - Volume mounting
//! - Secrets materialization
//! - Workload process spawning and supervision
//! - Signal forwarding
//! - Exec service for `plfm exec`
//!
//! Reference: docs/specs/runtime/guest-init.md

use std::process::ExitCode;

use anyhow::Result;
use tracing::{error, info};

mod config;
mod error;
mod exec;
mod handshake;
mod logging;
mod mount;
mod network;
mod secrets;
mod workload;

/// Guest init version (semver).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Guest init protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// vsock port for config handshake (guest connects to host).
pub const CONFIG_VSOCK_PORT: u32 = 5161;

/// vsock port for exec service (guest listens).
pub const EXEC_VSOCK_PORT: u32 = 5162;

/// Boot log path.
pub const BOOT_LOG_PATH: &str = "/run/platform/guest-init.log";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Initialize logging to boot log file
    if let Err(e) = logging::init(BOOT_LOG_PATH) {
        eprintln!("Failed to initialize logging: {}", e);
        return ExitCode::from(1);
    }

    info!(
        version = VERSION,
        protocol = PROTOCOL_VERSION,
        "guest-init starting"
    );

    match run().await {
        Ok(exit_code) => {
            info!(exit_code, "guest-init exiting normally");
            ExitCode::from(exit_code as u8)
        }
        Err(e) => {
            error!(error = %e, "guest-init failed");
            // Log the error chain
            let mut source = e.source();
            while let Some(cause) = source {
                error!(cause = %cause, "caused by");
                source = cause.source();
            }
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<i32> {
    // Step 1: Perform config handshake with host agent
    info!("performing config handshake with host agent");
    let config = handshake::perform_handshake(CONFIG_VSOCK_PORT).await?;
    info!(
        instance_id = %config.instance_id,
        generation = config.generation,
        "config received"
    );

    // Step 2: Configure networking
    info!("configuring network");
    network::configure(&config.network).await?;
    handshake::report_status("config_applied").await?;
    info!("network configured");

    // Step 3: Mount volumes
    if !config.mounts.is_empty() {
        info!(count = config.mounts.len(), "mounting volumes");
        for mount_config in &config.mounts {
            mount::mount_volume(mount_config)?;
        }
        info!("volumes mounted");
    }

    // Step 4: Materialize secrets
    if let Some(secrets_config) = &config.secrets {
        info!("materializing secrets");
        secrets::materialize(secrets_config).await?;
        info!("secrets materialized");
    }

    // Step 5: Start exec service (background)
    let exec_handle = if config.exec.enabled {
        info!(port = config.exec.vsock_port, "starting exec service");
        Some(tokio::spawn(exec::run_exec_service(config.exec.vsock_port)))
    } else {
        None
    };

    // Step 6: Launch workload
    info!("launching workload");
    let exit_code = workload::run(&config.workload).await?;

    // Cleanup exec service
    if let Some(handle) = exec_handle {
        handle.abort();
    }

    // Report exit to host agent
    handshake::report_exit(exit_code).await?;

    Ok(exit_code)
}
