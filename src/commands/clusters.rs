use crate::config::Config;

/// Print a table of all configured clusters and their signal availability.
/// The current default_cluster is marked with `*`.
pub fn run(config: &Config) {
    if config.clusters.is_empty() {
        println!("no clusters configured");
        return;
    }

    let default = config.default_cluster.as_deref();

    // Simple aligned table.
    println!(
        "{:<3} {:<28} {:<6} {:<8} {:<7}",
        "", "CLUSTER", "LOGS", "METRICS", "ALERTS"
    );
    println!("{}", "-".repeat(58));
    for (key, cluster) in &config.clusters {
        let marker = if Some(key.as_str()) == default {
            "*"
        } else {
            " "
        };
        let logs = if cluster.logs.is_some() { "yes" } else { "no" };
        let metrics = if cluster.metrics.is_some() {
            "yes"
        } else {
            "no"
        };
        let alerts = if cluster.alerts.is_some() {
            "yes"
        } else {
            "no"
        };
        println!(
            "{:<3} {:<28} {:<6} {:<8} {:<7}",
            marker, key, logs, metrics, alerts
        );
    }
}
