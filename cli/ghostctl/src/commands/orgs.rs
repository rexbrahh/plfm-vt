//! Organization commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Organization commands.
#[derive(Debug, Args)]
pub struct OrgsCommand {
    #[command(subcommand)]
    command: OrgsSubcommand,
}

#[derive(Debug, Subcommand)]
enum OrgsSubcommand {
    /// List organizations.
    List,

    /// Create a new organization.
    Create(CreateOrgArgs),

    /// Get organization details.
    Get(GetOrgArgs),
}

#[derive(Debug, Args)]
struct CreateOrgArgs {
    /// Organization name.
    name: String,
}

#[derive(Debug, Args)]
struct GetOrgArgs {
    /// Organization ID or name.
    org: String,
}

impl OrgsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            OrgsSubcommand::List => list_orgs(ctx).await,
            OrgsSubcommand::Create(args) => create_org(ctx, args).await,
            OrgsSubcommand::Get(args) => get_org(ctx, args).await,
        }
    }
}

/// Organization response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct OrgResponse {
    #[tabled(rename = "ID")]
    id: String,
    
    #[tabled(rename = "Name")]
    name: String,
    
    #[tabled(rename = "Created")]
    created_at: String,
}

/// List response from API.
#[derive(Debug, Deserialize)]
struct ListOrgsResponse {
    items: Vec<OrgResponse>,
    #[allow(dead_code)]
    total: i64,
}

/// Create org request.
#[derive(Debug, Serialize)]
struct CreateOrgRequest {
    name: String,
}

/// List all organizations.
async fn list_orgs(ctx: CommandContext) -> Result<()> {
    let client = ctx.client()?;
    
    let response: ListOrgsResponse = client.get("/v1/orgs").await?;
    
    print_output(&response.items, ctx.format);
    Ok(())
}

/// Create a new organization.
async fn create_org(ctx: CommandContext, args: CreateOrgArgs) -> Result<()> {
    let client = ctx.client()?;
    
    let request = CreateOrgRequest { name: args.name.clone() };
    let response: OrgResponse = client.post("/v1/orgs", &request).await?;
    
    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!("Created organization '{}' ({})", response.name, response.id));
        }
    }
    
    Ok(())
}

/// Get organization details.
async fn get_org(ctx: CommandContext, args: GetOrgArgs) -> Result<()> {
    let client = ctx.client()?;
    
    let response: OrgResponse = client
        .get(&format!("/v1/orgs/{}", args.org))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Organization '{}' not found", args.org))
            }
            other => other,
        })?;
    
    print_single(&response, ctx.format);
    Ok(())
}
