//! What a recorded child is now. A pause can last days and a crashed
//! parent's children outlive it, so by the time anyone reads a recorded pid
//! it may name an unrelated process: a process child is its recorded pid
//! only while that pid's process started when the recorded one did
//! ([`crate::proc::start_stamp`]). A child recorded without a start time
//! can't be confirmed, so it is never read as running or signalled.
//!
//! Whether the child's run ended is a separate question, answered by its own
//! records ([`own_end`]): an agent's transcript, a workflow's `run.json`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rupu_orchestrator::{RunStatus, RunStore, RunStoreError};
use rupu_tools::{ChildKind, ChildState, ChildTokens};
use rupu_transcript::StopCause;

use crate::ledger::{Finished, Started};
use crate::proc::{is_zombie, leads_own_group, pid_is_running, start_stamp};

/// A process child's process as recorded: its pid and start time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recorded {
    pub pid: u32,
    pub start: u64,
}

/// What a recorded child's process is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leader {
    /// The child, running, leading its group.
    Live,
    /// The child, exited but not yet reaped: its pid, and so its group's id,
    /// is still reserved.
    Zombie,
    /// Not the child any more: no process has the pid, or another one does.
    /// Its group, if anything is left in it, can't be tied to the child.
    Gone,
}

impl Recorded {
    /// `None` for a child with no process, or one whose start time was not
    /// recorded.
    pub fn of(s: &Started) -> Option<Self> {
        Some(Self {
            pid: s.pgid.or(s.pid)?,
            start: s.proc_start?,
        })
    }

    pub fn leader(&self) -> Leader {
        if !pid_is_running(self.pid) || start_stamp(self.pid) != Some(self.start) {
            return Leader::Gone;
        }
        if is_zombie(self.pid) {
            return Leader::Zombie;
        }
        if !leads_own_group(self.pid) {
            return Leader::Gone;
        }
        Leader::Live
    }
}

/// What a child's own records say about its run.
#[derive(Debug, Clone, PartialEq)]
pub enum Own {
    /// It ended on its own: this end, under its handle.
    Ended(Finished),
    /// It started and did not end.
    Unended,
    /// Nothing says it started.
    NotStarted,
}

/// What `s`'s own records say. A run whose end carries a stop as its typed
/// cause ([`own_stop`]) did not end on its own. `stopped_at`: when the
/// parent stopped it, if it did — a workflow run that failed after that
/// failed of the stop (its runner killed), so it did not end on its own; one
/// that failed before did. A cancelled or rejected workflow run ended for
/// good whenever it did: only an operator records those.
pub fn own_end(global: &Path, s: &Started, stopped_at: Option<DateTime<Utc>>) -> Own {
    match s.kind {
        ChildKind::Workflow => workflow_end(global, s, stopped_at),
        ChildKind::Agent | ChildKind::Subagent => match &s.transcript {
            Some(t) => agent_end(global, t, s),
            None => Own::NotStarted,
        },
    }
}

fn workflow_end(global: &Path, s: &Started, stopped_at: Option<DateTime<Utc>>) -> Own {
    let store = RunStore::new(global.join("runs"));
    let rec = match store.load(&s.run_id) {
        Ok(rec) => rec,
        Err(RunStoreError::NotFound(_)) => return Own::NotStarted,
        Err(_) => return Own::Unended,
    };
    let failed = |why: String| {
        Own::Ended(Finished {
            tokens: tokens_of(global, s.kind, &s.run_id, None),
            ..Finished::new(&s.handle, ChildState::Failed, Some(why))
        })
    };
    let why = || {
        rec.error_message
            .clone()
            .unwrap_or_else(|| format!("workflow run {}", rec.status.as_str()))
    };
    match rec.status {
        RunStatus::Completed => Own::Ended(Finished {
            success: Some(true),
            output: Some(crate::process::last_step_output(&store, &s.run_id)),
            tokens: tokens_of(global, s.kind, &s.run_id, None),
            ..Finished::new(&s.handle, ChildState::Done, None)
        }),
        RunStatus::Cancelled | RunStatus::Rejected => failed(why()),
        RunStatus::Failed if rec.cause.as_ref().and_then(StopCause::of).is_some() => Own::Unended,
        RunStatus::Failed if stopped_at.is_some_and(|t| rec.finished_at.is_none_or(|f| f >= t)) => {
            Own::Unended
        }
        RunStatus::Failed => failed(why()),
        RunStatus::Pending
        | RunStatus::Running
        | RunStatus::Paused
        | RunStatus::AwaitingApproval => Own::Unended,
    }
}

/// How an agent transcript closed: whether its run started, and its
/// `run_complete` (status, error, typed cause), if any.
struct Closed {
    started: bool,
    end: Option<(
        rupu_transcript::RunStatus,
        Option<String>,
        Option<rupu_transcript::OutcomeRecord>,
    )>,
}

fn closed(transcript: &Path) -> Option<Closed> {
    let events = rupu_transcript::JsonlReader::iter(transcript).ok()?;
    let mut c = Closed {
        started: false,
        end: None,
    };
    for ev in events.flatten() {
        match ev {
            rupu_transcript::Event::RunStart { .. } => c.started = true,
            rupu_transcript::Event::RunComplete {
                status,
                error,
                outcome,
                ..
            } => c.end = Some((status, error, outcome)),
            _ => {}
        }
    }
    Some(c)
}

