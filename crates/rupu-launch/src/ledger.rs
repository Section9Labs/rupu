//! The children ledger (spec W7 §3.5, D20): `<parent run dir>/children.jsonl`,
//! append-only, one line per transition of a child the run launched — the one
//! index of what a run launched, of every kind. The CP run detail, the
//! agentiflow's budget and units view, and the orphan reaper read it.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use rupu_runtime::argv::LaunchLink;
use rupu_tools::{ChildKind, ChildState, ChildTokens, PermissionMode};
use serde::{Deserialize, Serialize};

/// The ledger's file name under a run's directory.
pub const CHILDREN_FILE: &str = "children.jsonl";

/// One line of the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum LedgerLine {
    /// The child was launched: queued for a slot, or started at once.
    Started(Started),
    /// A process child's process was spawned.
    Spawned(Spawned),
    Finished(Finished),
    /// `join` returned the child's final status: a `join` naming no handles
    /// skips it, here and in a run that continues this one.
    Joined(Joined),
    /// The run `by_run` took the child on under the same handle, continuing
    /// or restoring it: this ledger's record of it is closed for good.
    Continued(Continued),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Joined {
    pub handle: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Continued {
    pub handle: String,
    pub by_run: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spawned {
    pub handle: String,
    /// It leads its own group, so this is its pgid too.
    pub pid: u32,
    /// `pid`'s start time ([`crate::proc::start_stamp`]), read as it was
    /// spawned. `None`: its identity can't be confirmed later, so it is
    /// never signalled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<u64>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Started {
    pub handle: String,
    pub kind: ChildKind,
    pub name: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// Its address on the parent's agentiflow board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<PathBuf>,
    /// A process child's pid, from its [`Spawned`] line (or, in a ledger
    /// written before there was one, this line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<u64>,
    /// The process that launched it: the orphan reaper stops the process
    /// children of a parent that died.
    pub parent_pid: u32,
    /// `parent_pid`'s start time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_start: Option<u64>,
    pub depth: u32,
    pub at: DateTime<Utc>,
    /// What it was launched with: a child that never started is launched
    /// afresh from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Box<Request>>,
    /// The earlier run of this child that this run continues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues: Option<Box<PriorRun>>,
    /// The tokens this child's earlier runs used (runs other than
    /// `run_id`): `join` reports them with this run's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_tokens: Option<ChildTokens>,
    /// The directory of the run at the root of its launch tree (where the
    /// tree's usage is rolled up): whether that run ended says whether
    /// anything can still continue the child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<PathBuf>,
}

impl Started {
    /// A `started` line with nothing optional set.
    pub fn new(
        handle: &str,
        kind: ChildKind,
        name: &str,
        run_id: &str,
        parent_pid: u32,
        depth: u32,
    ) -> Self {
        Self {
            handle: handle.to_string(),
            kind,
            name: name.to_string(),
            run_id: run_id.to_string(),
            codename: None,
            address: None,
            transcript: None,
            pid: None,
            pgid: None,
            proc_start: None,
            parent_pid,
            parent_start: None,
            depth,
            at: Utc::now(),
            request: None,
            continues: None,
            prior_tokens: None,
            tree: None,
        }
    }
}

/// How a child was launched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The mode it runs at (a sub-agent's ceiling, a process child's
    /// `--mode`).
    pub mode: PermissionMode,
    /// A process child's link to its parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<LaunchLink>,
    /// An agent child's prompt, inputs folded in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// A workflow child's file: the one it was vetted as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
}

/// An earlier run of a child.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriorRun {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<PathBuf>,
    /// Its own [`Started::prior_tokens`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_tokens: Option<ChildTokens>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finished {
    pub handle: String,
    pub status: ChildState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// Its answer, as `join` returns it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The typed outcome a failed agent run stopped on
    /// (`rupu_transcript::OutcomeRecord`, serialized).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<ChildTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// When it ended; for `interrupted`, when it was stopped (before any
    /// signal), so an end its own run recorded later was caused by the stop.
    pub at: DateTime<Utc>,
}

