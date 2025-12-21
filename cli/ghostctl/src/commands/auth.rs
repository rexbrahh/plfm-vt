//! Authentication commands.

use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::config::Credentials;
use crate::error::CliError;
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

#[derive(Debug, Serialize)]
struct DeviceStartRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceStartResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in_seconds: i64,
    poll_interval_seconds: u32,
}

#[derive(Debug, Serialize)]
struct DeviceTokenRequest {
    device_code: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    token_type: String,
    expires_in_seconds: i64,
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
    let mut creds = if let Some(token) = args.token {
        Credentials::new(token)
    } else {
        let token = device_login(&ctx).await?;
        let mut creds = Credentials::new(token.access_token);
        creds.refresh_token = token.refresh_token;
        if token.expires_in_seconds > 0 {
            creds.expires_at = Some(Utc::now() + ChronoDuration::seconds(token.expires_in_seconds));
        }
        creds
    };

    let client = crate::client::ApiClient::new(&ctx.config, Some(&creds))?;
    let whoami: WhoAmIResponse = client.get("/v1/auth/whoami").await?;
    creds.user_id = Some(whoami.subject_id);
    creds.email = whoami.display_name;

    creds.save()?;

    print_success("Logged in successfully.");
    Ok(())
}

async fn device_login(ctx: &CommandContext) -> Result<TokenResponse> {
    let client = crate::client::ApiClient::new(&ctx.config, None)?;
    let start: DeviceStartResponse = client
        .post_with_idempotency_key(
            "/v1/auth/device/start",
            &DeviceStartRequest {
                device_name: Some("vt-cli".to_string()),
            },
            None,
        )
        .await?;

    print_info(&format!(
        "Visit {} and enter code {}",
        start.verification_uri, start.user_code
    ));

    if let Some(complete) = start.verification_uri_complete.as_deref() {
        print_info(&format!("Or open: {complete}"));
    }

    let expires_in = start.expires_in_seconds.max(0) as u64;
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    let mut poll_interval = start.poll_interval_seconds.max(1) as u64;

    loop {
        if Instant::now() >= deadline {
            anyhow::bail!("Device code expired. Run `vt auth login` again.");
        }

        let token_result: Result<TokenResponse, CliError> = client
            .post_with_idempotency_key(
                "/v1/auth/device/token",
                &DeviceTokenRequest {
                    device_code: start.device_code.clone(),
                },
                None,
            )
            .await;

        match token_result {
            Ok(token) => return Ok(token),
            Err(CliError::Api { code, message, .. }) => match code.as_str() {
                "authorization_pending" => {}
                "slow_down" => {
                    poll_interval = poll_interval.saturating_add(5);
                }
                "access_denied" | "expired_token" | "invalid_grant" => {
                    return Err(anyhow::anyhow!("Login failed: {message}"));
                }
                _ => {
                    return Err(anyhow::anyhow!("Login failed: {message}"));
                }
            },
            Err(err) => return Err(err.into()),
        }

        sleep(Duration::from_secs(poll_interval)).await;
    }
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
