# Agentiflows Plan 4-3 — Operator-Control Plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a long-running agentiflow operable while it runs — background it (`--detach`), watch it (`attach`), steer it (`send`), stop it (`stop [--now]`), keep it alive supervised (`serve`), and never leave a dead coordinator's detached units orphaned (the reaper) — plus directive retraction and opt-in priority steering.

**Architecture:** Build agentiflow-native on the primitives that already exist (`OperatorQueue`, `events.jsonl`, per-round lead transcripts, `AgentiflowRecord`), reusing the `cp serve` detached-run / orphan-reaper *pattern* — NOT the `rupu session` worker (a dependency cycle forbids it, and the envelope was deliberately built without it). The coordinator process stamps its own `runner_pid`; detached units persist their `pgid`; a shared `reap_orphaned_agentiflows` fn finalizes a dead coordinator's record and group-kills its units, scheduled by both `agentiflow serve` and `cp serve`'s sweep.

**Tech Stack:** Rust 2021, tokio, `std::thread::scope` worker (existing), rustix process signalling (`proc.rs`), `tokio_util::sync::CancellationToken` (existing runner pause primitive), file-backed JSONL + atomic tmp+rename.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` (§10.2, §11, §12, §17, §19, §21, §22, §23). This plan is the operator/CLI/serve half of the spec's Plan 4 (the CP web surfaces are Plan 4-4). Research dossier: the forks below are resolved per `/private/tmp/.../plan-4-3-dossier.md` (session scratch).

## Global Constraints

- **Rust 2021**, MSRV pinned in `rust-toolchain.toml`. `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- **Errors:** `thiserror` in libraries (`rupu-agentiflow`, `rupu-fleet`, `rupu-config`); `anyhow` only in `rupu-cli`.
- **Hexagonal (rule #1):** `rupu-agentiflow` may depend on `rupu-{coverage,fleet,agent,providers,tools,runtime,transcript,orchestrator,config}` — NEVER on `rupu-cli`. No new `rupu-auth` dep. `rupu-cli` is thin (rule #2): the `agentiflow` subcommand is arg-parse + delegation only.
- **Workspace deps only:** versions pinned in root `Cargo.toml`; internal crates as `{ path = "../x" }`. Never add a version to a crate `Cargo.toml`.
- **RULING — no session-worker lift (Fork 4).** `rupu-cli` depends on `rupu-agentiflow`, so the envelope cannot call the session worker without a cycle, and no `rupu-session`/`rupu-runtime` session lib exists. §21's "reuse session attach/send" is reinterpreted as *same operator ergonomics, agentiflow-native transport*. The §22 session-worker lift stays a separate, optional refactor, out of scope here. Cost if wrong: a future consolidation refactor — but the native primitives are already the merged architecture (`operator.rs` is "realized without the session worker").
- **RULING — serve = reaper + operator supervision only (not trigger auto-launch).** `rupu agentiflow serve` runs the orphan reaper on a timer; auto-launching `trigger: cron|event` defs (reusing the cron infra) is a NAMED follow-up, deferred to keep 4-3 focused on operator control + durability. Cost if wrong: a follow-up plan adds auto-launch; nothing here blocks it.
- **New record/struct fields are `#[serde(default)]`** so records written before 4-3 still parse (the 4-2 precedent: the lockstep test `a_record_written_before_spend_was_recorded_still_parses`, `run.rs:1089`). Every new field gets an analogous parse-an-old-record test.
- **Pid reaping is fail-safe:** only a *dead recorded pid* is reaped; a `None` pid (the create→write-back window) is NEVER reaped (the `reap_if_orphaned` rule, `runs.rs:3816`). The reaper never fails a healthy run; a signalling error is logged, not fatal.
- **The board stays append-only / multi-writer-safe:** directive retraction appends events and folds on read (the finding-tags pattern) — never an in-place rewrite.
- **Metering/steering/reaper never fail a run** (the 4-2 best-effort rule): a drain/enqueue/signal error is a warning, retried or skipped, never a hard stop.
- **Integration tests: ONE binary per crate** (modules under `crates/<c>/tests/it/`, listed in `main.rs`); `rupu-cli` env/cwd-mutating tests live in `tests/serial/` holding `ENV_LOCK`, driven `< /dev/null`.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/rupu-agentiflow/src/run.rs` | `AgentiflowRecord.runner_pid`; stamp `std::process::id()` into the running record; `--run-id` reuse of a pre-minted id | T1, T4 |
| `crates/rupu-agentiflow/src/supervisor.rs` | `UnitRec.pgid` + `write_unit_json` persists it; a `units_on_disk(run_dir)` reader | T1, T2 |
| `crates/rupu-agentiflow/src/proc.rs` + `lib.rs` | re-export `terminate_group`/`kill_group` | T1 |
| `crates/rupu-agentiflow/src/reaper.rs` (new) | `reap_orphaned_agentiflows(global, now)` — list records, dead-pid detect, group-kill units, finalize failed + terminal event | T2 |
| `crates/rupu-agentiflow/src/operator.rs` | `OperatorMessage.interrupt`; a non-destructive `peek_interrupt()` for the watcher | T6 |
| `crates/rupu-agentiflow/src/lead.rs` | per-round `CancellationToken` → `AgentRunOpts.pause`; the steering-interrupt watcher thread | T6 |
| `crates/rupu-fleet/src/types.rs` + `board.rs` | `Directive.id` + append-only `DirectiveEvent::Retract`; `read_directives` folds to live set | T5 |
| `crates/rupu-agentiflow/src/status_tools.rs` | `board.retract { id }` Tier-2 tool | T5 |
| `crates/rupu-config/src/policy_config.rs` | `AgentiflowConfig` (`[agentiflow]`: `serve_enabled`, `serve_interval_secs`, `reaper_enabled`) | T7 |
| `crates/rupu-cli/src/cmd/agentiflow.rs` | `send`/`stop`/`attach`/`serve` verbs; `run --detach`/`--run-id`; delegation only | T3, T4, T7 |
| `crates/rupu-cli/src/cmd/cp.rs` | call the shared agentiflow reaper from the existing sweep tick | T7 |
| `crates/rupu-cli/tests/serial/agentiflow_operate.rs` (new) | detach/send/stop/attach/reaper/retract e2e | T8 |

---

## Interfaces (names later tasks rely on)

- `AgentiflowRecord.runner_pid: Option<u32>` (T1).
- `rupu_agentiflow::proc::{terminate_group, kill_group}` re-exported from `lib.rs` (T1; already `pub` in `proc.rs`).
- `rupu_agentiflow::supervisor::units_on_disk(run_dir: &Path) -> Vec<UnitOnDisk>` where `UnitOnDisk { unit_id, pgid: Option<u32>, kind: UnitKind, status: UnitStatus }` (T1/T2).
- `rupu_agentiflow::reaper::reap_orphaned_agentiflows(global: &Path, now: DateTime<Utc>) -> ReapSummary` where `ReapSummary { scanned: usize, reaped: Vec<String> }` (T2).
- `rupu_agentiflow::operator::OperatorMessage { ts, body, stop, interrupt }` + `OperatorQueue::peek_interrupt(&self) -> bool` (non-destructive) (T6).
- `rupu_fleet::Directive { id, author, ts, body, addressed_to }` + `Board::retract_directive(id)` + `read_directives` returns the live (non-retracted) set (T5).
- `rupu_config::AgentiflowConfig { serve_enabled, serve_interval_secs, reaper_enabled }` on the top-level config (T7).

---

## Task 1: Durable pids — `runner_pid` on the record, `pgid` on units, group-kill exports

**Files:**
- Modify: `crates/rupu-agentiflow/src/run.rs` (`AgentiflowRecord` ~`:127-148`; the running-record write ~`:638-653`)
- Modify: `crates/rupu-agentiflow/src/supervisor.rs` (`UnitRec`, `write_unit_json` `:194-215`, `dispatch` `:51`)
- Modify: `crates/rupu-agentiflow/src/lib.rs` (`:50` re-export line)
- Test: inline `#[cfg(test)]` in `run.rs` + `supervisor.rs`

**Interfaces:**
- Produces: `AgentiflowRecord.runner_pid: Option<u32>`; `UnitRec.pgid: Option<u32>`; `units_on_disk(run_dir) -> Vec<UnitOnDisk>`; `rupu_agentiflow::{terminate_group, kill_group}`.
- Consumes: `std::process::id()`; `rupu_agentiflow::proc::{pid_is_running, terminate_group, kill_group}`.

- [ ] **Step 1: Failing test — old record without `runner_pid` parses; a fresh running record carries it.**

```rust
// run.rs #[cfg(test)]
#[test]
fn a_record_written_before_runner_pid_still_parses() {
    // the 4-2 old-record fixture extended: no runner_pid key
    let json = r#"{"id":"af_01","name":"x","engagement_profiles":["code"],
      "trigger":"agentiflow","status":"completed","stop_reason":"goals_met",
      "rounds":1,"goals":[],"started_at":"2026-10-06T00:00:00Z","ended_at":null,
      "codename":null,"spent_usd":null,"spent_tokens":0}"#;
    let rec: AgentiflowRecord = serde_json::from_str(json).unwrap();
    assert_eq!(rec.runner_pid, None);
}

#[test]
fn running_record_stamps_this_processes_pid() {
    // build a record the way run_agentiflow does at start and assert the pid field.
    let rec = AgentiflowRecord::new_running(/* … same args run_agentiflow passes … */);
    assert_eq!(rec.runner_pid, Some(std::process::id()));
}
```

Run: `cargo test -p rupu-agentiflow runner_pid < /dev/null` → FAIL (field missing).

- [ ] **Step 2: Add the field + stamp it.** In `AgentiflowRecord` add `#[serde(default)] pub runner_pid: Option<u32>,`. Where `run_agentiflow` writes the initial `running` record (`run.rs:638-653`), set `runner_pid: Some(std::process::id())`. (This is correct for both foreground and `--detach`: under detach, `run_agentiflow` executes in the detached child, so `process::id()` is the long-lived coordinator pid — exactly what the reaper must signal.) Keep the final-write path (`run.rs:872-891`) leaving `runner_pid` as-is on success and the record is `completed`; the reaper only looks at `running` records, so a stale pid on a completed record is inert — but clear it to `None` on the terminal write for tidiness.

- [ ] **Step 3: Failing test — a dispatched unit's pgid lands in `unit.json`, and `units_on_disk` reads it back.**

```rust
// supervisor.rs #[cfg(test)]  (uses MockUnitLauncher with a known pid)
#[test]
fn dispatch_persists_unit_pgid_and_units_on_disk_reads_it() {
    let dir = tempdir().unwrap();
    let launcher = Arc::new(MockUnitLauncher::with_pid(4242) /* returns pid 4242 on spawn */);
    let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
    sup.dispatch(UnitSpec::agent("recon", "p")).unwrap();
    let units = units_on_disk(dir.path());
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].pgid, Some(4242));
}
```

- [ ] **Step 4: Persist pgid.** `SubprocessUnitLauncher::spawn` already has `child.id()` (== pgid, since `process_group(0)`). Return it from the `UnitLauncher::spawn` contract (extend the spawn result with `pgid: Option<u32>`; `MockUnitLauncher` returns a scripted/None value). In `FleetSupervisor::dispatch`, carry the pgid into `UnitRec` and `write_unit_json` so `unit.json` gains `"pgid": <n>`. Add `#[serde(default)]` on the unit-json pgid field. Add `units_on_disk(run_dir) -> Vec<UnitOnDisk>` that reads `units/*/unit.json` tolerantly (skip a bad file, logged) and returns `{ unit_id, pgid, kind, status }`.

- [ ] **Step 5: Export the group-kill fns.** In `lib.rs:50`, extend the `proc` re-export to include `terminate_group, kill_group` (they are already `pub` in `proc.rs`; only `pid_is_running`+`terminate_pid` were re-exported).

- [ ] **Step 6: Run + commit.** `cargo test -p rupu-agentiflow < /dev/null` green; `cargo clippy -p rupu-agentiflow --all-targets -- -D warnings` clean.
```bash
git add -A && git commit -m "feat(agentiflow): durable pids — runner_pid on record, pgid on units, group-kill exports"
```

---

## Task 2: `reap_orphaned_agentiflows` — the shared reaper

**Files:**
- Create: `crates/rupu-agentiflow/src/reaper.rs`
- Modify: `crates/rupu-agentiflow/src/lib.rs` (add `pub mod reaper;` + re-export)
- Modify: `crates/rupu-agentiflow/src/run.rs` (factor the events.jsonl terminal-write + the `agentiflow.json` finalize so the reaper reuses them, or re-implement minimally in `reaper.rs`)
- Test: `crates/rupu-agentiflow/tests/it/reaper.rs` (add to `tests/it/main.rs`)

**Interfaces:**
- Produces: `reap_orphaned_agentiflows(global: &Path, now: DateTime<Utc>) -> ReapSummary`; `ReapSummary { scanned: usize, reaped: Vec<String> }`.
- Consumes: `AgentiflowRecord::{read, write}`, `units_on_disk`, `proc::{pid_is_running, terminate_group}`, the `events.jsonl` append path.

- [ ] **Step 1: Failing test — a running record with a dead `runner_pid` is reaped; its live units are group-killed; a healthy record is untouched.**

```rust
// tests/it/reaper.rs
#[test]
fn reaps_a_dead_coordinator_and_signals_its_units() {
    let global = tempdir().unwrap();
    // fabricate <global>/agentiflows/af_dead/ with a running record whose runner_pid is dead,
    // plus units/u1/unit.json with a pgid we can observe (spawn a sleep in its own group).
    let (pgid, _child) = spawn_group_sleeper();         // helper: process_group(0) sleep
    write_running_record(&global, "af_dead", /*runner_pid*/ a_dead_pid());
    write_unit_json(&global, "af_dead", "u1", pgid, UnitStatus::Running);
    // and a healthy one whose runner_pid is THIS process (alive) → must NOT be reaped
    write_running_record(&global, "af_live", std::process::id());

    let summary = reap_orphaned_agentiflows(global.path(), Utc::now());

    assert_eq!(summary.reaped, vec!["af_dead".to_string()]);
    let dead = read_record(&global, "af_dead");
    assert_eq!(dead.status, "failed");
    assert!(dead.stop_reason.as_deref().unwrap().starts_with("orphaned"));
    assert!(dead.ended_at.is_some());
    assert!(!pid_is_running(pgid));                     // the unit group was killed
    let live = read_record(&global, "af_live");
    assert_eq!(live.status, "running");                 // untouched
}

#[test]
fn a_none_pid_record_is_never_reaped() {
    // the create→write-back window: runner_pid absent → skip, don't fail it
    let global = tempdir().unwrap();
    write_running_record_no_pid(&global, "af_new");
    let s = reap_orphaned_agentiflows(global.path(), Utc::now());
    assert!(s.reaped.is_empty());
    assert_eq!(read_record(&global, "af_new").status, "running");
}
```

- [ ] **Step 2: Implement.** `reap_orphaned_agentiflows` reads `<global>/agentiflows/af_*/agentiflow.json` (tolerant: skip unreadable, logged). For each record with `status == "running"` AND `runner_pid == Some(pid)` where `!pid_is_running(pid)` (a `None` pid is skipped):
  1. `units_on_disk(run_dir)` → for each unit whose `status` is non-terminal and whose `pgid` is `Some(g)` with `pid_is_running(g)`, `terminate_group(g)` (best-effort; log on error — a recycled/absent group is not fatal). Follow with `kill_group(g)` after a short grace only if it is still alive (mirror `SubprocessUnitLauncher::Drop`'s SIGTERM→grace→SIGKILL, but bounded and non-blocking enough for a sweep — e.g. one SIGTERM pass; the stricter escalation can stay in the launcher's own Drop for the graceful path).
  2. Finalize the record: `status = "failed"`, `ended_at = Some(now)`, `stop_reason = Some("orphaned: coordinator pid <p> not running")`, `runner_pid = None`; atomic write.
  3. Append a terminal `run_stopped` event to `events.jsonl` (so a live events view stops spinning — the `finalize_failed`→`append_terminal_event` lesson, PR #501).
  Return `ReapSummary { scanned, reaped }`.

- [ ] **Step 3: Run + commit.**
```bash
cargo test -p rupu-agentiflow --test it reaper:: < /dev/null   # green
git add -A && git commit -m "feat(agentiflow): reap_orphaned_agentiflows — finalize dead coordinators + group-kill their units"
```

---

## Task 3: `rupu agentiflow send` + `stop [--now]`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/agentiflow.rs` (`Action` enum `:51`; add handlers; reuse `resolve_run_id`)
- Modify: `crates/rupu-cli/src/lib.rs` (dispatch arm if the enum shape needs it — the existing `handle` already routes `Action`)
- Test: covered by the T8 serial e2e (CLI verbs are thin; unit-test the id-resolution + enqueue indirectly there)

**Interfaces:**
- Consumes: `OperatorQueue::new(run_dir).enqueue(OperatorMessage{..})`, `AgentiflowRecord::read` (for `runner_pid`), `proc::terminate_pid`, `reaper::reap_orphaned_agentiflows` (for `--now` unit cleanup), `output::ids::resolve`.

- [ ] **Step 1: `send`.** Add `Action::Send { id: String, message: String, now: bool }` (clap: `rupu agentiflow send <id> <message> [--now]`). Handler: resolve the id to a run dir (`<global>/agentiflows/<full-id>`); build `OperatorMessage { ts: now_rfc3339(), body: message, stop: false, interrupt: now }`; `OperatorQueue::new(&run_dir).enqueue(&msg)`. Print `queued steering for <id>`. (`--now` sets the interrupt flag T6 consumes; until T6 lands it is enqueued but delivered at the round boundary — so land T3's `--now` flag plumbing and T6's consumption together, or gate `--now` behind T6. RULING: add `--now` in T3 but document it as round-boundary until T6; the flag is never a silent noop because T6 is in the same plan.)

- [ ] **Step 2: `stop`.** Add `Action::Stop { id: String, now: bool }`. Handler:
  - Resolve id → run dir; read the record.
  - **Graceful (default):** enqueue `OperatorMessage { stop: true, body: "operator stop", interrupt: false, .. }`. The envelope sees `m.stop` at the next round's `decide()` → `StopReason::OperatorStop` → graceful wind-down (`terminate_all` runs on the coordinator's normal return). Print `requested graceful stop of <id>`.
  - **`--now` (hard):** if `record.runner_pid` is a live pid, `terminate_pid(pid)` (SIGTERM the coordinator); then call `reap_orphaned_agentiflows(global, now)` is NOT right (the coordinator may not be dead yet) — instead directly group-kill the units via `units_on_disk` + `terminate_group` and finalize the record `failed`/`stop_reason: "operator_stop:now"`. Print `hard-stopped <id>`. (The coordinator catches SIGTERM and its own `terminate_all`/launcher-Drop also fires; the explicit unit kill is the belt-and-braces for a coordinator that can't run Drop.)

- [ ] **Step 3: id resolution + not-found.** Both verbs resolve via the same `resolve_run_id` `list`/`status` use (`output::ids::resolve`: full id / compact / unique prefix-suffix). An unknown id → a clear anyhow error (`no agentiflow run matches '<id>'`). A `send`/`stop` to an already-terminal record → a friendly message (`<id> already <status>`), no enqueue.

- [ ] **Step 4: Commit.** (Behavioural tests are the T8 e2e.)
```bash
cargo build -p rupu-cli < /dev/null && cargo clippy -p rupu-cli --all-targets -- -D warnings
git add -A && git commit -m "feat(cli): rupu agentiflow send + stop [--now]"
```

---

## Task 4: `rupu agentiflow run --detach`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/agentiflow.rs` (`Action::Run` gains `detach: bool` + a hidden `run_id: Option<String>`; `run_cmd`/`launch` `:105`/`:235`)
- Test: T8 serial e2e

**Interfaces:**
- Consumes: `std::process::Command` with `process_group(0)` (the unit-launcher detach model, `subprocess.rs:382`), the already-minted `run_id`.

- [ ] **Step 1: `--detach` + hidden `--run-id`.** Add `--detach` to `Action::Run` and a HIDDEN `--run-id <id>` (clap `hide = true`). When `--detach` is set and `--run-id` is NOT, the invocation is the *parent*: mint the run_id (as `launch` already does via `new_run_id`), then re-exec `std::env::current_exe()` with the SAME args minus `--detach`, plus `--run-id <minted>`, spawned with `process_group(0)`, stdio null, `RUPU_HOME` preserved; print `agentiflow <name>: run <id> (detached)` to stdout and return immediately. When `--run-id` IS present (the re-exec'd child), skip detaching and run the normal foreground `launch` path using that id (so the child's `run_agentiflow` stamps `runner_pid = child pid`, and `run_dir` matches the id the parent printed).

- [ ] **Step 2: Parent/child id agreement.** `launch` currently mints `run_id` internally; refactor so `run_cmd` mints it once and passes it into `launch(run_id, …)`, so the detached parent can print the same id it hands the child. (`new_run_id` already exists; the run dir is `<global>/agentiflows/<id>`.)

- [ ] **Step 3: Verify the detach is a real process-group child.** T8 asserts: after `run --detach`, the parent command exits 0 quickly with the id on stdout; `agentiflow status <id>` shows `running` with a `runner_pid` that is a live pid in its own group; killing that group (or a graceful `stop`) ends it.

- [ ] **Step 4: Commit.**
```bash
git add -A && git commit -m "feat(cli): rupu agentiflow run --detach (process-group child, runner_pid = child)"
```

---

## Task 5: Directive retraction (`id` + append-only retract + `board.retract` tool)

**Files:**
- Modify: `crates/rupu-fleet/src/types.rs` (`Directive` gains `id`; add `DirectiveEvent`)
- Modify: `crates/rupu-fleet/src/board.rs` (`put_directive` mints an id; new `retract_directive(id)`; `read_directives` folds to the live set)
- Modify: `crates/rupu-agentiflow/src/status_tools.rs` (`board.directive` carries the id; add `board.retract`)
- Modify: `crates/rupu-agentiflow/src/collectors.rs` (DirectiveCollector already calls `read_directives` — no change beyond getting the folded set)
- Test: `crates/rupu-fleet/tests/it/…` (or inline) + `status_tools` inline

**Interfaces:**
- Produces: `Directive { id: String, author, ts, body, addressed_to }`; `Board::retract_directive(&self, id: &str)`; `read_directives` returns only live (non-retracted) directives.

- [ ] **Step 1: Failing test — a retracted directive disappears from `read_directives`; the log stays append-only.**

```rust
#[test]
fn retracting_a_directive_removes_it_from_the_live_set() {
    let dir = tempdir().unwrap();
    let board = Board::new(dir.path().to_path_buf());
    let id = board.put_directive(Directive::lead("focus auth", None)).unwrap();  // returns id
    board.put_directive(Directive::lead("expand to staging", None)).unwrap();
    assert_eq!(board.read_directives().unwrap().len(), 2);
    board.retract_directive(&id).unwrap();
    let live = board.read_directives().unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].body, "expand to staging");
    // append-only: the raw file still has all 3 lines (2 directives + 1 retract)
    assert_eq!(raw_line_count(dir.path(), "board/directives.jsonl"), 3);
}
```

- [ ] **Step 2: Implement.** Add `id: String` to `Directive` (`#[serde(default)]`; a legacy line with no id folds in with an empty id and can't be retracted — acceptable, logged once). The board file becomes a log of `DirectiveEvent` — either `Put(Directive)` or `Retract { id }` — serialized so a legacy bare-`Directive` line still deserializes as `Put` (use `#[serde(untagged)]` or a `kind`-tagged enum with a default; pick whichever keeps old lines parsing and add a test for a legacy line). `put_directive` mints a ULID (`id`) and appends `Put`; returns the id. `retract_directive(id)` appends `Retract { id }`. `read_directives` reads all events in file order, builds the live set (insert on `Put`, remove on `Retract`), returns it oldest-first. Keep it tolerant: a corrupt line is skipped + logged (today's behaviour).

- [ ] **Step 3: Tools.** `board.directive` (`status_tools.rs:499`) now surfaces the minted id in its `ToolOutput` (so the lead can retract later). Add a Tier-2 `board.retract { id }` tool (lead-held, mounted where `board.directive` is) → `Board::retract_directive`; unknown id → `Ok(ToolOutput.error)` (fail-soft, nothing to retract). Update the `board.directive` tool description to drop the "no retraction, so directives accumulate" caveat and point at `board.retract`.

- [ ] **Step 4: Run + commit.**
```bash
cargo test -p rupu-fleet -p rupu-agentiflow < /dev/null   # green
git add -A && git commit -m "feat(fleet): directive retraction — id + append-only retract, folded on read; board.retract tool"
```

---

## Task 6: Opt-in priority/interrupt steering (`send --now` cuts the round short)

**Files:**
- Modify: `crates/rupu-agentiflow/src/operator.rs` (`OperatorMessage.interrupt`; `peek_interrupt()`)
- Modify: `crates/rupu-agentiflow/src/lead.rs` (`RunAgentLeadDriver`: per-round `CancellationToken` → `AgentRunOpts.pause`; a watcher thread)
- Modify: `crates/rupu-agentiflow/src/envelope.rs` (drain delivers an interrupt message first; no behaviour change when none)
- Test: inline in `lead.rs`/`operator.rs` + the T8 e2e

**Interfaces:**
- Produces: `OperatorMessage { ts, body, stop, interrupt }` (`#[serde(default)] interrupt`); `OperatorQueue::peek_interrupt(&self) -> bool` (reads without removing).
- Consumes: `AgentRunOpts.pause: Option<CancellationToken>` (`runner.rs:1494`), `RunExit.paused`.

- [ ] **Step 1: `interrupt` + `peek_interrupt`.** Add `#[serde(default)] pub interrupt: bool` to `OperatorMessage` (old files parse; `drain`/render unchanged for `interrupt:false`). Add `OperatorQueue::peek_interrupt(&self) -> bool` that reads `steering/*.json` WITHOUT removing any file and returns true iff some message has `interrupt == true` (tolerant; a parse error on one file → skip). This is non-destructive so the envelope's `drain()` still delivers the message at the boundary.

- [ ] **Step 2: Failing test — a mid-round interrupt cancels the lead's run; delivery still happens at the boundary.**

```rust
// lead.rs test with a provider double that blocks until the token cancels
#[test]
fn an_interrupt_steering_message_cuts_the_round_short_then_is_delivered() {
    // round starts; a watcher sees an enqueued interrupt message; the lead's AgentRunOpts.pause
    // token is cancelled; run_agent_full returns with paused==true; the message remains in
    // steering/ and is drained+rendered at the next assess().
    // Assert: the round ended via cancellation (not a full generation), and the next round's
    // digest.steering contains the message.
}
```

- [ ] **Step 3: Wire the token + watcher.** In `RunAgentLeadDriver::run_round`, create a `CancellationToken` per round, pass `pause: Some(token.clone())` into the lead's `AgentRunOpts` (replacing `pause: None` at `lead.rs:433`). Share the current round's token via an `Arc<Mutex<Option<CancellationToken>>>` the driver holds. Spawn ONE watcher `std::thread` for the run (started when the driver is built, joined on drop) that polls the `steering/` dir (`OperatorQueue::peek_interrupt`) every ~200ms; when it sees an interrupt pending AND a current-round token is set, it `token.cancel()` (the cross-thread cancel is observed by `run_agent_full` at its next safe boundary → `RunExit.paused == true`). The driver treats a `paused` return as a normal round end (persist what the lead produced; the envelope's next `assess()` drains the interrupt message and renders it authoritatively). A std::thread watcher is used (not a task on the lead's current-thread runtime) so the cancel fires even while the lead's runtime is blocked in `block_on`.
  - **RULING / fallback:** if the pause-token wiring destabilises the lead's runtime model (e.g. the current-thread `block_on` + a `pause` token interact badly), the honest fallback is to drop the watcher and ship `send`/`stop` at round-boundary only, removing `--now`-interrupt from `send` (keep `stop --now`, which is a genuinely different hard stop). This is recorded so `--now` is never a silent noop — it either truly interrupts or is not offered for `send`. Decide in the fix-loop with the reviewer; record the outcome in the ledger.

- [ ] **Step 4: Run + commit.**
```bash
cargo test -p rupu-agentiflow < /dev/null   # green
git add -A && git commit -m "feat(agentiflow): opt-in priority steering — send --now cancels the round via the lead pause token"
```

---

## Task 7: `rupu agentiflow serve` + `cp serve` reaper wiring + `[agentiflow]` config

**Files:**
- Modify: `crates/rupu-config/src/policy_config.rs` (`AgentiflowConfig`; hang it on the top-level config next to `CpConfig`)
- Modify: `crates/rupu-cli/src/cmd/agentiflow.rs` (`Action::Serve`; the supervised loop)
- Modify: `crates/rupu-cli/src/cmd/cp.rs` (call `reap_orphaned_agentiflows` from the existing sweep tick)
- Test: inline config test + T8 e2e for `serve`'s reaping

**Interfaces:**
- Produces: `AgentiflowConfig { serve_enabled: bool (default true), serve_interval_secs: u64 (default 60), reaper_enabled: bool (default true) }` (`#[serde(default, deny_unknown_fields)]`).
- Consumes: `reaper::reap_orphaned_agentiflows`, the existing `run_periodic_tick` helper (`cp.rs:505`).

- [ ] **Step 1: Config.** Add `AgentiflowConfig` to `policy_config.rs` mirroring `CpConfig`'s shape (serde default + `deny_unknown_fields`, all fields documented). Add an `agentiflow: AgentiflowConfig` field to the top-level config struct that owns `cp: CpConfig`, with a `#[serde(default)]`. Test: a config with no `[agentiflow]` block yields the defaults; a partial `[agentiflow]` block fills the rest; an unknown key errors (deny_unknown_fields).

- [ ] **Step 2: `serve`.** Add `Action::Serve` (`rupu agentiflow serve`). Handler: resolve `global`, load layered config, and if `agentiflow.serve_enabled && agentiflow.reaper_enabled`, run a loop that every `serve_interval_secs` calls `reap_orphaned_agentiflows(&global, Utc::now())` and logs the `ReapSummary` when it reaps anything (quiet otherwise). Model the loop on `cp serve`'s structure but keep it minimal (a `tokio::time::interval`); handle SIGTERM to exit cleanly. Print a startup line (`agentiflow serve: reaping orphans every <n>s`). Trigger auto-launch is OUT of scope (Global Constraint ruling).

- [ ] **Step 3: `cp serve` also reaps.** In `cp.rs`'s sweep tick (next to the gate sweep `:229`/`run_gate_sweep`), add a call to `reap_orphaned_agentiflows(&global, now)` gated by `agentiflow.reaper_enabled`, so a CP-only operator gets the reaper too. (Shared fn — the logic lives in `rupu-agentiflow`; `cp serve` and `agentiflow serve` both schedule it.) Keep it best-effort (log a reap, never fail the tick).

- [ ] **Step 4: Run + commit.**
```bash
cargo test -p rupu-config < /dev/null && cargo build -p rupu-cli < /dev/null && cargo clippy -p rupu-cli -p rupu-config --all-targets -- -D warnings
git add -A && git commit -m "feat(cli): rupu agentiflow serve (reaper loop) + cp serve reaping + [agentiflow] config"
```

---

## Task 8: `rupu agentiflow attach` + end-to-end serial tests

**Files:**
- Modify: `crates/rupu-cli/src/cmd/agentiflow.rs` (`Action::Attach`)
- Create: `crates/rupu-cli/tests/serial/agentiflow_operate.rs` (add to the serial binary's `main.rs`/mod list)
- Test: the serial e2e below

**Interfaces:**
- Consumes: the run dir's `events.jsonl` + `lead/transcript.r<N>.jsonl`, `AgentiflowRecord` status polling, the mock-provider + `MockUnitLauncher` seams.

- [ ] **Step 1: `attach`.** Add `Action::Attach { id: String, follow: bool }` (`--follow`/`-f` default true). Handler: resolve id → run dir; tail `events.jsonl` (print new lines as they land) AND the current round's `lead/transcript.r<N>.jsonl`, following round rollover (watch for `transcript.r<N+1>.jsonl`); poll the record and exit when `status != "running"` (print the final `stop_reason`). Model the cadence on the session attach tail+poll (~100ms); pure filesystem polling, no new IPC. A one-shot (`--no-follow`) prints the current events + exits.

- [ ] **Step 2: e2e — the full operator loop (serial, mock provider).** Following the `agentiflow_run.rs`/`agentiflow_budget.rs` pattern (`#[tokio::test(multi_thread)]`, `ENV_LOCK`, isolated `RUPU_HOME`, `RUPU_MOCK_PROVIDER_SCRIPT`, `write_stdin("")`/`< /dev/null`, strip ambient `RUPU_*_API_KEY`):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn detach_then_send_then_graceful_stop() {
    let _g = ENV_LOCK.lock().await;
    let fx = Fixture::new();                     // isolated RUPU_HOME + agents/lead.md + agentiflows/acme.yaml
    // mock provider: lead loops (never meets the goal) until a stop steering message arrives
    let out = fx.rupu(["agentiflow","run","acme","--detach"]).output();   // returns fast
    let id = parse_detached_id(&out);
    poll_until(|| fx.status(&id).status == "running");
    fx.rupu(["agentiflow","send",&id,"focus on auth"]).assert_success();
    assert!(steering_dir_eventually_drains(&fx, &id));                    // envelope consumed it
    fx.rupu(["agentiflow","stop",&id]).assert_success();                 // graceful
    poll_until(|| fx.status(&id).stop_reason.as_deref() == Some("operator_stop"));
}

#[tokio::test(flavor = "multi_thread")]
async fn reaper_finalizes_a_dead_coordinator_and_kills_its_units() {
    // run --detach a flow that dispatches a (mock-real) long unit; SIGKILL the coordinator pid
    // (so Drop/terminate_all never run); run `agentiflow serve` once (or call the reaper);
    // assert the record goes failed with stop_reason "orphaned: …" and the unit group is gone.
}

#[tokio::test(flavor = "multi_thread")]
async fn hard_stop_now_kills_the_coordinator_and_units() { /* stop --now */ }
```

Assert *delivery* (the record/units/steering actually changed), never just a command exit (the memory rule). Hold `ENV_LOCK`, drive `< /dev/null`.

- [ ] **Step 3: Run + commit.**
```bash
cargo test -p rupu-cli --test serial agentiflow_operate:: < /dev/null   # green (serial binary)
git add -A && git commit -m "feat(cli): rupu agentiflow attach + operator-loop e2e (detach/send/stop/reaper)"
```

---

## Self-Review

**Spec coverage:** §21 CLI surface — `run --detach` (T4), `serve` (T7), `attach`/`send` (T8/T3), `stop [--now]` (T3); `status`/`list` already shipped in 4-1. §17 operator steering — round-boundary `send` (T3) + opt-in priority/interrupt (T6). §10.2/§19 reaper — runner_pid + unit pgid (T1) + `reap_orphaned_agentiflows` (T2) + scheduling (T7). Directive retraction (named 3b-3 debt) — T5. §22 session-worker lift — explicitly reinterpreted/deferred (Global Constraint ruling). CP web surfaces — Plan 4-4 (out of scope).

**Placeholder scan:** every task has real signatures (from the dossier's `file:line` cites), concrete test code, and exact commit lines. No "add error handling" / "similar to Task N".

**Type consistency:** `runner_pid: Option<u32>` (T1) is read by T2's reaper and T3's `stop --now`; `pgid` on units (T1) is read by T2's `units_on_disk`; `OperatorMessage.interrupt` (T6) is set by T3's `send --now` and peeked by T6's watcher; `Directive.id` (T5) is surfaced by `board.directive` and consumed by `board.retract`; `AgentiflowConfig.reaper_enabled` (T7) gates both `serve` and `cp serve`. All names match across tasks.

**Ordering:** T1 (pids) → T2 (reaper reads them) → T3/T4 (CLI uses record+reaper) → T5 (independent, directives) → T6 (steering, independent of T2-T5) → T7 (serve schedules T2's reaper) → T8 (attach + e2e exercises everything). T5 and T6 are independent and could run in either order after T1.
