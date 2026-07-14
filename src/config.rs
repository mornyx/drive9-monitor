use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Top-level config file mapping cluster keys to their signal configs.
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_cluster: Option<String>,
    #[serde(default)]
    pub clusters: BTreeMap<String, ClusterConfig>,
}

/// Per-cluster config containing optional logs, metrics, and alerts signal configs.
#[derive(Debug, Deserialize)]
pub struct ClusterConfig {
    #[serde(default)]
    pub logs: Option<SignalConfig>,
    #[serde(default)]
    pub metrics: Option<SignalConfig>,
    #[serde(default)]
    pub alerts: Option<SignalConfig>,
}

/// A single telemetry signal (logs or metrics) for a cluster.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SignalConfig {
    pub source_type: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    // TKE-specific fields (used by tke_cls and tke_prometheus source types).
    #[serde(default)]
    pub secret_id: Option<String>,
    #[serde(default)]
    pub secret_key: Option<String>,
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub topic_id: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
}

impl Config {
    /// Load config from the given path, or resolve from env/default.
    pub fn load(opt_path: Option<&str>) -> Result<Self> {
        let path = resolve_config_path(opt_path)?;
        if !path.exists() {
            bail!(
                "config file not found at {}\nCreate it with per-cluster [clusters.\"<key>\".logs] sections, then run `clusters` to verify.",
                path.display()
            );
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("failed to parse config file: {}", path.display()))?;
        Ok(config)
    }

    /// Look up a cluster by key, returning a helpful error if not found.
    pub fn cluster(&self, key: &str) -> Result<&ClusterConfig> {
        self.clusters
            .get(key)
            .with_context(|| {
                let valid: Vec<&str> = self.clusters.keys().map(|k| k.as_str()).collect();
                format!(
                    "unknown cluster '{}'\nvalid clusters: {}",
                    key,
                    valid.join(", ")
                )
            })
    }

    /// Resolve a cluster key: use the provided key, or fall back to `default_cluster` from config.
    pub fn resolve_cluster_key<'a>(&'a self, opt_key: Option<&'a str>) -> Result<String> {
        match opt_key {
            Some(k) => Ok(k.to_string()),
            None => self.default_cluster.clone().with_context(|| {
                "no --cluster specified and no default_cluster set in config"
            }),
        }
    }

    /// Set the default_cluster in the config file.
    pub fn set_default_cluster(opt_path: Option<&str>, key: &str) -> Result<()> {
        let path = resolve_config_path(opt_path)?;
        if !path.exists() {
            bail!(
                "config file not found at {}\nCreate it first, then run `set-cluster`.",
                path.display()
            );
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;

        // Validate the cluster key exists.
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("failed to parse config file: {}", path.display()))?;
        if !config.clusters.contains_key(key) {
            let valid: Vec<&str> = config.clusters.keys().map(|k| k.as_str()).collect();
            bail!(
                "cluster '{}' not found in config\nvalid clusters: {}",
                key,
                valid.join(", ")
            );
        }

        // Update or insert the default_cluster line.
        let new_contents = set_default_cluster_line(&contents, key);
        std::fs::write(&path, new_contents)
            .with_context(|| format!("failed to write config file: {}", path.display()))?;
        Ok(())
    }
}

impl ClusterConfig {
    /// Get the logs signal config, or error if not configured for this cluster.
    pub fn logs(&self) -> Result<&SignalConfig> {
        self.logs
            .as_ref()
            .with_context(|| "this cluster has no logs signal configured".to_string())
    }

    /// Get the metrics signal config, or error if not configured for this cluster.
    pub fn metrics(&self) -> Result<&SignalConfig> {
        self.metrics
            .as_ref()
            .with_context(|| "this cluster has no metrics signal configured".to_string())
    }
}

/// Resolve config path: --config flag > env var > default location.
fn resolve_config_path(opt_path: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = opt_path {
        return Ok(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("DRIVE9_MONITOR_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    // Spec: ~/.config/drive9-monitor/config.toml (XDG-style, not platform-specific).
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let mut path = home;
    path.push(".config");
    path.push("drive9-monitor");
    path.push("config.toml");
    Ok(path)
}

/// Update or insert the `default_cluster` line in the config TOML content.
fn set_default_cluster_line(contents: &str, key: &str) -> String {
    let new_line = format!("default_cluster = \"{}\"", key);
    // Try to replace an existing default_cluster line.
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("default_cluster") {
            return contents.replacen(line, &new_line, 1);
        }
    }
    // Insert at the top of the file.
    format!("{}\n{}", new_line, contents)
}