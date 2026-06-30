# rupu multi-host — Slice 3a: distributed fan-out units

Status: approved (design), pending implementation plan
Date: 2026-06-29

## Context

Slices 1–2b made the central CP able to dispatch a **whole** workflow/agent run
to one host, over five transports (`Local`, `HttpCp`, `Tunnel`, `Ssh`,
`Bucket`), with node runs mirrored into the central `RunStore` and observed
through one host-aware `HostConnector` port. A run is still atomic to a single
host.

Slice 3 is the headline multi-host capability: the steps of a **single**
workflow run execute across the fleet. The orchestrator map established the
governing constraint:

- Steps are a **linear list** (`Vec<Step>`), executed in declared order; a
  downstream step consumes an upstream step's output as a **string**
  (`{{ steps.<id>.output }}` / `.results[]`) — small, trivially movable.
- But all steps' agents read/write a **single shared `workspace_path`** (a git
  worktree). Moving a *file-mutating* step to another host is the hard part —
  the files must move, not just the data.

That splits Slice 3 into a tractable part (distribute work that does **not**
need the repo files relocated) and a hard part (sync the workspace). The
decomposition (each its own slice/PR):

- **3a (this spec) — distributed fan-out units.** A `for_each` step's units are
  already independent concurrent dispatches; fan them across the fleet.
- **3b — per-step host placement.** A `host:` on a (text/prompt) step.
- **3c — cross-host workspace/file sync.** Move artifacts/the worktree (likely
  via the bucket) so file-mutating steps can run remotely. Deferred.

3a is first: it is the horizontal scale-out win, maps onto the already-independent
unit-dispatch model, and exercises the whole distribution machinery (placement →
remote dispatch → result/event relay → aggregation → partial failure) on the
case that does not require workspace sync.

## Spine decisions (approved)

1. **Distribute `for_each` units across an explicit host list, round-robin.** A
   `distribute:` block on the step names fleet hosts; units are assigned
   round-robin. Absent ⇒ runs locally exactly as today (fully backward
   compatible). Capability/label selectors and `parallel:` sub-step distribution
   are later refinements.
2. **A `UnitDispatcher` port (hexagonal).** `rupu-orchestrator` defines the
   trait; the local impl is today's in-process `run_agent`; a remote impl (in
   `rupu-cli`, which already depends on both orchestrator + cp) dispatches a unit
   to its host via `HostConnector::launch_agent`. The orchestrator stays
   transport-agnostic.
3. **The coordinator is the `rupu workflow run` process, given fleet access.**
   `rupu-cli` builds a `HostRegistry` from the host store and injects the
   registry-backed `UnitDispatcher`. No need to move orchestration into
   `cp serve`. Without fleet access / without `distribute:`, everything runs
   local (unchanged).
4. **Uniform unit-output retrieval via the mirror — agent runs gain a `run.json`.**
   Verified: a bare `rupu run <agent> --run-id <id>` writes only a transcript +
   metadata file — it produces **no** `runs/<run_id>/` artifacts. So today a
   remotely-dispatched agent run's *output* is not retrievable centrally (the
   mirror tails the run dir, which is empty for agent runs; only a `create_run`
   stub + terminal status come back — a latent Slice-2 gap). 3a closes this: make
   `rupu run --run-id` write a **minimal `run.json`** (a `RunRecord` with
   terminal status + a new `final_output: Option<String>` = the final assistant
   text) under `<global>/runs/<run_id>/` on completion. The node-tail / CP poller
   already stream `run.json` (the `RunJson` artifact) on **every** transport, so
   the mirror carries `final_output` back uniformly. The remote `UnitDispatcher`
   reads `final_output` + status from the mirrored run after terminal — no
   per-transport transcript relay, and dispatch stays `launch_agent` (agents are
   shared definitions; dispatching a synthetic 1-step workflow would require
   shipping a workflow def to the host).
5. **Self-contained units only (the crux guardrail).** A distributed unit must be
   computable from its `item` + prior-step **string** context — no dependence on
   the shared file workspace being present on the remote host. Units that need
   the repo's files wait for 3c. The spec states this; validation warns where
   feasible.

## Goals (Slice 3a)

- A `for_each` step with `distribute: { hosts: [...] }` runs its units across the
  named fleet hosts (round-robin), each as a remote agent run on its host,
  honoring `max_parallel` as the total in-flight cap.
