use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

/// A single time-series result from a query_range call.
#[derive(Debug, Clone)]
pub struct MetricSeries {
    /// Label set identifying this series.
    pub metric: BTreeMap<String, String>,
    /// Time-series data points: (timestamp, value).
    pub points: Vec<(DateTime<Utc>, f64)>,
}

/// VictoriaMetrics HTTP client (Prometheus-compatible API).
pub struct VmClient {
    endpoint: String,
    http: Client,
}

/// API response envelope: `{ "status": "success", "data": ... }`.
#[derive(Deserialize)]
struct ApiResponse {
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, rename = "errorType")]
    error_type: Option<String>,
    data: Option<QueryRangeData>,
}

/// query_range data: `{ "resultType": "matrix", "result": [...] }`.
#[derive(Deserialize)]
struct QueryRangeData {
    #[serde(rename = "resultType")]
    #[allow(dead_code)]
    result_type: String,
    result: Vec<SeriesEntry>,
}

/// A series entry in the result.
#[derive(Deserialize)]
struct SeriesEntry {
    metric: BTreeMap<String, String>,
    values: Vec<(f64, String)>,
}

impl VmClient {
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

    /// Query metrics via `/api/v1/query_range`.
    pub async fn query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step: Duration,
    ) -> Result<Vec<MetricSeries>> {
        let url = format!("{}/api/v1/query_range", self.endpoint);
        let step_str = format!("{}s", step.as_secs_f64());

        let resp = self
            .http
            .get(&url)
            .query(&[
                ("query", query),
                ("start", &start.timestamp().to_string()),
                ("end", &end.timestamp().to_string()),
                ("step", &step_str),
            ])
            .send()
            .await
            .context("failed to send query_range request")?;

        let status = resp.status();
        let body = resp.text().await.context("failed to read response body")?;

        if !status.is_success() {
            check_http_error(status, &body)?;
        }

        let parsed: ApiResponse = serde_json::from_str(&body)
            .with_context(|| format!("failed to parse query_range response: {}", truncate(&body, 500)))?;

        if parsed.status != "success" {
            bail!(
                "victoriametrics error: {} ({})",
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
}

/// Check HTTP status and produce an appropriate error.
fn check_http_error(status: reqwest::StatusCode, body: &str) -> Result<()> {
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}