impl Finished {
    /// `handle`'s end as `status`, saying why.
    pub fn new(handle: &str, status: ChildState, error: Option<String>) -> Self {
        Self {
            handle: handle.to_string(),
            status,
            success: None,
            output: None,
            error,
            cause: None,
            tokens: None,
            duration_ms: None,
            at: Utc::now(),
        }
    }
}

/// A child as the ledger tells it: its start, and its end once it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildEntry {
    pub started: Started,
    pub finished: Option<Finished>,
    /// `join` returned its final status.
    pub joined: bool,
    /// The run that took it on under the same handle: this record of it is
    /// closed, and that run's is its latest.
    pub continued_by: Option<String>,
}

impl ChildEntry {
    /// Its state as last recorded: `Running` until it finished.
    pub fn state(&self) -> ChildState {
        self.finished
            .as_ref()
            .map_or(ChildState::Running, |f| f.status)
    }

    /// Nothing more is recorded for it here: it ended for good (anything
    /// but `interrupted`, which the run that continues its parent picks
    /// up), or another run took it on.
    pub fn has_final_end(&self) -> bool {
        self.continued_by.is_some()
            || self
                .finished
                .as_ref()
                .is_some_and(|f| f.status != ChildState::Interrupted)
    }

    /// Recorded `interrupted` and not yet taken on by another run.
    pub fn is_interrupted(&self) -> bool {
        self.continued_by.is_none() && self.state() == ChildState::Interrupted
    }
}

/// Append `line` to `dir`'s ledger under an exclusive lock. Best-effort: a
/// failure is logged, never fails the launch it records. A child ends once:
/// a `finished` line for a child the ledger already has a final end for (or
/// a second `interrupted`) is dropped, so of two stoppers racing, the first
/// one's end stands. An `interrupted` end is not final: one end may follow
/// it. Nothing follows a `continued` line.
pub fn append(dir: &Path, line: &LedgerLine) {
    if let Err(e) = try_append(dir, line) {
        tracing::warn!(dir = %dir.display(), error = %e, "could not append to the children ledger");
    }
}

fn try_append(dir: &Path, line: &LedgerLine) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut text = serde_json::to_string(line).map_err(std::io::Error::other)?;
    text.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(dir.join(CHILDREN_FILE))?;
    f.lock_exclusive()?;
    let written = match line {
        LedgerLine::Finished(end) => current(&mut f, &end.handle).and_then(|entry| {
            let dropped = entry.is_some_and(|e| {
                e.has_final_end() || (end.status == ChildState::Interrupted && e.is_interrupted())
            });
            if dropped {
                Ok(())
            } else {
                f.write_all(text.as_bytes()).and_then(|()| f.flush())
            }
        }),
        _ => f.write_all(text.as_bytes()).and_then(|()| f.flush()),
    };
    let _ = FileExt::unlock(&f);
    written
}

fn current(f: &mut std::fs::File, handle: &str) -> std::io::Result<Option<ChildEntry>> {
    use std::io::Read;
    let mut text = String::new();
    f.read_to_string(&mut text)?;
    Ok(fold(&text).into_iter().find(|e| e.started.handle == handle))
}

/// Every child `dir`'s ledger records, in start order. A missing ledger is
/// none; an unparseable line (a torn tail) is skipped.
pub fn read(dir: &Path) -> Vec<ChildEntry> {
    std::fs::read_to_string(dir.join(CHILDREN_FILE))
        .map(|text| fold(&text))
        .unwrap_or_default()
}

