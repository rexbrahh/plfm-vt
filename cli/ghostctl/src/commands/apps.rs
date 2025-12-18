//! Application commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Application commands.
#[derive(Debug, Args)]
pub struct AppsCommand {
    #[command(subcommand)]
    command: AppsSubcommand,
}

#[derive(Debug, Subcommand)]
enum AppsSubcommand {
    /// List applications in an organization.
    List,

    /// Create a new application.
    Create(CreateAppArgs),

    /// Get application details.
    Get(GetAppArgs),
}

#[derive(Debug, Args)]
struct CreateAppArgs {
    /// Application name.
    name: String,

    /// Optional description.
    #[arg(long)]
    description: Option<String>,
}

#[derive(Debug, Args)]
struct GetAppArgs {
    /// Application ID or name.
    app: String,
}

impl AppsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            AppsSubcommand::List => list_apps(ctx).await,
            AppsSubcommand::Create(args) => create_app(ctx, args).await,
            AppsSubcommand::Get(args) => get_app(ctx, args).await,
        }
    }
}

/// Application response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct AppResponse {
    #[tabled(rename = "ID")]
    id: String,
    
    #[tabled(rename = "Org ID")]
    org_id: String,
    
    #[tabled(rename = "Name")]
    name: String,
    
    #[tabled(rename = "Description", display_with = "display_option")]
    #[serde(default)]
    description: Option<String>,
    
    #[tabled(rename = "Created")]
    created_at: String,
}

fn display_option(opt: &Option<String>) -> String {
    opt.as_deref().unwrap_or("-").to_string()
}

/// List response from API.
#[derive(Debug, Deserialize)]
struct ListAppsResponse {
    items: Vec<AppResponse>,
    #[allow(dead_code)]
    total: i64,
}

/// Create app request.
#[derive(Debug, Serialize)]
struct CreateAppRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// List all applications in the current org.
async fn list_apps(ctx: CommandContext) -> Result<()> {
    let org = ctx.require_org()?;
    let client = ctx.client()?;
    
    let response: ListAppsResponse = client
        .get(&format!("/v1/orgs/{}/apps", org))
        .await?;
    
    print_output(&response.items, ctx.format);
    Ok(())
}

/// Create a new application.
async fn create_app(ctx: CommandContext, args: CreateAppArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let client = ctx.client()?;
    
    let request = CreateAppRequest {
        name: args.name.clone(),
        description: args.description,
    };
    
    let response: AppResponse = client
        .post(&format!("/v1/orgs/{}/apps", org), &request)
        .await?;
    
    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created application '{}' ({}) in org {}",
                response.name, response.id, org
            ));
        }
    }
    
    Ok(())
}

/// Get application details.
async fn get_app(ctx: CommandContext, args: GetAppArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let client = ctx.client()?;
    
    let response: AppResponse = client
        .get(&format!("/v1/orgs/{}/apps/{}", org, args.app))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Application '{}' not found", args.app))
            }
            other => other,
        })?;
    
    print_single(&response, ctx.format);
    Ok(())
}
