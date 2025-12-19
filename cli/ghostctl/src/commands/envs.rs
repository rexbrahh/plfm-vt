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
    List(ListEnvsArgs),

    /// Create a new environment.
    Create(CreateEnvArgs),

    /// Get environment details.
    Get(GetEnvArgs),
}

#[derive(Debug, Args)]
struct ListEnvsArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
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
            EnvsSubcommand::List(args) => list_envs(ctx, args).await,
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
#[derive(Debug, Serialize, Deserialize)]
struct ListEnvsResponse {
    items: Vec<EnvResponse>,
    next_cursor: Option<String>,
}

/// Create env request.
#[derive(Debug, Serialize)]
struct CreateEnvRequest {
    name: String,
}

/// List all environments in the current app.
async fn list_envs(ctx: CommandContext, args: ListEnvsArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;

    let mut path = format!("/v1/orgs/{}/apps/{}/envs?limit={}", org, app, args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListEnvsResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

/// Create a new environment.
async fn create_env(ctx: CommandContext, args: CreateEnvArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;

    let request = CreateEnvRequest {
        name: args.name.clone(),
    };
    let path = format!("/v1/orgs/{}/apps/{}/envs", org, app);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("envs.create", &path, &request)?,
    };

    let response: EnvResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created environment '{}' ({}) in {}/{}",
                response.name, response.id, org, app
            ));
        }
    }

    Ok(())
}

/// Get environment details.
async fn get_env(ctx: CommandContext, args: GetEnvArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org, app, &args.env).await?;

    let response: EnvResponse = client
        .get(&format!("/v1/orgs/{}/apps/{}/envs/{}", org, app, env_id))
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
