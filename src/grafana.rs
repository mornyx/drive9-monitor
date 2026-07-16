use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::http;
use crate::loki::{Direction, LogEntry};
use crate::prom::{self, MetricSeries};

/// Grafana datasource proxy client.
///
/// Queries Prometheus/Loki through Grafana's datasource proxy API with
/// Basic Auth. Uses UID-based proxy paths:
/// `/api/datasources/proxy/uid/<uid>/...`.
pub struct GrafanaClient {
    base_url: String,
    datasource_uid: String,
    username: String,
    password: String,
    http: reqwest::Client,
}

impl GrafanaClient {
    pub fn new(
        base_url: &str,
        datasource_uid: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            datasource_uid: datasource_uid.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            http: http::build_client()?,
        })
    }

    /// Build a proxied API URL: `<base>/api/datasources/proxy/uid/<uid>/<path>`.
    fn proxy_url(&self, path: &str) -> String {
        format!(
            "{}/api/datasources/proxy/uid/{}/{}",
            self.base_url, self.datasource_uid, path
        )
    }

    /// Query metrics via the proxied Prometheus `/api/v1/query_range`.
    pub async fn query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step: Duration,
    ) -> Result<Vec<MetricSeries>> {
        let body = http::send_checked(
            self.http
                .get(self.proxy_url("api/v1/query_range"))
                .basic_auth(&self.username, Some(&self.password))
                .query(&[
                    ("query", query),
                    ("start", start.timestamp().to_string().as_str()),
                    ("end", end.timestamp().to_string().as_str()),
                    ("step", format!("{}s", step.as_secs_f64()).as_str()),
                ]),
            "failed to send query_range request",
        )
        .await?;
        prom::parse_matrix(&body)
    }

    /// Query logs via the proxied Loki `/loki/api/v1/query_range`.
    pub async fn loki_query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: u32,
        direction: Direction,
    ) -> Result<Vec<LogEntry>> {
        let body = http::send_checked(
            self.http
                .get(self.proxy_url("loki/api/v1/query_range"))
                .basic_auth(&self.username, Some(&self.password))
                .query(&[
                    ("query", query),
                    (
                        "start",
                        start
                            .timestamp_nanos_opt()
                            .unwrap_or(0)
                            .to_string()
                            .as_str(),
                    ),
                    (
                        "end",
                        end.timestamp_nanos_opt().unwrap_or(0).to_string().as_str(),
                    ),
                    ("limit", limit.to_string().as_str()),
                    ("direction", direction.as_str()),
                ]),
            "failed to send loki query_range request",
        )
        .await?;
        crate::loki::parse_query_range(&body)
    }
}