- Each unit's output + success are aggregated back into `steps.<id>.results[]`
  exactly as a local fan-out would — downstream steps are unaffected.
- The distributed run is **one** run (one `run_id`); each unit's
  `UnitStarted`/`UnitCompleted` event + `unit_checkpoints.jsonl` entry records
  the host that ran it; remote unit runs are observable via the mirror,
  host-attributed.
- A unit whose host is unreachable / fails to dispatch is **reassigned once** to
  the next host in the pool; a second failure is a failed unit honoring the
  step's existing `continue_on_error`.
- No `distribute:` ⇒ byte-for-byte the current local behavior.

## Non-goals (later)

- Cross-host **workspace/file sync** — units needing repo files (3c).
- **Per-step** placement of non-fan-out steps (3b).
- `parallel:` sub-step distribution (natural follow-on; 3a scopes to `for_each`).
- Capability/label host selection, load-aware scheduling (round-robin only).
- mTLS (Slice 4); sessions (Slice 5).

## Architecture

### 1. Placement model (`crates/rupu-orchestrator/src/workflow.rs`)

Add an optional `distribute: Option<Distribute>` to `Step`:

```rust
pub struct Distribute {
    /// Fleet host ids/names to spread this step's units across (round-robin).
    pub hosts: Vec<String>,
    // strategy: round_robin (only option in 3a)
}
```

Serde-optional, defaulting `None`. Validation: `distribute` is only meaningful on
a `for_each` step (error if set on a non-fan-out step); `hosts` must be
non-empty.

### 2. `UnitDispatcher` port (`crates/rupu-orchestrator/src/runner.rs` or a new `unit_dispatch.rs`)

Abstract the per-unit dispatch that `dispatch_one` performs today:

```rust
pub struct UnitOutcome { pub output: String, pub success: bool, pub error: Option<String> }

#[async_trait]
pub trait UnitDispatcher: Send + Sync {
    /// Run one unit (an agent invocation) on the placement target and return
    /// its final output + success. `placement` is None for local execution.
    async fn dispatch_unit(&self, unit: UnitDispatch<'_>, placement: Option<&str>)
        -> Result<UnitOutcome, RunError>;
}
```

`UnitDispatch` carries what `dispatch_one` needs (step_id, agent, rendered
prompt, run_id, workspace ids/paths, transcript_path). The orchestrator holds an
`Option<Arc<dyn UnitDispatcher>>` in `OrchestratorRunOpts`:

- **`None` (default)** → the existing in-process path: `dispatch_one` +
  `read_final_assistant_text(transcript_path)` — unchanged behavior.
- **`Some(dispatcher)`** → `run_fanout_step` computes each unit's placement
  (round-robin over `distribute.hosts`, or local if no `distribute`) and calls
  `dispatcher.dispatch_unit(unit, placement)`. For a `None` placement the
  dispatcher runs locally (same as today); for a host placement it dispatches
  remotely (below). The returned `UnitOutcome.output` feeds the existing
  `ItemResult { output, success, .. }` aggregation; remote units skip the
  local-transcript read (the output came back over the wire).

This keeps `run_fanout_step`'s structure (semaphore, `max_parallel`, ordering,
`unit_checkpoints`, events) and only swaps the per-unit "run it" call behind the
port.

### 3. Local + remote dispatchers (`crates/rupu-cli`)

- **`LocalUnitDispatcher`** wraps the existing `dispatch_one` + transcript read.
- **`FleetUnitDispatcher { registry: Arc<HostRegistry> }`** (rupu-cli):
  - placement `None` → delegate to the local dispatcher.
  - placement `Some(host_id)` → resolve the host via the registry →
    `HostConnector::launch_agent(AgentLaunchRequest { agent, prompt: rendered, .. })`
    → returns a `run_id` → poll `get_run(run_id)` (the mirrored run) until
    terminal → read `final_output` + status from the mirrored run record →
    `UnitOutcome`. On `launch_agent`/unreachable error or a failed run, return
    an error so `run_fanout_step` can reassign/fail per policy.

`rupu workflow run` builds the `HostRegistry` (from the host store) and injects
`FleetUnitDispatcher` when the workflow contains any `distribute:` step (else the
local dispatcher / `None`). Coordinator runs on a host with fleet reachability
(the CP host).

### 4. Agent runs become run-dir-backed (`run.json` + `final_output`)

