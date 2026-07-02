# rupu — Pause / interrupt-and-resume for agent runs & workflows

Status: approved (design), pending implementation plan
Date: 2026-07-01

## Context

Today a run can be **cancelled** (terminal): `InProcessExecutor` holds a
`tokio_util::sync::CancellationToken` whose `cancel()` aborts the run task,
`HostConnector::cancel_run` cancels a remote run, `POST /api/runs/:id/cancel`
exposes it, and the executor surfaces `ExecutorError::Cancelled`. `RunStatus` is
`Pending / Running / Completed / Failed / AwaitingApproval / Rejected /
Cancelled` — there is no non-terminal **Paused**.

Workflows already have pause/resume *machinery*: `OrchestratorRunOpts.resume_from:
Option<ResumeState>`, `AwaitingInfo`, the `AwaitingApproval` status,
`step_results.jsonl` persistence, and a resume worker that lives in the full
`rupu cp serve` runtime (per the record-in-CP / resume-in-cp-serve split). A
`workspace: sync` workflow currently **refuses** checkpoint resume
(`RunWorkflowError::ResumeWithWorkspaceSync`).

Sessions interrupt via `cancel_active_turn` (Esc cancels the active *turn*; the
persistent session conversation survives; the "next turn" is a fresh provider
request). This is the UX to lift to runs and workflows: interrupt now, keep the
run resumable, continue easily — **not** a terminal failure.

**Key technical reality:** a partial LLM/provider SSE stream cannot be resumed.
Once interrupted, resuming means a **new provider request** built from the
preserved transcript (completed messages + tool results) — exactly how sessions
already work.

## Spine decisions (approved)

1. **Boundary: stop the stream now, but let a running tool finish.** A pause hit
   while streaming assistant text stops the stream immediately and **drops the
   partial assistant text** (transcript ends at the last complete message). A
   pause hit while a tool is executing **lets that tool finish and records its
   result**, then stops. No dangling tool calls, no half-done side effects.
2. **Resume = a fresh provider request from the preserved transcript.** The run's
   JSONL transcript is the source of truth; resume re-enters the agent loop and
   continues (the session model). No attempt to resume a partial stream.
3. **Reach: in-process, remote hosts, and distributed fan-out** (sequenced in the
   plan). Where a transport can't pause, a clear error — never a fake pause.
4. **New non-terminal `Paused` `RunStatus`**, distinct from `Cancelled`
   (terminal) and `AwaitingApproval` (waiting on a human approve/reject
   decision). Pause is **human-triggered at any time**; that is the difference
   from an approval gate even though the resumable state is similar. Both share
   the resume path.
