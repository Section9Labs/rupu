# Multi-host Slice 3a — distributed fan-out units Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run a `for_each` step's units across the fleet (each unit on an assigned host), aggregating outputs back into `steps.<id>.results[]` exactly as a local fan-out — the run stays one coherent run.

**Architecture:** A `distribute: { hosts: [...] }` block on a `for_each` step. The orchestrator's `run_fanout_step` keeps running local units inline (unchanged) and routes **host-placed** units through a remote-only `UnitDispatcher` port. A `rupu-cli` `FleetUnitDispatcher` dispatches each placed unit via `HostConnector::launch_agent`, then reads the unit's output from the mirrored run's new `final_output` field — agent runs are made run-dir-backed (write `run.json`+`final_output`) so this rides the existing mirror uniformly across all transports.

**Tech Stack:** Rust 2021, tokio, serde, async-trait, thiserror (libs) / anyhow (CLI). No new dependencies.

## Global Constraints

- **Backward compatible:** a workflow with no `distribute:` runs byte-for-byte as today; existing fan-out tests must stay green.
- **Hexagonal:** `rupu-orchestrator` knows ONLY the `UnitDispatcher` trait — never `rupu-cp`/`HostConnector`. The fleet impl lives in `rupu-cli`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden (no `unsafe`).
- Library errors `thiserror`; CLI binary `anyhow`.
- Per-file `rustfmt` only — `main` is fmt-dirty; NEVER workspace-wide `cargo fmt`.
- `rupu-cli` has a PRE-EXISTING red toolchain baseline; verify only NEW code compiles + its tests pass.
- Workspace deps only (no new deps).
- **Self-contained-unit guardrail:** a distributed unit must be computable from its `item` + prior-step **string** context — no dependence on the shared file workspace being present on the remote host (repo-file units are Slice 3c). Document; validator warns where feasible.
- Round-robin placement only; one reassignment on host failure, then honor the step's existing `continue_on_error`.

## File Structure

- `crates/rupu-orchestrator/src/workflow.rs` — `Distribute` struct + `Step.distribute` + validation. (T1)
- `crates/rupu-orchestrator/src/runs.rs` — `RunRecord.final_output`. (T2)
- `crates/rupu-orchestrator/src/runner.rs` — expose `read_final_assistant_text`; `UnitDispatcher` trait + `UnitOutcome`/`UnitDispatch`; `OrchestratorRunOpts.unit_dispatcher`; `run_fanout_step` placement/retry. (T2 expose, T3)
- `crates/rupu-orchestrator/src/executor/event.rs` + `runs.rs` (`UnitCheckpoint`) — `host` tag on unit events/checkpoint. (T4)
- `crates/rupu-cli/src/cmd/run.rs` — agent run writes `run.json`+`final_output`. (T2)
- `crates/rupu-cli/src/cmd/workflow.rs` — build `HostRegistry`, inject `FleetUnitDispatcher`. (T5)
- `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (new) — `FleetUnitDispatcher`. (T5)
- Tests: orchestrator unit tests (T1, T3), runs/mirror test (T2), rupu-cli unit tests (T5), e2e (T6).

---

## Task 1: `Distribute` placement on `for_each` steps

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (`Step` struct + a `Distribute` struct + the step validator)

**Interfaces — Produces:** `Distribute { hosts: Vec<String> }`; `Step.distribute: Option<Distribute>`.

- [ ] **Step 1: Write failing tests** in `workflow.rs` `#[cfg(test)]` (match the existing Step/serde test style):

