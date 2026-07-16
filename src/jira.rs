use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use chrono::{DateTime, Local};
use reqwest::Client;
use serde::Deserialize;

/// A single Jira issue from the REST API v3 search endpoint.
#[derive(Debug, Clone)]
pub struct JiraIssue {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub status_category: String,
    pub priority: String,
    pub created: DateTime<Local>,
    pub updated: DateTime<Local>,
    pub project_key: String,
    pub project_name: String,
    pub components: Vec<String>,
    pub description: String,
}

/// Jira REST API v3 client.
pub struct JiraClient {
    endpoint: String,
    auth_header: String,
    http: Client,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct SearchResponse {
    #[serde(default)]
    issues: Vec<RawIssue>,
    #[serde(default)]
    nextPageToken: Option<String>,
    #[serde(default)]
    isLast: Option<bool>,
}

#[derive(Deserialize)]
struct RawIssue {
    key: String,
    fields: RawFields,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct RawFields {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    status: Option<RawStatus>,
    #[serde(default)]
    priority: Option<RawNamed>,
    #[serde(default)]
    created: String,
    #[serde(default)]
    updated: String,
    #[serde(default)]
    project: Option<RawProject>,
    #[serde(default)]
    components: Vec<RawNamed>,
    #[serde(default)]
    description: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct RawStatus {
    name: String,
    #[serde(default)]
    statusCategory: Option<RawStatusCategory>,
}

#[derive(Deserialize)]
struct RawStatusCategory {
    key: String,
}

#[derive(Deserialize)]
struct RawNamed {
    name: String,
}

#[derive(Deserialize)]
struct RawProject {
    key: String,
    #[serde(default)]
    name: String,
}

impl JiraClient {
    pub fn new(endpoint: &str, email: &str, token: &str) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        let credentials = format!("{}:{}", email, token);
        let auth_header = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials)
        );
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            auth_header,
            http,
        })
    }

    /// Search Jira issues via `/rest/api/3/search/jql`.
    ///
    /// Uses cursor-based pagination to fetch up to `limit` issues (0 = all).
    pub async fn search(&self, jql: &str, limit: usize) -> Result<Vec<JiraIssue>> {
        let url = format!("{}/rest/api/3/search/jql", self.endpoint);
        let fields = "summary,status,priority,created,updated,project,components,description";
        let page_size = 100usize;

        let mut all_issues: Vec<JiraIssue> = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut req = self
                .http
                .get(&url)
                .header("Authorization", &self.auth_header)
                .header("Accept", "application/json")
                .query(&[
                    ("jql", jql),
                    ("fields", fields),
                    ("maxResults", &page_size.to_string()),
                ]);
            if let Some(token) = &page_token {
                req = req.query(&[("nextPageToken", token)]);
            }

            let resp = req
                .send()
                .await
                .context("failed to send Jira search request")?;
            let status = resp.status();
            let body = resp
                .text()
                .await
                .context("failed to read Jira response body")?;

            if status.as_u16() == 401 || status.as_u16() == 403 {
                bail!(
                    "HTTP {} — Jira authentication failed.\n\
                     Check `jira_email` and `jira_token` in config — the API token may be expired or revoked.\n\
                     \n\
                     response body: {}",
                    status.as_u16(),
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

            let parsed: SearchResponse = serde_json::from_str(&body).with_context(|| {
                format!("failed to parse Jira response: {}", truncate(&body, 500))
            })?;

            for raw in parsed.issues {
                let issue = parse_issue(&raw)?;
                all_issues.push(issue);
                if limit > 0 && all_issues.len() >= limit {
                    all_issues.truncate(limit);
                    return Ok(all_issues);
                }
            }

            // Cursor pagination: stop if isLast is true or no next token.
            if parsed.isLast == Some(true) || parsed.nextPageToken.is_none() {
                break;
            }
            page_token = parsed.nextPageToken;
        }

        Ok(all_issues)
    }
}

fn parse_issue(raw: &RawIssue) -> Result<JiraIssue> {
    let f = &raw.fields;
    let status = f
        .status
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let status_category = f
        .status
        .as_ref()
        .and_then(|s| s.statusCategory.as_ref())
        .map(|c| c.key.clone())
        .unwrap_or_default();
    let priority = f
        .priority
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let (project_key, project_name) = f
        .project
        .as_ref()
        .map(|p| (p.key.clone(), p.name.clone()))
        .unwrap_or_default();
    let components = f.components.iter().map(|c| c.name.clone()).collect();
    let description = f
        .description
        .as_ref()
        .map(extract_text_from_adf)
        .unwrap_or_default();

    Ok(JiraIssue {
        key: raw.key.clone(),
        summary: f.summary.clone(),
        status,
        status_category,
        priority,
        created: parse_jira_ts(&f.created)?,
        updated: parse_jira_ts(&f.updated)?,
        project_key,
        project_name,
        components,
        description,
    })
}

/// Extract plain text from Atlassian Document Format (ADF) JSON.
/// Block-level elements (paragraph, heading, panel) are separated by newlines.
fn extract_text_from_adf(value: &serde_json::Value) -> String {
    let mut lines = Vec::new();
    extract_adf_lines(value, &mut lines);
    lines.join("\n")
}

fn extract_adf_lines(value: &serde_json::Value, lines: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(obj) => {
            // Block-level types that should start a new line.
            let block_types = [
                "paragraph",
                "heading",
                "panel",
                "bulletList",
                "listItem",
                "codeBlock",
                "blockquote",
            ];
            let is_block = obj
                .get("type")
                .and_then(|v| v.as_str())
                .is_some_and(|t| block_types.contains(&t));

            if is_block {
                // Collect inline text within this block, then push as a line.
                let mut inline = Vec::new();
                if let Some(content) = obj.get("content") {
                    collect_inline_text(content, &mut inline);
                }
                if !inline.is_empty() {
                    lines.push(inline.join(""));
                }
            } else {
                // Not a block — recurse into all values.
                for v in obj.values() {
                    extract_adf_lines(v, lines);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_adf_lines(v, lines);
            }
        }
        _ => {}
    }
}

/// Collect inline text (text nodes) from a content array, preserving spaces.
fn collect_inline_text(value: &serde_json::Value, texts: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(obj) => {
            if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                texts.push(t.to_string());
            }
            for v in obj.values() {
                collect_inline_text(v, texts);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_inline_text(v, texts);
            }
        }
        _ => {}
    }
}

/// Parse a Jira timestamp (e.g. `2026-07-14T20:18:19.294+0800`) into Local time.
fn parse_jira_ts(s: &str) -> Result<DateTime<Local>> {
    // Jira uses `+0800` timezone offset (no colon). chrono's `%z` handles this.
    // Try RFC3339 first (with colon offset), then Jira format.
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .map(|dt| dt.with_timezone(&Local))
        .with_context(|| format!("invalid Jira timestamp: {}", s))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