5. **`workspace: sync` workflows refuse pause for now** (consistent with the
   existing `ResumeWithWorkspaceSync` resume refusal — their checkpoint resume
   isn't safe yet). A **TODO** is left to lift this: supporting pause/resume for
   `workspace: sync` workflows requires persisting unit workspace deltas so a
   checkpoint resume re-applies them — tracked as a follow-up.

## Goals

- Interrupt a running agent run or workflow at a safe boundary, leaving it
  `Paused` and resumable — not failed.
- Resume a paused run and have it genuinely continue to completion (new provider
  request from the preserved transcript / from the next workflow step).
- Works for in-process runs, remote-host runs, and distributed fan-out; a
  transport that can't pause returns a clear error.
- A paused run can be resumed or cancelled; it never silently fails or
  auto-resumes.
- Observable: `Paused` status, `RunPaused`/`RunResumed` events, CP + CLI
  affordances.

## Non-goals (v1 / later)

- Auto-resume / pause timeouts.
- Resuming a partial provider stream (impossible by design).
- Pausing an already-terminal run (Completed/Failed/Cancelled/Rejected).
- Lifting the `workspace: sync` pause refusal (TODO — needs delta-persisting
  resume; tracked follow-up).

## Architecture

### 1. `Paused` status + two-mode interrupt signal

- Add `RunStatus::Paused` (`"paused"`), non-terminal. Persisted in `RunRecord`;
  a paused run is resumable/cancellable.
- Extend the in-process interrupt from a single cancel to a **two-mode signal**:
  `Cancel` (terminal abort — unchanged) vs `Pause` (cooperative stop-at-boundary).
  Concretely, alongside the `CancellationToken` add a `PauseToken` (or a shared
  `InterruptSignal { cancel, pause }`); `cancel()` keeps its meaning, `pause()`
  requests a cooperative stop. Events: `Event::RunPaused` / `Event::RunResumed`
  (+ `StepPaused`/`StepResumed` for workflows), serde-optional/additive.

### 2. Agent loop (`rupu-agent`) — cooperative pause at safe boundaries

The agent loop checks the pause flag at **safe boundaries**: (a) when a provider
stream is in flight, stop consuming it and drop the partial assistant text;
(b) if a tool call is executing, let it complete and record its result first;
(c) between the stream and the next tool, and after each tool result, check pause
and unwind. On pause it returns a distinct `Paused` outcome (not an error), with
the transcript persisted through the last clean boundary. Resume calls back into
the loop with the persisted transcript and issues a fresh provider request.

### 3. Orchestrator (`rupu-orchestrator`) — run + workflow pause/resume

- **Agent run:** a bare `rupu run` / an agent unit honors the pause signal via
  the agent loop; on pause the run record is set `Paused` and `RunPaused` is
  emitted. Resume re-enters the loop from the transcript.
- **Workflow:** pause at a **step boundary** → set `Paused`, persist a
  `ResumeState` checkpoint (reuse `resume_from` + `step_results.jsonl`) → resume
  runs the remaining steps. A pause *during* a step's agent uses the agent-run
  pause; the step is recorded paused-incomplete and re-run from its transcript on
  resume. `Paused` and `AwaitingApproval` are distinct statuses that flow through
  the **same** resume entry point.
- `workspace: sync` workflow → pause is **refused** with a clear error (mirrors
  `ResumeWithWorkspaceSync`); TODO to support it.

### 4. Remote hosts (`rupu-cp` `HostConnector`)

Add `pause_run(run_id)` + `resume_run(run_id)` (beside `cancel_run`), default
`Unsupported`. The remote `rupu` (running this same feature) honors the pause on
its own in-process executor. **Local / SSH / HttpCp** implement it; **Bucket /
Tunnel** inherit the `Unsupported` default. The coordinator routes pause/resume
to the owning host; a pause request to an unsupported transport surfaces the
clear error.

### 5. Distributed fan-out

Pausing a `distribute:` step signals all **in-flight** units to pause at their
boundary; **completed** units keep their results; **not-yet-dispatched** units
aren't dispatched; **resume re-dispatches only the paused/pending** units
(extends the 3a "resume runs only the incomplete units" model to `Paused`).
Partial-pause is tracked per unit in the checkpoint.

### 6. API + CLI + where it lives

- `POST /api/runs/:id/pause` and `POST /api/runs/:id/resume`, beside `/cancel`.
  Pause interrupts the in-flight run (executor / host). **Resume requires the
  full `cp serve` runtime** (record-in-CP / resume-in-cp-serve) — it is
  launcher-gated like other mutations (501 on a read-only deploy).
- CLI: `rupu run pause|resume <id>` and `rupu workflow pause|resume <id>`, plus
  **Esc** in the live-run view (matching sessions) with a Resume affordance.

### 7. UX / observability

CP RunDetail **Pause / Resume** buttons (Resume replaces Pause when paused);
a new **Paused** `NodeStatus` in the graph view; the CLI live-run view Esc →
pause + a resume prompt; sidebar/menubar status reflects `Paused`;
`RunPaused`/`RunResumed` stream live.

## Errors & security

- Pause genuinely interrupts — verified by the `Paused` status + a `RunPaused`
  event emitted at the boundary (no silent no-op). Resume genuinely re-enters
  (new provider request; the run continues to completion or the next pause).
- An unsupported transport returns a clear `Unsupported`/`not_available` error;
  never a fake pause.
- Pausing an already-terminal run is rejected with a clear error.
- A `workspace: sync` workflow pause is refused with a clear message (+ TODO).
- Resume is launcher-gated (full `cp serve` runtime); the read-only CP surface
  cannot resume.
- No new secrets. `#![deny(clippy::all)]`; no `unsafe`; library errors
  `thiserror`, CLI/API `anyhow`/`ApiError`; workspace deps only. Hexagonal: the
  orchestrator/agent know only the interrupt signal + the `UnitDispatcher` /
  transport ports, never `rupu-cp`.

## Testing

- **Status/lifecycle:** `Paused` serde + non-terminal semantics; Running→Paused
  →Running; Paused→Cancelled; pausing a terminal run rejected.
- **Agent loop:** pause during a stream drops partial text + stops (transcript
  ends clean); pause during a tool call lets it finish + records the result then
  stops; resume issues a fresh request from the transcript and continues.
- **Workflow:** pause at a step boundary → Paused + checkpoint; resume runs the
  remaining steps; pause during a step re-runs that step on resume; Paused vs
  AwaitingApproval both resume via the same path; `workspace: sync` pause refused.
- **Remote:** Local/SSH/HttpCp `pause_run`/`resume_run` round-trip; Bucket/Tunnel
  → Unsupported.
- **Fan-out:** pausing a distribute step pauses in-flight units, keeps completed
  results, resume re-dispatches only the incomplete units.
- **API/CLI:** pause/resume endpoints (gating, terminal-run rejection);
  `RunPaused`/`RunResumed` events; CLI pause/resume + Esc.
- **e2e:** start a run → pause (assert Paused + event + no partial/half-done
  state) → resume (assert it completes); a workflow variant; a fan-out variant.

## Open questions

- **Interrupt-signal shape:** a distinct `PauseToken` alongside the existing
  `CancellationToken` vs a unified `InterruptSignal { cancel, pause }` enum.
  Resolve in the plan; **prefer** the unified signal so a single check-point
  handles both modes and cancel-after-pause is natural.
- **Paused-workflow resume vs approval resume unification:** how much of the
  `awaiting_approval` resume code the `Paused` resume reuses vs a parallel path.
  Resolve in the plan; **prefer** one resume entry point parameterized by the
  pause reason (approval vs manual).
- **Remote pause propagation for a run the coordinator launched but a host
  executes:** confirm the pause reaches the host's in-process executor through
  the same `HostConnector` the launch used. Resolve in the plan.
