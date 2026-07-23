# rupu CP — One Control Language — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement task-by-task. Checkbox (`- [ ]`) steps.

**Goal:** One control taxonomy across the CP (kit + FilterBar + table rules) and the two data fixes that make the run tables show real data.

**Architecture:** New primitives in `components/ui/` + a `usePagedList` hook; `SortableTable` gains `fit`/truncation column support; list pages migrate onto them. Two additive backend DTO fixes in `run_streams.rs`. Spec: `docs/superpowers/specs/2026-07-23-rupu-cp-control-language-design.md`. Visual reference (operator-approved): the "One Control Language" artifact.

**Tech Stack:** React/TS/Tailwind (token classes), vitest/@testing-library; Rust axum for §7 fixes.

## Global Constraints

- Tokens only — no color literals; BOTH themes must read. No new npm/Rust deps.
- `#![deny(clippy::all)]`; rupu-cp has exactly 2 pre-existing clippy errors in `host/ssh.rs` — introduce zero new findings in touched files.
- Wire compatibility: only ADD optional DTO fields; `ListParams` unchanged.
- **Table rules (enforced in components):** labels nowrap inside StatusPill/Badge/SeverityChip; one flexible truncating subject column per table, all label/number/time columns `fit` (width:1% + nowrap); numbers/times right-aligned `tabular-nums`.
- **FilterBar slot order is fixed:** Segmented view → FilterPills group(s) → spacer → SearchInput? → HostSelect.
- FilterPills semantics: All-first, single-select per group, brand-filled active.
- Investigation line numbers cite v0.59.2 (verified identical at v0.63.0 baseline `ae353ef7`); implementers re-verify against HEAD before editing.
- Commits end with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Branch `cp-control-language`. Never run package-wide `cargo fmt`; `rustfmt --edition 2021 <file>` per file.
- Each phase ends: full `npx vitest run` + `tsc --noEmit` + `npm run build` green; Rust phases also `cargo test -p rupu-cp` + clippy-delta zero. Operator light/dark gate before merge.

---

## Phase 1 — data fixes + the kit (two parallel workstreams, disjoint files)

### Task A: Agent Runs + Autoflow data fixes (backend + AutoflowRuns UI)

**Files:** Modify `crates/rupu-cp/src/api/run_streams.rs`; `crates/rupu-cp/web/src/lib/api.ts` (add `detail?` to the autoflow event row type); `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx`. Tests inline (Rust) + AutoflowRuns test.

**A1 — agent name + started_at (spec §7a):**
- In `collect_standalone_runs` (~run_streams.rs:188-260): for each meta, read the companion transcript's FIRST `run_start` line and fill `agent` + `started_at` (parse via `rupu_transcript`'s event types — the working precedent is `rupu-transcript/src/aggregate.rs` ~139-148; do NOT re-aggregate the whole file, read only the first event line, tolerating a missing/corrupt line → fields stay None). For `trigger_source == "session_turn"`, if agent still None, join `sessions/<session_id>/session.json`'s `agent_name` (the session branch already parses that shape, ~126-134). Cache session lookups per request (HashMap) — no per-row re-read of the same session.
- Rust tests: fixture meta + transcript with a `run_start` line → row has agent+started_at; corrupt/missing transcript → None (no panic); session_turn fallback joins session.json.
- Result check: rows no longer all-sort-to-top (started_at now present).

**A2 — autoflow failure detail + issue ref (spec §7b):**
- DTO `AutoflowEventRow` (~161-175): add `detail: Option<String>` (skip-if-none); mapping (~730-742): forward `rec.event.detail`, and `issue_display_ref: event.issue_display_ref.or(event.issue_ref)` (mirror `run_resolve.rs` ~483-486).
- `api.ts`: add `detail?: string | null` to the autoflow event row interface.
- `AutoflowRuns.tsx`: KIND column header → **Event**; `cycle_failed` label renders `CYCLE FAILED` via the status lexicon; rows with `detail` use SortableTable's existing `renderDetail` to expand the error text (mirror the Claims tab `last_error` rendering, ~539); non-run events render empty (not "—") in Run/Status/usage cells.
- Tests: Rust — DTO serializes detail + falls back issue_ref; web — a cycle_failed row with detail expands and shows the error text; issue ref renders from fallback.

### Task B: the kit (web only, all-new files + 3 small modifications)

**Files:** Create `components/ui/Segmented.tsx`, `ui/FilterPills.tsx`, `ui/FilterBar.tsx`, `ui/Select.tsx`, `ui/Input.tsx`, `ui/SearchInput.tsx`, `ui/EmptyState.tsx`, `ui/ErrorBanner.tsx`, `lib/usePagedList.ts` (+ co-located tests). Modify `ui/Button.tsx` (add `ring`, `ring-danger`, `link` variants), `components/lists/SortableTable.tsx` (Column gains `fit?: boolean`, `align?: 'right'`; subject truncation via `maxWidthZero` + ellipsis + title), `components/StatusPill.tsx` + `ui/Badge.tsx` + `coverage/SeverityChip.tsx` (add nowrap), `components/ScopeChip.tsx` (indigo → tokens), `components/HostSelect.tsx` (export `ALL_HOSTS`; restyle via ui/Select). `PivotPicker` + `UsageRangeControls` become `Segmented` consumers (visual parity).

