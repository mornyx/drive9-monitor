use std::io::IsTerminal;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use colored::Colorize;

use crate::config::Config;
use crate::loki::{Direction, LogEntry, LokiClient};

/// Output format for log entries.
enum OutputFormat {
    Json,
    Text,
    Raw,
}

impl OutputFormat {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "raw" => Ok(Self::Raw),
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            other => anyhow::bail!(
                "invalid output format '{}': expected raw, json, or text",
                other
            ),
        }
    }
}

/// Arguments for the `logs` subcommand.
pub struct LogsArgs {
    pub cluster: Option<String>,
    pub query: Option<String>,
    pub since: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: u32,
    pub direction: String,
    pub follow: bool,
    pub output: String,
}

/// Entry point for the `logs` subcommand.
pub async fn run(config: &Config, args: LogsArgs) -> Result<()> {
    let cluster_key = config.resolve_cluster_key(args.cluster.as_deref())?;
    let cluster = config.cluster(&cluster_key)?;
    let logs = cluster.logs()?;
    let output = OutputFormat::parse(&args.output)?;
    let use_color = std::io::stdout().is_terminal();

    match logs.source_type.as_str() {
        "loki" => {
            let client = LokiClient::new(&logs.endpoint)?;
            let query = resolve_query(&args.query, logs);
            if args.follow {
                run_tail(&client, &query, &output, use_color).await
            } else {
                let direction = Direction::parse(&args.direction)?;
                let (start, end) = resolve_time_range(&args.since, &args.from, &args.to)?;
                run_query_range(
                    &client, &query, start, end, args.limit, direction, &output, use_color,
                )
                .await
            }
        }
        "tke_cls" => {
            if args.follow {
                anyhow::bail!("--follow is not supported for tke_cls source type");
            }
            let secret_id = logs
                .secret_id
                .as_ref()
                .context("tke_cls requires secret_id")?;
            let secret_key = logs
                .secret_key
                .as_ref()
                .context("tke_cls requires secret_key")?;
            let topic_id = logs
                .topic_id
                .as_ref()
                .context("tke_cls requires topic_id")?;
            let region = logs.region.as_ref().context("tke_cls requires region")?;
            let client =
                crate::tke_cls::TkeClsClient::new(secret_id, secret_key, topic_id, region)?;

            let query = resolve_cls_query(&args.query, &logs.labels);
            let (start, end) = resolve_time_range(&args.since, &args.from, &args.to)?;
            let from_ms = start.timestamp_millis();
            let to_ms = end.timestamp_millis();
            let entries = client
                .search_log(from_ms, to_ms, args.limit, &query)
                .await?;

            // CLS returns newest-first by default. Reverse for chronological order.
            for entry in entries.into_iter().rev() {
                print_entry(&entry, &output, use_color);
            }
            Ok(())
        }
        other => anyhow::bail!("unsupported logs source_type '{}'", other),
    }
}

/// Build the final LogQL query.
///
/// Config labels are always applied as filters. If the user provides no
/// query, the selector is built entirely from config labels. If the user
/// provides a query, config labels are merged into the query's stream
/// selector — user-specified labels take precedence over config labels
/// for the same key.
fn resolve_query(opt_query: &Option<String>, signal: &crate::config::SignalConfig) -> String {
    let user_query = match opt_query {
        Some(q) => q.clone(),
        None => return build_selector(&signal.labels),
    };
    merge_labels_into_query(&user_query, &signal.labels)
}

/// Build the final CLS query string.
///
/// Config labels are appended as `key:value` pairs. If the user provides
/// a query, it is prepended and config labels are appended with ` AND `.
/// If no query, only the label filters are used.
fn resolve_cls_query(
    opt_query: &Option<String>,
    config_labels: &std::collections::BTreeMap<String, String>,
) -> String {
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

/// Build a LogQL stream selector from a label map: `{key="val", ...}`.
fn build_selector(labels: &std::collections::BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return "{}".to_string();
    }
    let parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, v))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Merge config labels into a user-provided LogQL query.
