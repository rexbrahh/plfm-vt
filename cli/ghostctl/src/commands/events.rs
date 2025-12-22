//! Events command (org-scoped event querying/tailing).

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
    #[tabled(display = "display_option")]
    #[serde(default)]
    aggregate_type: Option<String>,

    #[tabled(rename = "Agg ID")]
    #[tabled(display = "display_option")]
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

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct EventStreamLine {
    ts: String,
    seq: i64,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    aggregate_type: Option<String>,
    #[serde(default)]
    aggregate_id: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    env_id: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
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
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let app_ident = args
        .app_id
        .or_else(|| ctx.resolve_app().map(|s| s.to_string()));
    let env_ident = args
        .env_id
        .or_else(|| ctx.resolve_env().map(|s| s.to_string()));

    let app_id = match app_ident.as_deref() {
        None => None,
        Some(ident) => Some(crate::resolve::resolve_app_id(&client, org_id, ident).await?),
    };

    let env_id = match env_ident.as_deref() {
        None => None,
        Some(ident) => match app_id {
            Some(app_id) => {
                Some(crate::resolve::resolve_env_id(&client, org_id, app_id, ident).await?)
            }
            None => {
                if let Ok(id) = ident.parse::<plfm_id::EnvId>() {
                    Some(id)
                } else {
                    anyhow::bail!(
                        "Resolving env name '{}' requires app context (use --app or --app-id).",
                        ident
                    );
                }
            }
        },
    };

    let mut path = format!(
        "/v1/orgs/{}/events?after_event_id={}&limit={}",
        org_id, args.after, args.limit
    );

    if let Some(event_type) = args.event_type.as_deref() {
        path.push_str(&format!("&event_type={event_type}"));
    }
    if let Some(app_id) = app_id.as_ref() {
        path.push_str(&format!("&app_id={app_id}"));
    }
    if let Some(env_id) = env_id.as_ref() {
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
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let app_ident = args
        .app_id
        .or_else(|| ctx.resolve_app().map(|s| s.to_string()));
    let env_ident = args
        .env_id
        .or_else(|| ctx.resolve_env().map(|s| s.to_string()));

    let app_id = match app_ident.as_deref() {
        None => None,
        Some(ident) => Some(crate::resolve::resolve_app_id(&client, org_id, ident).await?),
    };

    let env_id = match env_ident.as_deref() {
        None => None,
        Some(ident) => match app_id {
            Some(app_id) => {
                Some(crate::resolve::resolve_env_id(&client, org_id, app_id, ident).await?)
            }
            None => {
                if let Ok(id) = ident.parse::<plfm_id::EnvId>() {
                    Some(id)
                } else {
                    anyhow::bail!(
                        "Resolving env name '{}' requires app context (use --app or --app-id).",
                        ident
                    );
                }
            }
        },
    };

    let mut path = format!(
        "/v1/orgs/{}/events/stream?after_event_id={}&limit={}",
        org_id, args.after, args.limit
    );

    if let Some(event_type) = args.event_type.as_deref() {
        path.push_str(&format!("&event_type={event_type}"));
    }
    if let Some(app_id) = app_id.as_ref() {
        path.push_str(&format!("&app_id={app_id}"));
    }
    if let Some(env_id) = env_id.as_ref() {
        path.push_str(&format!("&env_id={env_id}"));
    }
    path.push_str(&format!("&poll_ms={}", args.poll_ms.max(100)));

    let mut response = client.get_ndjson_stream(&path).await?;
    let mut buffer = String::new();

    loop {
        let chunk = response.chunk().await?;
        let Some(chunk) = chunk else { break };

        buffer.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));

        while let Some(delim) = buffer.find('\n') {
            let line = buffer[..delim].trim().to_string();
            buffer.drain(..delim + 1);

            if line.is_empty() {
                continue;
            }

            match ctx.format {
                OutputFormat::Json => println!("{}", line),
                OutputFormat::Table => {
                    if let Ok(event) = serde_json::from_str::<EventStreamLine>(&line) {
                        let agg = match (&event.aggregate_type, &event.aggregate_id) {
                            (Some(t), Some(id)) => format!("{}/{}", t, id),
                            _ => "-".to_string(),
                        };
                        println!("{}\t{}\t{}\t{}", event.seq, event.ts, event.event_type, agg);
                    }
                }
            }
        }
    }

    Ok(())
}
