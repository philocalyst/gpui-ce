#![cfg(target_family = "wasm")]

use thiserror::Error;

#[derive(Error, Debug)]
pub enum WebError {
    #[error("JavaScript interop error: {description}")]
    Js {
        description: String,
        js_error: Option<String>,
    },
    #[error("DOM element '{element}' not found")]
    MissingDomElement { element: &'static str },
    #[error("feature not available: {feature}")]
    FeatureNotAvailable { feature: &'static str },
    #[error("WGPU error: {0}")]
    Wgpu(String),
    #[error("HTTP request failed to {url}: status {status}")]
    Http { status: u16, url: String },
    #[error("HTTP client error: {0}")]
    HttpError(String),
}

pub type Result<T> = std::result::Result<T, WebError>;