**Prop contracts (later tasks rely on these EXACT shapes):**
```ts
Segmented:   { options: {value:string; label:string}[]; value:string; onChange:(v:string)=>void; size?:'sm'|'md'; ariaLabel?:string }
FilterPills: { label?:string; options:{value:string;label:string}[]; value:string; onChange:(v:string)=>void }  // caller includes {value:'all',label:'All'} first
FilterBar:   { view?:ReactNode; filters?:ReactNode; search?:ReactNode; scope?:ReactNode }                        // renders fixed order + spacer
EmptyState:  { title:string; hint?:string; action?:ReactNode }
ErrorBanner: { children:ReactNode }
usePagedList<T>: { fetch:(p:{offset:number;limit:number})=>Promise<T[]>; deps:unknown[]; poll?:boolean }
             -> { rows:T[]; loading:boolean; error:string|null; hasMore:boolean; sentinelRef:Ref; refresh:()=>void; ended:boolean }
```
- Styling per the approved artifact (boxed segmented `p-0.5` active `bg-surface`; pills rounded-full active brand fill; ring button `rounded px-2 py-0.5 text-note ring-1`; EmptyState dashed box).
- usePagedList: PAGE=20, appends via existing `useInfiniteScroll`, resets when `deps` change, 5s poll only when `poll`, exposes `ended` for "— end of N —".
- Tests: variant/active-state + nowrap assertions per component; SortableTable fit/truncate (fit col has nowrap style; subject cell gets title attr); usePagedList with fake timers (page append, deps reset, poll gating).
- NOTHING migrates to the kit in this task except `PivotPicker`/`UsageRangeControls`/`HostSelect` internal restyles (their consumers' props unchanged) — zero page-level visual risk.

- [ ] Phase 1 exit: suites green (web + cargo), clippy delta zero, both workstreams reviewed (spec + quality), betas NOT yet cut (Phase 2 rides along unless operator wants 1 alone).

---

## Phase 2 — migrate the four run pages (sequential; each shares the kit)

For each page: replace bespoke filter chrome with `FilterBar` slots; replace the local fetch/poll/sentinel machine with `usePagedList`; apply table rules (`fit` on Run/Status/Source/host/tokens/cost/time columns; subject flexible+truncating); statuses/badges via StatusPill/Badge; empty/loading/error via EmptyState/Spinner/ErrorBanner; row actions via `Button ring`. Keep behavior parity (same filters, same URLs/state semantics, same polling). Delete the superseded local components/copies (local StatusBadge/SourceChip/shortId, inline host selects, ALL_HOSTS copies).

- [ ] **Task C:** `pages/runs/WorkflowRuns.tsx` (lifecycle pills + Archived pill; trigger pills; host scope; Archive/Delete → ring buttons)
- [ ] **Task D:** `pages/runs/AgentRuns.tsx` (lifecycle pills; host scope; agent col = subject) — lands on top of Task A's data
- [ ] **Task E:** `pages/runs/AutoflowRuns.tsx` (Runs/Cycles/Claims → Segmented; host scope; keeps Task A2's Event/detail rendering)
- [ ] **Task F:** `pages/Sessions.tsx` (Active/Archived pills; host scope; row actions → ring buttons)

Each task: co-located tests updated; page test asserts FilterBar order + fit columns; full suite + tsc + build green; per-task review.

- [ ] Phase 2 exit: operator light/dark gate on all four pages → merge → bump minor → beta.

---

## Phase 3 — the long tail (one structured sweep task per group)

- [ ] **Task G:** `ProjectRunsTab` + `ProjectSessionsTab` onto FilterPills/usePagedList/table rules (their pill rows are the 2 remaining copies).
- [ ] **Task H:** No-filter list pages onto table rules + EmptyState/Spinner (`Workflows`, `Agents`, `Projects`, `Hosts`, `Workers`, `Coverage*`); Findings keeps metric-tile filtering (intentional) but adopts EmptyState + fit columns.
- [ ] **Task I:** Loading/empty/error sweep — remaining 16 "Loading…", 7 raw Loader2, dashed boxes → kit; Dashboard/Usage/PivotPicker raw `[rgb(var(--c-*))]` classes → token utilities; TabBar adoption in FileNavigator + any residual re-rolls.
- [ ] Phase 3 exit: suites green; operator gate; merge + beta.

## Phase 4 (optional, separate follow-up) — Agent Builder `.ab-*` kit port. Not scheduled here.

## Self-review notes
- Spec §3-§6 → Task B (+C-F application); §5 table rules → B (SortableTable) + C-F; §7a/7b → Task A; §8 phases mirrored; §9 constraints in Global Constraints. Prop contracts pinned so Phase 2 tasks can be dispatched without re-reading Task B's diff. Line numbers are v0.59.2-cited; every task re-verifies at HEAD (confirmed unchanged at v0.63.0).
