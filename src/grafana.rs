use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::loki::{Direction, LogEntry};
use crate::victoriametrics::MetricSeries;

#[derive(Deserialize)]
struct ApiResponse {
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, rename = "errorType")]
    error_type: Option<String>,
    data: Option<QueryRangeData>,
}

#[derive(Deserialize)]
struct QueryRangeData {
    #[serde(rename = "resultType")]
    #[allow(dead_code)]
    result_type: String,
    result: Vec<SeriesEntry>,
}

#[derive(Deserialize)]
struct SeriesEntry {
    metric: BTreeMap<String, String>,
    values: Vec<(f64, String)>,
}

/// Loki API response envelope.
#[derive(Deserialize)]
struct LokiResponse<T> {
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_type: Option<String>,
    data: Option<T>,
}

/// Loki query_range data.
#[derive(Deserialize)]
struct LokiRangeData {
    #[serde(rename = "resultType")]
    #[allow(dead_code)]
    result_type: String,
    result: Vec<LokiStreamEntry>,
}

/// Loki stream entry.
#[derive(Deserialize)]
struct LokiStreamEntry {
    stream: BTreeMap<String, String>,
    values: Vec<(String, String)>,
}

/// Grafana datasource proxy client.
/// Queries Prometheus/Loki through Grafana's datasource proxy API with Basic Auth.
/// Uses UID-based proxy path: /api/datasources/proxy/uid/<uid>/api/v1/...
pub struct GrafanaClient {
    base_url: String,
    datasource_uid: String,
    http: Client,
}

impl GrafanaClient {
    pub fn new(base_url: &str, datasource_uid: &str) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            datasource_uid: datasource_uid.to_string(),
            http,
        })
    }

    pub async fn query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step: Duration,
        username: &str,
        password: &str,
    ) -> Result<Vec<MetricSeries>> {
        let url = format!(
            "{}/api/datasources/proxy/uid/{}/api/v1/query_range",
            self.base_url, self.datasource_uid
        );

        let resp = self
            .http
            .get(&url)
            .basic_auth(username, Some(password))
            .query(&[
                ("query", query),
                ("start", start.timestamp().to_string().as_str()),
                ("end", end.timestamp().to_string().as_str()),
                ("step", format!("{}s", step.as_secs_f64()).as_str()),
            ])
            .send()
            .await
            .context("failed to send query_range request")?;

        let status = resp.status();
        let body = resp.text().await.context("failed to read response body")?;

        if status.as_u16() == 403 {
            bail!(
                "HTTP 403 Forbidden\n\
                 This endpoint may require VPN access or correct credentials.\n\
                 \n\
                 response body: {}",
                truncate(&body, 500)
            );
        }

        if !status.is_success() {
            bail!(
                "HTTP {} — {}\nresponse body: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("error"),
                truncate(&body, 500)
            );
        }

        let parsed: ApiResponse = serde_json::from_str(&body).with_context(|| {
            format!(
                "failed to parse query_range response: {}",
                truncate(&body, 500)
            )
        })?;

        if parsed.status != "success" {
            bail!(
                "prometheus error: {} ({})",
                parsed.error.unwrap_or_else(|| "unknown".into()),
                parsed.error_type.unwrap_or_else(|| "unknown".into())
            );
        }

        let data = parsed.data.context("response has no data field")?;
        let mut series = Vec::new();
        for entry in data.result {
            let mut points = Vec::new();
            for (ts, val) in entry.values {
                let dt = DateTime::<Utc>::from_timestamp(ts as i64, 0)
                    .context("invalid timestamp in response")?;
                let v: f64 = val.parse().unwrap_or(f64::NAN);
                points.push((dt, v));
            }
            series.push(MetricSeries {
                metric: entry.metric,
                points,
            });
        }
        Ok(series)
    }

    /// Query logs via Grafana datasource proxy to a Loki datasource.
    #[allow(clippy::too_many_arguments)]
    pub async fn loki_query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: u32,
        direction: Direction,
        username: &str,
        password: &str,
    ) -> Result<Vec<LogEntry>> {
        let url = format!(
            "{}/api/datasources/proxy/uid/{}/loki/api/v1/query_range",
            self.base_url, self.datasource_uid
        );

        let start_ns = start.timestamp_nanos_opt().unwrap_or(0).to_string();
        let end_ns = end.timestamp_nanos_opt().unwrap_or(0).to_string();

        let resp = self
            .http
            .get(&url)
            .basic_auth(username, Some(password))
            .query(&[
                ("query", query),
                ("start", start_ns.as_str()),
                ("end", end_ns.as_str()),
                ("limit", limit.to_string().as_str()),
                ("direction", direction.as_str()),
            ])
            .send()
            .await
            .context("failed to send loki query_range request")?;

        let status = resp.status();
        let body = resp.text().await.context("failed to read response body")?;

        if status.as_u16() == 403 {
            bail!(
                "HTTP 403 Forbidden\n\
                 This endpoint may require VPN access or correct credentials.\n\
                 \n\
                 response body: {}",
                truncate(&body, 500)
            );
        }

        if !status.is_success() {
            bail!(
                "HTTP {} — {}\nresponse body: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("error"),
                truncate(&body, 500)
            );
        }

        let parsed: LokiResponse<LokiRangeData> =
            serde_json::from_str(&body).with_context(|| {
                format!(
                    "failed to parse loki query_range response: {}",
                    truncate(&body, 500)
                )
            })?;

        if parsed.status != "success" {
            bail!(
                "loki error: {} ({})",
                parsed.error.unwrap_or_else(|| "unknown".into()),
                parsed.error_type.unwrap_or_else(|| "unknown".into())
            );
        }

        let data = parsed.data.context("response has no data field")?;
        let mut entries = Vec::new();
        for stream in data.result {
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
