# SSH Lazy Transcript Mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every transcript produced on an SSH host is visible in rupu.app and the web CP: live while it runs, complete after it finishes, readable with the host offline, and counted by the usage rollup.

**Architecture:** Remote step transcripts are served from a coordinator-local cache under `<global>/mirror/<host_id>/transcripts/`, filled lazily by one shared `tail -F` over ssh while a viewer is focused on the step, and filled authoritatively by a one-shot batched `cat` when the tail pump sees the run go terminal. Placed units inside a local workflow reuse the run id the coordinator already mints so their PR #646 mirror path is known before dispatch. Both frontends keep the existing `/api/transcript` + `/api/transcript/stream` contract, gaining an optional `run=` parameter and an optional `partial` response field.

**Tech Stack:** Rust (tokio, axum, futures-util), Swift 6 / SwiftUI / Swift Testing, React + TypeScript (vitest).

**Spec:** `docs/superpowers/specs/2026-09-04-rupu-ssh-lazy-transcript-mirror-design.md`

## Global Constraints

- Workspace deps only: versions pinned in root `Cargo.toml`, never in a crate `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- `rupu-cli` stays thin: no business logic in the CLI crate beyond the dispatcher adapter.
- Never run package-wide `cargo fmt`; format only the files you touched (`rustfmt <file>`), because `main` is fmt-dirty under the pinned toolchain.
- No `git stash` / `git stash pop`; use a WIP commit if you must set work aside.
- Swift: Swift Testing (not XCTest), no new third-party dependencies, `VersionGate.minimum` stays `"0.74.0"`.
- Every new query parameter and response field is optional (§12): old clients and old servers keep today's behaviour.
- Placed-unit deviation from spec §7, applied throughout: `UnitDispatch.run_id` is **already** the per-unit run id the coordinator mints (`run_<ULID>`, minted at `runner.rs` ~6106 for placed steps and ~6581 for fan-out units), and `ItemResult.run_id` / `UnitCheckpoint.run_id` already carry it. No `unit_run_id` field is added; the existing field is documented and reused.
- The `<global>/mirror` cache root already lies inside `/api/transcript`'s allowed roots (`allowed_roots` starts with `s.global_dir`); no roots change is needed.

---

### Task 1: `recorded_transcript_paths` + cache key mapping

**Files:**
- Create: `crates/rupu-cp/src/host/transcript_paths.rs`
- Modify: `crates/rupu-cp/src/host/mod.rs` (add `pub mod transcript_paths;`)

**Interfaces:**
- Produces:
  - `pub fn recorded_transcript_paths(store: &RunStore, run_id: &str) -> Vec<PathBuf>` — deduplicated, sorted, non-empty paths a mirrored run's artifacts claim (§3.4).
  - `pub fn cache_key(recorded: &Path) -> Option<String>` — §3.1 key for a remote transcript path; `None` for anything that is not a transcript-shaped absolute `.jsonl` path.
  - `pub fn cache_path(global: &Path, host_id: &str, recorded: &Path) -> Option<PathBuf>` — `<global>/mirror/<host_id>/transcripts/<key>.jsonl`.
  - `pub fn complete_marker(cache: &Path) -> PathBuf` — `<cache>.complete`.
  - `pub fn is_complete(cache: &Path) -> bool`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cp/src/host/transcript_paths.rs` with only the test module first:

```rust
//! Which transcript files a mirrored run claims, and how a remote transcript
//! path maps to a coordinator-local cache file (spec §3.1, §3.4). Shared by
//! the SSH read path's allowlist and the tail pump's terminal pull so the two
//! can never disagree about what a run owns.

use std::collections::BTreeSet;
use std::io::{BufRead as _, BufReader};
use std::path::{Component, Path, PathBuf};

use rupu_orchestrator::{executor::Event, runs::RunStore};

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::runs::{ItemResultRecord, StepKind, StepResultRecord, UnitCheckpoint};
    use rupu_orchestrator::{RunRecord, RunStatus};
    use std::io::Write as _;

    fn store() -> (RunStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        (RunStore::new(tmp.path().join("runs")), tmp)
    }

    fn record(id: &str) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: "wf".into(),
            status: RunStatus::Running,
            inputs: Default::default(),
            event: None,
            workspace_id: String::new(),
            workspace_path: PathBuf::from("."),
            transcript_dir: PathBuf::from("/remote/.rupu/transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: Some("host_abc".into()),
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: Some("build".into()),
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: Some(PathBuf::from(
                "/remote/proj/.rupu/transcripts/run_01ACTIVE.jsonl",
            )),
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
        }
    }

    #[test]
    fn recorded_paths_union_every_artifact_and_dedup() {
        let (store, _tmp) = store();
        let id = "run_01RECORDED";
        store.create(record(id), "").unwrap();
        store
            .append_step_result(
                id,
                &StepResultRecord {
                    step_id: "plan".into(),
                    run_id: "run_01PLAN".into(),
                    transcript_path: PathBuf::from("/remote/proj/.rupu/transcripts/run_01PLAN.jsonl"),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: StepKind::ForEach,
                    items: vec![ItemResultRecord {
                        index: 0,
                        item: serde_json::Value::Null,
                        sub_id: String::new(),
                        rendered_prompt: String::new(),
                        run_id: "run_01ITEM".into(),
                        transcript_path: PathBuf::from("/remote/proj/.rupu/transcripts/run_01ITEM.jsonl"),
                        output: String::new(),
                        success: true,
                        is_fixer: false,
                    }],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    run_outcome: None,
                    host: None,
                },
            )
            .unwrap();
        store
            .append_unit_checkpoint(
                id,
                &UnitCheckpoint {
                    step_id: "plan".into(),
                    index: 1,
                    item: serde_json::Value::Null,
                    run_id: "run_01CKPT".into(),
                    transcript_path: PathBuf::from("/remote/proj/.rupu/transcripts/run_01CKPT.jsonl"),
                    output: String::new(),
                    success: true,
                    finished_at: chrono::Utc::now(),
                    host: Some("host_abc".into()),
                },
            )
            .unwrap();
        // events.jsonl: a StepWorking with a path, one without, a UnitStarted,
        // and a duplicate of the step-result path.
        let mut f = std::fs::File::create(store.events_path(id)).unwrap();
        for ev in [
            Event::StepWorking {
                run_id: id.into(),
                step_id: "build".into(),
                note: None,
                transcript_path: Some(PathBuf::from("/remote/proj/.rupu/transcripts/run_01WORK.jsonl")),
            },
            Event::StepWorking {
                run_id: id.into(),
                step_id: "build".into(),
                note: Some("tool".into()),
                transcript_path: None,
            },
            Event::UnitStarted {
                run_id: id.into(),
                step_id: "panel".into(),
                index: 0,
                unit_key: "a".into(),
                agent: None,
                transcript_path: PathBuf::from("/remote/.rupu/runs/run_01RECORDED/sub_runs/sub_01P/transcript.jsonl"),
                host: None,
            },
            Event::StepWorking {
                run_id: id.into(),
                step_id: "plan".into(),
                note: None,
                transcript_path: Some(PathBuf::from("/remote/proj/.rupu/transcripts/run_01PLAN.jsonl")),
            },
        ] {
            writeln!(f, "{}", serde_json::to_string(&ev).unwrap()).unwrap();
        }

        let got = recorded_transcript_paths(&store, id);
        let want: Vec<PathBuf> = [
            "/remote/.rupu/runs/run_01RECORDED/sub_runs/sub_01P/transcript.jsonl",
            "/remote/proj/.rupu/transcripts/run_01ACTIVE.jsonl",
            "/remote/proj/.rupu/transcripts/run_01CKPT.jsonl",
            "/remote/proj/.rupu/transcripts/run_01ITEM.jsonl",
            "/remote/proj/.rupu/transcripts/run_01PLAN.jsonl",
            "/remote/proj/.rupu/transcripts/run_01WORK.jsonl",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn recorded_paths_of_unknown_or_empty_run_is_empty() {
        let (store, _tmp) = store();
        assert!(recorded_transcript_paths(&store, "run_01NOPE").is_empty());
        let mut rec = record("run_01EMPTY");
        rec.active_step_transcript_path = Some(PathBuf::new());
        store.create(rec, "").unwrap();
        assert!(recorded_transcript_paths(&store, "run_01EMPTY").is_empty(), "empty paths are dropped");
    }

    #[test]
    fn cache_key_maps_step_unit_and_sub_run_shapes() {
        assert_eq!(
            cache_key(Path::new("/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl")).as_deref(),
            Some("run_01STEP")
        );
        assert_eq!(
            cache_key(Path::new("/home/ci/.rupu/transcripts/run_01AGENT.jsonl")).as_deref(),
            Some("run_01AGENT")
        );
        assert_eq!(
            cache_key(Path::new("/home/ci/.rupu/runs/run_01P/sub_runs/sub_01X/transcript.jsonl")).as_deref(),
            Some("run_01P__sub_runs__sub_01X")
        );
    }

    #[test]
    fn cache_key_rejects_non_transcript_shapes() {
        for bad in [
            "relative/.rupu/transcripts/run_01A.jsonl",
            "/etc/passwd",
            "/home/ci/.rupu/transcripts/run_01A.json",
            "/home/ci/.rupu/transcripts/../secrets/run_01A.jsonl",
            "/home/ci/notes/run_01A.jsonl",
            "/home/ci/.rupu/transcripts/we ird.jsonl",
            "/home/ci/.rupu/runs/run_01P/sub_runs/transcript.jsonl",
        ] {
            assert_eq!(cache_key(Path::new(bad)), None, "{bad} must not map");
        }
    }

    #[test]
    fn cache_path_and_complete_marker_layout() {
        let cache = cache_path(
            Path::new("/g"),
            "host_abc",
            Path::new("/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl"),
        )
        .unwrap();
        assert_eq!(cache, PathBuf::from("/g/mirror/host_abc/transcripts/run_01STEP.jsonl"));
        assert_eq!(
            complete_marker(&cache),
            PathBuf::from("/g/mirror/host_abc/transcripts/run_01STEP.jsonl.complete")
        );
        let tmp = tempfile::tempdir().unwrap();
        let c = tmp.path().join("x.jsonl");
        assert!(!is_complete(&c));
        std::fs::write(complete_marker(&c), b"").unwrap();
        assert!(is_complete(&c));
    }
}
```

Note: `StepResultRecord.host` does not exist until Task 8. For this task's test, omit the `host: None` line; Task 8 adds it back when the field lands (the test module is listed there).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp transcript_paths 2>&1 | tail -20`
Expected: compile errors, `recorded_transcript_paths` / `cache_key` not found.

- [ ] **Step 3: Implement**

Add above the test module:

```rust
/// Every transcript path a mirrored run's own artifacts claim (§3.4):
/// step-result rows and their items, unit checkpoints, `StepWorking` /
/// `UnitStarted` events, and the record's active-step path. Deduplicated,
/// sorted, empty paths dropped. Missing artifacts contribute nothing.
pub fn recorded_transcript_paths(store: &RunStore, run_id: &str) -> Vec<PathBuf> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    if let Ok(rec) = store.load(run_id) {
        if let Some(p) = rec.active_step_transcript_path {
            out.insert(p);
        }
    }
    for sr in store.read_step_results(run_id).unwrap_or_default() {
        out.insert(sr.transcript_path.clone());
        for item in &sr.items {
            out.insert(item.transcript_path.clone());
        }
    }
    for cp in store.read_unit_checkpoints(run_id).unwrap_or_default() {
        out.insert(cp.transcript_path.clone());
    }
    if let Ok(file) = std::fs::File::open(store.events_path(run_id)) {
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Event>(&line) {
                Ok(Event::StepWorking { transcript_path: Some(p), .. }) => {
                    out.insert(p);
                }
                Ok(Event::UnitStarted { transcript_path, .. }) => {
                    out.insert(transcript_path);
                }
                _ => {}
            }
        }
    }
    out.retain(|p| !p.as_os_str().is_empty());
    out.into_iter().collect()
}

/// A file-name component we are willing to turn into a cache key: ULID-style
/// run ids and sub-run ids only. Anything else is not a transcript we serve.
fn safe_component(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// §3.1: `…/transcripts/<name>.jsonl` → `<name>`;
/// `…/runs/<parent>/sub_runs/<sub>/transcript.jsonl` → `<parent>__sub_runs__<sub>`.
/// `None` for anything else — relative paths, `..`, non-`.jsonl`, or a
/// directory shape this design does not serve.
pub fn cache_key(recorded: &Path) -> Option<String> {
    if !recorded.is_absolute() {
        return None;
    }
    if recorded.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return None;
    }
    if recorded.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let comps: Vec<&str> = recorded
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let n = comps.len();
    if n >= 2 && comps[n - 2] == "transcripts" {
        let stem = recorded.file_stem()?.to_str()?;
        return safe_component(stem).then(|| stem.to_string());
    }
    if n >= 4 && comps[n - 1] == "transcript.jsonl" && comps[n - 3] == "sub_runs" {
        let (parent, sub) = (comps[n - 4], comps[n - 2]);
        if safe_component(parent) && safe_component(sub) {
            return Some(format!("{parent}__sub_runs__{sub}"));
        }
    }
    None
}

/// `<global>/mirror/<host_id>/transcripts/<key>.jsonl`.
pub fn cache_path(global: &Path, host_id: &str, recorded: &Path) -> Option<PathBuf> {
    let key = cache_key(recorded)?;
    Some(
        global
            .join("mirror")
            .join(host_id)
            .join("transcripts")
            .join(format!("{key}.jsonl")),
    )
}

/// The `.complete` sidecar (§6.1) that marks a cache file authoritative.
pub fn complete_marker(cache: &Path) -> PathBuf {
    PathBuf::from(format!("{}.complete", cache.display()))
}

pub fn is_complete(cache: &Path) -> bool {
    complete_marker(cache).is_file()
}
```

