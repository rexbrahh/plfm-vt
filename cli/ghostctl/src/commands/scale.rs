//! Scale command (set environment scale).

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Scale command - set the number of instances for a process type.
#[derive(Debug, Args)]
pub struct ScaleCommand {
    /// Process type and count in format TYPE=COUNT (e.g., web=3).
    /// Can be specified multiple times.
    #[arg(required = true)]
    processes: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ScaleState {
    #[allow(dead_code)]
    env_id: String,
    processes: Vec<ProcessScale>,
    #[allow(dead_code)]
    updated_at: String,
    #[serde(default)]
    resource_version: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Tabled)]
struct ProcessScale {
    #[tabled(rename = "Process")]
    process_type: String,
    #[tabled(rename = "Desired")]
    desired: i32,
}

#[derive(Debug, Serialize)]
struct ScaleUpdateRequest {
    processes: Vec<ProcessScale>,
    expected_version: i32,
}

impl ScaleCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        let client = ctx.client()?;

        let org_id = ctx.require_org()?;
        let app_id = ctx.require_app()?;
        let env_id = ctx.resolve_env().ok_or_else(|| {
            anyhow::anyhow!("No environment specified. Use --env or set a default context.")
        })?;

        // Parse process specifications (deterministic ordering)
        let mut process_counts = std::collections::BTreeMap::<String, i32>::new();
        for spec in &self.processes {
            let Some((process_type_raw, count_raw)) = spec.split_once('=') else {
                return Err(anyhow::anyhow!(
                    "Invalid process specification '{}'. Use format TYPE=COUNT (e.g., web=3)",
                    spec
                ));
            };

            let process_type = process_type_raw.trim().to_string();
            if process_type.is_empty() {
                return Err(anyhow::anyhow!(
                    "Invalid process specification '{}'. process type cannot be empty.",
                    spec
                ));
            }

            let count: i32 = count_raw.parse().map_err(|_| {
                anyhow::anyhow!(
                    "Invalid count '{}' for process type '{}'. Must be a number.",
                    count_raw,
                    process_type
                )
            })?;
            if count < 0 {
                return Err(anyhow::anyhow!(
                    "Count must be non-negative for process type '{}'",
                    process_type
                ));
            }

            if process_counts.insert(process_type.clone(), count).is_some() {
                return Err(anyhow::anyhow!(
                    "Process type '{}' specified multiple times",
                    process_type
                ));
            }
        }

        let path = format!("/v1/orgs/{}/apps/{}/envs/{}/scale", org_id, app_id, env_id);

        let current: ScaleState = client.get(&path).await.map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Environment '{}' not found", env_id))
            }
            other => other,
        })?;

        let expected_version = current.resource_version.unwrap_or(0);
        let processes: Vec<ProcessScale> = process_counts
            .into_iter()
            .map(|(process_type, desired)| ProcessScale {
                process_type,
                desired,
            })
            .collect();

        let request = ScaleUpdateRequest {
            processes: processes.clone(),
            expected_version,
        };
        let idempotency_key = match ctx.idempotency_key.as_deref() {
            Some(key) => key.to_string(),
            None => crate::idempotency::default_idempotency_key("envs.set_scale", &path, &request)?,
        };

        let response: ScaleState = client
            .put_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
            .await?;

        match ctx.format {
            OutputFormat::Json => print_single(&response, ctx.format),
            OutputFormat::Table => {
                let version = response.resource_version.unwrap_or(0);
                print_success(&format!(
                    "Updated scale for environment {} in {}/{} (resource_version {})",
                    env_id, org_id, app_id, version
                ));

                let mut rows = response.processes.clone();
                rows.sort_by(|a, b| a.process_type.cmp(&b.process_type));
                print_output(&rows, ctx.format);
            }
        }

        Ok(())
    }
}
