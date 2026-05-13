use crate::cmd::session::prune_archived_sessions;
use crate::cmd::transcript::prune_archived_transcripts;
use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use anyhow::Context;
use clap::Args as ClapArgs;
use comfy_table::Cell;
use serde::Serialize;

#[derive(ClapArgs, Debug, Clone)]
pub struct Args {
    /// Retention cutoff override for every selected resource, e.g. `30d`, `12h`, or `1w`.
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Preview deletions without removing files.
    #[arg(long)]
    pub dry_run: bool,
    /// Only clean archived sessions.
    #[arg(long, conflicts_with = "transcripts")]
    pub sessions: bool,
    /// Only clean archived standalone transcripts.
    #[arg(long, conflicts_with = "sessions")]
    pub transcripts: bool,
}

#[derive(Serialize)]
struct CleanupRow {
    kind: String,
    id: String,
    scope: String,
    retained_at: String,
    action: String,
    detail: String,
}

#[derive(Serialize)]
struct CleanupCsvRow {
    kind: String,
    id: String,
    scope: String,
    retained_at: String,
    action: String,
    detail: String,
}

#[derive(Serialize)]
struct CleanupReport {
    kind: &'static str,
    version: u8,
    rows: Vec<CleanupRow>,
}

struct CleanupOutput {
    report: CleanupReport,
    csv_rows: Vec<CleanupCsvRow>,
}

impl CollectionOutput for CleanupOutput {
    type JsonReport = CleanupReport;
    type CsvRow = CleanupCsvRow;

    fn command_name(&self) -> &'static str {
        "cleanup"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["kind", "id", "scope", "retained_at", "action", "detail"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["KIND", "ID", "SCOPE", "RETAINED", "ACTION", "DETAIL"]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.kind),
                Cell::new(&row.id),
                Cell::new(&row.scope),
                Cell::new(&row.retained_at),
                Cell::new(&row.action),
                Cell::new(&row.detail),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

pub async fn handle(args: Args, global_format: Option<OutputFormat>) -> std::process::ExitCode {
    match run(args, global_format).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[error] {error:#}");
            std::process::ExitCode::from(1)
        }
    }
}

pub fn ensure_output_format(format: OutputFormat) -> anyhow::Result<()> {
    crate::output::formats::ensure_supported("cleanup", format, report::TABLE_JSON_CSV)
}

async fn run(args: Args, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let include_sessions = args.sessions || !args.transcripts;
    let include_transcripts = args.transcripts || !args.sessions;

    let mut rows = Vec::new();
    if include_sessions {
        for session in prune_archived_sessions(args.older_than.as_deref(), args.dry_run)
            .context("cleanup archived sessions")?
        {
            rows.push(CleanupRow {
                kind: "session".into(),
                id: session.session_id,
                scope: session.scope,
                retained_at: session.updated_at,
                action: session.action,
                detail: format!("status {}", session.status),
            });
        }
    }
    if include_transcripts {
        for transcript in prune_archived_transcripts(args.older_than.as_deref(), args.dry_run)
            .context("cleanup archived transcripts")?
        {
            rows.push(CleanupRow {
                kind: "transcript".into(),
                id: transcript.run_id,
                scope: transcript.scope,
                retained_at: transcript.archived_at,
                action: transcript.action,
                detail: transcript.location,
            });
        }
    }
    rows.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.scope.cmp(&b.scope))
    });
    let csv_rows = rows
        .iter()
        .map(|row| CleanupCsvRow {
            kind: row.kind.clone(),
            id: row.id.clone(),
            scope: row.scope.clone(),
            retained_at: row.retained_at.clone(),
            action: row.action.clone(),
            detail: row.detail.clone(),
        })
        .collect();
    let output = CleanupOutput {
        report: CleanupReport {
            kind: "cleanup",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}
