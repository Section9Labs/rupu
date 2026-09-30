# Agent Codenames — Plan 2 (Control Plane) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The rupu control plane (API + web UI) shows every run, session, agent instance and finding by its codename — crew chip with tint, role badge + leaf name, and agent · provider/model next to it — and its searches/filters match codenames.

**Architecture:** Plan 1 already stores codenames on `RunRecord`, `StepResultRecord`/items/findings, `UnitCheckpoint`, transcript `RunStart`, coverage `Attribution`, `SessionRecord`, and executor events (`StepStarted`/`UnitStarted`/`DispatchStarted`/`AgentStarted`). Records that `rupu-cp` passes through verbatim already carry them; this plan adds them to the hand-mapped DTOs, derives legacy names server-side (`codename_derived: true`), exports the palette to a generated, drift-tested TS module, and renders it in the React UI.

**Tech Stack:** Rust (axum, serde) in `crates/rupu-cp`; React 18 + TypeScript + Tailwind + `@xyflow/react` + Vitest 4 in `crates/rupu-cp/web`.

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-agent-codenames-design.md` (§6 legacy derive-on-read, §8 CP surfaces, §9 scale)

## Global Constraints

- Clients never compute names: the web only displays `codename` strings from DTOs; the only client-side table is the generated palette (tints + badges), keyed by word.
- DTOs carry `codename` and `codename_derived: bool` (true only when derived for a legacy record); derived names render muted.
- New serde fields on shared records stay `skip_serializing_if`/`default` (Plan 1 convention). New DTO-only fields may always serialize.
- `rupu-cp` depends on `rupu-codename` via `{ workspace = true }`.
- Any serde shape change consumed by fixtures → run `make macos-fixtures` and commit only `apps/rupu-macos/Fixtures/*.json` (macOS app is deprecated; no Swift edits).
- Web: tests colocated `*.test.ts(x)`, Vitest (`cd crates/rupu-cp/web && npx vitest run <path>`); DOM tests start with `// @vitest-environment jsdom`. Typecheck = `npm run build` (runs `tsc -b`). Never commit `web/dist`.
- Colors come from the generated palette (light/dark pair chosen by the resolved theme `mode` from `ThemeContext`), never hardcoded hex in components.
- Rust: never package-wide `cargo fmt` (main is fmt-dirty); clippy `-D warnings` with `-A clippy::question_mark` (pre-existing unrelated failure in rupu-cli `cmd/completers.rs:127`).
- Git: never `git stash`; never `git add -f` under `.superpowers/`; commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-cp/src/codename.rs` | `named(stored, id, agent) -> (String, bool)` derive-on-read helper + `inject_codename(&mut Value, …)` for pass-through JSON |
| `crates/rupu-cp/src/codename_palette.rs` | renders the TS palette module from `rupu_codename`; drift test |
| `crates/rupu-cp/web/src/lib/codenamePalette.gen.ts` | GENERATED — crew tints + role badges |
| `crates/rupu-cp/web/src/lib/codename.ts` | parse `crew`/`leaf`/last role; tint + badge lookup by theme |
| `crates/rupu-cp/web/src/components/codename/{CrewChip,RoleBadge,AgentName}.tsx` | visual identity components |

---

### Task 1: API — codenames on every run/session/agent-run/finding DTO, derived for legacy

**Files:**
- Modify: `crates/rupu-cp/Cargo.toml` (`rupu-codename = { workspace = true }`)
- Create: `crates/rupu-cp/src/codename.rs`; Modify `crates/rupu-cp/src/lib.rs` (`pub mod codename;` or `mod`, matching neighbours)
- Modify: `crates/rupu-cp/src/api/runs.rs` (`RunListRow` :479, `From<&RunRecord>` :491, `query_run_detail` :585, list handlers :647/:754 that inject `host_id` into `serde_json::Value`)
- Modify: `crates/rupu-cp/src/api/sessions.rs` (`SessionDto` :33 and where it is serialized + `scope/usage/host_id` injected: `scan_session_dir` :173, `collect_sessions` :225, `get_session` :367)
- Modify: `crates/rupu-cp/src/api/run_streams.rs` (`AgentRunRow` :148, standalone rows :361 via `read_transcript_run_start` :232, session rows :474 via `SessionForRunsDto` :137)
- Modify: `crates/rupu-cp/src/api/findings.rs` (`FindingOut` :23, built in `collect_all_findings` :181)
- Modify: `crates/rupu-cp/src/api/graph.rs` (`merge_event_units` :183 — events-only units at :221–240 drop `UnitStarted.codename`)
- Modify: fixtures via `make macos-fixtures`

**Interfaces:**
- Produces (`crate::codename`):
  ```rust
  /// Stored codename, or a derived one for a legacy record. Returns (name, derived).
  pub fn named(stored: Option<&str>, id: &str, agent: Option<&str>) -> (String, bool)
  /// For pass-through JSON objects: if `obj[key_codename]` is absent/null, set it to
  /// `derive_legacy(id, agent)` and set `codename_derived: true`; else set `codename_derived: false`.
  pub fn inject_codename(obj: &mut serde_json::Value, id: &str, agent: Option<&str>)
  ```
- JSON shapes consumers (Task 3) rely on:
  - run list row: `codename: string`, `codename_derived: bool` (crew only)
  - run detail: `run.codename: string`, `run.codename_derived: bool` (steps/items/findings keep their optional `codename` as stored)
  - session row + detail: `codename: string` (crew/role), `codename_derived: bool`
  - agent-run row: `codename: string` (standalone: from `RunStart.codename` else derive `(run_id, agent)`; session turn: the session's codename), `codename_derived: bool`
  - finding: `codename: string` (from `declared_by.codename` else derive `(declared_by.run_id, None)`), `codename_derived: bool`
  - graph `units[]`: events-only units now include `codename` when the `UnitStarted` event had one

- [ ] **Step 1: Failing tests** in `crates/rupu-cp/src/codename.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn stored_wins_else_derived() {
          assert_eq!(named(Some("cobalt-harbor"), "run_X", None), ("cobalt-harbor".into(), false));
          assert_eq!(named(None, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None), ("jade-reef".into(), true));
          assert_eq!(named(None, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", Some("triage")), ("jade-reef/numbat".into(), true));
      }
      #[test]
      fn inject_fills_only_missing() {
          let mut a = serde_json::json!({"id": "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"});
          inject_codename(&mut a, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", None);
          assert_eq!(a["codename"], "jade-reef");
          assert_eq!(a["codename_derived"], true);
          let mut b = serde_json::json!({"codename": "cobalt-harbor"});
          inject_codename(&mut b, "run_X", None);
          assert_eq!(b["codename"], "cobalt-harbor");
          assert_eq!(b["codename_derived"], false);
      }
  }
  ```
  Plus, in each touched API module's existing test module, one assertion per DTO: a legacy record (no codename) yields `codename_derived: true` with the derived name; a record with a stored codename yields it with `codename_derived: false`. For `graph.rs`, assert an events-only unit built from a `UnitStarted { codename: Some("jade-reef/hedgehog#1"), .. }` carries `"codename"`.

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-cp codename` → compile errors.

- [ ] **Step 3: Implement.**
  - `named`/`inject_codename` with `rupu_codename::derive_legacy`. `inject_codename` operates only on JSON objects (no-op otherwise).
  - `RunListRow`: add `pub codename: String, pub codename_derived: bool` filled via `named(r.codename.as_deref(), &r.id, None)` in `From<&RunRecord>`. Note: rupu-cli's `run list` emits this struct verbatim (SSH `list_runs` shells it) — additive, fine. In the list handlers that turn rows into `serde_json::Value` and inject `host_id`, also call `inject_codename(&mut v, id, None)` so rows from an OLDER remote rupu (no codename key) still get a derived one.
  - `query_run_detail`: after building the JSON, `inject_codename(&mut out["run"], &record.id, None)`.
  - `SessionDto`: add `#[serde(default)] codename: Option<String>`; at each place the DTO is serialized and `scope/usage/host_id` are injected, call `inject_codename(&mut v, &session_id, Some(&agent_name))` (remote SSH rows from `rupu session list --format json` already carry `codename` — `inject_codename` leaves them and marks `codename_derived: false`).
  - `AgentRunRow`: add `codename: String, codename_derived: bool`. Extend `read_transcript_run_start` to also return `RunStart.codename`; standalone rows use `named(start.codename, run_id, agent)`; session rows use the session's stored codename (add `#[serde(default)] codename: Option<String>` to `SessionForRunsDto`) via `named(.., session_id, Some(agent))`.
  - `FindingOut`: add `pub codename: String, pub codename_derived: bool`, filled in `collect_all_findings` from `record.declared_by.codename` / `named(.., &record.declared_by.run_id, None)`.
  - `graph.rs` events-only units: include `"codename": codename` in the hand-built JSON when `Some`.
  - `synthesize_unpersisted_run` (runs.rs ~:936): `codename: Some(rupu_codename::crew_for(&id))` instead of `None`.

- [ ] **Step 4: Run tests + fixtures** — `cargo test -p rupu-cp`; if fixture drift fails, `make macos-fixtures` then re-run; `cargo clippy -p rupu-cp --all-targets -- -D warnings`.

- [ ] **Step 5: Commit** — `feat(cp): codenames on run/session/agent-run/finding DTOs, derived for legacy`.

---

### Task 2: Generated TS palette with drift test

**Files:**
- Create: `crates/rupu-cp/src/codename_palette.rs` (+ module decl)
- Create (generated, committed): `crates/rupu-cp/web/src/lib/codenamePalette.gen.ts`
- Modify: `Makefile` — add `cp-codename-palette:` target: `REGEN_CODENAME_PALETTE=1 cargo test -p rupu-cp codename_palette_ts_is_current`

**Interfaces:**
- Consumes: `rupu_codename::{COLORS, ROLES, role_badge, Shape}`
- Produces (TS):
  ```ts
  // GENERATED by crates/rupu-cp/src/codename_palette.rs — do not edit.
  // Regenerate: make cp-codename-palette
  export type BadgeShape = 'circle' | 'triangle' | 'square' | 'diamond' | 'pentagon' | 'hexagon' | 'star' | 'cross' | 'ring' | 'chevron' | 'inv-triangle' | 'half';
  export interface Tint { light: string; dark: string }
  export const CREW_TINTS: Record<string, Tint> = { amber: { light: '#b45309', dark: '#fbbf24' }, /* …every COLORS entry… */ };
  export const ROLE_BADGES: Record<string, { shape: BadgeShape } & Tint> = { adder: { shape: 'hexagon', light: '…', dark: '…' }, /* …every ROLES entry… */ };
  ```
  `pub fn render_ts() -> String` in Rust (deterministic ordering = list order).

- [ ] **Step 1: Failing test** in `codename_palette.rs`:
  ```rust
  #[test]
  fn codename_palette_ts_is_current() {
      let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/src/lib/codenamePalette.gen.ts");
      let rendered = render_ts();
      if std::env::var_os("REGEN_CODENAME_PALETTE").is_some() {
          std::fs::write(&path, &rendered).expect("write palette");
          return;
      }
      let on_disk = std::fs::read_to_string(&path).expect("missing codenamePalette.gen.ts; run `make cp-codename-palette`");
      assert_eq!(on_disk, rendered, "codenamePalette.gen.ts drifted from rupu-codename; run `make cp-codename-palette`");
  }
  #[test]
  fn render_covers_every_word() {
      let ts = render_ts();
      for (name, _, _) in rupu_codename::COLORS { assert!(ts.contains(&format!("  {name}: {{"))); }
      for role in rupu_codename::ROLES { assert!(ts.contains(&format!("  {role}: {{"))); }
  }
  ```
- [ ] **Step 2:** run → fails (no `render_ts`).
- [ ] **Step 3:** implement `render_ts()` (shape → kebab string via an exhaustive `match` on `rupu_codename::Shape`), then `make cp-codename-palette` to write the file.
- [ ] **Step 4:** `cargo test -p rupu-cp codename_palette` passes; `cd crates/rupu-cp/web && npx tsc --noEmit -p .` (or `npm run build`) typechecks the generated file.
- [ ] **Step 5: Commit** — `feat(cp): generated codename palette TS module with drift test`.

---

### Task 3: Web core — types, `lib/codename.ts`, `CrewChip` / `RoleBadge` / `AgentName`

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts` — `RunRecord` (:84) `codename?: string; codename_derived?: boolean`; `StepResultRecord` (:116) `codename?: string`, findings entries `codename?: string`; `RunListRow` (:344), `AgentRunRow` (:573), `SessionSummary` (:1105), `FindingOut` (:1324): `codename: string; codename_derived: boolean`; `UnitCheckpoint` (:402) `codename?: string`; `StepStartedEvent` (:187) and `UnitStartedEvent` (:229) `codename?: string`; NEW `DispatchStartedEvent { type: 'dispatch_started'; run_id; sub_run_id; agent?; transcript_path; codename?; provider?; model? }` and `AgentStartedEvent { type: 'agent_started'; run_id; step_id; unit_index?; codename?; agent; provider?; model?; agent_run_id; transcript_path }`, both added to the event union and `KNOWN_EVENT_TYPES` (:311).
- Modify: `crates/rupu-cp/web/src/lib/transcript.ts` — `run_start` data (:13) `codename?: string`; `TranscriptSummary` (:40) `codename?: string`.
- Create: `crates/rupu-cp/web/src/lib/codename.ts` (+ `codename.test.ts`)
- Create: `crates/rupu-cp/web/src/components/codename/CrewChip.tsx`, `RoleBadge.tsx`, `AgentName.tsx` (+ `codename.test.tsx`, jsdom)

