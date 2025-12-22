//! Logs command (view application logs).

use anyhow::Result;
use clap::Args;
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::output::{print_single, OutputFormat};

use super::CommandContext;

/// Logs command - view application logs.
#[derive(Debug, Args)]
pub struct LogsCommand {
    /// Process type to filter logs (optional).
    #[arg(long, short)]
    process: Option<String>,

    /// Instance ID to filter logs (optional).
    #[arg(long, short)]
    instance: Option<String>,

    /// Number of lines to show (default: 100).
    #[arg(long, short, default_value = "100")]
    lines: u32,

    /// Follow logs in real-time.
    #[arg(long, short)]
    follow: bool,

    /// Show timestamps.
    #[arg(long, short)]
    timestamps: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct LogLine {
    ts: String,
    #[serde(rename = "type", default)]
    entry_type: Option<String>,
    #[serde(default)]
    instance_id: Option<String>,
    #[serde(default)]
    process_type: Option<String>,
    #[serde(default)]
    stream: Option<String>,
    line: String,
    #[serde(default)]
    truncated: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LogsResponse {
    items: Vec<LogLine>,
}

impl LogsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        let org_ident = ctx.require_org()?;
        let app_ident = ctx.require_app()?;
        let env_ident = ctx.resolve_env().ok_or_else(|| {
            anyhow::anyhow!("No environment specified. Use --env or set a default context.")
        })?;

        let client = ctx.client()?;
        let org_id = crate::resolve::resolve_org_id(&client, org_ident).await?;
        let app_id = crate::resolve::resolve_app_id(&client, org_id, app_ident).await?;
        let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env_ident).await?;

        if self.follow {
            let mut path = format!(
                "/v1/orgs/{}/apps/{}/envs/{}/logs/stream",
                org_id, app_id, env_id
            );

            let mut has_query = false;
            if let Some(process_type) = self.process.as_deref() {
                path.push_str(if has_query { "&" } else { "?" });
                has_query = true;
                path.push_str(&format!("process_type={process_type}"));
            }
            if let Some(instance_id) = self.instance.as_deref() {
                path.push_str(if has_query { "&" } else { "?" });
                path.push_str(&format!("instance_id={instance_id}"));
            }

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
                            if let Ok(log) = serde_json::from_str::<LogLine>(&line) {
                                print_log_line(&log, self.timestamps);
                            }
                        }
                    }
                }
            }

            return Ok(());
        }

        let mut path = format!(
            "/v1/orgs/{}/apps/{}/envs/{}/logs?tail_lines={}",
            org_id, app_id, env_id, self.lines
        );

        if let Some(process_type) = self.process.as_deref() {
            path.push_str(&format!("&process_type={process_type}"));
        }
        if let Some(instance_id) = self.instance.as_deref() {
            path.push_str(&format!("&instance_id={instance_id}"));
        }

        let response: LogsResponse = client.get(&path).await?;
        if matches!(ctx.format, OutputFormat::Json) {
            print_single(&response, OutputFormat::Json);
            return Ok(());
        }

        if response.items.is_empty() {
            println!("{}", "No items found.".dimmed());
            return Ok(());
        }

        for line in response.items {
            print_log_line(&line, self.timestamps);
        }

        Ok(())
    }
}

fn print_log_line(line: &LogLine, timestamps: bool) {
    let mut prefix_parts: Vec<&str> = Vec::new();
    if timestamps {
        prefix_parts.push(line.ts.as_str());
    }
    if let Some(instance_id) = line.instance_id.as_deref() {
        prefix_parts.push(instance_id);
    }
    if let Some(process_type) = line.process_type.as_deref() {
        prefix_parts.push(process_type);
    }

    if prefix_parts.is_empty() {
        println!("{}", line.line);
    } else {
        println!("{} {}", prefix_parts.join(" "), line.line);
    }
}
