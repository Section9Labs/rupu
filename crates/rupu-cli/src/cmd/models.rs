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
        &rupu_runtime::model_limits::cache_dir(&global),
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
    let resolver = std::sync::Arc::new(crate::accounts::resolver_for(&cfg));
    let refreshed = run_refresh(
        &cfg,
        &rupu_runtime::model_limits::cache_dir(&global),
        &global_config_path()?,
        resolver,
        filter.as_deref(),
        RefreshTimings {
            fetch: rupu_runtime::model_limits::FETCH_TIMEOUT,
            unfinished_wait: UNFINISHED_JOB_WAIT,
        },
        &mut |line| match line {
            ReportLine::Out(l) => println!("{l}"),
            ReportLine::Err(l) => eprintln!("{l}"),
        },
    )
    .await?;
    if !refreshed {
        anyhow::bail!("no provider was refreshed");
    }
    Ok(())
}

/// The listing timeout, and how long to wait afterwards for provider jobs
/// that outlived it.
struct RefreshTimings {
    fetch: std::time::Duration,
    unfinished_wait: std::time::Duration,
}

/// Refresh, report each provider as it settles through `emit`, and say
/// whether anything was refreshed.
async fn run_refresh(
    cfg: &rupu_config::Config,
    cache_dir: &std::path::Path,
    cfg_path: &std::path::Path,
    resolver: std::sync::Arc<dyn rupu_auth::CredentialResolver>,
    filter: Option<&str>,
    timings: RefreshTimings,
    emit: &mut (dyn FnMut(ReportLine) + Send),
) -> anyhow::Result<bool> {
    let report = rupu_runtime::model_limits::refresh(
        cfg,
        cache_dir,
        cfg_path,
        resolver,
        filter,
        timings.fetch,
    )
    .await?;
    let (lines, mut any_refreshed) = refresh_report(report.outcomes);
    lines.into_iter().for_each(&mut *emit);
    // A timed-out provider job is still running — possibly mid-way through
    // an OAuth token refresh whose rotated token must be persisted. This
    // process's runtime would cancel it on exit, so give it time to finish,
    // and report how it ended: a job that finishes here did refresh.
    if !report.unfinished.is_empty() {
        any_refreshed |= settle_unfinished(report.unfinished, timings.unfinished_wait, emit).await;
    }
    Ok(any_refreshed)
}

/// Wait (at most `wait`, one deadline for all) for the provider jobs that
/// outlived the listing timeout, reporting each through `emit` as it lands;
/// at the deadline, names the ones still running. `true` when one of them
/// refreshed.
async fn settle_unfinished(
    unfinished: Vec<rupu_runtime::model_limits::UnfinishedRefresh>,
    wait: std::time::Duration,
    emit: &mut (dyn FnMut(ReportLine) + Send),
) -> bool {
    use futures_util::stream::{FuturesUnordered, StreamExt};
    emit(ReportLine::Err(format!(
        "rupu: waiting for {} provider job(s) to finish…",
        unfinished.len()
    )));
    let mut still_running: Vec<String> = unfinished.iter().map(|u| u.provider.clone()).collect();
    let mut landing: FuturesUnordered<_> = unfinished
        .into_iter()
        .map(|u| async move { (u.provider, u.job.await) })
        .collect();
    let deadline = tokio::time::Instant::now() + wait;
    let mut any_refreshed = false;
    loop {
        match tokio::time::timeout_at(deadline, landing.next()).await {
            Ok(Some((provider, finished))) => {
                still_running.retain(|p| *p != provider);
                match finished {
                    Ok(outcome) => {
                        any_refreshed |= outcome.ok;
                        emit(late_report_line(outcome));
                    }
                    Err(e) => emit(ReportLine::Err(format!(
                        "rupu: skip {provider}: the provider job failed after the wait: {e}"
                    ))),
                }
            }
            Ok(None) => break,
            Err(_) => {
                emit(ReportLine::Err(format!(
                    "rupu: gave up waiting after {}s; still running: {}",
                    wait.as_secs(),
                    still_running.join(", ")
                )));
                break;
            }
        }
    }
    any_refreshed
}

/// The line for a provider job that finished during the exit wait.
fn late_report_line(o: rupu_runtime::model_limits::RefreshOutcome) -> ReportLine {
    match (o.ok, o.error) {
        (true, _) => ReportLine::Out(format!(
            "rupu: refreshed {} ({} models) after the wait",
            o.provider, o.count
        )),
        (false, Some(e)) => {
            ReportLine::Err(format!("rupu: skip {}: {e} (after the wait)", o.provider))
        }
        (false, None) => ReportLine::Err(format!("rupu: skip {} (after the wait)", o.provider)),
    }
}

/// How long `rupu models refresh` waits, before exiting, for provider jobs
/// that outlived the listing timeout.
const UNFINISHED_JOB_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// One printed line of a `rupu models refresh` report.
#[derive(Debug, PartialEq, Eq)]
enum ReportLine {
    Out(String),
    Err(String),
}

