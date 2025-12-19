//! Scale command (set environment scale).

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};

use super::CommandContext;

/// Scale command - set the number of instances for a process type.
#[derive(Debug, Args)]
pub struct ScaleCommand {
    /// Process type and count in format TYPE=COUNT (e.g., web=3).
    /// Can be specified multiple times.
    #[arg(required = true)]
    processes: Vec<String>,
}

/// Set scale request.
#[derive(Debug, Serialize)]
struct SetScaleRequest {
    process_counts: std::collections::HashMap<String, i32>,
}

/// Set scale response.
#[derive(Debug, Deserialize)]
struct SetScaleResponse {
    #[allow(dead_code)]
    success: bool,
}

impl ScaleCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        let client = ctx.client()?;

        let org_id = ctx.require_org()?;
        let app_id = ctx.require_app()?;
        let env_id = ctx.resolve_env().ok_or_else(|| {
            anyhow::anyhow!("No environment specified. Use --env or set a default context.")
        })?;

        // Parse process specifications
        let mut process_counts = std::collections::HashMap::new();
        for spec in &self.processes {
            let parts: Vec<&str> = spec.split('=').collect();
            if parts.len() != 2 {
                return Err(anyhow::anyhow!(
                    "Invalid process specification '{}'. Use format TYPE=COUNT (e.g., web=3)",
                    spec
                ));
            }
            let process_type = parts[0].to_string();
            let count: i32 = parts[1].parse().map_err(|_| {
                anyhow::anyhow!(
                    "Invalid count '{}' for process type '{}'. Must be a number.",
                    parts[1],
                    parts[0]
                )
            })?;
            if count < 0 {
                return Err(anyhow::anyhow!(
                    "Count must be non-negative for process type '{}'",
                    process_type
                ));
            }
            process_counts.insert(process_type, count);
        }

        let request = SetScaleRequest {
            process_counts: process_counts.clone(),
        };
        let path = format!("/v1/orgs/{}/apps/{}/envs/{}/scale", org_id, app_id, env_id);
        let idempotency_key = match ctx.idempotency_key.as_deref() {
            Some(key) => key.to_string(),
            None => crate::idempotency::default_idempotency_key("envs.set_scale", &path, &request)?,
        };

        let _response: SetScaleResponse = client
            .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
            .await?;

        // Print what was set
        println!("Scaling environment {} in {}/{}:", env_id, org_id, app_id);
        for (process_type, count) in &process_counts {
            println!("  {} -> {} instances", process_type, count);
        }

        Ok(())
    }
}
