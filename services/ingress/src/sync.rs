//! Control plane synchronization (stub).
//!
//! In v1, ingress will consume desired routing state from the control plane.
//! For now, we implement a minimal event consumer that tails org-scoped events
//! and maintains an in-memory route table.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use plfm_events::{
    RouteCreatedPayload, RouteDeletedPayload, RouteProxyProtocol, RouteUpdatedPayload,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::config::Config;

#[derive(Debug, Deserialize)]
struct EventsResponse {
    items: Vec<EventItem>,
    next_after_event_id: i64,
}

#[derive(Debug, Deserialize)]
struct EventItem {
    event_id: i64,
    event_type: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteState {
    route_id: String,
    hostname: String,
    listen_port: i32,
    env_id: String,
    backend_process_type: String,
    backend_port: i32,
    proxy_protocol: RouteProxyProtocol,
    backend_expects_proxy_protocol: bool,
    ipv4_required: bool,
}

impl RouteState {
    fn from_created(payload: RouteCreatedPayload) -> Self {
        Self {
            route_id: payload.route_id.to_string(),
            hostname: payload.hostname,
            listen_port: payload.listen_port,
            env_id: payload.env_id.to_string(),
            backend_process_type: payload.backend_process_type,
            backend_port: payload.backend_port,
            proxy_protocol: payload.proxy_protocol,
            backend_expects_proxy_protocol: payload.backend_expects_proxy_protocol,
            ipv4_required: payload.ipv4_required,
        }
    }

    fn apply_update(&mut self, payload: RouteUpdatedPayload) -> Vec<&'static str> {
        let mut changed = Vec::new();

        if let Some(v) = payload.backend_process_type {
            if v != self.backend_process_type {
                self.backend_process_type = v;
                changed.push("backend_process_type");
            }
        }

        if let Some(v) = payload.backend_port {
            if v != self.backend_port {
                self.backend_port = v;
                changed.push("backend_port");
            }
        }

        if let Some(v) = payload.proxy_protocol {
            if v != self.proxy_protocol {
                self.proxy_protocol = v;
                changed.push("proxy_protocol");
            }
        }

        if let Some(v) = payload.backend_expects_proxy_protocol {
            if v != self.backend_expects_proxy_protocol {
                self.backend_expects_proxy_protocol = v;
                changed.push("backend_expects_proxy_protocol");
            }
        }

        if let Some(v) = payload.ipv4_required {
            if v != self.ipv4_required {
                self.ipv4_required = v;
                changed.push("ipv4_required");
            }
        }

        changed
    }
}

fn proxy_protocol_label(p: RouteProxyProtocol) -> &'static str {
    match p {
        RouteProxyProtocol::Off => "off",
        RouteProxyProtocol::V2 => "v2",
    }
}

fn read_cursor(path: &Path) -> Result<i64> {
    if !path.exists() {
        return Ok(0);
    }

    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read cursor file {}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }

    trimmed
        .parse::<i64>()
        .with_context(|| format!("Invalid cursor in {}", path.display()))
}

fn write_cursor(path: &Path, cursor: i64) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let tmp: PathBuf = path.with_extension("tmp");
    fs::write(&tmp, cursor.to_string())
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "Failed to move cursor file into place ({} -> {})",
            tmp.display(),
            path.display()
        )
    })?;

    Ok(())
}

async fn fetch_events(
    client: &reqwest::Client,
    base_url: &str,
    org_id: &str,
    after_event_id: i64,
    limit: i64,
) -> Result<EventsResponse> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/v1/orgs/{org_id}/events");

    let resp = client
        .get(url)
        .query(&[("after_event_id", after_event_id), ("limit", limit)])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "events query failed (status={}): {}",
            status,
            body
        ));
    }

    Ok(resp.json::<EventsResponse>().await?)
}

