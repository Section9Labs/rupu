//! `rupu transcript list | show`.
//!
//! `list` scans `<project>/.rupu/transcripts/*.jsonl` and
//! `<global>/transcripts/*.jsonl`, summarises each file via
//! [`rupu_transcript::JsonlReader::summary`], and prints a table sorted
//! newest-first by `started_at`.
//!
//! `show <run_id>` finds `<run_id>.jsonl` in either transcripts directory
//! and pretty-prints each event as JSON.

use crate::paths;
use clap::Subcommand;
use rupu_transcript::{JsonlReader, RunStatus};
use std::cmp::Reverse;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all transcripts (project-local + global) sorted newest first.
    List,
    /// Print a transcript's full event stream.
    Show { run_id: String },
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::List => list().await,
        Action::Show { run_id } => show(&run_id).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn list() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let mut paths_to_scan: Vec<PathBuf> = Vec::new();

    // Collect .jsonl paths from a directory — miss is a silent skip.
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

    if let Some(ref proj) = project_root {
        collect_jsonl(&proj.join(".rupu/transcripts"), &mut paths_to_scan);
    }
    collect_jsonl(&global.join("transcripts"), &mut paths_to_scan);

    // Build summaries, skipping files that error.
    struct Row {
        run_id: String,
        agent: String,
        status: RunStatus,
        total_tokens: u64,
        started_at: chrono::DateTime<chrono::Utc>,
    }

    let mut rows: Vec<Row> = Vec::new();
    for path in &paths_to_scan {
        match JsonlReader::summary(path) {
            Ok(s) => rows.push(Row {
                run_id: s.run_id,
                agent: s.agent,
                status: s.status,
                total_tokens: s.total_tokens,
                started_at: s.started_at,
            }),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable transcript");
            }
        }
    }

    // Sort newest first.
    rows.sort_by_key(|r| Reverse(r.started_at));

    println!(
        "{:<36} {:<22} {:<10} {:<8} STARTED_AT",
        "RUN_ID", "AGENT", "STATUS", "TOKENS"
    );
    for row in &rows {
        let status_str = match row.status {
            RunStatus::Ok => "ok",
            RunStatus::Error => "error",
            RunStatus::Aborted => "aborted",
        };
        println!(
            "{:<36} {:<22} {:<10} {:<8} {}",
            row.run_id,
            row.agent,
            status_str,
            row.total_tokens,
            row.started_at.format("%Y-%m-%dT%H:%M:%SZ"),
        );
    }
    Ok(())
}

async fn show(run_id: &str) -> anyhow::Result<()> {
    let path = locate_transcript(run_id)?;
    for event in JsonlReader::iter(&path)? {
        let event = event?;
        let pretty = serde_json::to_string_pretty(&event)?;
        println!("{pretty}");
    }
    Ok(())
}

fn locate_transcript(run_id: &str) -> anyhow::Result<PathBuf> {
    let filename = format!("{run_id}.jsonl");

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Project-local first.
    if let Some(ref proj) = project_root {
        let candidate = proj.join(".rupu/transcripts").join(&filename);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // Global fallback.
    let candidate = global.join("transcripts").join(&filename);
    if candidate.is_file() {
        return Ok(candidate);
    }

    Err(anyhow::anyhow!("transcript not found: {run_id}"))
}