**Interfaces:**
- Produces (`lib/codename.ts`):
  ```ts
  export interface ParsedCodename { crew: string; leaf: string; role?: string }
  /** crew = before '/', leaf = after '/' (or crew when none), role = last segment's word (strip #n, .a, trailing digits). */
  export function parseCodename(c: string): ParsedCodename
  export function crewTint(crew: string, mode: 'light' | 'dark'): string | undefined   // CREW_TINTS[crew.split('-')[0]]
  export function roleBadge(role: string, mode: 'light' | 'dark'): { shape: BadgeShape; color: string } | undefined
  /** `leaf · agent · provider/model`, omitting absent parts (mirrors the CLI's member_label). */
  export function memberLabel(codename: string | undefined, agent: string | undefined, provider?: string, model?: string): string
  ```
- Produces (components; all accept `derived?: boolean` → muted `opacity-60` + `title="derived for a run recorded before codenames"`):
  - `<CrewChip crew="cobalt-harbor" derived? />` — tint dot + mono name.
  - `<RoleBadge role="heron" size? />` — inline SVG of the badge shape filled with the role hue.
  - `<AgentName codename="cobalt-harbor/heron#412>lynx#3" agent? provider? model? showCrew? derived? />` — `RoleBadge` + leaf (+ `· agent · provider/model` when given); full codename in `title`; optional leading `CrewChip` when `showCrew`.
