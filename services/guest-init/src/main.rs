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
mod health;
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
    let config = match perform_setup().await {
        Ok(config) => config,
        Err(e) => {
            report_init_failure(&e).await;
            return Err(e);
        }
    };

    let exec_handle = if config.exec.enabled {
        info!(port = config.exec.vsock_port, "starting exec service");
        Some(tokio::spawn(exec::run_exec_service(config.exec.vsock_port)))
    } else {
        None
    };

    info!("launching workload");
    let health_config = config.health;
    let workload_handle = tokio::spawn(workload::run(config.workload));

    let health_handle = if let Some(hc) = health_config {
        info!("starting health check loop");
        Some(tokio::spawn(health::run_health_checks(hc)))
    } else {
        info!("no health config, reporting ready immediately");
        handshake::report_status("ready").await?;
        None
    };

    let exit_code = tokio::select! {
        result = workload_handle => {
            match result {
                Ok(Ok(code)) => code,
                Ok(Err(e)) => {
                    report_init_failure(&e).await;
                    if let Some(handle) = exec_handle {
                        handle.abort();
                    }
                    if let Some(handle) = health_handle {
                        handle.abort();
                    }
                    return Err(e);
                }
                Err(e) => {
                    let err = anyhow::anyhow!("workload task panicked: {}", e);
                    report_init_failure(&err).await;
                    if let Some(handle) = exec_handle {
                        handle.abort();
                    }
                    if let Some(handle) = health_handle {
                        handle.abort();
                    }
                    return Err(err);
                }
            }
        }
    };

    if let Some(handle) = exec_handle {
        handle.abort();
    }
    if let Some(handle) = health_handle {
        handle.abort();
    }

    handshake::report_exit(exit_code).await?;

    Ok(exit_code)
}

async fn perform_setup() -> Result<config::GuestConfig> {
    info!("performing config handshake with host agent");
    let config = handshake::perform_handshake(CONFIG_VSOCK_PORT).await?;
    info!(
        instance_id = %config.instance_id,
        generation = config.generation,
        "config received"
    );

    info!("configuring network");
    network::configure(&config.network).await?;
    info!("network configured");

    if !config.mounts.is_empty() {
        info!(count = config.mounts.len(), "mounting volumes");
        for mount_config in &config.mounts {
            mount::mount_volume(mount_config)?;
        }
        info!("volumes mounted");
    }

    if let Some(secrets_config) = &config.secrets {
        info!("materializing secrets");
        secrets::materialize(secrets_config).await?;
        info!("secrets materialized");
    }

    handshake::report_status("config_applied").await?;
    info!("config applied");

    Ok(config)
}

async fn report_init_failure(err: &anyhow::Error) {
    let (reason, detail) = extract_failure_info(err);
    if let Err(e) = handshake::report_failure(&reason, &detail).await {
        error!(error = %e, "failed to report failure to host");
    }
}

fn extract_failure_info(err: &anyhow::Error) -> (String, String) {
    if let Some(init_err) = err.downcast_ref::<error::InitError>() {
        (init_err.reason_code().to_string(), init_err.to_string())
    } else {
        ("unknown".to_string(), err.to_string())
    }
}
