use anyhow::{Context as _, Result};
use cloud_objects::ids::ServerId;
use serde::de::DeserializeOwned;
use warp_core::channel::ChannelState;
use warp_errors::{ErrorExt, register_error};

use crate::base_client::{BaseClient, TEAM_UID_HEADER};

#[derive(serde::Deserialize)]
pub struct PresignedDownloadResponse {
    pub url: String,
}

/// Typed error for HTTP operations so retry classifiers can inspect status failures.
#[derive(Debug, thiserror::Error)]
#[error("HTTP request failed with status {status}: {body}")]
pub struct HttpStatusError {
    pub status: u16,
    pub body: String,
    api_error_message: Option<String>,
}

impl HttpStatusError {
    pub fn new(status: u16, body: String) -> Self {
        let api_error_message = serde_json::from_str::<PublicApiError>(&body)
            .ok()
            .map(|error| error.error);
        Self {
            status,
            body,
            api_error_message,
        }
    }

    fn api_error_message(&self) -> Option<&str> {
        self.api_error_message.as_deref()
    }
}

impl ErrorExt for HttpStatusError {
    fn is_actionable(&self) -> bool {
        !matches!(self.status, 408 | 429)
    }
}

register_error!(HttpStatusError);

#[derive(serde::Deserialize)]
struct PublicApiError {
    error: String,
}

impl BaseClient {
    /// Downloads bytes from a presigned URL without attaching Warp headers or credentials.
    pub async fn download_presigned(&self, url: &str) -> Result<bytes::Bytes> {
        let response = self
            .external_http_client()
            .get(url)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Failed to send presigned download request")?;
        if !response.status().is_success() {
            return Err(HttpStatusError::new(response.status().as_u16(), String::new()).into());
        }
        response
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Failed to read presigned download")
    }

    /// Sends a GET request to a public API endpoint and returns the raw response on success.
    ///
    /// Unlike [`get_public_api`], this does not attempt JSON deserialization on the
    /// response body, allowing the caller to decode it however they need.
    pub async fn get_public_api_response(&self, path: &str) -> Result<http_client::Response> {
        self.get_public_api_response_for_team(path, None).await
    }

    pub async fn get_public_api_response_for_team(
        &self,
        path: &str,
        team_uid: Option<ServerId>,
    ) -> Result<http_client::Response> {
        let auth_token = self
            .get_or_refresh_access_token()
            .await
            .context("Failed to get access token for API request")?;
        let url = format!("{}/api/v1/{path}", ChannelState::server_root_url());
        let mut request = self.http_client().get(&url);
        if let Some(token) = auth_token.as_bearer_token() {
            request = request.bearer_auth(token);
        }
        if let Some(team_uid) = team_uid {
            request = request.header(TEAM_UID_HEADER, team_uid.uid());
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("Failed to send API request to {url}"))?;

        if response.status().is_success() {
            Ok(response)
        } else {
            self.observe_iap_challenge(&response);
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let status_error = HttpStatusError::new(status.as_u16(), body);
            let context = status_error
                .api_error_message()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("API request failed with status {status}"));
            Err(anyhow::Error::new(status_error).context(context))
        }
    }

    /// Sends a GET request to a public API endpoint.
    ///
    /// # Arguments
    /// * `path` - Endpoint path relative to `/api/v1` (e.g., "agent/tasks/{task_id}")
    pub async fn get_public_api<R>(&self, path: &str) -> Result<R>
    where
        R: DeserializeOwned,
    {
        self.get_public_api_for_team(path, None).await
    }

    pub async fn get_public_api_for_team<R>(
        &self,
        path: &str,
        team_uid: Option<ServerId>,
    ) -> Result<R>
    where
        R: DeserializeOwned,
    {
        let response = self
            .get_public_api_response_for_team(path, team_uid)
            .await?;
        let response_url = response.url().clone();
        response
            .json::<R>()
            .await
            .with_context(|| format!("Failed to deserialize response from {response_url}"))
    }
}

#[cfg(test)]
#[path = "public_api_tests.rs"]
mod tests;
