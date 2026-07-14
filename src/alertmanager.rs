use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

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
    http: Client,
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
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            http,
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
        let resp = req
            .query(&[
                ("active", if active { "true" } else { "false" }),
                ("silenced", if silenced { "true" } else { "false" }),
                ("inhibited", if inhibited { "true" } else { "false" }),
            ])
            .send()
            .await
            .context("failed to send alerts request")?;

        let status = resp.status();
        let body = resp.text().await.context("failed to read response body")?;

        if status.as_u16() == 403 {
            bail!(
                "HTTP 403 Forbidden\n\
                 This endpoint is only reachable via Feilian/VPN.\n\
                 Please connect to Feilian first, then retry.\n\
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

        let raw: Vec<RawAlert> = serde_json::from_str(&body)
            .with_context(|| format!("failed to parse alerts response: {}", truncate(&body, 500)))?;

        let alerts = raw
            .into_iter()
            .map(|r| Alert {
                labels: r.labels,
                annotations: r.annotations,
                starts_at: parse_rfc3339(&r.startsAt).unwrap_or_else(|_| Utc::now()),
                ends_at: parse_rfc3339(&r.endsAt).unwrap_or_else(|_| Utc::now()),
                state: r.status.state,
                fingerprint: r.fingerprint,
            })
            .collect();
        Ok(alerts)
    }
}

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("invalid timestamp: {}", s))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}