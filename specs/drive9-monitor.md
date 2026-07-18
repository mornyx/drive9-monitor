# drive9-monitor

A CLI tool for querying monitoring data (logs, metrics, and alerts) from [drive9-server](https://github.com/mem9-ai/drive9) across all deployment clusters. It is designed primarily for AI/agent consumption: queries are expressed as raw LogQL/MetricsQL/CLS query strings rather than fragmented flags, giving the caller full expressive power.

## Background

drive9-server is deployed to multiple clusters across regions and cloud providers. The CLI supports multiple data source types for each telemetry signal (logs, metrics, alerts). The choice of data source type is determined by the cluster's config — the CLI does not assume any mapping between clusters and backends.

### Data source types

**Logs**:
- `loki` — direct HTTP access to a Loki instance. Query language is LogQL. Supports `--follow` (WebSocket tail).
- `grafana` — Grafana datasource proxy to a Loki datasource. Query language is LogQL (same as `loki`). Requires `datasource` (Grafana datasource UID), `username`, `password`. `--follow` is not supported.
- `tke_cls` — Tencent Cloud CLS (Cloud Log Service) via Tencent Cloud API v3. Requires `secret_id`, `secret_key`, `topic_id`, `region`. Query language is CLS syntax (not LogQL). `--follow` is not supported.

**Metrics**:
- `prometheus` — direct HTTP access to a VictoriaMetrics instance. Query language is MetricsQL/PromQL.
- `grafana` — Grafana datasource proxy to a Prometheus datasource. Query language is PromQL (same as `prometheus`). Requires `datasource` (Grafana datasource UID), `username`, `password`.
- `tke_prometheus` — Tencent Cloud TMP Prometheus via `ExportPrometheusReadOnlyDynamicAPI`. Requires `secret_id`, `secret_key`, `instance_id`, `region`. Query language is PromQL.

**Alerts**:
- `alertmanager` — direct HTTP access to an Alertmanager instance.

**Jira alerts** (global, not per-cluster):
- Jira Cloud REST API v3. Requires `jira.endpoint`, `jira.email`, `jira.token` in config.

## Goals

- Provide a single CLI to query logs from any drive9 cluster without manually constructing backend API calls.
- Provide a single CLI to query metrics from any drive9 cluster without manually constructing backend API calls.
- Provide a single CLI to query alerts from any drive9 cluster without manually constructing Alertmanager API calls.
- Provide a single CLI to query Jira alert tickets without manually constructing Jira API calls.
- Accept raw LogQL / MetricsQL / PromQL / CLS query strings as the query input so AI/agents can express arbitrarily complex queries without flag-level abstraction leaks.
- Resolve cluster connection details from a global config file so no endpoint is hardcoded in the binary.
- Keep the scope narrow: read-only queries. No log ingestion, no alert management, no dashboarding.

## Global config

A TOML file at `~/.config/drive9-monitor/config.toml` (overridable via `--config` flag or `DRIVE9_MONITOR_CONFIG` env var). It records, per cluster, the data source type, endpoint, and labels for each telemetry signal. A top-level `default_cluster` key sets the cluster used when `--cluster` is omitted.

```toml
default_cluster = "prod"

[jira]
endpoint = "https://your-domain.atlassian.net"
email = "you@example.com"
token = "ATATT3xFfGF0..."
labels = {component = "drive9"}

[clusters."aws-ap-southeast-1"]
logs.source_type = "loki"
logs.endpoint = "https://<o11y-host>/loki/self-monitoring/loki"
logs.labels = {cluster_env = "prod", cluster = "drive9/eks/ap-southeast-1", app = "drive9-server"}
metrics.source_type = "prometheus"
metrics.endpoint = "https://<o11y-host>/internal/metrics/<o11y-id>"
metrics.labels = {container = "drive9-server"}
alerts.source_type = "alertmanager"
alerts.endpoint = "https://<o11y-host>/internal/alerts"
alerts.labels = {component = "drive9"}

# ... one entry per cluster ...
```

### Config schema

| Field                  | Type   | Required | Description                                      |
|------------------------|--------|----------|--------------------------------------------------|
| `default_cluster`      | string | no       | Cluster key used when `--cluster` is omitted     |
| `logs.source_type`     | string | yes      | `loki`, `grafana`, or `tke_cls`                  |
| `logs.endpoint`        | string | for `loki`/`grafana` | Full base URL (Loki or Grafana)           |
| `logs.datasource`      | string | for `grafana` | Grafana datasource UID (Loki)              |
| `logs.username`        | string | for `grafana` | Grafana Basic Auth username                  |
| `logs.password`        | string | for `grafana` | Grafana Basic Auth password                  |
| `logs.secret_id`       | string | for `tke_cls` | Tencent Cloud SecretId                      |
| `logs.secret_key`      | string | for `tke_cls` | Tencent Cloud SecretKey                    |
| `logs.topic_id`        | string | for `tke_cls` | CLS topic ID                               |
| `logs.region`          | string | for `tke_cls` | Tencent Cloud region                       |
| `logs.labels`          | map    | no       | Default label selectors appended to every query  |
| `metrics.source_type`  | string | no       | `prometheus`, `grafana`, or `tke_prometheus`    |
| `metrics.endpoint`     | string | for `prometheus`/`grafana` | Full base URL (VictoriaMetrics or Grafana) |
| `metrics.datasource`   | string | for `grafana` | Grafana datasource UID                         |
| `metrics.username`     | string | for `grafana` | Grafana Basic Auth username                     |
| `metrics.password`     | string | for `grafana` | Grafana Basic Auth password                     |
| `metrics.secret_id`    | string | for `tke_prometheus` | Tencent Cloud SecretId               |
| `metrics.secret_key`   | string | for `tke_prometheus` | Tencent Cloud SecretKey              |
| `metrics.instance_id`  | string | for `tke_prometheus` | Prometheus instance ID               |
| `metrics.region`       | string | for `tke_prometheus` | Tencent Cloud region (e.g. `ap-beijing`) |
| `metrics.labels`       | map    | no       | Default label selectors appended to every query  |
| `alerts.source_type`   | string | no       | `alertmanager`                                   |
| `alerts.endpoint`      | string | for `alertmanager` | Full Alertmanager base URL                |
| `alerts.labels`         | map    | no       | Default label selectors appended to every alert query |
| `jira.endpoint`         | string | yes (for jira-alerts) | Jira base URL (e.g. `https://your-domain.atlassian.net`)     |
| `jira.email`             | string | yes (for jira-alerts) | Atlassian account email for Basic Auth     |
| `jira.token`             | string | yes (for jira-alerts) | Jira Cloud API token                        |
| `jira.labels`            | map    | no       | Default JQL conditions (e.g. `{component = "drive9"}`) |

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
- **`grafana`**: a full LogQL log query (same syntax as `loki`), proxied through the Grafana datasource proxy API.
- **`tke_cls`**: a CLS query string (e.g. `level:error AND tenant_id:abc123`). Passed to the CLS `SearchLog` API verbatim. If omitted, an empty query is used (returns all logs). `--direction` is ignored for `tke_cls` — `SearchLog` has no direction parameter and always returns newest-first; the CLI prints entries in chronological order.

Config labels (`logs.labels`) are always applied as filters, regardless of whether a query is provided:

- **No query**: the stream selector is built entirely from config labels.
- **Query provided**: config labels are merged into the query. If the user already specifies a label with the same key, the user's value takes precedence.

For `loki` and `grafana`, labels are merged into the `{...}` stream selector (LogQL syntax). For `tke_cls`, labels are appended as `key:value` pairs (CLS query syntax).

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

When `-f` is set, the CLI issues a Loki tail query (`/api/v1/tail`) and streams new log entries to stdout until interrupted. `--limit` and `--direction` are ignored in this mode. `--since`/`--from`/`--to` are also ignored — tail starts from "now" and only delivers new entries, consistent with `kubectl logs -f` behavior. `--follow` is only supported for `loki` source type — `grafana` and `tke_cls` do not support tailing; using `-f` with either returns an error.

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

`<query>` is a positional argument containing a full MetricsQL / PromQL expression (e.g. `drive9_service_gauge{component="tenant_pool",name="cached_backends"}` or `rate(http_requests_total[5m])`). The query is passed to the metrics API with no escaping or flag-level filtering abstractions. Both `prometheus` (VictoriaMetrics) and `grafana` (Grafana datasource proxy) source types accept the same PromQL/MetricsQL syntax.

Config labels (`metrics.labels`) are always applied as filters, regardless of whether a query is provided — labels are merged into the query's `{...}` selector, same merge rules as `logs` (see above).

#### Time semantics

- Same `--since`/`--from`/`--to` behavior as `logs`.
- `--step` controls the query resolution step for `query_range` calls. Supports Go-style duration syntax (`15s`, `1m`, `5m`).

#### Output formats

- **tui** (default): interactive terminal UI rendering a time-series line chart. The chart auto-refreshes every `--refresh` interval (default 10s). Each series is plotted as a separate line with its label set shown in a legend. Press `q` or `Ctrl-C` to exit.
- **table**: human-readable table — one row per time step, columns for each series (identified by a compact label representation). Intended for quick scanning in the terminal.
- **json**: a Prometheus-style matrix JSON document reconstructed from the parsed API response (`{"status":"success","data":{"resultType":"matrix","result":[...]}}`), one per query. Suitable for AI/agent parsing.

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
- **json**: a JSON array of alert objects reconstructed from the Alertmanager API v2 response (`labels`, `annotations`, `startsAt`, `endsAt`, `status.state`, `fingerprint`). Suitable for AI/agent parsing.

### `jira-alerts`

Query alert tickets from Jira. Unlike `alerts` (per-cluster Alertmanager), Jira is a global signal — all clusters' tickets live in the same O11Y project.

```
drive9-monitor jira-alerts [flags] [query]
```

#### Arguments

| Flag          | Short | Type    | Default        | Description                                       |
|---------------|-------|---------|----------------|---------------------------------------------------|
| `--limit`     | `-n`  | int     | `5`            | Max number of tickets to return (`0` = all)       |
| `--output`    | `-o`  | string  | `text`         | Output format: `text` \| `json`                  |
| `--config`    |       | string  | (default path) | Path to config file                               |

`[query]` is an optional positional argument containing a JQL expression fragment (e.g. `statusCategory != "Done"` or `project = "O11Y"`). If omitted, only config labels are used as JQL conditions.

Config labels (`jira.labels`) are always applied as base JQL conditions, AND-joined as `key = "value"` pairs (e.g. `{component = "drive9"}` becomes `component = "drive9"`). The user query is AND-ed with the base JQL. `ORDER BY created DESC` is appended automatically.

#### Output formats

- **text** (default): one ticket per entry, multi-line block format:
  ```
  TIME PRIORITY KEY STATUS SUMMARY {
      created=...,
      updated=...,
      project=...,
      components=[...],
      description=<plain text extracted from Atlassian Document Format>,
  }
  ```
  `TIME` is the `created` timestamp in human-friendly format with timezone (e.g. `2026-07-14 20:18:19 +08:00`). `PRIORITY` is from the Jira priority field. `KEY` is the issue key (e.g. `O11Y-2615909`). `STATUS` is the status name. `SUMMARY` is the issue summary. The block also includes `created`, `updated`, `project`, `components` (list of component names), and `description` (plain text extracted from the Atlassian Document Format description, which contains alert labels including `o11y_region`, `namespace`, `severity`, `alertname`, etc.). Labels are omitted from the block as they are auto-generated hashes with no useful information. Colorized when stdout is a TTY (priority colored: blocker/重要=red, others=yellow).
- **json**: a JSON array of issue objects with normalized fields (`key`, `summary`, `status`, `statusCategory`, `priority`, `created`, `updated`, `project`, `components`, `description`). Suitable for AI/agent parsing.

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

### Grafana datasource proxy (`source_type = "grafana"`)

The CLI calls the Grafana datasource proxy API with Basic Auth. The `endpoint` is the Grafana base URL, `datasource` is the Loki datasource UID.

- API path: `GET <endpoint>/api/datasources/proxy/uid/<datasource>/loki/api/v1/query_range?query=...&start=...&end=...&limit=...&direction=...`
- Authentication: HTTP Basic Auth with `username`/`password`.
- Query syntax and response format are identical to direct Loki access (LogQL, stream entries).
- Config labels are merged into the query's `{...}` stream selector (same as `loki`).
- `--follow` is not supported (returns error).

### TKE CLS (`source_type = "tke_cls"`)

The CLI calls the Tencent Cloud CLS `SearchLog` API via `cls.tencentcloudapi.com` (version `2020-10-16`). Authentication uses TC3-HMAC-SHA256 with `secret_id`/`secret_key`.

- Request body: `{TopicId, From, To, Limit, Query}` where `From`/`To` are millisecond timestamps.
- Config labels are appended to the query as CLS `key:value` filter pairs.
- `--follow` is not supported (returns error).
- Response: `Results[].LogJson` contains the raw JSON log line (same format as Loki), `Results[].Time` is the millisecond timestamp.

## Metrics API usage

### Prometheus/VictoriaMetrics (`source_type = "prometheus"`)

The CLI targets the VictoriaMetrics HTTP API (Prometheus-compatible). The `endpoint` in config is the base URL (including the `/internal/metrics/<o11y_id>` path). API paths are appended directly to the endpoint.

| CLI command     | API path                          |
|-----------------|-----------------------------------|
| `metrics`       | `/api/v1/query_range`             |

Query construction:

1. Config labels (`metrics.labels`) are always merged into the query's `{...}` label selector as filters (user-specified labels take precedence; same merge rules as `logs`).
2. The resulting query is sent to the VictoriaMetrics API as the `query` parameter — no escaping, no additional transformation.
3. `query_range` calls include `start`, `end`, and `step` parameters derived from the flags.
4. The query language is MetricsQL (a superset of PromQL). Both PromQL and MetricsQL expressions are accepted by the API without any flag or mode switch.

### Grafana datasource proxy (`source_type = "grafana"`)

The CLI calls the Grafana datasource proxy API with Basic Auth. The `endpoint` is the Grafana base URL, `datasource` is the datasource UID.

- API path: `GET <endpoint>/api/datasources/proxy/uid/<datasource>/api/v1/query_range?query=...&start=...&end=...&step=...`
- Authentication: HTTP Basic Auth with `username`/`password`.
- The API proxies to the backend Prometheus instance; query/response format is standard Prometheus.
- Config labels are merged into the query's `{...}` selector (same as VictoriaMetrics).
- Response is a standard Prometheus `matrix` JSON — parsed the same way as VictoriaMetrics.

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

## Jira API usage

### Jira Cloud (`jira.endpoint` / `jira.email` / `jira.token`)

The CLI targets the Jira REST API v3. The `jira.endpoint` in config is the Jira base URL (e.g. `https://your-domain.atlassian.net`). API paths are appended directly to the endpoint.

| CLI command     | API path                          |
|-----------------|-----------------------------------|
| `jira-alerts`   | `/rest/api/3/search/jql`          |

Authentication: HTTP Basic Auth with `jira.email` as the username and `jira.token` as the password.

Query construction:

1. Config labels (`jira.labels`) are always converted to JQL conditions as `key = "value"` pairs and AND-joined (e.g. `{component = "drive9"}` becomes `component = "drive9"`).
2. The user query (if provided) is AND-ed with the base JQL conditions.
3. `ORDER BY created DESC` is appended automatically.
4. The final JQL is sent as the `jql` query parameter.

Pagination: the `/search/jql` endpoint uses cursor-based pagination. The CLI fetches pages of 100 issues at a time using the `nextPageToken` parameter until `--limit` is reached or `isLast` is true. `--limit 0` fetches all pages.

Response: `issues[]` array, each issue has `key`, `id`, and `fields.{summary, status.{name, statusCategory.{key}}, priority.{name}, created, updated, project.{key, name}, components[].name, labels[], description}`. The `description` field is in Atlassian Document Format (nested JSON); the CLI extracts plain text from it for the text output format.

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
- Jira 401/403 -> error with a hint to check `jira.email` and `jira.token` in config (API token may be expired or revoked).
- Loki/VictoriaMetrics/Alertmanager/Tencent Cloud API error responses (non-2xx) -> parsed and displayed with the error message.
