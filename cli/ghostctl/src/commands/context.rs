//! Context commands (saved defaults for org/app/env).

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::output::{print_single, print_success, OutputFormat};

use super::CommandContext;

/// Manage saved CLI context (defaults for org/app/env).
#[derive(Debug, Args)]
pub struct ContextCommand {
    #[command(subcommand)]
    command: ContextSubcommand,
}

#[derive(Debug, Subcommand)]
enum ContextSubcommand {
    /// Show the saved context.
    Show,

    /// Clear the saved context.
    Clear,
}

#[derive(Debug, Serialize)]
struct ContextView {
    api_url: String,
    org: Option<String>,
    app: Option<String>,
    env: Option<String>,
}

impl ContextCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            ContextSubcommand::Show => show(ctx).await,
            ContextSubcommand::Clear => clear(ctx).await,
        }
    }
}

async fn show(ctx: CommandContext) -> Result<()> {
    let view = ContextView {
        api_url: ctx.config.api_url.clone(),
        org: ctx.config.context.org.clone(),
        app: ctx.config.context.app.clone(),
        env: ctx.config.context.env.clone(),
    };

    match ctx.format {
        OutputFormat::Json => print_single(&view, ctx.format),
        OutputFormat::Table => {
            println!("api_url: {}", view.api_url);
            println!("org: {}", view.org.as_deref().unwrap_or("-"));
            println!("app: {}", view.app.as_deref().unwrap_or("-"));
            println!("env: {}", view.env.as_deref().unwrap_or("-"));
        }
    }

    Ok(())
}

async fn clear(mut ctx: CommandContext) -> Result<()> {
    ctx.config.context.org = None;
    ctx.config.context.app = None;
    ctx.config.context.env = None;
    ctx.config.save()?;

    match ctx.format {
        OutputFormat::Json => print_single(&serde_json::json!({ "ok": true }), ctx.format),
        OutputFormat::Table => print_success("Cleared saved context"),
    }

    Ok(())
}
