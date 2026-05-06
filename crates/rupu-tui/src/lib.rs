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

use std::path::PathBuf;
use rupu_orchestrator::RunStore;

pub fn run_watch(run_id: String, runs_dir: PathBuf) -> TuiResult<()> {
    let store = RunStore::new(runs_dir.clone());
    let _record = store.load(&run_id)?;
    let workflow = match store.read_workflow_snapshot(&run_id) {
        Ok(s) => rupu_orchestrator::Workflow::parse(&s).ok(),
        Err(_) => None,
    };
    let source = Box::new(source::JsonlTailSource::new(runs_dir.join(&run_id))?);
    app::App::new(run_id, source, store, workflow).run()
}
