# rupu CP — Table standardization (Phase 3 completion) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox syntax.

## Context

Operator (matt) review of the CP list pages found three classes of inconsistency, all traced to the
**unshipped Phase 3 ("the long tail")** of the approved *One Control Language* spec
(`docs/superpowers/specs/2026-07-23-rupu-cp-control-language-design.md`, §5 table rules / §6 FilterBar):

1. **Row clickability** — only 9 of ~21 `SortableTable` users pass `rowHref`; the rest navigate via a
   dedicated link column or a subject-cell link. `rowHref` ALREADY EXISTS and is well-built
   (`components/lists/SortableTable.tsx:98,216,241` — wraps each cell in a `<Link>`, so rows are real
   anchors, keyboard-accessible, no nested-anchor problem). **This is a pure adoption gap — do NOT add
   a new prop.**
2. **Column order/data drift** — WorkflowRuns/AgentRuns/AutoflowRuns broadly agree; **Sessions is the
   outlier** (Status first, Turns/Duration swapped, no Started, own status dot instead of `StatusPill`).
   Definition pages disagree on last-run signal: `WorkflowSummary.last_run` exists, `AgentSummary` has
   no equivalent.
3. **Status glyph** — Sessions animates nothing; the other three already spin `running` via
   `StatusPill`'s `Loader2`.

Operator decisions (binding): **row click on AgentRuns goes to the transcript / workflow view**;
standardize order + data "as much as possible".

**Why definition pages have no Status:** a definition (agent `.md` / workflow YAML) has no intrinsic
status — status belongs to a run. The correct shared signal for a definition row is **Last run**
(recency). `usage.rs:171` already computes `last_active` ("most-recent contributing run timestamp") in
the same rollup that produces `run_count`, so wiring it onto agents is thin.

## Global Constraints

- **Behavior parity**: same filters, same URLs/state semantics, same polling. This is a
  standardization pass, not a redesign.
- **Tokens only** — no new `--c-*` custom properties; both themes; every new/changed animation MUST
  have a live `@media (prefers-reduced-motion: reduce)` guard (house rule).
- **Conform to the approved spec's §5**, do not amend it: ONE flexible truncating `subject` column per
  table; `fit` on every number/time/label column; labels single-line ≤12 chars from the `lib/status.ts`
  lexicon.
- **Rust changes are additive only** (new optional fields; `#[serde(default)]`/`Option`), never a
  breaking wire change. No `unsafe`. Never run package-wide `cargo fmt` — per-file only.
- Tests: vitest + jsdom for web, `cargo test -p <crate>` for Rust. `npx tsc -b` + `npm run build` clean.
- jsdom cannot verify pixels — matt eyeballs before merge.

## The canonical standard (the yardstick for every task below)

**Run-like tables** — WorkflowRuns, AgentRuns, AutoflowRuns, Sessions:

| Slot | Column | Notes |
|---|---|---|
| 1 | **Status** | `StatusPill`, ALWAYS first. Animated glyph for live states. |
| 2 | **Subject** | the ONE `subject` (flexible, truncating) column: Workflow / Agent / Workflow / Agent |
| 3 | **ID** | `fit` mono `shortId(...)` — Run / Run / Run / Session |
| 4 | **Source** | `fit`. Page-specific HEADER kept (Trigger / Source / Event / Worker — genuinely different concepts), POSITION identical. |
| 5 | **Host** | `fit` mono |
| 6 | *Model* | Sessions only (justified: a session pins a model) |
| 7-10 | **In / Out / Cached / Cost** | `fit` right, `tabular-nums` |
| 11 | **Turns** | `fit` right |
| 12 | **Duration** | `fit` right |
| 13 | **Started** | `fit` right, `relativeTime` |
| 14 | *(actions)* | header `''`, buttons `stopPropagation` |

AutoflowRuns additionally keeps **Issue Ref** (slot 4b) — page-specific, justified.

**Definition tables** — Agents, Workflows, AutoflowsDefs:
`[Name subject] [Scope] [Trigger?] [Description?] [Runs] [Tokens] [Cost] [Last run] [(actions)]`

---

## Task 1: Rust — additive last-run + autoflow parity fields

**Files:** `crates/rupu-cp/src/api/agents.rs` (AgentDto), `crates/rupu-cp/src/usage.rs` (if
`UsageBreakdownRow` needs `last_active` surfaced), `crates/rupu-cp/src/api/run_streams.rs`
(`AutoflowEventRow`). Tests alongside each.

1. Add `AgentDto.last_run: Option<String>` populated from the same breakdown that already sets
   `run_count` (`agents.rs:274`). `usage.rs:171` computes `last_active`; surface it on
   `UsageBreakdownRow` if not already present. Mirror `WorkflowSummary.last_run` semantics exactly
   (same field name, same ISO-8601 string, `None` when the agent never ran).
2. **Investigate** whether `AutoflowEventRow` (`run_streams.rs:163`) can carry `turns` + `duration_ms`
   from its source data. **If the source data genuinely isn't available, DO NOT fabricate it** — stop,
   document precisely why in the report, and leave those columns blank on the web side (a fabricated
   0 is exactly the silent-wrong-number the dashboard spec §8 rejects). Report which it was.

