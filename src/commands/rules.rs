use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use colored::Colorize;
use serde::Deserialize;

use crate::commands::common;

/// Prometheus alerting rules YAML structure.
#[derive(Deserialize)]
struct RulesFile {
    groups: Vec<RuleGroup>,
}

#[derive(Deserialize)]
struct RuleGroup {
    #[allow(dead_code)]
    name: String,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    alert: String,
    #[serde(default)]
    expr: String,
    #[serde(default)]
    for_duration: Option<String>,
    #[serde(default, rename = "for")]
    for_alias: Option<String>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

impl Rule {
    fn for_str(&self) -> String {
        self.for_duration
            .as_deref()
            .or(self.for_alias.as_deref())
            .unwrap_or("0s")
            .to_string()
    }
}

/// Entry point for the `rules` subcommand.
pub async fn run(name: Option<&str>) -> Result<()> {
    let yaml = fetch_rules().await?;
    let rules_file: RulesFile =
        serde_yaml_neo::from_str(&yaml).context("failed to parse rules YAML")?;

    let all_rules: Vec<&Rule> = rules_file
        .groups
        .iter()
        .flat_map(|g| g.rules.iter())
        .collect();

    match name {
        Some(n) => {
            let rule = all_rules
                .iter()
                .find(|r| r.alert == n)
                .with_context(|| format!("alert rule '{}' not found", n))?;
            print_rule_detail(rule);
        }
        None => {
            for rule in &all_rules {
                print_rule_list(rule);
            }
        }
    }

    Ok(())
}

/// Fetch the rules YAML from the private GitHub repo via gh CLI.
async fn fetch_rules() -> Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new("gh")
            .args([
                "api",
                "repos/tidbcloud/runbooks/contents/rules/mem9/mnemos/drive9-alerts.yaml",
                "--jq",
                ".content",
            ])
            .output(),
    )
    .await
    .context("timed out running gh CLI")?
    .context("failed to run gh CLI — is it installed?")?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.contains("not found") || err.contains("401") || err.contains("403") {
            bail!(
                "failed to fetch rules from GitHub: {}\n\
                 Make sure gh CLI is authenticated with access to tidbcloud/runbooks repo.",
                err.trim()
            );
        }
        bail!("gh CLI error: {}", err.trim());
    }

    let base64_content = String::from_utf8(output.stdout).context("invalid output from gh CLI")?;
    let base64_content = base64_content.trim().replace('\n', "");

    // Decode base64.
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(base64_content)
        .context("failed to decode base64 content")?;

    String::from_utf8(decoded).context("rules file is not valid UTF-8")
}

fn print_rule_list(rule: &Rule) {
    let severity = rule
        .labels
        .get("severity")
        .map(|s| s.as_str())
        .unwrap_or("info");
    let summary = rule
        .annotations
        .get("summary")
        .map(|s| s.as_str())
        .unwrap_or("");
    if common::use_color() {
        println!(
            "{} {} — {}",
            common::colorize_severity(severity),
            rule.alert.bold(),
            summary
        );
    } else {
        println!("{} {} — {}", severity, rule.alert, summary);
    }
}

fn print_rule_detail(rule: &Rule) {
    let severity = rule
        .labels
        .get("severity")
        .map(|s| s.as_str())
        .unwrap_or("info");
    let use_color = common::use_color();

    if use_color {
        println!(
            "{} ({}, {})",
            rule.alert.bold(),
            common::colorize_severity(severity),
            rule.for_str()
        );
    } else {
        println!("{} ({}, {})", rule.alert, severity, rule.for_str());
    }

    println!("expr: |");
    for line in rule.expr.lines() {
        println!("    {}", line);
    }

    println!("labels:");
    for (k, v) in &rule.labels {
        if use_color {
            println!("    {}={}", k.dimmed(), v);
        } else {
            println!("    {}={}", k, v);
        }
    }

    println!("annotations:");
    for (k, v) in &rule.annotations {
        if use_color {
            println!("    {}={}", k.dimmed(), v);
        } else {
            println!("    {}={}", k, v);
        }
    }
}
