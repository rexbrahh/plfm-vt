//! Name → ID resolution helpers for CLI context.
//!
//! The API is ID-addressed. For UX, the CLI accepts either stable IDs or names.
//! This module resolves names to IDs by listing within the appropriate scope.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::client::ApiClient;
use crate::error::CliError;

#[derive(Debug, Deserialize)]
struct ListOrgsResponse {
    items: Vec<OrgItem>,
}

#[derive(Debug, Deserialize)]
struct OrgItem {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ListAppsResponse {
    items: Vec<AppItem>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppItem {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ListEnvsResponse {
    items: Vec<EnvItem>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnvItem {
    id: String,
    name: String,
}

pub async fn resolve_org_id(client: &ApiClient, org_ident: &str) -> Result<plfm_id::OrgId> {
    let org_ident = org_ident.trim();
    if org_ident.is_empty() {
        anyhow::bail!("Organization cannot be empty");
    }

    if let Ok(id) = org_ident.parse::<plfm_id::OrgId>() {
        return Ok(id);
    }

    let response: ListOrgsResponse = client.get("/v1/orgs").await?;
    let mut matches: Vec<plfm_id::OrgId> = response
        .items
        .into_iter()
        .filter(|org| org.name == org_ident)
        .map(|org| {
            org.id.parse::<plfm_id::OrgId>().with_context(|| {
                format!(
                    "API returned invalid org id '{}' for org '{}'",
                    org.id, org.name
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;

    matches.sort();
    match matches.as_slice() {
        [] => Err(CliError::NotFound(format!("Organization '{}' not found", org_ident)).into()),
        [only] => Ok(*only),
        many => {
            let ids = many
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "Organization name '{}' is ambiguous ({}). Use an explicit org ID.",
                org_ident,
                ids
            );
        }
    }
}

pub async fn resolve_app_id(
    client: &ApiClient,
    org_id: plfm_id::OrgId,
    app_ident: &str,
) -> Result<plfm_id::AppId> {
    let app_ident = app_ident.trim();
    if app_ident.is_empty() {
        anyhow::bail!("Application cannot be empty");
    }

    if let Ok(id) = app_ident.parse::<plfm_id::AppId>() {
        return Ok(id);
    }

    let mut cursor: Option<String> = None;
    let mut matches: Vec<plfm_id::AppId> = Vec::new();

    loop {
        let mut path = format!("/v1/orgs/{org_id}/apps?limit=200");
        if let Some(c) = cursor.as_deref() {
            path.push_str(&format!("&cursor={c}"));
        }

        let response: ListAppsResponse = client.get(&path).await?;
        for app in response.items {
            if app.name == app_ident {
                let id = app.id.parse::<plfm_id::AppId>().with_context(|| {
                    format!(
                        "API returned invalid app id '{}' for app '{}'",
                        app.id, app.name
                    )
                })?;
                matches.push(id);
            }
        }

        cursor = response.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    matches.sort();
    match matches.as_slice() {
        [] => Err(CliError::NotFound(format!("Application '{}' not found", app_ident)).into()),
        [only] => Ok(*only),
        many => {
            let ids = many
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "Application name '{}' is ambiguous ({}). Use an explicit app ID.",
                app_ident,
                ids
            );
        }
    }
}

pub async fn resolve_env_id(
    client: &ApiClient,
    org_id: plfm_id::OrgId,
    app_id: plfm_id::AppId,
    env_ident: &str,
) -> Result<plfm_id::EnvId> {
    let env_ident = env_ident.trim();
    if env_ident.is_empty() {
        anyhow::bail!("Environment cannot be empty");
    }

    if let Ok(id) = env_ident.parse::<plfm_id::EnvId>() {
        return Ok(id);
    }

    let mut cursor: Option<String> = None;
    let mut matches: Vec<plfm_id::EnvId> = Vec::new();

    loop {
        let mut path = format!("/v1/orgs/{org_id}/apps/{app_id}/envs?limit=200");
        if let Some(c) = cursor.as_deref() {
            path.push_str(&format!("&cursor={c}"));
        }

        let response: ListEnvsResponse = client.get(&path).await?;
        for env in response.items {
            if env.name == env_ident {
                let id = env.id.parse::<plfm_id::EnvId>().with_context(|| {
                    format!(
                        "API returned invalid env id '{}' for env '{}'",
                        env.id, env.name
                    )
                })?;
                matches.push(id);
            }
        }

        cursor = response.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    matches.sort();
    match matches.as_slice() {
        [] => Err(CliError::NotFound(format!("Environment '{}' not found", env_ident)).into()),
        [only] => Ok(*only),
        many => {
            let ids = many
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "Environment name '{}' is ambiguous ({}). Use an explicit env ID.",
                env_ident,
                ids
            );
        }
    }
}
