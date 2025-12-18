//! Instance commands (VM instance management).

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single};

use super::CommandContext;

/// Instance commands.
#[derive(Debug, Args)]
pub struct InstancesCommand {
    #[command(subcommand)]
    command: InstancesSubcommand,
}

#[derive(Debug, Subcommand)]
enum InstancesSubcommand {
    /// List instances.
    List(ListInstancesArgs),

    /// Get instance details.
    Get(GetInstanceArgs),
}

#[derive(Debug, Args)]
struct ListInstancesArgs {
    /// Filter by environment (optional).
    #[arg(long)]
    env: Option<String>,

    /// Filter by node (optional).
    #[arg(long)]
    node: Option<String>,
}

#[derive(Debug, Args)]
struct GetInstanceArgs {
    /// Instance ID.
    instance: String,
}

impl InstancesCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            InstancesSubcommand::List(args) => list_instances(ctx, args).await,
            InstancesSubcommand::Get(args) => get_instance(ctx, args).await,
        }
    }
}

/// Instance response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct InstanceResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Env")]
    env_id: String,

    #[tabled(rename = "Node")]
    node_id: String,

    #[tabled(rename = "Process")]
    process_type: String,

    #[tabled(rename = "Desired")]
    desired_state: String,

    #[tabled(rename = "Status", display_with = "display_option")]
    #[serde(default)]
    status: Option<String>,

    #[tabled(rename = "Release")]
    release_id: String,

    #[tabled(rename = "Created")]
    created_at: String,
}

fn display_option(opt: &Option<String>) -> String {
    opt.as_deref().unwrap_or("-").to_string()
}

/// List response from API.
#[derive(Debug, Deserialize)]
struct ListInstancesResponse {
    items: Vec<InstanceResponse>,
    #[allow(dead_code)]
    total: i64,
}

/// List instances.
async fn list_instances(ctx: CommandContext, _args: ListInstancesArgs) -> Result<()> {
    let client = ctx.client()?;

    // TODO: Add query params for filtering by env/node
    let response: ListInstancesResponse = client.get("/v1/instances").await?;

    print_output(&response.items, ctx.format);
    Ok(())
}

/// Get instance details.
async fn get_instance(ctx: CommandContext, args: GetInstanceArgs) -> Result<()> {
    let client = ctx.client()?;

    let response: InstanceResponse = client
        .get(&format!("/v1/instances/{}", args.instance))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Instance '{}' not found", args.instance))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}