- Theme: read `mode` from `ThemeContext` (`components/theme/ThemeProvider.tsx`) with `useContext`; default `'light'` when the context is null (tests).

- [ ] **Step 1: Failing tests**
  `lib/codename.test.ts`:
  ```ts
  import { describe, expect, it } from 'vitest';
  import { parseCodename, crewTint, roleBadge, memberLabel } from './codename';
  describe('codename', () => {
    it('parses crew, leaf and role', () => {
      expect(parseCodename('cobalt-harbor')).toEqual({ crew: 'cobalt-harbor', leaf: 'cobalt-harbor', role: undefined });
      expect(parseCodename('cobalt-harbor/heron#412>lynx#3')).toEqual({ crew: 'cobalt-harbor', leaf: 'heron#412>lynx#3', role: 'lynx' });
      expect(parseCodename('cobalt-harbor/heron#4.2').role).toBe('heron');
    });
    it('looks up tints and badges by theme', () => {
      expect(crewTint('cobalt-harbor', 'light')).toBe('#1d4ed8');
      expect(crewTint('cobalt-harbor', 'dark')).toBe('#93b4fd');
      expect(crewTint('nope-harbor', 'light')).toBeUndefined();
      expect(roleBadge('heron', 'light')?.shape).toBeTypeOf('string');
    });
    it('builds member labels', () => {
      expect(memberLabel('jade-reef/heron#4', 'security-reviewer', 'anthropic', 'claude-opus-5-5'))
        .toBe('heron#4 · security-reviewer · anthropic/claude-opus-5-5');
      expect(memberLabel(undefined, 'triage')).toBe('triage');
    });
  });
  ```
  `components/codename/codename.test.tsx` (jsdom): render `<AgentName codename="jade-reef/heron#4" agent="security-reviewer" provider="anthropic" model="claude-opus-5-5" />` → text contains `heron#4 · security-reviewer · anthropic/claude-opus-5-5`, element with `title` = full codename, an `svg`; `<CrewChip crew="cobalt-harbor" derived />` has the muted class and the derived `title`.
