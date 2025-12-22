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
        })
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

    /// Make a GET request to an NDJSON endpoint and return the raw response body.
    pub async fn get_ndjson_stream(&self, path: &str) -> Result<reqwest::Response, CliError> {
        let response = self
            .client
            .get(self.url(path))
            .header(ACCEPT, "application/x-ndjson")
            .send()
            .await?;

        if response.status().is_success() {
            Ok(response)
        } else {
            self.handle_error(response).await
        }
    }

    /// Make a POST request with an optional Idempotency-Key.
    pub async fn post_with_idempotency_key<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
        idempotency_key: Option<&str>,
    ) -> Result<T, CliError> {
        let mut request = self.client.post(self.url(path)).json(body);
        if let Some(key) = idempotency_key {
            request = request.header(crate::idempotency::IDEMPOTENCY_KEY_HEADER, key);
        }
        let response = request.send().await?;

        self.handle_response(response).await
    }

    /// Make a PUT request with an optional Idempotency-Key.
    pub async fn put_with_idempotency_key<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
        idempotency_key: Option<&str>,
    ) -> Result<T, CliError> {
        let mut request = self.client.put(self.url(path)).json(body);
        if let Some(key) = idempotency_key {
            request = request.header(crate::idempotency::IDEMPOTENCY_KEY_HEADER, key);
        }
        let response = request.send().await?;

        self.handle_response(response).await
    }

    /// Make a PATCH request with an optional Idempotency-Key.
    pub async fn patch_with_idempotency_key<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
        idempotency_key: Option<&str>,
    ) -> Result<T, CliError> {
        let mut request = self.client.patch(self.url(path)).json(body);
        if let Some(key) = idempotency_key {
            request = request.header(crate::idempotency::IDEMPOTENCY_KEY_HEADER, key);
        }
        let response = request.send().await?;

        self.handle_response(response).await
    }

    /// Make a DELETE request with an optional Idempotency-Key.
    pub async fn delete_with_idempotency_key(
        &self,
        path: &str,
        idempotency_key: Option<&str>,
    ) -> Result<(), CliError> {
        let mut request = self.client.delete(self.url(path));
        if let Some(key) = idempotency_key {
            request = request.header(crate::idempotency::IDEMPOTENCY_KEY_HEADER, key);
        }

        let response = request.send().await?;

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
        let body = response.text().await.unwrap_or_default();

        let problem = serde_json::from_str::<ProblemDetailsResponse>(&body).ok();
        let legacy = serde_json::from_str::<LegacyApiErrorResponse>(&body).ok();

        if status == 401 {
            return Err(CliError::NotAuthenticated);
        }

        let code = problem
            .as_ref()
            .and_then(|problem| problem.code.as_deref())
            .or_else(|| legacy.as_ref().map(|error| error.code.as_str()))
            .unwrap_or("unknown")
            .to_string();
        let message = problem
            .as_ref()
            .and_then(|problem| problem.detail.as_deref())
            .or_else(|| {
                problem
                    .as_ref()
                    .and_then(|problem| problem.title.as_deref())
            })
            .or_else(|| legacy.as_ref().map(|error| error.message.as_str()))
            .unwrap_or("Unknown error")
            .to_string();
        let request_id = problem
            .as_ref()
            .and_then(|problem| problem.request_id.clone())
            .or_else(|| legacy.as_ref().and_then(|error| error.request_id.clone()));
        let retryable = problem
            .as_ref()
            .and_then(|problem| problem.retryable)
            .unwrap_or(false);
        let retry_after_seconds = problem
            .as_ref()
            .and_then(|problem| problem.retry_after_seconds);

        Err(CliError::api(
            status,
            code,
            message,
            request_id,
            retryable,
            retry_after_seconds,
        ))
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ProblemDetailsResponse {
    #[serde(rename = "type")]
    r#type: Option<String>,
    title: Option<String>,
    status: Option<u16>,
    detail: Option<String>,
    instance: Option<String>,
    code: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    retryable: Option<bool>,
    retry_after_seconds: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct LegacyApiErrorResponse {
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
        let client = ApiClient::new(&config, None).unwrap();
        assert!(client.url("/v1/orgs").contains("/v1/orgs"));
    }
}
