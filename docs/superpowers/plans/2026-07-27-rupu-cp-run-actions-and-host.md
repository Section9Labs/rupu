# rupu CP — Runs-section row actions + host-aware mutations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox syntax.

## Context

Operator: *"still I see no delete archive in the runs → agents. You are fixing the build section tables but not the runs section tables."* and *"fix the gap of `?host=` — we need to support other hosts since we will rely more and more on remote hosts."*

Today `WorkflowRuns` and `Sessions` have Archive/Restore/Delete; `AgentRuns` and `AutoflowRuns` have none. A read-only investigation (`.superpowers/sdd/audit-run-actions.md`) established exactly what is possible; this plan implements it in the order that investigation recommended.

**Key facts (verified, do not re-derive):**
- Agent runs are NOT in the workflow `RunStore` (rooted `<global>/runs/`). Standalone agent runs are two flat files `<global>/transcripts/<run_id>.{meta.json,jsonl}`; session turns live under `<global>/sessions/<id>/`. Generic `/api/runs/:id/*` therefore 404s for them — safely, but only because the id-spaces don't share a directory.
- Every `AgentRunRow` labelled `source: "session"` is guaranteed to carry a non-null `session_id` (`run_streams.rs:490-495`), so existing session endpoints cover that half with **no new backend**.
- Standalone rows need new backend. The CLI already has `rupu transcript archive|delete` plus a `transcripts-archive/` convention (`rupu-cli/src/paths.rs:61`, `cmd/transcript.rs:1530-1568`). **`rupu transcript restore` does NOT exist** — so there is no Restore for standalone rows in this plan.
- **Load-bearing guard:** `ensure_standalone_transcript()` (`cmd/transcript.rs:1711-1726`) refuses to archive/delete a transcript whose metadata carries a `session_id`. New backend MUST shell the existing CLI command and inherit this guard — never reimplement the file move.
- Archive/restore/delete run + session endpoints take **no `?host=`** and never proxy, while approve/reject/cancel/pause/resume all thread `RunControlQuery{host,gate}` (`api/runs.rs:91-97`). So those buttons are already broken for remote rows in `WorkflowRuns`/`Sessions` today (404, fails safe but misleading).
- AutoflowRuns event rows have no addressable storage; only rows carrying a `run_id` (`kind: "run_launched"`) are actionable, and those are genuine workflow runs.

## Global Constraints

