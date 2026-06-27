# rupu-cp — Phase 2e: Full interactive session chat in the web — Design

**Date:** 2026-06-26
**Surfaces:** `rupu-cp` (one read endpoint) + `rupu-cp/web` (the chat)
**Status:** pending matt's spec review
**Builds on:** 2c (send endpoint + composer). Part of CP Phase 2 (Control).

## Goal
Make the web Session detail page a **full interactive conversation**, like the CLI's attached session: you see the prior turns of the session, type a message, and watch the agent's response stream inline (thinking → tool calls → tool results → assistant text), then type again. "Exactly like the CLI" — using the rich transcript rendering + SSE streaming we already ship.

## What already exists (reused, not rebuilt)
- `TranscriptPanel({ path, live })` — renders one run's transcript and **streams it live** over `/api/transcript/stream` (SSE). Full event coverage: assistant text + thinking, tool_call input, tool_result output/error/duration, file edits (diff), command runs (terminal), findings, per-turn usage, run status. (`components/transcript/*`, `transcriptView.ts`.)
- The send endpoint `POST /api/sessions/:id/send` → `{run_id}` + the composer (2c).
- Per-session runs via `getAgentRuns()` (each with `transcript_path`, `status`, `started_at`).

## The one backend gap + fix
The transcript JSONL has **no user-message event** — the user's prompt for a turn lives only in `SessionRunRecord.prompt` (`session.json`), which the CP doesn't expose. So we can't render the user's side of each turn today.

**Fix — `GET /api/sessions/:id/runs`** (new, read-only): returns the session's ordered turns:
```jsonc
[ { "run_id": "run_…", "prompt": "review the auth module",
    "transcript_path": "/…/<run_id>.jsonl", "status": "completed",
    "started_at": "…", "completed_at": "…|null",
    "tokens_in": 0, "tokens_out": 0, "tokens_cached": 0, "duration_ms": 0 } ]
```
`crates/rupu-cp/src/api/sessions.rs` already parses `session.json`; the existing `SessionForRunsDto` (`run_streams.rs`) reads `runs` but drops `prompt`. Add a small `SessionRunRow` projection that keeps `prompt`. Active-vs-completed comes from `status` / the session's `active_run_id`. (Metadata only — the heavy transcript bodies stay lazy via `TranscriptPanel`.)

## Frontend — the chat

### SessionDetail becomes the conversation (chat as primary view)
- **Compact header** (top): session id, agent · model, status dot, a usage chip. (No big chart up here.)
- **Conversation** (main, scrolls): the turns, newest at the bottom.
- **Composer** (pinned bottom): the 2c textarea + Send (⌘/Ctrl+Enter), disabled when the session is stopped or a send is in flight.
- The current **usage chart + identity `<dl>` + the old Turn-Runs link list** move into a collapsible **"Details"** disclosure (or a small secondary section) — kept, but out of the way.

### `SessionConversation` component (new — composition only)
Props: `{ session, runs, onActiveRunId? }`.
- Renders the **most recent ~10 turns expanded** + a **"Load older turns"** button that pages back (prepends the next batch). (Avoids fetching dozens of transcripts on open; matches the CLI's backfill-then-lazy behavior.)
- Each turn = a **user bubble** (`run.prompt`) followed by the **agent response** (`<TranscriptPanel path={run.transcript_path} live={isActive} embedded />`), where `isActive = run.run_id === session.active_run_id || run.status === 'running'`.
- **Auto-scroll** to the bottom when a new turn arrives / the active turn streams (respect a user scroll-up: don't yank them down if they've scrolled away).
- On **send**: the composer calls `sendSessionMessage(id, prompt)` → `{run_id}`; optimistically append a new turn (user bubble = the typed prompt, agent = a `TranscriptPanel live` for the new run's transcript path) so it streams instantly; reconcile with the next `getSessionRuns` refetch.

### `TranscriptPanel` — add an `embedded` mode (small)
In the chat, each turn's TranscriptPanel shouldn't repeat the full run header/footer chrome. Add an optional `embedded?: boolean` prop that hides the run-level header/footer (keeps the turn/tool rendering). Default `false` (existing RunTranscript/RunDetail usage unchanged).

### API client
`getSessionRuns(id): Promise<SessionRunRow[]>` → `GET /api/sessions/:id/runs`. The conversation polls it (or reuses the existing 5s session poll) to discover new turns; the active turn streams via TranscriptPanel SSE regardless.

## Data flow
```
GET /api/sessions/:id            → header (identity, status, active_run_id)
GET /api/sessions/:id/runs       → ordered turns [{run_id, prompt, transcript_path, status, …}]  (NEW)
  per turn → user bubble (prompt) + <TranscriptPanel path live={isActive} embedded/>
              live turn ⇒ /api/transcript/stream SSE
POST /api/sessions/:id/send {prompt} → {run_id}  (2c)  ⇒ optimistic new turn streams
```

## Files
**Backend:** `crates/rupu-cp/src/api/sessions.rs` — `GET /api/sessions/:id/runs` + `SessionRunRow`; route. (Maybe a tiny shared projection with `run_streams.rs`.)
**Frontend:** `lib/api.ts` (`getSessionRuns` + `SessionRunRow`), `components/session/SessionConversation.tsx` (new), `components/transcript/TranscriptPanel.tsx` (`embedded` prop), `pages/SessionDetail.tsx` (restructure to chat + Details disclosure).

## Testing
- Backend: `GET /api/sessions/:id/runs` returns the runs with `prompt` + `transcript_path` in order; missing session → 404. clippy clean.
- Frontend: `SessionConversation` renders a user bubble per turn + a TranscriptPanel per run; shows the most recent N + "Load older"; marks the active run `live`; send appends an optimistic streaming turn. `TranscriptPanel embedded` hides the header. Existing suite green; build strict; no `any`; recharts out of main chunk.
- Visual validation by matt: open a session → see the conversation → type → watch the response stream inline → type again; older turns load on demand.

## Non-goals / deferred (TODO)
- No char-by-char `AssistantDelta` typing animation (web streams at SSE-event granularity — close enough; the live usage chip already exists).
- No inline approval-gate handling inside the chat (separate from session turns).
- Session lifecycle (stop/archive/delete/start) stays a 2c follow-up.
- Virtualized/infinite history beyond "load older" batches — later if very long sessions need it.
