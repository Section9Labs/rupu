# Slice D Plan 3 — Live executor wiring + status pulse

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `rupu.app` Graph view come alive — nodes transition Waiting → Active → Working → Complete from real workflow execution events, the drill-down pane streams the focused step's transcript, and the user can approve/reject `ask`-mode steps from inside the app (inline on the awaiting node *and* in the drill-down pane). Same code path serves runs started in the app (in-process broadcast) and runs started elsewhere (disk-tail of a new `events.jsonl`).

**Architecture:** New `executor` module in `rupu-orchestrator` defines `WorkflowExecutor` + `EventSink` traits and a step-level `Event` enum. `InProcessExecutor` runs `run_workflow` in a tokio task, fanning events through `InMemorySink` (broadcast) + `JsonlSink` (append to `events.jsonl`). A `FileTailRunSource` consumes `events.jsonl` for runs the app didn't start. `rupu-app` gains `executor/`, `run_model.rs` (pure `apply(Event)` mutator), `view/drilldown.rs`, sidebar status dots, an inline approval button on the awaiting node, and a real menubar badge. CLI keeps its existing `rupu run` / `rupu watch` UX but routes through the new traits with zero user-visible behavior change.

**Tech Stack:** Rust 2021, GPUI (already pinned), `tokio` + `tokio-stream` (broadcast → `Stream`) + `tokio-util` (`CancellationToken`), `notify` (already pinned, for disk-tail), `serde_json` for `Event` round-trips, `chrono`, `thiserror`. No new external dependencies beyond `tokio-stream` and `tokio-util`.

**Spec:** `docs/superpowers/specs/2026-05-12-rupu-slice-d-plan-3-live-executor-design.md`

---

## File structure

### New files

```
crates/rupu-orchestrator/src/executor/
  mod.rs                              # Module root: pub use of submodules + WorkflowExecutor trait
  errors.rs                           # ExecutorError (thiserror)
  event.rs                            # Event enum + StepKind re-export + serde tests
  sink.rs                             # EventSink trait + FanOutSink
  jsonl_sink.rs                       # JsonlSink — append-only events.jsonl writer
  in_memory_sink.rs                   # InMemorySink — tokio::sync::broadcast wrapper
  in_process.rs                       # InProcessExecutor + RunState
  file_tail.rs                        # FileTailRunSource — notify-based events.jsonl reader

crates/rupu-orchestrator/tests/
  executor_in_process.rs              # InProcessExecutor integration tests
  executor_file_tail.rs               # FileTailRunSource integration tests

crates/rupu-app/src/
  executor/
    mod.rs                            # AppExecutor — wraps Arc<InProcessExecutor>
    attach.rs                         # in-process tail vs FileTailRunSource decision
  run_model.rs                        # RunModel + apply(Event) pure function
  view/
    drilldown.rs                      # Drill-down pane: transcript stream + approval bar
    transcript_tail.rs                # Per-step transcript file watcher (drill-down-local)

crates/rupu-app/tests/
  run_model.rs                        # RunModel::apply snapshot + property tests
```

### Modified files

```
Cargo.toml                            # Add tokio-stream + tokio-util to [workspace.dependencies]

crates/rupu-orchestrator/Cargo.toml   # Add tokio-stream + tokio-util to dependencies
crates/rupu-orchestrator/src/lib.rs   # pub mod executor;
crates/rupu-orchestrator/src/runner.rs:106-156   # OrchestratorRunOpts: add event_sink field; emit events at every transition

crates/rupu-agent/src/runner.rs:75-154            # AgentRunOpts: add on_tool_call callback field
crates/rupu-agent/src/runner.rs:375-435           # Tool-dispatch site: invoke on_tool_call before tool.invoke()

crates/rupu-cli/src/cmd/workflow.rs:1822, 1944, 841, 1931   # All four run_workflow call sites: pass event_sink
crates/rupu-cli/src/cmd/watch.rs (if exists)              # Watch CLI: route through FileTailRunSource

crates/rupu-app/Cargo.toml            # Add tokio, tokio-stream, futures, notify, rupu-orchestrator (already there), rupu-agent (new)
crates/rupu-app/src/lib.rs            # pub mod executor; pub mod run_model;
crates/rupu-app/src/view/mod.rs       # pub mod drilldown; pub mod transcript_tail;
crates/rupu-app/src/view/graph.rs     # Extend render() to take &RunModel; nodes consume NodeStatus from model
crates/rupu-app/src/window/mod.rs     # Wire RunModel into the main area; horizontal split with drilldown
crates/rupu-app/src/window/sidebar.rs # Status dot per workflow row
crates/rupu-app/src/menu/menubar.rs   # Pending-approvals counter from AppExecutor

Makefile                              # app-smoke target: scripted run, verify nodes light up
CLAUDE.md                             # Mark Plan 3 complete; note `events.jsonl` schema
```

---

## Implementation tasks

### Task 1: Add workspace dependencies

**Files:**
- Modify: `Cargo.toml` (root)

- [ ] **Step 1: Add tokio-stream and tokio-util to workspace.dependencies**

In `Cargo.toml`, in the `[workspace.dependencies]` section, after the `tokio = ...` line (line 33), add:

```toml
tokio-stream = { version = "0.1", features = ["sync"] }
tokio-util = "0.7"
```

- [ ] **Step 2: Verify workspace builds**

Run: `cargo metadata --format-version 1 > /dev/null`
Expected: exits 0 with no errors.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: add tokio-stream + tokio-util to workspace deps for D-3 executor"
```

---

### Task 2: Scaffold `rupu-orchestrator::executor` module

**Files:**
- Create: `crates/rupu-orchestrator/src/executor/mod.rs`
- Create: `crates/rupu-orchestrator/src/executor/errors.rs`
- Modify: `crates/rupu-orchestrator/src/lib.rs` (add `pub mod executor;`)
- Modify: `crates/rupu-orchestrator/Cargo.toml` (add `tokio-stream` + `tokio-util` deps)

- [ ] **Step 1: Add deps to `crates/rupu-orchestrator/Cargo.toml`**

In the `[dependencies]` section add:

```toml
tokio-stream = { workspace = true }
tokio-util = { workspace = true }
```

- [ ] **Step 2: Create `crates/rupu-orchestrator/src/executor/errors.rs`**

```rust
//! Errors surfaced by the `WorkflowExecutor` trait and its impls.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("workflow parse error: {0}")]
    WorkflowParse(#[from] crate::WorkflowParseError),

    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("run already active for workflow: {0}")]
    RunAlreadyActive(PathBuf),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("cancelled")]
    Cancelled,

    #[error("internal executor error: {0}")]
    Internal(String),
}
```

- [ ] **Step 3: Create `crates/rupu-orchestrator/src/executor/mod.rs`**

```rust
//! Executor — the live-run surface for `rupu.app` (Slice D Plan 3).
//!
//! `WorkflowExecutor` is the trait. `InProcessExecutor` runs workflows
//! in a tokio task and fans events through any number of `EventSink`s
//! (`InMemorySink` for live broadcast, `JsonlSink` for on-disk
//! `events.jsonl`). `FileTailRunSource` consumes `events.jsonl` for
//! runs the executor didn't start (CLI, cron, MCP).

pub mod errors;
pub mod event;
pub mod sink;
pub mod jsonl_sink;
pub mod in_memory_sink;
pub mod in_process;
pub mod file_tail;

pub use errors::ExecutorError;
pub use event::Event;
pub use sink::{EventSink, FanOutSink};
pub use jsonl_sink::JsonlSink;
pub use in_memory_sink::InMemorySink;
pub use in_process::{InProcessExecutor, RunHandle, WorkflowExecutor, WorkflowRunOpts, RunFilter};
pub use file_tail::FileTailRunSource;
```

- [ ] **Step 4: Add `pub mod executor;` to `crates/rupu-orchestrator/src/lib.rs`**

Find the existing `pub mod` declarations in `lib.rs` and add `pub mod executor;` in alphabetical position (between `event_vocab` and `runs`, roughly).

- [ ] **Step 5: Create empty submodule stubs so the crate compiles**

Create each of these files with a single comment line. Real implementations land in subsequent tasks.

`crates/rupu-orchestrator/src/executor/event.rs`:
```rust
//! Event enum — populated in Task 3.
```

`crates/rupu-orchestrator/src/executor/sink.rs`:
```rust
//! EventSink trait + FanOutSink — populated in Task 4.
```

`crates/rupu-orchestrator/src/executor/jsonl_sink.rs`:
```rust
//! JsonlSink — populated in Task 5.
```

`crates/rupu-orchestrator/src/executor/in_memory_sink.rs`:
```rust
//! InMemorySink — populated in Task 6.
```

`crates/rupu-orchestrator/src/executor/in_process.rs`:
```rust
//! InProcessExecutor — populated in Task 9.
```

`crates/rupu-orchestrator/src/executor/file_tail.rs`:
```rust
//! FileTailRunSource — populated in Task 10.
```

- [ ] **Step 6: Fix the `pub use` in `mod.rs` to only re-export what exists**

Replace the bottom of `crates/rupu-orchestrator/src/executor/mod.rs` (the `pub use` lines) with just:

```rust
pub use errors::ExecutorError;
```

We'll add the other re-exports as each submodule is populated.

- [ ] **Step 7: Verify build**

Run: `cargo build -p rupu-orchestrator`
Expected: builds clean.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-orchestrator/Cargo.toml crates/rupu-orchestrator/src/lib.rs crates/rupu-orchestrator/src/executor/
git commit -m "feat(rupu-orchestrator): scaffold executor module"
```

---

### Task 3: `Event` enum

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/event.rs`
- Modify: `crates/rupu-orchestrator/src/executor/mod.rs` (add `pub use event::Event;`)
- Test: `crates/rupu-orchestrator/src/executor/event.rs` (in-file `#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests**

Replace `crates/rupu-orchestrator/src/executor/event.rs` with the test scaffold first:

```rust
//! Step-level workflow event. Serialized as one JSON object per line
//! into `events.jsonl`. Same enum round-trips through the in-process
//! broadcast channel and the on-disk log — `Deserialize` + `Serialize`
//! both required.

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    #[test]
    fn run_started_round_trips_through_json() {
        let ev = Event::RunStarted {
            event_version: 1,
            run_id: "run_01J0".into(),
            workflow_path: PathBuf::from("/wf/foo.yaml"),
            started_at: chrono::Utc.with_ymd_and_hms(2026, 5, 12, 0, 0, 0).unwrap(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        match back {
            Event::RunStarted { event_version, run_id, .. } => {
                assert_eq!(event_version, 1);
                assert_eq!(run_id, "run_01J0");
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[test]
    fn step_completed_serializes_as_tagged_json() {
        let ev = Event::StepCompleted {
            run_id: "run_x".into(),
            step_id: "classify_input".into(),
            success: true,
            duration_ms: 312,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""type":"step_completed""#));
        assert!(json.contains(r#""step_id":"classify_input""#));
    }

    #[test]
    fn unknown_event_type_errors() {
        let bad = r#"{"type":"step_warped","run_id":"r","step_id":"s"}"#;
        let res: Result<Event, _> = serde_json::from_str(bad);
        assert!(res.is_err(), "unknown variant should fail to deserialize");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator --lib executor::event`
Expected: compile error (`Event` not defined).

- [ ] **Step 3: Implement the `Event` enum**

Replace `crates/rupu-orchestrator/src/executor/event.rs` with:

