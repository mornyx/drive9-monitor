//! Shared HTTP helpers for all API clients.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, RequestBuilder, StatusCode};

/// Build an HTTP client with the standard 30s timeout used by all clients.
pub fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")
}

/// Send a request and return the response body on success.
///
/// Non-2xx responses are converted to errors via [`check_status`].
pub async fn send_checked(req: RequestBuilder, ctx: &'static str) -> Result<String> {
    let resp = req.send().await.context(ctx)?;
    let status = resp.status();
    let body = resp.text().await.context("failed to read response body")?;
    check_status(status, &body)?;
    Ok(body)
}

/// Check HTTP status and produce an appropriate error.
///
/// 403 is special-cased with a Feilian/VPN hint, since the observability
/// endpoints are only reachable via Feilian.
pub fn check_status(status: StatusCode, body: &str) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    if status.as_u16() == 403 {
        bail!(
            "HTTP 403 Forbidden\n\
             This endpoint is only reachable via Feilian/VPN.\n\
             Please connect to Feilian first, then retry.\n\
             \n\
             response body: {}",
            truncate(body, 500)
        );
    }
    bail!(
        "HTTP {} — {}\nresponse body: {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("error"),
        truncate(body, 500)
    );
}

/// Truncate a string to at most `max` bytes (char-boundary safe), appending
/// "..." if truncated.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Minimal URL percent-encoding for query strings.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", byte));
            }
        }
    }
    out
}