- **Rust:** additive/back-compatible; `?host=` params optional (absent ⇒ today's local behavior). `unsafe_code` forbidden. **NEVER run `cargo fmt` in any form** — hand-format only.
- **Destructive actions** keep the established pattern: `validate_*` guard before resolution, `window.confirm` naming exactly what is destroyed and that it cannot be undone, error surfaced via the page's banner, list refreshed only on success.
- **Row actions live in `interactive: true` columns** and every button calls BOTH `preventDefault()` and `stopPropagation()` (tables use `rowHref` whole-row navigation).
- No new `--c-*` tokens. No fabricated capabilities — if a verb doesn't exist server-side, don't render a button for it.
- Tests: `cargo test -p rupu-cp` + full `npx vitest run` + `npx tsc -b` + `npm run build` all clean. Collision/edge fixtures, not happy-path-only (this codebase has twice shipped wrong-target bugs that green happy-path suites missed).

---

## Task 1: Host-aware archive / restore / delete (runs + sessions)

**Files:** `crates/rupu-cp/src/api/runs.rs` (`archive_run`/`restore_run`/`delete_run`), `crates/rupu-cp/src/api/sessions.rs` (archive/restore/delete session), `web/src/lib/api.ts`, callers `web/src/pages/runs/WorkflowRuns.tsx`, `web/src/pages/Sessions.tsx`, `web/src/components/project/ProjectSessionsTab.tsx`; tests.

Mirror the EXISTING proxy pattern used by `approve_run`/`cancel_run` (`api/runs.rs:134-160`) exactly — same query struct shape, same host resolution, same `HostConnector` delegation. Absent/`local` host ⇒ current local behavior unchanged.
Thread `host` from every caller that renders these buttons (rows already carry `host_id`).

- [ ] Failing tests: a remote-host row's archive/delete proxies to that host (not the local store); local rows unchanged; absent host param behaves exactly as today.
- [ ] Implement; run suites. Commit `-m "feat(cp): host-aware run and session archive/restore/delete"`.

## Task 2: Transcript mutator backend (standalone agent runs)

**Files:** new `crates/rupu-cp/src/transcript_mutator.rs` (port), new `crates/rupu-cp/src/api/transcripts.rs` (routes), `crates/rupu-cli/src/cp_transcript_mutator.rs` (subprocess adapter), wiring in `crates/rupu-cp/src/state.rs` + `api/mod.rs` + the `cp serve` construction site; tests.

Mirror `session_mutator.rs` / `cp_session_mutator.rs` **1:1** — that pair is the reference implementation.
- Port: `Archive` + `Delete` only (no Restore — the CLI verb doesn't exist).
- Routes: `POST /api/transcripts/:id/archive`, `DELETE /api/transcripts/:id`; `validate_id` guard first; **501 when the mutator is absent** (i.e. not running under `rupu cp serve`), exactly as `mutate_session` does (`sessions.rs:697-719`).
- Adapter shells `rupu transcript archive|delete <run_id> [--force]` and classifies stderr: `"is managed by session"` ⇒ 409/Invalid, missing/unreadable ⇒ 404, else 500.
- Host-aware from birth (Task 1's pattern) — do not ship a second local-only mutation surface.

- [ ] Failing tests: archive/delete succeed for a standalone transcript; a session-owned transcript is REFUSED (the `ensure_standalone_transcript` guard must surface as an error, never a silent clobber); missing id ⇒ 404; no mutator ⇒ 501.
- [ ] Implement; run suites. Commit `-m "feat(cp): transcript archive/delete port and routes"`.

## Task 3: AgentRuns + AutoflowRuns row actions

**Files:** `web/src/pages/runs/AgentRuns.tsx`, `web/src/pages/runs/AutoflowRuns.tsx`, `web/src/lib/api.ts`; tests.

**AgentRuns** — action column keyed off `source`:
- `source === 'session'` ⇒ Archive/Restore/Delete via the SESSION endpoints using `r.session_id`. The action targets the WHOLE session: the confirm copy must say so explicitly (e.g. *"Archive session `sess_x` and all its turns?"*). Decide and document how multiple rows sharing one `session_id` behave (share the outcome, or render the action on a representative row only) — pick one, state it in the report.
- `source === 'standalone'` ⇒ Archive/Delete via Task 2's transcript routes. **No Restore** (no CLI verb).
**AutoflowRuns** — render Archive/Delete ONLY on rows carrying a `run_id`, delegating to the existing `api.archiveRun`/`deleteRun`. Rows without a `run_id` (`awaiting_human`, `awaiting_external`, `cycle_failed`) get NO action — they have no addressable storage. Do not add a uniform action column.
Both: host-aware (pass the row's `host_id`), `interactive: true`, `preventDefault()+stopPropagation()`.

- [ ] Failing tests: session-sourced row calls the session endpoint with `session_id`; standalone row calls the transcript endpoint with `run_id`; an autoflow row WITHOUT `run_id` renders no action while one WITH it does; no action button navigates.
- [ ] Implement; run suites. Commit `-m "feat(cp-web): row actions on agent-run and autoflow-run tables"`.

## Task 4: Verify + PR
- [ ] Full suites + build; final whole-branch review (most capable model); fix Critical/Important; draft PR requesting an in-browser pass (incl. a REMOTE-host row).

## Out of scope
- `rupu transcript restore` (CLI verb doesn't exist) — Restore is therefore absent for standalone rows.
- Autoflow cycle-record or event deletion (no storage unit smaller than the cycle).
- The AutoflowRuns Cycles tab (no per-row identity).