Add `pub mod transcript_paths;` to `crates/rupu-cp/src/host/mod.rs` (alphabetical, after `summary_build`).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp transcript_paths 2>&1 | tail -20`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/host/transcript_paths.rs
git add crates/rupu-cp/src/host/transcript_paths.rs crates/rupu-cp/src/host/mod.rs
git commit -m "feat(cp): recorded transcript paths + remote→cache key mapping for SSH transcripts

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Host-neutral connector hooks

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (imports at top; trait body after `get_transcript`, ~line 284)

**Interfaces:**
- Produces:
  - `pub struct FeedGuard` with `FeedGuard::noop()` and `FeedGuard::holding(Box<dyn Any + Send + Sync>)`.
  - Trait defaults on `HostConnector`:
    - `fn local_transcript_path(&self, recorded: &Path) -> PathBuf` (identity)
    - `async fn ensure_transcript_feed(&self, run_id: &str, recorded: &Path) -> Result<FeedGuard, HostConnectorError>` (`Unsupported`)
    - `async fn pull_transcript(&self, run_id: &str, recorded: &Path, terminal: bool) -> Result<(), HostConnectorError>` (`Unsupported`)

- [ ] **Step 1: Write the failing test**

Append to the existing `mod tests` at the bottom of `connector.rs` (it already defines a minimal mock connector; add a test that only uses the defaults):

```rust
    #[tokio::test]
    async fn transcript_hooks_default_to_identity_and_unsupported() {
        struct Bare;
        #[async_trait::async_trait]
        impl HostConnector for Bare {
            async fn info(&self) -> Result<HostInfo, HostConnectorError> { unimplemented!() }
            async fn launch_run(&self, _r: LaunchRequest) -> Result<String, HostConnectorError> { unimplemented!() }
            async fn launch_agent(&self, _r: AgentLaunchRequest) -> Result<String, HostConnectorError> { unimplemented!() }
            async fn start_session(&self, _r: SessionStartRequest) -> Result<String, HostConnectorError> { unimplemented!() }
            async fn send_session_turn(&self, _r: SendMessageRequest) -> Result<String, HostConnectorError> { unimplemented!() }
            async fn list_runs(&self, _q: RunListQuery) -> Result<Vec<serde_json::Value>, HostConnectorError> { unimplemented!() }
            async fn get_run(&self, _id: &str) -> Result<serde_json::Value, HostConnectorError> { unimplemented!() }
            async fn approve_run(&self, _id: &str, _m: &str) -> Result<(), HostConnectorError> { unimplemented!() }
            async fn reject_run(&self, _id: &str, _r: Option<&str>) -> Result<(), HostConnectorError> { unimplemented!() }
            async fn cancel_run(&self, _id: &str) -> Result<(), HostConnectorError> { unimplemented!() }
            async fn stream_run_events(&self, _id: &str) -> Result<EventByteStream, HostConnectorError> { unimplemented!() }
            async fn get_transcript(&self, _p: &str) -> Result<serde_json::Value, HostConnectorError> { unimplemented!() }
            async fn proxy_get_json(&self, _p: &str) -> Result<serde_json::Value, HostConnectorError> { unimplemented!() }
        }
        let c = Bare;
        let p = std::path::Path::new("/remote/.rupu/transcripts/run_01A.jsonl");
        assert_eq!(c.local_transcript_path(p), p.to_path_buf());
        assert!(matches!(c.ensure_transcript_feed("run_01R", p).await, Err(HostConnectorError::Unsupported(_))));
        assert!(matches!(c.pull_transcript("run_01R", p, true).await, Err(HostConnectorError::Unsupported(_))));
        let _ = FeedGuard::noop();
    }
```

Match the `reject_run` signature to the one already used by the existing mock in that test module (copy it verbatim; the parameter shape there is authoritative).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cp transcript_hooks_default 2>&1 | tail -20`
Expected: compile error, `local_transcript_path` not a member.

- [ ] **Step 3: Implement**

Imports: add `use std::path::{Path, PathBuf};` next to the existing `std` imports at the top of `connector.rs`.

Add after `EventByteStream`'s definition:

```rust
/// Keeps a remote→local transcript feed alive for as long as it lives (spec
/// §5). Dropping it releases the holder's interest; a connector stops the
/// underlying feed once the last guard for a file is gone. Connectors whose
/// recorded paths are already local hand back [`FeedGuard::noop`].
pub struct FeedGuard {
    _release: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl FeedGuard {
    pub fn noop() -> Self {
        Self { _release: None }
    }

    /// Hold `inner` (typically an `Arc` refcount on a shared feed) until drop.
    pub fn holding(inner: Box<dyn std::any::Any + Send + Sync>) -> Self {
        Self {
            _release: Some(inner),
        }
    }
}
```

Add to the `HostConnector` trait, directly after `get_transcript`:

```rust
    /// Map a transcript path *as recorded by this host's run artifacts* to
    /// the coordinator-local file that serves it (spec §3.2). Identity for
    /// hosts whose recorded paths are already local (Local, HTTP — the
    /// latter forwards reads to the remote CP instead). Mirror-backed
    /// transports return their cache path.
    fn local_transcript_path(&self, recorded: &Path) -> PathBuf {
        recorded.to_path_buf()
    }

    /// Ensure `recorded` (a path this host wrote, claimed by `run_id`'s own
    /// artifacts) is being fed into [`Self::local_transcript_path`] for as
    /// long as the returned guard lives. Default: unsupported.
    async fn ensure_transcript_feed(
        &self,
        _run_id: &str,
        _recorded: &Path,
    ) -> Result<FeedGuard, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "transcript feed is not supported for this host type".into(),
        ))
    }

    /// One-shot pull of `recorded` into its local counterpart. `terminal`
    /// marks the copy authoritative (spec §6.1). Default: unsupported.
    async fn pull_transcript(
        &self,
        _run_id: &str,
        _recorded: &Path,
        _terminal: bool,
    ) -> Result<(), HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "transcript pull is not supported for this host type".into(),
        ))
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp transcript_hooks_default 2>&1 | tail -5` then `cargo build -p rupu-cp -p rupu-cli 2>&1 | tail -3`
Expected: test passes; every existing connector (local/http/tunnel/bucket/ssh + all test mocks) still compiles because the methods are defaulted.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/host/connector.rs
git add crates/rupu-cp/src/host/connector.rs
git commit -m "feat(cp): host-neutral transcript feed/pull hooks on HostConnector

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: SSH cache mapping, allowlist, and one-shot pull

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs` (imports ~line 18–32; `impl SshHostConnector` after `agent_argv` ~line 970; `impl HostConnector for SshHostConnector` after `get_transcript` ~line 1997; `FakeExec` in `mod tests` ~line 2700–2870)

**Interfaces:**
- Consumes: Task 1's `cache_path`, `recorded_transcript_paths`, `complete_marker`, `is_complete`; Task 2's trait hooks.
- Produces:
  - `SshHostConnector::global_dir(&self) -> PathBuf` (private) — `run_store.root.parent()`, the same derivation `NodeMirror::transcript_mirror_path` uses.
  - `SshHostConnector::authorize_remote_transcript(&self, run_id, recorded) -> Result<PathBuf, HostConnectorError>` (`pub(crate)`) — §3.3 checks; returns the cache path.
  - `pub(crate) fn write_cache_file(cache: &Path, body: &str, complete: bool) -> Result<(), HostConnectorError>` — tmp + rename + optional sidecar.
  - `pub(crate) fn single_cat_command(remote: &str) -> String` — `cat '<p>' 2>/dev/null || printf '__RUPU_NO_FILE__\n'`.
  - `HostConnector::local_transcript_path` / `pull_transcript` implemented for SSH.
  - `FakeExec` gains `batch_cat_stdout: Option<String>` (used by Task 6) returned for commands starting with `for p in`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `ssh.rs` (after `launch_run_mints_creates_mirror_and_dispatches`):

```rust
    /// Seed a mirrored run owned by `host_abc` whose step_results claim
    /// `remote` — the shape every allowlist test starts from.
    fn seed_claimed_run(
        conn: &SshHostConnector,
        run_store: &rupu_orchestrator::RunStore,
        run_id: &str,
        remote: &str,
        terminal: bool,
    ) {
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "wf".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror.create_run(run_id, &conn.host_id, &spec).unwrap();
        run_store
            .append_step_result(
                run_id,
                &rupu_orchestrator::runs::StepResultRecord {
                    step_id: "build".into(),
                    run_id: "run_01STEPX".into(),
                    transcript_path: std::path::PathBuf::from(remote),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: Default::default(),
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    run_outcome: None,
                },
            )
            .unwrap();
        if terminal {
            conn.mirror.finish(run_id, &conn.host_id, "completed").unwrap();
        }
    }

    #[test]
    fn ssh_local_transcript_path_maps_into_the_host_cache() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let recorded = std::path::Path::new("/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl");
        assert_eq!(
            conn.local_transcript_path(recorded),
            tmp.path().join("mirror/host_abc/transcripts/run_01STEP.jsonl")
        );
        // A non-transcript path maps to itself, so the handler's "is it a
        // local file" check fails and the request falls to the run-scoped
        // branch, which rejects it.
        let odd = std::path::Path::new("/etc/passwd");
        assert_eq!(conn.local_transcript_path(odd), odd.to_path_buf());
    }

    #[tokio::test]
    async fn ssh_pull_transcript_requires_the_run_to_claim_the_path() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01OWNED", claimed, false);

        let other = std::path::Path::new("/home/ci/proj/.rupu/transcripts/run_01OTHER.jsonl");
        let err = conn.pull_transcript("run_01OWNED", other, false).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(ref m) if m.contains("did not record")), "{err}");

        let err = conn
            .pull_transcript("run_01MISSING", std::path::Path::new(claimed), false)
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::NotFound(_)), "{err}");

        // Nothing was shelled for a refused pull.
        assert!(fake.commands.lock().unwrap().iter().all(|c| !c.starts_with("cat ")));
    }

    #[tokio::test]
    async fn ssh_pull_transcript_rejects_a_run_owned_by_another_host() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01FOREIGN", claimed, false);
        let mut rec = run_store.load("run_01FOREIGN").unwrap();
        rec.worker_id = Some("host_other".into());
        run_store.update(&rec).unwrap();

        let err = conn
            .pull_transcript("run_01FOREIGN", std::path::Path::new(claimed), false)
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(ref m) if m.contains("does not belong")), "{err}");
    }

    #[tokio::test]
    async fn ssh_pull_transcript_writes_cache_and_marks_complete_only_when_terminal() {
        let mut fake = FakeExec::ok(vec![]);
        fake.cat_transcript_stdout = Some("{\"type\":\"run_start\"}\n{\"type\":\"turn_start\"}\n".into());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01PULL", claimed, false);
        let cache = tmp.path().join("mirror/host_abc/transcripts/run_01STEP.jsonl");

        conn.pull_transcript("run_01PULL", std::path::Path::new(claimed), false).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n{\"type\":\"turn_start\"}\n"
        );
        assert!(!crate::host::transcript_paths::is_complete(&cache), "non-terminal pull is a snapshot");
        assert!(!cache.with_extension("jsonl.tmp").exists(), "tmp file renamed away");

        conn.pull_transcript("run_01PULL", std::path::Path::new(claimed), true).await.unwrap();
        assert!(crate::host::transcript_paths::is_complete(&cache));

        let cmds = fake.commands.lock().unwrap();
        let cat = cmds.iter().find(|c| c.starts_with("cat ")).expect("a cat was issued");
        assert!(cat.contains(&format!("'{claimed}'")), "path must be single-quoted: {cat}");
        assert!(cat.contains("__RUPU_NO_FILE__"), "absent-file sentinel present: {cat}");
    }

    #[tokio::test]
    async fn ssh_pull_transcript_absent_remote_file_is_an_empty_complete_answer() {
        let mut fake = FakeExec::ok(vec![]);
        fake.cat_transcript_stdout = Some("__RUPU_NO_FILE__\n".into());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01NONE.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01ABSENT", claimed, false);
        let cache = tmp.path().join("mirror/host_abc/transcripts/run_01NONE.jsonl");

        conn.pull_transcript("run_01ABSENT", std::path::Path::new(claimed), true).await.unwrap();
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "");
        assert!(crate::host::transcript_paths::is_complete(&cache));
    }

    #[tokio::test]
    async fn ssh_pull_transcript_offline_is_unreachable_and_leaves_no_cache() {
        let fake = std::sync::Arc::new(FakeExec::offline("ssh: connect to host mini port 22: No route"));
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01OFF.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01OFFLINE", claimed, false);

        let err = conn
            .pull_transcript("run_01OFFLINE", std::path::Path::new(claimed), true)
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Unreachable(_)), "{err}");
        assert!(!tmp.path().join("mirror/host_abc/transcripts/run_01OFF.jsonl").exists());
    }