/// The stop `s`'s own records end on, when something outside it ended it
/// (a signal, its runner's death): a workflow run recorded `Failed` with a
/// stop as its typed cause, an agent transcript whose `run_complete`
/// carries one.
pub fn own_stop(global: &Path, s: &Started) -> Option<StopCause> {
    match s.kind {
        ChildKind::Workflow => {
            let rec = RunStore::new(global.join("runs")).load(&s.run_id).ok()?;
            (rec.status == RunStatus::Failed)
                .then(|| rec.cause.as_ref().and_then(StopCause::of))
                .flatten()
        }
        ChildKind::Agent | ChildKind::Subagent => {
            let (_, _, cause) = closed(s.transcript.as_deref()?)?.end?;
            cause.as_ref().and_then(StopCause::of)
        }
    }
}

fn agent_end(global: &Path, transcript: &Path, s: &Started) -> Own {
    let Some(Closed { started, end }) = closed(transcript) else {
        return Own::NotStarted;
    };
    let tokens = tokens_of(global, s.kind, &s.run_id, Some(transcript));
    match end {
        _ if !started => Own::NotStarted,
        Some((_, _, Some(cause))) if StopCause::of(&cause).is_some() => Own::Unended,
        Some((rupu_transcript::RunStatus::Ok, ..)) => Own::Ended(Finished {
            success: Some(true),
            output: Some(rupu_transcript::final_answer(transcript).unwrap_or_default()),
            tokens,
            ..Finished::new(&s.handle, ChildState::Done, None)
        }),
        Some((rupu_transcript::RunStatus::Error, error, _)) => Own::Ended(Finished {
            tokens,
            ..Finished::new(
                &s.handle,
                ChildState::Failed,
                Some(error.unwrap_or_else(|| "its run failed".into())),
            )
        }),
        Some((rupu_transcript::RunStatus::Aborted, ..)) | None => Own::Unended,
    }
}

/// The tokens a child's run `run_id` used: a process child's from its usage
/// ledger (its calls and its in-process children's), else an agent's
/// transcript.
pub fn tokens_of(
    global: &Path,
    kind: ChildKind,
    run_id: &str,
    transcript: Option<&Path>,
) -> Option<ChildTokens> {
    let ledger = RunStore::new(global.join("runs")).usage_ledger_path(run_id);
    let transcript = || transcript.and_then(transcript_tokens);
    match kind {
        ChildKind::Workflow => ledger_tokens(&ledger),
        ChildKind::Agent => ledger_tokens(&ledger).or_else(transcript),
        ChildKind::Subagent => transcript(),
    }
}

/// A usage ledger's token totals (rollup copies from further down are
/// counted by the run that made them). Rows count once by id.
pub fn ledger_tokens(path: &Path) -> Option<ChildTokens> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut seen = std::collections::HashSet::new();
    let mut t = ChildTokens::default();
    for row in text
        .lines()
        .filter_map(|l| serde_json::from_str::<rupu_runtime::usage_ledger::LedgerRow>(l).ok())
        .filter(|r| !r.rollup)
    {
        if seen.insert(row.id) {
            t.input += row.input_tokens;
            t.output += row.output_tokens;
        }
    }
    (!seen.is_empty()).then_some(t)
}

/// An agent run's token totals, from its transcript's usage events.
pub fn transcript_tokens(path: &Path) -> Option<ChildTokens> {
    let mut t = ChildTokens::default();
    let mut any = false;
    for ev in rupu_transcript::JsonlReader::iter(path).ok()?.flatten() {
        if let rupu_transcript::Event::Usage {
            input_tokens,
            output_tokens,
            ..
        } = ev
        {
            any = true;
            t.input += u64::from(input_tokens);
            t.output += u64::from(output_tokens);
        }
    }
    any.then_some(t)
}

/// `a` and `b` together.
pub fn plus(a: Option<ChildTokens>, b: Option<ChildTokens>) -> Option<ChildTokens> {
    match (a, b) {
        (Some(a), Some(b)) => Some(ChildTokens {
            input: a.input + b.input,
            output: a.output + b.output,
        }),
        (a, b) => a.or(b),
    }
}

/// The ledger directories below a child: its own run's, or, for a workflow
/// child, each of its step and unit attempts'.
pub fn below(global: &Path, kind: ChildKind, run_id: &str) -> Vec<PathBuf> {
    let runs = global.join("runs");
    match kind {
        ChildKind::Workflow => RunStore::new(runs.clone())
            .read_attempts(run_id)
            .unwrap_or_default()
            .into_iter()
            .map(|a| runs.join(a.agent_run_id))
            .collect(),
        ChildKind::Agent | ChildKind::Subagent => vec![runs.join(run_id)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    fn recorded(pid: u32, start: Option<u64>) -> Started {
        Started {
            pid: Some(pid),
            pgid: Some(pid),
            proc_start: start,
            ..Started::new("ch_1", ChildKind::Agent, "probe", "run_c", 1, 1)
        }
    }

    /// A recorded child is its pid only while that pid's process started
    /// when the recorded one did; one recorded without a start time is never
    /// confirmed.
    #[test]
    fn a_pid_is_the_child_only_while_its_start_time_agrees() {
        let mut proc = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = proc.id();
        let started = start_stamp(pid).expect("this platform gives start times");
        let live = Recorded::of(&recorded(pid, Some(started)))
            .unwrap()
            .leader();
        let recycled = Recorded::of(&recorded(pid, Some(started + 1)))
            .unwrap()
            .leader();
        let unconfirmed = Recorded::of(&recorded(pid, None));
        proc.kill().unwrap();
        proc.wait().unwrap();
        assert_eq!(live, Leader::Live);
        assert_eq!(recycled, Leader::Gone);
        assert_eq!(unconfirmed, None);
        assert_eq!(
            Recorded::of(&recorded(pid, Some(started)))
                .unwrap()
                .leader(),
            Leader::Gone
        );
    }
}
