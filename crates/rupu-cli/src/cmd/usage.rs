//! `rupu usage`. Walks transcripts (project + global) and prints
//! aggregated token spend keyed by `(provider, model, agent)`.
//!
//! Heavy lifting lives in [`rupu_transcript::aggregate`]; this
//! module is a thin clap dispatcher that collects paths, parses
//! the optional `--since` / `--until` flags, and renders a table
//! (or JSON when `--format json`).

use crate::cmd::usage_report::UsageDataset;
use crate::paths;
use chrono::{DateTime, Utc};
use clap::Args;
use comfy_table::Cell;
use rupu_transcript::{TimeWindow, UsageRow};
use serde::Serialize;
use std::process::ExitCode;

#[derive(Args, Debug)]
pub struct UsageArgs {
    /// Only count runs whose `started_at` is at or after this
    /// timestamp. RFC-3339 / ISO-8601 (`2026-05-01T00:00:00Z` or a
    /// relative form like `7d`, `24h`, `30m`).
    #[arg(long)]
    pub since: Option<String>,
    /// Only count runs whose `started_at` is at or before this
    /// timestamp. Same syntax as `--since`.
    #[arg(long)]
    pub until: Option<String>,
}

pub async fn handle(
    args: UsageArgs,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> ExitCode {
    let result = run(args, global_format).await;
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn run(
    args: UsageArgs,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let since = args
        .since
        .as_deref()
        .map(parse_time_arg)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--since: {e}"))?;
    let until = args
        .until
        .as_deref()
        .map(parse_time_arg)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--until: {e}"))?;
    let window = TimeWindow { since, until };

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Layered config supplies pricing overrides + UI prefs for the
    // colored cost cells. Failing to load the config files isn't
    // fatal — a default `Config` still picks up the built-in price
    // table and renders with no color overrides.
    let cfg = layered_config(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None);
    let dataset = UsageDataset::load(&global, project_root.as_deref(), window)?;
    let rows = dataset.composite_rows();
    let format =
        crate::output::formats::resolve(global_format, crate::output::formats::OutputFormat::Table);
    crate::output::formats::ensure_supported(
        "rupu usage",
        format,
        &[
            crate::output::formats::OutputFormat::Table,
            crate::output::formats::OutputFormat::Json,
            crate::output::formats::OutputFormat::Csv,
        ],
    )?;

    match format {
        crate::output::formats::OutputFormat::Table => print_table(&rows, &cfg.pricing, &prefs),
        crate::output::formats::OutputFormat::Json => print_json(&rows, &cfg.pricing)?,
        crate::output::formats::OutputFormat::Csv => print_csv(&rows, &cfg.pricing)?,
    }
    Ok(())
}

fn layered_config(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
) -> rupu_config::Config {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

/// Accept either a full RFC-3339 timestamp (`2026-05-01T00:00:00Z`)
/// or a relative shorthand (`7d`, `24h`, `30m`, `90s`). Relative
/// forms are interpreted as "now minus that duration" — useful for
/// `--since 7d`. Bare numbers (no unit) are rejected.
fn parse_time_arg(s: &str) -> Result<DateTime<Utc>, String> {
    let s = s.trim();
    // Full RFC-3339 first.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Relative shorthand.
    let (num_part, unit) = s.split_at(
        s.char_indices()
            .find(|(_, c)| c.is_alphabetic())
            .map(|(i, _)| i)
            .ok_or_else(|| {
                format!("`{s}` is not RFC-3339 and has no unit (try `7d` / `24h` / `30m`)")
            })?,
    );
    if num_part.is_empty() {
        return Err(format!(
            "`{s}` is missing a number before the unit (try `7d` / `24h` / `30m`)"
        ));
    }
    let n: i64 = num_part
        .parse()
        .map_err(|e| format!("invalid number `{num_part}`: {e}"))?;
    let dur = match unit {
        "s" => chrono::Duration::seconds(n),
        "m" => chrono::Duration::minutes(n),
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        "w" => chrono::Duration::weeks(n),
        other => return Err(format!("unknown unit `{other}` (expected s/m/h/d/w)")),
    };
    Ok(Utc::now() - dur)
}

fn print_table(
    rows: &[UsageRow],
    pricing: &rupu_config::PricingConfig,
    prefs: &crate::cmd::ui::UiPrefs,
) {
    if rows.is_empty() {
        println!("(no runs match — try `--since 30d` to widen the window)");
        return;
    }
    let mut table = crate::output::tables::new_table();
    table.set_header(vec![
        "PROVIDER",
        "MODEL",
        "AGENT",
        "INPUT",
        "OUTPUT",
        "CACHED",
        "RUNS",
        "COST (USD)",
    ]);
    let mut total_in = 0u64;
    let mut total_out = 0u64;
    let mut total_cached = 0u64;
    let mut total_runs = 0u64;
    let mut total_cost = 0.0f64;
    let mut any_priced = false;
    for row in rows {
        let cost = crate::pricing::lookup(pricing, &row.provider, &row.model, &row.agent)
            .map(|price| price.cost_usd(row.input_tokens, row.output_tokens, row.cached_tokens));
        if let Some(value) = cost {
            total_cost += value;
            any_priced = true;
        }
        table.add_row(vec![
            Cell::new(&row.provider),
            Cell::new(&row.model),
            Cell::new(&row.agent),
            Cell::new(format_count(row.input_tokens)),
            Cell::new(format_count(row.output_tokens)),
            Cell::new(format_count(row.cached_tokens)),
            Cell::new(row.runs.to_string()),
            cost_cell(cost, prefs),
        ]);
        total_in += row.input_tokens;
        total_out += row.output_tokens;
        total_cached += row.cached_tokens;
        total_runs += row.runs;
    }
    table.add_row(vec![
        Cell::new("TOTAL"),
        Cell::new(""),
        Cell::new(""),
        Cell::new(format_count(total_in)),
        Cell::new(format_count(total_out)),
        Cell::new(format_count(total_cached)),
        Cell::new(total_runs.to_string()),
        cost_cell(if any_priced { Some(total_cost) } else { None }, prefs),
    ]);
    println!("{table}");
    if !any_priced {
        println!(
            "(no pricing data — add `[pricing.<provider>.\"<model>\"]` or \
             `[pricing.agents.<agent>]` to your config.toml to enable cost)",
        );
    }
}

fn print_json(rows: &[UsageRow], pricing: &rupu_config::PricingConfig) -> anyhow::Result<()> {
    let rows = rows
        .iter()
        .map(|row| UsageRowOutput::from_usage_row(row, pricing))
        .collect::<Vec<_>>();
    crate::output::formats::print_json(&rows)
}

fn print_csv(rows: &[UsageRow], pricing: &rupu_config::PricingConfig) -> anyhow::Result<()> {
    let rows = rows
        .iter()
        .map(|row| UsageRowOutput::from_usage_row(row, pricing))
        .collect::<Vec<_>>();
    crate::output::formats::print_csv_rows(&rows)
}

/// Render a cost cell as `$1.2345` with 4 decimals (sub-cent visible
/// for cheap calls), or a dim em-dash placeholder when no price is
/// known. Sized for the COST column so the table stays compact.
fn cost_cell(cost_usd: Option<f64>, prefs: &crate::cmd::ui::UiPrefs) -> Cell {
    match cost_usd {
        Some(c) => Cell::new(format!("${c:.4}")),
        None => {
            if prefs.use_color() {
                Cell::new("\x1b[2m—\x1b[0m")
            } else {
                Cell::new("—")
            }
        }
    }
}

/// Format a token count with thousands separators (`1,234,567`). Keeps
/// the INPUT / OUTPUT / CACHED columns readable when transcripts span
/// millions of tokens.
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

#[derive(Debug, Clone, Serialize)]
struct UsageRowOutput {
    provider: String,
    model: String,
    agent: String,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    runs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
}

impl UsageRowOutput {
    fn from_usage_row(row: &UsageRow, pricing: &rupu_config::PricingConfig) -> Self {
        let cost_usd = crate::pricing::lookup(pricing, &row.provider, &row.model, &row.agent)
            .map(|p| p.cost_usd(row.input_tokens, row.output_tokens, row.cached_tokens));
        Self {
            provider: row.provider.clone(),
            model: row.model.clone(),
            agent: row.agent.clone(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_tokens: row.cached_tokens,
            runs: row.runs,
            cost_usd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_arg_accepts_rfc3339() {
        let ts = parse_time_arg("2026-05-01T00:00:00Z").unwrap();
        assert_eq!(ts.timestamp(), 1777593600);
    }

    #[test]
    fn parse_time_arg_accepts_relative_shorthand() {
        // "1h" should land within a few seconds of "now minus 1 hour".
        let ts = parse_time_arg("1h").unwrap();
        let expected = Utc::now() - chrono::Duration::hours(1);
        let drift = (ts - expected).num_seconds().abs();
        assert!(drift < 5, "drift too large: {drift}s");
    }

    #[test]
    fn parse_time_arg_rejects_bare_number() {
        assert!(parse_time_arg("7").is_err());
    }

    #[test]
    fn parse_time_arg_rejects_unknown_unit() {
        assert!(parse_time_arg("7y").is_err());
    }
}