```

`FakeExec::offline` returns `success: false` with empty stdout for every command, which is exactly what a real `ssh` exit-255 looks like through `RemoteExec::run` (that impl never returns `Err` for a nonzero exit).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp ssh_pull_transcript 2>&1 | tail -20`
Expected: compile errors (`pull_transcript` for SSH resolves to the trait default and `local_transcript_path` maps to identity → the first test's equality fails once it compiles).

- [ ] **Step 3: Implement**

Imports in `ssh.rs`: change `use std::path::Path;` to `use std::path::{Path, PathBuf};` and add `FeedGuard` to the `host::connector::{…}` import list (Task 4 uses it; adding now avoids a second import edit).

In `impl SshHostConnector` (after `agent_argv`):

```rust
    /// The CP global dir, derived exactly as `NodeMirror::transcript_mirror_path`
    /// derives it: the run store root is `<global>/runs`.
    fn global_dir(&self) -> PathBuf {
        self.run_store
            .root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.run_store.root.clone())
    }

    /// Spec §3.3: a remote read is scoped to a run. `run_id` must be a run
    /// this host executed (`worker_id`), and `recorded` must be a path that
    /// run's own artifacts claim. Returns the cache path the file serves
    /// from. Never touches the remote.
    pub(crate) fn authorize_remote_transcript(
        &self,
        run_id: &str,
        recorded: &Path,
    ) -> Result<PathBuf, HostConnectorError> {
        let cache = crate::host::transcript_paths::cache_path(&self.global_dir(), &self.host_id, recorded)
            .ok_or_else(|| {
                HostConnectorError::Invalid(format!(
                    "not a transcript path: {}",
                    recorded.display()
                ))
            })?;
        let rec = self
            .run_store
            .load(run_id)
            .map_err(|_| HostConnectorError::NotFound(run_id.to_string()))?;
        if rec.worker_id.as_deref() != Some(self.host_id.as_str()) {
            return Err(HostConnectorError::Invalid(format!(
                "run {run_id} does not belong to host {}",
                self.host_id
            )));
        }
        let claimed =
            crate::host::transcript_paths::recorded_transcript_paths(&self.run_store, run_id);
        if !claimed.iter().any(|p| p == recorded) {
            return Err(HostConnectorError::Invalid(format!(
                "run {run_id} did not record transcript {}",
                recorded.display()
            )));
        }
        Ok(cache)
    }
```

Free functions (near `parse_tail_marker`):

```rust
/// Sentinel the remote prints instead of a body when the file is absent, so
/// "no such file" (a complete, empty answer once the run is terminal) is
/// distinguishable from "ssh never reached the host" (no stdout at all).
pub(crate) const NO_FILE_SENTINEL: &str = "__RUPU_NO_FILE__";

/// One-shot read of a remote transcript. Single-quoted path; stderr dropped.
pub(crate) fn single_cat_command(remote: &str) -> String {
    format!(
        "cat {} 2>/dev/null || printf '{NO_FILE_SENTINEL}\\n'",
        shell_escape(remote)
    )
}

/// Write `body` to `cache` atomically (tmp + rename), then the `.complete`
/// sidecar when `complete` (spec §6.1 step 4).
pub(crate) fn write_cache_file(
    cache: &Path,
    body: &str,
    complete: bool,
) -> Result<(), HostConnectorError> {
    let io = |e: std::io::Error| HostConnectorError::Invalid(format!("transcript cache io: {e}"));
    if let Some(dir) = cache.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = PathBuf::from(format!("{}.tmp", cache.display()));
    std::fs::write(&tmp, body).map_err(io)?;
    std::fs::rename(&tmp, cache).map_err(io)?;
    if complete {
        std::fs::write(crate::host::transcript_paths::complete_marker(cache), b"").map_err(io)?;
    }
    Ok(())
}
```

In `impl HostConnector for SshHostConnector` (after `get_transcript`):

```rust
    fn local_transcript_path(&self, recorded: &Path) -> PathBuf {
        crate::host::transcript_paths::cache_path(&self.global_dir(), &self.host_id, recorded)
            .unwrap_or_else(|| recorded.to_path_buf())
    }

    async fn pull_transcript(
        &self,
        run_id: &str,
        recorded: &Path,
        terminal: bool,
    ) -> Result<(), HostConnectorError> {
        let cache = self.authorize_remote_transcript(run_id, recorded)?;
        let remote = recorded
            .to_str()
            .ok_or_else(|| HostConnectorError::Invalid("non-UTF-8 transcript path".into()))?;
        if remote.contains('\0') {
            return Err(HostConnectorError::Invalid("transcript path contains NUL".into()));
        }
        let out = self
            .exec
            .run(&single_cat_command(remote))
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success && out.stdout.is_empty() {
            return Err(HostConnectorError::Unreachable(format!(
                "host {} did not answer: {}",
                self.host_id,
                out.stderr.trim()
            )));
        }
        let body = if out.stdout.trim_end() == NO_FILE_SENTINEL {
            String::new()
        } else {
            out.stdout
        };
        write_cache_file(&cache, &body, terminal)
    }
```

`FakeExec`: add the field

```rust
        /// If set, returned as stdout for the pump's batched terminal pull
        /// (`for p in …; do printf '==> %s <==' …; cat …; done`, Task 6).
        batch_cat_stdout: Option<String>,
```

initialise `batch_cat_stdout: None` in all six constructors (`ok`, `offline`, `with_cat_stdout`, `with_bytes_ok`, `with_bytes_err`, and the variant at ~line 2790), and in `FakeExec::run` add a branch before the `cat ` branch:

```rust
                } else if remote.starts_with("for p in ") {
                    self.batch_cat_stdout.clone().unwrap_or_default()
```

The existing `cat ` branch already returns `cat_transcript_stdout` for any `cat` whose command contains `/transcripts/`, which the new single-cat command does.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp ssh_pull_transcript ssh_local_transcript_path 2>&1 | tail -20` then `cargo test -p rupu-cp host::ssh 2>&1 | tail -5`
Expected: 6 new tests pass; existing SSH tests unaffected.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/host/ssh.rs
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(ssh): run-scoped transcript allowlist, cache mapping, one-shot pull

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: `LazyTailRegistry` and `ensure_transcript_feed`

**Files:**
- Create: `crates/rupu-cp/src/host/lazy_tail.rs`
- Modify: `crates/rupu-cp/src/host/mod.rs` (add `pub mod lazy_tail;`), `crates/rupu-cp/src/host/ssh.rs` (struct field + `new()` ~line 909; trait impl next to `pull_transcript`)

**Interfaces:**
- Consumes: `RemoteExec`, `RemoteExecError`, `shell_escape`, `parse_tail_marker` from `ssh.rs` (all `pub(crate)`); `is_complete` from Task 1; `FeedGuard` from Task 2.
- Produces:
  - `pub(crate) struct LazyTailRegistry` with `new(exec: Arc<dyn RemoteExec>)`, `subscribe(&self, remote: &str, cache: &Path) -> Result<Option<Arc<FeedHandle>>, RemoteExecError>`, and `#[cfg(test)] live_feeds(&self) -> usize`.
  - `pub(crate) struct FeedHandle` — `Drop` aborts the feeding task; `alive()` reports whether the remote stream is still delivering.
  - `SshHostConnector.lazy: Arc<LazyTailRegistry>`; `HostConnector::ensure_transcript_feed` for SSH.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cp/src/host/lazy_tail.rs` starting with the test module:

```rust
//! Spec §5.1: one `tail -n +1 -F` over ssh per distinct cache file, shared by
//! every viewer of that file, killed when the last viewer leaves, never
//! opened for a file that already has a `.complete` sidecar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures_util::StreamExt as _;

use super::ssh::{parse_tail_marker, shell_escape, RemoteExec, RemoteExecError};
use super::transcript_paths::is_complete;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ssh::{LineStream, RemoteOutput};

    /// Scripted `spawn_lines`: replays `lines`, then either hangs (a real
    /// `tail -F`) or ends (a dropped ssh session). Counts spawns.
    struct ScriptedExec {
        lines: Vec<String>,
        hang: bool,
        spawns: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl RemoteExec for ScriptedExec {
        async fn run(&self, _c: &str) -> Result<RemoteOutput, RemoteExecError> {
            unimplemented!("lazy tail never calls run")
        }
        fn spawn_lines(&self, c: &str) -> Result<LineStream, RemoteExecError> {
            self.spawns.lock().unwrap().push(c.to_string());
            let items: Vec<std::io::Result<String>> = self.lines.iter().cloned().map(Ok).collect();
            let head = futures_util::stream::iter(items);
            if self.hang {
                Ok(Box::pin(head.chain(futures_util::stream::pending())))
            } else {
                Ok(Box::pin(head))
            }
        }
        async fn run_bytes(&self, _c: &str, _s: Option<Vec<u8>>) -> Result<Vec<u8>, RemoteExecError> {
            unimplemented!()
        }
    }

    fn exec(lines: &[&str], hang: bool) -> Arc<ScriptedExec> {
        Arc::new(ScriptedExec {
            lines: lines.iter().map(|s| s.to_string()).collect(),
            hang,
            spawns: Default::default(),
        })
    }

    async fn wait_for_content(path: &Path, needle: &str) {
        for _ in 0..100 {
            if std::fs::read_to_string(path).map(|s| s.contains(needle)).unwrap_or(false) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("{needle:?} never appeared in {}", path.display());
    }

    #[tokio::test]
    async fn first_subscriber_truncates_then_replays_from_the_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("mirror/h/transcripts/run_01A.jsonl");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, "STALE LINE FROM A PREVIOUS TAIL\n").unwrap();
        let ex = exec(&[r#"{"type":"run_start"}"#, r#"{"type":"turn_start"}"#], true);
        let reg = LazyTailRegistry::new(ex.clone());

        let guard = reg.subscribe("/remote/.rupu/transcripts/run_01A.jsonl", &cache).unwrap().unwrap();
        wait_for_content(&cache, "turn_start").await;
        let got = std::fs::read_to_string(&cache).unwrap();
        assert!(!got.contains("STALE"), "cache must be truncated before replay: {got:?}");
        assert_eq!(got, "{\"type\":\"run_start\"}\n{\"type\":\"turn_start\"}\n");
        let spawns = ex.spawns.lock().unwrap();
        assert_eq!(spawns.len(), 1);
        assert_eq!(spawns[0], "tail -n +1 -F '/remote/.rupu/transcripts/run_01A.jsonl'");
        drop(guard);
    }

    #[tokio::test]
    async fn two_subscribers_share_one_remote_tail_and_the_last_drop_kills_it() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01B.jsonl");
        let ex = exec(&[r#"{"type":"run_start"}"#], true);
        let reg = LazyTailRegistry::new(ex.clone());

        let a = reg.subscribe("/r/run_01B.jsonl", &cache).unwrap().unwrap();
        let b = reg.subscribe("/r/run_01B.jsonl", &cache).unwrap().unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(ex.spawns.lock().unwrap().len(), 1);
        assert_eq!(reg.live_feeds(), 1);
        wait_for_content(&cache, "run_start").await;

        drop(a);
        assert_eq!(reg.live_feeds(), 1, "one holder left");
        drop(b);
        assert_eq!(reg.live_feeds(), 0, "last holder gone → feed released");
        // The partial cache stays on disk.
        assert!(cache.exists());
    }

    #[tokio::test]
    async fn a_complete_cache_is_never_tailed() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01C.jsonl");
        std::fs::write(&cache, "{\"type\":\"run_start\"}\n").unwrap();
        std::fs::write(crate::host::transcript_paths::complete_marker(&cache), b"").unwrap();
        let ex = exec(&[], true);
        let reg = LazyTailRegistry::new(ex.clone());

        assert!(reg.subscribe("/r/run_01C.jsonl", &cache).unwrap().is_none());
        assert!(ex.spawns.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "{\"type\":\"run_start\"}\n", "not truncated");
    }

    #[tokio::test]
    async fn a_dead_feed_is_replaced_on_the_next_subscribe() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01D.jsonl");
        // `hang: false` → the "ssh session" ends right after replay.
        let ex = exec(&[r#"{"type":"run_start"}"#], false);
        let reg = LazyTailRegistry::new(ex.clone());

        let first = reg.subscribe("/r/run_01D.jsonl", &cache).unwrap().unwrap();
        wait_for_content(&cache, "run_start").await;
        for _ in 0..100 {
            if !first.alive() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!first.alive(), "feed must report dead once the stream ends");

        // A second viewer, while the first still holds its (dead) handle,
        // gets a fresh tail — which truncates and replays from byte zero.
        let second = reg.subscribe("/r/run_01D.jsonl", &cache).unwrap().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(ex.spawns.lock().unwrap().len(), 2);
        wait_for_content(&cache, "run_start").await;
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n",
            "replay from an empty file: no duplicate lines"
        );
    }

    #[tokio::test]
    async fn tail_headers_are_not_written_into_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("run_01E.jsonl");
        let ex = exec(&["==> /r/run_01E.jsonl <==", r#"{"type":"run_start"}"#, ""], true);
        let reg = LazyTailRegistry::new(ex.clone());
        let _g = reg.subscribe("/r/run_01E.jsonl", &cache).unwrap().unwrap();
        wait_for_content(&cache, "run_start").await;
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "{\"type\":\"run_start\"}\n");
    }
}
```

`RemoteOutput` and `LineStream` must be `pub(crate)` in `ssh.rs` (they already are `pub(crate)`; confirm and adjust visibility if the compiler objects).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp lazy_tail 2>&1 | tail -10`
Expected: compile errors, `LazyTailRegistry` not found.

- [ ] **Step 3: Implement**

Above the test module:

```rust
/// Refcount + liveness for one shared remote tail. Every subscriber holds an
/// `Arc`; the feeding task is aborted on drop of the last one, which drops
/// the ssh child through `kill_on_drop`.
pub(crate) struct FeedHandle {
    task: tokio::task::JoinHandle<()>,
    alive: Arc<AtomicBool>,
}

impl FeedHandle {
    /// `false` once the remote stream ended (ssh dropped, remote `tail`
    /// died). The cache file is left as-is; a later subscribe replaces the
    /// feed and replays from byte zero.
    pub(crate) fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for FeedHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) struct LazyTailRegistry {
    exec: Arc<dyn RemoteExec>,
    feeds: Mutex<HashMap<PathBuf, Weak<FeedHandle>>>,
}

impl LazyTailRegistry {
    pub(crate) fn new(exec: Arc<dyn RemoteExec>) -> Self {
        Self {
            exec,
            feeds: Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe to `remote` being tailed into `cache`.
    ///
    /// * `Ok(None)` — `cache` is already complete; nothing to tail.
    /// * `Ok(Some(handle))` — hold it for as long as the viewer is attached.
    ///   The first subscriber truncates the cache and spawns
    ///   `tail -n +1 -F` (a byte-zero replay, which is what makes the
    ///   truncate safe); later subscribers share the running feed; a dead
    ///   feed is replaced.
    pub(crate) fn subscribe(
        &self,
        remote: &str,
        cache: &Path,
    ) -> Result<Option<Arc<FeedHandle>>, RemoteExecError> {
        if is_complete(cache) {
            return Ok(None);
        }
        let mut feeds = self.feeds.lock().unwrap();
        if let Some(existing) = feeds.get(cache).and_then(Weak::upgrade) {
            if existing.alive() {
                return Ok(Some(existing));
            }
        }
        if let Some(dir) = cache.parent() {
            std::fs::create_dir_all(dir).map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        }
        // Truncate BEFORE spawning: the replay starts from an empty file.
        std::fs::File::create(cache).map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        let cmd = format!("tail -n +1 -F {}", shell_escape(remote));
        let mut stream = self.exec.spawn_lines(&cmd)?;
        let alive = Arc::new(AtomicBool::new(true));
        let alive_for_task = Arc::clone(&alive);
        let cache_owned = cache.to_path_buf();
        let task = tokio::spawn(async move {
            use std::io::Write as _;
            let file = std::fs::OpenOptions::new().append(true).open(&cache_owned);
            let Ok(mut file) = file else {
                alive_for_task.store(false, Ordering::SeqCst);
                return;
            };
            while let Some(Ok(line)) = stream.next().await {
                if parse_tail_marker(&line).is_some() || line.trim().is_empty() {
                    continue;
                }
                if writeln!(file, "{line}").and_then(|_| file.flush()).is_err() {
                    break;
                }
            }
            alive_for_task.store(false, Ordering::SeqCst);
        });
        let handle = Arc::new(FeedHandle { task, alive });
        feeds.insert(cache.to_path_buf(), Arc::downgrade(&handle));
        Ok(Some(handle))
    }

    #[cfg(test)]
    pub(crate) fn live_feeds(&self) -> usize {
        self.feeds
            .lock()
            .unwrap()
            .values()
            .filter(|w| w.strong_count() > 0)
            .count()
    }
}
```

`ssh.rs`:
- struct `SshHostConnector`: add `lazy: Arc<crate::host::lazy_tail::LazyTailRegistry>,`
- `new()`: before building `Self`, `let lazy = Arc::new(crate::host::lazy_tail::LazyTailRegistry::new(Arc::clone(&exec)));` and set the field.
- trait impl, after `pull_transcript`:

```rust
    async fn ensure_transcript_feed(
        &self,
        run_id: &str,
        recorded: &Path,
    ) -> Result<FeedGuard, HostConnectorError> {
        let cache = self.authorize_remote_transcript(run_id, recorded)?;
        let remote = recorded
            .to_str()
            .ok_or_else(|| HostConnectorError::Invalid("non-UTF-8 transcript path".into()))?;
        match self.lazy.subscribe(remote, &cache) {
            Ok(Some(handle)) => Ok(FeedGuard::holding(Box::new(handle))),
            Ok(None) => Ok(FeedGuard::noop()),
            Err(e) => Err(HostConnectorError::Unreachable(e.to_string())),
        }
    }
```

Add one SSH-level test next to the Task 3 tests:

```rust
    #[tokio::test]
    async fn ssh_ensure_transcript_feed_tails_the_claimed_path_into_the_cache() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![r#"{"type":"run_start"}"#.into()]));
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01FEED.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01FEEDRUN", claimed, false);
        let cache = tmp.path().join("mirror/host_abc/transcripts/run_01FEED.jsonl");

        let guard = conn
            .ensure_transcript_feed("run_01FEEDRUN", std::path::Path::new(claimed))
            .await
            .unwrap();
        for _ in 0..100 {
            if std::fs::read_to_string(&cache).map(|s| s.contains("run_start")).unwrap_or(false) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "{\"type\":\"run_start\"}\n");
        let cmds = fake.commands.lock().unwrap();
        assert!(cmds.iter().any(|c| c == &format!("tail -n +1 -F '{claimed}'")), "{cmds:?}");
        drop(guard);

        let err = conn
            .ensure_transcript_feed("run_01FEEDRUN", std::path::Path::new("/home/ci/proj/.rupu/transcripts/run_01X.jsonl"))
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp lazy_tail ssh_ensure_transcript_feed 2>&1 | tail -15`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/host/lazy_tail.rs crates/rupu-cp/src/host/ssh.rs
git add crates/rupu-cp/src/host/lazy_tail.rs crates/rupu-cp/src/host/mod.rs crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(ssh): lazy shared tail registry feeding the transcript cache

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: `/api/transcript` and `/api/transcript/stream` honour `host` + `run`

**Files:**
- Modify: `crates/rupu-cp/src/api/transcript.rs`, `crates/rupu-cp/src/error.rs`

**Interfaces:**
- Consumes: Task 2's `FeedGuard`, `local_transcript_path`, `ensure_transcript_feed`, `pull_transcript`; Task 1's `is_complete`.
- Produces:
  - `ApiError::bad_gateway(msg)` → 502.
  - Query `PathQ { path, host, run }`.
  - Response gains `"partial": true` only when §4.2 applies.
  - `enum RemoteRead { Local(PathBuf), Cache { cache: PathBuf, complete: bool } }` and `fn plan_remote_read(local: Option<PathBuf>, mapped: PathBuf, raw: &Path) -> RemoteRead` (pure, unit-tested).

- [ ] **Step 1: Write the failing tests**

Append a test module to `api/transcript.rs`:

```rust
#[cfg(test)]
mod remote_read_tests {
    use super::*;

    #[test]
    fn plan_prefers_a_local_file_then_a_complete_cache_then_a_pull() {
        let tmp = tempfile::tempdir().unwrap();
        let raw = Path::new("/remote/proj/.rupu/transcripts/run_01A.jsonl");
        let cache = tmp.path().join("mirror/h/transcripts/run_01A.jsonl");

        // 1. A validated, existing local file wins outright.
        let local = tmp.path().join("transcripts/run_01A.jsonl");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "").unwrap();
        assert!(matches!(
            plan_remote_read(Some(local.clone()), cache.clone(), raw),
            RemoteRead::Local(p) if p == local
        ));

        // 2. No local file: an incomplete (or absent) cache means "pull".
        assert!(matches!(
            plan_remote_read(None, cache.clone(), raw),
            RemoteRead::Cache { complete: false, .. }
        ));

        // 3. A complete cache is served as-is.
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, "").unwrap();
        std::fs::write(crate::host::transcript_paths::complete_marker(&cache), b"").unwrap();
        assert!(matches!(
            plan_remote_read(None, cache.clone(), raw),
            RemoteRead::Cache { complete: true, .. }
        ));
    }

    #[test]
    fn plan_treats_an_identity_mapping_as_a_pull_target_too() {
        // A connector that maps to itself (Task 2 default) still lands in
        // the run-scoped branch, where the connector's own allowlist decides.
        let raw = Path::new("/remote/x/run_01A.jsonl");
        assert!(matches!(
            plan_remote_read(None, raw.to_path_buf(), raw),
            RemoteRead::Cache { complete: false, cache } if cache == raw
        ));
    }

    #[test]
    fn partial_flag_is_absent_unless_set() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");
        std::fs::write(&p, "{\"type\":\"turn_start\",\"data\":{\"turn_idx\":0}}\n").unwrap();
        let page = read_local_page(&p).unwrap();
        assert!(page.get("partial").is_none());
        let flagged = mark_partial(page);
        assert_eq!(flagged["partial"], serde_json::json!(true));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp remote_read_tests 2>&1 | tail -10`
Expected: compile errors (`plan_remote_read`, `RemoteRead`, `read_local_page`, `mark_partial` missing).

- [ ] **Step 3: Implement**

`error.rs`, after `conflict`:

```rust
    /// 502 — a remote host this request depends on could not be reached.
    pub fn bad_gateway(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_GATEWAY, msg.into())
    }
```

`api/transcript.rs`:

Imports: add `use crate::host::connector::{FeedGuard, HostConnector};` (keep the existing `HostConnectorError` import), `use std::sync::Arc;`.

Query struct:

```rust
#[derive(serde::Deserialize)]
struct PathQ {
    path: String,
    /// When present and not `"local"`, serve from / through the named host.
    #[serde(default)]
    host: Option<String>,
    /// Spec §3.3: the run that recorded `path`. Required only when `host` is
    /// remote and the transcript has not been mirrored or cached yet — the
    /// connector uses it to check the path is one the run itself claims.
    #[serde(default)]
    run: Option<String>,
}
```

Helpers (above `routes()`):

```rust
/// Where a `?host=<remote>` transcript request is served from (§3.3 steps 1–3).
enum RemoteRead {
    /// A validated, existing local file (the PR #646 agent-run mirror).
    Local(PathBuf),
    /// The connector's cache file; `complete` = has a `.complete` sidecar.
    Cache { cache: PathBuf, complete: bool },
}

/// Pure decision: `local` is the already-validated local path if it exists,
/// `mapped` is `connector.local_transcript_path(raw)`.
fn plan_remote_read(local: Option<PathBuf>, mapped: PathBuf, _raw: &Path) -> RemoteRead {
    if let Some(p) = local {
        return RemoteRead::Local(p);
    }
    let complete = crate::host::transcript_paths::is_complete(&mapped);
    RemoteRead::Cache {
        cache: mapped,
        complete,
    }
}

/// Resolve the connector and the read plan for a remote-host request.
fn resolve_remote(
    s: &AppState,
    host_id: &str,
    raw: &str,
) -> ApiResult<(Arc<dyn HostConnector>, RemoteRead)> {
    let conn = s.hosts.resolve(host_id).map_err(|e| match e {
        HostConnectorError::NotFound(_) => ApiError::not_found(format!("host {host_id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    let local = validate_transcript_path(raw, &allowed_roots(s))
        .ok()
        .filter(|p| p.exists());
    let mapped = conn.local_transcript_path(Path::new(raw));
    let plan = plan_remote_read(local, mapped, Path::new(raw));
    Ok((conn, plan))
}

/// Read a local transcript file into the `{events, summary, unparsed}` page.
/// A missing file is an empty page (a freshly-sent turn's transcript).
fn read_local_page(path: &Path) -> ApiResult<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({ "events": [], "summary": null }));
    }
    let (events, unparsed) =
        read_events_counting_unparsed(path).map_err(|e| ApiError::internal(e.to_string()))?;
    let summary = rupu_transcript::JsonlReader::summary(path).ok();
    Ok(serde_json::json!({ "events": events, "summary": summary, "unparsed": unparsed }))
}

/// §4.2: the coordinator could not collect the rest of this transcript.
fn mark_partial(mut page: serde_json::Value) -> serde_json::Value {
    page["partial"] = serde_json::json!(true);
    page
}

fn map_conn_err(host_id: &str, e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Invalid(m) | HostConnectorError::Unsupported(m) => ApiError::bad_request(m),
        HostConnectorError::Unreachable(m) => ApiError::bad_gateway(format!("host {host_id} unreachable: {m}")),
        other => ApiError::internal(other.to_string()),
    }
}
```

Rewrite `get_transcript`'s remote branch:

```rust
    if host_id != "local" {
        let (conn, plan) = resolve_remote(&s, host_id, &q.path)?;
        return match plan {
            RemoteRead::Local(p) => Ok(Json(read_local_page(&p)?)),
            RemoteRead::Cache { cache, complete: true } => Ok(Json(read_local_page(&cache)?)),
            RemoteRead::Cache { cache, complete: false } => {
                let run_id = q.run.as_deref().ok_or_else(|| {
                    ApiError::bad_request(
                        "run is required to read a transcript that has not been mirrored yet",
                    )
                })?;
                let terminal = s
                    .run_store
                    .load(run_id)
                    .map(|r| r.status.is_terminal())
                    .unwrap_or(false);
                match conn.pull_transcript(run_id, Path::new(&q.path), terminal).await {
                    Ok(()) => Ok(Json(read_local_page(&cache)?)),
                    // Unreachable with something on disk: serve it, flagged (§4.2).
                    Err(HostConnectorError::Unreachable(_)) if cache.exists() => {
                        Ok(Json(mark_partial(read_local_page(&cache)?)))
                    }
                    Err(e) => Err(map_conn_err(host_id, e)),
                }
            }
        };
    }
```

and replace the local branch's body with `let path = validate…?; Ok(Json(read_local_page(&path)?))`.

Rewrite `stream_transcript`:

```rust
async fn stream_transcript(State(s): State<AppState>, Query(q): Query<PathQ>) -> Response {
    let host_id = q.host.as_deref().unwrap_or("local");
    // Held for the SSE stream's lifetime (moved into the map closure below).
    let mut guard: Option<FeedGuard> = None;
    let path = if host_id == "local" {
        match validate_transcript_path(&q.path, &allowed_roots(&s)) {
            Ok(p) => p,
            Err(e) => return ApiError::bad_request(e).into_response(),
        }
    } else {
        match resolve_remote(&s, host_id, &q.path) {
            Err(e) => return e.into_response(),
            Ok((_, RemoteRead::Local(p))) => p,
            Ok((_, RemoteRead::Cache { cache, complete: true })) => cache,
            Ok((conn, RemoteRead::Cache { cache, complete: false })) => {
                let Some(run_id) = q.run.as_deref() else {
                    return ApiError::bad_request(
                        "run is required to stream a transcript that has not been mirrored yet",
                    )
                    .into_response();
                };
                match conn.ensure_transcript_feed(run_id, Path::new(&q.path)).await {
                    Ok(g) => {
                        guard = Some(g);
                        cache
                    }
                    Err(e) => return map_conn_err(host_id, e).into_response(),
                }
            }
        }
    };
    let tail = match crate::transcript_tail::TranscriptTail::open(&path).await {
        Ok(t) => t,
        Err(e) => return ApiError::internal(e.to_string()).into_response(),
    };
    let stream = tail.map(move |ev| {
        // Keep the remote feed alive exactly as long as this stream.
        let _keep = &guard;
        let sse = SseEvent::default()
            .json_data(&ev)
            .unwrap_or_else(|_| SseEvent::default().comment("event serialize error"));
        Ok::<_, Infallible>(sse)
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
```

`TranscriptTail::open` polls until the file exists, and `subscribe` created it, so the tail attaches to the freshly truncated cache and sees the replay from byte zero.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp api::transcript 2>&1 | tail -10` then `cargo clippy -p rupu-cp --all-targets 2>&1 | tail -5`
Expected: all transcript tests pass (existing `read_tests` + 3 new); clippy clean.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/api/transcript.rs crates/rupu-cp/src/error.rs
git add crates/rupu-cp/src/api/transcript.rs crates/rupu-cp/src/error.rs
git commit -m "feat(cp): /api/transcript{,/stream} serve remote-host transcripts via the lazy cache

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Pump — `run.json` on every tick, live active step for agent runs, terminal batched pull

**Files:**
- Modify: `crates/rupu-cp/src/node/mirror.rs` (`finish` ~line 313; new methods), `crates/rupu-cp/src/host/ssh.rs` (`pump_finalize_if_terminal` ~line 749; the transcript-header branch in `spawn_tail_pump` ~line 1177; new free functions near `pump_catch_up_transcript`)

**Interfaces:**
- Consumes: Task 1's `recorded_transcript_paths`, `cache_path`, `is_complete`; Task 3's `write_cache_file`, `shell_escape`, `parse_tail_marker`; `FakeExec.batch_cat_stdout`.
- Produces:
  - `NodeMirror::run_store(&self) -> &RunStore`, `NodeMirror::global_dir(&self) -> PathBuf`, `NodeMirror::note_transcript_started(run_id, node_id) -> Result<(), MirrorError>`.
  - `pub(crate) fn batch_cat_command(paths: &[String]) -> String`.
  - `pub(crate) fn split_batched_cat(stdout: &str) -> HashMap<String, String>`.
  - `async fn pump_pull_step_transcripts(exec, mirror, run_id, host_id)`.

- [ ] **Step 1: Write the failing tests**

`mirror.rs` has no test module of its own (its behaviour is covered from `ssh.rs` and `tests/node_tunnel.rs`), so every test below goes into `ssh.rs`'s `mod tests`, using `make_conn` and this helper:

```rust
    fn agent_spec() -> crate::node::protocol::RunSpec {
        crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "reviewer".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        }
    }

    fn workflow_spec() -> crate::node::protocol::RunSpec {
        crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "wf".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        }
    }

    #[test]
    fn note_transcript_started_sets_the_live_active_step_and_finish_clears_it() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        conn.mirror.create_run("run_01LIVE", "host_abc", &agent_spec()).unwrap();

        conn.mirror.note_transcript_started("run_01LIVE", "host_abc").unwrap();
        let rec = run_store.load("run_01LIVE").unwrap();
        assert_eq!(rec.active_step_id.as_deref(), Some("agent"));
        assert_eq!(
            rec.active_step_transcript_path,
            Some(conn.mirror.transcript_mirror_path("run_01LIVE"))
        );
        assert!(matches!(
            conn.mirror.note_transcript_started("run_01LIVE", "host_other"),
            Err(crate::node::MirrorError::WrongNode(_))
        ));

        conn.mirror.finish("run_01LIVE", "host_abc", "completed").unwrap();
        let rec = run_store.load("run_01LIVE").unwrap();
        assert_eq!(rec.active_step_id, None);
        assert_eq!(rec.active_step_transcript_path, None);
    }
