//! plfm-vt Node Agent Binary
//!
//! This is the main entry point for the node agent.
//! See the library crate (`plfm_node_agent`) for documentation.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// Use the library crate
use plfm_node_agent::actors::NodeSupervisor;
use plfm_node_agent::config::Config;
use plfm_node_agent::firecracker::{FirecrackerRuntime, FirecrackerRuntimeConfig};
use plfm_node_agent::heartbeat;
use plfm_node_agent::image::{
    ImageCache, ImageCacheConfig, ImagePuller, ImagePullerConfig, OciConfig, RootDiskConfig,
};
use plfm_node_agent::reconciler::{Reconciler, ReconcilerConfig};
use plfm_node_agent::exec_gateway::ExecGateway;
use plfm_node_agent::vsock::{ConfigDeliveryService, ConfigStore};
use plfm_node_agent::{ControlPlaneClient, InstanceManager, MockRuntime};

async fn build_firecracker_runtime(
    config: &Config,
    control_plane_client: Arc<ControlPlaneClient>,
) -> Result<Arc<FirecrackerRuntime>> {
    let data_dir = PathBuf::from(&config.data_dir);
    let image_dir = data_dir.join("images");
    let cache_config = ImageCacheConfig {
        rootdisk_dir: image_dir.join("rootdisks"),
        ..Default::default()
    };
    let image_cache = Arc::new(ImageCache::new(cache_config));
    if let Err(e) = image_cache.init().await {
        warn!(error = %e, "Image cache init failed");
    }

    let puller_config = ImagePullerConfig {
        oci: OciConfig {
            blob_dir: image_dir.join("oci/blobs"),
            ..Default::default()
        },
        rootdisk: RootDiskConfig {
            unpack_dir: image_dir.join("unpacked"),
            rootdisk_dir: image_dir.join("rootdisks"),
            tmp_dir: image_dir.join("tmp"),
            ..Default::default()
        },
        ..Default::default()
    };
    let image_puller = Arc::new(ImagePuller::new(puller_config, image_cache)?);

    let mut fc_config = FirecrackerRuntimeConfig::default();
    fc_config.data_dir = data_dir;
    if let Ok(path) =
        std::env::var("PLFM_FIRECRACKER_PATH").or_else(|_| std::env::var("GHOST_FIRECRACKER_PATH"))
    {
        fc_config.firecracker_path = PathBuf::from(path);
    }
    if let Ok(path) =
        std::env::var("PLFM_JAILER_PATH").or_else(|_| std::env::var("GHOST_JAILER_PATH"))
    {
        fc_config.jailer_path = PathBuf::from(path);
    }
    if let Ok(path) =
        std::env::var("PLFM_KERNEL_PATH").or_else(|_| std::env::var("GHOST_KERNEL_PATH"))
    {
        fc_config.kernel_path = PathBuf::from(path);
    }
    if let Ok(path) =
        std::env::var("PLFM_INITRD_PATH").or_else(|_| std::env::var("GHOST_INITRD_PATH"))
    {
        fc_config.initrd_path = Some(PathBuf::from(path));
    }
    if let Ok(value) = std::env::var("PLFM_SCRATCH_DISK_BYTES")
        .or_else(|_| std::env::var("GHOST_SCRATCH_DISK_BYTES"))
    {
        if let Ok(bytes) = value.parse::<u64>() {
            fc_config.scratch_disk_bytes = bytes;
        }
    }
    if let Ok(value) =
        std::env::var("PLFM_USE_JAILER").or_else(|_| std::env::var("GHOST_USE_JAILER"))
    {
        fc_config.use_jailer = value == "1" || value.to_lowercase() == "true";
    }

    Ok(Arc::new(FirecrackerRuntime::new(
        fc_config,
        image_puller,
        Some(control_plane_client),
    )))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration
    let config = Config::from_env()?;

    // Initialize tracing (prefer RUST_LOG, fallback to GHOST_LOG_LEVEL)
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| config.log_level.clone().into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!("Starting plfm-vt node agent");
    info!(
        node_id = %config.node_id,
        control_plane_url = %config.control_plane_url,
        data_dir = %config.data_dir,
        "Configuration loaded"
    );

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let control_plane_client = Arc::new(ControlPlaneClient::new(&config));

    let runtime_kind = std::env::var("PLFM_RUNTIME")
        .or_else(|_| std::env::var("GHOST_RUNTIME"))
        .unwrap_or_else(|_| "mock".to_string());

    // Config delivery service for guest-init
    let config_store = Arc::new(ConfigStore::new());
    let config_delivery = ConfigDeliveryService::new(Arc::clone(&config_store));
    let config_delivery_handle = tokio::spawn(async move {
        if let Err(e) = config_delivery.run().await {
            error!(error = %e, "Config delivery service failed");
        }
    });

    // Determine whether to use actor-based supervision or legacy mode
    let use_actors = std::env::var("VT_USE_ACTORS")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    if use_actors {
        // === Actor-based supervision tree ===
        info!("Using actor-based supervision tree");

        if runtime_kind == "firecracker" {
            let runtime = build_firecracker_runtime(&config, Arc::clone(&control_plane_client)).await?;
            let mut supervisor =
                NodeSupervisor::new(config.clone(), Arc::clone(&runtime), shutdown_rx.clone());
            supervisor.start();

            let supervisor_handle = tokio::spawn(async move {
                supervisor.run().await;
            });

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Received shutdown signal");
                }
                _ = supervisor_handle => {
                    info!("Supervisor exited");
                }
            }
        } else {
            let runtime = Arc::new(MockRuntime::new());
            let mut supervisor =
                NodeSupervisor::new(config.clone(), Arc::clone(&runtime), shutdown_rx.clone());
            supervisor.start();

            let supervisor_handle = tokio::spawn(async move {
                supervisor.run().await;
            });

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    info!("Received shutdown signal");
                }
                _ = supervisor_handle => {
                    info!("Supervisor exited");
                }
            }
        }

        // Signal shutdown
        let _ = shutdown_tx.send(true);
    } else {
        // === Legacy mode (backward compatible) ===
        info!("Using legacy reconciliation mode");

        let runtime: Arc<dyn plfm_node_agent::runtime::Runtime> = if runtime_kind == "firecracker"
        {
            build_firecracker_runtime(&config, Arc::clone(&control_plane_client)).await?
        } else {
            Arc::new(MockRuntime::new())
        };

        // Create the instance manager
        let instance_manager = Arc::new(InstanceManager::new(
            runtime,
            Arc::clone(&config_store),
            Arc::clone(&control_plane_client),
        ));

        // Start exec gateway listener
        let exec_gateway = ExecGateway::new(config.exec_listen_addr, Arc::clone(&instance_manager));
        let exec_handle = tokio::spawn(async move {
            if let Err(e) = exec_gateway.run().await {
                error!(error = %e, "Exec gateway failed");
            }
        });

        // Start the heartbeat loop
        let heartbeat_handle = tokio::spawn({
            let config = config.clone();
            let instance_manager = Arc::clone(&instance_manager);
            let shutdown_rx = shutdown_rx.clone();
            async move { heartbeat::run_heartbeat_loop(config, instance_manager, shutdown_rx).await }
        });

        // Start the reconciliation loop
        let reconciler = Reconciler::new(
            &config,
            Arc::clone(&instance_manager),
            ReconcilerConfig::default(),
        );
        let reconciler_handle = tokio::spawn({
            let shutdown_rx = shutdown_rx.clone();
            async move {
                reconciler.run(shutdown_rx).await;
            }
        });

        // Wait for shutdown signal
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received shutdown signal");
            }
            result = heartbeat_handle => {
                match result {
                    Ok(Ok(())) => info!("Heartbeat loop exited normally"),
                    Ok(Err(e)) => error!(error = %e, "Heartbeat loop error"),
                    Err(e) => error!(error = %e, "Heartbeat task panicked"),
                }
            }
            _ = reconciler_handle => {
                info!("Reconciler exited");
            }
            _ = exec_handle => {
                warn!("Exec gateway exited");
            }
            _ = config_delivery_handle => {
                warn!("Config delivery service exited");
            }
        }

        // Signal shutdown to all workers
        let _ = shutdown_tx.send(true);

        // Give workers time to shut down gracefully
        info!("Waiting for workers to shut down...");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    info!("Node agent shutdown complete");
    Ok(())
}
