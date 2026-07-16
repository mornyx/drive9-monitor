use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::http;

/// A single alert from Alertmanager API v2.
#[derive(Debug, Clone)]
pub struct Alert {
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub state: String,
    pub fingerprint: String,
}

/// Alertmanager HTTP API v2 client.
pub struct AmClient {
    endpoint: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct RawAlert {
    labels: BTreeMap<String, String>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
    startsAt: String,
    endsAt: String,
    status: AlertStatus,
    fingerprint: String,
}

#[derive(Deserialize)]
struct AlertStatus {
    state: String,
}

impl AmClient {
    pub fn new(endpoint: &str) -> Result<Self> {
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            http: http::build_client()?,
        })
    }

    /// Query alerts via `/api/v2/alerts`.
    pub async fn alerts(
        &self,
        filters: &[String],
        active: bool,
        silenced: bool,
        inhibited: bool,
    ) -> Result<Vec<Alert>> {
        let url = format!("{}/api/v2/alerts", self.endpoint);

        let mut req = self.http.get(&url);
        for f in filters {
            req = req.query(&[("filter", f.as_str())]);
        }
        let body = http::send_checked(
            req.query(&[
                ("active", if active { "true" } else { "false" }),
                ("silenced", if silenced { "true" } else { "false" }),
                ("inhibited", if inhibited { "true" } else { "false" }),
            ]),
            "failed to send alerts request",
        )
        .await?;

        let raw: Vec<RawAlert> = serde_json::from_str(&body).with_context(|| {
            format!(
                "failed to parse alerts response: {}",
                http::truncate(&body, 500)
            )
        })?;

        let mut alerts = Vec::with_capacity(raw.len());
        for r in raw {
            alerts.push(Alert {
                starts_at: parse_rfc3339(&r.startsAt)
                    .with_context(|| format!("invalid startsAt for alert '{}'", r.fingerprint))?,
                ends_at: parse_rfc3339(&r.endsAt)
                    .with_context(|| format!("invalid endsAt for alert '{}'", r.fingerprint))?,
                labels: r.labels,
                annotations: r.annotations,
                state: r.status.state,
                fingerprint: r.fingerprint,
            });
        }
        Ok(alerts)
    }
}

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("invalid timestamp: {}", s))
}