- [ ] **Step 2:** `npx vitest run src/lib/codename.test.ts src/components/codename` → fails.
- [ ] **Step 3:** implement. Badge SVG shapes on a 12×12 viewBox (circle, triangle, square, diamond, pentagon, hexagon, star, cross, ring = stroked circle, chevron, inv-triangle, half = half-filled circle).
- [ ] **Step 4:** tests pass; `npm run build` typechecks.
- [ ] **Step 5: Commit** — `feat(cp-web): codename types, palette lookup, CrewChip/RoleBadge/AgentName`.

---

### Task 4: Web surfaces — Activity tables, run/session headers, filters, gate banners

**Files (display site → change):**
- `web/src/pages/runs/WorkflowRuns.tsx` — add a leading "name" column rendering `<CrewChip crew={r.codename} derived={r.codename_derived} />` (keep workflow + short id columns); filter (:131–138) also matches `r.codename`.
- `web/src/pages/runs/AgentRuns.tsx` — agent column (:451) becomes `<AgentName codename={r.codename} agent={r.agent} showCrew derived={r.codename_derived} />`; filter (:260–265) matches `r.codename`.
- `web/src/pages/Sessions.tsx` — agent column (:249–255) → `<AgentName codename={s.codename} agent={s.agent_name} model={s.model} showCrew derived={s.codename_derived} />`; filter (:84–89) matches `s.codename`.
- `web/src/components/project/ProjectRunsTab.tsx` — same as WorkflowRuns (:102/:108, filter :211–217).
- `web/src/pages/RunDetail.tsx` — header (:737/:746): `<CrewChip crew={run.codename} …/>` beside the workflow name; gate banners (:868 multi, :973 single) read `"<crew> is waiting for approval · <step>"` using `run.codename` when present.
- `web/src/pages/SessionDetail.tsx` — header (:219–225): `<AgentName codename={session.codename} agent={session.agent_name} model={session.model} showCrew />` next to the session id.
- Tests: extend each page's existing test (if one exists) or add a small jsdom test for `WorkflowRuns` + `RunDetail` header asserting the crew name renders and the filter matches by codename.