- [ ] Failing tests first (agent list returns `last_run` for an agent with runs; `None` when never run).
- [ ] Implement; `cargo test -p rupu-cp`; `cargo clippy -p rupu-cp` clean.
- [ ] Commit `-m "feat(cp): additive last_run on agent summaries"`.

## Task 2: Shared animated status glyph + reduced-motion fix

**Files:** `web/src/components/ui/StatusPill.tsx` (or equivalent), `web/src/lib/sessionStatus.ts`,
`web/src/lib/status.ts`, `web/src/styles.css`; tests.

1. Give `StatusPill` a live-state animation shared by ALL pages: `running` = motion (reuse the existing
   token-driven pulse keyframe at `styles.css:202`, or the existing `Loader2` spin — pick ONE and use it
   everywhere), `awaiting` = attention pulse, terminal states static (`failed` loud, `completed`/`idle`
   quiet).
2. **Map Sessions' 4-value vocabulary** (`idle|running|failed|stopped`, per
   `rupu-cli/src/cmd/session.rs:213-220`) through the SAME `StatusPill` so all four pages share one
   status language. Keep the session words (don't rename to run words) — map the VISUAL, not the label.
3. **Bug fix:** `.rg-pulse-run`, `.rg-pulse-await`, `.rg-loop-spin` (`styles.css:210-246`) have NO
   `prefers-reduced-motion` guard. Add one. (Note: `.rg-march` edges are already covered by the
   `.react-flow__edge-path` guard at `styles.css:574` — verify before touching.)

- [ ] Failing tests: StatusPill renders the motion marker for running/awaiting and not for terminal
      states; a session `running` renders the same marker as a run `running`; reduced-motion rule present.
- [ ] Implement; `npx vitest run src/components/ui src/lib`; `tsc -b`.
- [ ] Commit `-m "feat(cp-web): shared animated status glyph + reduced-motion guards"`.

## Task 3: Sessions → canonical order + StatusPill

**Files:** `web/src/pages/Sessions.tsx`, `web/src/components/project/ProjectSessionsTab.tsx`; tests.

Reorder to the canonical run-like standard: Status first (via `StatusPill` from Task 2), then
Agent(subject) → Session id → Worker → Host → Model → In/Out/Cached/Cost → **Turns → Duration** (fix
the swap) → **Started** (new, from `created_at`, already on the wire) → actions. Apply `fit`/`subject`
per §5. Keep behavior/filters identical.

- [ ] Failing tests: column order matches canonical; Started renders `relativeTime(created_at)`;
      status cell uses StatusPill.
- [ ] Implement; run tests; `tsc -b`. Commit `-m "feat(cp-web): sessions adopt canonical run-table order"`.

## Task 4: Row-click adoption (`rowHref`)

**Files:** `web/src/pages/{Hosts,HostDetail,Sessions,Workflows,ProjectDefinitions}.tsx`,
`web/src/components/project/ProjectSessionsTab.tsx`, `web/src/pages/runs/AgentRuns.tsx`; tests.

1. Add `rowHref` to: Hosts, HostDetail, Sessions, ProjectSessionsTab, Workflows, ProjectDefinitions
   (all three sub-tables). Convert each inner subject/link-column `<Link>` to PLAIN TEXT and delete the
   now-redundant dedicated link column (nested anchors break — see `AutoflowRuns.tsx:186`).
2. Add `stopPropagation` to Hosts' Remove and Workflows' Run buttons BEFORE adopting `rowHref` there.
3. **AgentRuns (operator decision): row click → the transcript / workflow view.** Keep any competing
   session link as an explicit inner control with `stopPropagation`.
4. Leave genuinely non-navigable tables alone (expand-only / action-only / no detail route) — list them
   in the report with the reason.

- [ ] Failing tests per page: row renders as a link to the expected route; inner action buttons do NOT
      navigate.
- [ ] Implement; full `npx vitest run`; `tsc -b`. Commit `-m "feat(cp-web): whole-row navigation across list tables"`.

## Task 5: Definition pages — Last run + AutoflowsDefs kit adoption

**Files:** `web/src/pages/{Agents,Workflows,AutoflowsDefs}.tsx`, `web/src/lib/api.ts` (AgentSummary
type += `last_run`); tests.

1. Add `last_run` to the `AgentSummary` TS type and a **Last run** column to Agents (from Task 1's
   field), matching Workflows' existing rendering exactly (`relativeTime`, right, `fit`, em-dash when
   null).
2. Apply the definition-table canonical order to all three.
3. **AutoflowsDefs adopts the kit**: replace the bare `Loading…` with `Spinner`, the inline error div
   with `ErrorBanner`, add `EmptyState`, apply `fit`/`subject` per §5 (it never adopted the kit at all).

- [ ] Failing tests; implement; full suite; `tsc -b`.
- [ ] Commit `-m "feat(cp-web): definition tables share last-run signal and the kit"`.

## Task 6: Verify + PR
- [ ] `cargo test -p rupu-cp` + full `npx vitest run` + `npx tsc -b` + `npm run build`.
- [ ] Final whole-branch review (most capable model); fix Critical/Important.
- [ ] Draft PR summarizing the canonical standard, with an explicit in-browser check request.

## Out of scope
- Phase 4 (Agent Builder `.ab-*` kit port). Any redesign beyond ordering/parity. Renaming the
  session status vocabulary. Fabricating unavailable data.