```rust
//! Step-level workflow event. Serialized as one JSON object per line
//! into `events.jsonl`. Same enum round-trips through the in-process
//! broadcast channel and the on-disk log — `Deserialize` + `Serialize`
//! both required.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runs::{RunStatus, StepKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        event_version: u32,
        run_id: String,
        workflow_path: PathBuf,
        started_at: DateTime<Utc>,
    },
    StepStarted {
        run_id: String,
        step_id: String,
        kind: StepKind,
        agent: Option<String>,
    },
    StepWorking {
        run_id: String,
        step_id: String,
        note: Option<String>,
    },
    StepAwaitingApproval {
        run_id: String,
        step_id: String,
        reason: String,
    },
    StepCompleted {
        run_id: String,
        step_id: String,
        success: bool,
        duration_ms: u64,
    },
    StepFailed {
        run_id: String,
        step_id: String,
        error: String,
    },
    StepSkipped {
        run_id: String,
        step_id: String,
        reason: String,
    },
    RunCompleted {
        run_id: String,
        status: RunStatus,
        finished_at: DateTime<Utc>,
    },
    RunFailed {
        run_id: String,
        error: String,
        finished_at: DateTime<Utc>,
    },
}

impl Event {
    pub fn run_id(&self) -> &str {
        match self {
            Event::RunStarted { run_id, .. }
            | Event::StepStarted { run_id, .. }
            | Event::StepWorking { run_id, .. }
            | Event::StepAwaitingApproval { run_id, .. }
            | Event::StepCompleted { run_id, .. }
            | Event::StepFailed { run_id, .. }
            | Event::StepSkipped { run_id, .. }
            | Event::RunCompleted { run_id, .. }
            | Event::RunFailed { run_id, .. } => run_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    #[test]
    fn run_started_round_trips_through_json() {
        let ev = Event::RunStarted {
            event_version: 1,
            run_id: "run_01J0".into(),
            workflow_path: PathBuf::from("/wf/foo.yaml"),
            started_at: chrono::Utc.with_ymd_and_hms(2026, 5, 12, 0, 0, 0).unwrap(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        match back {
            Event::RunStarted { event_version, run_id, .. } => {
                assert_eq!(event_version, 1);
                assert_eq!(run_id, "run_01J0");
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[test]
    fn step_completed_serializes_as_tagged_json() {
        let ev = Event::StepCompleted {
            run_id: "run_x".into(),
            step_id: "classify_input".into(),
            success: true,
            duration_ms: 312,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""type":"step_completed""#));
        assert!(json.contains(r#""step_id":"classify_input""#));
    }

    #[test]
    fn unknown_event_type_errors() {
        let bad = r#"{"type":"step_warped","run_id":"r","step_id":"s"}"#;
        let res: Result<Event, _> = serde_json::from_str(bad);
        assert!(res.is_err(), "unknown variant should fail to deserialize");
    }
}
```

- [ ] **Step 4: Add `pub use event::Event;` to `crates/rupu-orchestrator/src/executor/mod.rs`**

After the existing `pub use errors::ExecutorError;` line, append:

```rust
pub use event::Event;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --lib executor::event`
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/executor/event.rs crates/rupu-orchestrator/src/executor/mod.rs
git commit -m "feat(rupu-orchestrator): Event enum for executor (round-trip serialization)"
```

---

### Task 4: `EventSink` trait + `FanOutSink`

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/sink.rs`
- Modify: `crates/rupu-orchestrator/src/executor/mod.rs` (add `pub use sink::{EventSink, FanOutSink};`)

- [ ] **Step 1: Write the failing tests**

Replace `crates/rupu-orchestrator/src/executor/sink.rs` with:

```rust
//! EventSink trait + FanOutSink for delivering events to multiple
//! consumers (in-memory broadcast + on-disk JSONL).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Event;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CountingSink {
        count: Mutex<usize>,
    }

    impl EventSink for CountingSink {
        fn emit(&self, _run_id: &str, _ev: &Event) {
            *self.count.lock().unwrap() += 1;
        }
    }

    #[test]
    fn fan_out_delivers_to_every_sink() {
        let a = Arc::new(CountingSink::default());
        let b = Arc::new(CountingSink::default());
        let fan = FanOutSink::new(vec![
            a.clone() as Arc<dyn EventSink>,
            b.clone() as Arc<dyn EventSink>,
        ]);

        let ev = Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: crate::runs::StepKind::Linear,
            agent: None,
        };
        fan.emit("r", &ev);
        fan.emit("r", &ev);

        assert_eq!(*a.count.lock().unwrap(), 2);
        assert_eq!(*b.count.lock().unwrap(), 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator --lib executor::sink`
Expected: compile error (`EventSink` not defined).

- [ ] **Step 3: Implement `EventSink` + `FanOutSink`**

Replace `crates/rupu-orchestrator/src/executor/sink.rs` with:

```rust
//! EventSink trait + FanOutSink for delivering events to multiple
//! consumers (in-memory broadcast + on-disk JSONL).

use std::sync::Arc;

use crate::executor::Event;

pub trait EventSink: Send + Sync {
    fn emit(&self, run_id: &str, ev: &Event);
}

/// Fan-out wrapper: holds a vec of sinks and forwards every emit to
/// each. The runner uses one of these per run so it doesn't need to
/// know how many sinks are attached.
pub struct FanOutSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl FanOutSink {
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        Self { sinks }
    }

    pub fn push(&mut self, sink: Arc<dyn EventSink>) {
        self.sinks.push(sink);
    }
}

impl EventSink for FanOutSink {
    fn emit(&self, run_id: &str, ev: &Event) {
        for sink in &self.sinks {
            sink.emit(run_id, ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Event;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CountingSink {
        count: Mutex<usize>,
    }

    impl EventSink for CountingSink {
        fn emit(&self, _run_id: &str, _ev: &Event) {
            *self.count.lock().unwrap() += 1;
        }
    }

    #[test]
    fn fan_out_delivers_to_every_sink() {
        let a = Arc::new(CountingSink::default());
        let b = Arc::new(CountingSink::default());
        let fan = FanOutSink::new(vec![
            a.clone() as Arc<dyn EventSink>,
            b.clone() as Arc<dyn EventSink>,
        ]);

        let ev = Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: crate::runs::StepKind::Linear,
            agent: None,
        };
        fan.emit("r", &ev);
        fan.emit("r", &ev);

        assert_eq!(*a.count.lock().unwrap(), 2);
        assert_eq!(*b.count.lock().unwrap(), 2);
    }
}
```

- [ ] **Step 4: Add `pub use sink::{EventSink, FanOutSink};` to `mod.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --lib executor::sink`
Expected: 1 test passes.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/executor/sink.rs crates/rupu-orchestrator/src/executor/mod.rs
git commit -m "feat(rupu-orchestrator): EventSink trait + FanOutSink"
```

---

### Task 5: `JsonlSink`

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/jsonl_sink.rs`
- Modify: `crates/rupu-orchestrator/src/executor/mod.rs` (add `pub use jsonl_sink::JsonlSink;`)

- [ ] **Step 1: Write the failing test**

Replace `crates/rupu-orchestrator/src/executor/jsonl_sink.rs` with the test scaffold:

```rust
//! JsonlSink — appends serialized events to <run_dir>/events.jsonl,
//! one JSON line per event. Append-only, never rotated. fsync on
//! drop. Write failures log a warning but never propagate.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Event;
    use crate::runs::StepKind;

    #[test]
    fn writes_each_event_as_one_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let sink = JsonlSink::create(&path).expect("create");

        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: None,
        });
        sink.emit("r", &Event::StepCompleted {
            run_id: "r".into(),
            step_id: "s1".into(),
            success: true,
            duration_ms: 17,
        });
        drop(sink);

        let body = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("step_started"));
        assert!(lines[1].contains("step_completed"));
    }

    #[test]
    fn round_trips_through_serde() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let sink = JsonlSink::create(&path).expect("create");
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: Some("classifier".into()),
        });
        drop(sink);
        let body = std::fs::read_to_string(&path).unwrap();
        let ev: Event = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        match ev {
            Event::StepStarted { step_id, agent, .. } => {
                assert_eq!(step_id, "s1");
                assert_eq!(agent.as_deref(), Some("classifier"));
            }
            _ => panic!("expected StepStarted"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator --lib executor::jsonl_sink`
Expected: compile error (`JsonlSink` not defined).

- [ ] **Step 3: Implement `JsonlSink`**

Replace `crates/rupu-orchestrator/src/executor/jsonl_sink.rs` with:

```rust
//! JsonlSink — appends serialized events to <run_dir>/events.jsonl,
//! one JSON line per event. Append-only, never rotated. fsync on
//! drop. Write failures log a warning but never propagate.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::warn;

use crate::executor::sink::EventSink;
use crate::executor::Event;

pub struct JsonlSink {
    path: PathBuf,
    file: Mutex<File>,
}

impl JsonlSink {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EventSink for JsonlSink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        let line = match serde_json::to_string(ev) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "JsonlSink: failed to serialize event");
                return;
            }
        };
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!(error = %e, "JsonlSink: file mutex poisoned");
                return;
            }
        };
        if let Err(e) = writeln!(*guard, "{line}") {
            warn!(error = %e, path = %self.path.display(), "JsonlSink: append failed");
        }
    }
}

impl Drop for JsonlSink {
    fn drop(&mut self) {
        if let Ok(guard) = self.file.lock() {
            let _ = guard.sync_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Event;
    use crate::runs::StepKind;

    #[test]
    fn writes_each_event_as_one_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let sink = JsonlSink::create(&path).expect("create");

        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: None,
        });
        sink.emit("r", &Event::StepCompleted {
            run_id: "r".into(),
            step_id: "s1".into(),
            success: true,
            duration_ms: 17,
        });
        drop(sink);

        let body = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("step_started"));
        assert!(lines[1].contains("step_completed"));
    }

    #[test]
    fn round_trips_through_serde() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.jsonl");
        let sink = JsonlSink::create(&path).expect("create");
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: Some("classifier".into()),
        });
        drop(sink);
        let body = std::fs::read_to_string(&path).unwrap();
        let ev: Event = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        match ev {
            Event::StepStarted { step_id, agent, .. } => {
                assert_eq!(step_id, "s1");
                assert_eq!(agent.as_deref(), Some("classifier"));
            }
            _ => panic!("expected StepStarted"),
        }
    }
}
```

- [ ] **Step 4: Add `pub use jsonl_sink::JsonlSink;` to `mod.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --lib executor::jsonl_sink`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/executor/jsonl_sink.rs crates/rupu-orchestrator/src/executor/mod.rs
git commit -m "feat(rupu-orchestrator): JsonlSink — append-only events.jsonl writer"
```

---

### Task 6: `InMemorySink`

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/in_memory_sink.rs`
- Modify: `crates/rupu-orchestrator/src/executor/mod.rs` (add `pub use in_memory_sink::InMemorySink;`)

- [ ] **Step 1: Write the failing test**

Replace `crates/rupu-orchestrator/src/executor/in_memory_sink.rs` with:

```rust
//! InMemorySink — wraps a tokio::sync::broadcast::Sender so the
//! executor can fan events to live subscribers (e.g. the rupu.app
//! Graph view). Non-blocking emit, drops on no-subscribers.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::sink::EventSink;
    use crate::executor::Event;
    use crate::runs::StepKind;

    #[tokio::test]
    async fn two_subscribers_both_receive_the_same_event() {
        let sink = InMemorySink::with_capacity(16);
        let mut a = sink.subscribe();
        let mut b = sink.subscribe();
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: StepKind::Linear,
            agent: None,
        });
        let ev_a = a.recv().await.expect("a recv");
        let ev_b = b.recv().await.expect("b recv");
        assert_eq!(ev_a.run_id(), "r");
        assert_eq!(ev_b.run_id(), "r");
    }

    #[tokio::test]
    async fn no_subscribers_drops_silently() {
        let sink = InMemorySink::with_capacity(16);
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: StepKind::Linear,
            agent: None,
        });
        // No panic, no error — just dropped.
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-orchestrator --lib executor::in_memory_sink`
Expected: compile error.

- [ ] **Step 3: Implement `InMemorySink`**

Replace `crates/rupu-orchestrator/src/executor/in_memory_sink.rs` with:

