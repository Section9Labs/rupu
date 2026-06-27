# rupu CP web — live session feel + error surfacing + e2e

Date: 2026-06-27
Status: approved (design)

## Problem

The CP session view feels un-live and hides failures. In reality the agent
runner already **streams token deltas live** (`crates/rupu-agent/src/runner.rs`
writes `AssistantDelta` + flushes per `TextDelta`), and `SessionConversation`
already opens an SSE `TranscriptPanel` for the active turn — so the active turn
*does* stream. The actual gaps:

1. **Errors are invisible** — `SessionRecord.last_error` and per-run
   `runs[].error` live on disk but are NOT in the `/api/sessions/:id` or
   `/api/sessions/:id/runs` DTOs, so a failed turn (e.g. provider 401) shows
   nothing.
2. **Session state lags a flat 5s poll** — a new turn appearing / a turn
   finishing isn't instant.
3. **No "working…" status** while a turn is in flight.
4. **No session e2e test.**

Scope (decided): errors + e2e + live-feel polish. Deferred: streaming
tool-call activity (`ToolUseStart` → transcript, touches rupu-agent) and live
thinking-token deltas (provider-dependent).

## A. Surface errors

### Backend (`crates/rupu-cp/src/api/sessions.rs`)
- Add `last_error: Option<String>` to the session DTO (the `/api/sessions/:id`
  + list shape), read from `SessionRecord.last_error` (the on-disk session
  json already has it; add the field to the DTO struct that deserializes it and
  to the serialized response).
- Add `error: Option<String>` to the per-run chat row DTO
  (`SessionRunChatRecord` → the `/api/sessions/:id/runs` row), read from
  `runs[].error`.

### Frontend (`crates/rupu-cp/web/src/lib/api.ts`)
- `SessionSummary`: add `last_error?: string | null`.
- `SessionRunRow`: add `error?: string | null`.

### Render
- `SessionDetail` / `SessionConversation`: a **session-level error banner**
  (red, dismissible-not-required) shown when the session status is `failed` (or
  `last_error` present), reading `last_error`. And a **per-turn error** line on
  any run row whose `error` is set (small red text under that turn).

## B. Live-feel polish (frontend only; streaming already works)

In `crates/rupu-cp/web/src/pages/SessionDetail.tsx` (the poll loop) +
`components/session/SessionConversation.tsx` + `components/TranscriptPanel.tsx`:

- **Instant turn appearance:** after a successful `sendSessionMessage`, call the
  runs reload immediately (don't wait for the next poll tick) so the new turn
  shows and its `TranscriptPanel` starts streaming right away.
- **Adaptive refresh:** poll fast (~1500ms) while the session is active (status
  running / has `active_run_id`), slow (~5000ms) when idle. Replaces the flat
  5s interval. Implement by recomputing the interval from the latest
  session/runs state (effect re-subscribes when "active" changes).
- **Instant completion:** add an optional `onComplete?: () => void` prop to
  `TranscriptPanel`; when the live stream emits a `run_complete` / `run_failed`
  event, call it. `SessionConversation` passes a handler that triggers the runs
  reload, so final status / tokens / error update the moment the turn ends
  (rather than on the next poll). (RunDetail already keys off these event types;
  reuse the same `isKnownRunEvent`/type check.)
- **"Working…" pill:** in the session header, show a pulsing "working…"
  indicator while the session is active, mirroring `TranscriptPanel`'s existing
  live dot styling.

No backend changes for B — the transcript SSE + per-token flush already exist.

## C. Session e2e test

New `crates/rupu-cp/tests/sessions_live.rs` (mirror `tests/transcript.rs`'s
boot-axum + reqwest pattern, and `sse.rs` for the streaming read):
- Write on-disk fixtures under a tempdir `global`:
  - a session dir `sessions/<id>/session.json` for a **failed** session with
    `last_error` set and a `runs[]` entry whose `error` is set and whose
    `transcript_path` points to a real `.jsonl` containing a
    `run_complete { error }` event (+ an `assistant_message`).
- Boot `rupu_cp::server::router` on an ephemeral port; assert:
  - `GET /api/sessions/:id` → 200 and body `last_error` matches.
  - `GET /api/sessions/:id/runs` → the run row's `error` matches.
  - `GET /api/transcript?path=<turn>` → events include the run_complete.
  - `GET /api/transcript/stream?path=<turn>` → first SSE `data:` line parses to
    a transcript event (validates live wiring).
- Pure read-path e2e; no live LLM / worker.

## Testing
- Backend: the e2e above + a unit assertion that the session DTO mapping copies
  `last_error`/run `error` (if a pure mapping fn exists; else covered by e2e).
- web (vitest): SessionConversation/SessionDetail renders a `last_error` banner
  and a per-turn error; the adaptive-interval + onComplete-refresh logic
  (extract a pure `pollIntervalFor(session)` helper to unit-test:
  active→1500, idle→5000).

## Out of scope
- Live tool-call activity (`ToolUseStart` streaming) — rupu-agent change.
- Live thinking-token deltas — provider change.
- A dedicated session-state SSE (adaptive poll + onComplete covers the feel).
