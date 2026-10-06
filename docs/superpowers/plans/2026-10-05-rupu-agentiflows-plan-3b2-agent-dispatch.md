# Agentiflows Plan 3b-2 — agent-unit dispatch (non-blocking, process-isolated) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the agentiflow lead **dispatch a pool agent as its own detached subprocess** — non-blocking (the lead gets a handle and keeps planning), process-isolated (crash isolation + per-unit SIGTERM), attached to the shared board/mailboxes, pooling its findings into the agentiflow scope the goal evaluator reads — and `join` it. Plus the two carry-forward `rupu-fleet` correctness fixes a real (multi-participant) fleet needs. End state: `run_agentiflow`'s lead holds `dispatch`/`join` Tier-1/2 tools over a `FleetSupervisor`; a dispatched unit runs as `rupu run <agent> --fleet-run-dir … --fleet-participant …`, writes findings to `target_id(workspace, agentiflow_id)`, posts to the board, and is observable/joinable/reapable.

**Architecture:** The legacy in-process fork-join `dispatch_agent` (`rupu-tools`) is **untouched** (spec §10.1). The agentiflow fleet uses a NEW non-blocking, process-isolated path in `rupu-agentiflow`: a `UnitLauncher` port (fire → poll, mirroring rupu-cp's `HostConnector` split but local and lean), a `SubprocessUnitLauncher` that spawns a detached `rupu run` with `process_group(0)` + a pre-minted run id + the engagement set, and a `FleetSupervisor` that tracks units (pid + `Child`), polls status (started-evidence + `try_wait`), joins, terminates (SIGTERM by pid), and reaps zombies. `rupu run` gains a hidden fleet-attach flag pair that makes a spawned unit build the 3b-1 `fleet_tools` + collectors and pool findings into the agentiflow scope — this pulls `rupu-agentiflow` into `rupu-cli` (acyclic). The lead's `dispatch`/`join` tools are `extra_tools` (as in 3b-1) driving the supervisor.

**Tech Stack:** Rust 2021; `rupu-fleet` (board/mailbox + the two fixes; `ulid` added for the claim nonce); `rupu-agentiflow` (the launcher/supervisor/tools; `rustix` added for pid signals, `ulid` for unit ids); `rupu-agent` (`AgentRunOpts`, `run_agent`, `fleet_tools`/`lead_collectors` from 3b-1); `rupu-cli` (`rupu run` attach hook; gains a `rupu-agentiflow` dep); `rupu_transcript::final_turn_text` (unit outcome); `std::process`/`std::os::unix::process::CommandExt` (detached spawn); `tokio`, `serde_json`, `chrono`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` — §9 (board/mailboxes, incl. the participant/broadcast model), §10 (dispatch — non-blocking, process-isolated, §10.1 legacy untouched, §10.2 detached subprocess + tracked pid + reaper, §10.3 MAX_DEPTH), §13 (Tier-2 dispatch/join), §19 (run dir: `units/<unit_id>/`). Read §10 in full. Builds on 3a (`run_agentiflow`, `LeadConfig`) + 3b-1 (`fleet_tools`, `lead_collectors`, `FleetToolCtx`, the pooled-scope wiring, `AgentRunOpts.extra_tools`).

## Global Constraints

- **Legacy dispatch is byte-for-byte untouched (spec §10.1).** Do NOT modify `rupu-tools`'s `AgentDispatcher`, `dispatch_agent`, `dispatch_agents_parallel`, or `ToolContext.dispatcher`/`dispatchable_agents`. The agentiflow fleet is a separate path.
- **Non-blocking + process-isolated (spec §10.2).** A unit is a detached subprocess (`process_group(0)`, stdio null) with a tracked pid and a retained `Child` (for `try_wait`/zombie reaping). `dispatch` returns a handle immediately; `join` polls to terminal. Per-unit cancel is SIGTERM by pid. No in-process fork-join.
- **Fail-closed.** `dispatch` refuses an agent not in the agentiflow `pool` (pool ⊇ dispatched agents); refuses past `MAX_DEPTH` (=5); a spawn/attach error surfaces as `ToolOutput.error`, never a panic. A unit's findings MUST pool into `target_id(workspace, agentiflow_id)` (not the unit's agent-name scope) — the same pooling fix 3b-1 made for the lead, now for units.
- **Lean deps, no cycles.** `rupu-agentiflow` MAY gain `rustix` (pid signals) + `ulid`; do NOT add `rupu-orchestrator` (heavy — reimplement the ~10-line `pid_is_running`/`terminate_pid` rustix wrappers). `rupu-cli` gains `rupu-agentiflow` (acyclic: agentiflow depends on agent/runtime/coverage/tools/fleet, none of which is cli). `rupu-fleet` gains `ulid`.
- **Workspace-pinned deps only;** `[lints] workspace = true`; no `unsafe`; `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate; `thiserror` for library errors.
- **Unix-only process control is fine** (macOS + Linux + WSL, matching `rupu-fleet`'s existing `flock`); gate pid/signal code `#[cfg(unix)]` with a compile-erroring stub elsewhere, mirroring `rupu-fleet`.

## Scope boundary

3b-2 includes ONLY: the two `rupu-fleet` fixes; the `rupu run` fleet-attach hook; the process-isolated `UnitLauncher`/`SubprocessUnitLauncher`/`FleetSupervisor`; the lead's `dispatch`/`join` tools + pool check + agent-unit profile propagation; and wiring into `run_agentiflow`. It does NOT include: **workflow** units / `run_workflow` (3b-3 — `rupu workflow run` has no engagement-profile carrier yet), `generate_workflow`, the **roster** (`agents.list`/`workflows.list`/`catalog.search` + `RosterCollector`), `board.directive` write tool, the `goal/budget/coverage.status` pull-tools (3b-3); the verify path (3c); the CLI `rupu agentiflow` command + CP surfaces (Plan 4). Sub-leads/recursive dispatch are out (spec §25: v1 dispatch-only; a unit gets no dispatcher).

## File Structure

- `crates/rupu-fleet/Cargo.toml` — add `ulid` (workspace).
- `crates/rupu-fleet/src/types.rs` — modify: `ClaimRecord` gains `token`; `ClaimGuard` gains `lock_path` + `token`; `Drop` does a locked compare-and-delete.
- `crates/rupu-fleet/src/board.rs` — modify: mint + store the token on grant; build the richer `ClaimGuard`.
- `crates/rupu-fleet/src/mailbox.rs` — modify: `broadcast_send` + `read_broadcast` (append-only log + per-reader cursor).
- `crates/rupu-agentiflow/src/collectors.rs` — modify: `MailboxCollector` drains own inbox only + delivers broadcast via cursor.
- `crates/rupu-agentiflow/src/tools.rs` — modify: `msg.send` routes `to == "broadcast"` to `broadcast_send`.
- `crates/rupu-agentiflow/Cargo.toml` — add `rustix` (features `["process"]`), `ulid` (workspace).
- `crates/rupu-agentiflow/src/proc.rs` — **new**: `pid_is_running` / `terminate_pid` (rustix, `#[cfg(unix)]`).
- `crates/rupu-agentiflow/src/unit.rs` — **new**: `UnitLauncher` port, `UnitSpec`/`UnitId`/`UnitStatus`/`UnitOutcome`/`UnitError`, `MockUnitLauncher`.
- `crates/rupu-agentiflow/src/supervisor.rs` — **new**: `FleetSupervisor` (spawn/status/join/terminate_all; `units/<id>/` records).
- `crates/rupu-agentiflow/src/subprocess.rs` — **new**: `SubprocessUnitLauncher` (detached `rupu run` spawn, `Child`/pid, `try_wait`, transcript outcome).
- `crates/rupu-agentiflow/src/dispatch_tools.rs` — **new**: `dispatch` + `join` `rupu_tools::Tool` impls over the supervisor; `fleet_dispatch_tools(...)`.
- `crates/rupu-agentiflow/src/run.rs` — modify: build the supervisor, add `dispatch`/`join` to the lead's `extra_tools`, pass the engagement set + pool.
- `crates/rupu-agentiflow/src/lib.rs` — modify: new `mod`s + re-exports.
- `crates/rupu-cli/Cargo.toml` — add `rupu-agentiflow` (workspace).
- `crates/rupu-cli/src/cmd/run.rs` — modify: `--fleet-run-dir` + `--fleet-participant` flags + the attach branch in `run_inner`.

---

## Task 1: Fix `ClaimGuard` — a stale guard must not delete a successor's claim

**Files:** `crates/rupu-fleet/Cargo.toml` (+`ulid`), `crates/rupu-fleet/src/types.rs`, `crates/rupu-fleet/src/board.rs`.

**Interfaces:**
- Produces: `ClaimRecord { owner, acquired_at, lease_expires_at, token: String }` (`#[serde(default)]` on `token`); `ClaimGuard { path, lock_path, token }`; `ClaimGuard::drop` removes the claim file only if the on-disk record's `token` equals this guard's, under the per-key `.reaplock` flock.

**Why:** `ClaimGuard::drop` currently does an unconditional `std::fs::remove_file(&self.path)`. Sequence: owner A grants → lease (default 3600s) expires → owner B reaps A's lease and re-grants (same key/path) → A's guard drops and deletes B's live claim file. A per-grant `token` (unique even for same-owner re-claims) + a locked compare-and-delete closes it. The reap path (`reap_expired`, board.rs:131) is already safe (re-reads under the flock); only `Drop` is wrong.

- [ ] **Step 1: Failing test.** In `board.rs` tests:
```rust
#[test]
fn a_stale_guards_drop_does_not_delete_a_successors_claim() {
    let dir = tempfile::tempdir().unwrap();
    let board = Board::new(dir.path());
    // A grants with a 0s lease (immediately expired).
    let a = board.claim("host:1.1.2.2", "agent-a", std::time::Duration::from_secs(0)).unwrap();
    let ClaimOutcome::Granted(a_guard) = a else { panic!("A granted") };
    // B claims the same key: reaps A's expired lease, re-grants to B.
    let b = board.claim("host:1.1.2.2", "agent-b", std::time::Duration::from_secs(3600)).unwrap();
    assert!(matches!(b, ClaimOutcome::Granted(_)), "B granted after reaping A");
    // A's guard drops (A's run ends). It must NOT delete B's live claim.
    drop(a_guard);
    assert_eq!(board.claim_holder("host:1.1.2.2").unwrap().as_deref(), Some("agent-b"),
        "B still holds the key after A's stale guard dropped");
}
```

- [ ] **Step 2: Run → FAIL** (A's drop deletes B's claim; holder becomes `None`).

- [ ] **Step 3: `ulid` dep.** Add `ulid = { workspace = true }` to `crates/rupu-fleet/Cargo.toml` `[dependencies]` (confirm it's in the root `[workspace.dependencies]` — it is; `rupu-codename`/orchestrator use it).

- [ ] **Step 4: `ClaimRecord` + `ClaimGuard`.** In `types.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClaimRecord {
    pub owner: String,
    pub acquired_at: String,
    pub lease_expires_at: String,
    /// Unique per grant; lets a guard delete only its OWN claim file.
    #[serde(default)]
    pub token: String,
}

#[derive(Debug)]
pub struct ClaimGuard {
    pub(crate) path: PathBuf,
    pub(crate) lock_path: PathBuf,
    pub(crate) token: String,
}
impl Drop for ClaimGuard {
    fn drop(&mut self) {
        // Compare-and-delete under the per-key reap lock: remove the file only
        // if it still holds OUR grant's token. A successor's re-grant has a
        // different token, so a stale guard can never delete a live claim.
        let Ok(lock) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&self.lock_path) else { return };
        if rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).is_err() { return; }
        // (blocking lock — Drop is best-effort but must be correct)
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                if let Ok(rec) = serde_json::from_slice::<ClaimRecord>(&bytes) {
                    if rec.token == self.token {
                        let _ = std::fs::remove_file(&self.path);
                    }
                }
            }
            Err(_) => {} // already gone / unreadable: nothing to do
        }
    }
}
```
(`types.rs` must `use` whatever `rupu-fleet` already uses for `rustix`; `board.rs` already uses `rustix::fs::flock` so the dep + import exist. Keep `ClaimRecord` `pub(crate)`.)

- [ ] **Step 5: Mint + carry the token on grant.** In `board.rs`, change `try_create_claim` to take/return the token (mint `ulid::Ulid::new().to_string()` inside, write it into the `ClaimRecord`, return it on success), and build the guard with it:
```rust
// in claim():
if let Some(token) = self.try_create_claim(&path, owner, ttl)? {   // now Result<Option<String>, _>
    return Ok(ClaimOutcome::Granted(ClaimGuard {
        path,
        lock_path: self.reap_lock_path(key),
        token,
    }));
}
```
`reap_lock_path` is already a private method (board.rs:36). Update `try_create_claim`'s signature + the `ClaimRecord` construction (add `token: token.clone()`), returning `Ok(Some(token))` on create, `Ok(None)` on `AlreadyExists`.

- [ ] **Step 6: Run → PASS.** Then the whole crate: `cargo test -p rupu-fleet` + `cargo clippy -p rupu-fleet --all-targets -- -D warnings`. Existing claim tests must stay green (the `#[serde(default)]` keeps old records readable; new ones carry a token).

- [ ] **Step 7: Commit.** `git add -A && git commit -m "fix(fleet): ClaimGuard deletes only its own grant (per-grant token)"`

---

## Task 2: Fix broadcast — append-only log + per-reader cursor (no consume-once starvation)

**Files:** `crates/rupu-fleet/src/mailbox.rs`; `crates/rupu-agentiflow/src/collectors.rs`; `crates/rupu-agentiflow/src/tools.rs`.

**Interfaces:**
- Produces: `Mailbox::broadcast_send(&self, msg: &FleetMessage, cap: usize) -> Result<(), FleetError>` (appends to `mailboxes/broadcast/log.jsonl` under the broadcast `.lock`, capped by total lines); `Mailbox::read_broadcast(&self, participant: &str) -> Result<Vec<FleetMessage>, FleetError>` (under the participant's `.lock`, returns log lines past the reader's persisted cursor `mailboxes/<participant>/broadcast.cursor` and advances it; a first-time reader's cursor starts at the current log length — no backlog). `MailboxCollector` drains ONLY the participant's own inbox and additionally emits `read_broadcast(participant)` results (both `Cadence::Once`). `msg.send` routes `to == "broadcast"` to `broadcast_send`.

**Why:** `drain("broadcast")` renames the single broadcast inbox away, so the first reader consumes it for every reader. A shared append-only log with a per-reader cursor gives real fan-out (each reader sees each broadcast exactly once, nobody consumes for others).

- [ ] **Step 1: Failing tests** (`mailbox.rs`):
```rust
#[test]
fn broadcast_fans_out_to_every_reader_once() {
    let dir = tempfile::tempdir().unwrap();
    let mb = Mailbox::new(dir.path());
    mb.broadcast_send(&FleetMessage { from: "lead".into(), ts: "t".into(), body: "all hands".into() }, 256).unwrap();
    // two distinct readers each see it once
    let a1 = mb.read_broadcast("unit-a").unwrap();
    let b1 = mb.read_broadcast("unit-b").unwrap();
    assert_eq!(a1.len(), 1); assert_eq!(b1.len(), 1);
    assert_eq!(a1[0].body, "all hands"); assert_eq!(b1[0].body, "all hands");
    // neither sees it again (cursor advanced)
    assert!(mb.read_broadcast("unit-a").unwrap().is_empty());
    assert!(mb.read_broadcast("unit-b").unwrap().is_empty());
}

#[test]
fn a_late_reader_starts_at_the_current_log_tip() {
    let dir = tempfile::tempdir().unwrap();
    let mb = Mailbox::new(dir.path());
    mb.broadcast_send(&FleetMessage{from:"lead".into(),ts:"t".into(),body:"early".into()}, 256).unwrap();
    // late joiner's first read sees no backlog...
    assert!(mb.read_broadcast("late").unwrap().is_empty());
    mb.broadcast_send(&FleetMessage{from:"lead".into(),ts:"t".into(),body:"after".into()}, 256).unwrap();
    // ...but does see broadcasts sent after it first read.
    let got = mb.read_broadcast("late").unwrap();
    assert_eq!(got.len(), 1); assert_eq!(got[0].body, "after");
}
```

- [ ] **Step 2: Run → FAIL** (methods don't exist).

- [ ] **Step 3: Implement in `mailbox.rs`.** Reuse the existing `lock_exclusive`/`count_lines`/`read_messages` helpers and the `mailboxes/<sanitized>/` layout. `broadcast_send` appends to `self.root.join("mailboxes").join("broadcast").join("log.jsonl")` under `lock_path("broadcast")`, capped by `count_lines`. `read_broadcast(p)` under `lock_path(p)`: read the cursor file `mailboxes/<sanitize(p)>/broadcast.cursor` (a single integer line, default = current broadcast-log line count on first read — write it and return empty), read the broadcast log, return lines `[cursor..]`, write the new cursor = total lines. Use line counts (not byte offsets) since the log is append-only JSONL. Add a `broadcast_log_path()` + `broadcast_cursor_path(p)` helper. Handle a missing log (no broadcast ever sent) as empty + cursor 0.

- [ ] **Step 4: Run → PASS**; add a cap test if cheap. `cargo test -p rupu-fleet mailbox::` then full crate + clippy. Commit rupu-fleet side:
`git add -A && git commit -m "feat(fleet): broadcast log + per-reader cursor (fan-out, no consume-once)"`

- [ ] **Step 5: Update the agentiflow consumers (failing test first).** In `collectors.rs`, change `MailboxCollector::collect` to (a) drain ONLY `self.participant`'s inbox (DROP the `"broadcast"` drain), and (b) ALSO call `self.mailbox.read_broadcast(&self.participant)` and emit each as an `Injection{ kind: Message, cadence: Once, priority: 200, source: format!("broadcast:{}", self.participant) }`. Update the 3b-1 broadcast test (`broadcast` is no longer drained destructively). In `tools.rs`, `MsgSend::invoke`: if `to == "broadcast"`, call `self.0.mailbox.broadcast_send(&msg, self.0.msg_cap)`, else the existing `send`. Add a test: two `FleetToolCtx` on the same mailbox, one `msg.send(broadcast)`, both see it via a `MailboxCollector` once.

- [ ] **Step 6: Run → PASS** (`cargo test -p rupu-agentiflow` + clippy). Commit:
`git add -A && git commit -m "feat(agentiflow): deliver broadcasts via per-reader cursor, not a destructive drain"`

---

## Task 3: `rupu run` fleet-attach hook — a spawned unit joins the board + pools findings

**Files:** `crates/rupu-cli/Cargo.toml` (+`rupu-agentiflow`); `crates/rupu-cli/src/cmd/run.rs`.

**Interfaces:**
- Produces: `rupu run` gains `--fleet-run-dir <PATH>` and `--fleet-participant <ID>` (hidden, `#[arg(hide = true)]`; both-or-neither). When set, `run_inner` builds the fleet attachment: `extra_tools = rupu_agentiflow::fleet_tools(Arc::new(FleetToolCtx::new(Board::new(run_dir), Mailbox::new(run_dir), participant)))`, `collectors = rupu_agentiflow::lead_collectors(mailbox, board, participant)`, `scope_name = Some(<run_dir basename>)` (the agentiflow id), and `findings_engagement` from the already-resolved `--engagement-profile` set. Extract the build into a testable helper `fleet_attachment(run_dir: &Path, participant: &str, engagement: Option<Arc<ActiveSet>>) -> FleetAttachment { extra_tools, collectors, scope_name }`.

**Why (dossier):** `rupu run` hard-codes `collectors: Vec::new()`, `extra_tools: Vec::new()`, `scope_name: None`. A spawned unit must attach to the SAME file-backed board/mailboxes the lead created (`<global>/agentiflows/<id>/`) and pool findings into `target_id(workspace, <id>)` — otherwise its findings land under `target_id(workspace, agent_name)`, invisible to the goal evaluator (the identical bug 3b-1 fixed for the lead). The run dir's basename IS the agentiflow id, so one path yields both the stores and the scope.

- [ ] **Step 1: Cargo dep.** Add `rupu-agentiflow = { workspace = true }` to `crates/rupu-cli/Cargo.toml` `[dependencies]` (confirm in root workspace deps; add there if missing). `cargo build -p rupu-cli` to confirm no cycle.

- [ ] **Step 2: Failing test** (in `run.rs` tests or a `cmd::run` unit test): call `fleet_attachment(tmp_run_dir, "unit-x", Some(active))` and assert `scope_name == Some(tmp_run_dir.file_name())`, `extra_tools` contains a `board.post` tool (by `name()`), and `collectors` is non-empty. Plus a clap parse test: `--fleet-run-dir /x --fleet-participant u` parses; one-without-the-other is a usage error (enforce both-or-neither in `run_inner`, returning a clear error).

- [ ] **Step 3: Add the flags + the helper + the branch.** Add the two `#[arg(long, hide = true)]` fields to the run args struct (near `run_id`). Write `fleet_attachment(...)`. In `run_inner`, where `AgentRunOpts` is built (dossier: `run.rs:1097-1142`), when `fleet_run_dir` is set, set `extra_tools`/`collectors`/`scope_name`/`tool_context.findings` from the helper (engagement comes from the existing `resolve_engagement` result already computed at run.rs:969). Keep the non-fleet path byte-for-byte unchanged.

- [ ] **Step 4: Run → PASS.** `cargo test -p rupu-cli --lib` (or the relevant `tests/it` module) + `cargo clippy -p rupu-cli --all-targets -- -D warnings`. NOTE (per repo rules): a new top-level `tests/*.rs` is banned — put any integration test under `rupu-cli/tests/it/` or as a `#[cfg(test)]` unit test; cwd/env-mutating tests go in `tests/serial/` holding `ENV_LOCK`.

- [ ] **Step 5: Commit.** `git add -A && git commit -m "feat(cli): rupu run --fleet-run-dir/--fleet-participant attaches a unit to the agentiflow board"`

---

## Task 4: The unit port + `FleetSupervisor` + `MockUnitLauncher` + pid wrappers

**Files:** `crates/rupu-agentiflow/Cargo.toml` (+`rustix` features `["process"]`, +`ulid`); `crates/rupu-agentiflow/src/proc.rs` (new); `crates/rupu-agentiflow/src/unit.rs` (new); `crates/rupu-agentiflow/src/supervisor.rs` (new); `lib.rs`.

**Interfaces:**
- Produces:
```rust
// proc.rs  (#[cfg(unix)]; a non-unix stub is a compile_error! like rupu-fleet)
pub fn pid_is_running(pid: u32) -> bool;   // rustix kill(pid, None); EPERM => alive
pub fn terminate_pid(pid: u32) -> bool;    // rustix kill(pid, SIGTERM); refuses own pid

// unit.rs
pub struct UnitSpec { pub agent: String, pub prompt: String, pub engagement: Vec<String>, pub participant: String }
pub struct UnitId(pub String);             // the pre-minted run id ("run_<ULID>")
pub enum UnitStatus { Pending, Running, Done(UnitOutcome), Failed(String) }
pub struct UnitOutcome { pub output: String, pub success: bool }
pub enum UnitError { Spawn(String), Unknown }
pub trait UnitLauncher: Send + Sync {
    /// Fire a unit; return its handle immediately (non-blocking).
    fn spawn(&self, spec: &UnitSpec, run_dir: &Path) -> Result<UnitId, UnitError>;
    /// Current status (started-evidence + liveness + terminal outcome). Cheap, pollable.
    fn poll(&self, id: &UnitId, run_dir: &Path) -> UnitStatus;
    /// SIGTERM the unit; best-effort.
    fn terminate(&self, id: &UnitId);
}
pub struct MockUnitLauncher { /* scripted: id -> Vec<UnitStatus> drained per poll, + a spawn side effect closure */ }

// supervisor.rs
pub struct FleetSupervisor { launcher: Arc<dyn UnitLauncher>, run_dir: PathBuf, units: Mutex<HashMap<String, UnitRec>> }
impl FleetSupervisor {
    pub fn new(launcher: Arc<dyn UnitLauncher>, run_dir: PathBuf) -> Self;
    pub fn dispatch(&self, spec: UnitSpec) -> Result<String, UnitError>;  // returns UnitId string; writes units/<id>/unit.json
    pub fn status(&self, id: &str) -> UnitStatus;                          // delegates to launcher.poll
    pub fn join(&self, id: &str, timeout: Duration, now: &dyn Fn() -> DateTime<Utc>) -> UnitStatus; // poll to terminal/timeout
    pub fn terminate_all(&self);                                          // SIGTERM every non-terminal unit (envelope wind-down)
    pub fn live_ids(&self) -> Vec<String>;
}
```
- `MAX_DEPTH`/pool checks live in the dispatch TOOL (Task 6), not the supervisor.

- [ ] **Step 1: Cargo deps** — `rustix = { workspace = true, features = ["process"] }`, `ulid = { workspace = true }` in `rupu-agentiflow/Cargo.toml` (both already workspace deps). `cargo build -p rupu-agentiflow`.

- [ ] **Step 2: `proc.rs` + tests.** Mirror `rupu-orchestrator::runs::pid_is_running`/`terminate_pid` (dossier §2): `rustix::process::kill_process(Pid::from_raw(pid as i32)?, None|Signal::Term)`; `pid_is_running` treats `Errno::PERM` as alive; `terminate_pid` refuses `std::process::id()`. `#[cfg(not(unix))] compile_error!("agentiflow unit control is unix-only")`. Tests: `pid_is_running(std::process::id())` is true; `pid_is_running(0x7fff_fffe)` (an unlikely-live pid) is false; `terminate_pid(std::process::id())` returns false (refuses self).

- [ ] **Step 3: `unit.rs` — the port + the mock (test-first).** Define the types above. `MockUnitLauncher`: constructed with a per-unit scripted `Vec<UnitStatus>` (each `poll` pops the next, staying on the last) and an optional `on_spawn: Box<dyn Fn(&UnitSpec, &Path) + Send + Sync>` side effect (so a test's mock unit can write a finding into the pooled scope + a board post, simulating a real unit). `spawn` mints `UnitId("run_"+ULID)`, records the script, runs `on_spawn`, returns the id. Test the mock: spawn → poll returns Pending then Running then Done per the script.

- [ ] **Step 4: `supervisor.rs` — test-first with the mock.**
```rust
#[test]
fn supervisor_dispatches_tracks_and_joins_via_the_mock() {
    let dir = tempfile::tempdir().unwrap();
    let launcher = Arc::new(MockUnitLauncher::scripted(vec![
        UnitStatus::Running, UnitStatus::Done(UnitOutcome { output: "found it".into(), success: true }),
    ]));
    let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
    let id = sup.dispatch(UnitSpec { agent: "recon".into(), prompt: "scan".into(), engagement: vec![], participant: "recon#1".into() }).unwrap();
    // units/<id>/unit.json written
    assert!(dir.path().join("units").join(&id).join("unit.json").is_file());
    // join polls to terminal
    let out = sup.join(&id, Duration::from_secs(5), &|| Utc::now());
    assert!(matches!(out, UnitStatus::Done(o) if o.success && o.output.contains("found it")));
}
```
Implement: `dispatch` calls `launcher.spawn`, records `UnitRec{agent, started_at, status: Pending}` in the map, writes `units/<id>/unit.json` (agent, run_id, participant, started_at, status). `status`/`join` delegate to `launcher.poll` (join loops with a short sleep until terminal or timeout, using the injected clock for the deadline; a real sleep is fine here — keep it small, e.g. 100ms, and cap total by `timeout`). `terminate_all` calls `launcher.terminate` for every unit whose last status wasn't terminal. `join` updates `unit.json` on terminal. No `.unwrap()` on fallible IO in non-test code — a `unit.json` write failure is `tracing::warn!`, not a panic (the in-memory map is the source of truth).

- [ ] **Step 5: lib.rs** — `mod proc; mod unit; mod supervisor;` + `pub use unit::{UnitLauncher, UnitSpec, UnitId, UnitStatus, UnitOutcome, UnitError, MockUnitLauncher}; pub use supervisor::FleetSupervisor;`. Full crate test + clippy.

- [ ] **Step 6: Commit.** `git add -A && git commit -m "feat(agentiflow): UnitLauncher port + FleetSupervisor + mock + pid wrappers"`

---

## Task 5: `SubprocessUnitLauncher` — spawn `rupu run` as a detached, tracked subprocess

**Files:** `crates/rupu-agentiflow/src/subprocess.rs` (new); `lib.rs`.

**Interfaces:**
- Produces: `SubprocessUnitLauncher { exe: PathBuf, global: PathBuf }` implementing `UnitLauncher`. `spawn` launches `rupu run <agent> --run-id <id> --mode bypass --prompt <p> [--engagement-profile a,b] --fleet-run-dir <run_dir> --fleet-participant <participant>` detached (`process_group(0)`, stdio null, `current_dir(workspace)`...), retains the `Child` + records the pid in an internal `Mutex<HashMap<String, Live>>` (`Live { child: Child, pid: u32 }`). `poll` = started-evidence (non-empty `<global>/transcripts/<id>.jsonl`) + `try_wait()` on the `Child` (exited? → read the outcome) + `pid_is_running` fallback; on exit, read the unit's outcome from its transcript via `rupu_transcript::final_turn_text`. `terminate` = `terminate_pid(pid)`.

**Why (dossier):** the existing launchers discard the `Child`, don't record pids, and ignore the supplied run id; `rupu run` writes `run.json` only at the end with `runner_pid: None`, so `reap_if_orphaned` can't see it. The agentiflow supervisor therefore owns unit liveness itself: a retained `Child` gives `try_wait` (status + zombie reaping) and the pid gives SIGTERM; the transcript (written incrementally from `run.rs:1070`) gives both started-evidence and the final outcome.

- [ ] **Step 1: Failing test with a trivial command.** To test the lifecycle without a built `rupu` binary, make `SubprocessUnitLauncher` build its argv through a small seam: an `ArgvBuilder` fn (default = the real `rupu run …` argv) injectable in tests. Test A (argv): assert the default builder emits `run`, `--run-id <id>`, `--mode bypass`, `--fleet-run-dir <run_dir>`, `--fleet-participant <p>`, and `--engagement-profile a,b` when the set is non-empty (and omits it when empty). Test B (lifecycle, unix): a launcher configured to spawn `/bin/sh -c 'sleep 0.3'` — `spawn` returns an id; `poll` is `Running` immediately; `pid_is_running(pid)` true; after the child exits, `poll` becomes terminal and `try_wait` reaped it (no zombie — assert a second `poll` is still terminal and doesn't error); `terminate` on a `sleep 5` child makes it exit promptly.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement.** Detached spawn idiom (dossier §2): `let mut cmd = std::process::Command::new(&self.exe); cmd.args(argv); cmd.stdin(null()).stdout(null()).stderr(null()); if let Some(cwd) = workspace { cmd.current_dir(cwd); } #[cfg(unix)] { use std::os::unix::process::CommandExt; cmd.process_group(0); }` then `let child = cmd.spawn().map_err(|e| UnitError::Spawn(e.to_string()))?; let pid = child.id();` — store `Live { child, pid }`. `poll`: lock the map; `match live.child.try_wait() { Ok(Some(status)) => terminal (read outcome), Ok(None) => if started_evidence { Running } else { Pending }, Err(_) => Failed }`. Reading the outcome: `rupu_transcript` reader over `<global>/transcripts/<id>.jsonl` → `final_turn_text`; `success = status.success()`. `terminate`: look up the pid, `crate::proc::terminate_pid(pid)`. Started-evidence: the transcript file exists and is non-empty (dossier: local evidence is a non-empty `<global>/transcripts/<run_id>.jsonl`).
   - **Zombie note:** always `try_wait` in `poll` so an exited child is reaped; a `SubprocessUnitLauncher` dropped with live children should `terminate` + `try_wait` them in its own `Drop` (best-effort) — add that.
   - `rupu_transcript` is already a dep of `rupu-agentiflow`? If not, add it (workspace) — it's a leaf crate, acyclic.

- [ ] **Step 4: Run → PASS** (unix). Full crate test + clippy. Gate the lifecycle test `#[cfg(unix)]`.

- [ ] **Step 5: lib.rs** — `mod subprocess;` + `pub use subprocess::SubprocessUnitLauncher;`. Commit:
`git add -A && git commit -m "feat(agentiflow): SubprocessUnitLauncher — detached rupu run units with tracked pids"`

---

## Task 6: The lead's `dispatch`/`join` tools + pool check + profile propagation + wiring + e2e

**Files:** `crates/rupu-agentiflow/src/dispatch_tools.rs` (new); `crates/rupu-agentiflow/src/run.rs`; `lib.rs`.

**Interfaces:**
- Produces: `dispatch` + `join` as `rupu_tools::Tool` impls over an `Arc<FleetSupervisor>` + the run's `pool`/`engagement`/`depth`. `fleet_dispatch_tools(sup: Arc<FleetSupervisor>, pool: Arc<Vec<String>>, engagement: Vec<String>) -> Vec<Arc<dyn Tool>>`. `run_agentiflow` builds a `FleetSupervisor` (prod: `SubprocessUnitLauncher { exe: current_exe, global }`), and the lead's `extra_tools` = `fleet_tools(...)` (3b-1) ++ `fleet_dispatch_tools(...)`.
- `dispatch { "agent": "...", "prompt": "..." }`: refuse if `agent ∉ pool` (`ToolOutput.error`, fail-closed); refuse if the lead's depth ≥ `MAX_DEPTH` (=5; the lead is depth 0, so this guards a future sub-lead — keep the check); else `sup.dispatch(UnitSpec { agent, prompt, engagement: engagement.clone(), participant: <minted unit codename/id> })` → returns the handle (unit id) as `ToolOutput.stdout` (JSON `{ "handle": "<id>" }`).
- `join { "handle": "...", "timeout_secs"?: n }`: `sup.join(handle, timeout, now)` → the unit's outcome (JSON: status + output). Bounded default timeout (e.g. 300s).

- [ ] **Step 1: Failing e2e test** (`run.rs` tests, `MockUnitLauncher`): the mock's `on_spawn` writes a finding into the pooled scope (`CoveragePaths::new(workspace, target_id(workspace, id))`, the same shape Task-3 units use) and a board post; scripted `poll`: Running → Done. Script the lead to call `dispatch {"agent":"recon","prompt":"scan"}` (recon ∈ the test def's pool), then `join {"handle": <id>}`, then stop. Assert via `CapturingMockProvider` requests + store state: the lead received a handle, the join returned the unit's output, the pooled ledger has the unit's finding, and `units/<id>/unit.json` exists. Also assert `dispatch {"agent":"not-in-pool",...}` returns an error (fail-closed).
  - Inject the `MockUnitLauncher` into `run_agentiflow` for the test: add a seam — `RunAgentiflowOpts` gains an optional `unit_launcher: Option<Arc<dyn UnitLauncher>>` (None ⇒ prod `SubprocessUnitLauncher`). Keep it `#[cfg_attr(not(test), …)]`-free but documented as a test seam / Plan-4 injection point.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement the tools** (`dispatch_tools.rs`) + the pool/depth checks + minting a unit participant codename (reuse `rupu-codename` if a `CrewNamer` is in reach, else `format!("{}#{}", agent, n)` with an atomic counter — keep it simple and deterministic; the lead's participant is `"lead"`, a unit's is its codename). Wire into `run.rs`: build the supervisor (from `opts.unit_launcher` or the subprocess default), `pool = Arc::new(def.pool.agents…)`, `engagement = def.engagement_profiles…`, and extend the lead's `extra_tools` with `fleet_dispatch_tools(...)`. `terminate_all` on the supervisor at envelope wind-down (after the lead's final round) so a stop SIGTERMs live units — call it in `run_agentiflow` after `envelope.run` returns, before writing the final record.

- [ ] **Step 4: Run → PASS.** Full crate test + clippy. Guard any subprocess-touching bits so the default-path tests use the mock.

- [ ] **Step 5: Commit.** `git add -A && git commit -m "feat(agentiflow): lead dispatch/join tools over the process-isolated fleet supervisor"`

---

## Self-Review

- **Spec coverage:** §10.1 legacy untouched (no `rupu-tools` dispatch changes) ✓; §10.2 detached subprocess + tracked pid + SIGTERM + reaper (Tasks 4/5) ✓; §10.3 `MAX_DEPTH` + pool-gated spawn (Task 6) ✓; §9 board/mailbox fixes (Tasks 1/2) ✓; §13 Tier-2 dispatch/join (Task 6) ✓; §19 `units/<id>/` (Task 4) ✓; profile propagation to agent units via `--engagement-profile` (Tasks 3/6) ✓. Deferred (stated in Scope boundary): workflow units/`run_workflow`, `generate_workflow`, roster, `board.directive`, status pull-tools (3b-3); verify (3c); CLI/CP (Plan 4).
- **Type consistency:** `UnitLauncher`/`UnitSpec`/`UnitStatus` identical across unit.rs (def), supervisor.rs (consumer), subprocess.rs (impl), dispatch_tools.rs (via supervisor); `fleet_attachment` (Task 3) reuses 3b-1's `fleet_tools`/`lead_collectors`/`FleetToolCtx` exactly; the pooled-scope target `target_id(workspace, agentiflow_id)` is identical for lead (3a), units (Task 3), and the e2e assertion (Task 6).
- **Placeholder scan:** the only "read the real thing" is the exact `rupu run` arg-struct field names (Task 3 — dossier gave them: `run.rs:25-101`) and the `rupu_transcript` reader API (Task 5). The ArgvBuilder/`unit_launcher` seams are named with signatures.
- **Fail-closed:** pool membership + `MAX_DEPTH` on dispatch; spawn/attach errors → `ToolOutput.error`; unit findings pool into the agentiflow scope (never the agent-name scope); `terminate_pid` refuses self; `ClaimGuard` compare-and-delete.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-10-05-rupu-agentiflows-plan-3b2-agent-dispatch.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, task review between, broad review at the end. Tasks 4+5 (the process-isolation core) are the high-risk ones; review them closely and run their `#[cfg(unix)]` lifecycle tests.
2. **Inline Execution** — execute in this session with checkpoints.

3b-3 (workflow units + `run_workflow` + roster + `generate_workflow` + `board.directive`) is the next plan and depends on this one (it reuses this supervisor/launcher for workflow units and this attach model).