- [ ] **Step 1:** failing tests (filter by `"cobalt"` finds the row whose codename is `cobalt-harbor`; RunDetail renders `cobalt-harbor`).
- [ ] **Step 2:** run → fail. **Step 3:** implement. **Step 4:** `npx vitest run` (touched tests) + `npm run build`.
- [ ] **Step 5: Commit** — `feat(cp-web): codenames in activity tables, run/session headers and filters`.

---

### Task 5: Web — run graph, transcript, tool cards

**Files:**
- `web/src/lib/runGraphModel.ts` — `GraphNode` gains `codename?: string; provider?: string; model?: string`; `UnitView` gains `codename?: string; provider?: string; model?: string`. Sources: `step_results[].codename` and live `step_started.codename` → node; `units[].codename` (checkpoints) and live `unit_started.codename` → unit; live `agent_started` → set provider/model (and codename) on the node (`unit_index` absent) or unit (`unit_index` present) matching `step_id`.
- `web/src/components/graph/StepNode.tsx` (:76–79 agent chip) → `<AgentName codename={node.codename} agent={node.agent} provider={node.provider} model={node.model} />` when `node.codename`, else unchanged.
- `web/src/components/graph/FanoutNode.tsx` — header shows the role badge + role word + `×N` (N = unit count) when the node has a codename; unit titles (:142) show `#n` leaf via `AgentName` when `u.codename`.
- `web/src/components/graph/PanelLoopNode.tsx` — unit chips (:110/:119) use `AgentName` when `u.codename`.
- `web/src/components/RunGraph.tsx` — selection label (:114/:136) prefers the codename leaf.
- `web/src/components/run/StepTranscriptBrowser.tsx` — unit rows (:138–140) show `AgentName` when codename.
- `web/src/components/transcript/transcriptView.ts` — `TranscriptHeader` (:61) gains `codename?: string` from `run_start` (:342–349); `components/TranscriptPanel.tsx` (:189) renders `<AgentName codename agent provider model />` when present.
- `web/src/components/transcript/ToolCard.tsx` — `SubrunPayload` (:449) gains `codename?: string` parsed in `subrunPayloadFromRecord` (:464) from the tool output JSON's `codename`; `SubrunPayloadRow` (:502) shows `AgentName`.
- Tests: `runGraphModel` unit tests (existing test file for it, else new) — an `agent_started` event with `unit_index: 3` sets provider/model on unit 3; checkpoint codename flows to `UnitView.codename`. `transcriptView` test — `run_start.codename` lands in the header. `ToolCard` subrun payload test — `codename` parsed.

