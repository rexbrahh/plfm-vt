//! Authentication commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;

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
async fn login(_ctx: CommandContext, args: LoginArgs) -> Result<()> {
    let token = if let Some(token) = args.token {
        token
    } else {
        // Interactive login would go here
        // For now, prompt for token
        print_info("Interactive login not yet implemented.");
        print_info("Use --token or set VT_TOKEN environment variable.");
        return Ok(());
    };

    // Validate token by making an API call
    // For now, just save it
    let creds = Credentials::new(token);
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
    match ctx.credentials {
        Some(creds) => {
            if let Some(email) = creds.email {
                println!("{}", email);
            } else if let Some(user_id) = creds.user_id {
                println!("{}", user_id);
            } else {
                println!("(authenticated, but user details not available)");
            }
        }
        None => {
            println!("Not logged in");
        }
    }

    Ok(())
}
