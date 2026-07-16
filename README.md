# drive9-monitor

A CLI for querying drive9-server monitoring data (logs, metrics, and alerts) across all deployment clusters. Designed for both human operators and AI agents.

## Installation

```sh
cargo install drive9-monitor
```

## Prerequisites

- **Feilian/VPN**: o11y endpoints (Loki, VictoriaMetrics, Alertmanager) are only reachable via Feilian/VPN.
- **gh CLI**: required for `rules` command (private repo access to `tidbcloud/runbooks`).
- **Jira Cloud API token**: required for `jira-alerts` command. Create one at https://id.atlassian.com/manage-profile/security/api-tokens.

## Quick Start

```sh
# List configured clusters
drive9-monitor clusters

# Set a default cluster so you don't need -c every time
drive9-monitor clusters use prod

# Query logs (default: last 1h, 100 lines, text format)
drive9-monitor logs
drive9-monitor logs --since 30m -n 50
drive9-monitor logs '| json | level="error"'

# Query metrics (default: TUI chart, auto-refresh every 10s)
drive9-monitor metrics 'drive9_service_gauge{name="cached_backends"}'
drive9-monitor metrics -o table 'rate(drive9_http_requests_total[5m])' --step 1m

# Query active alerts
drive9-monitor alerts
drive9-monitor alerts '{severity="critical"}'
drive9-monitor alerts --state all

# Query Jira alert tickets (global, not per-cluster)
drive9-monitor jira-alerts
drive9-monitor jira-alerts -n 10
drive9-monitor jira-alerts 'statusCategory != "Done"'

# View alert rule definitions
drive9-monitor rules
drive9-monitor rules Drive9ServiceOperationErrorRateHigh
```

## Configuration

Config file lives at `~/.config/drive9-monitor/config.toml` (overridable via `--config` flag or `DRIVE9_MONITOR_CONFIG` env var):

```toml
default_cluster = "prod"

[jira]
email = "you@pingcap.com"
token = "ATATT3xFfGF0..."
labels = {component = "drive9"}

[clusters."prod"]
logs.source_type = "loki"
logs.endpoint = "<loki base url>"
logs.labels = {cluster_env = "prod", cluster = "...", app = "..."}
metrics.source_type = "prometheus"
metrics.endpoint = "<victoriametrics base url>"
metrics.labels = {container = "..."}
alerts.source_type = "alertmanager"
alerts.endpoint = "<alertmanager base url>"
alerts.labels = {component = "..."}

# TKE (Tencent Cloud) clusters use different source types:
# [clusters."tencentcloud-ap-beijing"]
# logs.source_type = "tke_cls"
# logs.secret_id = "..."
# logs.secret_key = "..."
# logs.topic_id = "..."
# logs.region = "ap-beijing"
# metrics.source_type = "tke_prometheus"
# metrics.secret_id = "..."
# metrics.secret_key = "..."
# metrics.instance_id = "..."
# metrics.region = "ap-beijing"
```

### Source Types

| Signal  | Source Types                    |
|---------|-------------------------------|
| logs    | `loki`, `tke_cls`              |
| metrics | `prometheus`, `tke_prometheus` |
| alerts  | `alertmanager`                 |

TKE (Tencent Cloud) clusters use `tke_cls` / `tke_prometheus` which require `secret_id`, `secret_key`, and Tencent Cloud-specific fields (`topic_id`, `instance_id`, `region`).

### Config Labels

Each cluster has default label filters that are auto-merged into every query. User-specified labels in the query override config labels. This means you don't need to repeat `component=drive9` or `namespace=drive9-tidbcloud` in every query.

## Commands

### `clusters`

List all configured clusters. The default cluster is marked with `*`.

```
drive9-monitor clusters
```

### `clusters use <key>`

Set the default cluster.

```
drive9-monitor clusters use prod
```

### `logs`

Query logs from a cluster.

```
drive9-monitor logs [--cluster <key>] [--since <dur>] [--from <ts>] [--to <ts>]
                   [--limit <n>] [--direction forward|backward] [--follow] [--output text|raw|json]
                   [<query>]
```

- `--cluster` / `-c`: cluster key (defaults to `default_cluster`)
- `--since` / `-s`: lookback duration (default `1h`; e.g. `30m`, `2h`, `1d`)
- `--from` / `--to`: explicit start/end time (RFC3339)
- `--limit` / `-n`: max lines (default `100`)
- `--follow` / `-f`: tail live logs (Loki only; not supported for TKE CLS)
- `--output` / `-o`: `text` (default), `raw`, `json`

