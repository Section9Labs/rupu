# rupu multi-host — Slice 3b: per-step host placement

Status: approved (design), pending implementation plan
Date: 2026-06-30

## Context

Slice 3a (shipped in v0.30.0) distributes a `for_each` step's **units** across the
fleet via a remote-only `UnitDispatcher` port: the coordinator (`rupu workflow
run`) dispatches each unit's agent to a host with `HostConnector::launch_agent`
and retrieves its output from the mirrored run's `final_output`. It established
the whole machinery: the `UnitDispatcher` trait + `FleetUnitDispatcher`
(`launch_agent` → poll `get_run` → read `run.final_output`), agent runs writing
`run.json` + `final_output`, the coordinator building a `HostRegistry`,
host-attribution on observability, and the supported-transports reality.

Slice 3b is the second part of distributed workflows: place a **whole
(non-fan-out) step** on a named host. A `host:` field on a linear step runs that
step's agent on the host; the coordinator renders the prompt (from prior-step
string outputs it holds) and feeds the returned output into downstream steps.

3b is almost entirely **reuse** of 3a — the same dispatch + retrieval path, the
same coordinator/transports/guardrails. The remaining Slice-3 part is **3c**
(cross-host workspace/file sync), the hard one that lifts the self-contained
guardrail.

## Spine decisions (approved)

1. **`host: Option<String>` on a linear `Step`** — a single fleet host id. Valid
   only on a linear step (agent + prompt; not `for_each`/`parallel`/`panel`) and
   not co-occurring with `distribute:` (which is `for_each`-only). Absent ⇒ runs
   locally as today (backward compatible).
2. **Reuse the `UnitDispatcher` port — no new seam.** A placed linear step is
   exactly "run this agent+prompt on a host, return the output" = one unit.
   `run_linear_step`, when `step.host` is `Some`, calls
   `dispatch_unit(UnitDispatch{ index: 0, .. }, host)` instead of the inline
   `dispatch_one` + `read_final_assistant_text`, and builds the `StepResult` from
   the returned `UnitOutcome`. The local path (no `host:`) stays byte-for-byte.
3. **No reassignment.** A single named host has no alternate; on
   unreachable/failed dispatch the step **fails, honoring the step's
   `continue_on_error`** — identical to a local step failure. (3a's one-retry
   reassignment was only meaningful for a fan-out host pool.)
4. **Host attribution on step events.** `Event::StepStarted` /
   `Event::StepCompleted` gain `host: Option<String>` (None = local), mirroring
   the unit-event attribution 3a added. The run stays one coherent run; the step
   shows where it ran.
5. **Same coordinator / transports / guardrail as 3a.** Extend
   `build_dispatcher_if_needed`'s trigger to fire when the workflow has any
   `distribute:` **or** `host:` step. Supported transports unchanged: **Local /
   SSH / HttpCp** work from the `rupu workflow run` coordinator; **Bucket** needs
   a co-located `cp serve` sharing the runs dir; **Tunnel** placement is
   unsupported from the coordinator (clean, fast `Unreachable`). Same
   **self-contained-step guardrail**: a placed step must be computable from its
   rendered prompt + prior-step **string** context, not the shared file
   workspace (repo-file steps wait for **3c**).

## Goals (Slice 3b)

- A linear step with `host: <id>` runs its agent on that host; its output feeds
  downstream steps exactly as a local step would.
- The step's `host` appears on `StepStarted`/`StepCompleted` and in the run
  detail; the run is one coherent run.
- An unreachable/failed host placement fails the step honoring
  `continue_on_error` (no hang, no silent wrong result).
- No `host:` ⇒ byte-for-byte the current local behavior.

## Non-goals (later)

- Placing `parallel` / `panel` steps, and fan-out+placement combinations.
- A host **selector** (capability/label) — single id only in 3b.
- Cross-host workspace/file sync (3c) — placed steps remain self-contained.
- mTLS (Slice 4) / sessions (Slice 5).

## Architecture

### 1. Placement model (`crates/rupu-orchestrator/src/workflow.rs`)

Add `host: Option<String>` to `Step` (`#[serde(default, skip_serializing_if =
"Option::is_none")]`). Validation in the step-shape validator: if `host.is_some()`
then the step must be a **linear** step (has `agent` + `prompt`, and none of
`for_each`/`parallel`/`panel`) — else a clear error; and `host` must not be set
together with `distribute` (structurally impossible on a valid step since
`distribute` requires `for_each`, but assert it for a clear message). Empty
`host` string → error.

