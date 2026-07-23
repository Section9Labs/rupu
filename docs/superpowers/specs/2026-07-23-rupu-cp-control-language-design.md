# rupu CP — One Control Language (design)

**Date:** 2026-07-23
**Status:** Approved (operator signed off on the visual spec artifact "rupu CP — One Control Language", incl. the single-line-label table rules)
**Scope:** `crates/rupu-cp/web` (SPA) + two small `rupu-cp` API data fixes. Baseline: main v0.63.0 (`ae353ef7`).

## 1. Problem

Every list view invents its own filter chrome and scaffolding. Audited on v0.59.2 (verified unchanged through v0.63.0 — the 0.60-0.63 releases touched only Flow Designer / Agent Builder):

- **≥6 segmented/pill dialects** across ~12 files (boxed segmented ×2 flavors, brand-filled rounded-md tabs ×3 copies, brand-filled rounded-full pills ×4 copies with inconsistent toggle semantics, ink-inverted pills, ring-lift toggles); zero shared Tailwind component.
- **~80 styled raw `<button>`s** despite a kit `Button`; the ring-pill Archive/Delete idiom copy-pasted in 3 files; ~8 inline brand-600 primaries.
- **4 inline copies** of the "This host" `<select>` + a duplicated `ALL_HOSTS` sentinel — while a shared `HostSelect.tsx` sits unused by list pages.
- **26 hand-rolled dashed empty-state boxes** in 22 files; 16 bare "Loading…" texts; 7 direct `Loader2` uses; a copy-pasted error banner.
- **~60 lines of fetch/paginate/poll scaffolding** duplicated per list page (WorkflowRuns, AgentRuns, AutoflowRuns, Sessions, ProjectRunsTab, ProjectSessionsTab); no shared hook.
- Tables themselves are already uniform (`SortableTable` everywhere; the one bespoke table is `ModelBreakdownTable`) — but column sizing is per-page and labels wrap.
- Two **data bugs** make the tables look broken regardless of styling (§7).

## 2. Goals / Non-Goals

**Goals**
- One control taxonomy — each control type has exactly one meaning and one style (§3).
- A shared kit in `components/ui/` (§4) + fixed FilterBar composition (§6) + table layout rules (§5), all token-only, both themes.
- Fix the two data bugs so the standardized tables actually show data (§7).
- Migrate every list page onto the kit (phases, §8).

**Non-Goals**
- No visual redesign of the CP's look (tokens, spacing, dark palette stay as-is — the kit codifies the *best existing* styles).
- No table-engine change (`SortableTable` stays; it gains column-sizing support).
- Flow Designer / Agent Builder internals: only Phase 4 (optional) ports the `.ab-*` parallel kit.
- No server-side pagination redesign (the `ListParams` wire shape stays).

## 3. The rule: each control answers exactly one question

