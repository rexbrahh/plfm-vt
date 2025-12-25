//! Control plane synchronization.
//!
//! This module syncs route configuration from the control plane and updates
//! the shared route table used by the proxy.
//!
//! Per docs/specs/networking/ingress-l4.md:
//! - Config updates must be applied atomically
//! - Control plane outage: edge continues operating on last applied config

use std::{
    collections::BTreeMap,
    fs,
    net::Ipv6Addr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use plfm_events::{
    RouteCreatedPayload, RouteDeletedPayload, RouteProtocolHint, RouteProxyProtocol,
    RouteUpdatedPayload,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::config::Config;
use plfm_ingress::persistence::{PersistedRoute, StatePersistence};
use plfm_ingress::{Backend, BackendSelector, ProtocolHint, ProxyProtocol, Route, RouteTable};

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
    app_id: String,
    env_id: String,
    backend_process_type: String,
    backend_port: i32,
    protocol_hint: RouteProtocolHint,
    proxy_protocol: RouteProxyProtocol,
    backend_expects_proxy_protocol: bool,
    ipv4_required: bool,
    env_ipv4_address: Option<String>,
}

impl RouteState {
    fn from_created(payload: RouteCreatedPayload) -> Self {
        Self {
            route_id: payload.route_id.to_string(),
            hostname: payload.hostname,
            listen_port: payload.listen_port,
            app_id: payload.app_id.to_string(),
            env_id: payload.env_id.to_string(),
            backend_process_type: payload.backend_process_type,
            backend_port: payload.backend_port,
            protocol_hint: payload.protocol_hint,
            proxy_protocol: payload.proxy_protocol,
            backend_expects_proxy_protocol: payload.backend_expects_proxy_protocol,
            ipv4_required: payload.ipv4_required,
            env_ipv4_address: payload.env_ipv4_address,
        }
    }

    fn from_persisted(p: &PersistedRoute) -> Self {
        Self {
            route_id: p.route_id.clone(),
            hostname: p.hostname.clone(),
            listen_port: p.listen_port,
            app_id: p.app_id.clone(),
            env_id: p.env_id.clone(),
            backend_process_type: p.backend_process_type.clone(),
            backend_port: p.backend_port,
            protocol_hint: PersistedRoute::protocol_hint_from_string(&p.protocol_hint),
            proxy_protocol: PersistedRoute::proxy_protocol_from_string(&p.proxy_protocol),
            backend_expects_proxy_protocol: p.backend_expects_proxy_protocol,
            ipv4_required: p.ipv4_required,
            env_ipv4_address: p.env_ipv4_address.clone(),
        }
    }

    fn to_persisted(&self) -> PersistedRoute {
        PersistedRoute {
            route_id: self.route_id.clone(),
            hostname: self.hostname.clone(),
            listen_port: self.listen_port,
            app_id: self.app_id.clone(),
            env_id: self.env_id.clone(),
            backend_process_type: self.backend_process_type.clone(),
            backend_port: self.backend_port,
            protocol_hint: PersistedRoute::protocol_hint_to_string(self.protocol_hint),
            proxy_protocol: PersistedRoute::proxy_protocol_to_string(self.proxy_protocol),
            backend_expects_proxy_protocol: self.backend_expects_proxy_protocol,
            ipv4_required: self.ipv4_required,
            env_ipv4_address: self.env_ipv4_address.clone(),
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

        if let Some(v) = payload.env_ipv4_address {
            if v != self.env_ipv4_address {
                self.env_ipv4_address = v;
                changed.push("env_ipv4_address");
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

fn route_state_to_proxy_route(state: &RouteState) -> Route {
    let protocol = match state.protocol_hint {
        RouteProtocolHint::TlsPassthrough => ProtocolHint::TlsPassthrough,
        RouteProtocolHint::TcpRaw => ProtocolHint::TcpRaw,
    };
    let allow_non_tls_fallback = matches!(state.protocol_hint, RouteProtocolHint::TcpRaw);

    Route {
        id: state.route_id.clone(),
        hostname: Route::normalize_hostname(&state.hostname),
        port: state.listen_port as u16,
        protocol,
        proxy_protocol: match state.proxy_protocol {
            RouteProxyProtocol::Off => ProxyProtocol::Off,
            RouteProxyProtocol::V2 => ProxyProtocol::V2,
        },
        app_id: state.app_id.clone(),
        env_id: state.env_id.clone(),
        backend_process_type: state.backend_process_type.clone(),
        backend_port: state.backend_port as u16,
        allow_non_tls_fallback,
        env_ipv4_address: state.env_ipv4_address.clone(),
    }
}

/// Update the shared route table from internal state.
async fn update_proxy_route_table(routes: &BTreeMap<String, RouteState>, route_table: &RouteTable) {
    let proxy_routes: Vec<Route> = routes.values().map(route_state_to_proxy_route).collect();
    route_table.update(proxy_routes).await;
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

/// Poll route events and update the shared route table.
pub async fn run_route_sync_loop(
    config: &Config,
    route_table: Arc<RouteTable>,
    _backend_selector: Arc<BackendSelector>,
) -> Result<()> {
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

    // Initialize persistence if configured
    let persistence = config
        .state_file
        .as_ref()
        .map(|path| StatePersistence::new(path.clone()));

    // Load initial state from persistence (if available)
    let mut cursor = if let Some(ref p) = persistence {
        match p.load() {
            Ok(state) => {
                // Restore routes from persisted state
                for (id, persisted_route) in &state.routes {
                    routes.insert(id.clone(), RouteState::from_persisted(persisted_route));
                }

                // Update route table with restored state
                if !routes.is_empty() {
                    update_proxy_route_table(&routes, &route_table).await;
                    info!(
                        route_count = routes.len(),
                        cursor = state.cursor,
                        "Restored routes from persisted state"
                    );
                }

                state.cursor
            }
            Err(e) => {
                warn!(error = %e, "Failed to load persisted state, starting fresh");
                0
            }
        }
    } else {
        // Fall back to cursor-only file if no state persistence
        match &config.cursor_file {
            Some(path) => read_cursor(path)?,
            None => 0,
        }
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

        let mut routes_changed = false;

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
            routes_changed = true;
        }

        // Update the shared route table if routes changed
        if routes_changed {
            update_proxy_route_table(&routes, &route_table).await;
        }

        cursor = resp.next_after_event_id.max(cursor);

        // Persist state atomically if configured
        if let Some(ref p) = persistence {
            let persisted_routes: BTreeMap<String, PersistedRoute> = routes
                .iter()
                .map(|(id, r)| (id.clone(), r.to_persisted()))
                .collect();

            if let Err(e) = p.save_with_cursor(&persisted_routes, cursor) {
                warn!(error = %e, "Failed to persist state");
            }
        } else if let Some(path) = &config.cursor_file {
            // Fall back to cursor-only file
            write_cursor(path, cursor)?;
        }
    }
}

/// Sync backend instances for all routes.
///
/// This fetches instance lists from the control plane and updates the backend
/// selector with healthy instances for each route.
pub async fn sync_backends(
    config: &Config,
    route_table: &RouteTable,
    backend_selector: &BackendSelector,
) -> Result<()> {
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

    // Get all route IDs
    let route_ids = route_table.route_ids().await;

    for route_id in route_ids {
        let Some(route) = route_table.get(&route_id).await else {
            continue;
        };

        // Fetch instances for this route's environment and process type
        match fetch_route_backends(&client, config, &route).await {
            Ok(backends) => {
                backend_selector
                    .update_route_backends(&route_id, backends)
                    .await;
            }
            Err(e) => {
                warn!(
                    route_id = %route_id,
                    error = %e,
                    "Failed to fetch backends"
                );
            }
        }
    }

    Ok(())
}

/// Response for listing instances.
#[derive(Debug, Deserialize)]
struct InstancesResponse {
    items: Vec<InstanceItem>,
}

/// Instance item from API.
#[derive(Debug, Deserialize)]
struct InstanceItem {
    id: String,
    #[serde(default)]
    overlay_ipv6: Option<String>,
}

/// Fetch backends for a specific route.
async fn fetch_route_backends(
    client: &reqwest::Client,
    config: &Config,
    route: &Route,
) -> Result<Vec<Backend>> {
    let base = config.control_plane_url.trim_end_matches('/');

    // Fetch instances for this environment and process type
    // API: GET /v1/orgs/{org}/apps/{app}/envs/{env}/instances?process_type={pt}&status=ready
    let url = format!(
        "{}/v1/orgs/{}/apps/{}/envs/{}/instances",
        base, config.org_id, route.app_id, route.env_id
    );

    let resp = client
        .get(&url)
        .query(&[
            ("process_type", route.backend_process_type.as_str()),
            ("status", "ready"),
            ("limit", "100"),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("instances query failed (status={}): {}", status, body);
    }

    let instances: InstancesResponse = resp.json().await?;

    // Convert to backends
    let backends: Vec<Backend> = instances
        .items
        .into_iter()
        .filter_map(|inst| {
            let overlay_ipv6 = inst.overlay_ipv6.as_ref()?;
            let addr: Ipv6Addr = overlay_ipv6.parse().ok()?;
            Some(Backend::new(addr, route.backend_port, inst.id))
        })
        .collect();

    debug!(
        route_id = %route.id,
        backend_count = backends.len(),
        "Fetched backends"
    );

    Ok(backends)
}

/// Run periodic backend sync loop.
pub async fn run_backend_sync_loop(
    config: Config,
    route_table: Arc<RouteTable>,
    backend_selector: Arc<BackendSelector>,
) -> Result<()> {
    loop {
        if let Err(e) = sync_backends(&config, &route_table, &backend_selector).await {
            warn!(error = %e, "Backend sync failed");
        }

        tokio::time::sleep(config.backend_sync_interval).await;
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
            app_id: "app_123".to_string(),
            env_id: "env_123".to_string(),
            backend_process_type: "web".to_string(),
            backend_port: 8080,
            protocol_hint: RouteProtocolHint::TlsPassthrough,
            proxy_protocol: RouteProxyProtocol::Off,
            backend_expects_proxy_protocol: false,
            ipv4_required: false,
            env_ipv4_address: None,
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
            env_ipv4_address: None,
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