```rust
#[test]
fn distribute_parses_on_for_each() {
    let yaml = r#"
id: scan
for_each: "{{ steps.list.results }}"
agent: scanner
distribute:
  hosts: [edge-1, edge-2]
"#;
    let step: Step = serde_yaml::from_str(yaml).unwrap();
    let d = step.distribute.expect("distribute present");
    assert_eq!(d.hosts, vec!["edge-1".to_string(), "edge-2".to_string()]);
}

#[test]
fn distribute_omitted_is_none() {
    let step: Step = serde_yaml::from_str("id: s\nfor_each: \"x\"\nagent: a\n").unwrap();
    assert!(step.distribute.is_none());
}

#[test]
fn validate_rejects_distribute_without_for_each() {
    // a linear step (agent+prompt, no for_each) with distribute → validation error
    let step: Step = serde_yaml::from_str(
        "id: s\nagent: a\nprompt: hi\ndistribute:\n  hosts: [h1]\n").unwrap();
    let err = validate_step_shape(&step).unwrap_err(); // use the real validator fn name
    assert!(err.to_string().contains("distribute"));
}

#[test]
fn validate_rejects_empty_hosts() {
    let step: Step = serde_yaml::from_str(
        "id: s\nfor_each: \"x\"\nagent: a\ndistribute:\n  hosts: []\n").unwrap();
    assert!(validate_step_shape(&step).is_err());
}
```

Read the REAL validator fn (grep `fn validate_step_shape` / where step shape is validated in `workflow.rs`) and call it with the real name; adapt the error-type assertion to the real error type.

- [ ] **Step 2: Run** `cargo test -p rupu-orchestrator distribute` → FAIL (no field / no validation).

- [ ] **Step 3: Add the struct + field.** In `workflow.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Distribute {
    /// Fleet host ids/names to spread this step's units across (round-robin).
    pub hosts: Vec<String>,
}
```

Add to `Step` (after the fan-out fields, with the existing serde-skip style):
```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribute: Option<Distribute>,
```

- [ ] **Step 4: Add validation.** In the step validator (`validate_step_shape` or equivalent), after the shape checks, add: if `step.distribute.is_some()` then `step.for_each.must be Some` (else error "distribute is only valid on a for_each step") and `!hosts.is_empty()` (else error "distribute.hosts must be non-empty"). Use the file's existing error constructor.

- [ ] **Step 5: Run** `cargo test -p rupu-orchestrator distribute` → PASS; `cargo clippy -p rupu-orchestrator` → clean.

- [ ] **Step 6: Commit**
```bash
git add crates/rupu-orchestrator/src/workflow.rs
git commit -m "feat(orchestrator): for_each distribute: { hosts } placement field"
```

---

## Task 2: Agent runs become run-dir-backed (`run.json` + `final_output`)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs` (`RunRecord.final_output`)
- Modify: `crates/rupu-orchestrator/src/runner.rs` (make `read_final_assistant_text` `pub`)
- Modify: `crates/rupu-cli/src/cmd/run.rs` (write `run.json`+`final_output` on completion)
- Test: `crates/rupu-orchestrator` (final_output on RunRecord round-trips, incl. through the mirror)

**Interfaces — Produces:** `RunRecord.final_output: Option<String>`; `pub fn read_final_assistant_text(transcript_path: &Path, success: bool, run_id: &str, step_id: &str) -> String`.

- [ ] **Step 1: Failing test** — `final_output` round-trips on a `RunRecord` AND survives `NodeMirror`'s `RunJson` re-pin (it must NOT be nulled like `transcript_dir` is). In `crates/rupu-cp/tests/node_tunnel.rs` (where the mirror tests live), add:

```rust
#[test]
fn mirror_run_json_preserves_final_output() {
    use rupu_orchestrator::RunStore;
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    let mirror = crate::node::NodeMirror::new(std::sync::Arc::new(store));
    // create_run as node "n1", then append a RunJson whose JSON sets final_output
    // and a completed status; load → assert final_output preserved.
    // (mirror RunJson re-pins id/worker_id/workspace/transcript + nulls resume_*,
    //  but keeps run-state fields incl final_output.)
    // Build the run.json body from a RunRecord with final_output Some("hello out").
    // ... (follow the existing mirror RunJson test pattern in this file) ...
}
```

Write it concretely following the existing `mirror_run_json_*` test(s) in the file (there is already a RunJson re-pin test — model on it; set `final_output: Some("hello out".into())` on the incoming record, append as `ArtifactFile::RunJson`, load, assert `rec.final_output.as_deref() == Some("hello out")`).