For `loki`, `<query>` is LogQL (e.g. `{app="foo"} |= "error" | json | level="error"`). For `tke_cls`, it's CLS query syntax (e.g. `level:error`).

#### Output Formats

- **text** (default): `TIME LEVEL MESSAGE k1=v1 k2=v2 ...` — parsed from JSON log line, human-friendly timestamp with timezone, all fields as key=value pairs in alphabetical order. Colorized when stdout is a TTY.
- **raw**: `<timestamp> <stream labels> <raw log line>` — the full log line as returned by Loki/CLS. Colorized when stdout is a TTY.
- **json**: the raw JSON log line, one per line. No transformation — safe for pipe consumption.

### `metrics`

Query metrics from a cluster.

```
drive9-monitor metrics [--cluster <key>] [--since <dur>] [--from <ts>] [--to <ts>]
                      [--step <dur>] [--refresh <dur>] [--output tui|table|json]
                      <query>
```

- `--cluster` / `-c`: cluster key (defaults to `default_cluster`)
- `--since` / `-s`: lookback duration (default `1h`)
- `--step`: query resolution step (default `30s`)
- `--refresh`: TUI auto-refresh interval (default `10s`)
- `--output` / `-o`: `tui` (default), `table`, `json`

`<query>` is a MetricsQL / PromQL expression. Config labels are auto-merged as filters.

#### Output Formats

- **tui** (default): interactive terminal UI rendering a time-series line chart. Auto-refreshes every `--refresh` interval. Each series is a separate line with a legend showing metric name and labels. `q`/`Ctrl-C` exits, `r` forces refresh.
- **table**: human-readable table — one row per time step, columns for each series.
- **json**: Prometheus-style matrix JSON reconstructed from the API response (`{"status":"success","data":{"resultType":"matrix","result":[...]}}`).

### `alerts`

Query active alerts from a cluster.

```
drive9-monitor alerts [--cluster <key>] [--state active|silenced|inhibited|all]
                     [--output text|json] [<query>]
```

- `--cluster` / `-c`: cluster key (defaults to `default_cluster`)
- `--state`: alert state filter (default `active`)
- `--output` / `-o`: `text` (default), `json`
- `<query>`: optional label matcher (e.g. `{severity="critical"}`)

#### Output Formats

- **text** (default): full block format per alert, showing all fields:
  ```
  TIME SEVERITY NAME STATE {
      startsAt=...,
      endsAt=...,
      fingerprint=...,
      labels: {
          k1=v1,
          k2=v2,
      },
      annotations: {
          k1=v1,
          k2=v2,
      },
  }
  ```
  Includes `startsAt`, `endsAt`, `fingerprint`, all `labels` (alphabetical, excluding `severity`/`alertname` in header), and all `annotations` (alphabetical). Colorized when stdout is a TTY.

- **json**: JSON array of alert objects reconstructed from the Alertmanager API v2 response (`labels`, `annotations`, `startsAt`, `endsAt`, `status.state`, `fingerprint`).

Note: Alertmanager only returns currently active alerts. Resolved alerts disappear automatically. Use `--state all` to include silenced/inhibited alerts.

### `rules`

Show alert rule definitions fetched from the `tidbcloud/runbooks` private repo via `gh` CLI.

```
drive9-monitor rules [name]
```

- No argument: list all rule names with severity and summary.
- With `name`: show full rule definition — `expr` (PromQL), `for` (duration), `labels`, `annotations`.

Requires `gh` CLI installed and authenticated with access to `tidbcloud/runbooks`.

### `jira-alerts`

Query alert tickets from Jira. Unlike `alerts` (per-cluster Alertmanager), Jira is a global signal — all clusters' tickets live in the same O11Y project.

```
drive9-monitor jira-alerts [--limit <n>] [--output text|json] [<query>]
```

- `--limit` / `-n`: max number of tickets (default `5`; `0` = all)
- `--output` / `-o`: `text` (default), `json`
- `<query>`: optional JQL fragment (e.g. `statusCategory != "Done"`)

Config labels (`jira.labels`) are always applied as base JQL conditions (AND-joined as `key = "value"`). The user query is AND-ed with the base JQL. `ORDER BY created DESC` is appended automatically. At least one condition is required (Jira rejects unrestricted queries).

#### Output Formats

