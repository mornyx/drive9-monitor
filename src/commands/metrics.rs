use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::config::Config;
use crate::victoriametrics::{MetricSeries, VmClient};

/// Arguments for the `metrics` subcommand.
pub struct MetricsArgs {
    pub cluster: Option<String>,
    pub query: String,
    pub since: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub step: String,
    pub refresh: String,
    pub output: String,
}

/// Output format for metrics.
enum OutputFormat {
    Tui,
    Table,
    Json,
}

impl OutputFormat {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "tui" => Ok(Self::Tui),
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            other => anyhow::bail!("invalid output format '{}': expected tui, table, or json", other),
        }
    }
}

/// A metrics query executor that abstracts over different backends.
enum MetricsBackend {
    Vm(VmClient),
    TkeProm(crate::tke_prometheus::TkePromClient),
}

impl MetricsBackend {
    async fn query_range(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step: Duration,
    ) -> Result<Vec<MetricSeries>> {
        match self {
            MetricsBackend::Vm(c) => c.query_range(query, start, end, step).await,
            MetricsBackend::TkeProm(c) => c.query_range(query, start, end, step).await,
        }
    }
}

/// Entry point for the `metrics` subcommand.
pub async fn run(config: &Config, args: MetricsArgs) -> Result<()> {
    let cluster_key = config.resolve_cluster_key(args.cluster.as_deref())?;
    let cluster = config.cluster(&cluster_key)?;
    let metrics = cluster.metrics()?;

    let query = super::logs::merge_labels_into_query(&args.query, &metrics.labels);
    let output = OutputFormat::parse(&args.output)?;
    let step = humantime::parse_duration(&args.step)
        .with_context(|| format!("invalid --step duration '{}'", args.step))?;

    let backend = match metrics.source_type.as_str() {
        "prometheus" => {
            MetricsBackend::Vm(VmClient::new(&metrics.endpoint)?)
        }
        "tke_prometheus" => {
            let secret_id = metrics.secret_id.as_ref().context("tke_prometheus requires secret_id")?;
            let secret_key = metrics.secret_key.as_ref().context("tke_prometheus requires secret_key")?;
            let instance_id = metrics.instance_id.as_ref().context("tke_prometheus requires instance_id")?;
            let region = metrics.region.as_ref().context("tke_prometheus requires region")?;
            MetricsBackend::TkeProm(crate::tke_prometheus::TkePromClient::new(
                secret_id, secret_key, instance_id, region,
            )?)
        }
        other => anyhow::bail!("unsupported metrics source_type '{}'", other),
    };

    match output {
        OutputFormat::Json => {
            let (start, end) = resolve_time_range(&args.since, &args.from, &args.to)?;
            let series = backend.query_range(&query, start, end, step).await?;
            print_json(&series);
            Ok(())
        }
        OutputFormat::Table => {
            let (start, end) = resolve_time_range(&args.since, &args.from, &args.to)?;
            let series = backend.query_range(&query, start, end, step).await?;
            print_table(&series);
            Ok(())
        }
        OutputFormat::Tui => {
            let refresh = humantime::parse_duration(&args.refresh)
                .with_context(|| format!("invalid --refresh duration '{}'", args.refresh))?;
            run_tui(backend, &cluster_key, &args, &query, step, refresh).await
        }
    }
}

