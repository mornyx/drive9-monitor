use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::tencentcloud::TcClient;
use crate::victoriametrics::MetricSeries;

/// TKE TMP Prometheus client via ExportPrometheusReadOnlyDynamicAPI.
pub struct TkePromClient {
    tc: TcClient,
    instance_id: String,
}

/// Prometheus API response (wrapped in TKE's HTTP.ResponseBody).
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

impl TkePromClient {
    pub fn new(secret_id: &str, secret_key: &str, instance_id: &str, region: &str) -> Result<Self> {
        let tc = TcClient::new(
            secret_id,
            secret_key,
            "monitor",
            "monitor.tencentcloudapi.com",
            region,
        )?;
        Ok(Self {
            tc,
            instance_id: instance_id.to_string(),
        })
    }

    /// Query metrics via the TKE Prometheus proxy API.
    pub async fn query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step: Duration,
    ) -> Result<Vec<MetricSeries>> {
        let path = format!(
            "/api/v1/query_range?query={}&start={}&end={}&step={}s",
            url_encode(query),
            start.timestamp(),
            end.timestamp(),
            step.as_secs_f64()
        );

        let body = serde_json::json!({
            "InstanceId": self.instance_id,
            "Method": "GET",
            "Path": path,
        });

        let response = self
            .tc
            .call_api("ExportPrometheusReadOnlyDynamicAPI", "2018-07-24", &body)
            .await?;

        // Extract HTTP.ResponseBody (a JSON string) from the response.
        let response_body = response
            .get("HTTP")
            .and_then(|h| h.get("ResponseBody"))
            .and_then(|r| r.as_str())
            .context("response missing HTTP.ResponseBody")?;

        let parsed: ApiResponse = serde_json::from_str(response_body).with_context(|| {
            format!(
                "failed to parse Prometheus response: {}",
                &response_body[..response_body.len().min(500)]
            )
        })?;

        if parsed.status != "success" {
            anyhow::bail!(
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
}

/// Minimal URL percent-encoding for query strings.
fn url_encode(s: &str) -> String {
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
