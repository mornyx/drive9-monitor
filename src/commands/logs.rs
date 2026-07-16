use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;

use crate::commands::common;
use crate::config::{Config, SignalConfig};
use crate::labels::{self, LabelMap};
use crate::loki::{Direction, LogEntry, LokiClient};

/// Output format for log entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Raw,
    Json,
}

/// Arguments for the `logs` subcommand.
pub struct LogsArgs {
    pub cluster: Option<String>,
    pub query: Option<String>,
    pub since: std::time::Duration,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: u32,
    pub direction: Direction,
    pub follow: bool,
    pub output: OutputFormat,
}

/// Entry point for the `logs` subcommand.
pub async fn run(config: &Config, args: LogsArgs) -> Result<()> {
    let cluster_key = config.resolve_cluster_key(args.cluster.as_deref())?;
    let cluster = config.cluster(&cluster_key)?;
    let logs = cluster.logs()?;
    let use_color = common::use_color();

    match logs.source_type.as_str() {
        "loki" => {
            let client = LokiClient::new(&logs.endpoint)?;
            let query = resolve_query(&args.query, logs);
            if args.follow {
                client
                    .tail(&query, |entry| print_entry(&entry, args.output, use_color))
                    .await
            } else {
                let (start, end) = common::resolve_time_range(args.since, args.from, args.to);
                let entries = client
                    .query_range(&query, start, end, args.limit, args.direction)
                    .await?;
                print_entries(entries, args.direction, args.output, use_color);
                Ok(())
            }
        }
        "grafana" => {
            if args.follow {
                anyhow::bail!("--follow is not supported for grafana source type");
            }
            let (datasource, username, password) = logs.grafana_auth()?;
            let client =
                crate::grafana::GrafanaClient::new(&logs.endpoint, datasource, username, password)?;
            let query = resolve_query(&args.query, logs);
            let (start, end) = common::resolve_time_range(args.since, args.from, args.to);
            let entries = client
                .loki_query_range(&query, start, end, args.limit, args.direction)
                .await?;
            print_entries(entries, args.direction, args.output, use_color);
            Ok(())
        }
        "tke_cls" => {
            if args.follow {
                anyhow::bail!("--follow is not supported for tke_cls source type");
            }
            let secret_id = logs
                .secret_id
                .as_deref()
                .context("tke_cls requires secret_id")?;
            let secret_key = logs
                .secret_key
                .as_deref()
                .context("tke_cls requires secret_key")?;
            let topic_id = logs
                .topic_id
                .as_deref()
                .context("tke_cls requires topic_id")?;
            let region = logs.region.as_deref().context("tke_cls requires region")?;
            let client =
                crate::tke_cls::TkeClsClient::new(secret_id, secret_key, topic_id, region)?;

            let query = resolve_cls_query(&args.query, &logs.labels);
            let (start, end) = common::resolve_time_range(args.since, args.from, args.to);
            let entries = client
                .search_log(
                    start.timestamp_millis(),
                    end.timestamp_millis(),
                    args.limit,
                    &query,
                )
                .await?;

            // CLS SearchLog always returns newest-first and has no direction
            // parameter; print in chronological order regardless of --direction.
            for entry in entries.iter().rev() {
                print_entry(entry, args.output, use_color);
            }
            Ok(())
        }
        other => anyhow::bail!("unsupported logs source_type '{}'", other),
    }
}

/// Print entries in chronological order (backward queries come back newest-first).
fn print_entries(
    entries: Vec<LogEntry>,
    direction: Direction,
    output: OutputFormat,
    use_color: bool,
) {
    let ordered: Vec<LogEntry> = if direction == Direction::Backward {
        entries.into_iter().rev().collect()
    } else {
        entries
    };
    for entry in &ordered {
        print_entry(entry, output, use_color);
    }
}

/// Build the final LogQL query.
///
/// Config labels are always applied as filters. If the user provides no
/// query, the selector is built entirely from config labels. If the user
/// provides a query, config labels are merged into the query's stream
/// selector — user-specified labels take precedence over config labels
/// for the same key.
fn resolve_query(opt_query: &Option<String>, signal: &SignalConfig) -> String {
    match opt_query {
        Some(q) => labels::merge_labels_into_query(q, &signal.labels),
        None => labels::build_selector(&signal.labels),
    }
}

