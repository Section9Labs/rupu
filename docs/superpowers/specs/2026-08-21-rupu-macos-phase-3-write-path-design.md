# rupu.app macOS — Phase 3: Write Path design

**Date:** 2026-08-21
**Status:** Approved in design session (matt, 2026-08-21); spec review pending
**Parent:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella, §8 Phase 3 row)
**Visual contract:** `docs/macOS_design/HANDOFF.md` — screen 9 (Launcher); gate/permission
tone rules (read-only=done, ask=await, bypass=fail — always loud)

Phase 3 makes the app an operator console: the Launcher sheet, gate Approve/Reject,
run controls (cancel/pause/resume), session send, and archive/restore — one phase,
one plan (settled with matt).

## 1. The mutation contract: pending-state, not optimistic (umbrella §6 amendment)

The umbrella prescribed optimistic mutations (instant flip, rollback on failure).
That contradicts the backend's real semantics: `cp serve` runs are detached
subprocesses — approve is a marker file consumed by a resume worker/sweep;
cancel/pause travel by signal to a recorded `runner_pid`. A POST's 200 means
*recorded*, not *done*. Phase 3 therefore ships the honest contract, and this spec
amends umbrella §6 accordingly (same PR):

- POST 200 ⇒ **pending** ("recorded — taking effect").
- **Confirmation = observed effect**: the run's status transition arriving through
  the Phase 2 live machinery (run-scoped stream, firehose patch, or refresh) —
  approve ⇒ status leaves `awaiting_approval`; cancel ⇒ `cancelled`;
  pause ⇒ `paused`; resume ⇒ leaves `paused`; archive/restore ⇒ the row moves
  between scopes on the next refresh.
- Pending older than 30s ⇒ a dim "still pending — server sweep may be delayed"
  note (never fake success, never red).
- POST failure ⇒ failed state with the error inline and a retry affordance.

## 2. Write plumbing

**CPClient** gains POSTs: `approveRun/rejectRun(id:host:gate:)`,
`cancelRun/pauseRun/resumeRun/archiveRun/restoreRun(id:host:)`,
`launchAgentRun(name:body:)`, `startAgentSession(name:body:)`,
`runWorkflow(name:body:)`, `validateWorkflow(name:body:)`,
`sendToSession(id:body:)`, `fanOut(body:)` — and the Launcher's read endpoints:
`agentDefinitions()`, `workflowDefinitions()`, `tools()`. Exact request/response
shapes are extracted from the Rust handlers at plan time (Phase 2 discipline) and
locked with golden fixtures **in both directions**: response fixtures decoded by
Swift tests, and request-body fixtures asserted by rupu-cp tests so the server side
notices if the app's request shape drifts.

**`PendingAction`** (new RupuStore primitive): a keyed map
`(entityID, verb) → idle | pending(since) | confirmed | failed(String)` with a pure,
fixture-testable reduction: `resolve(status:)` applies the per-verb confirmation
table above. Stores own instances; views render buttons from it (spinner-in-button
while pending, inline error + retry on failure). Keys are shared across screens so
an Activity row and the Run detail page can never disagree about an in-flight
action.

## 3. Mutations in existing screens

- **Run detail**: the awaiting banner gets Approve/Reject (per-gate via
  `awaiting[].step_id` when several gates are parked), replacing the Phase 2
  "ACTIONS ARRIVE IN PHASE 3" label. Header: Cancel, and Pause **or** Resume —
  rendered only when the current status makes the verb meaningful (no dead verbs,
  no disabled-forever buttons). Overflow menu: Archive/Restore.
- **Activity**: `awaiting` rows get inline Approve/Reject (same PendingAction keys).
- **Session detail**: send box (text field + Send) replaces the read-only label;
  Send disabled while a turn is pending; the reply arrives via the existing
  child-run/transcript machinery. Archive/Restore in an overflow.

## 4. Launcher (new module `RupuLauncher`, HANDOFF screen 9)

Sheet from toolbar "+ New run" and ⌘N:

1. Kind segmented: agent run / session / workflow.
2. Definition picker fed by `agentDefinitions()` / `workflowDefinitions()`
   (model + scope shown per row).
3. Body: prompt textarea (agent/session) or declared-inputs kv rows (workflow).
4. Mode segmented read-only / ask / bypass — tone rule: bypass always loud (fail
   color), matching the Library permission-badge rule.
5. **Host chips, resilient** (Phase 2 lesson is binding): populated progressively
   from `/api/hosts` — online hosts appear as they're known, load figures shown
   only if the API already provides them cheaply, offline/faulted chips disabled;
   the sheet NEVER blocks on a slow host. Plus "fan out: all healthy" via
   `host_fanout`.
6. Footer: Launch with PendingAction feedback; workflow `validate` runs before
   launch and renders errors inline. On success, navigate to the new run detail /
   session detail — Phase 2's live machinery takes over from there.

## 5. Carry-overs (from the Phase 2 ledger)

- **Navigation stack**: `navigateBack`'s single-level `lastActivityRoute` becomes a
  real push/pop route stack in AppModel — back always returns to the previous
  screen (fixes the session → child run → back quirk matt flagged). Sidebar
  highlight rules unchanged.
- **stopTail ordering token**: `reloadTranscriptSnapshot`/tail teardown gets a
  generation/teardown token so a straggling tail apply cannot land after the
  terminal snapshot (closes the parked final-review residual).
- **statusOverrides pruning**: overlay entries pruned per-key when their run id
  ships in a refreshed page (not only `removeAll`).
- **Firehose scope**: documented behavior — standalone agent runs never enter
  RunStore and thus never appear on the firehose; the pill/live patching remain
  orchestrator-run-scoped by design. A code comment + CLAUDE.md note, not a
  workaround.

## 6. Testing & validation

- PendingAction reduction: per-verb confirmation table, timeout, failure, retry.
- Store mutation tests with scripted streams: approve POST → pending → status
  event arrives → confirmed; cancel/pause/resume likewise; send-turn pending.
- Launcher form-state tests: kind switching resets appropriately, validation
  gates Launch, host-chip states from scripted `/api/hosts` data.
- Fixtures both directions (see §2).
- Live validation with matt: from the app — launch a gated workflow, approve the
  gate, watch it complete; cancel a running run; pause/resume; send a session
  turn; archive/restore a run. GUI rule unchanged: matt runs the app before merge.

## 7. Out of scope

Menu bar extra + its inline approvals (Phase 6), Overview needs-you widget
(Phase 4), saved views (Phase 4), remote streaming (Phase 5), workflow authoring/
editing (CP web only), delete-run (destructive; not in the phase row), transcripts
archive endpoints beyond run archive (tracked in the umbrella ledger).
