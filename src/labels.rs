//! Label matcher parsing and LogQL/PromQL selector merging.
//!
//! Shared by the logs (LogQL), metrics (MetricsQL/PromQL), and alerts
//! (Alertmanager matcher) commands.

use std::collections::BTreeMap;
use std::fmt;

/// A map of label key -> value.
pub type LabelMap = BTreeMap<String, String>;

/// Label matcher operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchOp {
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `=~`
    Re,
    /// `!~`
    Nr,
}

impl MatchOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            MatchOp::Eq => "=",
            MatchOp::Ne => "!=",
            MatchOp::Re => "=~",
            MatchOp::Nr => "!~",
        }
    }
}

/// A single label matcher: `key <op> "value"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matcher {
    pub key: String,
    pub op: MatchOp,
    pub value: String,
}

impl Matcher {
    /// An equality matcher from a config label pair.
    pub fn eq(key: &str, value: &str) -> Self {
        Self {
            key: key.to_string(),
            op: MatchOp::Eq,
            value: value.to_string(),
        }
    }
}

impl fmt::Display for Matcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}\"{}\"", self.key, self.op.as_str(), self.value)
    }
}

/// Parse the inside of a `{...}` selector (or a bare matcher list) into
/// matchers. Commas inside quoted values are handled correctly, and the
/// operators `=`, `!=`, `=~`, `!~` are preserved.
pub fn parse_matchers(s: &str) -> Vec<Matcher> {
    let mut out = Vec::new();
    for part in split_matchers(s) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // Find the operator. Two-char operators are checked first so that
        // e.g. `=~` is not mistaken for `=`; ties at the same position are
        // resolved by declaration order.
        let found = ["!~", "!=", "=~", "="]
            .iter()
            .filter_map(|cand| part.find(cand).map(|p| (p, *cand)))
            .min_by_key(|(p, _)| *p);
        let Some((pos, op_str)) = found else {
            continue;
        };
        let op = match op_str {
            "!=" => MatchOp::Ne,
            "=~" => MatchOp::Re,
            "!~" => MatchOp::Nr,
            _ => MatchOp::Eq,
        };
        let key = part[..pos].trim().to_string();
        let raw_val = part[pos + op_str.len()..].trim();
        let value = raw_val
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(raw_val)
            .to_string();
        if key.is_empty() {
            continue;
        }
        out.push(Matcher { key, op, value });
    }
    out
}

/// Split a matcher list on commas that are outside double quotes.
fn split_matchers(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_str => {
                cur.push(c);
                escaped = true;
            }
            '"' => {
                cur.push(c);
                in_str = !in_str;
            }
            ',' if !in_str => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

/// Build a `{key="val", ...}` selector from a label map (sorted by key).
pub fn build_selector(labels: &LabelMap) -> String {
    if labels.is_empty() {
        return "{}".to_string();
    }
    let parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| Matcher::eq(k, v).to_string())
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Merge config labels into a user-provided LogQL/PromQL query.
///
/// The first `{...}` selector in the query is parsed for existing matchers;
/// config labels are added for any key not already present — user-specified
/// matchers take precedence and their operators are preserved. If the query
/// has no selector, one is added (prepended before a leading pipeline).
pub fn merge_labels_into_query(query: &str, config_labels: &LabelMap) -> String {
    if config_labels.is_empty() {
        return query.to_string();
    }

    // Find the first `{` and its matching `}` (respecting quoted strings).
    let bytes = query.as_bytes();
    if let Some(s) = bytes.iter().position(|&b| b == b'{') {
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
            } else if c == b'"' {
                in_str = true;
            } else if c == b'}' {
                let merged = merge_selector(&query[s + 1..j], config_labels);
                return format!("{}{}{}", &query[..s], merged, &query[j + 1..]);
            }
            j += 1;
        }
        // Unbalanced brace — leave the query as-is.
        return query.to_string();
    }

    // No stream selector found — append one after the metric name, or
    // prepend before a leading pipeline (`|= ...`).
    let selector = build_selector(config_labels);
    if query.starts_with('|') {
        format!("{} {}", selector, query)
    } else {
        format!("{}{}", query, selector)
    }
}

/// Merge config labels into an existing selector body, returning `{...}`.
/// User matchers (by key) override config labels; the result is sorted by key.
pub fn merge_selector(selector_body: &str, config_labels: &LabelMap) -> String {
    let mut merged: BTreeMap<String, Matcher> = config_labels
        .iter()
        .map(|(k, v)| (k.clone(), Matcher::eq(k, v)))
        .collect();
    for m in parse_matchers(selector_body) {
        merged.insert(m.key.clone(), m);
    }
    let parts: Vec<String> = merged.values().map(|m| m.to_string()).collect();
    format!("{{{}}}", parts.join(", "))
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
    fn parse_matchers_basic() {
        let m = parse_matchers(r#"service="foo", app="bar""#);
        assert_eq!(
            m,
            vec![Matcher::eq("service", "foo"), Matcher::eq("app", "bar"),]
        );
    }

    #[test]
    fn parse_matchers_operators_and_quoted_commas() {
        let m = parse_matchers(r#"pod=~"a|b,c", severity!="critical", x!~"y""#);
        assert_eq!(
            m,
            vec![
                Matcher {
                    key: "pod".into(),
                    op: MatchOp::Re,
                    value: "a|b,c".into(),
                },
                Matcher {
                    key: "severity".into(),
                    op: MatchOp::Ne,
                    value: "critical".into(),
                },
                Matcher {
                    key: "x".into(),
                    op: MatchOp::Nr,
                    value: "y".into(),
                },
            ]
        );
    }

    #[test]
    fn build_selector_sorted_by_key() {
        assert_eq!(
            build_selector(&labels(&[("b", "2"), ("a", "1")])),
            r#"{a="1", b="2"}"#
        );
        assert_eq!(build_selector(&LabelMap::new()), "{}");
    }

    #[test]
    fn merge_into_bare_metric_name() {
        assert_eq!(
            merge_labels_into_query("drive9_metric", &labels(&[("container", "drive9-server")])),
            r#"drive9_metric{container="drive9-server"}"#
        );
    }

    #[test]
    fn merge_prepends_before_pipeline() {
        assert_eq!(
            merge_labels_into_query(r#"|= "error""#, &labels(&[("app", "x")])),
            r#"{app="x"} |= "error""#
        );
    }

    #[test]
    fn merge_into_existing_selector() {
        assert_eq!(
            merge_labels_into_query(r#"{app="x"} |= "y""#, &labels(&[("container", "c")])),
            r#"{app="x", container="c"} |= "y""#
        );
    }

    #[test]
    fn user_labels_override_config() {
        assert_eq!(
            merge_labels_into_query(r#"{app="x"}"#, &labels(&[("app", "cfg"), ("b", "2")])),
            r#"{app="x", b="2"}"#
        );
    }

    #[test]
    fn merge_preserves_regex_operator() {
        assert_eq!(
            merge_labels_into_query(r#"{pod=~"a|b"}"#, &labels(&[("c", "d")])),
            r#"{c="d", pod=~"a|b"}"#
        );
    }

    #[test]
    fn empty_config_leaves_query_untouched() {
        assert_eq!(
            merge_labels_into_query(r#"{app="x"} |= "y""#, &LabelMap::new()),
            r#"{app="x"} |= "y""#
        );
    }

    #[test]
    fn unbalanced_brace_leaves_query_untouched() {
        assert_eq!(
            merge_labels_into_query(r#"{app="x""#, &labels(&[("c", "d")])),
            r#"{app="x""#
        );
    }
}
