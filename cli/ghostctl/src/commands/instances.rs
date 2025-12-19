//! Instance commands (VM instance management).

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, OutputFormat};

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
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,

    /// Filter by process type (optional).
    #[arg(long)]
    process_type: Option<String>,

    /// Filter by instance status (optional).
    #[arg(long)]
    status: Option<String>,
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

    #[tabled(rename = "Process")]
    process_type: String,

    #[tabled(rename = "Status")]
    status: String,

    #[tabled(rename = "Node", display_with = "display_option")]
    #[serde(default)]
    node_id: Option<String>,

    #[tabled(rename = "Gen", display_with = "display_option_i32")]
    #[serde(default)]
    generation: Option<i32>,

    #[tabled(rename = "Last Transition", display_with = "display_option")]
    #[serde(default)]
    last_transition_at: Option<String>,

    #[tabled(rename = "Failure", display_with = "display_option")]
    #[serde(default)]
    failure_reason: Option<String>,

    #[tabled(rename = "Created")]
    created_at: String,
}

fn display_option(opt: &Option<String>) -> String {
    opt.as_deref().unwrap_or("-").to_string()
}

fn display_option_i32(opt: &Option<i32>) -> String {
    opt.map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// List response from API.
#[derive(Debug, Serialize, Deserialize)]
struct ListInstancesResponse {
    items: Vec<InstanceResponse>,
    next_cursor: Option<String>,
}

/// List instances.
async fn list_instances(ctx: CommandContext, args: ListInstancesArgs) -> Result<()> {
    let client = ctx.client()?;

    let org_ident = ctx.require_org()?;
    let app_ident = ctx.require_app()?;
    let env_ident = ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })?;
    let org_id = crate::resolve::resolve_org_id(&client, org_ident).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, app_ident).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env_ident).await?;

    if let Some(status) = args.status.as_deref() {
        match status {
            "booting" | "ready" | "draining" | "stopped" | "failed" => {}
            _ => return Err(anyhow::anyhow!("Invalid status '{}'", status)),
        }
    }

    let mut path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/instances?limit={}",
        org_id, app_id, env_id, args.limit
    );
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }
    if let Some(process_type) = args.process_type.as_deref() {
        path.push_str(&format!("&process_type={process_type}"));
    }
    if let Some(status) = args.status.as_deref() {
        path.push_str(&format!("&status={status}"));
    }

    let response: ListInstancesResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

/// Get instance details.
async fn get_instance(ctx: CommandContext, args: GetInstanceArgs) -> Result<()> {
    let client = ctx.client()?;

    let org_ident = ctx.require_org()?;
    let app_ident = ctx.require_app()?;
    let env_ident = ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })?;
    let org_id = crate::resolve::resolve_org_id(&client, org_ident).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, app_ident).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env_ident).await?;

    let response: InstanceResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/envs/{}/instances/{}",
            org_id, app_id, env_id, args.instance
        ))
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