/// One line per outcome, and whether the command succeeded: at least one
/// targeted provider refreshed. Every one failing used to exit 0, so a
/// scripted refresh could not tell "refreshed" from "nothing worked".
fn refresh_report(
    outcomes: Vec<rupu_runtime::model_limits::RefreshOutcome>,
) -> (Vec<ReportLine>, bool) {
    let any_refreshed = outcomes.iter().any(|o| o.ok);
    let lines = outcomes
        .into_iter()
        .map(|o| match (o.ok, o.count, o.error) {
            // The fetch succeeded but the listing was empty. A failing
            // fetch surfaces as `skip …` below, so this is a provider that
            // genuinely answered with nothing (or one whose client swallows
            // a body it cannot parse); `RUST_LOG=warn` surfaces that client's
            // tracing logs.
            (true, 0, _) => ReportLine::Err(format!(
                "rupu: refreshed {} (0 models — re-run with `RUST_LOG=warn` to see why)",
                o.provider
            )),
            (true, n, _) => ReportLine::Out(format!("rupu: refreshed {} ({n} models)", o.provider)),
            (false, _, Some(e)) => ReportLine::Err(format!("rupu: skip {}: {e}", o.provider)),
            (false, _, None) => ReportLine::Err(format!("rupu: skip {}", o.provider)),
        })
        .collect();
    (lines, any_refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(provider: &str, ok: bool) -> rupu_runtime::model_limits::RefreshOutcome {
        rupu_runtime::model_limits::RefreshOutcome {
            provider: provider.into(),
            ok,
            count: if ok { 3 } else { 0 },
            error: (!ok).then(|| "401 — not logged in".to_string()),
        }
    }

    /// One provider refreshed, one failed: the command succeeded at what it
    /// could do, and reports both.
    #[test]
    fn a_mixed_refresh_succeeds_and_reports_every_provider() {
        let (lines, ok) = refresh_report(vec![outcome("oracle", true), outcome("boxy", false)]);
        assert!(ok);
        assert_eq!(
            lines,
            vec![
                ReportLine::Out("rupu: refreshed oracle (3 models)".into()),
                ReportLine::Err("rupu: skip boxy: 401 — not logged in".into()),
            ]
        );
    }

    /// Every targeted provider failed: nothing was refreshed, so the command
    /// fails (it used to exit 0).
    #[test]
    fn a_refresh_where_every_provider_failed_fails() {
        let (lines, ok) = refresh_report(vec![outcome("oracle", false), outcome("boxy", false)]);
        assert!(!ok);
        assert_eq!(lines.len(), 2, "every failure is still reported");
    }

    /// Hands every account the same API key; the mock never checks it.
    struct AnyKey;

    #[async_trait::async_trait]
    impl rupu_auth::CredentialResolver for AnyKey {
        async fn get(
            &self,
            _provider: &str,
            _hint: Option<rupu_providers::AuthMode>,
        ) -> anyhow::Result<(
            rupu_providers::AuthMode,
            rupu_providers::auth::AuthCredentials,
        )> {
            Ok((
                rupu_providers::AuthMode::ApiKey,
                rupu_providers::auth::AuthCredentials::ApiKey { key: "k".into() },
            ))
        }
        async fn refresh(
            &self,
            _provider: &str,
            _mode: rupu_providers::AuthMode,
        ) -> anyhow::Result<rupu_providers::auth::AuthCredentials> {
            unreachable!()
        }
    }

    fn late_job(
        provider: &str,
        after: std::time::Duration,
    ) -> rupu_runtime::model_limits::UnfinishedRefresh {
        let name = provider.to_string();
        rupu_runtime::model_limits::UnfinishedRefresh {
            provider: name.clone(),
            job: tokio::spawn(async move {
                tokio::time::sleep(after).await;
                rupu_runtime::model_limits::RefreshOutcome {
                    provider: name,
                    ok: true,
                    count: 3,
                    error: None,
                }
            }),
        }
    }

    /// The wait is per job under one deadline, not all-or-nothing: a job
    /// that lands in time is reported (and counts toward success) even when
    /// another is still running at the deadline, which is named.
    #[tokio::test]
    async fn the_wait_reports_each_job_as_it_lands_and_names_the_stragglers() {
        let mut lines = Vec::new();
        let refreshed = settle_unfinished(
            vec![
                late_job("fast", std::time::Duration::from_millis(300)),
                late_job("stuck", std::time::Duration::from_secs(3600)),
            ],
            std::time::Duration::from_secs(1),
            &mut |line| lines.push(line),
        )
        .await;
        assert!(refreshed, "{lines:?}");
        assert!(
            lines.contains(&ReportLine::Out(
                "rupu: refreshed fast (3 models) after the wait".into()
            )),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| matches!(l, ReportLine::Err(m)
                if m.contains("gave up waiting") && m.contains("still running: stuck"))),
            "{lines:?}"
        );
    }

    /// A provider job that misses the listing timeout but finishes within
    /// the exit wait DID refresh: its late outcome is reported and counts
    /// toward success (it used to be discarded, and the command exited
    /// non-zero after a refresh that worked).
    #[tokio::test]
    async fn a_job_that_finishes_during_the_wait_counts_as_refreshed() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .delay(std::time::Duration::from_millis(300))
                .json_body(serde_json::json!({
                    "data": [{ "id": "box-model", "max_model_len": 4096 }]
                }));
        });
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = rupu_config::Config::default();
        cfg.providers.insert(
            "boxy".into(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some(format!("{}/v1", server.url(""))),
                default_model: Some("box-model".into()),
                ..Default::default()
            },
        );
        let mut lines = Vec::new();
        let refreshed = run_refresh(
            &cfg,
            &tmp.path().join("cache"),
            &tmp.path().join("config.toml"),
            std::sync::Arc::new(AnyKey),
            Some("boxy"),
            RefreshTimings {
                fetch: std::time::Duration::from_millis(100),
                unfinished_wait: std::time::Duration::from_secs(5),
            },
            &mut |line| lines.push(line),
        )
        .await
        .unwrap();
        assert!(refreshed, "the late refresh succeeded: {lines:?}");
        assert!(
            lines.contains(&ReportLine::Out(
                "rupu: refreshed boxy (1 models) after the wait".into()
            )),
            "{lines:?}"
        );
    }

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