///
/// The user's stream selector (the first `{...}` in the query) is parsed
/// for existing label selectors. Config labels are added for any key not
/// already present — user-specified labels take precedence. If the query
/// has no stream selector, one is prepended from config labels.
pub fn merge_labels_into_query(
    query: &str,
    config_labels: &std::collections::BTreeMap<String, String>,
) -> String {
    if config_labels.is_empty() {
        return query.to_string();
    }

    // Find the first stream selector `{...}` in the query.
    // LogQL requires balanced braces and quoted strings inside, so we
    // scan for the opening brace and track quote state to find the match.
    let bytes = query.as_bytes();
    let mut start = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && start.is_none() {
            start = Some(i);
        }
        if let Some(s) = start {
            // Scan forward respecting string literals.
            let mut j = s + 1;
            let mut in_str = false;
            while j < bytes.len() {
                let c = bytes[j];
                if in_str {
                    if c == b'\\' {
                        j += 2;
                        continue;
                    }
                    if c == b'"' {
                        in_str = false;
                    }
                } else {
                    if c == b'"' {
                        in_str = true;
                    } else if c == b'}' {
                        // Found the closing brace.
                        let selector_str = &query[s + 1..j];
                        let user_labels = parse_selector_labels(selector_str);
                        let merged = merge_label_maps(config_labels, &user_labels);
                        let new_selector = build_selector(&merged);
                        let rest = &query[j + 1..];
                        return format!("{}{}", new_selector, rest);
                    }
                }
                j += 1;
            }
            break; // Unbalanced brace — leave query as-is.
        }
        i += 1;
    }

    // No stream selector found — prepend one from config labels.
    format!("{} {}", build_selector(config_labels), query)
}

/// Parse label selectors from inside `{...}`, e.g. `service="foo", app="bar"`.
fn parse_selector_labels(s: &str) -> std::collections::BTreeMap<String, String> {
    let mut labels = std::collections::BTreeMap::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // Format: key="value" or key=~"regex" or key!="value" etc.
        // Find the operator: =, !=, =~, !~
        if let Some(eq_pos) = part.find('=') {
            let key = part[..eq_pos]
                .trim()
                .trim_end_matches('!')
                .trim_end_matches('~')
                .trim()
                .to_string();
            let val_part = part[eq_pos + 1..].trim();
            // Strip surrounding quotes.
            let val = val_part
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(val_part)
                .to_string();
            labels.insert(key, val);
        }
    }
    labels
}

/// Merge two label maps: config labels as base, user labels override.
fn merge_label_maps(
    config: &std::collections::BTreeMap<String, String>,
    user: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    let mut merged = config.clone();
    for (k, v) in user {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// Resolve the time range from --since/--from/--to flags.
fn resolve_time_range(
    since: &str,
    from: &Option<String>,
    to: &Option<String>,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let end = match to {
        Some(t) => parse_rfc3339(t)?,
        None => Utc::now(),
    };

    let start = match from {
        Some(f) => parse_rfc3339(f)?,
        None => {
            let dur = humantime::parse_duration(since).with_context(|| {
                format!(
                    "invalid --since duration '{}': expected e.g. 30m, 2h, 1d",
                    since
                )
            })?;
            end - Duration::from_std(dur)?
        }
    };

    Ok((start, end))
}

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("invalid timestamp '{}': expected RFC3339 format", s))
}

/// Execute a query_range request and print results.
#[allow(clippy::too_many_arguments)]
async fn run_query_range(
    client: &LokiClient,
    query: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    limit: u32,
    direction: Direction,
    output: &OutputFormat,
    use_color: bool,
) -> Result<()> {
    let entries = client
        .query_range(query, start, end, limit, direction)
        .await?;

    // Loki returns entries in query direction order. For backward (default),
    // entries are newest-first. For display, reverse to chronological order.
    let entries = if direction == Direction::Backward {
        entries.into_iter().rev().collect::<Vec<_>>()
    } else {
        entries
    };

    for entry in entries {
        print_entry(&entry, output, use_color);
    }
    Ok(())
}

/// Tail logs via WebSocket and print entries as they arrive.
async fn run_tail(
    client: &LokiClient,
    query: &str,
    output: &OutputFormat,
    use_color: bool,
) -> Result<()> {
    client
        .tail(query, |entry| {
            print_entry(&entry, output, use_color);
        })
        .await
}

/// Print a single log entry in the selected format.
fn print_entry(entry: &LogEntry, output: &OutputFormat, use_color: bool) {
    match output {
        OutputFormat::Raw => {
            // <timestamp> <stream labels> <raw log line>
            let ts = entry.ts.to_rfc3339();
            let labels_str = format_labels(&entry.labels);
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

/// Format labels as `{key="val", ...}` for raw output.
fn format_labels(labels: &std::collections::BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return "{}".to_string();
    }
    let parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, v))
        .collect();
    format!("{{{}}}", parts.join(", "))
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