/// Run the interactive TUI with auto-refresh.
async fn run_tui(
    backend: MetricsBackend,
    cluster_key: &str,
    args: &MetricsArgs,
    query: &str,
    step: Duration,
    refresh: Duration,
) -> Result<()> {
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use crossterm::event::{Event, KeyCode, KeyEventKind};
    use crossterm::event::EventStream;
    use futures_util::StreamExt;
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use std::io::stdout;

    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let term_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(term_backend).context("failed to create terminal")?;

    let mut last_series: Vec<MetricSeries> = Vec::new();
    let mut last_error: Option<String> = None;
    let mut next_refresh = tokio::time::Instant::now();

    loop {
        // Check if it's time to refresh.
        let now = tokio::time::Instant::now();
        if now >= next_refresh {
            let (start, end) = resolve_time_range(&args.since, &args.from, &args.to)?;
            match backend.query_range(query, start, end, step).await {
                Ok(s) => {
                    last_series = s;
                    last_error = None;
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                }
            }
            next_refresh = now + refresh;
        }

        // Render.
        terminal.draw(|f| {
            draw_tui(f, query, &cluster_key, refresh, next_refresh, &last_series, last_error.as_deref());
        })?;

        // Wait for either a key event or the next refresh time.
        let wait = next_refresh.saturating_duration_since(tokio::time::Instant::now());
        let mut event_stream = EventStream::new();
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            maybe_event = event_stream.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => break,
                            KeyCode::Char('r') | KeyCode::Char('R') => {
                                next_refresh = tokio::time::Instant::now();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to show cursor")?;

    Ok(())
}

/// Draw the TUI.
fn draw_tui(
    frame: &mut ratatui::Frame,
    query: &str,
    cluster: &str,
    refresh: Duration,
    next_refresh: tokio::time::Instant,
    series: &[MetricSeries],
    error: Option<&str>,
) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Axis, Block, Chart, Dataset, Paragraph, Borders};

    // Layout: chart (main) + legend + bottom bar.
    let legend_height = series.len().min(6) as u16 + 2; // +2 for borders
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(legend_height), Constraint::Length(3)])
        .split(area);

    // Chart area.
    let chart_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" metrics — cluster: {} ", cluster));

    if let Some(err) = error {
        let para = Paragraph::new(format!("Error: {}", err))
            .style(Style::default().fg(Color::Red))
            .block(chart_block);
        frame.render_widget(para, chunks[0]);
    } else if series.is_empty() {
        let para = Paragraph::new("No data")
            .style(Style::default().fg(Color::Yellow))
            .block(chart_block);
        frame.render_widget(para, chunks[0]);
    } else {
        // Build datasets.
        let colors = [Color::Cyan, Color::Green, Color::Yellow, Color::Magenta, Color::Blue, Color::Red];
        let all_data: Vec<Vec<(f64, f64)>> = series
            .iter()
            .map(|s| {
                s.points
                    .iter()
                    .map(|(dt, v)| (dt.timestamp() as f64, *v))
                    .collect()
            })
            .collect();

        let datasets: Vec<Dataset> = all_data
            .iter()
            .enumerate()
            .map(|(i, data)| {
                Dataset::default()
                    .name(format_series_label(&series[i].metric))
                    .marker(ratatui::symbols::Marker::Braille)
                    .graph_type(ratatui::widgets::GraphType::Line)
                    .style(Style::default().fg(colors[i % colors.len()]))
                    .data(data)
            })
            .collect();

        // Determine X axis range.
        let (x_min, x_max) = series
            .iter()
            .flat_map(|s| s.points.iter())
            .map(|(dt, _)| dt.timestamp() as f64)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), x| {
                (mn.min(x), mx.max(x))
            });

        // Determine Y axis range.
        let (y_min, y_max) = series
            .iter()
            .flat_map(|s| s.points.iter())
            .map(|(_, v)| *v)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), y| {
                (mn.min(y), mx.max(y))
            });

        let chart = Chart::new(datasets)
            .block(chart_block)
            .x_axis(
                Axis::default()
                    .title("time")
                    .style(Style::default().fg(Color::Gray))
                    .bounds([x_min, x_max])
                    .labels(vec![
                        Span::styled(format_time_label(x_min), Style::default().fg(Color::Gray)),
                        Span::styled(format_time_label((x_min + x_max) / 2.0), Style::default().fg(Color::Gray)),
                        Span::styled(format_time_label(x_max), Style::default().fg(Color::Gray)),
                    ]),
            )
            .y_axis(
                Axis::default()
                    .title("value")
                    .style(Style::default().fg(Color::Gray))
                    .bounds([y_min, y_max])
                    .labels(vec![
                        Span::styled(format!("{:.2}", y_min), Style::default().fg(Color::Gray)),
                        Span::styled(format!("{:.2}", (y_min + y_max) / 2.0), Style::default().fg(Color::Gray)),
                        Span::styled(format!("{:.2}", y_max), Style::default().fg(Color::Gray)),
                    ]),
            );
        frame.render_widget(chart, chunks[0]);
    }

    // Legend.
    let colors = [Color::Cyan, Color::Green, Color::Yellow, Color::Magenta, Color::Blue, Color::Red];
    let legend_lines: Vec<Line> = series
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let color = colors[i % colors.len()];
            let name = s.metric.get("__name__").cloned().unwrap_or_default();
            let labels: Vec<String> = s
                .metric
                .iter()
                .filter(|(k, _)| *k != "__name__")
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            let label_str = labels.join(", ");
            Line::from(vec![
                Span::styled("● ", Style::default().fg(color)),
                Span::styled(name, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {{{}}}", label_str)),
            ])
        })
        .collect();
    let legend_block = Block::default()
        .borders(Borders::ALL)
        .title(" legend ");
    let legend = Paragraph::new(legend_lines).block(legend_block);
    frame.render_widget(legend, chunks[1]);

    // Bottom bar.
    let now = tokio::time::Instant::now();
    let secs_until = next_refresh.saturating_duration_since(now).as_secs();
    let bottom = Line::from(vec![
        Span::styled(
            format!(" q:quit  r:refresh  "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("refresh={}s  ", refresh.as_secs())),
        Span::raw(format!("next in {}s  ", secs_until)),
        Span::styled(query, Style::default().fg(Color::DarkGray)),
    ]);
    let bottom_block = Block::default()
        .borders(Borders::ALL)
        .title(" controls ");
    let para = Paragraph::new(bottom).block(bottom_block);
    frame.render_widget(para, chunks[2]);
}