- [ ] Steps: failing tests → run → implement → `npx vitest run` touched + `npm run build` → commit `feat(cp-web): codenames + provider/model on run graph, transcript and sub-agent cards`.

---

### Task 6: Web — Live Events, roster, findings

**Files:**
- `web/src/lib/situationRoom/cards.ts` — `StreamCard` (:41) gains `codename?: string; crew?: string; provider?: string; model?: string`. `cardFromEvent` (:115): `step_started`/`unit_started` set `codename` (+ `crew` = `parseCodename(c).crew`) from the event; NEW `agent_started` card (group same as `unit_started`'s) with title `memberLabel(codename, agent, provider, model)`; to avoid doubling cards at fan-out scale, `unit_started` returns no card **when it carries a codename** (new-era runs always follow it with `agent_started`); legacy `unit_started` (no codename) keeps its card. `dispatch_started` (if handled) sets codename/provider/model. Run-level cards (`run_started`, awaiting, completed) set `crew` when a `crewByRun` lookup is provided by the caller (EventStream/Events page already resolve workflow/project from `runId` at `pages/Events.tsx:305–308` — extend that resolver to also map `runId → run.codename` from the runs it already loads). `cardFromFinding` (:208) sets `codename` from `FindingOut.codename`.
- `web/src/components/situationRoom/EventCard.tsx` — left tint stripe from `crewTint(card.crew)`; agent line (:192) renders `<AgentName codename={card.codename} agent={card.agent} provider={card.provider} model={card.model} />` when `card.codename`, else unchanged; run link (:182–185) shows the crew name instead of the 8-char id slice when `card.crew`.
- `web/src/lib/situationRoom/filter.ts` — `haystack()` (:14) adds `c.codename, c.crew, c.provider, c.model`; update `filter.test.ts` with a codename query case.
- `web/src/lib/situationRoom/roster.ts` — `deriveActivity` (:34) action for `step_started`/`agent_started` uses the codename leaf when present: `` `${leaf} · ${agent}` ``.
- `web/src/components/findings/FindingsTable.tsx` — new "agent" column rendering `<AgentName codename={f.codename} showCrew derived={f.codename_derived} />`; `web/src/components/findings/FindingRow.tsx` (per-run) shows the same.
- Tests: `cards.test.ts` (existing or new) — `agent_started` → one card with member label; `unit_started` with codename → no card; legacy `unit_started` → card. `filter.test.ts` — query `"heron"` matches a card whose codename is `jade-reef/heron#4`. `FindingsTable` jsdom test — agent column renders codename.

- [ ] Steps: failing tests → run → implement → `npx vitest run` touched + `npm run build` → commit `feat(cp-web): codenames in Live Events, roster and findings`.

---

### Task 7: Build, docs, verification

**Files:** `CLAUDE.md` (rupu-cp bullet: one sentence on codename DTO fields + `codename_derived` + `make cp-codename-palette`; Read-first: add Plan 2 path), spec status → "Plan 1 + Plan 2 complete".

- [ ] **Step 1:** `cd crates/rupu-cp/web && npx vitest run` (full web suite) — all pass (report pre-existing failures separately, if any, by checking whether the failing test file is touched by this branch).
- [ ] **Step 2:** `cd crates/rupu-cp/web && npm run build` — typecheck + bundle clean. Do NOT commit `web/dist`.
- [ ] **Step 3:** `cargo test -p rupu-cp && cargo clippy -p rupu-cp --all-targets -- -D warnings`.
- [ ] **Step 4:** docs edits; commit `docs: agent codenames plan 2 (CP) complete`.
