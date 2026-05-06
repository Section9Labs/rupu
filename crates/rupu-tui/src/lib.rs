//! Live + replay TUI for rupu runs. Consumes `rupu-transcript` JSONL
//! and `rupu-orchestrator` RunRecords; renders a DAG canvas using
//! ratatui. See `docs/superpowers/specs/2026-05-05-rupu-slice-c-tui-design.md`.

#![doc(html_root_url = "https://docs.rs/rupu-tui")]

pub mod app;
pub mod control;
pub mod err;
pub mod source;
pub mod state;
pub mod view;
pub use err::{TuiError, TuiResult};

use rupu_orchestrator::RunStore;
use std::path::PathBuf;

pub fn run_watch(run_id: String, runs_dir: PathBuf) -> TuiResult<()> {
    let store = RunStore::new(runs_dir.clone());
    let _record = store.load(&run_id).map_err(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(_) => {
            TuiError::RunNotFound(run_id.clone(), runs_dir.clone())
        }
        other => TuiError::Orchestrator(other),
    })?;
    let workflow = match store.read_workflow_snapshot(&run_id) {
        Ok(s) => rupu_orchestrator::Workflow::parse(&s).ok(),
        Err(_) => None,
    };
    let source = Box::new(source::JsonlTailSource::new(runs_dir.join(&run_id))?);
    app::App::new(run_id, source, store, workflow).run()
}

pub fn run_attached(run_id: String, runs_dir: PathBuf) -> TuiResult<()> {
    run_watch(run_id, runs_dir)
}

pub fn run_replay(run_id: String, runs_dir: PathBuf, pace_us: u64) -> TuiResult<()> {
    let store = RunStore::new(runs_dir.clone());
    // Validate the run exists before proceeding.
    store.load(&run_id).map_err(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(_) => {
            TuiError::RunNotFound(run_id.clone(), runs_dir.clone())
        }
        other => TuiError::Orchestrator(other),
    })?;
    let workflow = match store.read_workflow_snapshot(&run_id) {
        Ok(s) => rupu_orchestrator::Workflow::parse(&s).ok(),
        Err(_) => None,
    };
    let scripted = collect_scripted(&runs_dir.join(&run_id))?;
    let source = Box::new(source::ReplaySource::new(scripted, pace_us));
    app::App::new(run_id, source, store, workflow).run()
}

fn collect_scripted(run_dir: &std::path::Path) -> TuiResult<Vec<(String, rupu_transcript::Event)>> {
    let mut out = Vec::new();
    let transcripts = run_dir.join("transcripts");
    let Ok(rd) = std::fs::read_dir(&transcripts) else {
        return Ok(out);
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let step_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        for line in bytes.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_slice::<rupu_transcript::Event>(line) {
                out.push((step_id.clone(), event));
            }
        }
    }
    Ok(out)
}