/// Format a Unix timestamp as a short time label.
fn format_time_label(ts: f64) -> String {
    let dt = DateTime::<Utc>::from_timestamp(ts as i64, 0);
    dt.map(|d| {
        let local = d.with_timezone(&chrono::Local);
        local.format("%H:%M:%S").to_string()
    })
    .unwrap_or_else(|| format!("{}", ts))
}

/// Format a series label set compactly: `{key="val", ...}`.
fn format_series_label(metric: &std::collections::BTreeMap<String, String>) -> String {
    if metric.is_empty() {
        return "{}".to_string();
    }
    let parts: Vec<String> = metric
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, v))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Print metrics as JSON (raw API response-like).
fn print_json(series: &[MetricSeries]) {
    let result: Vec<serde_json::Value> = series
        .iter()
        .map(|s| {
            let values: Vec<serde_json::Value> = s
                .points
                .iter()
                .map(|(dt, v)| {
                    serde_json::json!([dt.timestamp(), format!("{}", v)])
                })
                .collect();
            serde_json::json!({
                "metric": s.metric,
                "values": values,
            })
        })
        .collect();
    let output = serde_json::json!({
        "status": "success",
        "data": {
            "resultType": "matrix",
            "result": result,
        }
    });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

/// Print metrics as a table.
fn print_table(series: &[MetricSeries]) {
    if series.is_empty() {
        println!("no data");
        return;
    }

    // Collect all unique timestamps across all series.
    let mut timestamps: Vec<DateTime<Utc>> = series
        .iter()
        .flat_map(|s| s.points.iter().map(|(dt, _)| *dt))
        .collect();
    timestamps.sort();
    timestamps.dedup();

    // Header.
    let mut header = format!("{:<20}", "TIME");
    for (i, s) in series.iter().enumerate() {
        let label = format_series_label(&s.metric);
        let label = if label.len() > 30 {
            format!("{}...", &label[..27])
        } else {
            label
        };
        header.push_str(&format!("  {:>15}", if i == 0 { label } else { label }));
    }
    println!("{}", header);
    println!("{}", "-".repeat(header.len()));

    // Rows.
    for ts in &timestamps {
        let local = ts.with_timezone(&chrono::Local);
        let mut row = format!("{:<20}", local.format("%Y-%m-%d %H:%M:%S").to_string());
        for s in series {
            let val = s
                .points
                .iter()
                .find(|(dt, _)| dt == ts)
                .map(|(_, v)| format!("{:.4}", v))
                .unwrap_or_else(|| "-".to_string());
            row.push_str(&format!("  {:>15}", val));
        }
        println!("{}", row);
    }
}

/// Resolve the time range from --since/--from/--to flags.
fn resolve_time_range(
    since: &str,
    from: &Option<String>,
    to: &Option<String>,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let end = match to {
        Some(t) => parse_rfc3339(t)?,
        None => Utc::now(),
    };

    let start = match from {
        Some(f) => parse_rfc3339(f)?,
        None => {
            let dur = humantime::parse_duration(since)
                .with_context(|| format!("invalid --since duration '{}'", since))?;
            end - ChronoDuration::from_std(dur)?
        }
    };

    Ok((start, end))
}

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("invalid timestamp '{}': expected RFC3339 format", s))
}