```rust
//! InMemorySink — wraps a tokio::sync::broadcast::Sender so the
//! executor can fan events to live subscribers (e.g. the rupu.app
//! Graph view). Non-blocking emit, drops on no-subscribers.

use tokio::sync::broadcast;

use crate::executor::sink::EventSink;
use crate::executor::Event;

pub struct InMemorySink {
    tx: broadcast::Sender<Event>,
}

impl InMemorySink {
    pub fn with_capacity(cap: usize) -> Self {
        let (tx, _) = broadcast::channel(cap);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl EventSink for InMemorySink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        // send() returns Err when there are no live receivers; that's
        // expected (the run started before anyone subscribed) and we
        // deliberately drop. The on-disk JsonlSink is the durable copy.
        let _ = self.tx.send(ev.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Event;
    use crate::runs::StepKind;

    #[tokio::test]
    async fn two_subscribers_both_receive_the_same_event() {
        let sink = InMemorySink::with_capacity(16);
        let mut a = sink.subscribe();
        let mut b = sink.subscribe();
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: StepKind::Linear,
            agent: None,
        });
        let ev_a = a.recv().await.expect("a recv");
        let ev_b = b.recv().await.expect("b recv");
        assert_eq!(ev_a.run_id(), "r");
        assert_eq!(ev_b.run_id(), "r");
    }

    #[tokio::test]
    async fn no_subscribers_drops_silently() {
        let sink = InMemorySink::with_capacity(16);
        sink.emit("r", &Event::StepStarted {
            run_id: "r".into(),
            step_id: "s".into(),
            kind: StepKind::Linear,
            agent: None,
        });
    }
}
```

- [ ] **Step 4: Add `pub use in_memory_sink::InMemorySink;` to `mod.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --lib executor::in_memory_sink`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/executor/in_memory_sink.rs crates/rupu-orchestrator/src/executor/mod.rs
git commit -m "feat(rupu-orchestrator): InMemorySink — broadcast channel for live subscribers"
```

---

### Task 7: Runner wiring — thread `event_sink` through `run_workflow`

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs:106-156` (`OrchestratorRunOpts` adds `event_sink` field)
- Modify: `crates/rupu-orchestrator/src/runner.rs:298-...` (`run_workflow` emits events at every transition)
- Test: `crates/rupu-orchestrator/tests/runner_events.rs` (new)

- [ ] **Step 1: Add `event_sink` field to `OrchestratorRunOpts`**

In `crates/rupu-orchestrator/src/runner.rs`, find the `OrchestratorRunOpts` struct (line ~106) and add after the `strict_templates: bool` field (line ~155):

```rust
    /// Optional event sink. When `Some`, the runner emits
    /// `Event::RunStarted` / `Event::StepStarted` / etc. at each
    /// transition. When `None`, behavior is unchanged (back-compat for
    /// any direct caller).
    pub event_sink: Option<std::sync::Arc<dyn crate::executor::EventSink>>,
```

- [ ] **Step 2: Update all `OrchestratorRunOpts { ... }` constructors in the test suite**

Run a search to find every `OrchestratorRunOpts {` constructor:

```
rg -n 'OrchestratorRunOpts \{' crates/
```

For each construction site, add `event_sink: None,` to keep behavior unchanged. Typical CLI call sites already use the struct literal — append the new field everywhere.

- [ ] **Step 3: Write the failing test**

Create `crates/rupu-orchestrator/tests/runner_events.rs`:

```rust
//! Integration test: the runner emits Run/Step events at every transition.

use std::sync::{Arc, Mutex};

use rupu_orchestrator::executor::{Event, EventSink};

#[derive(Default)]
struct CollectSink {
    events: Mutex<Vec<Event>>,
}

impl EventSink for CollectSink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

#[tokio::test]
async fn run_workflow_emits_run_and_step_events_in_order() {
    // Build a minimal workflow with two linear steps using the
    // MockProvider + BypassDecider from rupu-agent::runner::tests.
    // The test asserts the event stream is:
    //   RunStarted, StepStarted(s1), StepCompleted(s1),
    //   StepStarted(s2), StepCompleted(s2), RunCompleted.
    //
    // See `crates/rupu-agent/src/runner.rs` for MockProvider helpers
    // exposed by the agent crate's tests module; replicate the
    // factory wiring here against the public mock surface.

    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    let _opts_event_sink: Arc<dyn EventSink> = sink.clone();
    // ... call rupu_orchestrator::run_workflow with event_sink set ...
    // (Filled in after the runner is wired to actually emit.)

    let events = sink.events.lock().unwrap();
    assert!(matches!(events.first(), Some(Event::RunStarted { .. })),
        "first event must be RunStarted, got {:?}", events.first());
    assert!(matches!(events.last(), Some(Event::RunCompleted { .. })),
        "last event must be RunCompleted, got {:?}", events.last());
}
```

