//! Events command (org-scoped event querying/tailing).

use std::time::Duration;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::output::{print_output, print_single, OutputFormat};

use super::CommandContext;

/// Events command.
#[derive(Debug, Args)]
pub struct EventsCommand {
    #[command(subcommand)]
    command: EventsSubcommand,
}

#[derive(Debug, Subcommand)]
enum EventsSubcommand {
    /// List events (one-shot).
    List(EventsListArgs),

    /// Tail events (polling).
    Tail(EventsTailArgs),
}

#[derive(Debug, Args)]
struct EventsListArgs {
    /// Return events with event_id > after_event_id.
    #[arg(long, default_value = "0")]
    after: i64,

    /// Max number of events to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Filter by exact event type.
    #[arg(long)]
    event_type: Option<String>,

    /// Filter by app_id (defaults to current context if set).
    #[arg(long)]
    app_id: Option<String>,

    /// Filter by env_id (defaults to current context if set).
    #[arg(long)]
    env_id: Option<String>,
}

#[derive(Debug, Args)]
struct EventsTailArgs {
    /// Return events with event_id > after_event_id.
    #[arg(long, default_value = "0")]
    after: i64,

    /// Max number of events to fetch per poll (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Filter by exact event type.
    #[arg(long)]
    event_type: Option<String>,

    /// Filter by app_id (defaults to current context if set).
    #[arg(long)]
    app_id: Option<String>,

    /// Filter by env_id (defaults to current context if set).
    #[arg(long)]
    env_id: Option<String>,

    /// Poll interval in milliseconds.
    #[arg(long, default_value = "1000")]
    poll_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct EventRow {
    #[tabled(rename = "ID")]
    event_id: i64,

    #[tabled(rename = "Occurred At")]
    occurred_at: String,

    #[tabled(rename = "Type")]
    event_type: String,

    #[tabled(rename = "Agg Type")]
    #[tabled(display_with = "display_option")]
    #[serde(default)]
    aggregate_type: Option<String>,

    #[tabled(rename = "Agg ID")]
    #[tabled(display_with = "display_option")]
    #[serde(default)]
    aggregate_id: Option<String>,
}

fn display_option(opt: &Option<String>) -> String {
    opt.as_deref().unwrap_or("-").to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct EventsResponse {
    items: Vec<EventRow>,
    next_after_event_id: i64,
}

impl EventsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            EventsSubcommand::List(args) => list_events(ctx, args).await,
            EventsSubcommand::Tail(args) => tail_events(ctx, args).await,
        }
    }
}

async fn list_events(ctx: CommandContext, args: EventsListArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let app_id = args
        .app_id
        .or_else(|| ctx.resolve_app().map(|s| s.to_string()));
    let env_id = args
        .env_id
        .or_else(|| ctx.resolve_env().map(|s| s.to_string()));

    let mut path = format!(
        "/v1/orgs/{}/events?after_event_id={}&limit={}",
        org_id, args.after, args.limit
    );

    if let Some(event_type) = args.event_type.as_deref() {
        path.push_str(&format!("&event_type={event_type}"));
    }
    if let Some(app_id) = app_id.as_deref() {
        path.push_str(&format!("&app_id={app_id}"));
    }
    if let Some(env_id) = env_id.as_deref() {
        path.push_str(&format!("&env_id={env_id}"));
    }

    let response: EventsResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }

    Ok(())
}

async fn tail_events(ctx: CommandContext, args: EventsTailArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let app_id = args
        .app_id
        .or_else(|| ctx.resolve_app().map(|s| s.to_string()));
    let env_id = args
        .env_id
        .or_else(|| ctx.resolve_env().map(|s| s.to_string()));

    let mut after_event_id = args.after;
    let poll = Duration::from_millis(args.poll_ms.max(100));

    loop {
        let mut path = format!(
            "/v1/orgs/{}/events?after_event_id={}&limit={}",
            org_id, after_event_id, args.limit
        );

        if let Some(event_type) = args.event_type.as_deref() {
            path.push_str(&format!("&event_type={event_type}"));
        }
        if let Some(app_id) = app_id.as_deref() {
            path.push_str(&format!("&app_id={app_id}"));
        }
        if let Some(env_id) = env_id.as_deref() {
            path.push_str(&format!("&env_id={env_id}"));
        }

        let response: EventsResponse = client.get(&path).await?;

        for event in &response.items {
            match ctx.format {
                OutputFormat::Table => {
                    let agg = match (&event.aggregate_type, &event.aggregate_id) {
                        (Some(t), Some(id)) => format!("{}/{}", t, id),
                        _ => "-".to_string(),
                    };
                    println!(
                        "{}\t{}\t{}\t{}",
                        event.event_id, event.occurred_at, event.event_type, agg
                    );
                }
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string())
                    );
                }
            }
        }

        after_event_id = response.next_after_event_id;
        tokio::time::sleep(poll).await;
    }
}
