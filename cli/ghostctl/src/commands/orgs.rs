//! Organization commands.

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
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

    /// Manage organization members.
    Members(MembersCommand),
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
            OrgsSubcommand::Members(cmd) => cmd.run(ctx).await,
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

// =============================================================================
// Org Members
// =============================================================================

#[derive(Debug, Args)]
struct MembersCommand {
    #[command(subcommand)]
    command: MembersSubcommand,
}

#[derive(Debug, Subcommand)]
enum MembersSubcommand {
    /// List org members.
    List(ListMembersArgs),

    /// Add an org member (admin only).
    Add(AddMemberArgs),

    /// Update an org member role (admin only).
    Update(UpdateMemberArgs),

    /// Remove an org member (admin only).
    Remove(RemoveMemberArgs),
}

#[derive(Debug, Args)]
struct ListMembersArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct AddMemberArgs {
    /// Member email.
    email: String,

    /// Member role.
    #[arg(long, value_enum, default_value = "developer")]
    role: MemberRoleArg,
}

#[derive(Debug, Args)]
struct UpdateMemberArgs {
    /// Member ID.
    member_id: String,

    /// New member role.
    #[arg(long, value_enum)]
    role: MemberRoleArg,

    /// Expected resource version (for optimistic concurrency).
    #[arg(long)]
    expected_version: i32,
}

#[derive(Debug, Args)]
struct RemoveMemberArgs {
    /// Member ID.
    member_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum MemberRoleArg {
    Owner,
    Admin,
    Developer,
    Readonly,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct MemberResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Email")]
    email: String,

    #[tabled(rename = "Role")]
    role: String,

    #[tabled(rename = "Ver")]
    resource_version: i32,

    #[tabled(rename = "Updated")]
    updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListMembersResponse {
    items: Vec<MemberResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateMemberRequest {
    email: String,
    role: MemberRoleArg,
}

#[derive(Debug, Serialize)]
struct UpdateMemberRequest {
    role: MemberRoleArg,
    expected_version: i32,
}

#[derive(Debug, Serialize)]
struct DeleteResponse {
    ok: bool,
}

impl MembersCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            MembersSubcommand::List(args) => list_members(ctx, args).await,
            MembersSubcommand::Add(args) => add_member(ctx, args).await,
            MembersSubcommand::Update(args) => update_member(ctx, args).await,
            MembersSubcommand::Remove(args) => remove_member(ctx, args).await,
        }
    }
}

async fn list_members(ctx: CommandContext, args: ListMembersArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let mut path = format!("/v1/orgs/{org_id}/members?limit={}", args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListMembersResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }

    Ok(())
}

async fn add_member(ctx: CommandContext, args: AddMemberArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let request = CreateMemberRequest {
        email: args.email,
        role: args.role,
    };
    let path = format!("/v1/orgs/{org_id}/members");
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("members.create", &path, &request)?,
    };

    let response: MemberResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Added member '{}' ({}) to org {} as {}",
                response.email, response.id, org_id, response.role
            ));
        }
    }

    Ok(())
}

async fn update_member(ctx: CommandContext, args: UpdateMemberArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    if args.expected_version < 0 {
        return Err(anyhow::anyhow!("expected_version must be >= 0"));
    }

    let request = UpdateMemberRequest {
        role: args.role,
        expected_version: args.expected_version,
    };
    let path = format!("/v1/orgs/{org_id}/members/{}", args.member_id);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("members.update", &path, &request)?,
    };

    let response: MemberResponse = client
        .patch_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Updated member '{}' ({}) in org {} to {}",
                response.email, response.id, org_id, response.role
            ));
        }
    }

    Ok(())
}

async fn remove_member(ctx: CommandContext, args: RemoveMemberArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let request_hash_input = serde_json::json!({
        "member_id": &args.member_id
    });

    let path = format!("/v1/orgs/{org_id}/members/{}", args.member_id);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key(
            "members.delete",
            &path,
            &request_hash_input,
        )?,
    };

    client
        .delete_with_idempotency_key(&path, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => {
            let response = DeleteResponse { ok: true };
            print_single(&response, ctx.format);
        }
        OutputFormat::Table => {
            print_success(&format!(
                "Removed member {} from org {}",
                args.member_id, org_id
            ));
        }
    }

    Ok(())
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

    let request = CreateOrgRequest {
        name: args.name.clone(),
    };
    let path = "/v1/orgs";
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("orgs.create", path, &request)?,
    };
    let response: OrgResponse = client
        .post_with_idempotency_key(path, &request, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created organization '{}' ({})",
                response.name, response.id
            ));
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
