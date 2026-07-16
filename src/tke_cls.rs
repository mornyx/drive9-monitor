use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::loki::LogEntry;
use crate::tencentcloud::TcClient;

/// TKE CLS (Cloud Log Service) log client.
pub struct TkeClsClient {
    tc: TcClient,
    topic_id: String,
}

/// CLS SearchLog response.
#[derive(Deserialize)]
#[allow(non_snake_case)]
struct SearchLogResponse {
    #[serde(default)]
    Results: Vec<ClsLogInfo>,
    #[serde(default)]
    #[allow(dead_code)]
    Context: String,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct ClsLogInfo {
    Time: i64,
    LogJson: String,
}

impl TkeClsClient {
    pub fn new(secret_id: &str, secret_key: &str, topic_id: &str, region: &str) -> Result<Self> {
        let tc = TcClient::new(
            secret_id,
            secret_key,
            "cls",
            "cls.tencentcloudapi.com",
            region,
        )?;
        Ok(Self {
            tc,
            topic_id: topic_id.to_string(),
        })
    }

    /// Search logs via CLS SearchLog API.
    pub async fn search_log(
        &self,
        from_ms: i64,
        to_ms: i64,
        limit: u32,
        query: &str,
    ) -> Result<Vec<LogEntry>> {
        let body = serde_json::json!({
            "TopicId": self.topic_id,
            "From": from_ms,
            "To": to_ms,
            "Limit": limit,
            "Query": query,
        });

        let response = self.tc.call_api("SearchLog", "2020-10-16", &body).await?;

        let parsed: SearchLogResponse =
            serde_json::from_value(response).context("failed to parse SearchLog response")?;

        let mut entries = Vec::new();
        for log_info in parsed.Results {
            let ts = DateTime::<Utc>::from_timestamp(
                log_info.Time / 1000,
                ((log_info.Time % 1000) * 1_000_000) as u32,
            )
            .with_context(|| format!("invalid CLS log timestamp: {}", log_info.Time))?;
            entries.push(LogEntry {
                ts,
                labels: BTreeMap::new(),
                line: log_info.LogJson,
            });
        }
        Ok(entries)
    }
}