### 2. Routing a placed step (`crates/rupu-orchestrator/src/runner.rs`)

In `run_linear_step`: compute `placement = step.host.clone()`. If `Some(host)`:
require `opts.unit_dispatcher` (else the existing clear "fleet access required"
`RunError`); call
`opts.unit_dispatcher.dispatch_unit(UnitDispatch { step_id, agent, rendered_prompt, index: 0, run_id }, &host).await`;
build the `StepResult` from the `UnitOutcome` (`output`, `success`) the same way
the local path builds it from `dispatch_one` + `read_final_assistant_text`. On a
remote failure (`Err` or `Ok(success:false)`) produce a failed `StepResult`
whose error is surfaced so the existing `continue_on_error` abort logic applies
identically to a local failure — **no reassignment**. If `None`: the existing
inline path, unchanged.

This reuses the 3a port verbatim; no new trait. The placement host is threaded to
the step events (below).

### 3. Step-event host attribution (`crates/rupu-orchestrator/src/executor/event.rs` + emit sites)

Add `host: Option<String>` (serde-optional) to `Event::StepStarted` and
`Event::StepCompleted`. In `run_linear_step`'s emit sites, pass the placement
host (`Some(host)` for placed, `None` for local). Update all other constructors
of these events (other step kinds, tests) to `host: None`. (The persisted
`StepResultRecord` already exists; if cheap, surface the host there too via an
optional field so run detail can show it — otherwise the events carry it; decide
in the plan based on what the run-detail read path needs.)

### 4. Coordinator wiring (`crates/rupu-cli/src/fleet_unit_dispatcher.rs`)

Extend `build_dispatcher_if_needed`'s short-circuit from
`steps.iter().any(|s| s.distribute.is_some())` to
`steps.iter().any(|s| s.distribute.is_some() || s.host.is_some())`, so a
host-placed step also gets the registry-backed `FleetUnitDispatcher` injected at
all opts-build sites (run / resume / approve-resume). Everything else (registry
construction, the dispatcher, the 60 s bounded poll, the `get_run` envelope read)
is reused unchanged.

## Errors & security

- A `host:` step with no dispatcher (coordinator without fleet access) → the
  existing clear `RunError` (reused). No silent local fallback.
- Unreachable/failed host → failed step honoring `continue_on_error`; no hang
  (bounded poll), no silent wrong result.
- Tunnel host placement → clean fast `Unreachable` (documented limitation).
- No new secrets / no new inbound surface; reuses the authenticated transports.
- Self-contained-step guardrail documented; full enforcement is 3c's concern.
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI `anyhow`;
  workspace deps only; hexagonal (orchestrator knows only `UnitDispatcher`).

## Testing

- **Workflow model:** `host` serde on a Step; validation (host only on a linear
  step; rejected on for_each/parallel/panel; empty host rejected).
- **`run_linear_step` routing (fake `UnitDispatcher`):** a linear step with
  `host` set is dispatched through the port (assert the dispatcher saw the step's
  agent + rendered prompt + host) and the returned `UnitOutcome` becomes the
  `StepResult.output`; downstream steps see it via `{{ steps.<id>.output }}`. A
  remote failure (`Err` and `Ok(success:false)`) fails the step and aborts under
  `continue_on_error:false` / is tolerated under `true`. A `host:` step with no
  dispatcher → clear error. (Reuse the 3a fake-dispatcher harness.)
- **Backward compat:** a linear step with no `host:` runs through the unchanged
  inline path; existing linear-runner tests stay green.
- **Step-event host:** `StepStarted`/`StepCompleted` carry the host for a placed
  step, `None` for local (serde round-trip + an emit assertion).
- **Wiring:** `build_dispatcher_if_needed` returns `Some` for a workflow with a
  `host:` step and `None` for a plain workflow (no overhead when unused).
- **e2e:** a 2-step workflow where step 2 has `host:` (fake dispatcher) consumes
  step 1's output, runs "remotely", and feeds its output onward; the run renders
  as one run with per-step host attribution; a no-`host:` control is identical to
  today.

## Open questions

- **`StepResultRecord.host` vs event-only:** whether to persist the host on the
  step result record (for run-detail rendering) or rely on the
  `StepStarted`/`StepCompleted` events. Resolve in the plan based on how the CP
  run-detail read path surfaces step host (prefer the events if the UI already
  reads them; add the record field only if needed). Either way the host is
  observable.
