# CP: Live Events history + Settings redesign + nav — Implementation Plan

> **For agentic workers:** subagent-driven-development. Steps use `- [ ]`.

**Goal:** Three CP changes matt asked for:
1. **Nav:** remove the now-single-item **Observe** group; make **Live Events** a top-level leaf directly under **Projects**.
2. **Settings UI redesign:** the config edit UI (`Settings.tsx` + `ConfigEditor.tsx`) looks raw/poor — give it a proper frontend-design pass (polished form + raw editors), via a design-focused agent.
3. **Live Events history + global-view redesign:** the page is a live-firehose-only view (empty when idle). Add a historical events endpoint that **aggregates recent events from existing per-run `events.jsonl`** (no new persistent store — approved), load history on mount + stream live on top, and redesign the timeline adopting **Okesu's `EventTimeline` UX** (rolling-window grouping of repeated events, filter, lazy-load via a `before_ts` cursor).

## Global Constraints
- #3 is **aggregate-recent** (approved): NO new persistent event store. Reuse per-run `events.jsonl` as the source; bound the scan (recent runs / a cap) + cache; cursor via `before_ts` (newest-first). Live SSE stream unchanged (append on top of history).
- Reference **Okesu** for #3 design (READ-ONLY): `/Users/matt/Code/Oracle/Okesu/web/src/components/EventTimeline.tsx` (grouping/filter/lazy-load UX), `controlplane/api/events.go` (`GET /api/events?limit&before_ts` shape), `controlplane/ports/eventstore.go` (`Recent(limit, beforeTs)`). Adapt the UX; do NOT port the persistent store.
- #2 + #3-frontend are **design-quality** work — use the **frontend-design** skill; make it distinctive and polished, theme-aware (the CP is light+dark), consistent with the existing CP design language (Tailwind tokens `bg-panel`/`border-border`/`text-ink`, existing components). matt validates the CP UI at runtime before merge.
- `#![deny(clippy::all)]`; no `unsafe`; `ApiError`/`thiserror`; workspace deps only. Per-file rustfmt (never lib.rs/mod.rs; `--skip-children` not in rustfmt 1.9.0 → hand-format). Web: `cd crates/rupu-cp/web && npm test && npx tsc --noEmit && npm run build`. Pre-existing `ssh.rs` 1.95 clippy unrelated.

## Grounded shapes (verified)
- Nav: `crates/rupu-cp/web/src/lib/sidebarNav.ts` — leaves Dashboard(/dashboard), Projects(/projects), a divider, then groups; **Observe** group now = only Live Events (`/events`, icon `Radio`). Coverage/Findings already moved to Security (prior PR). Routes in App.tsx unchanged.
- Settings: `crates/rupu-cp/web/src/pages/Settings.tsx` (577), `crates/rupu-cp/web/src/components/ConfigEditor.tsx` (712) — form tabs (General/Providers/Autoflow/SCM/Pricing/CP/Policy) + a Raw TOML tab; writes via the config API. Shared `ScopeChip`, `ui/Button`, etc. exist.
- Events: `crates/rupu-cp/web/src/pages/Events.tsx` — subscribes to `/api/events/stream` (SSE firehose), renders `EventTimelineList` newest-first; types `SeqEvent`/`RunEvent` (lib/api.ts + `components/RunEventFeed`). Backend `crates/rupu-cp/src/api/events.rs` — only `GET /api/events/stream` (SSE; case 3 = merged live firehose across all runs). `RunStore` (`s.run_store`) lists runs; each run's `events.jsonl` is read via the run store (`read`/tail helpers used elsewhere). `RunStore::list()` gives recent runs (sortable by started/finished).

---

## Task 1: Nav — Live Events top-level leaf (web)

**Files:** Modify `crates/rupu-cp/web/src/lib/sidebarNav.ts`; Test: `sidebarNav.test.ts`.

- [ ] **Step 1: Failing/assert test** — `sidebarNav` has a top-level `leaf` for Live Events (`/events`) positioned right after the Projects leaf (before the divider/groups); there is NO `observe` group anymore.
- [ ] **Step 2:** Implement — remove the `observe` group entirely; add `{ kind: 'leaf', item: { to: '/events', label: 'Live Events', icon: Radio, enabled: true } }` immediately after the Projects leaf (line ~46). Keep the divider + other groups. Update `GroupID` type if `'observe'` was a member. If CommandPalette references the observe group, it's independent (groups by kind) — leave it.
- [ ] **Step 3:** `npm test && npx tsc --noEmit && npm run build` clean; commit `feat(cp-web): Live Events as a top-level nav item; drop the Observe group`.

## Task 2: Recent-events aggregation endpoint (backend)

**Files:** Modify `crates/rupu-cp/src/api/events.rs` (+ route); Test: same file.

**Interfaces — Produces:** `GET /api/events?limit=<n>&before_ts=<unix-ms>` → a JSON array of recent events **newest-first**, cursor-paginated (only events strictly older than `before_ts` when given), aggregated across recent runs' `events.jsonl`. Each event row carries what the timeline needs (kind/type, run_id, timestamp, + the event payload), matching the SSE frame shape so the frontend can merge them uniformly.