/// Build the final CLS query string.
///
/// Config labels are appended as `key:value` pairs. If the user provides
/// a query, it is prepended and config labels are appended with ` AND `.
/// If no query, only the label filters are used.
fn resolve_cls_query(opt_query: &Option<String>, config_labels: &LabelMap) -> String {
    let label_filters: Vec<String> = config_labels
        .iter()
        .map(|(k, v)| format!("{}:{}", k, v))
        .collect();
    let label_str = label_filters.join(" AND ");

    match opt_query {
        Some(q) if !q.is_empty() => {
            if label_str.is_empty() {
                q.clone()
            } else {
                format!("{} AND {}", q, label_str)
            }
        }
        _ => label_str,
    }
}

/// Print a single log entry in the selected format.
fn print_entry(entry: &LogEntry, output: OutputFormat, use_color: bool) {
    match output {
        OutputFormat::Raw => {
            // <timestamp> <stream labels> <raw log line>
            let ts = entry.ts.to_rfc3339();
            let labels_str = labels::build_selector(&entry.labels);
            if use_color {
                println!("{} {} {}", ts.cyan(), labels_str.dimmed(), entry.line);
            } else {
                println!("{} {} {}", ts, labels_str, entry.line);
            }
        }
        OutputFormat::Json => {
            // Full log line as returned by Loki — plain, no highlighting.
            println!("{}", entry.line);
        }
        OutputFormat::Text => {
            // Try to parse the log line as JSON to extract structured fields.
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&entry.line) {
                if let serde_json::Value::Object(map) = obj {
                    let level = map.get("level").and_then(|v| v.as_str()).unwrap_or("?");
                    let msg = map
                        .get("msg")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&entry.line);

                    // Collect remaining fields (exclude level, msg).
                    let reserved: &[&str] = &["level", "msg"];
                    let mut extras: Vec<(&String, &serde_json::Value)> = map
                        .iter()
                        .filter(|(k, _)| !reserved.contains(&k.as_str()))
                        .collect();
                    extras.sort_by(|a, b| a.0.cmp(b.0));
                    let extra_str: Vec<String> = extras
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, format_json_value(v)))
                        .collect();
                    let extra_joined = extra_str.join(" ");

                    // Build: TIME LEVEL MESSAGE k1=v1 k2=v2 ...
                    let ts_human = format_human_time(&entry.ts);
                    let mut line = format!("{} {} {}", ts_human, level, msg);
                    if !extra_joined.is_empty() {
                        line.push_str(&format!(" {}", extra_joined));
                    }

                    if use_color {
                        let mut colored =
                            format!("{} {} {}", ts_human.cyan(), colorize_level(level), msg);
                        if !extra_joined.is_empty() {
                            colored.push_str(&format!(
                                " {}",
                                extra_str
                                    .iter()
                                    .map(|s| s.dimmed().to_string())
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            ));
                        }
                        println!("{}", colored);
                    } else {
                        println!("{}", line);
                    }
                } else {
                    // JSON but not an object — print the raw line.
                    println!("{} {}", format_human_time(&entry.ts), entry.line);
                }
            } else {
                // Not valid JSON — print the raw line.
                println!("{} {}", format_human_time(&entry.ts), entry.line);
            }
        }
    }
}

/// Format a timestamp as a human-friendly local time string with timezone.
fn format_human_time(ts: &DateTime<Utc>) -> String {
    let local = ts.with_timezone(&chrono::Local);
    local.format("%Y-%m-%d %H:%M:%S %:z").to_string()
}

/// Colorize a log level string.
fn colorize_level(level: &str) -> colored::ColoredString {
    match level {
        "error" => level.red(),
        "warn" | "warning" => level.yellow(),
        "info" => level.green(),
        "debug" => level.blue(),
        "trace" => level.magenta(),
        _ => level.normal(),
    }
}

/// Format a JSON value as a compact string for text output.
fn format_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_json_value).collect();
            format!("[{}]", items.join(","))
        }
        serde_json::Value::Object(obj) => {
            let pairs: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("{}={}", k, format_json_value(v)))
                .collect();
            format!("{{{}}}", pairs.join(","))
        }
    }
}