| Control | Answers | Semantics | Replaces |
|---|---|---|---|
| **Segmented** | "Which view of this data?" | Exclusive, always one active. Boxed, joined, `p-0.5`, active = `bg-surface`. | Runs/Cycles/Claims · 7d/30d/All · PivotPicker · Dashboard ranges |
| **FilterPills** | "Show me a subset." | Rounded-full; active = brand fill; every group leads with **All**; single-select per group; optional tiny uppercase group label. | Lifecycle rounded-md tabs (3 copies) · trigger/scope pill rows (4 copies) · Situation-Room ink pills |
| **Tabs** (existing `TabBar`) | "Which section of this page?" | Unchanged visually; adopted where re-rolled (FileNavigator toggle, AutoflowRuns strip). | ring-lift one-offs |
| **Scope select** (`HostSelect`) | "Data from where?" | The existing shared component, adopted; one `ALL_HOSTS` sentinel exported from it. | 4 inline copies |
| **SearchInput** | "Find by text." | Icon-left input (FileNavigator's style promoted). | 3 bespoke search fields |

## 4. The kit (`components/ui/`)

New/changed primitives (token classes only; both themes; every one gets a co-located test):

- **`Segmented.tsx`** — `{ options: {value,label}[], value, onChange, size? }`. One style (boxed/joined). `PivotPicker` and `UsageRangeControls` become consumers.
- **`FilterPills.tsx`** — `{ label?, options, value, onChange }`; renders the All-first pill group. Value `'all'` is the neutral state.
- **`Button.tsx`** — gains variants `ring` (compact row action: `rounded px-2 py-0.5 text-note ring-1`, plus `ring-danger`) and `link` (text link button). Existing variants untouched.
- **`Select.tsx` / `Input.tsx` / `SearchInput.tsx`** — promote `HostSelect`'s select style and `settings/ConfigField.fieldCls` into ui/; `SearchInput` = icon-left (Code navigator style). `ConfigField` re-exports from ui to avoid churn in Settings.
- **`EmptyState.tsx`** — promote `EmptyTabState` out of `settings/`: `{ title, hint?, action? }`, dashed border, centered. 
- **`ErrorBanner.tsx`** — the `border-err/30 bg-err-bg` banner as a component.
- **Pill taxonomy** — `StatusPill` (lifecycle, dot+color, from `lib/status.ts`) and `Badge` (quiet metadata) are THE two pills. `Chip` becomes internal/base. `ScopeChip`'s hardcoded `indigo-*` classes replaced with tokens (dark-theme bug).
- **Loading policy** — `Spinner` (with label) or `Skeleton` only; no bare `Loader2`, no bare "Loading…" text in list/detail views.
- **`lib/usePagedList.ts`** — owns the duplicated state machine: `{ endpoint fetcher, filters }` → `{ rows, loading, error, hasMore, sentinelRef, refresh }`; page size 20, infinite-scroll sentinel, "— end of N —" footer state, 5s poll for active views (opt-in flag), reset on filter/host change. Consumes existing `ListParams`/`useInfiniteScroll`.
- Dedupe: single `shortId` (drop local copies), `ALL_HOSTS` exported from `HostSelect`.

## 5. Table layout rules (operator feedback — enforced in components, not per page)

1. **One flexible column per table** — the subject (workflow/agent/file/…). `SortableTable.Column` gains `fit?: boolean` (renders `width:1%; white-space:nowrap`) and the subject column truncates (`max-width:0` technique + ellipsis + `title` tooltip) instead of pushing.
2. **Labels are single-line by construction** — `white-space:nowrap` inside `StatusPill`/`Badge`/`SeverityChip`; the label lexicon lives in `lib/status.ts` and stays short (one word preferred, ≤12 chars: `RUNNING · COMPLETED · FAILED · AWAITING · CYCLE FAILED`). If a label would wrap, fix the word, not the column.
3. **Numbers/times: right-aligned, `tabular-nums`, fit-width** — token counts, cost, relative times never stretch a column. `Column` gains `align:'right'` + `fit` used together for these.

## 6. FilterBar recipe

Composition (a light `FilterBar.tsx` layout component with slots), fixed order on every list page:

```
[Segmented view] [FilterPills group(s)…] [--- spacer ---] [SearchInput?] [HostSelect]
```

- A page uses only the slots it needs; the ORDER never changes.
- One row, `flex flex-wrap items-center gap-2.5`.
- FilterPills semantics identical everywhere: All-first, single-select, brand-filled active.

## 7. Data fixes (ship in Phase 1 — verified root causes, v0.59.2 code unchanged at v0.63.0)

### 7a. Agent Runs — names + started_at (backend)
`GET /api/runs/agents` (`crates/rupu-cp/src/api/run_streams.rs`): the standalone branch hardcodes `agent: None` (~line 247) and `started_at: None` (~251) because `StandaloneMetaDto` only carries `run_id`/`session_id`/`trigger_source` — but the handler already opens each row's transcript for usage. Fix: read the transcript's first line (`run_start` event: `data.agent`, `data.started_at`) — via `rupu_transcript` (the exact working precedent is `aggregate.rs` ~139-148 feeding `UsageRunRow.agent`) — and fill `agent` + `started_at`. For `trigger_source == "session_turn"` rows, fall back to joining `session.json`'s `agent_name` via `session_id`. Standalone rows then also sort correctly (they currently float to the top with no date). Frontend needs no change (already renders `r.agent`).

### 7b. Autoflow events — failure reason + issue ref (backend + frontend)
`GET /api/runs/autoflows/events` (`run_streams.rs` ~161-175 DTO, ~730-742 mapping): `AutoflowCycleEvent` on disk carries `detail` (the error text) and `issue_ref`, both dropped by the DTO. Fix:
- DTO adds `detail: Option<String>`; `issue_display_ref` falls back to `issue_ref` (mirror `run_resolve.rs` ~483-486).
- Frontend (`pages/runs/AutoflowRuns.tsx`): rename the KIND column to **Event**; `cycle_failed` renders `CYCLE FAILED` (StatusPill lexicon) — visually distinct from run status; rows with `detail` get a `renderDetail` expandable showing the error (the Claims tab's `last_error` pattern, ~line 539); for non-run events the run-shaped columns (Run/Status/tokens/Cost) render nothing rather than a wall of dashes (they're `fit` columns, so they collapse).

## 8. Rollout (each phase = shippable beta)

1. **Phase 1 — data fixes + the kit.** §7a/§7b; build Segmented, FilterPills, FilterBar, Button variants, Select/Input/SearchInput, EmptyState, ErrorBanner, usePagedList, SortableTable `fit`/truncation support, pill nowrap + ScopeChip token fix. All tested standalone; nothing migrated yet (kit ships dark, unused = zero visual risk beyond the two data fixes).
2. **Phase 2 — the four run pages.** WorkflowRuns, AgentRuns, AutoflowRuns, Sessions onto FilterBar + usePagedList + table rules (the operator's screenshot pages).
3. **Phase 3 — the long tail.** ProjectRunsTab/ProjectSessionsTab, Hosts, Workflows, Agents, Projects, Findings chrome; loading/empty/error sweep (26 empty boxes, 16 "Loading…", 7 Loader2); Dashboard/Usage `[rgb(var(--c-*))]` → token classes; TabBar adoption (FileNavigator, AutoflowRuns strip).
4. **Phase 4 (optional) — deep cuts.** Agent Builder `.ab-*` CSS kit onto ui/ primitives.

## 9. Constraints & testing

- Tokens only (no color literals); both themes must read; no new npm/Rust deps; `#![deny(clippy::all)]` (rupu-cp has 2 pre-existing `host/ssh.rs` clippy errors — introduce none).
- The wire shapes stay: `ListParams` unchanged; §7 adds optional fields only (`detail`) — no breaking DTO changes.
- Tests: kit components (variant/active/nowrap assertions), usePagedList (paging, filter-reset, poll gating — mocked timers), 7a (standalone row gains agent+started_at from a fixture transcript; session_turn joins session.json), 7b (DTO forwards detail + issue_ref fallback; frontend renders expandable error).
- Per-phase operator gate: matt eyeballs light+dark before each merge (the approved artifact is the visual reference: https://claude.ai/code/artifact/37cea336-89a1-4555-805b-1f142d1fb676).
- Investigation baselines cite v0.59.2 line numbers; implementers re-verify against HEAD (confirmed identical at v0.63.0).
