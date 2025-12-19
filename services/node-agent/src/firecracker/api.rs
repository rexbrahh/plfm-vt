//! Firecracker HTTP API client.
//!
//! This module provides an HTTP client for Firecracker's Unix socket API.
//! It handles configuration of the microVM before boot and instance actions.
//!
//! Reference: https://github.com/firecracker-microvm/firecracker/blob/main/src/api_server/swagger/firecracker.yaml

use std::path::Path;

use hyper::{body::Buf, Body, Client, Method, Request};
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use serde::Serialize;
use thiserror::Error;
use tracing::{debug, error};

use super::config::{BootSource, DriveConfig, MachineConfig, NetworkInterface, VsockConfig};

/// Errors from the Firecracker API.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] hyper::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("Socket not found: {0}")]
    SocketNotFound(String),
}

/// Firecracker API client for Unix socket communication.
pub struct FirecrackerClient {
    socket_path: String,
    client: Client<UnixConnector>,
}

impl FirecrackerClient {
    /// Create a new Firecracker client for the given socket path.
    pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
        let socket_path = socket_path.as_ref().to_string_lossy().to_string();
        let client = Client::unix();
        Self {
            socket_path,
            client,
        }
    }

    /// Check if the socket exists.
    pub fn socket_exists(&self) -> bool {
        Path::new(&self.socket_path).exists()
    }

    /// Configure the machine (vCPUs, memory).
    pub async fn put_machine_config(&self, config: &MachineConfig) -> Result<(), ApiError> {
        self.put("/machine-config", config).await
    }

    /// Configure the boot source (kernel, initrd, boot args).
    pub async fn put_boot_source(&self, config: &BootSource) -> Result<(), ApiError> {
        self.put("/boot-source", config).await
    }

    /// Add or update a drive.
    pub async fn put_drive(&self, config: &DriveConfig) -> Result<(), ApiError> {
        let path = format!("/drives/{}", config.drive_id);
        self.put(&path, config).await
    }

    /// Add or update a network interface.
    pub async fn put_network_interface(&self, config: &NetworkInterface) -> Result<(), ApiError> {
        let path = format!("/network-interfaces/{}", config.iface_id);
        self.put(&path, config).await
    }

    /// Configure vsock device.
    pub async fn put_vsock(&self, config: &VsockConfig) -> Result<(), ApiError> {
        self.put("/vsock", config).await
    }

    /// Start the microVM instance.
    pub async fn start_instance(&self) -> Result<(), ApiError> {
        #[derive(Serialize)]
        struct Action {
            action_type: &'static str,
        }
        self.put("/actions", &Action { action_type: "InstanceStart" }).await
    }

    /// Send CtrlAltDel to the guest (graceful shutdown).
    pub async fn send_ctrl_alt_del(&self) -> Result<(), ApiError> {
        #[derive(Serialize)]
        struct Action {
            action_type: &'static str,
        }
        self.put("/actions", &Action { action_type: "SendCtrlAltDel" }).await
    }

    /// Pause the microVM.
    pub async fn pause(&self) -> Result<(), ApiError> {
        #[derive(Serialize)]
        struct State {
            state: &'static str,
        }
        self.patch("/vm", &State { state: "Paused" }).await
    }

    /// Resume the microVM.
    pub async fn resume(&self) -> Result<(), ApiError> {
        #[derive(Serialize)]
        struct State {
            state: &'static str,
        }
        self.patch("/vm", &State { state: "Resumed" }).await
    }

    /// Get instance info.
    pub async fn get_instance_info(&self) -> Result<InstanceInfo, ApiError> {
        self.get("/").await
    }

    /// Perform a PUT request.
    async fn put<T: Serialize>(&self, path: &str, body: &T) -> Result<(), ApiError> {
        let body_bytes = serde_json::to_vec(body)?;
        let uri = Uri::new(&self.socket_path, path);

        debug!(path = path, "PUT request to Firecracker API");

        let request = Request::builder()
            .method(Method::PUT)
            .uri(uri)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(Body::from(body_bytes))?;

        let response = self.client.request(request).await?;
        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let body = hyper::body::aggregate(response.into_body()).await?;
            let message = String::from_utf8_lossy(body.chunk()).to_string();
            error!(status = %status, message = %message, "Firecracker API error");
            Err(ApiError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }

    /// Perform a PATCH request.
    async fn patch<T: Serialize>(&self, path: &str, body: &T) -> Result<(), ApiError> {
        let body_bytes = serde_json::to_vec(body)?;
        let uri = Uri::new(&self.socket_path, path);

        debug!(path = path, "PATCH request to Firecracker API");

        let request = Request::builder()
            .method(Method::PATCH)
            .uri(uri)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(Body::from(body_bytes))?;

        let response = self.client.request(request).await?;
        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let body = hyper::body::aggregate(response.into_body()).await?;
            let message = String::from_utf8_lossy(body.chunk()).to_string();
            Err(ApiError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }

    /// Perform a GET request.
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let uri = Uri::new(&self.socket_path, path);

        debug!(path = path, "GET request to Firecracker API");

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("Accept", "application/json")
            .body(Body::empty())?;

        let response = self.client.request(request).await?;
        let status = response.status();
        let body = hyper::body::aggregate(response.into_body()).await?;

        if status.is_success() {
            let result = serde_json::from_reader(body.reader())?;
            Ok(result)
        } else {
            let message = String::from_utf8_lossy(body.chunk()).to_string();
            Err(ApiError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }
}

/// Instance information from Firecracker.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct InstanceInfo {
    /// Application name.
    pub app_name: String,
    /// Instance ID.
    pub id: String,
    /// State of the instance.
    pub state: String,
    /// VMM version.
    pub vmm_version: String,
}

impl From<hyper::http::Error> for ApiError {
    fn from(err: hyper::http::Error) -> Self {
        ApiError::Api {
            status: 0,
            message: err.to_string(),
        }
    }
}
