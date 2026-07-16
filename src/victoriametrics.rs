use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::http;
use crate::prom::{self, MetricSeries};

/// VictoriaMetrics HTTP client (Prometheus-compatible API).
pub struct VmClient {
    endpoint: String,
    http: reqwest::Client,
}

impl VmClient {
    pub fn new(endpoint: &str) -> Result<Self> {
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            http: http::build_client()?,
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
        let body = http::send_checked(
            self.http.get(&url).query(&[
                ("query", query),
                ("start", &start.timestamp().to_string()),
                ("end", &end.timestamp().to_string()),
                ("step", &format!("{}s", step.as_secs_f64())),
            ]),
            "failed to send query_range request",
        )
        .await?;
        prom::parse_matrix(&body)
    }
}
