use anyhow::{Context, Result, bail};
use colored::Colorize;

use crate::commands::common;
use crate::config::Config;
use crate::jira::{JiraClient, JiraIssue};
use crate::labels::LabelMap;

/// Output format for jira-alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Arguments for the `jira-alerts` subcommand.
pub struct JiraAlertsArgs {
    pub query: Option<String>,
    pub limit: usize,
    pub output: OutputFormat,
}

/// Entry point for the `jira-alerts` subcommand.
pub async fn run(config: &Config, args: JiraAlertsArgs) -> Result<()> {
    let jira = config.jira.as_ref().context(
        "no [jira] section in config — add a `[jira]` table with `endpoint`, `email`, and `token`",
    )?;

    let endpoint = jira.endpoint.trim_end_matches('/');
    let client = JiraClient::new(endpoint, &jira.email, &jira.token)?;

    let jql = build_jql(&args.query, &jira.labels)?;
    let issues = client.search(&jql, args.limit).await?;

    match args.output {
        OutputFormat::Json => print_json(&issues),
        OutputFormat::Text => print_text(&issues, common::use_color()),
    }

    Ok(())
}

/// Build a JQL query from user query + config labels.
///
/// Config labels are converted to `key = "value"` pairs and AND-joined.
/// The user query (if provided) is AND-ed with the base conditions.
/// `ORDER BY created DESC` is appended automatically.
///
/// Jira's `/search/jql` endpoint rejects unrestricted queries (no WHERE clause),
/// so at least one condition is required.
fn build_jql(opt_query: &Option<String>, config_labels: &LabelMap) -> Result<String> {
    let mut conditions: Vec<String> = config_labels
        .iter()
        .map(|(k, v)| format!("{} = \"{}\"", k, v))
        .collect();

    if let Some(q) = opt_query {
        let q = q.trim();
        if !q.is_empty() {
            conditions.push(q.to_string());
        }
    }

    if conditions.is_empty() {
        bail!(
            "Jira requires at least one filter condition.\n\
             Set `labels` under `[jira]` in config (e.g. `labels = {{component = \"drive9\"}}`)\n\
             or provide a JQL query as a positional argument (e.g. `drive9-monitor jira-alerts 'component = \"drive9\"'`)."
        );
    }

    let mut jql = conditions.join(" AND ");
    jql.push_str(" ORDER BY created DESC");
    Ok(jql)
}

fn print_text(issues: &[JiraIssue], use_color: bool) {
    if issues.is_empty() {
        println!("no tickets found");
        return;
    }

    for issue in issues {
        let ts = issue.created.format("%Y-%m-%d %H:%M:%S %:z").to_string();
        let updated = issue.updated.format("%Y-%m-%d %H:%M:%S %:z").to_string();

        if use_color {
            println!(
                "{} {} {} {} {} {{",
                ts.cyan(),
                colorize_priority(&issue.priority),
                issue.key.bold(),
                issue.status.dimmed(),
                issue.summary,
            );
        } else {
            println!(
                "{} {} {} {} {} {{",
                ts, issue.priority, issue.key, issue.status, issue.summary
            );
        }

        let field = |k: &str, v: &str| {
            if use_color {
                format!("    {}={}", k.dimmed(), v)
            } else {
                format!("    {}={}", k, v)
            }
        };

        println!("{},", field("created", &ts));
        println!("{},", field("updated", &updated));
        println!("{},", field("project", &issue.project_key));
        println!(
            "{},",
            field("components", &format!("[{}]", issue.components.join(", ")))
        );
        // Description is multi-line; indent continuation lines for readability.
        let desc_lines: Vec<&str> = issue.description.lines().collect();
        if desc_lines.len() <= 1 {
            println!("{},", field("description", &issue.description));
        } else {
            for (i, line) in desc_lines.iter().enumerate() {
                if i == 0 {
                    println!("{},", field("description", line));
                } else {
                    println!("      {}", line);
                }
            }
        }
        println!("}}");
    }
}

fn print_json(issues: &[JiraIssue]) {
    let result: Vec<serde_json::Value> = issues
        .iter()
        .map(|i| {
            serde_json::json!({
                "key": i.key,
                "summary": i.summary,
                "status": i.status,
                "statusCategory": i.status_category,
                "priority": i.priority,
                "created": i.created.to_rfc3339(),
                "updated": i.updated.to_rfc3339(),
                "project": {"key": i.project_key, "name": i.project_name},
                "components": i.components,
                "description": i.description,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

fn colorize_priority(priority: &str) -> colored::ColoredString {
    match priority {
        "blocker" | "critical" | "严重" | "最高" | "Highest" | "P0" => priority.red(),
        "major" | "重要" | "高" | "High" | "P1" => priority.yellow(),
        _ => priority.normal(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> LabelMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn jql_from_labels_only() {
        let jql = build_jql(&None, &labels(&[("component", "drive9")])).unwrap();
        assert_eq!(jql, r#"component = "drive9" ORDER BY created DESC"#);
    }

    #[test]
    fn jql_ands_user_query() {
        let jql = build_jql(
            &Some(r#"statusCategory != "Done""#.to_string()),
            &labels(&[("component", "drive9")]),
        )
        .unwrap();
        assert_eq!(
            jql,
            r#"component = "drive9" AND statusCategory != "Done" ORDER BY created DESC"#
        );
    }

    #[test]
    fn jql_errors_without_any_condition() {
        assert!(build_jql(&None, &LabelMap::new()).is_err());
        assert!(build_jql(&Some("  ".to_string()), &LabelMap::new()).is_err());
    }
}
