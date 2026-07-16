//! Helpers shared across subcommands.

use std::io::IsTerminal;
use std::time::Duration;

use chrono::{DateTime, Utc};
use colored::{ColoredString, Colorize};

/// clap value parser for `--from`/`--to` (RFC3339 timestamps).
pub fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc))
}

/// Resolve the `(start, end)` time range from --since/--from/--to.
/// `--from`/`--to` take precedence; `--since` looks back from `--to`
/// (default: now).
pub fn resolve_time_range(
    since: Duration,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let end = to.unwrap_or_else(Utc::now);
    let start = from.unwrap_or_else(|| {
        end - chrono::Duration::from_std(since).unwrap_or_else(|_| chrono::Duration::zero())
    });
    (start, end)
}

/// Whether to colorize stdout output.
pub fn use_color() -> bool {
    std::io::stdout().is_terminal()
}

/// Colorize a severity string (shared by alerts and rules output).
pub fn colorize_severity(severity: &str) -> ColoredString {
    match severity {
        "critical" | "error" | "major" => severity.red(),
        "warning" | "warn" => severity.yellow(),
        "info" => severity.green(),
        _ => severity.normal(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_to_take_precedence() {
        let from = parse_rfc3339("2026-01-01T00:00:00Z").unwrap();
        let to = parse_rfc3339("2026-01-02T00:00:00Z").unwrap();
        let (start, end) = resolve_time_range(Duration::from_secs(3600), Some(from), Some(to));
        assert_eq!(start, from);
        assert_eq!(end, to);
    }

    #[test]
    fn since_looks_back_from_to() {
        let to = parse_rfc3339("2026-01-02T00:00:00Z").unwrap();
        let (start, end) = resolve_time_range(Duration::from_secs(3600), None, Some(to));
        assert_eq!((end - start).num_seconds(), 3600);
    }
}
