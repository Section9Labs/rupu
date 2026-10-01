# Dashboard Live View — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single-column live view with a three-pane dashboard TUI — a rail-rendered **structure** pane (the DAG), the selection-following **stream** pane, and the always-live **firehose** pane — reusing the Plan 1–3 plumbing and extending `rupu-app-canvas`'s DAG walk for split/join/branch/loop.

**Architecture:** Two halves. (A) Extend `rupu-app-canvas` (a CLI-only crate) so its `render_rows(&Workflow, status_lookup)` emits split/join/branch/loop structure, not just linear/run/action/gate/parallel/for_each/panel. (B) In `rupu-cli`, add `output/live_view/structure.rs` (maps app-canvas `GraphRow`s → `Line`s with live overlays + per-kind glyphs + collapse/phase/drill), `output/live_view/panes.rs` (composes header + structure | stream/firehose + footer, with a narrow-terminal fallback), extend `NavState` with pane focus, and rewire `run_live_view` to build the pane frame instead of `live_layout` (which, with the flat `graph`, is removed). `RunView`, `TranscriptMux`, `LiveRenderer`, `row.rs` are unchanged.

**Tech Stack:** Rust 2021, `rupu-app-canvas` + `rupu-cli`. `insta` snapshots, `crossterm` (events only — already used). Reuses `rupu_orchestrator::{Workflow, Step, StepKind, is_approval_gate, loop_of_step, loop_internal_edges}`, Plan-1 `RunView`, Plan-2 `row`/`nav`, Plan-3 `mux`/`render`.

**Spec:** `docs/superpowers/specs/2026-10-01-rupu-cli-dashboard-live-view-design.md`

