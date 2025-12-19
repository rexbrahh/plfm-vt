//! Authentication commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::config::Credentials;
use crate::output::{print_info, print_success};

use super::CommandContext;

/// Authentication commands.
#[derive(Debug, Args)]
pub struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
enum AuthSubcommand {
    /// Log in to the platform.
    Login(LoginArgs),

    /// Log out from the platform.
    Logout,

    /// Show current authentication status.
    Status,

    /// Show who you are logged in as.
    Whoami,
}

#[derive(Debug, Args)]
struct LoginArgs {
    /// API token (for non-interactive login).
    #[arg(long, env = "VT_TOKEN")]
    token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OrgMembership {
    org_id: String,
    role: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WhoAmIResponse {
    subject_type: String,
    subject_id: String,
    #[serde(default)]
    display_name: Option<String>,
    org_memberships: Vec<OrgMembership>,
    scopes: Vec<String>,
}

impl AuthCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            AuthSubcommand::Login(args) => login(ctx, args).await,
            AuthSubcommand::Logout => logout(ctx).await,
            AuthSubcommand::Status => status(ctx).await,
            AuthSubcommand::Whoami => whoami(ctx).await,
        }
    }
}

/// Log in to the platform.
async fn login(ctx: CommandContext, args: LoginArgs) -> Result<()> {
    let token = if let Some(token) = args.token {
        token
    } else {
        // Interactive login would go here
        // For now, prompt for token
        print_info("Interactive login not yet implemented.");
        print_info("Use --token or set VT_TOKEN environment variable.");
        return Ok(());
    };

    let mut creds = Credentials::new(token);

    // Validate token and fetch identity.
    let client = crate::client::ApiClient::new(&ctx.config, Some(&creds))?;
    let whoami: WhoAmIResponse = client.get("/v1/auth/whoami").await?;
    creds.user_id = Some(whoami.subject_id);

    creds.save()?;

    print_success("Logged in successfully.");
    Ok(())
}

/// Log out from the platform.
async fn logout(_ctx: CommandContext) -> Result<()> {
    Credentials::delete()?;
    print_success("Logged out successfully.");
    Ok(())
}

/// Show authentication status.
async fn status(ctx: CommandContext) -> Result<()> {
    match ctx.credentials {
        Some(creds) => {
            println!("{} Authenticated", "Status:".green().bold());

            if let Some(email) = &creds.email {
                println!("  Email: {}", email);
            }

            if let Some(user_id) = &creds.user_id {
                println!("  User ID: {}", user_id);
            }

            if creds.is_expired() {
                println!(
                    "  {} Token has expired. Run `vt auth login`.",
                    "Warning:".yellow()
                );
            } else if let Some(expires_at) = creds.expires_at {
                println!("  Expires: {}", expires_at);
            }
        }
        None => {
            println!("{} Not authenticated", "Status:".red().bold());
            println!("\nRun {} to log in.", "vt auth login".cyan());
        }
    }

    Ok(())
}

/// Show who you are logged in as.
async fn whoami(ctx: CommandContext) -> Result<()> {
    let client = ctx.client()?;
    let whoami: WhoAmIResponse = client.get("/v1/auth/whoami").await?;

    match ctx.format {
        crate::output::OutputFormat::Json => crate::output::print_single(&whoami, ctx.format),
        crate::output::OutputFormat::Table => {
            if let Some(display_name) = whoami.display_name.as_deref() {
                println!("{display_name}");
            } else {
                println!("{}", whoami.subject_id);
            }
        }
    };

    Ok(())
}
