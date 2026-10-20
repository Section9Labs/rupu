//! The conditions that watch a transcript instead of tailing a log: a
//! standalone agent run reaching its `run_complete` (`agent_run_end`) and a
//! session turn ending (`session_turn`). Both are re-checked like the run
//! state conditions; a run or turn that is still appearing is looked for
//! during [`APPEAR_GRACE`] before the call says it doesn't exist.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rupu_transcript::{Event, JsonlReader};
use serde_json::{json, Value};
use tokio::time::Instant;

use super::parse::{Condition, Kind};
use super::APPEAR_GRACE;
use crate::state::AppState;

/// What one look at a condition's subject found.
enum Probe {
    /// The condition holds, with its detail.
    Fired(Value),
    /// The subject exists and has not ended.
    Pending,
    /// The subject isn't there (yet); the reason, for the error once the
    /// grace is spent.
    Missing(String),
}

/// Record each `agent_run_end` / `session_turn` condition that holds now in
/// `fired`. An error names a run, session or turn that still isn't there
/// after [`APPEAR_GRACE`] counted from `since`.
pub(super) async fn check(
    state: &AppState,
    watch: &[Condition],
    since: Instant,
    fired: &mut BTreeMap<usize, Value>,
) -> Result<(), String> {
    for (i, c) in watch.iter().enumerate() {
        if fired.contains_key(&i) {
            continue;
        }
        let global = state.global_dir.clone();
        let target = c.target.clone();
        let probe = match &c.kind {
            Kind::AgentRunEnd => {
                tokio::task::spawn_blocking(move || agent_run_end(&global, &target)).await
            }
            Kind::SessionTurn { run } => {
                let run = run.clone();
                tokio::task::spawn_blocking(move || session_turn(&global, &target, run.as_deref()))
                    .await
            }
            _ => continue,
        };
        match probe.map_err(|e| format!("checking `{}`: {e}", c.target))?? {
            Probe::Fired(detail) => {
                fired.insert(i, detail);
            }
            Probe::Pending => {}
            Probe::Missing(why) if since.elapsed() >= APPEAR_GRACE => return Err(why),
            Probe::Missing(_) => {}
        }
    }
    Ok(())
}

/// A finished transcript's end: `(status, error, total_tokens)` of its
/// `run_complete`, `None` while it has none (or can't be read yet).
fn run_end(path: &Path) -> Option<(Value, Option<String>, u64)> {
    let mut end = None;
    for ev in JsonlReader::iter(path).ok()?.flatten() {
        if let Event::RunComplete {
            status,
            total_tokens,
            error,
            ..
        } = ev
        {
            end = Some((serde_json::to_value(status).ok()?, error, total_tokens));
        }
    }
    end
}

fn agent_run_end(global: &Path, run: &str) -> Result<Probe, String> {
    let Some(path) = crate::api::run_streams::agent_run_transcript(global, run) else {
        return Ok(Probe::Missing(format!("no agent run `{run}`")));
    };
    Ok(match run_end(&path) {
        None => Probe::Pending,
        Some((status, error, total_tokens)) => Probe::Fired(json!({
            "status": status,
            "error": error,
            "total_tokens": total_tokens,
            "final_answer": rupu_transcript::final_answer(&path),
        })),
    })
}

fn session_turn(global: &Path, session: &str, run: Option<&str>) -> Result<Probe, String> {
    let mut dto = None;
    for dir in ["sessions", "sessions-archive"] {
        if let Some(d) = crate::api::sessions::load_session_file(&global.join(dir).join(session))
            .map_err(|e| e.1)?
        {
            dto = Some(d);
            break;
        }
    }
    let Some(dto) = dto else {
        return Ok(Probe::Missing(format!("no session `{session}`")));
    };
    let turns = dto.turns();
    let stopped = dto.status_word() != "running";
    let turn = match run {
        Some(r) => Some(r.to_string()),
        None => dto
            .active_run_id()
            .map(str::to_string)
            .or_else(|| turns.last().map(|(r, _)| r.clone())),
    };
    // The turn may not be in `runs[]` yet while it is the active one.
    let known = |r: &str| dto.active_run_id() == Some(r) || turns.iter().any(|(t, _)| t == r);
    if let Some(r) = run.filter(|r| !known(r)) {
        return Ok(Probe::Missing(format!(
            "no turn `{r}` in session `{session}`"
        )));
    }
    let path: Option<PathBuf> = turn.as_deref().and_then(|r| {
        turns
            .iter()
            .find(|(t, _)| t == r)
            .and_then(|(_, p)| p.as_deref().filter(|p| !p.is_empty()))
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| crate::api::run_streams::agent_run_transcript(global, r))
    });
    let end = path.as_deref().and_then(run_end);
    if end.is_none() && !stopped {
        return Ok(Probe::Pending);
    }
    let error = end
        .as_ref()
        .and_then(|(_, e, _)| e.clone())
        .or_else(|| dto.last_error().map(str::to_string));
    Ok(Probe::Fired(json!({
        "session": session,
        "run_id": turn,
        "status": dto.status_word(),
        "final_answer": path.as_deref().and_then(rupu_transcript::final_answer),
        "error": error,
    })))
}