**Depends on / branch:** Builds on the Plan 1–3 plumbing. Lands on `claude/workflow-live-view-plan-3` (PR #695 is held open; this rework stacks on it so #695's plumbing + this design merge together — the "hold #695, fix the graph first" decision).

## Global Constraints

- Workspace deps only; never pin a version in a crate `Cargo.toml`.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- **CI lints with `cargo clippy --workspace --all-targets --locked -- -D warnings` on pinned 1.95; the worktree runs 1.97 and cannot verify 1.95 lints.** Write lint-clean proactively (match guards, no sole-`if`-in-match-arm, no nested collapsible `if`, no match-on-bool, no `needless_return`, no manual clamp/min-max). The pre-existing `completers.rs:127` `question_mark` is a 1.97-only false alarm — ignore it. Lint is CI-arbitrated; expect a round-trip.
- **No mock features:** meters/counts/density shown only when truthfully available; empty stream/firehose → a `waiting for activity…` placeholder, never a fabricated line or zero-height panel.
- Per-file `rustfmt` only; never package-wide `cargo fmt`.
- The view restores the terminal on every exit path incl. panic (AltScreen RAII, already in `render.rs`).
- **`rupu-app-canvas` changes must keep its existing snapshot tests green** (add new `.snap`s for new kinds; don't break the ones the CP-era walk relies on). It is a CLI-only consumer, so extension is safe.
- GUI rule: `cargo test` green ≠ rendering green. A human tty smoke is the required pre-merge gate (Task 8).

## Reuse map (do NOT rebuild these)

`run_model.rs` (`RunView`), `live_view/row.rs` (`Line`/`Style`/`truncate_to`/`render_plain`), `live_view/mux.rs` (`TranscriptMux`, `project_event`, pinned buffer + firehose), `live_view/render.rs` (`LiveRenderer`, `style_ansi`, `AltScreen`) are UNCHANGED. `live_view/nav.rs` is EXTENDED (pane focus). `live_view/layout.rs` (`live_layout`, flat `graph`) is REMOVED; `fanout_block` is re-homed into `structure.rs`.

---

### Task 1: app-canvas — split + join rails

**Files:**
- Modify: `crates/rupu-app-canvas/src/git_graph.rs` (add dispatch arms + `emit_split_step`, `emit_join_step`)
- Test: `crates/rupu-app-canvas` inline/snapshot tests

**Interfaces:**
- Consumes: `rupu_orchestrator::Step` (`step.split: Option<Vec<String>>`, `step.join: Option<Join>` where `Join{ wait: JoinWait }`), `StepKind`.
- Produces: `render_rows` now routes a `step.split.is_some()` step to `emit_split_step` (a `◈`-marked node with its target ids as `┣`/`┗` lane labels) and a `step.join.is_some()` step to `emit_join_step` (a `◈ join ◄─` node whose `Meta` carries `wait:all|any|n`). Both use the existing `GraphCell::{Bullet,Branch(BranchGlyph::*),Label,Meta}` vocabulary; add a `GraphCell` or `Meta` convention for the split/join glyph if a bullet is insufficient (prefer a `Meta("split")`/`Meta("join · wait:all")` + `BranchGlyph::Top/Mid/Bot` for the fork, `BranchGlyph::Merge` for the join).

- [ ] **Step 1: Write the failing test** — a `Workflow` with a `split:` step (targets `[a,b,c]`) and a `join:` step; call `render_rows(&wf, |_| NodeStatus::Waiting)`; assert the split row set contains the target labels with branch glyphs and the join row carries `join` + `wait:…`. (Use app-canvas's existing test helper for building a `Workflow` / `GraphRow` assertions — grep its tests.)
- [ ] **Step 2: Run to verify it fails** — `cargo test -p rupu-app-canvas 2>&1 | tail` → FAIL (split/join currently fall through to `emit_linear_step`, so no target lanes / no `join` meta).
- [ ] **Step 3: Implement** the two dispatch arms (before the `for_each`/linear fallthrough in `render_rows`, ~git_graph.rs:110-125) + `emit_split_step` / `emit_join_step`, modeled on `emit_parallel_step` (which already nests sub-rows with `BranchGlyph`). Keep the anchor (`anchor_step_id`/`anchor_status`) on the split/join node so the CLI can overlay + select it.
- [ ] **Step 4: Run tests** — new tests pass; ALL existing `rupu-app-canvas` tests still pass.
- [ ] **Step 5: Commit** — `feat(app-canvas): render split and join nodes with fork/merge rails`.

---

### Task 2: app-canvas — branch arms + loop framing

**Files:**
- Modify: `crates/rupu-app-canvas/src/git_graph.rs` (`emit_branch_step`; loop framing in `render_rows`)
- Test: app-canvas snapshot tests

**Interfaces:**
- Consumes: `step.branch: Option<Branch>` (`Branch{ condition: String, then: Vec<String>, r#else: Vec<String> }`); `rupu_orchestrator::{loop_of_step, loop_internal_edges}` + `Workflow.loops: BTreeMap<String, LoopDef>`.
- Produces: a branch step → a `◇`-marked node with `Meta("branch · when: <condition>")` and two arm groups (`▶ then → <ids>`, `⊘ else → <ids>`). A step that is a member of a loop (`loop_of_step(wf, step_id).is_some()`) is rendered inside a `↻ loop:<name>` framing: a loop header row before the first member, the members nested one indent under a loop gutter, and a `↺` loop-back row after the last member. (Use `loop_internal_edges`/`loop_of_step` to group members; keep it a pure structural grouping over `wf.steps` order.)

- [ ] **Step 1: Write the failing test** — a `Workflow` with a `branch:` step and a `loops:` entry wrapping two member steps; `render_rows` → assert the branch row carries `when:` + then/else arm labels, and the loop members are framed by a `loop:<name>` header + a loop-back row. FAIL first (branch falls through to linear; loop members render flat).
- [ ] **Step 2–5:** implement → pass (incl. existing tests) → commit `feat(app-canvas): render branch then/else arms and loop framing`.

---

### Task 3: CLI structure.rs — GraphRow → Line with live overlays

**Files:**
- Create: `crates/rupu-cli/src/output/live_view/structure.rs`
- Modify: `crates/rupu-cli/src/output/live_view/mod.rs` (`pub mod structure;`)
- Test: inline snapshots

**Interfaces:**
- Consumes: `rupu_app_canvas::git_graph::{render_rows, GraphRow, GraphCell, BranchGlyph}`, `rupu_app_canvas::node_status::NodeStatus`; Plan-1 `RunView` (+ `StepView`, `step_status`, `fmt_hms`); Plan-2 `row::{Line, Style}`.
- Produces:
  - `pub fn node_status_of(view: &RunView, step_id: &str) -> NodeStatus` — maps `RunView` step state → app-canvas `NodeStatus` (the `status_lookup` closure).
  - `pub fn structure_rows(view: &RunView, wf: &Workflow, nav: &NavState) -> Vec<Line>` — calls `render_rows(wf, |id| node_status_of(view, id))`, maps each `GraphCell` to a styled `Segment` (Pipe/Branch → `Style::Dim` rail glyphs via `BranchGlyph::as_str`; Bullet → the node's status glyph+color; Label → the step id `Strong`/`Plain`; Meta → `Dim`), then **overlays** per anchored node: codename (role hue via `Style::Role`), agent, provider/model, duration (`fmt_hms`), `@host` chip, and the per-kind leading glyph (`◈ ↻ ⟲ ◇ ⏸ ◆ ⊞ ⇉` per `StepView.kind`). The selected node (per `nav`) gets a `▸` marker.

- [ ] **Step 1: Write the failing test** — build a `Workflow` + `RunView` exercising a completed linear step (codename/model/duration), a running gate, a split→targets, a loop, a panel; call `structure_rows`; `insta::assert_snapshot!(render_plain(&rows))` — assert rails + per-kind glyphs + overlays read correctly; assert the selected node has `▸`.
- [ ] **Step 2–5:** fail → implement the GraphCell→Segment map + overlay + glyph table → accept snapshot → commit `feat(cli): structure pane — app-canvas rows with live overlays and per-kind glyphs`.

---

### Task 4: CLI structure.rs — fan-out density, collapse, phases, drill

**Files:**
- Modify: `crates/rupu-cli/src/output/live_view/structure.rs`
- Test: inline snapshots at scale

**Interfaces:**
- Produces: `pub fn structure_pane(view, wf, nav, w, h) -> Vec<Line>` — `structure_rows` plus: (a) re-home the Plan-2 **fan-out density line + movers** under a `for_each`/`parallel` node's rail (move `fanout_block`'s body here, nested at the node's indent); expand to the filterable unit list when that node is drilled; (b) **collapse** contiguous settled (`Complete`/`Skipped`) non-frontier non-selected steps to a `✓ a … b (+N done)` summary row; (c) **phase collapse** — a split→join sub-DAG off the frontier folds to one `⟦ phase ⟧` node, the frontier phase auto-expands, `enter` expands a collapsed phase; (d) bound to `w`×`h` (the structure column), frontier + selected never collapsed, right-hand state width-protected (`clip_row`).

- [ ] **Step 1: Write the failing test** — an 86-unit `for_each` under a split, plus 20 settled upstream steps, at a tight column height; assert the density line + movers nest under the node, settled steps collapse, the running frontier stays, the pane fits `h`. A second test: a collapsed phase vs the auto-expanded frontier phase.
- [ ] **Step 2–5:** fail → implement → accept snapshots → commit `feat(cli): structure pane — fan-out density, settled/phase collapse, drill`.

---

### Task 5: CLI panes.rs — compose the three-pane frame + fallback

**Files:**
- Create: `crates/rupu-cli/src/output/live_view/panes.rs`
- Modify: `mod.rs` (`pub mod panes;`); later REMOVE `layout.rs`'s `live_layout`/`graph` (Task 7)
- Test: inline snapshots (wide + narrow)

**Interfaces:**
- Consumes: `structure_pane` (Task 4); Plan-3 `mux` feed lines (`pinned_lines`/`firehose_lines`) passed in as `stream: &[Line]`, `firehose: &[Line]`; `RunView`, `NavState`.
- Produces: `pub fn dashboard_frame(view: &RunView, wf: &Workflow, nav: &NavState, stream: &[Line], firehose: &[Line], now: DateTime<Utc>, w: usize, h: usize) -> Vec<Line>` — composes:
  - **header** (1 row): title + crew + status + `fmt_hms` elapsed + adaptive meters (reuse Plan-2 `dashboard` meter logic).
  - **body**: left structure column (width = `clamp(w*2/5, 28, 48)`), a `│` divider, right column split into **stream** (top ~60% of body height) with a title row, and **firehose** (bottom, min 3 rows) under a `├─ live · N active ─` rule. Empty stream/firehose → one dim `waiting for activity…` row.
  - **footer** (1 row): pane- and gate-aware legend (reuse Plan-3 footer predicate; `Esc pause` labeled; add `tab pane`).
  - Every line `truncate_to(w)`; total height ≤ `h`; `w==0||h==0` → `Vec::new()`.
  - **Narrow fallback**: when `w < 76` (or below the body floor), render a single stacked pane — header + the focused pane (structure default) + footer — and show `tab` cycles which pane fills the body.

- [ ] **Step 1: Write the failing test** — snapshot `dashboard_frame` at `w=100,h=28` (assert all three regions present, divider aligned, firehose floor ≥3, height ≤28) and at `w=60,h=24` (assert single-pane fallback). Plus a gate-parked frame (footer shows `a/r/v`).
- [ ] **Step 2–5:** fail → implement → accept snapshots → commit `feat(cli): dashboard_frame — three-pane composition + narrow fallback`.

---

### Task 6: CLI nav.rs — pane focus + scroll routing

**Files:**
- Modify: `crates/rupu-cli/src/output/live_view/nav.rs`
- Test: inline unit tests

**Interfaces:**
- Produces: `pub enum Pane { Structure, Stream, Firehose }`; `NavState` gains `pane: Pane` (default `Structure`) + `NavKey::{PaneNext, PanePrev, ScrollUp, ScrollDown}` and per-pane scroll offsets; `pane()`, `cycle_pane(fwd)`, and scroll accessors. `apply(NavKey, view)` routes `PaneNext/Prev` to pane cycling; `ScrollUp/Down` scroll the focused pane; the existing drill/move keys apply only when `pane == Structure`. The structure-pane selection still drives which transcript the stream pane requests (`selected_unit`/`selected_step` unchanged).

- [ ] **Step 1: Write the failing test** — `tab` cycles Structure→Stream→Firehose→Structure; `↑↓` move selection only in Structure focus and scroll in Stream/Firehose focus; drilling still works from Structure. FAIL first.
- [ ] **Step 2–5:** fail → implement (keep `settle`/`sync`/panic-safety) → pass → commit `feat(cli): NavState pane focus + per-pane scroll`.

---

### Task 7: CLI live_run.rs — rewire the loop to the dashboard frame

**Files:**
- Modify: `crates/rupu-cli/src/output/live_run.rs` (build `dashboard_frame`; pass the `Workflow`; feed stream from the pinned buffer, firehose from `firehose_lines`; map `tab` → `PaneNext`); REMOVE `layout.rs`'s `live_layout`/`graph`/`fanout_block` (re-homed) and their now-dead tests.
- Test: a headless frame test + dead-code confirm.

**Interfaces:**
- Consumes everything above. The loop already has `workflow: Workflow` (passed to `run_live_view`). Each tick: `view.apply` + `mux.observe`; `nav.sync`; decode keys (add `tab`→`PaneNext`, `shift-tab`→`PanePrev`, scroll keys route by focus); `mux.pin(selected unit transcript)`; `stream = mux.pinned_lines()` (or the frontier node's stream when nothing pinned), `firehose = mux.firehose_lines()`; `frame = dashboard_frame(view, &workflow, &nav, &stream, &firehose, now, w, h)`; `renderer.draw`. Gate keys (`a/r/v`) and resume-generation guard unchanged.

- [ ] **Step 1: Write the failing test** — a headless test building a fixture run dir → `RunView::from_run_dir` + a `Workflow` + `NavState` → `dashboard_frame` via the same helper the loop uses → assert the gate footer shows when parked and the three panes compose. Confirm `cargo build -p rupu-cli` has NO dead-code warning after removing `live_layout`/`graph`.
- [ ] **Step 2–5:** fail → rewire + delete the flat path → build clean + tests pass → commit `feat(cli): drive the live view from the three-pane dashboard frame`.

---

### Task 8: dead-code sweep, whole-frame snapshots, wiring, tty-smoke gate

**Files:**
- Modify: `crates/rupu-cli/src/output/live_view/layout.rs` (delete the removed fns; keep anything still used), `mod.rs` doc.
- Test: whole-frame snapshots + the human smoke.

- [ ] Confirm no dangling refs to `live_layout`/flat `graph`; `cargo build -p rupu-cli` and `cargo test -p rupu-cli` clean (no `-D warnings` dead code). Grep the crate.
- [ ] Whole-frame snapshots through `dashboard_frame` for: a composite DAG (split/parallel/for_each/join/loop/panel/branch/gate), a large collapsed/phased run, a drilled unit (stream pane populated), gate-parked. Accept + paste into the report.
- [ ] Confirm `cmd/workflow.rs` wiring + the Plan-1 completion summary still fire unchanged (the view self-exits/teardown path is Plan-3's).
- [ ] **Manual tty smoke (REQUIRED, human — matt):** run a real workflow + a wide fan-out; verify the three panes read, the structure pane shows the DAG with rails and splits/joins/loops, `tab` cycles panes, drilling a unit fills the stream, the firehose stays live, a gate shows `a/r/v` and resumes, `q`/`Esc`/resize behave, and a completion summary prints once after teardown. Record in the PR.
- [ ] Commit — `refactor(cli): remove the single-column layout; dashboard-frame snapshots`.

---

## Self-Review (run against the spec before handoff)

1. **Spec coverage:** structure pane reads as a DAG (Tasks 1–4); every construct has a glyph + rail (Tasks 1–3 + app-canvas's existing kinds); stream pane follows selection (Task 7, from the pinned buffer); firehose always visible (Task 5); scale via collapse/phase/drill (Task 4); pane focus (Task 6); narrow fallback (Task 5). List any construct with no rendering.
2. **Reuse honored:** `RunView`/`mux`/`render`/`row` untouched; `nav` extended not rewritten; app-canvas extended with its tests green.
3. **Placeholders:** Tasks 1–2 + 6 carry interfaces + test shapes; Tasks 3–5, 7–8 are snapshot/integration with explicit fixtures + delete-lists + the human smoke. No "TBD".
4. **1.95 clippy hazards:** the GraphCell→Segment match, the kind-glyph table, the pane composition, decode-key additions — guards not sole-ifs, no match-on-bool, no manual clamp. Called out in Global Constraints.
5. **Terminal safety + no-mock:** AltScreen restores on panic; empty panes show `waiting…` not zero rows; meters/density truthful.

## Done definition

The live view is a three-pane dashboard: a rail-rendered DAG structure that shows splits/joins/loops/panels/fan-outs, a selection-following stream, and an always-live firehose; it scales by collapse + drill; app-canvas renders every node kind; the old single-column path is gone; matt's tty smoke passes; and PR #695 (plumbing + this rework) merges as one coherent change.