- [ ] **Step 2: Run** → FAIL (no field).

- [ ] **Step 3: Add the field.** In `runs.rs` `RunRecord` add (near `error_message`):
```rust
    /// Final assistant text for an agent run (set by `rupu run`); None for
    /// workflow runs / older records. Carried by the mirror so a remotely
    /// dispatched unit's output is retrievable centrally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_output: Option<String>,
```
Update EVERY `RunRecord { .. }` struct literal in the codebase to set `final_output: None` (grep `RunRecord {` — there are several construction sites in runs.rs/mirror.rs/tests; the compiler will list them). Confirm `NodeMirror::append`'s `RunJson` re-pin block does NOT overwrite `final_output` (it re-pins id/worker_id/workspace_*/transcript_dir + nulls resume_*; `final_output` is taken from the incoming record — exactly what we want; if the mirror reconstructs the record field-by-field, ensure final_output is carried).

- [ ] **Step 4: Expose `read_final_assistant_text`.** In `runner.rs`, change `fn read_final_assistant_text` to `pub fn read_final_assistant_text` and re-export it (`pub use runner::read_final_assistant_text;` in the orchestrator `lib.rs` if the crate re-exports runner items — match the existing re-export style). It is pure (reads a transcript JSONL, returns the last `AssistantMessage.content`).

- [ ] **Step 5: `rupu run` writes the run.json.** In `crates/rupu-cli/src/cmd/run.rs`, after the agent run completes and the transcript is finalized (near the `println!("transcript: ...")` at the end), write a minimal run record:
```rust
// Make the agent run run-dir-backed so its output is retrievable centrally
// (the mirror carries run.json on every transport). run_<ULID> id matches the
// --run-id used for dispatch.
let runs_root = global.join("runs");
let store = rupu_orchestrator::RunStore::new(runs_root);
let final_output = rupu_orchestrator::read_final_assistant_text(
    &transcript_path, /*success*/ run_ok, &run_id, /*step_id*/ "agent");
let now = chrono::Utc::now();
let rec = rupu_orchestrator::runs::RunRecord {
    id: run_id.clone(),
    workflow_name: format!("agent:{}", spec.agent_name /*the agent name field*/),
    status: if run_ok { RunStatus::Completed } else { RunStatus::Failed },
    started_at: started_at, finished_at: Some(now),
    final_output: Some(final_output),
    transcript_dir: transcripts.clone(),
    // ...all other RunRecord fields with sensible defaults (inputs empty,
    //    workspace_id/workspace_path from the run, worker_id None, etc.)...
    ..Default::default() // ONLY if RunRecord derives Default; otherwise fill every field
};
let _ = store.create(rec, ""); // create the run dir + run.json; ignore if exists
```
IMPORTANT: confirm whether `RunStore::create` is the right call (it may require the record + workflow_yaml; pass `""`), or whether a dedicated "upsert run.json" method exists — read `runs.rs` RunStore API and use the real one (create vs update). If the `--run-id` may already exist (dispatched runs), prefer create-then-update or an idempotent write; match what the codebase offers. Capture `run_ok`, `started_at`, the agent name, and `workspace_id/path` from the values already in scope in `run.rs` (read the file to get the exact variable names). Do NOT change the transcript behavior.

- [ ] **Step 6: Run** `cargo test -p rupu-cp --test node_tunnel mirror_run_json_preserves_final_output` → PASS; `cargo build -p rupu-cli` → compiles; `cargo clippy -p rupu-orchestrator -p rupu-cp` → clean.

- [ ] **Step 7: Commit**
```bash
git add crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/lib.rs crates/rupu-cli/src/cmd/run.rs crates/rupu-cp/tests/node_tunnel.rs
git commit -m "feat: agent runs write run.json + final_output (mirror-carried)"
```

---

