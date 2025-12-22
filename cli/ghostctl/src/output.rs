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
            let json = format_json(data, "[]");
            println!("{}", json);
        }
    }
}

/// Print a single item in the specified format.
pub fn print_single<T: Serialize>(data: &T, format: OutputFormat) {
    match format {
        OutputFormat::Table => {
            let json = format_json(data, "{}");
            println!("{}", json);
        }
        OutputFormat::Json => {
            let json = format_json(data, "{}");
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

pub struct Receipt<'a, T: Serialize> {
    pub message: String,
    pub status: &'a str,
    pub kind: &'a str,
    pub resource_key: &'a str,
    pub resource: &'a T,
    pub ids: serde_json::Value,
    pub next: &'a [ReceiptNextStep],
}

pub struct ReceiptNoResource<'a> {
    pub message: String,
    pub status: &'a str,
    pub kind: &'a str,
    pub ids: serde_json::Value,
    pub next: &'a [ReceiptNextStep],
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

pub fn print_receipt<T: Serialize>(format: OutputFormat, receipt: Receipt<'_, T>) {
    match format {
        OutputFormat::Table => {
            print_success(&receipt.message);
            for step in receipt.next {
                print_info(&format!("{}: {}", step.label, step.cmd));
            }
        }
        OutputFormat::Json => {
            let out = receipt_value(
                receipt.status,
                receipt.kind,
                receipt.resource_key,
                receipt.resource,
                receipt.ids,
                receipt.next,
            );
            print_single(&out, OutputFormat::Json);
        }
    }
}

pub fn print_receipt_no_resource(format: OutputFormat, receipt: ReceiptNoResource<'_>) {
    match format {
        OutputFormat::Table => {
            print_success(&receipt.message);
            for step in receipt.next {
                print_info(&format!("{}: {}", step.label, step.cmd));
            }
        }
        OutputFormat::Json => {
            let out =
                receipt_value_no_resource(receipt.status, receipt.kind, receipt.ids, receipt.next);
            print_single(&out, OutputFormat::Json);
        }
    }
}

fn format_json<T: Serialize + ?Sized>(data: &T, fallback: &str) -> String {
    let value = serde_json::to_value(data).unwrap_or_else(|_| serde_json::json!({}));
    let mapped = to_proto_json_value(value);
    serde_json::to_string_pretty(&mapped).unwrap_or_else(|_| fallback.to_string())
}

fn to_proto_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(to_proto_json_value).collect())
        }
        serde_json::Value::Object(entries) => {
            let mut pairs: Vec<_> = entries.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let mut mapped = serde_json::Map::new();
            for (key, value) in pairs {
                mapped.insert(snake_to_lower_camel(&key), to_proto_json_value(value));
            }
            serde_json::Value::Object(mapped)
        }
        serde_json::Value::Number(number) => stringify_large_number(number),
        other => other,
    }
}

fn stringify_large_number(number: serde_json::Number) -> serde_json::Value {
    if let Some(value) = number.as_i64() {
        if value < i32::MIN as i64 || value > i32::MAX as i64 {
            return serde_json::Value::String(value.to_string());
        }
    }
    if let Some(value) = number.as_u64() {
        if value > u32::MAX as u64 {
            return serde_json::Value::String(value.to_string());
        }
    }
    serde_json::Value::Number(number)
}

fn snake_to_lower_camel(input: &str) -> String {
    let mut parts = input.split('_');
    let Some(first) = parts.next() else {
        return String::new();
    };
    let mut out = String::from(first);
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first_char) = chars.next() {
            out.push(first_char.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    out
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