(The test body is intentionally a scaffold — fill in the workflow wiring once the runner emits. The reviewer's job is to verify the event sequence assertion is correct; the implementer fills in the harness.)

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p rupu-orchestrator --test runner_events`
Expected: test compiles but the asserts fail (or the test scaffold is incomplete — fill it in).

- [ ] **Step 5: Implement emission in `run_workflow`**

In `crates/rupu-orchestrator/src/runner.rs`, inside `run_workflow` (after `let resolved_inputs = ...`, around line 302), emit `RunStarted`:

```rust
    if let Some(sink) = opts.event_sink.as_ref() {
        sink.emit(
            &run_id,
            &crate::executor::Event::RunStarted {
                event_version: 1,
                run_id: run_id.clone(),
                workflow_path: opts.workspace_path.join(&opts.workflow.name),
                started_at: chrono::Utc::now(),
            },
        );
    }
```

Inside the step-iteration loop (in `run_steps_inner`), emit:

- `StepStarted` before each step dispatch
- `StepCompleted { success: true }` after a successful step
- `StepFailed { error }` on agent error
- `StepSkipped { reason }` when `when:` falsies
- `StepAwaitingApproval { reason }` when an `ask`-mode step pauses

After the loop, emit `RunCompleted` or `RunFailed` based on the terminal outcome.

The exact insertion points are at every place `step_results.push(...)` is currently called in `run_steps_inner` — wrap each push with an emit of the appropriate variant.

- [ ] **Step 6: Run all orchestrator tests to verify nothing regressed**

Run: `cargo test -p rupu-orchestrator`
Expected: all existing tests pass; the new event test passes.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/tests/runner_events.rs
git commit -m "feat(rupu-orchestrator): runner emits step-level events via EventSink"
```

---

### Task 8: `on_tool_call` callback in `rupu-agent` (StepWorking source)

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs:75-154` (`AgentRunOpts` gets `on_tool_call` callback)
- Modify: `crates/rupu-agent/src/runner.rs:375-435` (tool dispatch site invokes the callback)
- Modify: `crates/rupu-orchestrator/src/runner.rs` (wires `on_tool_call` into each step's `AgentRunOpts`, translating to `Event::StepWorking`)

- [ ] **Step 1: Write the failing test**

In `crates/rupu-agent/src/runner.rs` (in the existing `#[cfg(test)]` mod), add:

```rust
#[tokio::test]
async fn on_tool_call_fires_once_per_tool_invocation() {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let calls_clone = calls.clone();
    let cb: OnToolCallCallback = std::sync::Arc::new(move |step_id, tool_name| {
        calls_clone.lock().unwrap().push(format!("{step_id}:{tool_name}"));
    });

    // Build AgentRunOpts with MockProvider configured to emit one
    // tool-call turn followed by a final-text turn. Set on_tool_call
    // = Some(cb).
    let opts = make_agent_opts_with_one_tool_call(/* step_id */ "s1", Some(cb));
    let _ = run_agent(opts).await.expect("agent runs");

    let log = calls.lock().unwrap();
    assert_eq!(log.len(), 1, "expected exactly one on_tool_call");
    assert!(log[0].starts_with("s1:"), "expected step_id prefix");
}
```

(`make_agent_opts_with_one_tool_call` is a test helper to be added alongside existing `MockProvider` helpers.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-agent --lib on_tool_call_fires`
Expected: compile error (`OnToolCallCallback` not defined).

- [ ] **Step 3: Add the callback type + field to `AgentRunOpts`**

In `crates/rupu-agent/src/runner.rs`, near the top of the module (after the existing `pub use` lines), add:

```rust
/// Callback invoked by `run_agent` immediately before each tool
/// dispatch. The runner translates this into `Event::StepWorking
/// { note: Some(tool_name) }` so the Graph view can pulse the
/// active node. Called from the agent's tokio task — must be
/// non-blocking.
pub type OnToolCallCallback = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;
```

In the `AgentRunOpts` struct (around line ~150), add a new field after the existing fields:

```rust
    pub on_tool_call: Option<OnToolCallCallback>,
```

Update every `AgentRunOpts { ... }` constructor in tests + the orchestrator's `StepFactory` impls to include `on_tool_call: None,`.

- [ ] **Step 4: Invoke the callback in the tool-dispatch site**

In `crates/rupu-agent/src/runner.rs` around line 389 (the `tool.invoke(...)` call), insert before the invoke:

```rust
        if let Some(cb) = opts.on_tool_call.as_ref() {
            // `step_id` comes from the AgentRunOpts that the orchestrator
            // populates per step. For sub-agent dispatch (depth>0) we
            // still emit so the parent step's working beacon stays
            // honest.
            cb(&opts.run_id, &tool_name);
        }
```

Adjust the first arg to be the step_id, not the run_id. The agent crate only has access to `run_id` and `parent_run_id` today, **not** the orchestrator's step_id. To pass it through, add another field:

```rust
    /// Step id that owns this agent run. Threaded through so
    /// `on_tool_call` can identify which step is calling. Empty
    /// for free-standing agent runs (no orchestrator).
    pub step_id: String,
```

And populate it from the orchestrator's `StepFactory::build_opts_for_step()` impls.

- [ ] **Step 5: Run the agent test to verify it passes**

Run: `cargo test -p rupu-agent --lib on_tool_call_fires`
Expected: 1 test passes.

- [ ] **Step 6: Wire `StepWorking` emission in the orchestrator**

In `crates/rupu-orchestrator/src/runner.rs`'s `run_steps_inner`, before dispatching each step, build the `on_tool_call` closure that emits `Event::StepWorking`:

```rust
    let sink_clone = opts.event_sink.clone();
    let run_id_for_cb = run_id.clone();
    let step_id_for_cb = step.id.clone();
    let on_tool_call: Option<rupu_agent::OnToolCallCallback> =
        sink_clone.map(|sink| {
            std::sync::Arc::new(move |_step_id: &str, tool_name: &str| {
                sink.emit(
                    &run_id_for_cb,
                    &crate::executor::Event::StepWorking {
                        run_id: run_id_for_cb.clone(),
                        step_id: step_id_for_cb.clone(),
                        note: Some(tool_name.to_string()),
                    },
                );
            }) as rupu_agent::OnToolCallCallback
        });
    // ... pass this into the StepFactory::build_opts_for_step call's
    //     opts.on_tool_call field via a small extension to the trait
    //     (or a constructor variant).
```

Note: this requires the `StepFactory` trait to accept an `on_tool_call` parameter. Extend the trait signature in `crates/rupu-orchestrator/src/runner.rs`:

```rust
pub trait StepFactory: Send + Sync {
    fn build_opts_for_step(
        &self,
        // ...existing args...
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts;
}
```

Update all impls (`crates/rupu-cli/src/factory.rs` and any test fakes) to accept the new arg.

- [ ] **Step 7: Run the full orchestrator + CLI test suite**

Run: `cargo test -p rupu-orchestrator -p rupu-cli -p rupu-agent`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-agent/src/runner.rs crates/rupu-orchestrator/src/runner.rs crates/rupu-cli/src/factory.rs
git commit -m "feat(rupu-agent): on_tool_call callback wires StepWorking events"
```

---

### Task 9: `InProcessExecutor` — start, list_runs, tail

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/in_process.rs`
- Modify: `crates/rupu-orchestrator/src/executor/mod.rs`
- Test: `crates/rupu-orchestrator/tests/executor_in_process.rs` (new)

- [ ] **Step 1: Write the failing integration test**

Create `crates/rupu-orchestrator/tests/executor_in_process.rs`:

```rust
//! Integration test: InProcessExecutor::start runs a workflow and
//! emits events that subscribers can tail in order.

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::StreamExt;
use rupu_orchestrator::executor::{
    Event, InProcessExecutor, RunFilter, WorkflowExecutor, WorkflowRunOpts,
};

#[tokio::test]
async fn start_then_tail_yields_events_in_order() {
    // Build a minimal two-step workflow YAML in a tempdir, point
    // workspace_path / transcript_dir at the same tempdir. Use the
    // mock-provider factory from the CLI's test harness (see
    // crates/rupu-cli/tests/cli_usage.rs for the helper or copy it
    // into a shared test-fixtures module).

    let tmp = tempfile::tempdir().expect("tempdir");
    let exec = Arc::new(InProcessExecutor::new());

    let handle = exec
        .start(WorkflowRunOpts {
            workflow_path: tmp.path().join("wf.yaml"),
            vars: Default::default(),
        })
        .await
        .expect("start");

    let mut stream = exec.tail(&handle.run_id).expect("tail");
    let mut kinds: Vec<&'static str> = Vec::new();
    while let Some(ev) = stream.next().await {
        kinds.push(match ev {
            Event::RunStarted { .. } => "run_started",
            Event::StepStarted { .. } => "step_started",
            Event::StepCompleted { .. } => "step_completed",
            Event::RunCompleted { .. } => "run_completed",
            _ => "other",
        });
        if kinds.last() == Some(&"run_completed") {
            break;
        }
    }
    assert_eq!(
        kinds.first(),
        Some(&"run_started"),
        "first event must be run_started, got {kinds:?}"
    );
    assert_eq!(
        kinds.last(),
        Some(&"run_completed"),
        "last event must be run_completed, got {kinds:?}"
    );
}

#[tokio::test]
async fn list_runs_returns_active_for_in_flight() {
    let exec = Arc::new(InProcessExecutor::new());
    // ... start a workflow, immediately list_runs(Active), assert it
    // contains the new run_id; await completion, list_runs(Active)
    // again, assert empty.
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-orchestrator --test executor_in_process`
Expected: compile error (`InProcessExecutor` not defined).

- [ ] **Step 3: Implement `InProcessExecutor` (start, list_runs, tail)**

Replace `crates/rupu-orchestrator/src/executor/in_process.rs` with:

```rust
//! InProcessExecutor — runs workflows in a tokio task and fans
//! events through every attached sink. The rupu.app singleton holds
//! one of these; the CLI builds a short-lived one per command.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_util::Stream;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

use crate::executor::errors::ExecutorError;
use crate::executor::sink::{EventSink, FanOutSink};
use crate::executor::{Event, InMemorySink, JsonlSink};
use crate::runs::{RunRecord, RunStatus, RunStore};

pub type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;

pub struct WorkflowRunOpts {
    pub workflow_path: PathBuf,
    pub vars: std::collections::BTreeMap<String, String>,
}

pub struct RunHandle {
    pub run_id: String,
    pub workflow_path: PathBuf,
}

pub enum RunFilter {
    All,
    ByWorkflowPath(PathBuf),
    ByStatus(RunStatus),
    Active,
}

pub trait WorkflowExecutor: Send + Sync {
    fn start(
        &self,
        opts: WorkflowRunOpts,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<RunHandle, ExecutorError>> + Send + '_>>;

    fn list_runs(&self, filter: RunFilter) -> Vec<RunRecord>;

    fn tail(&self, run_id: &str) -> Result<EventStream, ExecutorError>;

    fn approve(
        &self,
        run_id: &str,
        approver: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>>;

    fn reject(
        &self,
        run_id: &str,
        reason: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>>;

    fn cancel(
        &self,
        run_id: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>>;
}

struct RunState {
    in_memory: Arc<InMemorySink>,
    #[allow(dead_code)]
    jsonl: Arc<JsonlSink>,
    join: Mutex<Option<JoinHandle<()>>>,
    cancel: CancellationToken,
    record: Mutex<RunRecord>,
}

pub struct InProcessExecutor {
    run_store: Arc<RunStore>,
    runs: Mutex<HashMap<String, Arc<RunState>>>,
}

impl InProcessExecutor {
    pub fn new(run_store: Arc<RunStore>) -> Self {
        Self {
            run_store,
            runs: Mutex::new(HashMap::new()),
        }
    }
}

impl WorkflowExecutor for InProcessExecutor {
    fn start(
        &self,
        opts: WorkflowRunOpts,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<RunHandle, ExecutorError>> + Send + '_>>
    {
        Box::pin(async move {
            // Parse workflow YAML, build OrchestratorRunOpts. Use the
            // existing factory wiring (rupu-cli's StepFactory) — for the
            // app this is a different factory, see Task 12. For the
            // in-process executor in rupu-orchestrator's own tests we
            // accept a factory dependency in the constructor (see
            // `new_with_factory` overload — add it).

            // 1. read workflow yaml from opts.workflow_path
            // 2. construct InMemorySink + JsonlSink, wrap in FanOutSink
            // 3. spawn tokio task that calls run_workflow(opts) with
            //    event_sink = Some(fan_out)
            // 4. insert RunState into self.runs
            // 5. return RunHandle

            todo!("implementation per design — fill in")
        })
    }

    fn list_runs(&self, filter: RunFilter) -> Vec<RunRecord> {
        let runs = self.runs.lock().unwrap();
        runs.values()
            .filter_map(|state| {
                let rec = state.record.lock().unwrap().clone();
                let pass = match &filter {
                    RunFilter::All => true,
                    RunFilter::ByWorkflowPath(p) => rec.workspace_path == *p,
                    RunFilter::ByStatus(s) => &rec.status == s,
                    RunFilter::Active => matches!(
                        rec.status,
                        RunStatus::Running | RunStatus::AwaitingApproval
                    ),
                };
                pass.then_some(rec)
            })
            .collect()
    }

    fn tail(&self, run_id: &str) -> Result<EventStream, ExecutorError> {
        let runs = self.runs.lock().unwrap();
        let state = runs
            .get(run_id)
            .ok_or_else(|| ExecutorError::RunNotFound(run_id.into()))?;
        let rx = state.in_memory.subscribe();
        // BroadcastStream yields Result<Event, BroadcastStreamRecvError>;
        // drop lagged events (subscribers reconcile via FileTailRunSource
        // against events.jsonl).
        let stream = BroadcastStream::new(rx).filter_map(|res| async move { res.ok() });
        Ok(Box::pin(stream))
    }

    fn approve(
        &self,
        _run_id: &str,
        _approver: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>> {
        Box::pin(async move { todo!("Task 9b") })
    }

    fn reject(
        &self,
        _run_id: &str,
        _reason: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>> {
        Box::pin(async move { todo!("Task 9b") })
    }

    fn cancel(
        &self,
        _run_id: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>> {
        Box::pin(async move { todo!("Task 9b") })
    }
}
```

- [ ] **Step 4: Wire `pub use` in `mod.rs`**

Add to `crates/rupu-orchestrator/src/executor/mod.rs`:

```rust
pub use in_process::{
    EventStream, InProcessExecutor, RunFilter, RunHandle, WorkflowExecutor, WorkflowRunOpts,
};
```

- [ ] **Step 5: Implement the `start()` body fully**

Replace the `todo!()` in `start()` with the real implementation: parse the workflow YAML, build a `RunStore`-backed record, create sinks, spawn the task, populate `self.runs`. The runner work goes in a private `async fn execute(...)` that the spawn delegates to. See spec §"Implementations § InProcessExecutor" for the full sequence.

- [ ] **Step 6: Run the tail test**

Run: `cargo test -p rupu-orchestrator --test executor_in_process start_then_tail_yields_events_in_order`
Expected: passes.

- [ ] **Step 7: Implement and run `list_runs_returns_active_for_in_flight`**

Fill in the test body (the harness already exists from step 1), then run:

Run: `cargo test -p rupu-orchestrator --test executor_in_process list_runs_returns_active`
Expected: passes.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-orchestrator/src/executor/ crates/rupu-orchestrator/tests/executor_in_process.rs
git commit -m "feat(rupu-orchestrator): InProcessExecutor (start, list_runs, tail)"
```

---

### Task 9b: `InProcessExecutor` — approve, reject, cancel

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/in_process.rs`
- Test: `crates/rupu-orchestrator/tests/executor_in_process.rs` (extend)

- [ ] **Step 1: Write the failing test for `approve`**

Add to `crates/rupu-orchestrator/tests/executor_in_process.rs`:

```rust
#[tokio::test]
async fn approve_unsticks_an_awaiting_step() {
    let exec = Arc::new(InProcessExecutor::new(/* run_store */));
    let handle = exec.start(make_opts_with_ask_step()).await.expect("start");

    let mut stream = exec.tail(&handle.run_id).expect("tail");
    let mut saw_awaiting = false;
    while let Some(ev) = stream.next().await {
        if let Event::StepAwaitingApproval { step_id, .. } = &ev {
            saw_awaiting = true;
            exec.approve(&handle.run_id, "test").await.expect("approve");
            // Continue draining; next event should be StepStarted for
            // the subsequent step.
        }
        if let Event::RunCompleted { .. } = ev {
            break;
        }
    }
    assert!(saw_awaiting, "expected to see StepAwaitingApproval");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-orchestrator --test executor_in_process approve_unsticks`
Expected: panics on `todo!()` in `approve()`.

- [ ] **Step 3: Implement `approve` / `reject` / `cancel`**

In `crates/rupu-orchestrator/src/executor/in_process.rs`, replace the three `todo!()` bodies:

```rust
fn approve(
    &self,
    run_id: &str,
    approver: &str,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>> {
    let run_id = run_id.to_string();
    let approver = approver.to_string();
    Box::pin(async move {
        // Delegate to RunStore::approve (writes run.json + flips
        // status to Running). The runner's polling loop picks up
        // the status change on its next tick and resumes the
        // workflow — which then emits the next StepStarted via the
        // existing event_sink wiring. No additional emit needed
        // here.
        let _ = self
            .run_store
            .approve(&run_id, &approver, chrono::Utc::now())
            .map_err(|e| ExecutorError::Internal(e.to_string()))?;
        Ok(())
    })
}

fn reject(
    &self,
    run_id: &str,
    reason: &str,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>> {
    let run_id = run_id.to_string();
    let reason = reason.to_string();
    Box::pin(async move {
        let _ = self
            .run_store
            .reject(&run_id, "app", &reason, chrono::Utc::now())
            .map_err(|e| ExecutorError::Internal(e.to_string()))?;
        Ok(())
    })
}

fn cancel(
    &self,
    run_id: &str,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), ExecutorError>> + Send + '_>> {
    let run_id = run_id.to_string();
    Box::pin(async move {
        let runs = self.runs.lock().unwrap();
        let state = runs
            .get(&run_id)
            .ok_or_else(|| ExecutorError::RunNotFound(run_id.clone()))?;
        state.cancel.cancel();
        Ok(())
    })
}
```

The `approve` path additionally needs to push an `Event::StepStarted` (or `StepCompleted` for the awaiting step) when the runner's polling tick detects the status flip. That happens inside the runner; no extra emit at this layer.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p rupu-orchestrator --test executor_in_process approve_unsticks`
Expected: passes.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-orchestrator/src/executor/in_process.rs crates/rupu-orchestrator/tests/executor_in_process.rs
git commit -m "feat(rupu-orchestrator): InProcessExecutor approve/reject/cancel"
```

---

### Task 10: `FileTailRunSource`

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/file_tail.rs`
- Modify: `crates/rupu-orchestrator/src/executor/mod.rs`
- Test: `crates/rupu-orchestrator/tests/executor_file_tail.rs` (new)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-orchestrator/tests/executor_file_tail.rs`:

```rust
//! FileTailRunSource yields events as the file grows.

use futures_util::StreamExt;
use rupu_orchestrator::executor::{Event, FileTailRunSource};

#[tokio::test]
async fn yields_lines_as_file_grows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    // Write one event up front
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string(&Event::RunStarted {
                event_version: 1,
                run_id: "r1".into(),
                workflow_path: dir.path().to_path_buf(),
                started_at: chrono::Utc::now(),
            })
            .unwrap()
        ),
    )
    .unwrap();

    let mut source = FileTailRunSource::open(&path).await.expect("open");
    let first = source.next().await.expect("first event");
    assert!(matches!(first, Event::RunStarted { .. }));

    // Append another line and assert it's yielded
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(
        f,
        "{}",
        serde_json::to_string(&Event::RunCompleted {
            run_id: "r1".into(),
            status: rupu_orchestrator::runs::RunStatus::Completed,
            finished_at: chrono::Utc::now(),
        })
        .unwrap()
    )
    .unwrap();
    drop(f);

    let second = tokio::time::timeout(std::time::Duration::from_secs(2), source.next())
        .await
        .expect("timeout")
        .expect("second event");
    assert!(matches!(second, Event::RunCompleted { .. }));
}

#[tokio::test]
async fn waits_for_file_to_be_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    // Do not create the file yet
    let mut source = FileTailRunSource::open(&path).await.expect("open");

    tokio::spawn({
        let path = path.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            std::fs::write(
                &path,
                format!(
                    "{}\n",
                    serde_json::to_string(&Event::RunStarted {
                        event_version: 1,
                        run_id: "rN".into(),
                        workflow_path: path.parent().unwrap().to_path_buf(),
                        started_at: chrono::Utc::now(),
                    })
                    .unwrap()
                ),
            )
            .unwrap();
        }
    });

    let ev = tokio::time::timeout(std::time::Duration::from_secs(3), source.next())
        .await
        .expect("timeout")
        .expect("event");
    assert!(matches!(ev, Event::RunStarted { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-orchestrator --test executor_file_tail`
Expected: compile error.

- [ ] **Step 3: Implement `FileTailRunSource`**

Replace `crates/rupu-orchestrator/src/executor/file_tail.rs` with:

```rust
//! FileTailRunSource — notify-driven consumer of events.jsonl for
//! runs the executor didn't start (CLI / cron / MCP). Yields parsed
//! Event values as a Stream.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::executor::Event;

pub struct FileTailRunSource {
    rx: mpsc::Receiver<Event>,
    _watcher: notify::RecommendedWatcher,
}

impl FileTailRunSource {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Event>(64);
        let path_buf: PathBuf = path.to_path_buf();
        let parent = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        // Spawn a background task that:
        //   1. waits for the file to exist (poll with 100ms tick)
        //   2. reads existing lines and pushes them
        //   3. installs a notify watcher; on each modify event, reads
        //      new bytes from `offset` and pushes Events.
        let tx_clone = tx.clone();
        let path_for_task = path_buf.clone();
        tokio::spawn(async move {
            while !path_for_task.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let mut offset: u64 = 0;
            // Initial drain
            if let Ok(bytes) = std::fs::read(&path_for_task) {
                for line in std::str::from_utf8(&bytes).unwrap_or("").lines() {
                    if let Ok(ev) = serde_json::from_str::<Event>(line) {
                        if tx_clone.send(ev).await.is_err() {
                            return;
                        }
                    }
                }
                offset = bytes.len() as u64;
            }
            // Park; the watcher below feeds new bytes
            let _ = offset; // suppress unused (the real loop reads via the notify callback)
        });

        // Set up the watcher. notify callbacks run on a separate
        // thread; bridge into the async mpsc via a std mpsc + tokio
        // bridge or block_on. The simplest pattern: notify's
        // RecommendedWatcher fires on a thread; we use
        // tokio::task::spawn_blocking inside the callback to send,
        // but mpsc::Sender::blocking_send works fine.
        let tx_for_watcher = tx.clone();
        let path_for_watcher = path_buf.clone();
        let mut offset_state: u64 = 0;
        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                if let Ok(evt) = res {
                    if matches!(evt.kind, notify::EventKind::Modify(_) | notify::EventKind::Create(_)) {
                        let Ok(bytes) = std::fs::read(&path_for_watcher) else {
                            return;
                        };
                        if (bytes.len() as u64) <= offset_state {
                            return;
                        }
                        let new = &bytes[offset_state as usize..];
                        for line in std::str::from_utf8(new).unwrap_or("").lines() {
                            if let Ok(ev) = serde_json::from_str::<Event>(line) {
                                let _ = tx_for_watcher.blocking_send(ev);
                            }
                        }
                        offset_state = bytes.len() as u64;
                    }
                }
            },
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(Self {
            rx,
            _watcher: watcher,
        })
    }
}

impl Stream for FileTailRunSource {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let this = self.get_mut();
        this.rx.poll_recv(cx)
    }
}
```

(Note: the offset bookkeeping in the snippet above has a deliberate simplification — the initial drain and the watcher's offset should share state. The implementer should wire an `Arc<AtomicU64>` for `offset` so both the initial drain and the watcher use the same source of truth. The test asserts behavior end-to-end so a working solution falls out from making the tests pass.)

- [ ] **Step 4: Add `pub use file_tail::FileTailRunSource;` to `mod.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-orchestrator --test executor_file_tail`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/executor/file_tail.rs crates/rupu-orchestrator/src/executor/mod.rs crates/rupu-orchestrator/tests/executor_file_tail.rs
git commit -m "feat(rupu-orchestrator): FileTailRunSource for disk-tail consumers"
```

---

### Task 11: CLI refactor — `rupu run` routes through `InProcessExecutor`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (`Action::Run` and `Action::Resume` handlers)
- Existing tests in `crates/rupu-cli/tests/cli_usage.rs` must continue to pass

- [ ] **Step 1: Confirm baseline tests pass**

Run: `cargo test -p rupu-cli`
Expected: all current tests pass.

- [ ] **Step 2: Refactor the `Action::Run` handler**

In `crates/rupu-cli/src/cmd/workflow.rs`, find the `run(...)` async function (around line 1090). Today it builds `OrchestratorRunOpts` and calls `run_workflow(opts).await` directly (around line 1944).

Replace the direct call with:

```rust
    use rupu_orchestrator::executor::{InProcessExecutor, JsonlSink, WorkflowExecutor, WorkflowRunOpts};
    use std::sync::Arc;

    // The CLI doesn't need an InMemorySink (no live subscribers in
    // the same process) — only the JSONL log so app/CLI-tail readers
    // can attach. Build a single-sink executor.
    let events_path = run_store.events_path(&run_id_for_executor);
    let jsonl = Arc::new(JsonlSink::create(&events_path).map_err(map_io_err)?);
    let executor = Arc::new(InProcessExecutor::with_sinks(run_store.clone(), vec![jsonl]));

    let handle = executor
        .start(WorkflowRunOpts {
            workflow_path: workflow_path.clone(),
            vars: resolved_inputs.clone(),
        })
        .await?;

    // Tail the run so the CLI keeps printing per-step progress.
    let mut stream = executor.tail(&handle.run_id)?;
    while let Some(ev) = stream.next().await {
        printer.handle_event(&ev); // existing CLI printer pattern
        if let rupu_orchestrator::executor::Event::RunCompleted { .. }
            | rupu_orchestrator::executor::Event::RunFailed { .. } = &ev
        {
            break;
        }
    }
```

The `printer.handle_event` adapter is a new helper on the existing `LineStreamPrinter` that pattern-matches `Event` and prints the same lines the CLI prints today. Add it in `crates/rupu-cli/src/output/printer.rs`:

```rust
pub fn handle_event(&mut self, ev: &rupu_orchestrator::executor::Event) {
    use rupu_orchestrator::executor::Event;
    match ev {
        Event::RunStarted { .. } => self.print_run_start(),
        Event::StepStarted { step_id, kind, agent, .. } => self.print_step_start(step_id, *kind, agent.as_deref()),
        Event::StepCompleted { step_id, success, duration_ms, .. } => self.print_step_complete(step_id, *success, *duration_ms),
        Event::StepFailed { step_id, error, .. } => self.print_step_failed(step_id, error),
        Event::StepSkipped { step_id, reason, .. } => self.print_step_skipped(step_id, reason),
        Event::StepAwaitingApproval { step_id, reason, .. } => self.print_awaiting(step_id, reason),
        Event::RunCompleted { status, .. } => self.print_run_completed(*status),
        Event::RunFailed { error, .. } => self.print_run_failed(error),
        Event::StepWorking { .. } => {} // CLI doesn't print working beacons
    }
}
```

Map each `print_*` method to the equivalent existing line that the CLI emits today (the methods already exist — wire the adapter).

- [ ] **Step 3: Add `InProcessExecutor::with_sinks` constructor**

In `crates/rupu-orchestrator/src/executor/in_process.rs`, add to `impl InProcessExecutor`:

```rust
pub fn with_sinks(
    run_store: Arc<RunStore>,
    extra_sinks: Vec<Arc<dyn EventSink>>,
) -> Self {
    let mut s = Self::new(run_store);
    s.extra_sinks = extra_sinks;
    s
}
```

And update the struct to hold `extra_sinks: Vec<Arc<dyn EventSink>>` plus the always-on `InMemorySink`. In `start()`, build the `FanOutSink` from `[in_memory, ...extra_sinks]`.

- [ ] **Step 4: Add `RunStore::events_path()` helper**

In `crates/rupu-orchestrator/src/runs.rs`, in `impl RunStore`, add:

```rust
pub fn events_path(&self, run_id: &str) -> PathBuf {
    self.run_dir(run_id).join("events.jsonl")
}
```

(Using whatever the existing helper name is for resolving a run's directory — `run_dir`, `record_path`'s parent, etc. Match the codebase's convention.)

- [ ] **Step 5: Run the CLI test suite**

Run: `cargo test -p rupu-cli`
Expected: all existing tests pass — no user-visible behavior change.

- [ ] **Step 6: Manually exercise `rupu run` against a sample workflow**

Run: `cargo run --bin rupu -- run smoke -- input=hi` (or whatever the local sample workflow is).
Expected: same CLI output as before; a new `events.jsonl` exists in the run's dir.

Run: `ls $XDG_STATE_HOME/rupu/runs/<latest>/`
Expected: `run.json`, `step_results.jsonl`, `events.jsonl`, `transcripts/`.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/cmd/workflow.rs crates/rupu-cli/src/output/printer.rs crates/rupu-orchestrator/src/executor/in_process.rs crates/rupu-orchestrator/src/runs.rs
git commit -m "refactor(rupu-cli): rupu run routes through InProcessExecutor + writes events.jsonl"
```

---

### Task 12: `rupu-app::executor::AppExecutor`

**Files:**
- Modify: `crates/rupu-app/Cargo.toml`
- Create: `crates/rupu-app/src/executor/mod.rs`
- Create: `crates/rupu-app/src/executor/attach.rs`
- Modify: `crates/rupu-app/src/lib.rs`

- [ ] **Step 1: Add deps to `crates/rupu-app/Cargo.toml`**

In `[dependencies]`, add:

```toml
futures-util.workspace = true
notify.workspace = true
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "sync"] }
tokio-stream.workspace = true
rupu-agent = { path = "../rupu-agent" }
```

- [ ] **Step 2: Create `crates/rupu-app/src/executor/mod.rs`**

```rust
//! AppExecutor — singleton per app instance. Wraps an
//! Arc<InProcessExecutor>; routes attach() between in-process tail
//! and disk-tail; mirrors approve/reject/cancel to the right backend.

pub mod attach;

use std::path::PathBuf;
use std::sync::Arc;

use rupu_orchestrator::executor::{
    EventStream, InProcessExecutor, RunFilter, WorkflowExecutor, WorkflowRunOpts,
};
use rupu_orchestrator::runs::{RunRecord, RunStore};

use crate::executor::attach::attach_stream;

pub struct AppExecutor {
    inner: Arc<InProcessExecutor>,
    run_store: Arc<RunStore>,
}

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("run not found: {0}")]
    RunNotFound(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl AppExecutor {
    pub fn new(run_store: Arc<RunStore>) -> Self {
        let inner = Arc::new(InProcessExecutor::new(run_store.clone()));
        Self { inner, run_store }
    }

    pub fn run_store(&self) -> &Arc<RunStore> {
        &self.run_store
    }

    pub async fn start_workflow(
        &self,
        workflow_path: PathBuf,
    ) -> Result<String, rupu_orchestrator::executor::ExecutorError> {
        let handle = self
            .inner
            .start(WorkflowRunOpts {
                workflow_path,
                vars: Default::default(),
            })
            .await?;
        Ok(handle.run_id)
    }

    pub fn list_active_runs(&self, workflow_path: Option<PathBuf>) -> Vec<RunRecord> {
        match workflow_path {
            Some(p) => self.inner.list_runs(RunFilter::ByWorkflowPath(p)),
            None => self.inner.list_runs(RunFilter::Active),
        }
    }

    pub async fn attach(&self, run_id: &str) -> Result<EventStream, AttachError> {
        attach_stream(&self.inner, &self.run_store, run_id).await
    }

    pub async fn approve(
        &self,
        run_id: &str,
        approver: &str,
    ) -> Result<(), rupu_orchestrator::executor::ExecutorError> {
        // Same path for both in-process and disk-tail runs — RunStore
        // is the authority. The runner picks up the status flip.
        self.inner.approve(run_id, approver).await
    }

    pub async fn reject(
        &self,
        run_id: &str,
        reason: &str,
    ) -> Result<(), rupu_orchestrator::executor::ExecutorError> {
        self.inner.reject(run_id, reason).await
    }

    pub async fn cancel(
        &self,
        run_id: &str,
    ) -> Result<(), rupu_orchestrator::executor::ExecutorError> {
        self.inner.cancel(run_id).await
    }
}
```

- [ ] **Step 3: Create `crates/rupu-app/src/executor/attach.rs`**

```rust
//! Decides whether to attach to a run via the in-process executor's
//! broadcast channel or via FileTailRunSource against events.jsonl.

use std::sync::Arc;

use rupu_orchestrator::executor::{
    EventStream, FileTailRunSource, InProcessExecutor, RunFilter, WorkflowExecutor,
};
use rupu_orchestrator::runs::RunStore;

use super::AttachError;

pub async fn attach_stream(
    inner: &Arc<InProcessExecutor>,
    run_store: &Arc<RunStore>,
    run_id: &str,
) -> Result<EventStream, AttachError> {
    // In-process first (cheaper, lower latency)
    let active = inner.list_runs(RunFilter::All);
    if active.iter().any(|r| r.id == run_id) {
        return inner
            .tail(run_id)
            .map_err(|_| AttachError::RunNotFound(run_id.into()));
    }
    // Fall back to disk tail
    let events_path = run_store.events_path(run_id);
    let source = FileTailRunSource::open(&events_path).await?;
    Ok(Box::pin(source))
}
```

- [ ] **Step 4: Add `pub mod executor;` to `crates/rupu-app/src/lib.rs`**

- [ ] **Step 5: Verify the crate builds**

Run: `cargo build -p rupu-app`
Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/Cargo.toml crates/rupu-app/src/lib.rs crates/rupu-app/src/executor/
git commit -m "feat(rupu-app): AppExecutor wraps InProcessExecutor + attach decision"
```

---

### Task 13: `RunModel` + `apply(Event)`

**Files:**
- Create: `crates/rupu-app/src/run_model.rs`
- Modify: `crates/rupu-app/src/lib.rs`
- Test: `crates/rupu-app/tests/run_model.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-app/tests/run_model.rs`:

```rust
//! RunModel::apply is a pure function — test each Event variant.

use rupu_app::run_model::RunModel;
use rupu_app_canvas::NodeStatus;
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{RunStatus, StepKind};

fn fixture(run_id: &str) -> RunModel {
    RunModel::new(run_id.into(), "wf.yaml".into())
}

#[test]
fn run_started_marks_run_running() {
    let model = fixture("r1");
    let model = model.apply(&Event::RunStarted {
        event_version: 1,
        run_id: "r1".into(),
        workflow_path: "wf.yaml".into(),
        started_at: chrono::Utc::now(),
    });
    assert_eq!(model.run_status, RunStatus::Running);
}

#[test]
fn step_started_flips_node_to_active() {
    let model = fixture("r1").apply(&Event::StepStarted {
        run_id: "r1".into(),
        step_id: "s1".into(),
        kind: StepKind::Linear,
        agent: None,
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Active));
    assert_eq!(model.active_step.as_deref(), Some("s1"));
}

#[test]
fn step_working_flips_node_to_working() {
    let model = fixture("r1")
        .apply(&Event::StepStarted {
            run_id: "r1".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: None,
        })
        .apply(&Event::StepWorking {
            run_id: "r1".into(),
            step_id: "s1".into(),
            note: Some("gh_pr_list".into()),
        });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Working));
}

#[test]
fn step_completed_flips_node_to_complete() {
    let model = fixture("r1")
        .apply(&Event::StepStarted {
            run_id: "r1".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: None,
        })
        .apply(&Event::StepCompleted {
            run_id: "r1".into(),
            step_id: "s1".into(),
            success: true,
            duration_ms: 42,
        });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Complete));
}

#[test]
fn step_awaiting_approval_flips_node_and_focus() {
    let model = fixture("r1").apply(&Event::StepAwaitingApproval {
        run_id: "r1".into(),
        step_id: "s1".into(),
        reason: "ok?".into(),
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Awaiting));
    assert_eq!(model.focused_step.as_deref(), Some("s1"));
}

#[test]
fn step_failed_flips_node_to_failed() {
    let model = fixture("r1").apply(&Event::StepFailed {
        run_id: "r1".into(),
        step_id: "s1".into(),
        error: "boom".into(),
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Failed));
}

#[test]
fn step_skipped_flips_node_to_skipped() {
    let model = fixture("r1").apply(&Event::StepSkipped {
        run_id: "r1".into(),
        step_id: "s1".into(),
        reason: "when:false".into(),
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Skipped));
}

#[test]
fn run_completed_finalizes_status() {
    let model = fixture("r1").apply(&Event::RunCompleted {
        run_id: "r1".into(),
        status: RunStatus::Completed,
        finished_at: chrono::Utc::now(),
    });
    assert_eq!(model.run_status, RunStatus::Completed);
    assert!(model.active_step.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app --test run_model`
Expected: compile error (`RunModel` not defined).

- [ ] **Step 3: Implement `RunModel`**

Create `crates/rupu-app/src/run_model.rs`:

```rust
//! RunModel — mutable per-run state in the app. Built by applying
//! Events from the executor stream. `apply()` is a pure function so
//! tests can drive it deterministically.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rupu_app_canvas::NodeStatus;
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::RunStatus;

#[derive(Debug, Clone)]
pub struct RunModel {
    pub run_id: String,
    pub workflow_path: PathBuf,
    pub run_status: RunStatus,
    pub nodes: BTreeMap<String, NodeStatus>,
    pub active_step: Option<String>,
    pub focused_step: Option<String>,
    pub focused_step_last_set: Option<chrono::DateTime<chrono::Utc>>,
}

impl RunModel {
    pub fn new(run_id: String, workflow_path: PathBuf) -> Self {
        Self {
            run_id,
            workflow_path,
            run_status: RunStatus::Pending,
            nodes: BTreeMap::new(),
            active_step: None,
            focused_step: None,
            focused_step_last_set: None,
        }
    }

    pub fn apply(mut self, ev: &Event) -> Self {
        match ev {
            Event::RunStarted { .. } => {
                self.run_status = RunStatus::Running;
            }
            Event::StepStarted { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Active);
                self.active_step = Some(step_id.clone());
            }
            Event::StepWorking { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Working);
            }
            Event::StepAwaitingApproval { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Awaiting);
                self.run_status = RunStatus::AwaitingApproval;
                // Auto-focus on awaiting unless the user manually
                // focused something else in the last 10s
                let should_auto_focus = self
                    .focused_step_last_set
                    .map(|t| chrono::Utc::now().signed_duration_since(t).num_seconds() >= 10)
                    .unwrap_or(true);
                if should_auto_focus {
                    self.focused_step = Some(step_id.clone());
                    self.focused_step_last_set = Some(chrono::Utc::now());
                }
            }
            Event::StepCompleted { step_id, success, .. } => {
                let status = if *success { NodeStatus::Complete } else { NodeStatus::SoftFailed };
                self.nodes.insert(step_id.clone(), status);
                if self.active_step.as_deref() == Some(step_id) {
                    self.active_step = None;
                }
            }
            Event::StepFailed { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Failed);
                if self.active_step.as_deref() == Some(step_id) {
                    self.active_step = None;
                }
            }
            Event::StepSkipped { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Skipped);
            }
            Event::RunCompleted { status, .. } => {
                self.run_status = *status;
                self.active_step = None;
            }
            Event::RunFailed { .. } => {
                self.run_status = RunStatus::Failed;
                self.active_step = None;
            }
        }
        self
    }

    /// Called when the user clicks a node — overrides auto-focus.
    pub fn set_user_focus(&mut self, step_id: Option<String>) {
        self.focused_step = step_id;
        self.focused_step_last_set = Some(chrono::Utc::now());
    }
}
```

- [ ] **Step 4: Add `pub mod run_model;` to `crates/rupu-app/src/lib.rs`**

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-app --test run_model`
Expected: 8 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/run_model.rs crates/rupu-app/src/lib.rs crates/rupu-app/tests/run_model.rs
git commit -m "feat(rupu-app): RunModel + pure apply(Event) mutator"
```

---

### Task 14: Extend `view::graph::render` to consume `RunModel`

**Files:**
- Modify: `crates/rupu-app/src/view/graph.rs:13` (entry point signature changes)
- Modify: `crates/rupu-app-canvas/src/git_graph.rs` (status lookup hook on `render_rows`)
- Test: `crates/rupu-app-canvas/tests/git_graph_snapshots.rs` (new snapshot with custom statuses)

- [ ] **Step 1: Add status lookup parameter to `render_rows`**

In `crates/rupu-app-canvas/src/git_graph.rs`, change the signature of `render_rows`:

```rust
/// Render workflow as graph rows, using `status_lookup` to pick the
/// `NodeStatus` for each step. Pass `|_| NodeStatus::Waiting` for the
/// D-2 static case.
pub fn render_rows<F>(wf: &Workflow, status_lookup: F) -> Vec<GraphRow>
where
    F: Fn(&str) -> NodeStatus,
{
    // existing body — but replace every place where it used
    // NodeStatus::Waiting unconditionally with `status_lookup(step_id)`.
}
```

Update existing call sites in `rupu-app/src/view/graph.rs:14` from `render_rows(workflow)` → `render_rows(workflow, |_| NodeStatus::Waiting)` (for D-2 compatibility).

- [ ] **Step 2: Write the failing snapshot test**

Add to `crates/rupu-app-canvas/tests/git_graph_snapshots.rs`:

```rust
#[test]
fn snapshot_panel_with_one_active_step() {
    let yaml = r#"
name: live-snapshot
steps:
  - id: classify
    agent: classifier
    prompt: "go"
  - id: review_panel
    panel:
      panelists: [sec, perf]
      subject: "review"
    actions: []
"#;
    let wf = rupu_orchestrator::Workflow::parse(yaml).expect("parse");
    let rows = rupu_app_canvas::render_rows(&wf, |id| match id {
        "classify" => rupu_app_canvas::NodeStatus::Complete,
        "review_panel" => rupu_app_canvas::NodeStatus::Active,
        "sec" => rupu_app_canvas::NodeStatus::Working,
        _ => rupu_app_canvas::NodeStatus::Waiting,
    });
    insta::assert_yaml_snapshot!(rows);
}
```

- [ ] **Step 3: Run the test to verify it fails or accept the snapshot**

Run: `cargo test -p rupu-app-canvas --test git_graph_snapshots snapshot_panel_with_one_active_step`
Expected: snapshot missing — run `cargo insta accept` after eyeballing the output.

- [ ] **Step 4: Extend `view::graph::render` to take `&RunModel`**

In `crates/rupu-app/src/view/graph.rs`, replace lines 13-30 with:

```rust
/// Top-level entry point: render a `RunModel` as the git-graph view.
/// For the static D-2 case (no live run yet), construct a `RunModel`
/// with all `Waiting` nodes.
pub fn render(workflow: &Workflow, model: &crate::run_model::RunModel) -> impl IntoElement {
    let rows = rupu_app_canvas::render_rows(workflow, |id| {
        model.nodes.get(id).copied().unwrap_or(NodeStatus::Waiting)
    });

    let mut container = div()
        .size_full()
        .bg(palette::BG_PRIMARY)
        .px(px(24.0))
        .py(px(20.0))
        .flex()
        .flex_col()
        .gap(px(2.0));

    for row in &rows {
        container = container.child(render_row(row));
    }

    container
}
```

- [ ] **Step 5: Update the caller in `window/mod.rs`**

In `crates/rupu-app/src/window/mod.rs`'s `render_main_for_workflow`, the existing call to `view::graph::render(&wf)` becomes:

```rust
let model = self.run_model.clone().unwrap_or_else(|| {
    crate::run_model::RunModel::new(String::new(), wf_path.clone())
});
view::graph::render(&wf, &model)
```

Add a `run_model: Option<RunModel>` field to `WorkspaceWindow`. (Populated by the executor wiring in Task 15.)

- [ ] **Step 6: Run the workspace build + tests**

Run: `cargo build -p rupu-app && cargo test -p rupu-app-canvas`
Expected: clean build; all snapshot tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-app/src/view/graph.rs crates/rupu-app-canvas/src/git_graph.rs crates/rupu-app-canvas/tests/ crates/rupu-app/src/window/mod.rs
git commit -m "feat(rupu-app): Graph view reads NodeStatus from RunModel via status_lookup"
```

---

### Task 15: Wire `AppExecutor` into `WorkspaceWindow`

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs`
- Modify: `crates/rupu-app/src/main.rs` (instantiate `AppExecutor` at startup, share via context)

- [ ] **Step 1: Instantiate `AppExecutor` at app startup**

In `crates/rupu-app/src/main.rs`, before opening the workspace window, create:

```rust
use std::sync::Arc;
use rupu_app::executor::AppExecutor;
use rupu_orchestrator::runs::RunStore;

let run_store = Arc::new(RunStore::new(rupu_app::workspace::storage::runs_root()));
let app_executor = Arc::new(AppExecutor::new(run_store));
```

Pass `app_executor` into the `WorkspaceWindow::new(...)` constructor.

- [ ] **Step 2: Add `Arc<AppExecutor>` field to `WorkspaceWindow`**

In `crates/rupu-app/src/window/mod.rs`:

```rust
pub struct WorkspaceWindow {
    workspace: Workspace,
    app_executor: Arc<AppExecutor>,
    run_model: Option<RunModel>,
}
```

- [ ] **Step 3: Spawn the subscription task on workflow click**

When the user clicks a workflow in the sidebar, the window should:
1. Check `app_executor.list_active_runs(Some(workflow_path))` for any active run.
2. If active → call `app_executor.attach(&run_id).await` and consume the stream.
3. If not → leave `run_model` as `None` (static Graph view).

The stream consumer runs in a tokio task spawned via `gpui::Context::spawn`:

```rust
fn on_workflow_clicked(&mut self, workflow_path: PathBuf, cx: &mut Context<Self>) {
    let active = self.app_executor.list_active_runs(Some(workflow_path.clone()));
    if let Some(run) = active.into_iter().next() {
        let app_executor = self.app_executor.clone();
        let run_id = run.id.clone();
        cx.spawn(|this, mut cx| async move {
            if let Ok(mut stream) = app_executor.attach(&run_id).await {
                while let Some(ev) = stream.next().await {
                    cx.update(|cx| {
                        this.update(cx, |this, _| {
                            if let Some(m) = this.run_model.take() {
                                this.run_model = Some(m.apply(&ev));
                            }
                        });
                    });
                }
            }
        }).detach();
        self.run_model = Some(RunModel::new(run.id.clone(), workflow_path));
    } else {
        self.run_model = None;
    }
    cx.notify();
}
```

(Exact GPUI subscription pattern depends on the current GPUI version pinned in the workspace — match `crates/rupu-app/src/window/mod.rs`'s existing patterns.)

- [ ] **Step 4: Verify build**

Run: `cargo build -p rupu-app`
Expected: builds clean.

- [ ] **Step 5: Manual smoke**

Run: `cargo run --bin rupu -- run smoke -- input=hi` in one terminal.
In another, run: `cargo run --bin rupu-app`
Click the `smoke` workflow in the sidebar; observe the Graph view nodes light up (Active → Complete) as events arrive.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs crates/rupu-app/src/main.rs
git commit -m "feat(rupu-app): WorkspaceWindow subscribes to AppExecutor on workflow click"
```

---

### Task 16: Drill-down pane — transcript tail

**Files:**
- Create: `crates/rupu-app/src/view/transcript_tail.rs`
- Create: `crates/rupu-app/src/view/drilldown.rs`
- Modify: `crates/rupu-app/src/view/mod.rs`
- Modify: `crates/rupu-app/src/window/mod.rs` (horizontal split with drill-down)

- [ ] **Step 1: Create `transcript_tail.rs`**

```rust
//! Per-step transcript file watcher. Same notify-driven pattern as
//! FileTailRunSource, but the parser yields `TranscriptLine` instead
//! of `Event`. Watches `transcripts/<step_id>.jsonl`.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct TranscriptLine {
    pub kind: String,           // "tool_call", "tool_result", "agent_text"
    pub payload: serde_json::Value,
}

pub struct TranscriptTail {
    rx: mpsc::Receiver<TranscriptLine>,
    _watcher: notify::RecommendedWatcher,
}

impl TranscriptTail {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<TranscriptLine>(128);
        // Same pattern as FileTailRunSource — wait for file, drain
        // existing lines, install watcher.
        let parent: PathBuf = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let path_buf: PathBuf = path.to_path_buf();

        let tx_clone = tx.clone();
        tokio::spawn(async move {
            while !path_buf.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if let Ok(bytes) = std::fs::read(&path_buf) {
                for line in std::str::from_utf8(&bytes).unwrap_or("").lines() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        let kind = v
                            .get("kind")
                            .and_then(|k| k.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let _ = tx_clone
                            .send(TranscriptLine { kind, payload: v })
                            .await;
                    }
                }
            }
        });

        let tx_for_watcher = tx.clone();
        let path_for_watcher = path.to_path_buf();
        let mut offset: u64 = 0;
        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                if let Ok(evt) = res {
                    if matches!(evt.kind, notify::EventKind::Modify(_)) {
                        let Ok(bytes) = std::fs::read(&path_for_watcher) else { return; };
                        if (bytes.len() as u64) <= offset { return; }
                        let new = &bytes[offset as usize..];
                        for line in std::str::from_utf8(new).unwrap_or("").lines() {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                                let kind = v.get("kind").and_then(|k| k.as_str())
                                    .unwrap_or("unknown").to_string();
                                let _ = tx_for_watcher.blocking_send(TranscriptLine { kind, payload: v });
                            }
                        }
                        offset = bytes.len() as u64;
                    }
                }
            },
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(Self {
            rx,
            _watcher: watcher,
        })
    }
}

impl Stream for TranscriptTail {
    type Item = TranscriptLine;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<TranscriptLine>> {
        let this = self.get_mut();
        this.rx.poll_recv(cx)
    }
}
```

- [ ] **Step 2: Create `drilldown.rs`**

```rust
//! Drill-down pane — focused step's transcript stream + approval bar.

use gpui::{div, prelude::*, px, IntoElement, AnyElement};

use crate::palette;
use crate::run_model::RunModel;
use crate::view::transcript_tail::TranscriptLine;

pub fn render(model: &RunModel, transcript: &[TranscriptLine]) -> impl IntoElement {
    let focused_id = match &model.focused_step {
        Some(id) => id.clone(),
        None => return div().into_any_element(),
    };
    let status = model.nodes.get(&focused_id).copied();

    let mut pane = div()
        .flex()
        .flex_col()
        .w(px(420.0))
        .h_full()
        .bg(palette::BG_PANE)
        .border_l_1()
        .border_color(palette::BORDER);

    // Header
    pane = pane.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .px(px(16.0))
            .py(px(12.0))
            .child(div().text_color(palette::TEXT_PRIMARY).child(focused_id.clone()))
            .child(div().flex_grow())
            .child(div().text_color(palette::TEXT_DIM).child(
                format!("{:?}", status.unwrap_or(rupu_app_canvas::NodeStatus::Waiting)),
            )),
    );

    // Approval bar (when awaiting)
    if status == Some(rupu_app_canvas::NodeStatus::Awaiting) {
        pane = pane.child(approval_bar(&focused_id));
    }

    // Transcript lines
    let mut log = div().flex().flex_col().px(px(16.0)).py(px(8.0));
    for line in transcript {
        log = log.child(
            div()
                .text_color(palette::TEXT_PRIMARY)
                .font_family("Menlo")
                .text_sm()
                .child(format!("• {} {}", line.kind, line.payload)),
        );
    }
    pane = pane.child(log);

    pane.into_any_element()
}

fn approval_bar(_step_id: &str) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(8.0))
        .bg(palette::BG_ACCENT)
        .child(
            div()
                .px(px(12.0))
                .py(px(6.0))
                .bg(palette::GREEN_500)
                .text_color(palette::TEXT_ON_ACCENT)
                .child("Approve"),
        )
        .child(
            div()
                .px(px(12.0))
                .py(px(6.0))
                .bg(palette::RED_500)
                .text_color(palette::TEXT_ON_ACCENT)
                .child("Reject"),
        )
        .into_any_element()
}
```

(The on-click handlers for Approve / Reject buttons are wired in Task 17; here they're visual only.)

- [ ] **Step 3: Add the new view modules**

In `crates/rupu-app/src/view/mod.rs`:

```rust
pub mod drilldown;
pub mod graph;
pub mod transcript_tail;
```

- [ ] **Step 4: Wire drill-down into the window's main-area split**

In `crates/rupu-app/src/window/mod.rs`'s `render_main_for_workflow`, wrap the existing graph in a horizontal flex with the drill-down to the right:

```rust
div()
    .flex()
    .flex_row()
    .child(view::graph::render(&wf, &model))
    .child(view::drilldown::render(&model, &self.transcript_lines))
```

Add a `transcript_lines: Vec<TranscriptLine>` field on `WorkspaceWindow` and append to it from a per-focused-step `TranscriptTail` consumer spawned similarly to the event-stream consumer in Task 15. When `focused_step` changes, drop the old tail and spawn a new one for the new step's transcript path (use `RunRecord::active_step_transcript_path` or compute it from `transcripts/<step_id>.jsonl`).

- [ ] **Step 5: Verify build + manual smoke**

Run: `cargo build -p rupu-app`
Then run: `cargo run --bin rupu-app`. Start a workflow with `rupu run smoke` from another terminal. Click a step in the Graph view — drill-down opens, transcript lines stream in.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/view/ crates/rupu-app/src/window/mod.rs
git commit -m "feat(rupu-app): drill-down pane with transcript stream"
```

---

### Task 17: Approval buttons — inline on node + drill-down pane

**Files:**
- Modify: `crates/rupu-app/src/view/graph.rs` (inline buttons on Awaiting nodes)
- Modify: `crates/rupu-app/src/view/drilldown.rs` (wire approve/reject click handlers)
- Modify: `crates/rupu-app/src/window/mod.rs` (handlers call `AppExecutor::approve / reject`)

- [ ] **Step 1: Add inline buttons on Awaiting node rows**

In `crates/rupu-app/src/view/graph.rs::render_row`, after the existing cell loop, check whether the row corresponds to a step in `NodeStatus::Awaiting` and append two pill buttons:

```rust
if row.has_status(NodeStatus::Awaiting) {
    hbox = hbox
        .child(div().w(px(12.0))) // spacer
        .child(
            div()
                .px(px(8.0))
                .py(px(2.0))
                .bg(palette::GREEN_500)
                .text_color(palette::TEXT_ON_ACCENT)
                .child("✓")
                .id(SharedString::from(format!("approve-{}", row.step_id())))
                .on_click(|_, _, cx| {
                    // Bubble up via a window-level handler — set via
                    // EntityId::dispatch_action or a small callback
                    // registry. See Task 17 step 3.
                }),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(2.0))
                .bg(palette::RED_500)
                .text_color(palette::TEXT_ON_ACCENT)
                .child("✗")
                .id(SharedString::from(format!("reject-{}", row.step_id()))),
        );
}
```

This requires `GraphRow` to carry the step_id and status of its "anchor" step. Update `rupu-app-canvas::GraphRow` to optionally carry `(step_id: String, status: NodeStatus)`. If a row has no anchor (a pure-pipe / merge-line row), `has_status` returns false.

- [ ] **Step 2: Wire the click handlers via a window callback**

In `crates/rupu-app/src/window/mod.rs`, add methods:

```rust
fn handle_approve(&mut self, step_id: String, cx: &mut Context<Self>) {
    let Some(model) = &self.run_model else { return; };
    let run_id = model.run_id.clone();
    let app_exec = self.app_executor.clone();
    cx.spawn(|_, _| async move {
        if let Err(e) = app_exec.approve(&run_id, "rupu.app").await {
            tracing::error!(error = %e, "approve failed");
        }
    }).detach();
}

fn handle_reject(&mut self, step_id: String, reason: String, cx: &mut Context<Self>) {
    let Some(model) = &self.run_model else { return; };
    let run_id = model.run_id.clone();
    let app_exec = self.app_executor.clone();
    cx.spawn(|_, _| async move {
        if let Err(e) = app_exec.reject(&run_id, &reason).await {
            tracing::error!(error = %e, "reject failed");
        }
    }).detach();
}
```

Pass these as `Arc<Fn>` callbacks down into `view::graph::render` and `view::drilldown::render` so the buttons can invoke them.

- [ ] **Step 3: Wire drill-down Approve/Reject buttons**

Replace the placeholder `approval_bar()` in `drilldown.rs` step 2 of Task 16 with click handlers wired to the same callbacks. Reject opens a small text input above the buttons; on submit, calls the reject callback with the typed reason.

- [ ] **Step 4: Add a keyboard handler for `a` / `r`**

In `WorkspaceWindow::render`, attach a global keyboard handler:

```rust
.on_action::<ApproveFocused>(cx.listener(|this, _, cx| {
    if let Some(step) = this.run_model.as_ref().and_then(|m| m.focused_step.clone()) {
        this.handle_approve(step, cx);
    }
}))
.on_action::<RejectFocused>(cx.listener(|this, _, cx| {
    if let Some(step) = this.run_model.as_ref().and_then(|m| m.focused_step.clone()) {
        this.handle_reject(step, "rejected via keyboard".into(), cx);
    }
}))
```

Define `ApproveFocused` and `RejectFocused` as zero-field GPUI actions bound to `a` and `r` keys in the app menu.

- [ ] **Step 5: Manual smoke**

Run an `ask`-mode workflow. Verify clicking inline ✓ approves (run continues), clicking inline ✗ rejects, clicking drill-down Approve / Reject works, `a` / `r` keyboard shortcuts work.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/view/ crates/rupu-app/src/window/mod.rs crates/rupu-app/src/menu/
git commit -m "feat(rupu-app): approval UI — inline on node + drill-down + keyboard"
```

---

### Task 18: Sidebar status dots + Run button toolbar

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs`
- Modify: `crates/rupu-app/src/window/mod.rs` (Run button in graph toolbar)

- [ ] **Step 1: Sidebar status dots**

In `crates/rupu-app/src/window/sidebar.rs`, for each workflow row, query `app_executor.list_active_runs(Some(path))` and pick the most recent active run's status:

```rust
fn status_dot(status: Option<RunStatus>) -> AnyElement {
    let color = match status {
        Some(RunStatus::Running) => palette::BLUE_500,
        Some(RunStatus::AwaitingApproval) => palette::YELLOW_500,
        Some(RunStatus::Failed) => palette::RED_500,
        _ => return div().w(px(8.0)).h(px(8.0)).into_any_element(),
    };
    div()
        .w(px(8.0))
        .h(px(8.0))
        .rounded_full()
        .bg(color)
        .into_any_element()
}
```

Append this to the workflow row's flex layout. The Running dot pulses via a 1Hz CSS-like animation; skip that polish if GPUI doesn't expose CSS-keyframe-style animations easily — a steady dot is acceptable for D-3.

- [ ] **Step 2: Run button in graph toolbar**

In `WorkspaceWindow::render_main_for_workflow`, add a toolbar above the graph:

```rust
div()
    .flex()
    .flex_row()
    .px(px(24.0))
    .py(px(8.0))
    .bg(palette::BG_PRIMARY)
    .border_b_1()
    .border_color(palette::BORDER)
    .child(div().flex_grow().child(wf.name.clone()))
    .child(
        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(if has_active_run { palette::BG_DISABLED } else { palette::BLUE_500 })
            .text_color(palette::TEXT_ON_ACCENT)
            .child("Run")
            .id("run-workflow")
            .on_click(cx.listener(|this, _, cx| {
                this.handle_run_clicked(cx);
            })),
    )
```

`handle_run_clicked` spawns:

```rust
fn handle_run_clicked(&mut self, cx: &mut Context<Self>) {
    let Some(path) = self.current_workflow_path() else { return; };
    let app_exec = self.app_executor.clone();
    cx.spawn(|this, mut cx| async move {
        if let Ok(run_id) = app_exec.start_workflow(path.clone()).await {
            // Attach immediately so events flow into the same model
            cx.update(|cx| {
                this.update(cx, |this, _| {
                    this.run_model = Some(RunModel::new(run_id.clone(), path.clone()));
                });
            });
            if let Ok(mut stream) = app_exec.attach(&run_id).await {
                while let Some(ev) = stream.next().await {
                    cx.update(|cx| {
                        this.update(cx, |this, _| {
                            if let Some(m) = this.run_model.take() {
                                this.run_model = Some(m.apply(&ev));
                            }
                        });
                    });
                }
            }
        }
    }).detach();
}
```

- [ ] **Step 3: Manual smoke**

Run `cargo run --bin rupu-app`. Click a workflow → static graph + Run button. Click Run → nodes light up live.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/window/
git commit -m "feat(rupu-app): sidebar status dots + Run button"
```

---

### Task 19: Menubar badge — pending approvals

**Files:**
- Modify: `crates/rupu-app/src/menu/menubar.rs`

- [ ] **Step 1: Poll the executor for pending approvals**

In `crates/rupu-app/src/menu/menubar.rs`, where the menubar stub currently exists (D-1 left a placeholder), add a tokio task that polls `app_executor.list_active_runs(None)` every 2 seconds and counts runs in `AwaitingApproval` status:

```rust
pub fn spawn_badge_updater(app_executor: Arc<AppExecutor>, item: Arc<NSStatusItem>) {
    tokio::spawn(async move {
        loop {
            let count = app_executor
                .list_active_runs(None)
                .into_iter()
                .filter(|r| r.status == RunStatus::AwaitingApproval)
                .count();
            update_badge_label(&item, count);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

fn update_badge_label(item: &NSStatusItem, count: usize) {
    let label = if count == 0 { "rupu".to_string() } else { format!("rupu ({count})") };
    // Use the existing NSStatusItem button.setTitle() FFI call from
    // the D-1 stub.
}
```

Wire `spawn_badge_updater` from `main.rs` after the status item is created.

- [ ] **Step 2: Manual smoke**

Run an `ask`-mode workflow, observe the menubar text changes from `rupu` → `rupu (1)` once the step pauses.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/menu/menubar.rs crates/rupu-app/src/main.rs
git commit -m "feat(rupu-app): menubar badge counts pending approvals"
```

---

### Task 20: `make app-smoke` — scripted live run

**Files:**
- Modify: `Makefile`
- Create: `crates/rupu-app/tests/smoke_live_run.rs` (or extend existing app-smoke harness)

- [ ] **Step 1: Extend the `app-smoke` make target**

In `Makefile`, find the existing `app-smoke` target. Extend it to:

1. Launch `rupu-app` in the background.
2. Run a known sample workflow via `rupu run smoke -- input=hi`.
3. Assert the log line `opened workspace` and additionally:
   - `Event::RunStarted ... run_id=run_`
   - `Event::StepStarted ... step_id=`
   - `Event::RunCompleted ... status=completed`

```make
app-smoke:
	@RUST_LOG=info,rupu_app=debug timeout 30s cargo run --bin rupu-app &
	@APP_PID=$$!
	@sleep 3
	@cargo run --bin rupu -- run smoke -- input=hi 2>&1 | tee /tmp/rupu-smoke.log
	@grep -q '"type":"run_completed"' /tmp/rupu-smoke.log || (echo "missing RunCompleted" && exit 1)
	@kill $$APP_PID 2>/dev/null || true
```

- [ ] **Step 2: Run app-smoke**

Run: `make app-smoke`
Expected: exits 0; log shows the full event sequence.

- [ ] **Step 3: Commit**

```bash
git add Makefile
git commit -m "test(rupu-app): app-smoke target asserts live-run event sequence"
```

---

### Task 21: Workspace gates + CLAUDE.md update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Run the full workspace gates**

Run:
```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
make app-smoke
```
Expected: all pass.

- [ ] **Step 2: Update `CLAUDE.md`**

Find the "Read first" section and add:

```markdown
- Slice D Plan 3 (live executor + graph pulse, complete): `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-3-live-executor.md`
```

In the crate descriptions, add a paragraph in the `rupu-orchestrator` entry:

```markdown
  - **Executor module** (`crates/rupu-orchestrator/src/executor/`): WorkflowExecutor + EventSink + Event traits; InProcessExecutor + InMemorySink (broadcast) + JsonlSink (events.jsonl) + FileTailRunSource impls. Both rupu-app and rupu-cli route through these.
```

In the `rupu-app` description (which doesn't yet exist as a CLAUDE.md entry — add it now):

```markdown
- **`rupu-app`** — native macOS desktop app via GPUI. Owns an `AppExecutor` (wrapping `InProcessExecutor`) that starts workflows in-process AND tails disk runs via `FileTailRunSource`. `RunModel::apply(Event)` mutates per-run state; the Graph view paints `NodeStatus` per node; the drill-down pane streams the focused step's transcript and exposes Approve / Reject buttons. Same Approve / Reject buttons also render inline on Awaiting nodes in the Graph view.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md — Slice D Plan 3 pointer + executor module description"
```

---

## Self-review notes

This plan covers the spec's full scope (one plan, full ride per matt's call). Mapping back:

| Spec section | Plan task(s) |
|---|---|
| Trait surface (WorkflowExecutor, EventSink, Event) | 2, 3, 4, 9 |
| JsonlSink + InMemorySink | 5, 6 |
| Runner wiring (event_sink field + emit calls) | 7 |
| StepWorking from `on_tool_call` | 8 |
| InProcessExecutor (start/list_runs/tail/approve/reject/cancel) | 9, 9b |
| FileTailRunSource | 10 |
| CLI refactor (rupu run + rupu watch) | 11 |
| AppExecutor + attach decision | 12 |
| RunModel + apply | 13 |
| Graph view consumes RunModel | 14, 15 |
| Drill-down pane + transcript tail | 16 |
| Inline + drill-down approval UI | 17 |
| Sidebar status dots + Run button | 18 |
| Menubar badge wired to pending approvals | 19 |
| `make app-smoke` extended | 20 |
| CLAUDE.md + workspace gates | 21 |

Acceptance criteria 1-4 from the spec are all covered by Tasks 11, 15, 18, 20.

Out-of-scope items from the spec stay out-of-scope here — no tasks for status pulse animations, ForEach/Parallel fan-out, run history, multi-run concurrency UI, or global approvals inbox.
