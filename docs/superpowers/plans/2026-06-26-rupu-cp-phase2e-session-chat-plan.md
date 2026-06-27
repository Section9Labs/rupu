# rupu-cp Phase 2e — Session chat — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Make the Session detail page a full interactive chat: prior turns visible, type → watch the agent stream inline → type again. Mostly assembling existing pieces (`TranscriptPanel` + SSE + the 2c composer) + one backend endpoint exposing per-turn prompts.

**Spec:** `docs/superpowers/specs/2026-06-26-rupu-cp-phase2e-session-chat-design.md`

**Constraints:** no `any` (TS); static Tailwind; recharts out of main chunk; stage only specific files; never `-A`/`.rupu/*`; never package-wide `cargo fmt`. rupu-cp/web are clean gates on the worktree's 1.95.

---

### Task 1: Backend — `GET /api/sessions/:id/runs`

**Files:** Modify `crates/rupu-cp/src/api/sessions.rs`.

**Context:** `sessions.rs` parses `session.json` into a minimal `SessionDto` (excludes `message_history`). The session's turns live in `runs: Vec<SessionRunRecord>` where each record has `run_id, prompt, transcript_path, started_at, completed_at?, status?, total_tokens_in/out/cached, duration_ms`. `run_streams.rs` has a `SessionForRunsDto` that already parses `runs` but DROPS `prompt`. The existing `get_session` resolves the dir (active `<global>/sessions/:id` then `sessions-archive`).

- [ ] **Step 1: Write failing test.** Write a `session.json` (tempdir) with a `runs` array of 2 turns (distinct `prompt`/`run_id`/`transcript_path`); the handler/projection returns them in order with `prompt` + `transcript_path` + `status` preserved; a missing session → 404.
- [ ] **Step 2:** Run `cargo test -p rupu-cp`, confirm failure.
- [ ] **Step 3: Implement.**
  - A `#[derive(Serialize)] struct SessionRunRow { run_id: String, prompt: String, transcript_path: String, status: Option<String>, started_at: Option<String>, completed_at: Option<String>, tokens_in: u64, tokens_out: u64, tokens_cached: u64, duration_ms: u64 }` (adjust field names to the `SessionRunRecord` serde — read it; map `total_tokens_*` → `tokens_*`). Status serialized lowercase if it's an enum.
  - A projection `SessionRunsDto` (or reuse a `#[serde(default)] runs: Vec<RawRun>` with the fields incl. `prompt`) to parse `session.json`'s `runs`.
  - `async fn get_session_runs(State(s), Path(id)) -> ApiResult<Json<Vec<SessionRunRow>>>`: resolve the session dir (active → archive, same as `get_session`); read `session.json`; map `runs` → `Vec<SessionRunRow>` (ordered as stored); 404 if the session dir/`session.json` is absent. Tolerate a missing `runs` key → `[]`.
  - Register `.route("/api/sessions/:id/runs", get(get_session_runs))`.
