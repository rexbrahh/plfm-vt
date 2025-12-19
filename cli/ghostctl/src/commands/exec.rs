//! Exec command.
//!
//! This creates an audited exec grant (session id + connect URL + short-lived token).
//! The token is sensitive and is not printed in table mode unless explicitly requested.

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::output::{print_info, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Create an exec session grant for an instance.
#[derive(Debug, Args)]
pub struct ExecCommand {
    /// Instance ID to exec into.
    pub instance: String,

    /// Allocate a pseudo-terminal (PTY).
    #[arg(long, default_value_t = true)]
    pub tty: bool,

    /// Print the session token in table mode (sensitive).
    #[arg(long)]
    pub show_token: bool,

    /// Command to run (after `--`).
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ExecGrantRequest {
    command: Vec<String>,
    tty: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExecGrantResponse {
    session_id: String,
    connect_url: String,
    session_token: String,
    expires_in_seconds: i64,
}

impl ExecCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        let org_id = ctx.require_org()?;
        let app_id = ctx.require_app()?;
        let env_id = require_env(&ctx)?;
        let client = ctx.client()?;

        let path = format!(
            "/v1/orgs/{}/apps/{}/envs/{}/instances/{}/exec",
            org_id, app_id, env_id, self.instance
        );

        let request = ExecGrantRequest {
            command: self.command.clone(),
            tty: self.tty,
        };

        let idempotency_key = match ctx.idempotency_key.as_deref() {
            Some(key) => key.to_string(),
            None => crate::idempotency::default_idempotency_key("exec.grant", &path, &request)?,
        };

        let response: ExecGrantResponse = client
            .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
            .await?;

        match ctx.format {
            OutputFormat::Json => print_single(&response, ctx.format),
            OutputFormat::Table => {
                print_success(&format!(
                    "Created exec grant session {} (expires in {}s)",
                    response.session_id, response.expires_in_seconds
                ));
                print_info(&format!("connect_url: {}", response.connect_url));
                if self.show_token {
                    print_info(&format!("session_token: {}", response.session_token));
                } else {
                    print_info(
                        "session_token is sensitive; use --show-token or --format json to print it",
                    );
                }
            }
        }

        Ok(())
    }
}

fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })
}
