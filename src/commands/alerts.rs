use std::io::IsTerminal;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::alertmanager::{Alert, AmClient};
use crate::config::Config;

/// Arguments for the `alerts` subcommand.
pub struct AlertsArgs {
    pub cluster: Option<String>,
    pub query: Option<String>,
    pub state: String,
    pub output: String,
}

/// Entry point for the `alerts` subcommand.
pub async fn run(config: &Config, args: AlertsArgs) -> Result<()> {
    let cluster_key = config.resolve_cluster_key(args.cluster.as_deref())?;
    let cluster = config.cluster(&cluster_key)?;
    let alerts = cluster.alerts.as_ref().context("this cluster has no alerts signal configured")?;

    let client = AmClient::new(&alerts.endpoint)?;
    // Alertmanager v2 uses separate filter params per matcher (key="value").
    // Build filters from user query + config labels.
    let filters = build_alert_filters(&args.query, &alerts.labels);

    // Determine state filter booleans.
    let (active, silenced, inhibited) = match args.state.as_str() {
        "active" => (true, false, false),
        "silenced" => (false, true, false),
        "inhibited" => (false, false, true),
        "all" => (true, true, true),
        other => anyhow::bail!("invalid state '{}': expected active, silenced, inhibited, or all", other),
    };

    let alerts = client.alerts(&filters, active, silenced, inhibited).await?;

    match args.output.as_str() {
        "json" => print_json(&alerts),
        "text" => {
            let use_color = std::io::stdout().is_terminal();
            print_text(&alerts, use_color);
        }
        other => anyhow::bail!("invalid output format '{}': expected text or json", other),
    }

    Ok(())
}

/// Build Alertmanager v2 filter strings from user query + config labels.
///
/// Each filter is a `key="value"` string (no braces). Alertmanager expects
/// separate `filter` query params per matcher. Config labels are appended;
/// user-specified labels take precedence.
fn build_alert_filters(
    opt_query: &Option<String>,
    config_labels: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    // Parse user query if provided (strip braces if present, parse key="value" pairs).
    let mut user_labels: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    if let Some(q) = opt_query {
        let q = q.trim();
        if !q.is_empty() {
            // Strip surrounding braces if present.
            let inner = q.strip_prefix('{').and_then(|s| s.strip_suffix('}')).unwrap_or(q);
            for part in inner.split(',') {
                let part = part.trim();
                if let Some(eq_pos) = part.find('=') {
                    let key = part[..eq_pos].trim().to_string();
                    let val = part[eq_pos + 1..].trim()
                        .strip_prefix('"').and_then(|v| v.strip_suffix('"'))
                        .unwrap_or(&part[eq_pos + 1..].trim())
                        .to_string();
                    user_labels.insert(key, val);
                }
            }
        }
    }

    // Merge: config labels as base, user labels override.
    let mut merged = config_labels.clone();
    for (k, v) in &user_labels {
        merged.insert(k.clone(), v.clone());
    }

    merged.iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, v))
        .collect()
}

fn print_text(alerts: &[Alert], use_color: bool) {
    for alert in alerts {
        let severity = alert.labels.get("severity").map(|s| s.as_str()).unwrap_or("info");
        let alertname = alert.labels.get("alertname").map(|s| s.as_str()).unwrap_or("?");
        let local_start = alert.starts_at.with_timezone(&chrono::Local);
        let ts = local_start.format("%Y-%m-%d %H:%M:%S %:z").to_string();
        let local_end = alert.ends_at.with_timezone(&chrono::Local);
        let ends_at = local_end.format("%Y-%m-%d %H:%M:%S %:z").to_string();

        if use_color {
            println!("{} {} {} {} {{",
                ts.cyan(),
                colorize_severity(severity),
                alertname.bold(),
                alert.state.dimmed(),
            );
        } else {
            println!("{} {} {} {} {{", ts, severity, alertname, alert.state);
        }

        let kv = |k: &str, v: &str| {
            if use_color {
                format!("        {}={}", k.dimmed(), v)
            } else {
                format!("        {}={}", k, v)
            }
        };

        let field = |k: &str, v: &str| {
            if use_color {
                format!("    {}={}", k.dimmed(), v)
            } else {
                format!("    {}={}", k, v)
            }
        };

        println!("{},", field("startsAt", &ts));
        println!("{},", field("endsAt", &ends_at));
        println!("{},", field("fingerprint", &alert.fingerprint));

        println!("    labels: {{");
        for (k, v) in alert.labels.iter()
            .filter(|(k, _)| *k != "severity" && *k != "alertname")
        {
            println!("{},", kv(k, v));
        }
        println!("    }},");

        println!("    annotations: {{");
        for (k, v) in &alert.annotations {
            println!("{},", kv(k, v));
        }
        println!("    }},");
        println!("}}");
    }
}

fn print_json(alerts: &[Alert]) {
    let result: Vec<serde_json::Value> = alerts.iter().map(|a| {
        serde_json::json!({
            "labels": a.labels,
            "annotations": a.annotations,
            "startsAt": a.starts_at.to_rfc3339(),
            "endsAt": a.ends_at.to_rfc3339(),
            "status": { "state": a.state },
            "fingerprint": a.fingerprint,
        })
    }).collect();
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

fn colorize_severity(severity: &str) -> colored::ColoredString {
    match severity {
        "critical" | "error" | "major" => severity.red(),
        "warning" | "warn" => severity.yellow(),
        "info" => severity.green(),
        _ => severity.normal(),
    }
}