- [ ] **Step 1: Failing tests** (tempdir global with a few runs' `events.jsonl` of differing timestamps): `recent_events_returns_newest_first_limited`; `recent_events_before_ts_paginates` (only older-than cursor); `recent_events_empty_when_no_runs`; `recent_events_bounded` (scan is capped — doesn't read unbounded history).
- [ ] **Step 2:** `cargo test -p rupu-cp --lib -- events` → FAIL.
- [ ] **Step 3:** Add `GET /api/events` handler: enumerate recent runs via `s.run_store.list()` (bounded — e.g. the most-recent K runs by started/finished, or stop once `limit` events collected walking newest runs first), read each run's `events.jsonl` (reuse the run store's event-read helper; tolerate missing/garbled), tag each event with its `run_id`, merge, sort **newest-first by timestamp**, apply `before_ts` filter + `limit`. Keep it bounded + reasonably cheap (cap runs scanned; document the bound); a short cache is optional. Return the array in the same wire shape as the SSE frames (so the frontend merges history + live uniformly).
- [ ] **Step 4:** tests pass; `cargo test -p rupu-cp --lib` green.
- [ ] **Step 5:** rustfmt events.rs; clippy `-p rupu-cp --no-deps`; commit `feat(cp): GET /api/events — recent events aggregated from run events.jsonl (cursor-paginated)`.

## Task 3: Live Events — load history + Okesu-style timeline redesign (web, design)

**Files:** Modify `crates/rupu-cp/web/src/pages/Events.tsx`, `components/EventTimelineList.tsx` (or a new `EventTimeline.tsx`), `lib/api.ts`; Test: the touched components' tests.

**Interfaces — Consumes:** Task 2's `GET /api/events?limit&before_ts` + the existing SSE stream.

- [ ] **Step 1:** USE the **frontend-design** skill. READ Okesu's `web/src/components/EventTimeline.tsx` for the UX (rolling-window grouping of repeated same-(type[,run]) events, lifecycle events that never group, a text filter, a memory ceiling + lazy-load older via the cursor). READ the current `Events.tsx`/`EventTimelineList.tsx` to preserve the follow-pin/connection-badge behavior.
- [ ] **Step 2: Failing vitest** — on mount the page calls `getEvents(limit)` and renders the historical events (so an idle page is NOT empty); the live SSE stream prepends new events on top; scrolling to the bottom lazy-loads older via `before_ts`; repeated events group; a filter narrows rows.
- [ ] **Step 3: Implement.** `api.ts`: `getEvents(limit?, beforeTs?): Promise<RunEvent[]>` → `/api/events`. `Events.tsx`: on mount load history (newest-first), keep the live SSE subscription appending new events at top, add lazy-load-older (infinite scroll / "load more") via `before_ts`. Redesign the timeline as a **polished global feed** (Okesu-style): grouped repeated events, a filter box, per-row run link (`/runs/:runId`), event-type styling, timestamps, theme-aware, matching the CP design language. Keep connection badge + follow behavior.
- [ ] **Step 4:** `npm test && npx tsc --noEmit && npm run build` clean; commit `feat(cp-web): Live Events loads history + Okesu-style grouped timeline (global view)`.

## Task 4: Settings edit UI redesign (web, design)

**Files:** Modify `crates/rupu-cp/web/src/pages/Settings.tsx`, `components/ConfigEditor.tsx` (+ small extracted components if it helps); Test: `Settings.test.tsx`.

- [ ] **Step 1:** USE the **frontend-design** skill. READ the current `Settings.tsx` + `ConfigEditor.tsx` (the form tabs + raw TOML editor + provenance/lock badges + secret masking) and the CP design language (existing polished pages like RunDetail/Projects for reference). The brief: the edit UI "looks very raw and poor" — make it **polished, clear, and consistent** with the rest of the CP: well-structured field groups, good spacing/labels/help text, clear provenance/lock affordances, a clean raw-editor, theme-aware. Preserve ALL functionality (form↔raw, validation, per-field provenance + 🔒 lock, secret masking as `set`/not-configured, launcher-gating messages, save/backup semantics) — this is a visual/UX redesign, not a behavior change.
- [ ] **Step 2: Failing/updated vitest** — keep the existing Settings tests green (behavior unchanged); add/adjust tests for any restructured markup (assert the key controls still render + save still calls the config API + validation errors still surface).
- [ ] **Step 3: Implement** the redesign. Do NOT change the config API contract or write semantics; keep provenance/lock/secret behavior. Improve layout, typography, spacing, grouping, affordances; extract small components if ConfigEditor is unwieldy.
- [ ] **Step 4:** `npm test && npx tsc --noEmit && npm run build` clean; commit `feat(cp-web): redesign the Settings config-edit UI (polished form + raw editors)`.

---

## Self-Review
Coverage: nav (T1); recent-events endpoint (T2) + Live Events history/redesign (T3); Settings redesign (T4). #3 aggregate-recent honored (no store). Design tasks (T3/T4) use frontend-design + Okesu reference; behavior preserved (T4) / additive (T3). Type flow: T2 `GET /api/events` → T3 `getEvents`. Parallelizable: T1 (nav) + T2 (backend) + T4 (settings) disjoint; T3 after T2.

## Execution
Subagent-driven. Parallel wave: T1 (nav) + T2 (backend events) + T4 (settings redesign, frontend-design) — disjoint. Then T3 (events frontend, needs T2, frontend-design). Review each; final whole-branch review; one PR to main (no self-merge; matt validates: Live Events under Projects with history + polished timeline, and the redesigned Settings).
