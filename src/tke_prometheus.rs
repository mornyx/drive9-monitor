use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::http;
use crate::prom::{self, MetricSeries};
use crate::tencentcloud::TcClient;

/// TKE TMP Prometheus client via ExportPrometheusReadOnlyDynamicAPI.
pub struct TkePromClient {
    tc: TcClient,
    instance_id: String,
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
            http::url_encode(query),
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

        prom::parse_matrix(response_body)
    }
}