```

(`MirrorError` must be re-exported from `crate::node` — check `node/mod.rs`; if it only re-exports `NodeMirror`, add `MirrorError` to that `pub use`.)

```rust
    #[test]
    fn batch_cat_command_quotes_each_path_and_brackets_each_file() {
        let cmd = batch_cat_command(&["/a/run_01A.jsonl".into(), "/b/it's.jsonl".into()]);
        assert_eq!(
            cmd,
            "for p in '/a/run_01A.jsonl' '/b/it'\\''s.jsonl'; do printf '==> %s <==\\n' \"$p\"; cat \"$p\" 2>/dev/null; printf '\\n==> end <==\\n'; done"
        );
    }

    #[test]
    fn split_batched_cat_separates_files_and_drops_the_synthetic_trailing_newline() {
        let stdout = "==> /a/one.jsonl <==\n{\"a\":1}\n{\"a\":2}\n\n==> end <==\n\
                      ==> /a/absent.jsonl <==\n\n==> end <==\n\
                      ==> /a/torn.jsonl <==\n{\"t\":1}\n{\"t\":\n==> end <==\n";
        let files = split_batched_cat(stdout);
        assert_eq!(files["/a/one.jsonl"], "{\"a\":1}\n{\"a\":2}\n");
        assert_eq!(files["/a/absent.jsonl"], "");
        assert_eq!(files["/a/torn.jsonl"], "{\"t\":1}\n{\"t\":\n", "a torn last line is kept");
        assert_eq!(files.len(), 3);
        // A stream cut mid-file never yields that file at all.
        let cut = "==> /a/one.jsonl <==\n{\"a\":1}\n";
        assert!(split_batched_cat(cut).is_empty());
    }

    /// Build the remote's `run.json` body from a REAL `RunRecord` so the
    /// mirror's `RunJson` parse cannot silently fail on a hand-written shape.
    fn remote_run_json(
        run_store: &rupu_orchestrator::RunStore,
        run_id: &str,
        status: rupu_orchestrator::RunStatus,
        active: Option<(&str, &str)>,
    ) -> String {
        let mut rec = run_store.load(run_id).unwrap();
        rec.status = status;
        rec.worker_id = None; // the remote does not know it is a mirror
        rec.active_step_id = active.map(|(id, _)| id.to_string());
        rec.active_step_transcript_path = active.map(|(_, p)| std::path::PathBuf::from(p));
        serde_json::to_string(&rec).unwrap()
    }

    #[tokio::test]
    async fn tail_pump_mirrors_run_json_while_the_run_is_still_alive() {
        let run_id = "run_01ALIVE";
        // Two-phase construction: the mirror record must exist before the
        // remote run.json body can be derived from it.
        let fake_probe = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn0, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake_probe));
        conn0.mirror.create_run(run_id, &conn0.host_id, &workflow_spec()).unwrap();
        let alive_json = remote_run_json(
            &run_store,
            run_id,
            rupu_orchestrator::RunStatus::Running,
            Some(("build", "/remote/proj/.rupu/transcripts/run_01BUILD.jsonl")),
        );
        let fake = std::sync::Arc::new(FakeExec::with_cat_stdout(vec![], alive_json));
        let mirror = std::sync::Arc::clone(&conn0.mirror);
        let exec: std::sync::Arc<dyn RemoteExec> = fake;
        let conn = SshHostConnector::new("host_abc", exec, mirror, std::sync::Arc::clone(&run_store));
        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.active_step_id.as_deref() == Some("build") {
                assert_eq!(
                    rec.active_step_transcript_path,
                    Some(std::path::PathBuf::from("/remote/proj/.rupu/transcripts/run_01BUILD.jsonl"))
                );
                assert_eq!(rec.status, rupu_orchestrator::RunStatus::Running);
                assert_eq!(rec.worker_id.as_deref(), Some("host_abc"), "identity re-pinned");
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("non-terminal run.json was never mirrored");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn tail_pump_first_transcript_header_marks_the_agent_step_live() {
        let run_id = "run_01LIVEHDR";
        let tail_lines = vec![
            format!("==> /home/ci/.rupu/transcripts/{run_id}.jsonl <=="),
            r#"{"type":"run_start","agent":"reviewer"}"#.to_string(),
        ];
        // No run.json yet (an agent run writes it only at the end) → the
        // interval probe answers Absent and the pump stays open.
        let fake = std::sync::Arc::new(FakeExec::ok(tail_lines));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        conn.mirror.create_run(run_id, &conn.host_id, &agent_spec()).unwrap();
        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.active_step_id.as_deref() == Some("agent") {
                assert_eq!(rec.active_step_transcript_path, Some(conn.mirror.transcript_mirror_path(run_id)));
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("active step never set from the transcript header");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn tail_pump_terminal_pulls_every_recorded_step_transcript_into_the_cache() {
        let run_id = "run_01PULLALL";
        let step_a = "/home/ci/proj/.rupu/transcripts/run_01STEPA.jsonl";
        let step_b = "/home/ci/proj/.rupu/transcripts/run_01STEPB.jsonl";
        // The pump's `select!` is unbiased, so the terminal probe can win
        // before any tailed line is consumed. Pre-seed the LOCAL mirror with
        // the artifacts the tail would have delivered (that is what
        // `recorded_transcript_paths` reads), and feed no tail lines.
        let fake_probe = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn0, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake_probe));
        conn0.mirror.create_run(run_id, &conn0.host_id, &workflow_spec()).unwrap();
        conn0
            .mirror
            .append(
                run_id,
                "host_abc",
                ArtifactFile::StepResults,
                &format!(r#"{{"step_id":"a","run_id":"run_01STEPA","transcript_path":"{step_a}","output":"","success":true,"skipped":false,"rendered_prompt":"","finished_at":"2026-09-04T00:00:00Z"}}"#),
            )
            .unwrap();
        conn0
            .mirror
            .append(
                run_id,
                "host_abc",
                ArtifactFile::Events,
                &format!(r#"{{"type":"step_working","run_id":"{run_id}","step_id":"b","transcript_path":"{step_b}"}}"#),
            )
            .unwrap();
        let run_json = remote_run_json(&run_store, run_id, rupu_orchestrator::RunStatus::Completed, None);
        let mut fake = FakeExec::with_cat_stdout(vec![], run_json);
        fake.batch_cat_stdout = Some(format!(
            "==> {step_a} <==\n{{\"type\":\"run_start\"}}\n\n==> end <==\n==> {step_b} <==\n\n==> end <==\n"
        ));
        let fake = std::sync::Arc::new(fake);
        let mirror = std::sync::Arc::clone(&conn0.mirror);
        let exec: std::sync::Arc<dyn RemoteExec> = std::sync::Arc::clone(&fake) as _;
        let conn = SshHostConnector::new("host_abc", exec, mirror, std::sync::Arc::clone(&run_store));
        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if run_store.load(run_id).unwrap().status != rupu_orchestrator::RunStatus::Running {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("pump never finalized");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Join the pump's terminal work the way the dispatcher does.
        conn.await_run_mirror(run_id).await;

        let cache_a = tmp.path().join("mirror/host_abc/transcripts/run_01STEPA.jsonl");
        let cache_b = tmp.path().join("mirror/host_abc/transcripts/run_01STEPB.jsonl");
        assert_eq!(std::fs::read_to_string(&cache_a).unwrap(), "{\"type\":\"run_start\"}\n");
        assert!(crate::host::transcript_paths::is_complete(&cache_a));
        assert_eq!(std::fs::read_to_string(&cache_b).unwrap(), "", "absent remote file → empty complete answer");
        assert!(crate::host::transcript_paths::is_complete(&cache_b));

        let cmds = fake.commands.lock().unwrap();
        let batch: Vec<_> = cmds.iter().filter(|c| c.starts_with("for p in ")).collect();
        assert_eq!(batch.len(), 1, "exactly one ssh invocation for all files: {cmds:?}");
        assert!(batch[0].contains(&format!("'{step_a}'")) && batch[0].contains(&format!("'{step_b}'")));
    }
```

The `step_results.jsonl` line in the last test relies on `StepResultRecord`'s serde defaults for `kind`/`items`/`findings`/`iterations`/`resolved`/`loop_iteration`/`run_outcome` (and `host` after Task 8) — all are `#[serde(default)]`. The `step_working` event line relies on `Event`'s `#[serde(tag = "type", rename_all = "snake_case")]` shape, which `graph.rs::merge_event_units` already parses the same way; confirm the tag spelling against `executor/event.rs` if the test's path is not picked up.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp note_transcript_started batch_cat split_batched tail_pump_mirrors_run_json tail_pump_first_transcript tail_pump_terminal_pulls 2>&1 | tail -20`
Expected: compile errors for the new symbols; the pump tests fail on their deadline once compiling.

- [ ] **Step 3: Implement**

`mirror.rs`:

```rust
    /// The store this mirror writes into. Readers that need the run's own
    /// artifacts (the tail pump's terminal pull) go through here.
    pub fn run_store(&self) -> &RunStore {
        &self.run_store
    }

    /// The CP global dir (`<global>/runs` is the store root).
    pub fn global_dir(&self) -> PathBuf {
        self.run_store
            .root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.run_store.root.clone())
    }

    /// Spec §8: the pump saw the agent transcript's first line. Point the
    /// local record's active step at the mirrored copy so the frontends'
    /// existing active-step fallback opens it live. No-op once terminal.
    pub fn note_transcript_started(&self, run_id: &str, node_id: &str) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let mut record = self.run_store.load(run_id)?;
        if record.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        if record.status.is_terminal() {
            return Ok(());
        }
        record.active_step_id = Some("agent".into());
        record.active_step_transcript_path = Some(self.transcript_mirror_path(run_id));
        self.run_store.update(&record)?;
        Ok(())
    }
```

In `finish`, before `self.run_store.update(&record)?;` add:

```rust
        record.active_step_id = None;
        record.active_step_transcript_path = None;
```

`ssh.rs` free functions (next to `pump_catch_up_transcript`):

```rust
/// Spec §6.1 step 3: one ssh invocation that prints the pump's own
/// `==> <path> <==` header, the file, then a synthetic newline + `==> end <==`
/// for every path. The synthetic newline guarantees the end marker starts a
/// fresh line even when the file's last line is torn; `split_batched_cat`
/// removes it again.
pub(crate) fn batch_cat_command(paths: &[String]) -> String {
    let mut cmd = String::from("for p in");
    for p in paths {
        cmd.push(' ');
        cmd.push_str(&shell_escape(p));
    }
    cmd.push_str(
        "; do printf '==> %s <==\\n' \"$p\"; cat \"$p\" 2>/dev/null; printf '\\n==> end <==\\n'; done",
    );
    cmd
}

/// Inverse of [`batch_cat_command`]: `path → body` for every file whose end
/// marker arrived. A file cut off mid-stream is absent from the map, so the
/// caller leaves its cache untouched and unmarked (§6.2).
pub(crate) fn split_batched_cat(stdout: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in stdout.split('\n') {
        match parse_tail_marker(line) {
            Some("end") => {
                if let Some((path, mut lines)) = current.take() {
                    // Drop the ONE synthetic empty line the command appended.
                    if lines.last() == Some(&"") {
                        lines.pop();
                    }
                    let mut body = lines.join("\n");
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    out.insert(path, body);
                }
            }
            Some(path) => current = Some((path.to_string(), Vec::new())),
            None => {
                if let Some((_, lines)) = current.as_mut() {
                    lines.push(line);
                }
            }
        }
    }
    out
}

/// Spec §6: pull every step transcript the run's artifacts recorded into the
/// host's cache, in one ssh round trip, marking each complete. Best-effort:
/// a failure leaves files unmarked for the on-demand retry (§4.2).
async fn pump_pull_step_transcripts(
    exec: &dyn RemoteExec,
    mirror: &NodeMirror,
    run_id: &str,
    host_id: &str,
) {
    let global = mirror.global_dir();
    let agent_mirror = mirror.transcript_mirror_path(run_id);
    let mut targets: Vec<(String, std::path::PathBuf)> = Vec::new();
    for recorded in crate::host::transcript_paths::recorded_transcript_paths(mirror.run_store(), run_id) {
        if recorded == agent_mirror {
            continue; // handled by pump_catch_up_transcript
        }
        let Some(cache) = crate::host::transcript_paths::cache_path(&global, host_id, &recorded) else {
            continue;
        };
        if crate::host::transcript_paths::is_complete(&cache) {
            continue;
        }
        if let Some(remote) = recorded.to_str() {
            targets.push((remote.to_string(), cache));
        }
    }
    if targets.is_empty() {
        return;
    }
    let remotes: Vec<String> = targets.iter().map(|(r, _)| r.clone()).collect();
    let Ok(out) = exec.run(&batch_cat_command(&remotes)).await else {
        return;
    };
    let files = split_batched_cat(&out.stdout);
    for (remote, cache) in &targets {
        if let Some(body) = files.get(remote) {
            if let Err(e) = write_cache_file(cache, body, true) {
                tracing::warn!(host_id, run_id, cache = %cache.display(), error = %e, "terminal transcript pull: cache write failed");
            }
        } else {
            tracing::warn!(host_id, run_id, remote, "terminal transcript pull did not deliver this file; left for on-demand retry");
        }
    }
}
```

`pump_finalize_if_terminal`: replace the `if !is_terminal_status(status)` block with

```rust
    if !is_terminal_status(status) {
        // The host HAS a record for this run — it is simply not done. Mirror
        // it (spec §5.2) so the local record carries the live active step,
        // then clear the startup deadline.
        let _ = mirror.append(run_id, host_id, ArtifactFile::RunJson, &trimmed);
        return PumpProbe::Alive;
    }
```

and after `pump_catch_up_transcript(...)` and before `mirror.finish(...)` add:

```rust
    pump_pull_step_transcripts(exec, mirror, run_id, host_id).await;
```

Also apply the same `pump_pull_step_transcripts` call in the `!terminal_seen` fallback path of `spawn_tail_pump`, right after its `pump_catch_up_transcript(...)` call — the host is still reachable there too.

In `spawn_tail_pump`'s transcript-header branch (`if !transcript_replayed { … reset_transcript … }`), add after `reset_transcript`:

```rust
                                                    let _ = mirror
                                                        .note_transcript_started(&run_id, &host_id);
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp node::mirror host::ssh 2>&1 | tail -10`
Expected: all pass, including the pre-existing pump tests (`tail_pump_terminal_cat_replaces_partial_transcript` etc.), which are unaffected because their step_results are empty.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/node/mirror.rs crates/rupu-cp/src/host/ssh.rs
git add crates/rupu-cp/src/node/mirror.rs crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(ssh): pump mirrors run.json while alive, marks agent step live, pulls every step transcript at terminal

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Usage rollup resolves transcripts through the host mapper

**Files:**
- Modify: `crates/rupu-cp/src/usage.rs` (after `run_transcript_paths`), `crates/rupu-cp/src/api/graph.rs` (`build_run_graph_json` signature + callers, line ~112), `crates/rupu-cp/src/api/agents.rs:584`, `crates/rupu-cp/src/api/usage.rs:430`, `crates/rupu-cp/src/api/netflow.rs:1122`, `crates/rupu-cp/src/api/run_streams.rs:924,971`, `crates/rupu-cp/src/api/usage_outliers.rs:155`

**Interfaces:**
- Consumes: Task 2's `local_transcript_path`, `serves_runs_from_local_mirror`; `HostRegistry::resolve`.
- Produces:
  - `pub fn run_transcript_paths_resolved(store: &RunStore, hosts: &HostRegistry, run_id: &str) -> Vec<PathBuf>`
  - `pub fn summarize_run_resolved(store: &RunStore, hosts: &HostRegistry, run_id: &str, pricing: &PricingConfig) -> UsageSummary`

- [ ] **Step 1: Write the failing test**

Append to `usage.rs` (create a `mod tests` if none exists):

```rust
#[cfg(test)]
mod resolved_paths_tests {
    use super::*;
    use crate::host::registry::HostRegistry;
    use std::sync::Arc;

    #[test]
    fn paths_of_a_run_owned_by_an_unknown_worker_pass_through_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(RunStore::new(tmp.path().join("runs")));
        let mirror = crate::node::NodeMirror::new(Arc::clone(&store));
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "wf".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
        };
        mirror.create_run("run_01USAGE", "host_unknown", &spec).unwrap();
        store
            .append_step_result(
                "run_01USAGE",
                &rupu_orchestrator::runs::StepResultRecord {
                    step_id: "a".into(),
                    run_id: "run_01A".into(),
                    transcript_path: PathBuf::from("/remote/proj/.rupu/transcripts/run_01A.jsonl"),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: Default::default(),
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    run_outcome: None,
                },
            )
            .unwrap();
        let local = crate::host::local::LocalHostConnector::new(
            None, None, None, None, Arc::clone(&store), tmp.path().to_path_buf(),
        );
        let hosts = HostRegistry::new(
            rupu_workspace::HostStore { root: tmp.path().join("hosts") },
            Arc::new(local),
        );
        let got = run_transcript_paths_resolved(&store, &hosts, "run_01USAGE");
        assert_eq!(got, vec![PathBuf::from("/remote/proj/.rupu/transcripts/run_01A.jsonl")]);
    }
}
```

(The SSH mapping itself is covered by Task 3's `ssh_local_transcript_path_maps_into_the_host_cache`; this test pins the fall-through so a local/session `worker_id` can never break usage.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cp resolved_paths_tests 2>&1 | tail -5`
Expected: compile error, `run_transcript_paths_resolved` not found.

- [ ] **Step 3: Implement**

`usage.rs`:

```rust
/// [`run_transcript_paths`] with each path mapped through the run's host
/// connector (spec §6.3): a run executed by a mirror-backed host (SSH) reads
/// its transcripts from the coordinator-local cache. A `worker_id` that is
/// not a registered host (a local session worker, or none) maps nothing.
pub fn run_transcript_paths_resolved(
    store: &RunStore,
    hosts: &crate::host::registry::HostRegistry,
    run_id: &str,
) -> Vec<PathBuf> {
    let paths = run_transcript_paths(store, run_id);
    let Ok(rec) = store.load(run_id) else { return paths };
    let Some(worker) = rec.worker_id.as_deref() else { return paths };
    let Ok(conn) = hosts.resolve(worker) else { return paths };
    if !conn.serves_runs_from_local_mirror() {
        return paths;
    }
    paths.iter().map(|p| conn.local_transcript_path(p)).collect()
}

/// [`summarize_run`], host-resolved.
pub fn summarize_run_resolved(
    store: &RunStore,
    hosts: &crate::host::registry::HostRegistry,
    run_id: &str,
    pricing: &PricingConfig,
) -> UsageSummary {
    summarize_paths(&run_transcript_paths_resolved(store, hosts, run_id), pricing)
}
```

Callers — switch each to the resolved variant, passing `&s.hosts` (all have `s: AppState`/`State(s)` in scope):
- `api/graph.rs`: `build_run_graph_json(store, pricing, id)` gains a `hosts: &HostRegistry` parameter; its body calls `summarize_run_resolved(store, hosts, id, pricing)`; update its three callers (`run_graph_from_host`, and the `Global`/`ProjectLocal` branches) to pass `&s.hosts`. The `MockConn`-based tests in that file construct a state with a registry already (they call `resolve_host`), so they compile unchanged; if a test calls `build_run_graph_json` directly, pass `&s.hosts` there too.
- `api/agents.rs:584`, `api/usage.rs:430`, `api/netflow.rs:1122` → `run_transcript_paths_resolved(&s.run_store, &s.hosts, &r.id)` (netflow passes its local `store` variable in place of `&s.run_store`).
- `api/run_streams.rs:924,971`, `api/usage_outliers.rs:155` → `summarize_run_resolved(&s.run_store, &s.hosts, id, &s.pricing)`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp 2>&1 | tail -5` then `cargo clippy -p rupu-cp --all-targets 2>&1 | tail -3`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/usage.rs crates/rupu-cp/src/api/graph.rs crates/rupu-cp/src/api/agents.rs crates/rupu-cp/src/api/usage.rs crates/rupu-cp/src/api/netflow.rs crates/rupu-cp/src/api/run_streams.rs crates/rupu-cp/src/api/usage_outliers.rs
git add crates/rupu-cp/src/usage.rs crates/rupu-cp/src/api/
git commit -m "feat(cp): usage rollups read SSH-hosted run transcripts from the mirror cache

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: Orchestrator — placed steps and units record the dispatcher's transcript path; `host` on step results

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`UnitDispatch` ~121, `UnitDispatcher` trait ~150, `StepResult` ~464 + its `Default` ~535, placed-step site ~6106, fan-out prepare loop ~6570–6600, linear `StepResult` literal ~6235, every other `StepResult {` literal the compiler flags), `crates/rupu-orchestrator/src/runs.rs` (`StepResultRecord` ~565, `From` impls ~703/757), `crates/rupu-cp/src/host/transcript_paths.rs` + `ssh.rs` + `usage.rs` test literals (add `host: None`)

**Interfaces:**
- Produces:
  - `UnitDispatch.run_id` documented as the per-unit run id the remote executes under.
  - `UnitDispatcher::unit_transcript_path(&self, host: &str, unit_run_id: &str) -> Option<PathBuf>` (default `None`).
  - `StepResult.host: Option<String>`, `StepResultRecord.host: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`).

- [ ] **Step 1: Write the failing tests**

In `runner.rs`'s test module, next to `placed_step_output_feeds_downstream` (~8534). These reuse that section's `make_opts(wf, dir, dispatcher)` (which sets `run_store: None`, `event_sink: None`) and `run_workflow(opts)`, whose result exposes in-memory `step_results`. Add a path-aware dispatcher and a sink that records path-carrying events:

```rust
    /// A dispatcher that knows where a unit's transcript will be mirrored
    /// BEFORE dispatch — the fleet dispatcher's contract after this arc.
    struct PathDispatcher {
        seen_run_ids: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl UnitDispatcher for PathDispatcher {
        async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
            self.seen_run_ids.lock().unwrap().push(unit.run_id.clone());
            Ok(UnitOutcome {
                output: format!("out-{}-on-{host}", unit.index),
                success: true,
                error: None,
                workspace_delta: None,
            })
        }
        fn unit_transcript_path(&self, host: &str, unit_run_id: &str) -> Option<PathBuf> {
            Some(PathBuf::from(format!("/mirror/{host}/{unit_run_id}.jsonl")))
        }
    }

    /// Records every transcript path announced on the live stream.
    struct PathSink {
        working: Mutex<Vec<PathBuf>>,
        started: Mutex<Vec<PathBuf>>,
    }

    impl crate::executor::EventSink for PathSink {
        fn emit(&self, _run_id: &str, ev: &crate::executor::Event) {
            match ev {
                crate::executor::Event::StepWorking { transcript_path: Some(p), .. } => {
                    self.working.lock().unwrap().push(p.clone())
                }
                crate::executor::Event::UnitStarted { transcript_path, .. } => {
                    self.started.lock().unwrap().push(transcript_path.clone())
                }
                _ => {}
            }
        }
    }

    /// The runner must record and announce the dispatcher's path, not a
    /// coordinator-local one that is never written.
    #[tokio::test]
    async fn placed_step_records_the_dispatcher_transcript_path_and_host() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(PathDispatcher { seen_run_ids: Mutex::new(Vec::new()) });
        let sink = Arc::new(PathSink { working: Mutex::new(Vec::new()), started: Mutex::new(Vec::new()) });
        let wf = Workflow::parse(WF_PLACED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher.clone());
        opts.inputs.insert("what".into(), "rupu".into());
        opts.event_sink = Some(sink.clone());

        let result = run_workflow(opts).await.expect("run ok");

        let sr = &result.step_results[0];
        assert_eq!(sr.step_id, "build");
        assert_eq!(sr.host.as_deref(), Some("worker-1"));
        assert!(sr.transcript_path.starts_with("/mirror/worker-1/run_"), "{:?}", sr.transcript_path);
        assert_eq!(
            sr.transcript_path,
            PathBuf::from(format!("/mirror/worker-1/{}.jsonl", sr.run_id)),
            "the path is keyed by the SAME run id the unit was dispatched under"
        );
        assert_eq!(dispatcher.seen_run_ids.lock().unwrap().as_slice(), &[sr.run_id.clone()]);
        assert_eq!(sink.working.lock().unwrap().as_slice(), &[sr.transcript_path.clone()]);
    }

    /// Each placed step records ITS OWN host and a path keyed by that host
    /// (both steps placed so the section's `PanicFactory` is never hit).
    #[tokio::test]
    async fn each_placed_step_records_its_own_host_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(PathDispatcher { seen_run_ids: Mutex::new(Vec::new()) });
        let yaml = "name: two-hosts\nsteps:\n  - id: build\n    agent: builder\n    prompt: \"build it\"\n    host: worker-1\n  - id: report\n    agent: reporter\n    prompt: \"x\"\n    host: worker-2\n";
        let wf = Workflow::parse(yaml).unwrap();
        let opts = make_opts(wf, dir.path().to_path_buf(), dispatcher);
        let result = run_workflow(opts).await.expect("run ok");
        assert_eq!(result.step_results[0].host.as_deref(), Some("worker-1"));
        assert_eq!(result.step_results[1].host.as_deref(), Some("worker-2"));
        assert!(result.step_results[1].transcript_path.starts_with("/mirror/worker-2/run_"));
    }

    #[tokio::test]
    async fn distributed_units_record_the_dispatcher_transcript_path() {
        let dir = tempfile::tempdir().unwrap();
        let dispatcher = Arc::new(PathDispatcher { seen_run_ids: Mutex::new(Vec::new()) });
        let sink = Arc::new(PathSink { working: Mutex::new(Vec::new()), started: Mutex::new(Vec::new()) });
        let wf = Workflow::parse(WF_DISTRIBUTED).unwrap();
        let mut opts = make_opts(wf, dir.path().to_path_buf(), dispatcher.clone());
        opts.event_sink = Some(sink.clone());

        let result = run_workflow(opts).await.expect("run ok");

        let step = &result.step_results[0];
        assert_eq!(step.items.len(), 4);
        for (i, item) in step.items.iter().enumerate() {
            let host = if i % 2 == 0 { "h1" } else { "h2" };
            assert_eq!(
                item.transcript_path,
                PathBuf::from(format!("/mirror/{host}/{}.jsonl", item.run_id)),
                "unit {i}"
            );
        }
        let mut started = sink.started.lock().unwrap().clone();
        started.sort();
        let mut want: Vec<PathBuf> = step.items.iter().map(|i| i.transcript_path.clone()).collect();
        want.sort();
        assert_eq!(started, want, "UnitStarted announced the mirror path for every unit");
    }
```

(`WF_PLACED` and `WF_DISTRIBUTED` are the constants those sections already define; `Mutex` is `std::sync::Mutex`, already imported by the fake dispatcher there.)

`runs.rs` test (next to the `UnitCheckpoint` round-trip test ~5213):

```rust
    #[test]
    fn step_result_record_host_round_trips_and_defaults_absent() {
        let legacy = r#"{"step_id":"s","run_id":"r","transcript_path":"/t.jsonl","output":"","success":true,"skipped":false,"rendered_prompt":"","finished_at":"2026-09-04T00:00:00Z"}"#;
        let rec: StepResultRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(rec.host, None);
        let json = serde_json::to_string(&rec).unwrap();
        assert!(!json.contains("\"host\""), "absent, not null: {json}");
        let mut placed = rec.clone();
        placed.host = Some("h1".into());
        let back: StepResultRecord = serde_json::from_str(&serde_json::to_string(&placed).unwrap()).unwrap();
        assert_eq!(back.host.as_deref(), Some("h1"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-orchestrator placed_step_records distributed_units_record step_result_record_host 2>&1 | tail -15`
Expected: compile errors (`unit_transcript_path` not a trait member, no `host` field).

- [ ] **Step 3: Implement**

`runner.rs`:

`UnitDispatch.run_id` doc comment:

```rust
    /// The run id this unit executes under on the remote host — minted by the
    /// runner per unit (`run_<ULID>`), so the coordinator knows the child
    /// run's id (and therefore its mirrored transcript path, see
    /// [`UnitDispatcher::unit_transcript_path`]) before dispatch.
    pub run_id: String,
```

Trait method (in `UnitDispatcher`, after `prepare_workspace`):

```rust
    /// Where `unit_run_id`'s transcript will be readable on the coordinator
    /// once the unit runs on `host` — known before dispatch because the
    /// runner mints the id. `None` (the default) means "unknown": the runner
    /// falls back to its local `transcript_dir` path.
    fn unit_transcript_path(&self, _host: &str, _unit_run_id: &str) -> Option<PathBuf> {
        None
    }
```

`StepResult`: add `pub host: Option<String>,` with doc `/// Host that executed this step. None = local. Some(id) = a remote fleet host (a placed step).`; set `host: None` in the `Default` impl and in every `StepResult {` literal the compiler flags that lacks `..Default::default()`.

Placed-step site (~6106):

```rust
    let run_id = format!("run_{}", Ulid::new());
    let transcript_path = step
        .host
        .as_deref()
        .zip(opts.unit_dispatcher.as_ref())
        .and_then(|(host, d)| d.unit_transcript_path(host, &run_id))
        .unwrap_or_else(|| opts.transcript_dir.join(format!("{run_id}.jsonl")));
```

Linear `StepResult` literal (~6235): add `host: step.host.clone(),`.

Fan-out: move the `distribute_hosts` extraction (currently after the prepare loop, ~6595) to before the prepare loop, and in the loop replace the two `run_id`/`transcript_path` lines with:

```rust
        let run_id = format!("run_{}", Ulid::new());
        let placed_on: Option<&str> = distribute_hosts
            .as_ref()
            .map(|hosts| hosts[idx % hosts.len()].as_str());
        let transcript_path = placed_on
            .zip(opts.unit_dispatcher.as_ref())
            .and_then(|(host, d)| d.unit_transcript_path(host, &run_id))
            .unwrap_or_else(|| opts.transcript_dir.join(format!("{run_id}.jsonl")));
```

`runs.rs`: `StepResultRecord` gains

```rust
    /// Host that executed this step. Absent for local steps and for every
    /// record written before the field existed (mirrors `UnitCheckpoint::host`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
```

`From<&StepResult> for StepResultRecord`: `host: sr.host.clone(),`; `From<&StepResultRecord> for StepResult`: `host: rec.host.clone(),`. Fix any other `StepResultRecord {` literal the compiler flags (`runs.rs` tests, `mirror.rs`'s `synthesize_transcript_step_result`, `rupu-cli` printers, `rupu-cp` tests from Tasks 1/3/6/7) with `host: None`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-orchestrator 2>&1 | tail -5` then `cargo build --workspace 2>&1 | tail -3` then `cargo test -p rupu-cp 2>&1 | tail -3`
Expected: green across the workspace.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/runs.rs
git add -A crates/
git commit -m "feat(orchestrator): placed steps/units record the dispatcher's mirrored transcript path; host on step results

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: CLI dispatcher passes the minted run id; SSH honours it

**Files:**
- Modify: `crates/rupu-cp/src/agent_launcher.rs` (`AgentLaunchRequest`), `crates/rupu-cp/src/host/ssh.rs` (`launch_agent` ~1614), `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (struct + constructors ~215–235, `dispatch_unit` launch ~283, `build_dispatcher_if_needed` ~569, tests), every `AgentLaunchRequest {` literal the compiler flags (`api/agents.rs`, `host/local.rs`, `host/http.rs`, `host/tunnel.rs`, tests)

**Interfaces:**
- Consumes: Task 8's `unit_transcript_path`, `UnitDispatch.run_id`.
- Produces:
  - `AgentLaunchRequest.run_id: Option<String>` — honoured by connectors that mint the run id themselves (SSH); ignored elsewhere this arc.
  - `FleetUnitDispatcher { resolver, global: PathBuf }`; `new(registry, global)`, `from_connector(conn, global)`.
  - `UnitDispatcher::unit_transcript_path` for `FleetUnitDispatcher`: `Some(<global>/transcripts/<id>.jsonl)` when the host's connector serves runs from the local mirror, else `None`.

- [ ] **Step 1: Write the failing tests**

`ssh.rs` (next to `launch_agent_dispatches_in_staged_working_dir`):

```rust
    #[tokio::test]
    async fn launch_agent_honours_a_coordinator_minted_run_id() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let id = conn
            .launch_agent(crate::agent_launcher::AgentLaunchRequest {
                agent: "reviewer".into(),
                prompt: Some("go".into()),
                mode: None,
                target: None,
                working_dir: None,
                run_id: Some("run_01MINTEDBYCOORD".into()),
            })
            .await
            .unwrap();
        assert_eq!(id, "run_01MINTEDBYCOORD");
        assert_eq!(run_store.load(&id).unwrap().worker_id.as_deref(), Some("host_abc"));
        let cmds = fake.commands.lock().unwrap();
        assert!(cmds.iter().any(|c| c.contains("'--run-id' 'run_01MINTEDBYCOORD'")), "{cmds:?}");
    }

    #[tokio::test]
    async fn launch_agent_rejects_a_malformed_supplied_run_id_without_dispatching() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let err = conn
            .launch_agent(crate::agent_launcher::AgentLaunchRequest {
                agent: "reviewer".into(),
                prompt: None,
                mode: None,
                target: None,
                working_dir: None,
                run_id: Some("../evil".into()),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)), "{err}");
        assert!(fake.commands.lock().unwrap().is_empty(), "nothing shelled");
    }
```

`fleet_unit_dispatcher.rs` tests (the `FakeConnector` there returns a fixed `run_id`; extend it to record the launch request):

```rust
    #[tokio::test]
    async fn dispatch_passes_the_units_run_id_to_launch_agent() {
        let conn = Arc::new(FakeConnector::completed());
        let d = FleetUnitDispatcher::from_connector(Arc::clone(&conn) as Arc<dyn HostConnector>, PathBuf::from("/g"));
        let mut unit = make_unit();
        unit.run_id = "run_01UNITID".into();
        d.dispatch_unit(unit, "h1").await.unwrap();
        assert_eq!(
            conn.launched_run_id.lock().unwrap().as_deref(),
            Some("run_01UNITID")
        );
    }

    #[test]
    fn unit_transcript_path_is_the_mirror_layout_for_mirror_backed_hosts() {
        let conn = Arc::new(FakeConnector::completed()); // serves_runs_from_local_mirror → true below
        let d = FleetUnitDispatcher::from_connector(conn, PathBuf::from("/g"));
        assert_eq!(
            d.unit_transcript_path("h1", "run_01X"),
            Some(PathBuf::from("/g/transcripts/run_01X.jsonl"))
        );
    }

    #[test]
    fn unit_transcript_path_is_unknown_for_wire_only_hosts() {
        // `UnreachableConnector` is a unit struct that keeps the trait's
        // `serves_runs_from_local_mirror` default (false).
        let conn = Arc::new(UnreachableConnector);
        let d = FleetUnitDispatcher::from_connector(conn, PathBuf::from("/g"));
        assert_eq!(d.unit_transcript_path("h1", "run_01X"), None);
    }
```

`FakeConnector`: add `launched_run_id: std::sync::Mutex<Option<String>>` (init `Default::default()` in every constructor), set it in `launch_agent` (`*self.launched_run_id.lock().unwrap() = req.run_id.clone();` — change its `_req` parameter to `req`), and add `fn serves_runs_from_local_mirror(&self) -> bool { true }` to its `HostConnector` impl (it models the SSH connector).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp launch_agent_honours launch_agent_rejects 2>&1 | tail -8` and `cargo test -p rupu-cli fleet_unit_dispatcher 2>&1 | tail -8`
Expected: compile errors (`run_id` field, `from_connector` arity, `unit_transcript_path`).

- [ ] **Step 3: Implement**

`agent_launcher.rs`:

```rust
    /// Run id to execute under, when the caller already minted one (a
    /// placed unit's coordinator, see `UnitDispatch::run_id`). Honoured by
    /// connectors that mint ids themselves (SSH); the local and HTTP
    /// connectors ignore it this arc. `None` → the connector mints.
    pub run_id: Option<String>,
```

Fix every `AgentLaunchRequest {` literal the compiler flags with `run_id: None`.

`ssh.rs` `launch_agent`: replace `let run_id = format!("run_{}", Ulid::new());` with

```rust
        let run_id = match req.run_id.as_deref() {
            Some(id) => {
                let ok = id.starts_with("run_")
                    && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !ok {
                    return Err(HostConnectorError::Invalid(format!(
                        "supplied run id {id:?} is not a valid run id"
                    )));
                }
                id.to_string()
            }
            None => format!("run_{}", Ulid::new()),
        };
```

(Validated before `build_remote_command` so nothing is shelled for a bad id, and before `create_run` so no mirror record is left behind.)

`fleet_unit_dispatcher.rs`:

```rust
pub struct FleetUnitDispatcher {
    resolver: Resolver,
    /// The CP global dir: `NodeMirror` mirrors a placed unit's transcript to
    /// `<global>/transcripts/<run_id>.jsonl` (PR #646), which is what
    /// `unit_transcript_path` hands the runner.
    global: PathBuf,
}

impl FleetUnitDispatcher {
    pub fn new(registry: Arc<HostRegistry>, global: PathBuf) -> Self {
        Self { resolver: Resolver::Registry(registry), global }
    }
    pub fn from_connector(conn: Arc<dyn HostConnector>, global: PathBuf) -> Self {
        Self { resolver: Resolver::Fixed(conn), global }
    }
}
```

In `dispatch_unit`, the `AgentLaunchRequest { … }` literal gains `run_id: Some(unit.run_id.clone()),` (clone before `unit` is partially moved; bind `let unit_run_id = unit.run_id.clone();` at the top of the function).

Trait impl addition:

```rust
    fn unit_transcript_path(&self, host: &str, unit_run_id: &str) -> Option<PathBuf> {
        let conn = self.resolver.resolve(host).ok()?;
        if !conn.serves_runs_from_local_mirror() {
            return None;
        }
        Some(self.global.join("transcripts").join(format!("{unit_run_id}.jsonl")))
    }
```

`build_dispatcher_if_needed`: `FleetUnitDispatcher::new(Arc::new(registry), global.to_path_buf())`. Update every other `FleetUnitDispatcher::new(` / `from_connector(` call the compiler flags (tests) with a `PathBuf::from("/g")` or the tempdir path.

Add `use std::path::PathBuf;` where missing.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp -p rupu-cli 2>&1 | tail -5` then `cargo clippy --workspace --all-targets 2>&1 | tail -3`
Expected: green.

- [ ] **Step 5: Commit**

```bash
rustfmt crates/rupu-cp/src/agent_launcher.rs crates/rupu-cp/src/host/ssh.rs crates/rupu-cli/src/fleet_unit_dispatcher.rs
git add -A crates/
git commit -m "feat(fleet): placed units launch under the coordinator-minted run id; dispatcher exposes the mirror path

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: Web client — `run` on both transcript URLs, `partial` badge

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (`getTranscript`, `subscribeTranscript` ~2635–2660), `crates/rupu-cp/web/src/lib/transcript.ts` (`TranscriptResponse` ~52), `crates/rupu-cp/web/src/components/TranscriptPanel.tsx` (fetch/subscribe effects ~74–136; badge ~190)
- Test: `crates/rupu-cp/web/src/components/TranscriptPanel.test.tsx`

**Interfaces:**
- Produces: `api.getTranscript(path, { host?, run? })`, `api.subscribeTranscript(path, onEvent, onError?, { host?, run? })`; `TranscriptResponse.partial?: boolean`.

- [ ] **Step 1: Write the failing tests**

Add to `TranscriptPanel.test.tsx`. That file spies on the real `api` object (`vi.spyOn(api, 'getTranscript')`) and restores spies in `afterEach`; add `waitFor` to its `@testing-library/react` import and a new `describe('TranscriptPanel remote reads', …)` block:

```tsx
it('forwards host and run id to the fetch and the stream', async () => {
  const getTranscript = vi.spyOn(api, 'getTranscript').mockResolvedValue({ events: [], summary: null });
  const subscribeTranscript = vi.spyOn(api, 'subscribeTranscript').mockReturnValue(() => {});
  render(
    <MemoryRouter>
      <TranscriptPanel path="/remote/.rupu/transcripts/run_01A.jsonl" live host="host_abc" runId="run_01PARENT" />
    </MemoryRouter>,
  );
  await waitFor(() => expect(getTranscript).toHaveBeenCalled());
  expect(getTranscript).toHaveBeenCalledWith('/remote/.rupu/transcripts/run_01A.jsonl', { host: 'host_abc', run: 'run_01PARENT' });
  expect(subscribeTranscript).toHaveBeenCalledWith(
    '/remote/.rupu/transcripts/run_01A.jsonl',
    expect.any(Function),
    expect.any(Function),
    { host: 'host_abc', run: 'run_01PARENT' },
  );
});

it('shows the partial badge when the server could not collect the whole transcript', async () => {
  vi.spyOn(api, 'getTranscript').mockResolvedValue({ events: [], summary: null, partial: true });
  render(
    <MemoryRouter>
      <TranscriptPanel path="/t/run-1.jsonl" live={false} host="host_abc" runId="run_01P" />
    </MemoryRouter>,
  );
  expect(await screen.findByText(/incomplete/i)).toBeInTheDocument();
});
```

Add to `api.test.ts` if it exists (otherwise skip; the panel test above pins the contract):

```ts
it('getTranscript appends host and run', async () => {
  // uses the file's existing fetch mock helper
  await api.getTranscript('/t/a.jsonl', { host: 'h', run: 'run_01X' });
  expect(lastRequestedUrl()).toBe('/api/transcript?path=%2Ft%2Fa.jsonl&host=h&run=run_01X');
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run TranscriptPanel 2>&1 | tail -15`
Expected: the two new tests fail (`run` not forwarded; no badge).

- [ ] **Step 3: Implement**

`transcript.ts`:

```ts
export interface TranscriptResponse {
  events: TranscriptEvent[];
  summary: TranscriptSummary | null;
  /** Lines the server could not parse (absent on older servers). */
  unparsed?: number;
  /** The coordinator could not collect the rest of this transcript from
   *  its host (spec §4.2). Absent unless true. */
  partial?: boolean;
}
```

`api.ts`:

```ts
  getTranscript(path: string, opts?: { host?: string; run?: string }): Promise<TranscriptResponse> {
    let url = `/api/transcript?path=${encodeURIComponent(path)}`;
    if (opts?.host) url += `&host=${encodeURIComponent(opts.host)}`;
    if (opts?.run) url += `&run=${encodeURIComponent(opts.run)}`;
    return request<TranscriptResponse>(url);
  },

  subscribeTranscript(
    path: string,
    onEvent: (e: TranscriptEvent) => void,
    onError?: (e: Event) => void,
    opts?: { host?: string; run?: string },
  ): () => void {
    let url = `/api/transcript/stream?path=${encodeURIComponent(path)}`;
    if (opts?.host) url += `&host=${encodeURIComponent(opts.host)}`;
    if (opts?.run) url += `&run=${encodeURIComponent(opts.run)}`;
    const es = new EventSource(url);
    es.onmessage = (m) => onEvent(JSON.parse(m.data) as TranscriptEvent);
    if (onError) es.onerror = onError;
    return () => es.close();
  },
```

`TranscriptPanel.tsx`:
- state: `const [partial, setPartial] = useState(false);` reset to `false` alongside `setUnparsed(undefined)`; after `setUnparsed(res.unparsed)` add `setPartial(res.partial === true);`.
- fetch: `api.getTranscript(path, { host, run: runId })`; deps `[path, host, runId]`.
- subscribe: `{ host, run: runId }`; deps `[path, live, host, runId]`.
- badge, next to the unparsed badge (same styling classes as that badge, `AlertTriangle` icon already imported):

```tsx
          {partial && (
            <span className="inline-flex items-center gap-1 rounded-md border border-warn/30 bg-warn-bg px-1.5 py-0.5 text-[11px] text-warn" title="The coordinator could not reach the host to collect the rest of this transcript">
              <AlertTriangle className="h-3 w-3" />
              incomplete
            </span>
          )}
```

- [ ] **Step 4: Run the tests**

Run: `cd crates/rupu-cp/web && npx vitest run 2>&1 | tail -8 && npx tsc --noEmit 2>&1 | tail -3`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): transcript panel forwards run id to remote reads and shows the partial badge

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 11: macOS — remote runs stream and tail; `partial` notice

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuAPI/CPClient.swift` (`transcript` ~152), `apps/rupu-macos/RupuKit/Sources/RupuAPI/TranscriptModels.swift` (`APITranscriptPage` ~323), `apps/rupu-macos/RupuKit/Sources/RupuStore/BackendController.swift` (`makeRunEventStream` ~318, `makeTranscriptStream` ~331), `apps/rupu-macos/RupuKit/Sources/RupuStore/RunDetailStore.swift` (convenience init ~274–300, `activate()` ~355–370, `focusStep` ~580–588, `focusPath` ~600–662, `transcriptPartial` property near `transcriptUnparsedCount` ~154, `startTail` ~1085, `makeTranscriptTailFactory` ~1268 and its run-stream sibling), `apps/rupu-macos/RupuKit/Sources/RupuRunDetail/TranscriptFeed.swift` (init ~98–120, notice ~144), `apps/rupu-macos/RupuKit/Sources/RupuRunDetail/RunDetailTabs.swift:140`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuStoreTests/RunDetailStoreTests.swift` (~930–1000)

**Interfaces:**
- Produces: `CPClient.transcript(path:host:run:)`, `BackendController.makeTranscriptStream(path:host:run:onConnectionChange:)`, `BackendController.makeRunEventStream(runID:host:onConnectionChange:)`, `APITranscriptPage.partial: Bool?`, `RunDetailStore.transcriptPartial: Bool`, `TranscriptFeed(…, partial:)`.

- [ ] **Step 1: Write the failing tests**

Replace `remoteStoreNeverInvokesStreamFactoriesEvenWhenSupplied` with:

```swift
// MARK: - (f) remote store streams and tails like a local one (SSH lazy mirror)

@MainActor @Test func remoteStoreStartsTheRunStreamAndTailsItsRunningStep() async {
    let runBox = RunStreamBox()
    let tailBox = TailStreamBox()
    let store = makeStore(
        isRemote: true,
        detailResult: {
            detail(run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"), steps: [])
        },
        runStreamBox: runBox,
        tailStreamBox: tailBox
    )

    await store.activate()

    #expect(store.isRemote == true)
    #expect(runBox.callCount == 1, "the run event stream is opened for a remote run")
    #expect(tailBox.callCount == 1, "a running remote step is tailed, not snapshotted")
    #expect(tailBox.latestPath == "t/build.jsonl")
    #expect(store.transcriptTailActive == true)
    #expect(store.detail.value != nil)

    store.deactivate()
    tailBox.latest.finish()
    runBox.latest.finish()
}

@MainActor @Test func focusPathSurfacesThePartialFlagFromTheSnapshot() async {
    // No tail box → REST snapshot path.
    let store = makeStore(
        isRemote: true,
        detailResult: {
            detail(run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"), steps: [])
        },
        transcriptResult: { _ in APITranscriptPage(events: [], summary: nil, unparsed: nil, partial: true) }
    )

    await store.activate()

    #expect(store.focusedTranscriptPath == "t/build.jsonl")
    #expect(store.transcriptPartial == true)

    store.deactivate()
}
```

Keep `remoteStoreStillRunsInitialFocusStepViaRESTOnly` as-is (it passes no boxes, so it still exercises the REST-only fallback).

Add to `apps/rupu-macos/RupuKit/Tests/RupuAPITests/TranscriptDecodingTests.swift`:

```swift
@Test func transcriptPageDecodesPartialWhenPresentAndNilOtherwise() throws {
    let with = try JSONDecoder().decode(APITranscriptPage.self, from: Data(#"{"events":[],"summary":null,"partial":true}"#.utf8))
    #expect(with.partial == true)
    let without = try JSONDecoder().decode(APITranscriptPage.self, from: Data(#"{"events":[],"summary":null}"#.utf8))
    #expect(without.partial == nil)
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `make macos-test 2>&1 | tail -20`
Expected: compile errors (`partial:` init label, `transcriptPartial`), then the stream test fails on `runBox.callCount`.

- [ ] **Step 3: Implement**

`TranscriptModels.swift` — `APITranscriptPage`: add `public let partial: Bool?` with doc `/// Spec §4.2: the coordinator could not collect the rest of this transcript from its host. Absent unless true.`; init gains `partial: Bool? = nil`; `CodingKeys` gains `partial`; decoder: `partial = try container.decodeIfPresent(Bool.self, forKey: .partial)`.

`CPClient.swift`:

```swift
    public func transcript(path: String, host: String? = nil, run: String? = nil) async throws -> APITranscriptPage {
        var query = [URLQueryItem(name: "path", value: path)]
        query.append(contentsOf: hostQuery(host))
        if let run { query.append(URLQueryItem(name: "run", value: run)) }
        return try await get("api/transcript", query: query)
    }
```

`BackendController.swift`:

```swift
    public func makeRunEventStream(runID: String, host: String? = nil, onConnectionChange: (@Sendable (Bool) -> Void)? = nil) -> JSONEventStream<CPEvent>? {
        guard let config = activeConfig,
              var components = URLComponents(url: config.baseURL.appendingPathComponent("api/events/stream"), resolvingAgainstBaseURL: false)
        else { return nil }
        var items = [URLQueryItem(name: "run", value: runID)]
        if let host, host != "local" { items.append(URLQueryItem(name: "host", value: host)) }
        components.queryItems = items
        guard let url = components.url else { return nil }
        return JSONEventStream<CPEvent>(url: url, token: config.token, onConnectionChange: onConnectionChange)
    }

    public func makeTranscriptStream(path: String, host: String? = nil, run: String? = nil, onConnectionChange: (@Sendable (Bool) -> Void)? = nil) -> JSONEventStream<TranscriptEvent>? {
        guard let config = activeConfig,
              var components = URLComponents(url: config.baseURL.appendingPathComponent("api/transcript/stream"), resolvingAgainstBaseURL: false)
        else { return nil }
        var items = [URLQueryItem(name: "path", value: path)]
        if let host, host != "local" { items.append(URLQueryItem(name: "host", value: host)) }
        if let run { items.append(URLQueryItem(name: "run", value: run)) }
        components.queryItems = items
        guard let url = components.url else { return nil }
        return JSONEventStream<TranscriptEvent>(url: url, token: config.token, onConnectionChange: onConnectionChange)
    }
```

`RunDetailStore.swift`:
- Property: `public private(set) var transcriptPartial: Bool = false` next to `transcriptUnparsedCount`; reset to `false` at every site that sets `transcriptUnparsedCount = 0`; in `focusPath`'s REST branch set `transcriptPartial = page.partial ?? false` next to `unparsedCount = page.unparsed ?? 0` (carry it as a local like `unparsedCount` and assign under the generation guard); in `reloadTranscriptSnapshot` do the same.
- Convenience init: `fetchTranscript: { path in try await client.transcript(path: path, host: host, run: runID) }`; run-signals factory and tail factory are built unconditionally (delete the `isRemote ? nil :` prefixes), passing `host` and `runID` through: `makeTranscriptTailFactory(backend:host:runID:)` calls `backend.makeTranscriptStream(path: path, host: host, run: runID, onConnectionChange: onChange)`; the run-signals factory calls `backend.makeRunEventStream(runID: runID, host: host, onConnectionChange: …)`.
- `activate()`: replace `if !isRemote { startRunStream() }` with `startRunStream()`; update the comment to say remote runs stream through the mirror (`/api/events/stream?run=&host=`).
- `focusStep`: `let willTail = path != nil && isStepRunning(stepID) && transcriptTailFactory != nil`.
- Update the type doc comment's "Local vs remote" paragraph: `isRemote` now only affects which endpoints carry `host`; nothing is gated off.

`TranscriptFeed.swift`: add `private let partial: Bool`, init parameter `partial: Bool = false` on both inits, and in the body before the unparsed notice:

```swift
                    if partial {
                        MetaLineRow(
                            label: "warning",
                            detail: "Transcript incomplete — the host could not be reached to collect the rest"
                        )
                    }
```

`RunDetailTabs.swift:140`: add `partial: store.transcriptPartial`.

- [ ] **Step 4: Run the tests and build**

Run: `make macos-test 2>&1 | tail -15 && make macos-build 2>&1 | tail -3`
Expected: all Swift tests pass; build green. Run `make macos-fixtures` and `cargo test -p rupu-cp macos_fixtures` — no serde type the fixtures cover changed (`partial` is a handler-level field), so the check is expected to be a no-op; if it regenerates anything, commit the regenerated file.

- [ ] **Step 5: Commit**

```bash
git add apps/rupu-macos
git commit -m "feat(macos): remote runs stream events and tail transcripts; partial-transcript notice

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 12: Docs, memory, and live verification

**Files:**
- Modify: `CLAUDE.md` (the `rupu-cp` / `rupu-cli` bullets under **Crates**), `/Users/matt/.claude/projects/-Users-matt-Code-Oracle-rupu/memory/project_remote_transcript_gap.md` + `MEMORY.md`

- [ ] **Step 1: Docs**

Add to `CLAUDE.md`'s `rupu-mcp`/`rupu-orchestrator` area a `rupu-cp` note (there is no `rupu-cp` bullet today; add one after `rupu-app-canvas`):

```
- **`rupu-cp`** — control-plane server. SSH-hosted transcripts are served through a lazy mirror (`docs/superpowers/specs/2026-09-04-rupu-ssh-lazy-transcript-mirror-design.md`): `host/transcript_paths.rs` (what a mirrored run claims + the `<global>/mirror/<host_id>/transcripts/<key>.jsonl` cache layout), `host/lazy_tail.rs` (one shared `tail -F` per open file), the SSH tail pump's terminal batched pull with `.complete` sidecars, and `/api/transcript{,/stream}?path=&host=&run=` (`run` required for a not-yet-mirrored remote path; `partial: true` when the host could not be reached to finish collecting). Placed units launch under the coordinator-minted `UnitDispatch.run_id` so their PR #646 mirror path is known before dispatch. Tunnel/bucket hosts implement `local_transcript_path` / `ensure_transcript_feed` / `pull_transcript` to join.
```

- [ ] **Step 2: Memory**

Rewrite `project_remote_transcript_gap.md` to record that the SSH side is implemented on branch `claude/ssh-workflow-transcripts-2c3041` per the 2026-09-04 spec, that tunnel/bucket remain open with the three connector hooks as the contract, and keep the original root-cause chain as history. Update its `MEMORY.md` line to: `— SSH fixed via lazy mirror (spec 2026-09-04); tunnel/bucket still open, contract = 3 connector hooks`.

- [ ] **Step 3: Full verification**

Run, in order, from the worktree:

```bash
cargo clippy --workspace --all-targets 2>&1 | tail -3
```

```bash
cargo test --workspace 2>&1 | tail -5
```

```bash
cd crates/rupu-cp/web && npx vitest run 2>&1 | tail -3 && npx tsc --noEmit
```

```bash
make macos-test 2>&1 | tail -3 && make macos-build 2>&1 | tail -3
```

Expected: all green. Then `make cp-web` so the embedded web UI in any local `make install` carries Task 10.

- [ ] **Step 4: Live check on the mini host (§11)**

Restart the local `rupu cp serve` daemon first (it is a persistent process and will not pick up new code otherwise). Then, batching ssh usage and never probing the host by hand:

1. Launch a workflow on the SSH host from the Launcher; open it in rupu.app and the web while running. Expect: the active step's transcript fills live; switching steps opens each one; `~/.rupu/mirror/<host_id>/transcripts/` gains one file per opened step, no `.complete` yet.
2. Let it finish. Expect: every step transcript now has a `.complete` sidecar (including steps never opened); the run's cost column is populated; stop the host's ssh (or block it) and reopen the run — transcripts still render, no `partial` badge.
3. Launch a local workflow with a `host:` placed step. Expect: the parent run's placed step shows its transcript live from the first line, and the child run in Activity shows the same file.
4. Launch a standalone agent on the SSH host. Expect: the transcript opens live before the run finishes.
5. While a remote step is open in both clients, `ps | grep 'tail -n +1 -F'` on the coordinator shows exactly one ssh child for that file; close both viewers and it disappears within a few seconds.

Record the outcome of each in the PR description. matt runs the app before merge (GUI validation rule).

- [ ] **Step 5: Commit and open the PR**

```bash
git add CLAUDE.md
git commit -m "docs: SSH lazy transcript mirror in CLAUDE.md

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git push -u origin claude/ssh-workflow-transcripts-2c3041
gh pr create --title "feat: SSH lazy transcript mirror — live + terminal transcripts for SSH-hosted runs" --body-file - <<'EOF'
Closes the remote-transcript visibility gap for SSH hosts per docs/superpowers/specs/2026-09-04-rupu-ssh-lazy-transcript-mirror-design.md.

- Lazy shared `tail -F` into `<global>/mirror/<host_id>/transcripts/` while a step is focused; batched pull of every step transcript at terminal with `.complete` sidecars; `partial: true` when the host cannot be reached to finish.
- `/api/transcript{,/stream}` gain `run=`; a remote read is scoped to the run that recorded the path.
- Placed units launch under the coordinator-minted run id, so the parent run points at the #646 mirror from the first line; standalone SSH agent runs expose their transcript live via the active step.
- Usage rollups read SSH-hosted transcripts from the cache.
- macOS: remote runs stream events and tail; web: run id forwarded; both show the partial badge.
- Tunnel/bucket: unchanged; contract is the three new `HostConnector` hooks.

Live check results: (fill in from Task 12 step 4)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
```
