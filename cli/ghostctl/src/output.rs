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

#[derive(Debug, Serialize)]
pub struct ReceiptNextStep {
    pub label: &'static str,
    pub cmd: String,
}

pub fn receipt_value<T: Serialize>(
    status: &str,
    kind: &str,
    resource_key: &str,
    resource: &T,
    ids: serde_json::Value,
    next: &[ReceiptNextStep],
) -> serde_json::Value {
    let mut receipt = serde_json::Map::new();
    receipt.insert("kind".to_string(), serde_json::json!(kind));
    receipt.insert("status".to_string(), serde_json::json!(status));
    receipt.insert("ids".to_string(), ids);
    receipt.insert(
        "next".to_string(),
        serde_json::to_value(next).unwrap_or_else(|_| serde_json::json!([])),
    );
    receipt.insert(
        resource_key.to_string(),
        serde_json::to_value(resource).unwrap_or_else(|_| serde_json::json!({})),
    );
    serde_json::json!({ "receipt": receipt })
}

pub fn receipt_value_no_resource(
    status: &str,
    kind: &str,
    ids: serde_json::Value,
    next: &[ReceiptNextStep],
) -> serde_json::Value {
    let mut receipt = serde_json::Map::new();
    receipt.insert("kind".to_string(), serde_json::json!(kind));
    receipt.insert("status".to_string(), serde_json::json!(status));
    receipt.insert("ids".to_string(), ids);
    receipt.insert(
        "next".to_string(),
        serde_json::to_value(next).unwrap_or_else(|_| serde_json::json!([])),
    );
    serde_json::json!({ "receipt": receipt })
}

pub fn print_receipt<T: Serialize>(
    format: OutputFormat,
    message: &str,
    status: &str,
    kind: &str,
    resource_key: &str,
    resource: &T,
    ids: serde_json::Value,
    next: &[ReceiptNextStep],
) {
    match format {
        OutputFormat::Table => {
            print_success(message);
            for step in next {
                print_info(&format!("{}: {}", step.label, step.cmd));
            }
        }
        OutputFormat::Json => {
            let out = receipt_value(status, kind, resource_key, resource, ids, next);
            print_single(&out, OutputFormat::Json);
        }
    }
}

pub fn print_receipt_no_resource(
    format: OutputFormat,
    message: &str,
    status: &str,
    kind: &str,
    ids: serde_json::Value,
    next: &[ReceiptNextStep],
) {
    match format {
        OutputFormat::Table => {
            print_success(message);
            for step in next {
                print_info(&format!("{}: {}", step.label, step.cmd));
            }
        }
        OutputFormat::Json => {
            let out = receipt_value_no_resource(status, kind, ids, next);
            print_single(&out, OutputFormat::Json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_value_includes_resource_and_next_steps() {
        let resource = serde_json::json!({ "id": "org_123", "name": "acme" });
        let next = vec![ReceiptNextStep {
            label: "Next",
            cmd: "vt orgs get org_123".to_string(),
        }];
        let value = receipt_value(
            "accepted",
            "orgs.create",
            "org",
            &resource,
            serde_json::json!({ "org_id": "org_123" }),
            &next,
        );
        let expected = serde_json::json!({
            "receipt": {
                "kind": "orgs.create",
                "status": "accepted",
                "ids": { "org_id": "org_123" },
                "next": [{ "label": "Next", "cmd": "vt orgs get org_123" }],
                "org": { "id": "org_123", "name": "acme" }
            }
        });
        assert_eq!(value, expected);
    }

    #[test]
    fn receipt_value_no_resource_includes_next_steps() {
        let next = vec![ReceiptNextStep {
            label: "Next",
            cmd: "vt orgs list".to_string(),
        }];
        let value = receipt_value_no_resource(
            "accepted",
            "orgs.delete",
            serde_json::json!({ "org_id": "org_123" }),
            &next,
        );
        let expected = serde_json::json!({
            "receipt": {
                "kind": "orgs.delete",
                "status": "accepted",
                "ids": { "org_id": "org_123" },
                "next": [{ "label": "Next", "cmd": "vt orgs list" }]
            }
        });
        assert_eq!(value, expected);
    }
}
