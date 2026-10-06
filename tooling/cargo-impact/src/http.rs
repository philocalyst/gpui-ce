//! Bounded HTTP transport. Credentials belong to the control plane, never builds.

use std::{io::Read, time::Duration};

use reqwest::{StatusCode, blocking::Client};
use serde::de::DeserializeOwned;
use thiserror::Error;
use url::Url;

const RESPONSE_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("API authentication failed (HTTP {0})")]
    Authentication(StatusCode),
    #[error("API rate limited; retry after {retry_after_seconds:?} seconds")]
    RateLimited { retry_after_seconds: Option<u64> },
    #[error("API returned HTTP {0}")]
    Status(StatusCode),
    #[error("API response exceeds 16 MiB")]
    TooLarge,
    #[error("HTTP request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("reading API response: {0}")]
    Read(#[from] std::io::Error),
    #[error("invalid API response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("invalid API URL: {0}")]
    Url(#[from] url::ParseError),
}

/// No automatic redirects: a forge token must never follow a redirect to another host.
pub struct Api {
    client: Client,
    base: Url,
    token: Option<String>,
}

impl Api {
    pub fn new(base: Url, token: Option<String>) -> Result<Self, ApiError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(concat!("cargo-impact/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base,
            token,
        })
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T, ApiError> {
        self.request(reqwest::Method::GET, path, query, None)
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, body: serde_json::Value) -> Result<T, ApiError> {
        self.request(reqwest::Method::POST, path, &[], Some(body))
    }

    fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<T, ApiError> {
        let url = self.base.join(path)?;
        // API paths are constructed internally. Keep even accidental absolute paths on origin.
        if url.origin() != self.base.origin() {
            return Err(ApiError::Status(StatusCode::BAD_REQUEST));
        }
        let mut request = self.client.request(method, url).query(query).header("Accept", "application/json");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send()?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && response.headers().get("x-ratelimit-remaining").is_some_and(|v| v == "0"))
        {
            return Err(ApiError::RateLimited {
                retry_after_seconds: response.headers().get("retry-after")
                    .and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()),
            });
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ApiError::Authentication(status));
        }
        if !status.is_success() {
            return Err(ApiError::Status(status));
        }
        let mut bytes = Vec::new();
        response.take(RESPONSE_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > RESPONSE_LIMIT {
            return Err(ApiError::TooLarge);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}
