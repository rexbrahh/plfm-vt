//! Project commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Project commands.
#[derive(Debug, Args)]
pub struct ProjectsCommand {
    #[command(subcommand)]
    command: ProjectsSubcommand,
}

#[derive(Debug, Subcommand)]
enum ProjectsSubcommand {
    /// List projects in an org.
    List(ListProjectsArgs),

    /// Create a new project.
    Create(CreateProjectArgs),

    /// Get project details.
    Get(GetProjectArgs),
}

#[derive(Debug, Args)]
struct ListProjectsArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct CreateProjectArgs {
    /// Project name.
    name: String,
}

#[derive(Debug, Args)]
struct GetProjectArgs {
    /// Project ID.
    project: String,
}

impl ProjectsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            ProjectsSubcommand::List(args) => list_projects(ctx, args).await,
            ProjectsSubcommand::Create(args) => create_project(ctx, args).await,
            ProjectsSubcommand::Get(args) => get_project(ctx, args).await,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct ProjectResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Org")]
    org_id: String,

    #[tabled(rename = "Name")]
    name: String,

    #[tabled(rename = "Ver")]
    resource_version: i32,

    #[tabled(rename = "Updated")]
    updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListProjectsResponse {
    items: Vec<ProjectResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateProjectRequest {
    name: String,
}

async fn list_projects(ctx: CommandContext, args: ListProjectsArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let mut path = format!("/v1/orgs/{org_id}/projects?limit={}", args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListProjectsResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }

    Ok(())
}

async fn create_project(ctx: CommandContext, args: CreateProjectArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let request = CreateProjectRequest { name: args.name };
    let path = format!("/v1/orgs/{org_id}/projects");
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("projects.create", &path, &request)?,
    };

    let response: ProjectResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created project '{}' ({}) in org {}",
                response.name, response.id, org_id
            ));
        }
    }

    Ok(())
}

async fn get_project(ctx: CommandContext, args: GetProjectArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let response: ProjectResponse = client
        .get(&format!("/v1/orgs/{}/projects/{}", org_id, args.project))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Project '{}' not found", args.project))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}