## Task 3: `UnitDispatcher` port + `run_fanout_step` placement

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (trait + types + `OrchestratorRunOpts` field + `run_fanout_step` placement/retry)
- Test: `crates/rupu-orchestrator/src/runner.rs` `#[cfg(test)]` (fake dispatcher)

**Interfaces — Produces:**
```rust
pub struct UnitDispatch {
    pub step_id: String, pub agent: String, pub rendered_prompt: String,
    pub index: usize, pub run_id: String,
}
pub struct UnitOutcome { pub output: String, pub success: bool, pub error: Option<String> }

#[async_trait::async_trait]
pub trait UnitDispatcher: Send + Sync {
    /// Run one unit (an agent invocation) on `host` and return its output.
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError>;
}
```
Plus `OrchestratorRunOpts.unit_dispatcher: Option<Arc<dyn UnitDispatcher>>`.

**Design note (simplification vs spec):** the port is **remote-only**. Local units keep the existing inline `dispatch_one` + `read_final_assistant_text` path unchanged (byte-for-byte). `run_fanout_step` only routes a unit through the dispatcher when it has a host placement.

- [ ] **Step 1: Write the failing fake-dispatcher test** in `runner.rs` tests:

```rust
struct FakeDispatcher { calls: std::sync::Mutex<Vec<(usize, String)>>, fail_first_host: Option<String> }
#[async_trait::async_trait]
impl UnitDispatcher for FakeDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        self.calls.lock().unwrap().push((unit.index, host.to_string()));
        if Some(host.to_string()) == self.fail_first_host {
            return Err(RunError::/* the generic variant */ ("host down".into()));
        }
        Ok(UnitOutcome { output: format!("out-{}-on-{host}", unit.index), success: true, error: None })
    }
}

#[tokio::test]
async fn distributed_fanout_round_robins_and_aggregates() {
    // Build a for_each step with distribute.hosts = [h1, h2], 4 items, max_parallel 4,
    // inject FakeDispatcher (no fail). Run run_fanout_step (via the minimal harness
    // the existing fanout tests use). Assert: each unit dispatched to host h1/h2/h1/h2
    // by index (round-robin), and the aggregated results[] are out-0-on-h1, out-1-on-h2,
    // ... in index order.
}

#[tokio::test]
async fn distributed_unit_reassigns_once_on_host_failure() {
    // distribute.hosts=[h1,h2], fail_first_host=Some("h1"); a unit assigned h1 fails,
    // is retried on h2 (next host), succeeds. Assert the calls show the h1 then h2 retry
    // and the unit's result is the h2 output.
}
```
Use the EXACT harness the existing `run_fanout_step` tests use (read them — they build a minimal `OrchestratorRunOpts`/step + a stub `StepFactory`; for distributed tests `unit_dispatcher: Some(Arc::new(FakeDispatcher{..}))`). Use the real `RunError` variant for the failure. Keep the harness minimal (no run_store needed).

- [ ] **Step 2: Run** → FAIL (trait/field not defined).

- [ ] **Step 3: Add the trait + types + opts field.** Add `UnitDispatch`/`UnitOutcome`/`UnitDispatcher` to `runner.rs` (or a new `unit_dispatch.rs` module re-exported from `lib.rs`). Add to `OrchestratorRunOpts`:
```rust
    /// Optional remote unit dispatcher. When a `for_each` step has
    /// `distribute:`, units are routed to hosts through this. `None` ⇒ all units
    /// run locally (a `distribute:` step with `None` is a run error).
    pub unit_dispatcher: Option<std::sync::Arc<dyn UnitDispatcher>>,
```
Update all `OrchestratorRunOpts { .. }` construction sites to set `unit_dispatcher: None` (grep — cmd/workflow.rs has ~2 sites; tests have several).

