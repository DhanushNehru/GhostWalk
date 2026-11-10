//! # ghostwalk-net
//!
//! Bounded HTTP fetching for the ghostwalk engine.
//!
//! Every knob that hostile or accidental web content can abuse has a
//! ceiling: total time, redirect count, and response size. The MVP is a
//! blocking client behind a small surface; the Tokio actor pipeline in
//! ARCHITECTURE.md replaces the transport, not this contract.

use std::time::Duration;

use thiserror::Error;
use ureq::ResponseExt;

/// Defaults chosen for unattended agent use, not browsers.
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_MAX_REDIRECTS: u32 = 5;
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const DEFAULT_USER_AGENT: &str = "ghostwalk/0.1 (+https://github.com/DhanushNehru/ghostwalk)";

/// Per-request limits.
#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub timeout: Duration,
    pub max_redirects: u32,
    pub max_bytes: u64,
    pub user_agent: String,
}

impl Default for FetchOptions {
    fn default() -> Self {
        FetchOptions {
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_bytes: DEFAULT_MAX_BYTES,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        }
    }
}

/// A completed fetch.
#[derive(Debug)]
pub struct FetchResult {
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: String,
}

#[derive(Debug, Error)]
pub enum NetError {
    #[error("unsupported URL scheme (http/https only): {0}")]
    UnsupportedUrl(String),

    #[error("request failed: {0}")]
    Transport(#[from] ureq::Error),

    #[error("could not read response body: {0}")]
    Body(String),
}

/// Fetch `url` with the configured ceilings.
///
/// Body decoding stops at `max_bytes`; the rest of the stream is
/// dropped. HTML is returned as a string (charset handling is a
/// known gap tracked in the roadmap).
pub fn fetch(url: &str, opts: &FetchOptions) -> Result<FetchResult, NetError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(NetError::UnsupportedUrl(url.to_string()));
    }

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(opts.timeout))
        .max_redirects(opts.max_redirects)
        .user_agent(opts.user_agent.as_str())
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let mut resp = agent.get(url).call()?;
    let status = resp.status().as_u16();
    let final_url = resp.get_uri().to_string();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let body = resp
        .body_mut()
        .with_config()
        .limit(opts.max_bytes)
        .read_to_string()
        .map_err(|e| NetError::Body(e.to_string()))?;

    Ok(FetchResult {
        final_url,
        status,
        content_type,
        body,
    })
}