- **text** (default): full block format per ticket:
  ```
  TIME PRIORITY KEY STATUS SUMMARY {
      created=...,
      updated=...,
      project=...,
      components=[...],
      labels=[...],
  }
  ```
  `TIME` is the `created` timestamp with timezone. Colorized when stdout is a TTY (priority colored: blocker/重要=red, others=yellow).
- **json**: structured JSON array with normalized fields per ticket (`key`, `summary`, `status`, `statusCategory`, `priority`, `created`, `updated`, `project`, `components`, `description`) — safe for pipe consumption.

#### Jira Config

Top-level config fields (not per-cluster), under a `[jira]` table:

| Field           | Required | Description                                      |
|-----------------|----------|--------------------------------------------------|
| `jira.endpoint` | yes      | Jira base URL (e.g. `https://tidb.atlassian.net`) |
| `jira.email`    | yes      | Atlassian account email for Basic Auth           |
| `jira.token`    | yes      | Jira Cloud API token                             |
| `jira.labels`   | no       | Default JQL conditions (e.g. `{component = "drive9"}`) |

## Alert Investigation Workflow

When an alert is received, follow these steps to investigate:

### 1. Identify the cluster

```sh
drive9-monitor clusters
```

The default cluster is marked with `*`. Switch if needed:

```sh
drive9-monitor clusters use <cluster-key>
```

### 2. Query the active alert

```sh
drive9-monitor alerts
```

Filter by severity if needed:

```sh
drive9-monitor alerts '{severity="critical"}'
```

Identify the alert by its `alertname`. Note the `startsAt` (when it began), `value` annotation (current trigger value), and any relevant labels.

### 3. Check Jira for historical alert tickets

Alertmanager only returns **currently active** alerts. Once an alert self-resolves, it disappears from `alerts`. To find historical alert tickets (including already-resolved ones), query Jira:

```sh
# All drive9 alert tickets (uses jira.labels from config)
drive9-monitor jira-alerts

# Filter by priority
drive9-monitor jira-alerts priority=critical

# Filter by status (e.g. only unresolved)
drive9-monitor jira-alerts 'statusCategory != "Done"'

# Show all tickets
drive9-monitor jira-alerts -n 0
```

Jira tickets are created automatically by the alerting system. Each ticket's `summary` contains the alert name (e.g. `[PROD]Drive9CriticalHTTPP99Latency`), so you can correlate a Jira ticket with an active Alertmanager alert by matching the alert name.

### 4. Look up the alert rule definition

```sh
drive9-monitor rules <alertname>
```

This shows the PromQL `expr` (the condition and threshold that triggers the alert), `for` duration (how long the condition must persist), and `annotations` (description template). Compare the rule's threshold with the alert's `value` to understand why it triggered.

### 5. Investigate with metrics

Use the alert's `expr` as a starting point to query the relevant metrics trend:

```sh
# Table format for quick scan
drive9-monitor metrics -o table '<relevant PromQL>' --since 1h --step 1m

# JSON for detailed analysis
drive9-monitor metrics -o json '<relevant PromQL>' --since 1h --step 1m
```

### 6. Check logs for related errors

Look for error-level logs around the alert's `startsAt` time:

```sh
# Error logs (LogQL for Loki clusters)
drive9-monitor logs '| json | level="error"' --since 1h -n 50

# JSON output for detailed analysis
drive9-monitor logs -o json '| json | level="error"' --since 30m -n 100

# TKE clusters use CLS syntax instead
drive9-monitor logs 'level:error' --since 1h -n 50
```

### 7. Summarize findings

Combine the information:
1. **What**: alert name and summary (from `alerts` or `jira-alerts`)
2. **Why**: rule expression and threshold (from `rules`), compared with current `value`
3. **When**: `startsAt` timestamp (from `alerts`) or `created` timestamp (from `jira-alerts`)
4. **Trend**: metrics data showing whether the issue is improving or worsening
5. **Context**: error logs that may explain the root cause
6. **History**: Jira tickets showing whether this alert has occurred before (from `jira-alerts`)

## Tips

- Use `-o json` when you need to parse output programmatically.
- `--since` accepts Go-style durations: `30m`, `2h`, `1d`, `1h30m`.
- Config labels are auto-merged — you don't need to repeat them in your query.
- For TKE clusters, logs use CLS syntax (`level:error`) not LogQL (`| json | level="error"`).
- Alerts only show currently active alerts — resolved alerts disappear from Alertmanager.