- [ ] **Step 4: Route placed units in `run_fanout_step`.** In the unit loop (around runner.rs:1116-1290), before spawning each unit task, compute placement:
```rust
let placement: Option<String> = step.distribute.as_ref().map(|d| {
    d.hosts[idx % d.hosts.len()].clone()
});
```
In the spawned task, branch:
- `placement == None` → the EXISTING inline path (`dispatch_one` + `read_final_assistant_text`) — unchanged.
- `placement == Some(host)` → require `opts.unit_dispatcher` (else return a `RunError` "distribute requires fleet access — run via the CP"); call `dispatch_unit(UnitDispatch{..}, &host)`. On `Err`, **reassign once** to `d.hosts[(idx+1) % len]` and retry; on a second `Err`, treat as a failed unit (success=false, output=error string). The `UnitOutcome.output`/`success` populate the unit's `ItemResult` (same fields the local path fills). Tag the unit's events/checkpoint with the host (Task 4 adds the field; here pass `Some(host)`).
Keep the semaphore (`max_parallel`), ordering (sort by idx), `unit_checkpoints` write, and `continue_on_error` abort check exactly as today. The only change is HOW a unit's `(output, success)` is produced for placed units.

- [ ] **Step 5: Run** `cargo test -p rupu-orchestrator distributed_fanout` (the 2 new tests) AND `cargo test -p rupu-orchestrator` (ALL existing fan-out tests still pass — backward-compat proof) → PASS; `cargo clippy -p rupu-orchestrator` → clean.

- [ ] **Step 6: Commit**
```bash
git add crates/rupu-orchestrator/src/runner.rs crates/rupu-orchestrator/src/lib.rs crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(orchestrator): UnitDispatcher port + distributed for_each placement"
```
(The cmd/workflow.rs change here is ONLY adding `unit_dispatcher: None` to the opts literals to keep it compiling; the real wiring is Task 5.)

---

## Task 4: Per-unit host attribution in events + checkpoints

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/event.rs` (`UnitStarted`/`UnitCompleted`), `crates/rupu-orchestrator/src/runs.rs` (`UnitCheckpoint`), and the emit sites in `runner.rs`.

**Interfaces — Produces:** `host: Option<String>` on `Event::UnitStarted`, `Event::UnitCompleted`, and `UnitCheckpoint` (None = local).

- [ ] **Step 1: Failing test** — a `UnitCheckpoint` serde round-trip with `host: Some("h1")`, and a `UnitStarted` event constructed with `host` (compile-level). Add a small serde test in `runs.rs` for `UnitCheckpoint` host round-trip.

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Add the field** (with `#[serde(default, skip_serializing_if = "Option::is_none")]`) to `Event::UnitStarted`, `Event::UnitCompleted` (event.rs) and `UnitCheckpoint` (runs.rs). Update the emit sites in `runner.rs` (`UnitStarted`/`UnitCompleted` emission + the `UnitCheckpoint` build) to pass the unit's placement host (`Some(host)` for placed units, `None` for local). Update all other constructors of these (grep) to pass `host: None`.

- [ ] **Step 4: Run** the serde test + `cargo test -p rupu-orchestrator` (existing event/checkpoint tests green) → PASS; clippy clean.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-orchestrator/src/executor/event.rs crates/rupu-orchestrator/src/runs.rs crates/rupu-orchestrator/src/runner.rs
git commit -m "feat(orchestrator): host attribution on unit events + checkpoints"
```

---

## Task 5: `FleetUnitDispatcher` + wire into `rupu workflow run`

**Files:**
- Create: `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (+ `mod` in lib.rs)
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (build registry, inject dispatcher)
- Test: `fleet_unit_dispatcher.rs` `#[cfg(test)]` (fake `HostConnector`)

**Interfaces — Consumes:** `UnitDispatcher`/`UnitDispatch`/`UnitOutcome` (T3); `rupu_cp::host::registry::HostRegistry` + `HostConnector::{launch_agent, get_run}`; `AgentLaunchRequest`; `RunRecord.final_output` (T2).

- [ ] **Step 1: Failing test** with a fake `HostConnector`:

