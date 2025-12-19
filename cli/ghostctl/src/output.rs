//! Output formatting for CLI commands.

use colored::Colorize;
use serde::Serialize;
use tabled::{Table, Tabled};

/// Output format.
#[derive(Debug, Clone, Copy, Default)]
pub enum OutputFormat {
    /// Human-readable table format.
    #[default]
    Table,
    /// JSON format.
    Json,
}

/// Print data in the specified format.
pub fn print_output<T: Serialize + Tabled>(data: &[T], format: OutputFormat) {
    match format {
        OutputFormat::Table => {
            if data.is_empty() {
                println!("{}", "No items found.".dimmed());
            } else {
                let table = Table::new(data).to_string();
                println!("{}", table);
            }
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "[]".to_string());
            println!("{}", json);
        }
    }
}

/// Print a single item in the specified format.
pub fn print_single<T: Serialize>(data: &T, format: OutputFormat) {
    match format {
        OutputFormat::Table => {
            let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
            println!("{}", json);
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
            println!("{}", json);
        }
    }
}

/// Print a success message.
pub fn print_success(message: &str) {
    println!("{} {}", "Success:".green().bold(), message);
}

/// Print an info message.
pub fn print_info(message: &str) {
    println!("{} {}", "Info:".blue().bold(), message);
}