fn apply_route_event(
    routes: &mut BTreeMap<String, RouteState>,
    event_id: i64,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<()> {
    match event_type {
        "route.created" => {
            let payload: RouteCreatedPayload =
                serde_json::from_value(payload).context("invalid route.created payload JSON")?;

            let state = RouteState::from_created(payload);
            let route_id = state.route_id.clone();
            let replaced = routes.insert(route_id.clone(), state.clone()).is_some();

            info!(
                event_id,
                route_id = %route_id,
                hostname = %state.hostname,
                listen_port = state.listen_port,
                env_id = %state.env_id,
                backend_process_type = %state.backend_process_type,
                backend_port = state.backend_port,
                proxy_protocol = proxy_protocol_label(state.proxy_protocol),
                backend_expects_proxy_protocol = state.backend_expects_proxy_protocol,
                ipv4_required = state.ipv4_required,
                replaced,
                "route upserted"
            );
        }
        "route.updated" => {
            let payload: RouteUpdatedPayload =
                serde_json::from_value(payload).context("invalid route.updated payload JSON")?;
            let route_id = payload.route_id.to_string();

            let Some(state) = routes.get_mut(&route_id) else {
                warn!(event_id, route_id = %route_id, "route.updated for unknown route_id");
                return Ok(());
            };

            let changed_fields = state.apply_update(payload);
            if changed_fields.is_empty() {
                debug!(event_id, route_id = %route_id, "route.updated had no effective changes");
                return Ok(());
            }

            info!(
                event_id,
                route_id = %route_id,
                changed_fields = ?changed_fields,
                backend_process_type = %state.backend_process_type,
                backend_port = state.backend_port,
                proxy_protocol = proxy_protocol_label(state.proxy_protocol),
                backend_expects_proxy_protocol = state.backend_expects_proxy_protocol,
                ipv4_required = state.ipv4_required,
                "route updated"
            );
        }
        "route.deleted" => {
            let payload: RouteDeletedPayload =
                serde_json::from_value(payload).context("invalid route.deleted payload JSON")?;
            let route_id = payload.route_id.to_string();

            let existed = routes.remove(&route_id).is_some();
            info!(
                event_id,
                route_id = %route_id,
                env_id = %payload.env_id,
                hostname = %payload.hostname,
                existed,
                "route deleted"
            );
        }
        _ => {}
    }

    Ok(())
}

/// Poll route events and keep an in-memory routing table (stub).
pub async fn run_route_sync_loop(config: &Config) -> Result<()> {
    let mut headers = HeaderMap::new();
    if let Some(token) = &config.control_plane_token {
        let raw = token.expose().trim();
        let bearer = if raw.starts_with("Bearer ") || raw.starts_with("bearer ") {
            raw.to_string()
        } else {
            format!("Bearer {raw}")
        };

        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&bearer).context("Invalid control-plane token format")?,
        );
    }

    let client = reqwest::Client::builder()
        .user_agent("plfm-ingress/0.1.0")
        .default_headers(headers)
        .build()?;

    let mut routes: BTreeMap<String, RouteState> = BTreeMap::new();

    let mut cursor = match &config.cursor_file {
        Some(path) => read_cursor(path)?,
        None => 0,
    };

    loop {
        let resp = fetch_events(
            &client,
            &config.control_plane_url,
            &config.org_id,
            cursor,
            config.fetch_limit,
        )
        .await;

        let resp = match resp {
            Ok(resp) => resp,
            Err(e) => {
                warn!(error = %e, cursor, "failed to fetch events; retrying");
                tokio::time::sleep(config.poll_interval).await;
                continue;
            }
        };

        if resp.items.is_empty() {
            if config.once {
                info!(cursor, route_count = routes.len(), "sync complete");
                return Ok(());
            }

            tokio::time::sleep(config.poll_interval).await;
            continue;
        }

        for item in resp.items {
            cursor = item.event_id;

            if !item.event_type.starts_with("route.") {
                continue;
            }

            let Some(payload) = item.payload else {
                warn!(
                    event_id = item.event_id,
                    event_type = %item.event_type,
                    "route event missing payload"
                );
                continue;
            };

            apply_route_event(&mut routes, item.event_id, &item.event_type, payload)?;
        }

        cursor = resp.next_after_event_id.max(cursor);

        if let Some(path) = &config.cursor_file {
            write_cursor(path, cursor)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plfm_id::{EnvId, OrgId, RouteId};

    #[test]
    fn test_route_state_apply_update_tracks_changed_fields() {
        let mut state = RouteState {
            route_id: "route_123".to_string(),
            hostname: "example.invalid".to_string(),
            listen_port: 443,
            env_id: "env_123".to_string(),
            backend_process_type: "web".to_string(),
            backend_port: 8080,
            proxy_protocol: RouteProxyProtocol::Off,
            backend_expects_proxy_protocol: false,
            ipv4_required: false,
        };

        let payload = RouteUpdatedPayload {
            route_id: RouteId::new(),
            org_id: OrgId::new(),
            env_id: EnvId::new(),
            backend_process_type: Some("worker".to_string()),
            backend_port: Some(9090),
            proxy_protocol: Some(RouteProxyProtocol::V2),
            backend_expects_proxy_protocol: Some(true),
            ipv4_required: None,
        };

        let changed = state.apply_update(payload);
        assert_eq!(
            changed,
            vec![
                "backend_process_type",
                "backend_port",
                "proxy_protocol",
                "backend_expects_proxy_protocol"
            ]
        );
        assert_eq!(state.backend_process_type, "worker");
        assert_eq!(state.backend_port, 9090);
        assert_eq!(state.proxy_protocol, RouteProxyProtocol::V2);
        assert!(state.backend_expects_proxy_protocol);
        assert!(!state.ipv4_required);
    }
}