```rust
// fake connector: launch_agent returns "run_x"; get_run returns a JSON run record
// whose status is "completed" and final_output is "fake-out" (after one poll).
#[tokio::test]
async fn fleet_dispatch_reads_final_output_from_mirror() {
    let reg = /* HostRegistry wired so resolve("h1") -> FakeConnector */;
    let d = FleetUnitDispatcher::new(reg);
    let out = d.dispatch_unit(UnitDispatch{ step_id:"s".into(), agent:"a".into(),
        rendered_prompt:"p".into(), index:0, run_id:"r".into() }, "h1").await.unwrap();
    assert_eq!(out.output, "fake-out");
    assert!(out.success);
}
#[tokio::test]
async fn fleet_dispatch_unreachable_host_errors() {
    // resolve("h1") -> connector whose launch_agent returns Unreachable → dispatch_unit Err
}
```
Build the fake by implementing `rupu_cp::host::connector::HostConnector` (only `launch_agent`+`get_run` need real behavior; others `unimplemented!()`), and construct a `HostRegistry` that resolves to it (reuse the registry test helpers from `crates/rupu-cp/tests/host_registry.rs` if accessible, or construct via `with_tunnel_deps` + a custom local; simplest: have FleetUnitDispatcher take an `Arc<dyn HostConnector>` resolver seam for tests — see Step 3).

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Implement `FleetUnitDispatcher`.**
```rust
pub struct FleetUnitDispatcher { registry: std::sync::Arc<rupu_cp::host::registry::HostRegistry> }
#[async_trait::async_trait]
impl rupu_orchestrator::runner::UnitDispatcher for FleetUnitDispatcher {
    async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, RunError> {
        let conn = self.registry.resolve(host).map_err(to_run_err)?;
        let run_id = conn.launch_agent(AgentLaunchRequest {
            agent: unit.agent, prompt: Some(unit.rendered_prompt), mode: None,
            target: None, working_dir: None,
        }).await.map_err(to_run_err)?;
        // Poll the mirrored run until terminal (bounded), then read final_output.
        loop {
            let rec = conn.get_run(&run_id).await.map_err(to_run_err)?;
            let status = rec["status"].as_str().unwrap_or("");
            if is_terminal(status) {
                let output = rec["final_output"].as_str().unwrap_or("").to_string();
                let success = status == "completed";
                return Ok(UnitOutcome { output, success, error: (!success).then(|| status.to_string()) });
            }
            tokio::time::sleep(POLL).await; // small interval, e.g. 500ms, with a max attempts cap
        }
    }
}
```
`get_run` returns `serde_json::Value` (the mirrored run detail) — read `status` + `final_output`. Bound the poll with a max-attempts/timeout → on timeout return Err. `to_run_err` maps `HostConnectorError`→`RunError`. For testability, consider a small internal seam so the test can inject a fake connector without a full registry (e.g. an `enum`/constructor `FleetUnitDispatcher::from_connector(Arc<dyn HostConnector>)` used by tests, and `new(registry)` for prod that resolves per call). Pick the cleanest testable shape.

- [ ] **Step 4: Wire into `rupu workflow run`.** In `crates/rupu-cli/src/cmd/workflow.rs`, where `OrchestratorRunOpts` is built (~line 2262 and the resume site ~3334): if the workflow has ANY step with `distribute.is_some()`, build a `HostRegistry` from the host store (mirror how `cp serve` builds it — `HostStore { root: global.join("hosts") }` + `with_tunnel_deps(node_registry, node_mirror over the run_store, run_store, pricing)` so mirrored runs land in the SAME run_store the coordinator reads) and set `unit_dispatcher: Some(Arc::new(FleetUnitDispatcher::new(registry)))`. Otherwise `unit_dispatcher: None`. (Read how `cp serve` / `lib.rs serve()` constructs the registry and replicate the minimal wiring.) A `distribute:` workflow run that cannot build a registry (no host store) surfaces the T3 run error.

- [ ] **Step 5: Run** `cargo test -p rupu-cli --lib fleet_unit_dispatcher` → PASS; `cargo build -p rupu-cli` → compiles; `cargo clippy -p rupu-cp` → clean (rupu-cli baseline noted).

