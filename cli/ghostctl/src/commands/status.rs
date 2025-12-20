//! Status command - Show desired vs current state for an app and environment.
//!
//! Per docs/cli/01-command-map.md, status shows:
//! - Current release ID, desired release ID
//! - Instance counts (desired vs running)
//! - Endpoint status
//! - Last reconcile time and last error if any

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::output::{print_single, OutputFormat};

use super::CommandContext;

/// Status command - show desired vs current state.
#[derive(Debug, Args)]
pub struct StatusCommand {
    /// Show verbose details.
    #[arg(long, short)]
    verbose: bool,
}

impl StatusCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        show_status(ctx, self.verbose).await
    }
}

/// Environment status response from API.
#[derive(Debug, Serialize, Deserialize)]
struct EnvStatusResponse {
    /// Environment ID.
    env_id: String,

    /// Environment name.
    env_name: String,

    /// App ID.
    app_id: String,

    /// App name.
    app_name: String,

    /// Current live release ID (if any).
    #[serde(default)]
    current_release_id: Option<String>,

    /// Desired release ID (target of latest deploy).
    #[serde(default)]
    desired_release_id: Option<String>,

    /// Whether current matches desired.
    #[serde(default)]
    release_synced: bool,

    /// Instance counts.
    instances: InstanceCounts,

    /// Route/endpoint summary.
    #[serde(default)]
    routes: Vec<RouteStatus>,

    /// Last reconciliation timestamp.
    #[serde(default)]
    last_reconcile_at: Option<String>,

    /// Last error (if any).
    #[serde(default)]
    last_error: Option<String>,

    /// Overall status (healthy, degraded, failed).
    status: String,
}

/// Instance count summary.
#[derive(Debug, Serialize, Deserialize)]
struct InstanceCounts {
    /// Desired instance count (sum of all process types).
    desired: i32,

    /// Ready instances.
    ready: i32,

    /// Booting instances.
    booting: i32,

    /// Draining instances.
    draining: i32,

    /// Failed instances.
    failed: i32,
}

/// Route/endpoint status.
#[derive(Debug, Serialize, Deserialize)]
struct RouteStatus {
    /// Route ID.
    id: String,

    /// Hostname.
    hostname: String,

    /// Target port.
    target_port: i32,

    /// Status (active, pending, error).
    status: String,

    /// Backend count.
    #[serde(default)]
    backend_count: i32,
}

/// Show status for the current app and environment.
async fn show_status(ctx: CommandContext, verbose: bool) -> Result<()> {
    let client = ctx.client()?;

    let org_ident = ctx.require_org()?;
    let app_ident = ctx.require_app()?;
    let env_ident = ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })?;

    let org_id = crate::resolve::resolve_org_id(&client, org_ident).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, app_ident).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env_ident).await?;

    // Fetch environment status
    let response: EnvStatusResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/envs/{}/status",
            org_id, app_id, env_id
        ))
        .await?;

    match ctx.format {
        OutputFormat::Json => {
            print_single(&response, ctx.format);
        }
        OutputFormat::Table => {
            print_status_table(&response, verbose);
        }
    }

    Ok(())
}

/// Print status in a human-readable table format.
fn print_status_table(status: &EnvStatusResponse, verbose: bool) {
    println!("App:         {}", status.app_name);
    println!("Environment: {}", status.env_name);
    println!("Status:      {}", format_status(&status.status));
    println!();

    // Release info
    println!("RELEASE");
    println!("  Current:  {}", status.current_release_id.as_deref().unwrap_or("-"));
    println!("  Desired:  {}", status.desired_release_id.as_deref().unwrap_or("-"));
    println!("  Synced:   {}", if status.release_synced { "yes" } else { "no" });
    println!();

    // Instance counts
    println!("INSTANCES");
    println!(
        "  Desired: {}  Ready: {}  Booting: {}  Draining: {}  Failed: {}",
        status.instances.desired,
        status.instances.ready,
        status.instances.booting,
        status.instances.draining,
        status.instances.failed
    );
    println!();

    // Routes/endpoints
    if !status.routes.is_empty() {
        println!("ROUTES");
        for route in &status.routes {
            println!(
                "  {} → :{} ({}, {} backends)",
                route.hostname, route.target_port, route.status, route.backend_count
            );
        }
        println!();
    }

    // Reconciliation info
    if verbose {
        println!("RECONCILIATION");
        if let Some(ts) = &status.last_reconcile_at {
            println!("  Last:  {}", ts);
        }
        if let Some(err) = &status.last_error {
            println!("  Error: {}", err);
        }
        println!();
    }
}

/// Format status with color hints (when appropriate).
fn format_status(status: &str) -> &str {
    // In a real CLI, we'd use colors here
    match status {
        "healthy" => "healthy ✓",
        "degraded" => "degraded ⚠",
        "failed" => "failed ✗",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_deserialization() {
        let json = r#"{
            "env_id": "env_123",
            "env_name": "production",
            "app_id": "app_456",
            "app_name": "myapp",
            "current_release_id": "rel_abc",
            "desired_release_id": "rel_abc",
            "release_synced": true,
            "instances": {
                "desired": 3,
                "ready": 3,
                "booting": 0,
                "draining": 0,
                "failed": 0
            },
            "routes": [
                {
                    "id": "route_123",
                    "hostname": "myapp.example.com",
                    "target_port": 8080,
                    "status": "active",
                    "backend_count": 3
                }
            ],
            "last_reconcile_at": "2025-12-19T12:00:00Z",
            "status": "healthy"
        }"#;

        let status: EnvStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(status.app_name, "myapp");
        assert_eq!(status.instances.ready, 3);
        assert!(status.release_synced);
        assert_eq!(status.routes.len(), 1);
    }
}
