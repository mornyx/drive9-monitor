# drive9-monitor

A CLI tool for querying monitoring data (logs, metrics, and alerts) from [drive9-server](https://github.com/mem9-ai/drive9) across all deployment clusters. It is designed primarily for AI/agent consumption: queries are expressed as raw LogQL/MetricsQL/CLS query strings rather than fragmented flags, giving the caller full expressive power.

## Background

drive9-server is deployed to multiple clusters across regions and cloud providers. Most clusters use TiDB Cloud observability (Loki + VictoriaMetrics + Alertmanager). Tencent Cloud TKE clusters use native CLS (logs) and TMP Prometheus (metrics) instead, accessed via Tencent Cloud API v3 with AK/SK authentication. TKE clusters do not have Alertmanager; alerts are only available for clusters using TiDB Cloud observability.

### TiDB Cloud observability

The Loki endpoint URL pattern is:

```
https://www.ds.<region>.<provider>.observability.tidbcloud.com/loki/self-monitoring/loki
```

where `<cloud-region>` is e.g. `ap-southeast-1.aws`, `us-east-1.aws`. The trailing `/loki` is the standard Loki API base path; the CLI appends `/api/v1/...` to it directly.

The VictoriaMetrics endpoint URL pattern is:

```
https://www.ds.<region>.<provider>.observability.tidbcloud.com/internal/metrics/<o11y_id>
```

where `<o11y_id>` is a per-project observability ID (e.g. `019d3e6c-3e28-7d90-a2f1-2b74e6176cfb`). The CLI appends `/api/v1/...` to it directly. VictoriaMetrics is Prometheus-compatible and accepts MetricsQL (a superset of PromQL) in the `query` parameter.

The Alertmanager endpoint URL pattern is:

```
https://www.ds.<region>.<provider>.observability.tidbcloud.com/internal/alerts
```

The CLI appends `/api/v2/...` to it directly. Alertmanager exposes active, silenced, and inhibited alerts with their labels, annotations, and timestamps.

### Tencent Cloud TKE

TKE clusters do not use TiDB Cloud observability. Instead:

- **Logs**: Cloud Log Service (CLS). Accessed via `cls.tencentcloudapi.com` (`SearchLog` API, version `2020-08-10`). Requires `secret_id`, `secret_key`, `topic_id`, and `region`. Time range is in millisecond timestamps. CLS uses its own query syntax (not LogQL).
- **Metrics**: TMP Prometheus. Accessed via `monitor.tencentcloudapi.com` (`ExportPrometheusReadOnlyDynamicAPI`, version `2018-07-24`). Requires `secret_id`, `secret_key`, `instance_id`, and `region`. The API proxies standard Prometheus API calls; the response is a standard Prometheus JSON envelope wrapped in an `HTTP.ResponseBody` string.

Both use TC3-HMAC-SHA256 signing (Tencent Cloud API v3).

## Goals

- Provide a single CLI to query logs from any drive9 cluster without manually constructing Loki API calls.
- Provide a single CLI to query metrics from any drive9 cluster without manually constructing VictoriaMetrics API calls.
- Provide a single CLI to query alerts from any drive9 cluster without manually constructing Alertmanager API calls.
- Accept raw LogQL / MetricsQL as the query input so AI/agents can express arbitrarily complex queries without flag-level abstraction leaks.
- Resolve cluster connection details from a global config file so no endpoint is hardcoded in the binary.
- Keep the scope narrow: read-only queries. No log ingestion, no alert management, no dashboarding.

## Global config

A TOML file at `~/.config/drive9-monitor/config.toml` (overridable via `--config` flag or `DRIVE9_MONITOR_CONFIG` env var). It records, per cluster, the data source type, endpoint, and labels for each telemetry signal. A top-level `default_cluster` key sets the cluster used when `--cluster` is omitted.

```toml
default_cluster = "prod"

[clusters."aws-ap-southeast-1"]
logs.source_type = "loki"
logs.endpoint = "https://www.ds.ap-southeast-1.aws.observability.tidbcloud.com/loki/self-monitoring/loki"
logs.labels = {cluster_env = "prod", cluster = "drive9/eks/ap-southeast-1", app = "drive9-server"}
metrics.source_type = "prometheus"
metrics.endpoint = "https://www.ds.ap-southeast-1.aws.observability.tidbcloud.com/internal/metrics/019d3e6c-3e28-7d90-a2f1-2b74e6176cfb"
metrics.labels = {container = "drive9-server"}
alerts.source_type = "alertmanager"
alerts.endpoint = "https://www.ds.ap-southeast-1.aws.observability.tidbcloud.com/internal/alerts"
alerts.labels = {component = "drive9"}

# ... one entry per cluster ...
```

### Config schema

| Field                  | Type   | Required | Description                                      |
|------------------------|--------|----------|--------------------------------------------------|
| `default_cluster`      | string | no       | Cluster key used when `--cluster` is omitted     |
| `logs.source_type`     | string | yes      | `loki` or `tke_cls`                              |
| `logs.endpoint`        | string | for `loki` | Full Loki base URL for this cluster            |
| `logs.labels`          | map    | no       | Default label selectors appended to every query  |
| `logs.secret_id`       | string | for `tke_cls` | Tencent Cloud SecretId                     |
| `logs.secret_key`      | string | for `tke_cls` | Tencent Cloud SecretKey                    |
| `logs.topic_id`        | string | for `tke_cls` | CLS topic ID                                |
| `logs.region`          | string | for `tke_cls` | Tencent Cloud region (e.g. `ap-beijing`)    |
| `metrics.source_type`  | string | no       | `prometheus` or `tke_prometheus`                |
| `metrics.endpoint`     | string | for `prometheus` | Full VictoriaMetrics base URL             |
| `metrics.labels`       | map    | no       | Default label selectors appended to every query  |
| `metrics.secret_id`    | string | for `tke_prometheus` | Tencent Cloud SecretId               |
| `metrics.secret_key`   | string | for `tke_prometheus` | Tencent Cloud SecretKey              |
| `metrics.instance_id`  | string | for `tke_prometheus` | Prometheus instance ID               |
| `metrics.region`       | string | for `tke_prometheus` | Tencent Cloud region (e.g. `ap-beijing`) |
| `alerts.source_type`   | string | no       | `alertmanager`                                   |
| `alerts.endpoint`      | string | for `alertmanager` | Full Alertmanager base URL                |
| `alerts.labels`         | map    | no       | Default label selectors appended to every alert query |

`logs.labels`, `metrics.labels`, and `alerts.labels` are maps of key->value pairs that are AND-ed into every query's label selector. For example `metrics.labels = {container = "drive9-server"}` causes every metrics query to include `{container="drive9-server"}`. Multiple clusters sharing the same endpoint differ only in their labels.

## Commands

### `clusters`

Cluster management commands.

```
drive9-monitor clusters
drive9-monitor clusters use <key>
```

#### `clusters` (no subcommand)

List all configured clusters. Output: a table of cluster keys and which signals are configured (logs y/n, metrics y/n, alerts y/n). The current `default_cluster` is highlighted with a `*` marker.

#### `clusters use <key>`

Set the default cluster in the config file. Validates that the key exists in the `clusters` map before writing.

### `logs`

Query logs from a specified cluster.

```
drive9-monitor logs --cluster <key> [flags] <query>
```

#### Arguments

| Flag          | Short | Type    | Default        | Description                                       |
|---------------|-------|---------|----------------|---------------------------------------------------|
| `--cluster`   | `-c`  | string  | (default_cluster) | Cluster key from config, or `default_cluster` from config |
| `--since`     | `-s`  | duration| `1h`           | Lookback duration from `--to`                     |
| `--from`      |       | RFC3339 | (now - since)  | Start time                                        |
| `--to`        |       | RFC3339 | now            | End time                                          |
| `--limit`     | `-n`  | int     | `100`          | Max number of log lines to return                 |
| `--direction` |       | string  | `backward`     | Query direction: `forward` \| `backward`          |
| `--follow`    | `-f`  | bool    | false          | Tail new log entries (stream until Ctrl-C)        |
| `--output`    | `-o`  | string  | `text`         | Output format: `text` \| `raw` \| `json`          |
| `--config`    |       | string  | (default path) | Path to config file                               |

`<query>` is a positional argument. The query language depends on the cluster's `logs.source_type`:

- **`loki`**: a full LogQL log query (e.g. `{app="foo"} |= "error" | json | line_format "{{.msg}}"`). Passed to the Loki API verbatim.
- **`tke_cls`**: a CLS query string (e.g. `level:error AND tenant_id:abc123`). Passed to the CLS `SearchLog` API verbatim. If omitted, an empty query is used (returns all logs).

Config labels (`logs.labels`) are always applied as filters, regardless of whether a query is provided:

- **No query**: the stream selector is built entirely from config labels.
- **Query provided**: config labels are merged into the query. If the user already specifies a label with the same key, the user's value takes precedence.

For `loki`, labels are merged into the `{...}` stream selector (LogQL syntax). For `tke_cls`, labels are appended as `key:value` pairs (CLS query syntax).

#### Time semantics

- `--since` and `--to` are mutually convenient shortcuts; `--from` + `--to` take precedence if both are given.
- `--since 1h` means "from 1 hour ago to now".
- Durations use Go-style syntax (`30m`, `2h`, `1d`).
- `--direction backward` (default) returns the most recent logs first; `forward` returns oldest first. When `--limit` is set, `backward` ensures the most recent N lines are returned rather than the oldest N.

#### Output formats

- **text** (default): structured single-line format parsed from the JSON log line: `TIME LEVEL MESSAGE k1=v1 k2=v2 ...`. `TIME` is a human-friendly timestamp with timezone (e.g. `2026-07-14 12:35:27 +08:00`), `LEVEL` is from the `level` field, `MESSAGE` is from the `msg` field. All remaining JSON fields (excluding `level`, `msg`) — including `caller`, `tenant_id`, etc. — are appended as `key=value` pairs in alphabetical order. If the log line is not valid JSON, the raw line is printed instead. Colorized when stdout is a TTY.
- **json**: the full log line as returned by Loki (the raw JSON content of the log entry), one per line. No syntax highlighting — always plain text for safe pipe consumption.
- **raw**: `<timestamp> <stream labels> <raw log line>`, one per line. Colorized when stdout is a TTY (timestamp cyan, labels dimmed).

#### `--follow` mode

When `-f` is set, the CLI issues a Loki tail query (`/api/v1/tail`) and streams new log entries to stdout until interrupted. `--limit` and `--direction` are ignored in this mode. `--since`/`--from`/`--to` are also ignored — tail starts from "now" and only delivers new entries, consistent with `kubectl logs -f` behavior. `--follow` is only supported for `loki` source type — `tke_cls` does not support tailing; using `-f` with `tke_cls` returns an error.

### `metrics`

Query metrics from a specified cluster.

```
drive9-monitor metrics --cluster <key> [flags] <query>
```

#### Arguments

| Flag          | Short | Type    | Default        | Description                                       |
|---------------|-------|---------|----------------|---------------------------------------------------|
| `--cluster`   | `-c`  | string  | (default_cluster) | Cluster key from config, or `default_cluster` from config |
| `--since`     | `-s`  | duration| `1h`           | Lookback duration from `--to`                     |
| `--from`      |       | RFC3339 | (now - since)  | Start time                                        |
| `--to`        |       | RFC3339 | now            | End time                                          |
| `--step`      |       | duration| `30s`          | Query resolution step                             |
| `--refresh`   |       | duration| `10s`           | Auto-refresh interval (only for `tui` output)    |
| `--output`    | `-o`  | string  | `tui`          | Output format: `tui` \| `table` \| `json`         |
| `--config`    |       | string  | (default path) | Path to config file                               |

`<query>` is a positional argument containing a full MetricsQL / PromQL expression (e.g. `drive9_service_gauge{component="tenant_pool",name="cached_backends"}` or `rate(http_requests_total[5m])`). The query is passed to the metrics API with no escaping or flag-level filtering abstractions. Both `prometheus` (VictoriaMetrics) and `tke_prometheus` (TMP Prometheus) source types accept the same PromQL/MetricsQL syntax.

Config labels (`metrics.labels`) are always applied as filters, regardless of whether a query is provided — labels are merged into the query's `{...}` selector, same merge rules as `logs` (see above).

#### Time semantics

- Same `--since`/`--from`/`--to` behavior as `logs`.
- `--step` controls the query resolution step for `query_range` calls. Supports Go-style duration syntax (`15s`, `1m`, `5m`).

#### Output formats

- **tui** (default): interactive terminal UI rendering a time-series line chart. The chart auto-refreshes every `--refresh` interval (default 10s). Each series is plotted as a separate line with its label set shown in a legend. Press `q` or `Ctrl-C` to exit.
- **table**: human-readable table — one row per time step, columns for each series (identified by a compact label representation). Intended for quick scanning in the terminal.
- **json**: the raw VictoriaMetrics API JSON response, one per query. No transformation — suitable for AI/agent parsing.

#### TUI behavior

- The TUI occupies the full terminal window.
- The chart shows the full `--since` time range on the X axis and auto-scales the Y axis.
- Each series is a line in the chart; the legend maps line colors to label sets.
- The bottom bar shows the query, cluster, refresh interval, and next refresh time.
- `q` / `Ctrl-C` exits. `r` forces an immediate refresh.

### `alerts`

Query alerts from a specified cluster.

```
drive9-monitor alerts --cluster <key> [flags] [query]
```

#### Arguments

| Flag          | Short | Type    | Default        | Description                                       |
|---------------|-------|---------|----------------|---------------------------------------------------|
| `--cluster`   | `-c`  | string  | (default_cluster) | Cluster key from config, or `default_cluster` from config |
| `--state`     |       | string  | `active`       | Alert state filter: `active` \| `silenced` \| `inhibited` \| `all` |
| `--output`    | `-o`  | string  | `text`         | Output format: `text` \| `json`                  |
| `--config`    |       | string  | (default path) | Path to config file                               |

`[query]` is an optional positional argument containing an Alertmanager label matcher expression (e.g. `{severity="critical"}` or `severity="critical"`). Braces are optional. If omitted, all alerts are returned.

Config labels (`alerts.labels`) are always applied as filters — same merge rules as `logs` (config labels merged into the `{...}` selector, user-specified labels take precedence).

#### State filter

- `--state active` (default): only alerts in `active` state.
- `--state silenced`: only silenced alerts.
- `--state inhibited`: only alerts inhibited by other alerts.
- `--state all`: all alerts regardless of state.

#### Output formats

- **text** (default): one alert per entry, multi-line block format:
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
  `TIME` is the `startsAt` timestamp in human-friendly format with timezone (e.g. `2026-07-14 15:30:00 +08:00`), `SEVERITY` is from the `severity` label, `NAME` is from the `alertname` label, `STATE` is the alert state (active/silenced/inhibited). The block includes `startsAt`, `endsAt`, `fingerprint`, all `labels` (in alphabetical order, excluding `severity` and `alertname` which are in the header), and all `annotations` (in alphabetical order). Colorized when stdout is a TTY (severity colored by level: critical/major=red, warning=yellow, info=green).
- **json**: the raw Alertmanager API v2 JSON response (array of alert objects), one JSON array. No transformation — suitable for AI/agent parsing.

### `rules`

Show alert rule definitions fetched from the [runbooks repository](https://github.com/tidbcloud/runbooks). The rules file is a Prometheus alerting rules YAML hosted at `rules/mem9/mnemos/drive9-alerts.yaml` in the `tidbcloud/runbooks` private repo. Access requires the `gh` CLI with valid authentication to the repo.

```
drive9-monitor rules [name]
```

#### Arguments

| Argument | Type   | Default | Description                                              |
|----------|--------|---------|---------------------------------------------------------|
| `name`   | string | (none)  | Optional alert name to filter. If omitted, lists all rules. |
| `--config` |     | string  | (default path) Path to config file                       |

#### Behavior

- The CLI runs `gh api repos/tidbcloud/runbooks/contents/rules/mem9/mnemos/drive9-alerts.yaml --jq '.content'` and decodes the base64 content to obtain the YAML.
- If `gh` is not installed or not authenticated, the error message hints the user to install and authenticate `gh` CLI.
- The YAML is parsed for alert rule definitions. Each rule has `alert` (name), `expr` (PromQL expression), `for` (duration), `labels`, and `annotations`.

#### Output

- **No name argument**: list all rule names, one per line: `SEVERITY ALERTNAME — summary`.
- **With name argument**: show the full rule definition for that alert:
  ```
  ALERTNAME (severity, for)
  expr: |
      <PromQL expression>
  labels:
      k1=v1
      k2=v2
  annotations:
      k1=v1
      k2=v2
  ```
  Colorized when stdout is a TTY.

## Logs API usage

### Loki (`source_type = "loki"`)

The CLI targets the Loki HTTP API. The `endpoint` in config is the standard Loki base URL (including any path prefix). API paths are appended directly to the endpoint.

| CLI command     | API path                          |
|-----------------|-----------------------------------|
| `logs`          | `/api/v1/query_range`             |
| `logs -f`       | `/api/v1/tail` (WebSocket)        |

Query construction:

1. Config labels (`logs.labels`) are always merged into the query's stream selector as filters (user-specified labels take precedence; see the `logs` command section above for the merge rules).
2. The resulting query is sent to the Loki API as the `query` parameter — no escaping, no additional transformation.
3. `query_range` calls include `start`, `end`, `limit`, and `direction` parameters derived from the flags (default `backward`).

### TKE CLS (`source_type = "tke_cls"`)

The CLI calls the Tencent Cloud CLS `SearchLog` API via `cls.tencentcloudapi.com` (version `2020-08-10`). Authentication uses TC3-HMAC-SHA256 with `secret_id`/`secret_key`.

- Request body: `{TopicId, From, To, Limit, Query}` where `From`/`To` are millisecond timestamps.
- Config labels are appended to the query as CLS `key:value` filter pairs.
- `--follow` is not supported (returns error).
- Response: `Results[].LogJson` contains the raw JSON log line (same format as Loki), `Results[].Time` is the millisecond timestamp.

## Metrics API usage

### VictoriaMetrics (`source_type = "prometheus"`)

The CLI targets the VictoriaMetrics HTTP API (Prometheus-compatible). The `endpoint` in config is the base URL (including the `/internal/metrics/<o11y_id>` path). API paths are appended directly to the endpoint.

| CLI command     | API path                          |
|-----------------|-----------------------------------|
| `metrics`       | `/api/v1/query_range`             |

Query construction:

1. Config labels (`metrics.labels`) are always merged into the query's `{...}` label selector as filters (user-specified labels take precedence; same merge rules as `logs`).
2. The resulting query is sent to the VictoriaMetrics API as the `query` parameter — no escaping, no additional transformation.
3. `query_range` calls include `start`, `end`, and `step` parameters derived from the flags.
4. The query language is MetricsQL (a superset of PromQL). Both PromQL and MetricsQL expressions are accepted by the API without any flag or mode switch.

### TKE Prometheus (`source_type = "tke_prometheus"`)

The CLI calls the Tencent Cloud Monitor `ExportPrometheusReadOnlyDynamicAPI` via `monitor.tencentcloudapi.com` (version `2018-07-24`). Authentication uses TC3-HMAC-SHA256 with `secret_id`/`secret_key`.

- Request body: `{InstanceId, Method:"GET", Path:"/api/v1/query_range?query=...&start=...&end=...&step=..."}`
- The API proxies to the internal Prometheus instance; query/response format is standard Prometheus.
- Config labels are merged into the query's `{...}` selector (same as VictoriaMetrics).
- Response: `HTTP.ResponseBody` is a JSON string containing a standard Prometheus `matrix` response — unwrapped and parsed the same way as VictoriaMetrics.

## Alerts API usage

### Alertmanager (`source_type = "alertmanager"`)

The CLI targets the Alertmanager HTTP API v2. The `endpoint` in config is the base URL (including the `/internal/alerts` path). API paths are appended directly to the endpoint.

| CLI command     | API path                          |
|-----------------|-----------------------------------|
| `alerts`        | `/api/v2/alerts`                  |

Query construction:

1. Config labels (`alerts.labels`) are always merged into the filter as `key="value"` pairs (user-specified labels take precedence; user query may use braces but they are stripped before sending).
2. Each matcher is sent as a separate `filter` query parameter to the Alertmanager API (not comma-separated).
3. The `active`, `silenced`, and `inhibited` boolean parameters are derived from the `--state` flag.
4. Response: a JSON array of alert objects, each with `labels`, `annotations`, `startsAt`, `endsAt`, `status.state`, `fingerprint`, and `receivers`.

## Rules API usage

The CLI fetches alert rule definitions from the `tidbcloud/runbooks` private GitHub repository via the `gh` CLI. The rules file is a Prometheus alerting rules YAML at `rules/mem9/mnemos/drive9-alerts.yaml`.

- Command: `gh api repos/tidbcloud/runbooks/contents/rules/mem9/mnemos/drive9-alerts.yaml --jq '.content'`
- The response is base64-encoded YAML; the CLI decodes it and parses the Prometheus rules format.
- Requires `gh` CLI installed and authenticated with access to the private repo.

## Error handling

- Unknown cluster key -> error with the list of valid keys.
- Config file missing -> error with the expected path and a hint to run `clusters` after creating it.
- HTTP 403 -> the endpoint rejected the request (likely an auth/network issue reachable only via Feilian/VPN). Print a prominent hint telling the user to connect to Feilian first, ahead of any other error detail.
- Other network / HTTP errors -> error with status code and response body snippet.
- Loki/VictoriaMetrics/Alertmanager/Tencent Cloud API error responses (non-2xx) -> parsed and displayed with the error message.