- [ ] **Step 6: Commit**
```bash
git add crates/rupu-cli/src/fleet_unit_dispatcher.rs crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(cli): FleetUnitDispatcher + wire distributed for_each into workflow run"
```

---

## Task 6: e2e — distributed fan-out across two placements

**Files:**
- Create: `crates/rupu-orchestrator/tests/distributed_fanout_e2e.rs` (orchestrator-level, using a fake dispatcher) OR extend an existing orchestrator integration test.

- [ ] **Step 1: Write the e2e.** Drive `run_workflow` (or `run_fanout_step` via the integration harness) with a real `Workflow` containing a `for_each` step with `distribute.hosts = [h1, h2]` and a fake `UnitDispatcher` that returns per-(index,host) outputs. Assert:
  1. all units dispatched (round-robin h1/h2 by index),
  2. `steps.<id>.results[]` aggregated in index order from the dispatcher outputs,
  3. the run completes as ONE run with `unit_checkpoints.jsonl` entries carrying the per-unit `host`,
  4. a no-`distribute` control workflow runs locally with identical results (backward-compat in the same test file).

Use a `run_store`-backed harness so checkpoints/events are written and assert the `host` attribution from `unit_checkpoints.jsonl`. (A real cross-transport fan-out is a manual/follow-up validation, not a code dependency — the dispatcher seam is the boundary.)

- [ ] **Step 2: Run** `cargo test -p rupu-orchestrator --test distributed_fanout_e2e` → PASS. Then full `cargo test -p rupu-orchestrator -p rupu-cp` green + `cargo clippy -p rupu-orchestrator -p rupu-cp` clean.

- [ ] **Step 3: Commit**
```bash
git add crates/rupu-orchestrator/tests/distributed_fanout_e2e.rs
git commit -m "test(orchestrator): distributed for_each e2e (round-robin + aggregation + host attribution)"
```

---

## Self-Review

**Spec coverage:** placement YAML → T1; agent-run run.json+final_output (uniform retrieval) → T2; UnitDispatcher port + run_fanout_step placement/retry → T3; per-unit host observability → T4; FleetUnitDispatcher + coordinator wiring + distribute-without-fleet error → T5; e2e + backward-compat → T6. Self-contained-unit guardrail → documented in spec + the T5/T3 error for missing fleet; validator warning for workspace-tool units is acknowledged in the spec as best-effort (not a hard task — note: a full validator warning can be folded into T1 if cheap, else deferred — see note below).

**GAP note:** the spec mentions a validator that "warns where feasible" for workspace-dependent units. That is heuristic and low-value for 3a; T1's validation covers the structural rules (distribute only on for_each, non-empty hosts). The workspace-dependence warning is explicitly deferred to 3c (where workspace sync makes it actionable) — call this out in the PR, do not build a brittle heuristic now.

**Placeholder scan:** the only deferred-to-real-API spots are: the exact `RunError` variant (T3), `RunStore::create` vs an upsert (T2 Step 5), and the registry-construction mirror of `cp serve` (T5 Step 4) — each flagged "read the real signature and adapt," which is using real APIs, not placeholders. No TBDs.

**Type consistency:** `UnitDispatch { step_id, agent, rendered_prompt, index, run_id }`, `UnitOutcome { output, success, error }`, `UnitDispatcher::dispatch_unit(unit, host)`, `OrchestratorRunOpts.unit_dispatcher: Option<Arc<dyn UnitDispatcher>>`, `RunRecord.final_output`, `Distribute { hosts }`, and `host: Option<String>` on unit events/checkpoint are used consistently across T1–T6.

---

## Process Note

Branch + single PR. `worktree-multi-host-slice-3a` (from main v0.29.4). Build subagent-driven (TDD). After all tasks: final whole-branch review (opus), then finishing-a-development-branch → push + PR (no self-merge). This is 3a of the Slice-3 arc (3b per-step placement, 3c workspace sync to follow).