- [ ] **Step 4:** `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` green/clean.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/src/api/sessions.rs` → `feat(cp): GET /api/sessions/:id/runs (per-turn prompt + transcript)`.

---

### Task 2: Frontend enablers — `TranscriptPanel embedded` + `getSessionRuns`

**Files:** Modify `crates/rupu-cp/web/src/components/transcript/TranscriptPanel.tsx`, `crates/rupu-cp/web/src/lib/api.ts`.

- [ ] **Step 1: `TranscriptPanel` `embedded` prop.** Read `TranscriptPanel.tsx` — it renders a run header + the Turn list + a footer (usage). Add `embedded?: boolean` (default `false`). When `embedded`, hide the run-level header + footer chrome, keeping the turn/tool rendering (the agent's response body). Existing callers (RunTranscript, RunDetail) keep the default (full chrome). Keep the live SSE behavior intact in both modes.
- [ ] **Step 2: `api.ts`.** Add `interface SessionRunRow { run_id: string; prompt: string; transcript_path: string; status?: string | null; started_at?: string | null; completed_at?: string | null; tokens_in: number; tokens_out: number; tokens_cached: number; duration_ms: number }` and `getSessionRuns(id: string): Promise<SessionRunRow[]>` → `GET /api/sessions/${id}/runs`. No `any`.
- [ ] **Step 3:** A focused test for the `embedded` mode (renders the turn body but not the run header) if TranscriptPanel is testable in isolation; else cover it via the SessionConversation test in Task 3. `npm test -- --run` + `npm run build` green.
- [ ] **Step 4: Commit.** `git add crates/rupu-cp/web/src/components/transcript/TranscriptPanel.tsx crates/rupu-cp/web/src/lib/api.ts <test?>` → `feat(cp/web): TranscriptPanel embedded mode + getSessionRuns client`.

---

### Task 3: Frontend — `SessionConversation` + SessionDetail chat restructure

**Files:** Create `crates/rupu-cp/web/src/components/session/SessionConversation.tsx`; Modify `crates/rupu-cp/web/src/pages/SessionDetail.tsx`; Test.

**Context:** SessionDetail (post-2c) has identity header + usage chart + `<dl>` + Turn-Runs link list + the composer. We restructure it so the conversation is the main view.

- [ ] **Step 1: `SessionConversation.tsx`** — props `{ session, runs, onSent? }` (session record for `active_run_id`/`status`; `runs: SessionRunRow[]` ordered oldest→newest):
  - Render the **most recent `VISIBLE_TURNS = 10`** turns; a **"Load older turns"** button at the top reveals the previous batch (increment the visible count). 
  - Each turn: a **user bubble** (`run.prompt`, right-aligned/distinct styling) then the **agent response** `<TranscriptPanel path={run.transcript_path} live={isActive(run)} embedded />` where `isActive = run.run_id === session.active_run_id || run.status === 'running'`.
  - **Auto-scroll:** a bottom ref; scroll into view on mount + when the newest turn changes, BUT only if the user is already near the bottom (track scroll position; don't yank them up-scrolled).
  - No `any`; static Tailwind (chat bubble styling — neutral user bubble, agent response full-width).
- [ ] **Step 2: SessionDetail restructure.**
  - Fetch `getSessionRuns(id)` (poll ~5s, like the existing turn-runs poll — replace the `getAgentRuns` turn list with `getSessionRuns`).
  - Layout: **compact header** (session id + agent·model + status dot + `UsageChip`) → `<SessionConversation session={session} runs={runs} />` (main, grows/scrolls) → the **composer** (move the existing 2c textarea+Send to pin at the bottom; on send, after `sendSessionMessage` resolves with `{run_id}`, optimistically pass the new run (run_id + prompt + a derived transcript path if known, else just refetch `getSessionRuns` immediately) so the new turn appears + streams).
  - Move the **usage chart + identity `<dl>` + (optional) old turn list** into a collapsible **`<details>`/disclosure "Session details"** above or below the chat — kept but secondary.
  - Disable the composer when the session is stopped (existing logic).
  - Optimistic send: simplest robust approach — on send success, immediately `getSessionRuns()` refetch (the new run is in `session.runs` synchronously after `session send` enqueues), so the new turn renders + its TranscriptPanel streams. (If a brief gap, the 5s poll covers it.)
- [ ] **Step 3: Test** (`SessionConversation.test.tsx` + extend `SessionDetail.test.tsx`): SessionConversation with 12 runs renders 10 + a "Load older" button; clicking it reveals more; a run matching `active_run_id` passes `live` to a (mocked) TranscriptPanel; a user bubble shows `run.prompt`. SessionDetail still sends via the composer. Mock `TranscriptPanel` + the api.
- [ ] **Step 4:** `npm test -- --run` + `npm run build` (strict) green; `grep -c recharts dist/assets/index-*.js` → 0.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/components/session/SessionConversation.tsx crates/rupu-cp/web/src/pages/SessionDetail.tsx <tests>` → `feat(cp/web): interactive session chat (conversation + composer)`.

---

### Final verification
- `cargo test -p rupu-cp` green; clippy clean. `npm test -- --run` green; `npm run build` strict; recharts out of main chunk.
- Final review (the runs endpoint shape; the conversation assembly; embedded TranscriptPanel; live streaming of the active turn; auto-scroll; optimistic send), then matt visual-validates the chat end-to-end.
