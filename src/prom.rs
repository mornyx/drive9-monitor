//! Shared Prometheus HTTP API (v1) response parsing.
//!
//! Used by the VictoriaMetrics, Grafana-proxy, and TKE Prometheus clients —
//! all three speak the same `/api/v1/query_range` matrix format.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::http;

/// A single time-series result from a query_range call.
#[derive(Debug, Clone)]
pub struct MetricSeries {
    /// Label set identifying this series.
    pub metric: BTreeMap<String, String>,
    /// Time-series data points: (timestamp, value).
    pub points: Vec<(DateTime<Utc>, f64)>,
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

/// Parse a Prometheus `/api/v1/query_range` matrix response body into series.
pub fn parse_matrix(body: &str) -> Result<Vec<MetricSeries>> {
    let parsed: ApiResponse = serde_json::from_str(body).with_context(|| {
        format!(
            "failed to parse query_range response: {}",
            http::truncate(body, 500)
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
