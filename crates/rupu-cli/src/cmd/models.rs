//! `rupu models list | refresh`.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use std::process::ExitCode;

use clap::Subcommand;
use serde::Serialize;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List available models (custom + cached + baked-in).
    List {
        /// Filter output to a single provider.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Re-fetch live model lists from each provider.
    Refresh {
        /// Limit refresh to a single provider.
        #[arg(long)]
        provider: Option<String>,
    },
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    match action {
        Action::List { provider } => match list(provider, global_format).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rupu models list: {e}");
                ExitCode::FAILURE
            }
        },
        Action::Refresh { provider } => match refresh(provider).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rupu models refresh: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("models list", report::TABLE_JSON_CSV),
        Action::Refresh { .. } => ("models refresh", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

/// Path of the global `config.toml` these subcommands read.
fn global_config_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::paths::global_dir()?.join("config.toml"))
}

/// Load the global `config.toml`, or an empty config if there isn't one.
///
/// Deliberately global-only and raw `toml::from_str` — `rupu models` has
/// never layered a project-level config on top and this change does not
/// start.
fn load_global_config() -> anyhow::Result<rupu_config::Config> {
    let cfg_path = global_config_path()?;
    if !cfg_path.exists() {
        return Ok(rupu_config::Config::default());
    }
    let text = std::fs::read_to_string(&cfg_path)?;
    Ok(toml::from_str(&text)?)
}

#[derive(Serialize)]
struct ModelListRow {
    provider: String,
    model: String,
    source: String,
    context: Option<u64>,
    output: Option<u64>,
    fetched_at: Option<String>,
}

#[derive(Serialize)]
struct ModelListCsvRow {
    provider: String,
    model: String,
    source: String,
    context: String,
    output: String,
    fetched_at: String,
}

#[derive(Serialize)]
struct ModelListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<ModelListRow>,
}

struct ModelListOutput {
    report: ModelListReport,
    csv_rows: Vec<ModelListCsvRow>,
}

impl CollectionOutput for ModelListOutput {
    type JsonReport = ModelListReport;
    type CsvRow = ModelListCsvRow;

    fn command_name(&self) -> &'static str {
        "models list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "provider",
            "model",
            "source",
            "context",
            "output",
            "fetched_at",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "PROVIDER", "MODEL", "SOURCE", "CONTEXT", "OUTPUT", "FETCHED",
        ]);
        let limit_cell = |limit: Option<u64>| {
            comfy_table::Cell::new(
                limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            )
        };
        for row in &self.report.rows {
            table.add_row(vec![
                comfy_table::Cell::new(&row.provider),
                comfy_table::Cell::new(&row.model),
                comfy_table::Cell::new(&row.source),
                limit_cell(row.context),
                limit_cell(row.output),
                comfy_table::Cell::new(fetched_age(row.fetched_at.as_deref())),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

/// The FETCHED cell: how long ago the live list was fetched, or `-` when the
/// row did not come from a live fetch (or the timestamp is unreadable).
fn fetched_age(fetched_at: Option<&str>) -> String {
    fetched_at
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| {
            rupu_providers::model_limits::fmt_age(
                chrono::Utc::now() - t.with_timezone(&chrono::Utc),
            )
        })
        .unwrap_or_else(|| "-".to_string())
}

async fn list(filter: Option<String>, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let cfg = load_global_config()?;
    let global = crate::paths::global_dir()?;
    let cats = rupu_runtime::model_limits::catalog(
        &cfg,
        &global,
        &global_config_path()?,
        filter.as_deref(),
    )
    .await?;
    let mut rows = Vec::new();
    for p in cats {
        for m in p.models {
            // The provider's fetch time describes the live list only; a
            // config-declared or baked-in row was never fetched.
            let fetched_at = if m.source == "live" {
                p.fetched_at
            } else {
                None
            };
            rows.push(ModelListRow {
                provider: p.provider.clone(),
                model: m.id,
                source: m.source,
                context: m.input_tokens.map(u64::from),
                output: m.output_tokens.map(u64::from),
                fetched_at: fetched_at.map(|t| t.to_rfc3339()),
            });
        }
    }
    let csv_rows: Vec<ModelListCsvRow> = rows
        .iter()
        .map(|row| ModelListCsvRow {
            provider: row.provider.clone(),
            model: row.model.clone(),
            source: row.source.clone(),
            context: row
                .context
                .map(|value| value.to_string())
                .unwrap_or_default(),
            output: row
                .output
                .map(|value| value.to_string())
                .unwrap_or_default(),
            fetched_at: row.fetched_at.clone().unwrap_or_default(),
        })
        .collect();
    let output = ModelListOutput {
        report: ModelListReport {
            kind: "model_list",
            version: 2,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

async fn refresh(filter: Option<String>) -> anyhow::Result<()> {
    let cfg = load_global_config()?;
    let global = crate::paths::global_dir()?;
    // `resolver_for` rather than `KeychainResolver::new()`: a bare resolver
    // knows no declared accounts, so a named account's SSO credential would
    // be read but never refreshed on near-expiry (the defect
    // `crate::accounts::account_specs` exists to prevent).
    let resolver = crate::accounts::resolver_for(&cfg);
    let outcomes = rupu_runtime::model_limits::refresh(
        &cfg,
        &global,
        &global_config_path()?,
        &resolver,
        filter.as_deref(),
    )
    .await?;
    for o in outcomes {
        match (o.ok, o.count, o.error) {
            // The fetch succeeded but the listing was empty. A failing
            // fetch surfaces as `skip …` below, so this is a provider that
            // genuinely answered with nothing (or one whose client swallows
            // a body it cannot parse); `RUST_LOG=warn` surfaces that client's
            // tracing logs.
            (true, 0, _) => eprintln!(
                "rupu: refreshed {} (0 models — re-run with `RUST_LOG=warn` to see why)",
                o.provider
            ),
            (true, n, _) => println!("rupu: refreshed {} ({n} models)", o.provider),
            (false, _, Some(e)) => eprintln!("rupu: skip {}: {e}", o.provider),
            (false, _, None) => eprintln!("rupu: skip {}", o.provider),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetched_age_is_a_dash_when_there_is_no_timestamp() {
        assert_eq!(fetched_age(None), "-");
        assert_eq!(fetched_age(Some("not a timestamp")), "-");
    }

    #[test]
    fn fetched_age_renders_the_relative_age() {
        let now = chrono::Utc::now();
        assert_eq!(fetched_age(Some(&now.to_rfc3339())), "just now");
        // 2h and a minute: well inside the 2h bucket whatever the test's own
        // scheduling jitter.
        let two_hours = now - chrono::Duration::minutes(121);
        assert_eq!(fetched_age(Some(&two_hours.to_rfc3339())), "2h ago");
    }
}