Add `RunRecord.final_output: Option<String>` (additive, backward-compatible).
Then make `rupu run <agent> --run-id <id>` (`crates/rupu-cli/src/cmd/run.rs`)
write a minimal `run.json` under `<global>/runs/<run_id>/` on completion: a
`RunRecord` with `status` (Completed/Failed), `started_at`/`finished_at`, and
`final_output` set from the transcript's final assistant text (the same
`read_final_assistant_text` logic the orchestrator uses locally). The agent run
keeps writing its transcript as today; this just adds the run-dir record.

Because the node-tail (`tunnel`/`ssh` via the run-dir tail, `bucket` via the
pull agent) and the CP poller already stream `run.json` as the `RunJson`
artifact, this `final_output` rides the **existing** mirror back to the central
RunStore on every transport — no new per-transport plumbing. The remote
`UnitDispatcher` then reads `final_output` from `get_run(run_id)` (the mirrored
record) once the run is terminal. This also makes remote **agent** runs (not
just workflow runs) observable centrally, closing a latent Slice-2 gap.

### 5. Placement + retry in `run_fanout_step`

Round-robin assign `distribute.hosts` to units by index. Wrap each unit's
`dispatch_unit` call: on a dispatch/host error, **reassign once** to
`hosts[(i+1) % len]` and retry; a second failure yields a failed `ItemResult`
(error recorded), then the existing `continue_on_error` logic decides abort vs
proceed. `max_parallel` still bounds total concurrent units (local + remote).

### 6. Observability

`run_fanout_step` already emits `UnitStarted`/`UnitCompleted` and writes
`unit_checkpoints.jsonl`. Extend the unit event + checkpoint with the **host**
that ran the unit (add a `host: Option<String>` to `UnitStarted`/`UnitCompleted`
and `UnitCheckpoint`; `None` = local). The distributed run remains one `run_id`
with one `events.jsonl`; remote unit runs are independently observable as
mirrored, host-attributed runs (the unit's `run_id` links them). The CP run
detail can later surface "unit N ran on host X" from these.

## Errors & security

- Unreachable host / dispatch failure → one reassignment, then failed unit
  (honors `continue_on_error`). No silent drops; the failure is recorded on the
  `ItemResult` and emitted as `UnitCompleted{success:false, host}`.
- A `distribute:` step encountered by a coordinator **without** fleet access
  (no registry-backed dispatcher injected) is a clear run error
  ("distribute requires fleet access; run via the CP") rather than silently
  running locally — so placement intent isn't lost.
- No new secrets; remote dispatch reuses the already-authenticated transports.
- Self-contained-unit guardrail documented; where detectable (e.g. a unit agent
  that declares file tools) the validator warns that distributed units can't see
  the local workspace (full enforcement is 3c's concern).
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI `anyhow`;
  workspace deps only.

## Testing

- **Workflow model:** `Distribute` serde + validation (only on `for_each`,
  non-empty `hosts`; rejected elsewhere).
- **UnitDispatcher seam (orchestrator):** with an injected **fake** dispatcher,
  a `for_each` step assigns units round-robin to the configured placements and
  aggregates the returned `UnitOutcome`s into `results[]` in order; `max_parallel`
  respected; a unit error triggers one reassignment then fails honoring
  `continue_on_error`. (No real hosts — the fake records placements + returns
  canned outputs.) This is the core TDD surface.
- **`final_output`:** an agent run records its final assistant text on the
  `RunRecord`; the mirror round-trips it (extend a mirror test).
- **`FleetUnitDispatcher` (rupu-cli, fake connector/registry):** a host
  placement calls `launch_agent` and reads `final_output` from the mirrored run;
  unreachable → error → reassignment. Uses a fake `HostConnector` (no real
  transport).
- **Backward compat:** a workflow with no `distribute:` runs through the local
  dispatcher with identical results to today (existing fan-out tests stay green).
- **e2e (in-process / Local + a fake-fleet host):** a `for_each` step with
  `distribute` over two placements runs units across them and aggregates; the run
  renders as one run with per-unit host attribution.

## Open questions

- **`final_output` size cap:** unit outputs can be large; 3a stores the full
  final assistant text on the record (consistent with `step_results.jsonl` which
  already stores `output`). A size cap/truncation policy can come later if needed.
- **Which transports to exercise first:** the mechanism is transport-uniform
  (mirror-read), so any transport works; the e2e uses Local + a fake host. Real
  multi-transport validation (tunnel/ssh/bucket fan-out) is a manual/follow-up
  step, not a code dependency.
