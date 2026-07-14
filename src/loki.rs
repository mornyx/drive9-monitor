use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// A single log line returned from a Loki query.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Timestamp parsed from the Loki nanosecond-epoch string.
    pub ts: DateTime<Utc>,
    /// Stream labels associated with this entry.
    pub labels: BTreeMap<String, String>,
    /// Raw log line.
    pub line: String,
}

/// Query direction for Loki query_range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "forward" => Ok(Direction::Forward),
            "backward" => Ok(Direction::Backward),
            other => bail!(
                "invalid direction '{}': expected 'forward' or 'backward'",
                other
            ),
        }
    }
}

/// Loki HTTP + WebSocket client.
pub struct LokiClient {
    endpoint: String,
    http: Client,
}

/// Loki API response envelope: `{ "status": "success", "data": ... }`.
#[derive(Deserialize)]
struct LokiResponse<T> {
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_type: Option<String>,
    data: T,
}

/// Stream entry in a query_range result: `{ "stream": {...}, "values": [[ns, line], ...] }`.
#[derive(Deserialize)]
struct StreamEntry {
    stream: BTreeMap<String, String>,
    values: Vec<(String, String)>,
}

/// query_range data payload: `{ "resultType": "streams", "result": [...], "stats": {...} }`.
#[derive(Deserialize)]
struct QueryRangeData {
    #[serde(rename = "resultType")]
    #[allow(dead_code)]
    result_type: String,
    result: Vec<StreamEntry>,
}

/// Tail (WebSocket) message: `{ "streams": [...], "dropped_entries": [...] }`.
#[derive(Deserialize)]
#[allow(dead_code)]
struct TailMessage {
    #[serde(default)]
    streams: Vec<StreamEntry>,
    #[serde(default)]
    dropped_entries: Vec<DroppedEntry>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct DroppedEntry {
    labels: BTreeMap<String, String>,
    timestamp: String,
}

impl LokiClient {
    pub fn new(endpoint: &str) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            http,
        })
    }

    /// Build a full API URL: `<endpoint>/api/v1/<path>`.
    fn api_url(&self, path: &str) -> String {
        format!("{}/api/v1/{}", self.endpoint, path)
    }

    /// Build a full WebSocket URL: `wss://<host>/api/v1/<path>`.
    fn ws_url(&self, path: &str) -> String {
        let base = self.endpoint.replacen("https://", "wss://", 1);
        format!("{}/api/v1/{}", base, path)
    }

    /// Query logs via `/api/v1/query_range`.
    pub async fn query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: u32,
        direction: Direction,
    ) -> Result<Vec<LogEntry>> {
        let url = self.api_url("query_range");
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("query", query),
                (
                    "start",
                    &start.timestamp_nanos_opt().unwrap_or(0).to_string(),
                ),
                ("end", &end.timestamp_nanos_opt().unwrap_or(0).to_string()),
                ("limit", &limit.to_string()),
                ("direction", direction.as_str()),
            ])
            .send()
            .await
            .context("failed to send query_range request")?;

        let status = resp.status();
        let body = resp.text().await.context("failed to read response body")?;

        if !status.is_success() {
            check_http_error(status, &body)?;
        }

        let parsed: LokiResponse<QueryRangeData> = serde_json::from_str(&body)
            .with_context(|| format!("failed to parse query_range response: {}", body))?;

        if parsed.status != "success" {
            bail!(
                "loki error: {} ({})",
                parsed.error.unwrap_or_else(|| "unknown".into()),
                parsed.error_type.unwrap_or_else(|| "unknown".into())
            );
        }

        let mut entries = Vec::new();
        for stream in parsed.data.result {
            for (ns_ts, line) in stream.values {
                let ts = parse_ns_timestamp(&ns_ts)?;
                entries.push(LogEntry {
                    ts,
                    labels: stream.stream.clone(),
                    line,
                });
            }
        }
        Ok(entries)
    }

    /// Tail logs via WebSocket `/api/v1/tail`.
    ///
    /// Calls `on_entry` for each log entry received. Returns when the
    /// WebSocket closes or an error occurs.
    pub async fn tail<F>(&self, query: &str, mut on_entry: F) -> Result<()>
    where
        F: FnMut(LogEntry),
    {
        // Loki tail uses a WebSocket connection with query params.
        let ws_url = format!(
            "{}?query={}",
            self.ws_url("tail"),
            urlencoding::encode_query(query)
        );

        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .context("failed to connect to tail WebSocket")?;

        let (_, mut read) = ws_stream.split();

        while let Some(msg) = read.next().await {
            let msg = msg.context("WebSocket error")?;
            match msg {
                Message::Text(text) => {
                    let parsed: TailMessage = match serde_json::from_str(&text) {
                        Ok(m) => m,
                        Err(_) => continue, // skip unparseable frames
                    };
                    for stream in parsed.streams {
                        for (ns_ts, line) in stream.values {
                            if let Ok(ts) = parse_ns_timestamp(&ns_ts) {
                                on_entry(LogEntry {
                                    ts,
                                    labels: stream.stream.clone(),
                                    line,
                                });
                            }
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        Ok(())
    }
}

/// Check HTTP status and produce an appropriate error.
/// 403 is special-cased with a Feilian hint.
fn check_http_error(status: reqwest::StatusCode, body: &str) -> Result<()> {
    if status.as_u16() == 403 {
        bail!(
            "HTTP 403 Forbidden — endpoint rejected the request.\n\
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

/// Parse a Loki nanosecond-epoch timestamp string into a DateTime.
fn parse_ns_timestamp(ns: &str) -> Result<DateTime<Utc>> {
    let ns: i64 = ns
        .parse()
        .with_context(|| format!("invalid timestamp: {}", ns))?;
    let secs = ns / 1_000_000_000;
    let subsec_nanos = ns % 1_000_000_000;
    DateTime::<Utc>::from_timestamp(secs, subsec_nanos as u32)
        .with_context(|| format!("timestamp out of range: {}", ns))
}

/// Truncate a string to `max` chars, appending "..." if truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Minimal URL query encoder for the WebSocket tail URL.
/// We avoid pulling in another crate just for this.
mod urlencoding {
    pub fn encode_query(s: &str) -> String {
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
}
