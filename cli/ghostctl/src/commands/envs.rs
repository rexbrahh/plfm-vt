//! Environment commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Environment commands.
#[derive(Debug, Args)]
pub struct EnvsCommand {
    #[command(subcommand)]
    command: EnvsSubcommand,
}

#[derive(Debug, Subcommand)]
enum EnvsSubcommand {
    /// List environments in an application.
    List,

    /// Create a new environment.
    Create(CreateEnvArgs),

    /// Get environment details.
    Get(GetEnvArgs),
}

#[derive(Debug, Args)]
struct CreateEnvArgs {
    /// Environment name (e.g., production, staging).
    name: String,
}

#[derive(Debug, Args)]
struct GetEnvArgs {
    /// Environment ID or name.
    env: String,
}

impl EnvsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            EnvsSubcommand::List => list_envs(ctx).await,
            EnvsSubcommand::Create(args) => create_env(ctx, args).await,
            EnvsSubcommand::Get(args) => get_env(ctx, args).await,
        }
    }
}

/// Environment response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct EnvResponse {
    #[tabled(rename = "ID")]
    id: String,
    
    #[tabled(rename = "App ID")]
    app_id: String,
    
    #[tabled(rename = "Org ID")]
    org_id: String,
    
    #[tabled(rename = "Name")]
    name: String,
    
    #[tabled(rename = "Created")]
    created_at: String,
}

/// List response from API.
#[derive(Debug, Deserialize)]
struct ListEnvsResponse {
    items: Vec<EnvResponse>,
    #[allow(dead_code)]
    total: i64,
}

/// Create env request.
#[derive(Debug, Serialize)]
struct CreateEnvRequest {
    name: String,
}

/// List all environments in the current app.
async fn list_envs(ctx: CommandContext) -> Result<()> {
    let app = ctx.require_app()?;
    let client = ctx.client()?;
    
    let response: ListEnvsResponse = client
        .get(&format!("/v1/apps/{}/envs", app))
        .await?;
    
    print_output(&response.items, ctx.format);
    Ok(())
}

/// Create a new environment.
async fn create_env(ctx: CommandContext, args: CreateEnvArgs) -> Result<()> {
    let app = ctx.require_app()?;
    let client = ctx.client()?;
    
    let request = CreateEnvRequest {
        name: args.name.clone(),
    };
    
    let response: EnvResponse = client
        .post(&format!("/v1/apps/{}/envs", app), &request)
        .await?;
    
    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created environment '{}' ({}) in app {}",
                response.name, response.id, app
            ));
        }
    }
    
    Ok(())
}

/// Get environment details.
async fn get_env(ctx: CommandContext, args: GetEnvArgs) -> Result<()> {
    let app = ctx.require_app()?;
    let client = ctx.client()?;
    
    let response: EnvResponse = client
        .get(&format!("/v1/apps/{}/envs/{}", app, args.env))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Environment '{}' not found", args.env))
            }
            other => other,
        })?;
    
    print_single(&response, ctx.format);
    Ok(())
}
