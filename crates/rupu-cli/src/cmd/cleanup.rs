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
    /// Show cleanup inventory instead of deleting archived resources.
    #[arg(long, conflicts_with_all = ["older_than", "dry_run"])]
    pub stats: bool,
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

#[derive(Serialize)]
struct CleanupStatsRow {
    kind: String,
    scope: String,
    count: u64,
    bytes: u64,
    oldest: Option<String>,
    newest: Option<String>,
}

#[derive(Serialize)]
struct CleanupStatsCsvRow {
    kind: String,
    scope: String,
    count: u64,
    bytes: u64,
    oldest: String,
    newest: String,
}

#[derive(Serialize)]
struct CleanupStatsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<CleanupStatsRow>,
}

struct CleanupStatsOutput {
    report: CleanupStatsReport,
    csv_rows: Vec<CleanupStatsCsvRow>,
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

impl CollectionOutput for CleanupStatsOutput {
    type JsonReport = CleanupStatsReport;
    type CsvRow = CleanupStatsCsvRow;

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
        Some(&["kind", "scope", "count", "bytes", "oldest", "newest"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["KIND", "SCOPE", "COUNT", "BYTES", "OLDEST", "NEWEST"]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.kind),
                Cell::new(&row.scope),
                Cell::new(row.count.to_string()),
                Cell::new(row.bytes.to_string()),
                Cell::new(row.oldest.as_deref().unwrap_or("—")),
                Cell::new(row.newest.as_deref().unwrap_or("—")),
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
    if args.stats {
        return render_stats(args, global_format).await;
    }
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

async fn render_stats(args: Args, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let include_sessions = args.sessions || !args.transcripts;
    let include_transcripts = args.transcripts || !args.sessions;
    let global = crate::paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = crate::paths::project_root_for(&pwd)?;

    let mut rows = Vec::new();
    if include_sessions {
        rows.push(scan_session_stats_row(
            "session",
            "active",
            &crate::paths::sessions_dir(&global),
        )?);
        rows.push(scan_session_stats_row(
            "session",
            "archived",
            &crate::paths::archived_sessions_dir(&global),
        )?);
    }
    if include_transcripts {
        rows.push(scan_transcript_stats_row(
            "transcript",
            "global_active",
            &global.join("transcripts"),
        )?);
        rows.push(scan_transcript_stats_row(
            "transcript",
            "global_archived",
            &crate::paths::archived_transcripts_dir(&global.join("transcripts")),
        )?);
        if let Some(project_root) = project_root {
            let project_transcripts = project_root.join(".rupu/transcripts");
            rows.push(scan_transcript_stats_row(
                "transcript",
                "project_active",
                &project_transcripts,
            )?);
            rows.push(scan_transcript_stats_row(
                "transcript",
                "project_archived",
                &crate::paths::archived_transcripts_dir(&project_transcripts),
            )?);
        }
    }
    rows.retain(|row| row.count > 0 || row.bytes > 0);
    rows.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.scope.cmp(&b.scope)));
    let csv_rows = rows
        .iter()
        .map(|row| CleanupStatsCsvRow {
            kind: row.kind.clone(),
            scope: row.scope.clone(),
            count: row.count,
            bytes: row.bytes,
            oldest: row.oldest.clone().unwrap_or_default(),
            newest: row.newest.clone().unwrap_or_default(),
        })
        .collect();
    let output = CleanupStatsOutput {
        report: CleanupStatsReport {
            kind: "cleanup_stats",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

fn scan_session_stats_row(
    kind: &str,
    scope: &str,
    dir: &std::path::Path,
) -> anyhow::Result<CleanupStatsRow> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    let mut oldest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut newest: Option<chrono::DateTime<chrono::Utc>> = None;
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let session_json = path.join("session.json");
            if !session_json.is_file() {
                continue;
            }
            count += 1;
            bytes += dir_size_bytes(&path)?;
            let metadata = std::fs::metadata(&session_json)?;
            if let Ok(modified) = metadata.modified() {
                let ts = chrono::DateTime::<chrono::Utc>::from(modified);
                oldest = Some(oldest.map_or(ts, |current| current.min(ts)));
                newest = Some(newest.map_or(ts, |current| current.max(ts)));
            }
        }
    }
    Ok(CleanupStatsRow {
        kind: kind.to_string(),
        scope: scope.to_string(),
        count,
        bytes,
        oldest: oldest.map(|value| value.to_rfc3339()),
        newest: newest.map(|value| value.to_rfc3339()),
    })
}

fn dir_size_bytes(dir: &std::path::Path) -> anyhow::Result<u64> {
    let mut total = 0u64;
    if !dir.is_dir() {
        return Ok(total);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            total += dir_size_bytes(&path)?;
        } else if file_type.is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

fn scan_transcript_stats_row(
    kind: &str,
    scope: &str,
    dir: &std::path::Path,
) -> anyhow::Result<CleanupStatsRow> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    let mut oldest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut newest: Option<chrono::DateTime<chrono::Utc>> = None;
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            count += 1;
            let metadata = std::fs::metadata(&path)?;
            bytes += metadata.len();
            let sidecar = path.with_extension("json");
            if sidecar.is_file() {
                bytes += std::fs::metadata(&sidecar)?.len();
            }
            if let Ok(modified) = metadata.modified() {
                let ts = chrono::DateTime::<chrono::Utc>::from(modified);
                oldest = Some(oldest.map_or(ts, |current| current.min(ts)));
                newest = Some(newest.map_or(ts, |current| current.max(ts)));
            }
        }
    }
    Ok(CleanupStatsRow {
        kind: kind.to_string(),
        scope: scope.to_string(),
        count,
        bytes,
        oldest: oldest.map(|value| value.to_rfc3339()),
        newest: newest.map(|value| value.to_rfc3339()),
    })
}
