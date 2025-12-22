//! Project commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{
    print_output, print_proto_single, print_receipt, print_single, OutputFormat, Receipt,
    ReceiptNextStep,
};

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

    #[command(about = "Update project")]
    Update(UpdateProjectArgs),

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
struct UpdateProjectArgs {
    project: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    expected_version: i32,
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
            ProjectsSubcommand::Update(args) => update_project(ctx, args).await,
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

const PROJECT_TYPE_URL: &str = "type.googleapis.com/plfm.controlplane.v1.Project";
const LIST_PROJECTS_TYPE_URL: &str =
    "type.googleapis.com/plfm.controlplane.v1.ListProjectsResponse";

#[derive(Debug, Serialize, Deserialize)]
struct ListProjectsResponse {
    items: Vec<ProjectResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateProjectRequest {
    name: String,
}

#[derive(Debug, Serialize)]
struct UpdateProjectRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    expected_version: i32,
}

async fn list_projects(ctx: CommandContext, args: ListProjectsArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let mut path = format!("/v1/orgs/{org_id}/projects?limit={}", args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListProjectsResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_proto_single(&response, ctx.format, LIST_PROJECTS_TYPE_URL),
    }

    Ok(())
}

async fn create_project(ctx: CommandContext, args: CreateProjectArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let request = CreateProjectRequest { name: args.name };
    let path = format!("/v1/orgs/{org_id}/projects");
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("projects.create", &path, &request)?,
    };

    let response: ProjectResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let project_id = response.id.clone();
    let project_name = response.name.clone();
    let org_id_str = org_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt projects get {}", project_id.clone()),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt apps create --org {} <app-name>", org_id_str.clone()),
        },
        ReceiptNextStep {
            label: "Debug",
            cmd: format!("vt events tail --org {}", org_id_str.clone()),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Created project '{}' ({}) in org {}",
                project_name,
                project_id.as_str(),
                org_id_str.as_str()
            ),
            status: "accepted",
            kind: "projects.create",
            resource_key: "project",
            resource: &response,
            ids: serde_json::json!({
                "project_id": project_id,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn update_project(ctx: CommandContext, args: UpdateProjectArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let request = UpdateProjectRequest {
        name: args.name.clone(),
        expected_version: args.expected_version,
    };
    let path = format!("/v1/orgs/{}/projects/{}", org_id, args.project);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("projects.update", &path, &request)?,
    };

    let response: ProjectResponse = client
        .patch_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Project '{}' not found", args.project))
            }
            other => other,
        })?;

    let project_id = response.id.clone();
    let project_name = response.name.clone();
    let org_id_str = org_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} projects get {}",
                org_id_str.clone(),
                project_id
            ),
        },
        ReceiptNextStep {
            label: "Debug",
            cmd: format!("vt events tail --org {}", org_id_str.clone()),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Updated project '{}' ({})",
                project_name,
                response.id.as_str()
            ),
            status: "accepted",
            kind: "projects.update",
            resource_key: "project",
            resource: &response,
            ids: serde_json::json!({
                "project_id": project_id,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn get_project(ctx: CommandContext, args: GetProjectArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let response: ProjectResponse = client
        .get(&format!("/v1/orgs/{}/projects/{}", org_id, args.project))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Project '{}' not found", args.project))
            }
            other => other,
        })?;

    match ctx.format {
        OutputFormat::Table => print_single(&response, ctx.format),
        OutputFormat::Json => print_proto_single(&response, ctx.format, PROJECT_TYPE_URL),
    }
    Ok(())
}
