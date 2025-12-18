//! Release commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Release commands.
#[derive(Debug, Args)]
pub struct ReleasesCommand {
    #[command(subcommand)]
    command: ReleasesSubcommand,
}

#[derive(Debug, Subcommand)]
enum ReleasesSubcommand {
    /// List releases for an application.
    List,

    /// Create a new release.
    Create(CreateReleaseArgs),

    /// Get release details.
    Get(GetReleaseArgs),
}

#[derive(Debug, Args)]
struct CreateReleaseArgs {
    /// OCI image reference (e.g., ghcr.io/org/app:v1.0.0).
    image: String,

    /// Optional description.
    #[arg(long)]
    description: Option<String>,
}

#[derive(Debug, Args)]
struct GetReleaseArgs {
    /// Release ID.
    release: String,
}

impl ReleasesCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            ReleasesSubcommand::List => list_releases(ctx).await,
            ReleasesSubcommand::Create(args) => create_release(ctx, args).await,
            ReleasesSubcommand::Get(args) => get_release(ctx, args).await,
        }
    }
}

/// Release response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct ReleaseResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "App ID")]
    app_id: String,

    #[tabled(rename = "Image")]
    image: String,

    #[tabled(rename = "Manifest Hash")]
    manifest_hash: String,

    #[tabled(rename = "Status")]
    status: String,

    #[tabled(rename = "Created")]
    created_at: String,
}

/// List response from API.
#[derive(Debug, Deserialize)]
struct ListReleasesResponse {
    items: Vec<ReleaseResponse>,
    #[allow(dead_code)]
    total: i64,
}

/// Create release request.
#[derive(Debug, Serialize)]
struct CreateReleaseRequest {
    image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// List all releases for the current app.
async fn list_releases(ctx: CommandContext) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let client = ctx.client()?;

    let response: ListReleasesResponse = client
        .get(&format!("/v1/orgs/{}/apps/{}/releases", org, app))
        .await?;

    print_output(&response.items, ctx.format);
    Ok(())
}

/// Create a new release.
async fn create_release(ctx: CommandContext, args: CreateReleaseArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let client = ctx.client()?;

    let request = CreateReleaseRequest {
        image: args.image.clone(),
        description: args.description,
    };

    let response: ReleaseResponse = client
        .post(&format!("/v1/orgs/{}/apps/{}/releases", org, app), &request)
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created release {} for app {} with image {}",
                response.id, app, args.image
            ));
        }
    }

    Ok(())
}

/// Get release details.
async fn get_release(ctx: CommandContext, args: GetReleaseArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let client = ctx.client()?;

    let response: ReleaseResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/releases/{}",
            org, app, args.release
        ))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Release '{}' not found", args.release))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}
