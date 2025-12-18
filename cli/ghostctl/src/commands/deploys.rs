//! Deploy commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Deploy commands.
#[derive(Debug, Args)]
pub struct DeploysCommand {
    #[command(subcommand)]
    command: DeploysSubcommand,
}

#[derive(Debug, Subcommand)]
enum DeploysSubcommand {
    /// List deploys for an environment.
    List,

    /// Create a new deploy (deploy a release to an environment).
    Create(CreateDeployArgs),

    /// Get deploy details.
    Get(GetDeployArgs),
}

#[derive(Debug, Args)]
struct CreateDeployArgs {
    /// Release ID to deploy.
    release: String,

    /// Optional description.
    #[arg(long)]
    description: Option<String>,
}

#[derive(Debug, Args)]
struct GetDeployArgs {
    /// Deploy ID.
    deploy: String,
}

impl DeploysCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            DeploysSubcommand::List => list_deploys(ctx).await,
            DeploysSubcommand::Create(args) => create_deploy(ctx, args).await,
            DeploysSubcommand::Get(args) => get_deploy(ctx, args).await,
        }
    }
}

/// Deploy response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct DeployResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Env ID")]
    env_id: String,

    #[tabled(rename = "Release ID")]
    release_id: String,

    #[tabled(rename = "Status")]
    status: String,

    #[tabled(rename = "Created")]
    created_at: String,
}

/// List response from API.
#[derive(Debug, Deserialize)]
struct ListDeploysResponse {
    items: Vec<DeployResponse>,
    #[allow(dead_code)]
    total: i64,
}

/// Create deploy request.
#[derive(Debug, Serialize)]
struct CreateDeployRequest {
    release_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// Require an env to be specified.
fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env()
        .ok_or_else(|| anyhow::anyhow!("No environment specified. Use --env or set a default context."))
}

/// List all deploys for the current env.
async fn list_deploys(ctx: CommandContext) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let env = require_env(&ctx)?;
    let client = ctx.client()?;

    let response: ListDeploysResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/envs/{}/deploys",
            org, app, env
        ))
        .await?;

    print_output(&response.items, ctx.format);
    Ok(())
}

/// Create a new deploy.
async fn create_deploy(ctx: CommandContext, args: CreateDeployArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let env = require_env(&ctx)?;
    let client = ctx.client()?;

    let request = CreateDeployRequest {
        release_id: args.release.clone(),
        description: args.description,
    };

    let response: DeployResponse = client
        .post(
            &format!("/v1/orgs/{}/apps/{}/envs/{}/deploys", org, app, env),
            &request,
        )
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created deploy {} for env {} with release {}",
                response.id, env, args.release
            ));
        }
    }

    Ok(())
}

/// Get deploy details.
async fn get_deploy(ctx: CommandContext, args: GetDeployArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let env = require_env(&ctx)?;
    let client = ctx.client()?;

    let response: DeployResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/envs/{}/deploys/{}",
            org, app, env, args.deploy
        ))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Deploy '{}' not found", args.deploy))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}
