//! Organization commands.

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{
    print_output, print_receipt, print_receipt_no_resource, print_single, print_success,
    OutputFormat, Receipt, ReceiptNextStep, ReceiptNoResource,
};

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

    #[command(about = "Update organization")]
    Update(UpdateOrgArgs),

    /// Get organization details.
    Get(GetOrgArgs),

    /// Set the default organization in local context.
    Use(UseOrgArgs),

    /// Manage organization members.
    Members(MembersCommand),
}

#[derive(Debug, Args)]
struct CreateOrgArgs {
    /// Organization name.
    name: String,
}

#[derive(Debug, Args)]
struct UpdateOrgArgs {
    org: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    expected_version: i32,
}

#[derive(Debug, Args)]
struct GetOrgArgs {
    /// Organization ID or name.
    org: String,
}

#[derive(Debug, Args)]
struct UseOrgArgs {
    /// Organization ID or name.
    org: String,
}

impl OrgsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            OrgsSubcommand::List => list_orgs(ctx).await,
            OrgsSubcommand::Create(args) => create_org(ctx, args).await,
            OrgsSubcommand::Update(args) => update_org(ctx, args).await,
            OrgsSubcommand::Get(args) => get_org(ctx, args).await,
            OrgsSubcommand::Use(args) => use_org(ctx, args).await,
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

#[derive(Debug, Serialize)]
struct UpdateOrgRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    expected_version: i32,
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
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

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
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

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

    let org_id_str = org_id.to_string();
    let member_id = response.id.clone();
    let member_email = response.email.clone();
    let member_role = response.role.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt orgs members list --org {}", org_id_str.clone()),
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
                "Added member '{}' ({}) to org {} as {}",
                member_email,
                member_id.as_str(),
                org_id_str.as_str(),
                member_role
            ),
            status: "accepted",
            kind: "orgs.members.add",
            resource_key: "member",
            resource: &response,
            ids: serde_json::json!({
                "org_id": org_id_str,
                "member_id": member_id
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn update_member(ctx: CommandContext, args: UpdateMemberArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

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

    let org_id_str = org_id.to_string();
    let member_id = response.id.clone();
    let member_email = response.email.clone();
    let member_role = response.role.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt orgs members list --org {}", org_id_str.clone()),
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
                "Updated member '{}' ({}) in org {} to {}",
                member_email,
                member_id.as_str(),
                org_id_str.as_str(),
                member_role
            ),
            status: "accepted",
            kind: "orgs.members.update",
            resource_key: "member",
            resource: &response,
            ids: serde_json::json!({
                "org_id": org_id_str,
                "member_id": member_id
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn remove_member(ctx: CommandContext, args: RemoveMemberArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

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

    let org_id_str = org_id.to_string();
    let member_id = args.member_id.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt orgs members list --org {}", org_id_str.clone()),
        },
        ReceiptNextStep {
            label: "Debug",
            cmd: format!("vt events tail --org {}", org_id_str.clone()),
        },
    ];

    print_receipt_no_resource(
        ctx.format,
        ReceiptNoResource {
            message: format!("Removed member {} from org {}", member_id, org_id_str),
            status: "accepted",
            kind: "orgs.members.remove",
            ids: serde_json::json!({
                "org_id": org_id_str,
                "member_id": member_id
            }),
            next: &next,
        },
    );

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

    let org_id = response.id.clone();
    let org_name = response.name.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt orgs get {}", org_id.clone()),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt projects create --org {} <project-name>", org_id.clone()),
        },
        ReceiptNextStep {
            label: "Debug",
            cmd: format!("vt events tail --org {}", org_id.clone()),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!("Created organization '{}' ({})", org_name, org_id.as_str()),
            status: "accepted",
            kind: "orgs.create",
            resource_key: "org",
            resource: &response,
            ids: serde_json::json!({ "org_id": org_id }),
            next: &next,
        },
    );

    Ok(())
}

async fn update_org(ctx: CommandContext, args: UpdateOrgArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, &args.org).await?;

    let request = UpdateOrgRequest {
        name: args.name.clone(),
        expected_version: args.expected_version,
    };
    let path = format!("/v1/orgs/{}", org_id);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("orgs.update", &path, &request)?,
    };

    let response: OrgResponse = client
        .patch_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Organization '{}' not found", args.org))
            }
            other => other,
        })?;

    let org_id_str = org_id.to_string();
    let org_name = response.name.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt orgs get {}", org_id_str.clone()),
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
                "Updated organization '{}' ({})",
                org_name,
                org_id_str.as_str()
            ),
            status: "accepted",
            kind: "orgs.update",
            resource_key: "org",
            resource: &response,
            ids: serde_json::json!({ "org_id": org_id_str }),
            next: &next,
        },
    );

    Ok(())
}

/// Get organization details.
async fn get_org(ctx: CommandContext, args: GetOrgArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, &args.org).await?;

    let response: OrgResponse = client
        .get(&format!("/v1/orgs/{}", org_id))
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

/// Set the default organization context.
async fn use_org(mut ctx: CommandContext, args: UseOrgArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, &args.org).await?;

    ctx.config.context.org = Some(org_id.to_string());
    ctx.config.context.app = None;
    ctx.config.context.env = None;
    ctx.config.save()?;

    match ctx.format {
        OutputFormat::Json => print_single(
            &serde_json::json!({
                "ok": true,
                "org_id": org_id,
                "app_id": null,
                "env_id": null
            }),
            ctx.format,
        ),
        OutputFormat::Table => {
            print_success(&format!("Set default org to {} (cleared app/env)", org_id));
        }
    }

    Ok(())
}
