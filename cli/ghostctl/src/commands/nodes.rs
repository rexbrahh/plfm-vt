//! Node commands (infrastructure management).

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single};

use super::CommandContext;

/// Node commands.
#[derive(Debug, Args)]
pub struct NodesCommand {
    #[command(subcommand)]
    command: NodesSubcommand,
}

#[derive(Debug, Subcommand)]
enum NodesSubcommand {
    /// List all nodes in the cluster.
    List(ListNodesArgs),

    /// Get node details.
    Get(GetNodeArgs),
}

#[derive(Debug, Args)]
struct ListNodesArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct GetNodeArgs {
    /// Node ID.
    node: String,
}

impl NodesCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            NodesSubcommand::List(args) => list_nodes(ctx, args).await,
            NodesSubcommand::Get(args) => get_node(ctx, args).await,
        }
    }
}

/// Node response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct NodeResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "State")]
    state: String,

    #[tabled(rename = "IPv6", display_with = "display_option")]
    #[serde(default)]
    public_ipv6: Option<String>,

    #[tabled(rename = "IPv4", display_with = "display_option")]
    #[serde(default)]
    public_ipv4: Option<String>,

    #[tabled(rename = "MTU", display_with = "display_option_i32")]
    #[serde(default)]
    mtu: Option<i32>,

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
struct ListNodesResponse {
    items: Vec<NodeResponse>,
    next_cursor: Option<String>,
}

/// List all nodes.
async fn list_nodes(ctx: CommandContext, args: ListNodesArgs) -> Result<()> {
    let client = ctx.client()?;

    let mut path = format!("/v1/nodes?limit={}", args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListNodesResponse = client.get(&path).await?;

    match ctx.format {
        crate::output::OutputFormat::Table => print_output(&response.items, ctx.format),
        crate::output::OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

/// Get node details.
async fn get_node(ctx: CommandContext, args: GetNodeArgs) -> Result<()> {
    let client = ctx.client()?;

    let response: NodeResponse = client
        .get(&format!("/v1/nodes/{}", args.node))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Node '{}' not found", args.node))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}
