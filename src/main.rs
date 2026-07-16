mod alertmanager;
mod commands;
mod config;
mod error;
mod grafana;
mod http;
mod jira;
mod labels;
mod loki;
mod prom;
mod tencentcloud;
mod tke_cls;
mod tke_prometheus;
mod victoriametrics;

use std::process::ExitCode;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};

use crate::commands::alerts::{self, AlertState, AlertsArgs};
use crate::commands::clusters;
use crate::commands::jira_alerts::{self, JiraAlertsArgs};
use crate::commands::logs::{self, LogsArgs};
use crate::commands::metrics::{self, MetricsArgs};
use crate::commands::rules;
use crate::config::Config;
use crate::loki::Direction;

/// CLI for querying drive9-server monitoring data (logs, metrics, alerts) across clusters.
#[derive(Parser)]
#[command(name = "drive9-monitor", version, about)]
struct Cli {
    /// Path to config file (default: ~/.config/drive9-monitor/config.toml).
    #[arg(long, global = true)]
    config: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum ClustersSub {
    /// Set the default cluster in the config file.
    Use {
        /// Cluster key to set as default.
        key: String,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Query alerts from a cluster.
    Alerts {
        /// Cluster key from config, or default_cluster from config if omitted.
        #[arg(short, long)]
        cluster: Option<String>,

        /// Alert state filter.
        #[arg(long, default_value = "active")]
        state: AlertState,

        /// Output format.
        #[arg(short, long, default_value = "text")]
        output: alerts::OutputFormat,

        /// Alertmanager label matcher expression (e.g. {severity="critical"}).
        query: Option<String>,
    },
    /// List all configured clusters, or set the default cluster.
    Clusters {
        #[command(subcommand)]
        subcommand: Option<ClustersSub>,
    },
    /// Query alert tickets from Jira (global, not per-cluster).
    JiraAlerts {
        /// Max number of tickets to return (0 = all).
        #[arg(short = 'n', long, default_value = "5")]
        limit: usize,

        /// Output format.
        #[arg(short, long, default_value = "text")]
        output: jira_alerts::OutputFormat,

        /// Optional JQL expression fragment (e.g. `statusCategory != "Done"`).
        query: Option<String>,
    },
    /// Query logs from a cluster.
    Logs {
        /// Cluster key from config, or default_cluster from config if omitted.
        #[arg(short, long)]
        cluster: Option<String>,

        /// Lookback duration from --to (e.g. 30m, 2h, 1d).
        #[arg(short, long, default_value = "1h", value_parser = humantime::parse_duration)]
        since: Duration,

        /// Start time (RFC3339).
        #[arg(long, value_parser = commands::common::parse_rfc3339)]
        from: Option<DateTime<Utc>>,

        /// End time (RFC3339, default: now).
        #[arg(long, value_parser = commands::common::parse_rfc3339)]
        to: Option<DateTime<Utc>>,

        /// Max number of log lines to return.
        #[arg(short = 'n', long, default_value = "100")]
        limit: u32,

        /// Query direction.
        #[arg(long, default_value = "backward")]
        direction: Direction,

        /// Tail new log entries (stream until Ctrl-C).
        #[arg(short, long)]
        follow: bool,

        /// Output format.
        #[arg(short, long, default_value = "text")]
        output: logs::OutputFormat,

        /// LogQL log query. If omitted, uses default labels from config.
        query: Option<String>,
    },
    /// Query metrics from a cluster.
    Metrics {
        /// Cluster key from config, or default_cluster from config if omitted.
        #[arg(short, long)]
        cluster: Option<String>,

        /// Lookback duration from --to (e.g. 30m, 2h, 1d).
        #[arg(short, long, default_value = "1h", value_parser = humantime::parse_duration)]
        since: Duration,

        /// Start time (RFC3339).
        #[arg(long, value_parser = commands::common::parse_rfc3339)]
        from: Option<DateTime<Utc>>,

        /// End time (RFC3339, default: now).
        #[arg(long, value_parser = commands::common::parse_rfc3339)]
        to: Option<DateTime<Utc>>,

        /// Query resolution step (e.g. 15s, 1m, 5m).
        #[arg(long, default_value = "30s", value_parser = humantime::parse_duration)]
        step: Duration,

        /// Auto-refresh interval for TUI (e.g. 10s, 30s).
        #[arg(long, default_value = "10s", value_parser = humantime::parse_duration)]
        refresh: Duration,

        /// Output format.
        #[arg(short, long, default_value = "tui")]
        output: metrics::OutputFormat,

        /// MetricsQL / PromQL query expression.
        query: String,
    },
    /// Show alert rule definitions from the runbooks repo.
    Rules {
        /// Optional alert name to show full definition. If omitted, lists all rules.
        name: Option<String>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", error::render(&e));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Alerts {
            cluster,
            state,
            output,
            query,
        } => {
            let config = Config::load(cli.config.as_deref())?;
            let args = AlertsArgs {
                cluster,
                query,
                state,
                output,
            };
            alerts::run(&config, args).await
        }
        Command::Clusters { subcommand } => {
            let config = Config::load(cli.config.as_deref())?;
            match subcommand {
                Some(ClustersSub::Use { key }) => {
                    Config::set_default_cluster(cli.config.as_deref(), &key)?;
                    println!("default_cluster set to '{}'", key);
                }
                None => {
                    clusters::run(&config);
                }
            }
            Ok(())
        }
        Command::JiraAlerts {
            limit,
            output,
            query,
        } => {
            let config = Config::load(cli.config.as_deref())?;
            let args = JiraAlertsArgs {
                limit,
                output,
                query,
            };
            jira_alerts::run(&config, args).await
        }
        Command::Logs {
            cluster,
            since,
            from,
            to,
            limit,
            direction,
            follow,
            output,
            query,
        } => {
            let config = Config::load(cli.config.as_deref())?;
            let args = LogsArgs {
                cluster,
                query,
                since,
                from,
                to,
                limit,
                direction,
                follow,
                output,
            };
            logs::run(&config, args).await
        }
        Command::Metrics {
            cluster,
            since,
            from,
            to,
            step,
            refresh,
            output,
            query,
        } => {
            let config = Config::load(cli.config.as_deref())?;
            let args = MetricsArgs {
                cluster,
                query,
                since,
                from,
                to,
                step,
                refresh,
                output,
            };
            metrics::run(&config, args).await
        }
        Command::Rules { name } => rules::run(name.as_deref()).await,
    }
}