fn fold(text: &str) -> Vec<ChildEntry> {
    let mut order: Vec<String> = Vec::new();
    let mut by_handle: BTreeMap<String, ChildEntry> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<LedgerLine>(line) {
            Ok(LedgerLine::Started(s)) => {
                if !by_handle.contains_key(&s.handle) {
                    order.push(s.handle.clone());
                }
                by_handle.insert(
                    s.handle.clone(),
                    ChildEntry {
                        started: s,
                        finished: None,
                        joined: false,
                        continued_by: None,
                    },
                );
            }
            Ok(LedgerLine::Spawned(p)) => {
                if let Some(e) = by_handle.get_mut(&p.handle) {
                    e.started.pid = Some(p.pid);
                    e.started.pgid = Some(p.pid);
                    e.started.proc_start = p.proc_start;
                }
            }
            Ok(LedgerLine::Finished(f)) => {
                if let Some(e) = by_handle.get_mut(&f.handle).filter(|e| !e.has_final_end()) {
                    e.finished = Some(f);
                }
            }
            Ok(LedgerLine::Joined(j)) => {
                if let Some(e) = by_handle.get_mut(&j.handle) {
                    e.joined = true;
                }
            }
            Ok(LedgerLine::Continued(c)) => {
                if let Some(e) = by_handle.get_mut(&c.handle) {
                    e.continued_by.get_or_insert(c.by_run);
                }
            }
            Err(_) => continue,
        }
    }
    order
        .into_iter()
        .filter_map(|h| by_handle.remove(&h))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(handle: &str) -> Started {
        Started {
            pid: Some(42),
            pgid: Some(42),
            ..Started::new(handle, ChildKind::Agent, "recon", "run_1", 7, 1)
        }
    }

    fn end(handle: &str, status: ChildState) -> Finished {
        Finished::new(handle, status, None)
    }

    #[test]
    fn transitions_fold_per_child_in_start_order() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &LedgerLine::Started(started("ch_b")));
        append(dir.path(), &LedgerLine::Started(started("ch_a")));
        append(
            dir.path(),
            &LedgerLine::Finished(Finished {
                handle: "ch_b".into(),
                status: ChildState::Done,
                success: Some(true),
                output: None,
                error: None,
                cause: None,
                tokens: None,
                duration_ms: Some(5),
                at: Utc::now(),
            }),
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(CHILDREN_FILE))
            .unwrap()
            .write_all(b"{\"t\":\"started\",\"hand")
            .unwrap();
        let children = read(dir.path());
        assert_eq!(
            children
                .iter()
                .map(|c| c.started.handle.as_str())
                .collect::<Vec<_>>(),
            ["ch_b", "ch_a"]
        );
        assert_eq!(children[0].state(), ChildState::Done);
        assert_eq!(children[1].state(), ChildState::Running);
    }

    /// Two stoppers racing: the first `finished` line stands, and the second
    /// is never written.
    #[test]
    fn a_child_ends_once() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &LedgerLine::Started(started("ch_1")));
        append(
            dir.path(),
            &LedgerLine::Finished(end("ch_1", ChildState::Abandoned)),
        );
        append(
            dir.path(),
            &LedgerLine::Finished(end("ch_1", ChildState::Failed)),
        );
        assert_eq!(read(dir.path())[0].state(), ChildState::Abandoned);
        let text = std::fs::read_to_string(dir.path().join(CHILDREN_FILE)).unwrap();
        assert_eq!(text.matches("\"t\":\"finished\"").count(), 1, "{text}");
    }

    /// An `interrupted` end is not final: the one end recorded after it (the
    /// child abandoned when nothing continues it) stands, and nothing after
    /// that.
    #[test]
    fn an_interrupted_child_ends_once_more() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &LedgerLine::Started(started("ch_1")));
        for status in [
            ChildState::Interrupted,
            ChildState::Abandoned,
            ChildState::Failed,
        ] {
            append(dir.path(), &LedgerLine::Finished(end("ch_1", status)));
        }
        assert_eq!(read(dir.path())[0].state(), ChildState::Abandoned);
        let text = std::fs::read_to_string(dir.path().join(CHILDREN_FILE)).unwrap();
        assert_eq!(text.matches("\"t\":\"finished\"").count(), 2, "{text}");
    }

    /// A child's answer and cause are recorded with its end, and `join`
    /// collecting it is a line of its own; lines written before either
    /// existed still read.
    #[test]
    fn an_end_keeps_the_answer_and_a_join_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &LedgerLine::Started(started("ch_1")));
        append(
            dir.path(),
            &LedgerLine::Finished(Finished {
                output: Some("the answer".into()),
                cause: Some(serde_json::json!({"class": "refusal"})),
                ..end("ch_1", ChildState::Done)
            }),
        );
        append(
            dir.path(),
            &LedgerLine::Joined(Joined {
                handle: "ch_1".into(),
                at: Utc::now(),
            }),
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(CHILDREN_FILE))
            .unwrap()
            .write_all(
                b"{\"t\":\"started\",\"handle\":\"ch_old\",\"kind\":\"agent\",\"name\":\"recon\",\"run_id\":\"run_2\",\"parent_pid\":7,\"depth\":1,\"at\":\"2026-10-01T00:00:00Z\"}\n\
                  {\"t\":\"finished\",\"handle\":\"ch_old\",\"status\":\"done\",\"success\":true,\"at\":\"2026-10-01T00:00:01Z\"}\n",
            )
            .unwrap();
        let children = read(dir.path());
        let f = children[0].finished.as_ref().unwrap();
        assert_eq!(f.output.as_deref(), Some("the answer"));
        assert_eq!(f.cause, Some(serde_json::json!({"class": "refusal"})));
        assert!(children[0].joined);
        assert_eq!(children[1].state(), ChildState::Done);
        assert_eq!(children[1].finished.as_ref().unwrap().output, None);
        assert!(!children[1].joined);
    }

    #[test]
    fn the_wire_shape_is_one_object_per_transition() {
        let line = serde_json::to_value(LedgerLine::Started(started("ch_1"))).unwrap();
        assert_eq!(line["t"], "started");
        assert_eq!(line["kind"], "agent");
        assert_eq!(line["pgid"], 42);
        assert!(read(Path::new("/nonexistent")).is_empty());
    }

    /// A second `interrupted` for a child already recorded `interrupted` is
    /// dropped; a process's spawn fills in the child's pid; and once another
    /// run took the child on (`continued`), nothing more is recorded for it.
    #[test]
    fn a_continued_child_is_closed_here() {
        let dir = tempfile::tempdir().unwrap();
        append(
            dir.path(),
            &LedgerLine::Started(Started::new(
                "ch_1",
                ChildKind::Agent,
                "recon",
                "run_1",
                7,
                1,
            )),
        );
        append(
            dir.path(),
            &LedgerLine::Spawned(Spawned {
                handle: "ch_1".into(),
                pid: 42,
                proc_start: Some(9),
                at: Utc::now(),
            }),
        );
        for _ in 0..2 {
            append(
                dir.path(),
                &LedgerLine::Finished(end("ch_1", ChildState::Interrupted)),
            );
        }
        append(
            dir.path(),
            &LedgerLine::Continued(Continued {
                handle: "ch_1".into(),
                by_run: "run_2".into(),
                at: Utc::now(),
            }),
        );
        append(
            dir.path(),
            &LedgerLine::Finished(end("ch_1", ChildState::Abandoned)),
        );
        let e = &read(dir.path())[0];
        assert_eq!(
            (e.started.pid, e.started.pgid, e.started.proc_start),
            (Some(42), Some(42), Some(9))
        );
        assert_eq!(e.state(), ChildState::Interrupted);
        assert_eq!(e.continued_by.as_deref(), Some("run_2"));
        assert!(e.has_final_end() && !e.is_interrupted());
        let text = std::fs::read_to_string(dir.path().join(CHILDREN_FILE)).unwrap();
        assert_eq!(text.matches("\"t\":\"finished\"").count(), 1, "{text}");
    }
}
