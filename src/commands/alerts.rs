use anyhow::Result;
use colored::Colorize;

use crate::alertmanager::{Alert, AmClient};
use crate::commands::common;
use crate::config::Config;
use crate::labels::{self, LabelMap, Matcher};

/// Alert state filter for the `alerts` subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AlertState {
    Active,
    Silenced,
    Inhibited,
    All,
}

impl AlertState {
    /// Map to Alertmanager's `(active, silenced, inhibited)` boolean params.
    fn as_bools(self) -> (bool, bool, bool) {
        match self {
            AlertState::Active => (true, false, false),
            AlertState::Silenced => (false, true, false),
            AlertState::Inhibited => (false, false, true),
            AlertState::All => (true, true, true),
        }
    }
}

/// Output format for alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Arguments for the `alerts` subcommand.
pub struct AlertsArgs {
    pub cluster: Option<String>,
    pub query: Option<String>,
    pub state: AlertState,
    pub output: OutputFormat,
}

/// Entry point for the `alerts` subcommand.
pub async fn run(config: &Config, args: AlertsArgs) -> Result<()> {
    let cluster_key = config.resolve_cluster_key(args.cluster.as_deref())?;
    let cluster = config.cluster(&cluster_key)?;
    let alerts = cluster.alerts()?;

    let client = AmClient::new(&alerts.endpoint)?;
    // Alertmanager v2 uses separate filter params per matcher.
    let filters = build_alert_filters(&args.query, &alerts.labels);
    let (active, silenced, inhibited) = args.state.as_bools();

    let alerts = client.alerts(&filters, active, silenced, inhibited).await?;

    match args.output {
        OutputFormat::Json => print_json(&alerts),
        OutputFormat::Text => print_text(&alerts, common::use_color()),
    }

    Ok(())
}

/// Build Alertmanager v2 filter strings from user query + config labels.
///
/// Each filter is one matcher (`key="value"`, `key=~"regex"`, ...; no braces),
/// sent as a separate `filter` query param. Config labels are appended;
/// user-specified matchers take precedence and their operators are preserved.
fn build_alert_filters(opt_query: &Option<String>, config_labels: &LabelMap) -> Vec<String> {
    let mut merged: std::collections::BTreeMap<String, Matcher> = config_labels
        .iter()
        .map(|(k, v)| (k.clone(), Matcher::eq(k, v)))
        .collect();

    if let Some(q) = opt_query {
        let q = q.trim();
        if !q.is_empty() {
            // Strip surrounding braces if present.
            let inner = q
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .unwrap_or(q);
            for m in labels::parse_matchers(inner) {
                merged.insert(m.key.clone(), m);
            }
        }
    }

    merged.values().map(|m| m.to_string()).collect()
}

fn print_text(alerts: &[Alert], use_color: bool) {
    for alert in alerts {
        let severity = alert
            .labels
            .get("severity")
            .map(|s| s.as_str())
            .unwrap_or("info");
        let alertname = alert
            .labels
            .get("alertname")
            .map(|s| s.as_str())
            .unwrap_or("?");
        let local_start = alert.starts_at.with_timezone(&chrono::Local);
        let ts = local_start.format("%Y-%m-%d %H:%M:%S %:z").to_string();
        let local_end = alert.ends_at.with_timezone(&chrono::Local);
        let ends_at = local_end.format("%Y-%m-%d %H:%M:%S %:z").to_string();

        if use_color {
            println!(
                "{} {} {} {} {{",
                ts.cyan(),
                common::colorize_severity(severity),
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
        for (k, v) in alert
            .labels
            .iter()
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
    let result: Vec<serde_json::Value> = alerts
        .iter()
        .map(|a| {
            serde_json::json!({
                "labels": a.labels,
                "annotations": a.annotations,
                "startsAt": a.starts_at.to_rfc3339(),
                "endsAt": a.ends_at.to_rfc3339(),
                "status": { "state": a.state },
                "fingerprint": a.fingerprint,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
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
    fn filters_merge_config_and_query() {
        let filters = build_alert_filters(
            &Some(r#"{severity="critical"}"#.to_string()),
            &labels(&[("component", "drive9")]),
        );
        assert_eq!(
            filters,
            vec![r#"component="drive9""#, r#"severity="critical""#]
        );
    }

    #[test]
    fn filters_work_without_braces() {
        let filters = build_alert_filters(
            &Some(r#"severity="critical""#.to_string()),
            &LabelMap::new(),
        );
        assert_eq!(filters, vec![r#"severity="critical""#]);
    }

    #[test]
    fn regex_matchers_are_preserved() {
        let filters = build_alert_filters(
            &Some(r#"{pod=~"a|b,c"}"#.to_string()),
            &labels(&[("component", "drive9")]),
        );
        assert_eq!(filters, vec![r#"component="drive9""#, r#"pod=~"a|b,c""#]);
    }

    #[test]
    fn user_matchers_override_config() {
        let filters = build_alert_filters(
            &Some(r#"{component="custom"}"#.to_string()),
            &labels(&[("component", "drive9"), ("team", "o11y")]),
        );
        assert_eq!(filters, vec![r#"component="custom""#, r#"team="o11y""#]);
    }

    #[test]
    fn empty_query_uses_config_only() {
        let filters = build_alert_filters(&None, &labels(&[("component", "drive9")]));
        assert_eq!(filters, vec![r#"component="drive9""#]);
    }
}
