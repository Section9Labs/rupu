//! `rupu usage`. Walks transcripts (project + global) and prints
//! aggregated token spend keyed by `(provider, model, agent)`.
//!
//! Heavy lifting lives in [`rupu_transcript::aggregate`]; this
//! module is a thin clap dispatcher that collects paths, parses
//! the optional `--since` / `--until` flags, and renders a table
//! (or JSON when `--format json`).

use crate::paths;
use chrono::{DateTime, Utc};
use clap::Args;
use rupu_transcript::{aggregate, TimeWindow, UsageRow};
use std::path::PathBuf;
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
    /// Output format. `table` (default) prints a fixed-width table;
    /// `json` prints one row per line as JSON for downstream
    /// scripting.
    #[arg(long, default_value = "table")]
    pub format: String,
}

pub async fn handle(args: UsageArgs) -> ExitCode {
    let result = run(args).await;
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e)
    }
}

async fn run(args: UsageArgs) -> anyhow::Result<()> {
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
    let mut paths_to_scan: Vec<PathBuf> = Vec::new();
    if let Some(ref proj) = project_root {
        collect_jsonl(&proj.join(".rupu/transcripts"), &mut paths_to_scan);
    }
    collect_jsonl(&global.join("transcripts"), &mut paths_to_scan);

    let rows = aggregate(&paths_to_scan, window);

    match args.format.as_str() {
        "json" => print_json(&rows)?,
        "table" => print_table(&rows),
        other => anyhow::bail!("unknown --format: {other} (expected `table` or `json`)"),
    }
    Ok(())
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

fn collect_jsonl(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

fn print_table(rows: &[UsageRow]) {
    if rows.is_empty() {
        println!("(no runs match — try `--since 30d` to widen the window)");
        return;
    }
    println!(
        "{:<14} {:<28} {:<28} {:>10} {:>10} {:>10} {:>6}",
        "PROVIDER", "MODEL", "AGENT", "INPUT", "OUTPUT", "CACHED", "RUNS"
    );
    let mut total_in = 0u64;
    let mut total_out = 0u64;
    let mut total_cached = 0u64;
    let mut total_runs = 0u64;
    for r in rows {
        println!(
            "{:<14} {:<28} {:<28} {:>10} {:>10} {:>10} {:>6}",
            r.provider, r.model, r.agent, r.input_tokens, r.output_tokens, r.cached_tokens, r.runs
        );
        total_in += r.input_tokens;
        total_out += r.output_tokens;
        total_cached += r.cached_tokens;
        total_runs += r.runs;
    }
    println!(
        "{:<14} {:<28} {:<28} {:>10} {:>10} {:>10} {:>6}",
        "TOTAL", "", "", total_in, total_out, total_cached, total_runs
    );
}

fn print_json(rows: &[UsageRow]) -> anyhow::Result<()> {
    for r in rows {
        let v = serde_json::json!({
            "provider": r.provider,
            "model": r.model,
            "agent": r.agent,
            "input_tokens": r.input_tokens,
            "output_tokens": r.output_tokens,
            "cached_tokens": r.cached_tokens,
            "runs": r.runs,
        });
        println!("{}", serde_json::to_string(&v)?);
    }
    Ok(())
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
