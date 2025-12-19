//! HTTP client for API communication.

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::config::{Config, Credentials};
use crate::error::CliError;

/// API client for communicating with the control plane.
#[derive(Debug, Clone)]
pub struct ApiClient {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

impl ApiClient {
    /// Create a new API client from config and credentials.
    pub fn new(config: &Config, credentials: Option<&Credentials>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(creds) = credentials {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", creds.token))
                    .context("Invalid token format")?,
            );
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url: config.api_url().trim_end_matches('/').to_string(),
            token: credentials.map(|c| c.token.clone()),
        })
    }

    /// Create a client without authentication.
    pub fn unauthenticated(config: &Config) -> Result<Self> {
        Self::new(config, None)
    }

    /// Check if the client has authentication.
    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    /// Build a URL for an endpoint.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Make a GET request.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, CliError> {
        let response = self.client.get(self.url(path)).send().await?;

        self.handle_response(response).await
    }

    /// Make a GET request and return the raw response body.
    pub async fn get_raw(&self, path: &str) -> Result<reqwest::Response, CliError> {
        let response = self.client.get(self.url(path)).send().await?;

        if response.status().is_success() {
            Ok(response)
        } else {
            self.handle_error(response).await
        }
    }

    /// Make a GET request to an SSE endpoint and return the raw response body.
    pub async fn get_event_stream(&self, path: &str) -> Result<reqwest::Response, CliError> {
        let response = self
            .client
            .get(self.url(path))
            .header(ACCEPT, "text/event-stream")
            .send()
            .await?;

        if response.status().is_success() {
            Ok(response)
        } else {
            self.handle_error(response).await
        }
    }

    /// Make a POST request.
    pub async fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CliError> {
        let response = self.client.post(self.url(path)).json(body).send().await?;

        self.handle_response(response).await
    }

    /// Make a PATCH request.
    pub async fn patch<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CliError> {
        let response = self.client.patch(self.url(path)).json(body).send().await?;

        self.handle_response(response).await
    }

    /// Make a DELETE request.
    pub async fn delete(&self, path: &str) -> Result<(), CliError> {
        let response = self.client.delete(self.url(path)).send().await?;

        if response.status().is_success() {
            Ok(())
        } else {
            self.handle_error(response).await
        }
    }

    /// Handle a successful or error response.
    async fn handle_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, CliError> {
        let status = response.status();

        if status.is_success() {
            response
                .json()
                .await
                .map_err(|e| CliError::Other(anyhow::anyhow!("Failed to parse response: {}", e)))
        } else {
            self.handle_error(response).await
        }
    }

    /// Handle an error response.
    async fn handle_error<T>(&self, response: reqwest::Response) -> Result<T, CliError> {
        let status = response.status().as_u16();

        // Try to parse error response
        let error_body: ApiErrorResponse =
            response.json().await.unwrap_or_else(|_| ApiErrorResponse {
                code: "unknown".to_string(),
                message: "Unknown error".to_string(),
                request_id: None,
            });

        if status == 401 {
            return Err(CliError::NotAuthenticated);
        }

        Err(CliError::api(status, error_body.code, error_body.message))
    }
}

/// API error response structure.
#[derive(Debug, Deserialize)]
struct ApiErrorResponse {
    code: String,
    message: String,
    #[serde(default)]
    request_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_building() {
        let config = Config::default();
        let client = ApiClient::unauthenticated(&config).unwrap();
        assert!(client.url("/v1/orgs").contains("/v1/orgs"));
    }
}
