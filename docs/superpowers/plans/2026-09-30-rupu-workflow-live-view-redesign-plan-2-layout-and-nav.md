# Workflow Live View Redesign — Plan 2: Row vocabulary, NavState, and adaptive live_layout

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure, snapshot-tested interaction and layout model for the redesigned live view — a `Line`/`Segment` row vocabulary, a `NavState` drill-navigation state machine (run → step → unit → sub-agent with a breadcrumb), and `live_layout`, which turns a `RunView` (Plan 1) + `NavState` + feed lines + terminal size into a bounded `Vec<Line>` that always keeps the active frontier and the feed on screen. No terminal I/O, no event tailing — those are Plan 3.

**Architecture:** Three new modules under `crates/rupu-cli/src/output/`. `live_view/row.rs` defines a color-role-tagged `Line`/`Segment`/`Style` vocabulary plus `render_plain` (structure-only text, for snapshots and `--no-color`). `live_view/nav.rs` defines `NavState` + a pure `NavKey` input enum (Plan 3 maps crossterm events → `NavKey`). `live_view/layout.rs` defines `live_layout`, built up zone by zone (dashboard, breadcrumb, adaptive graph with density-bar fan-out, feed region, footer) and bounded to height with a frontier-and-feed-always-visible rule. All three are pure functions over Plan 1's `RunView`; every task is `insta`-snapshot- or unit-tested the way `rupu-app-canvas` tests its `GraphRow` output. Plan 3 consumes these: `transcript_mux` produces the feed `Vec<Line>`, `live_render` diffs `Vec<Line>` to the terminal, and the rewritten `live_run.rs` loop maps keys → `NavKey` and drives `live_layout` each tick.

**Tech Stack:** Rust 2021, `rupu-cli`. `insta` (snapshots), `unicode-width` (column math — confirm it's a workspace dep, else add). Reuses Plan 1's `crate::output::run_model::{RunView, StepView, StepState, UnitView, UnitStatus, UnitCounts, DispatchView, GateView, fmt_hms, step_status}` and `crate::output::palette::Status`; codename tint/badge via `rupu_codename::{crew_tint, role_badge}` (resolved by Plan 3's renderer — Plan 2 only records the crew/role strings on `Style`).

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-workflow-live-view-redesign-design.md`

**Depends on:** Plan 1 (`docs/superpowers/plans/2026-09-30-rupu-workflow-live-view-redesign-plan-1-run-model.md`, PR #686). Implement Plan 2 on a branch cut from Plan 1's merged result (or stacked on #686).

## Global Constraints

- Workspace deps only; never pin a version in a crate `Cargo.toml`.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- **CI lints with `cargo clippy --workspace --all-targets --locked -- -D warnings` on the pinned 1.95 toolchain (`rust-toolchain.toml`).** The worktree has no rustup, so local clippy runs a different version and CANNOT fully verify 1.95 lints (e.g. 1.95 flags `collapsible_if` where 1.97 does not; 1.97 flags a `completers.rs:127` `question_mark` that 1.95 ignores). Write lint-clean code proactively: no sole-`if`-in-match-arm (use a match guard), no nested collapsible `if`, no `needless_return`. Lint is CI-arbitrated — expect a CI round-trip.
- **No mock features:** a zone/meter that cannot be truthfully computed is omitted, not shown as a zero. `live_layout` renders only what `RunView` actually carries.
- Per-file `rustfmt` only (main is fmt-dirty under the pinned toolchain); never package-wide `cargo fmt`.
- Snapshots assert STRUCTURE ONLY: `render_plain` emits no ANSI. Accept a generated `.snap.new` by inspecting it and renaming to `.snap` (strip the `assertion_line:` metadata) — `cargo insta` may not be installed.
- Every change lands on a feature branch via PR.

---

### Task 1: `Line` / `Segment` / `Style` row vocabulary + `render_plain`

**Files:**
- Create: `crates/rupu-cli/src/output/live_view/mod.rs` (declares the submodules)
- Create: `crates/rupu-cli/src/output/live_view/row.rs`
- Modify: `crates/rupu-cli/src/output/mod.rs` (add `pub mod live_view;`)
- Test: inline `#[cfg(test)]` in `row.rs`

**Interfaces:**
- Consumes: `crate::output::run_model::{StepState, UnitStatus}`; `crate::output::palette::Status`.
- Produces (every later task + Plan 3 rely on these):
  - `pub struct Segment { pub text: String, pub style: Style }`
  - `pub enum Style { Plain, Dim, Strong, Status(Status), Crew(String), Role(String), Meter, Danger, Good }`
  - `pub struct Line { pub segments: Vec<Segment> }` with builders `Line::new()`, `.plain(s)`, `.dim(s)`, `.strong(s)`, `.status(Status, s)`, `.crew(crew, s)`, `.role(role, s)`, `.meter(s)`, `.danger(s)`, `.good(s)` (each pushes a `Segment` and returns `self`), and `.width() -> usize` (sum of `unicode_width` of segment texts).
  - `pub fn render_plain(lines: &[Line]) -> String` — joins each line's segment texts, `\n`-separated, no ANSI, trailing spaces trimmed per line.
  - `pub fn truncate_to(line: Line, max_cols: usize) -> Line` — clips a line to `max_cols` display columns, appending `…` (U+2026) in the last column when clipped; protects nothing special (callers order segments so the right-hand status is placed before truncation happens — see Task 5's leader logic).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::palette::Status;

    #[test]
    fn render_plain_joins_segments_and_trims_trailing() {
        let lines = vec![
            Line::new().status(Status::Complete, "✓ ").strong("preflight").plain("   "),
            Line::new().dim("│"),
        ];
        assert_eq!(render_plain(&lines), "✓ preflight\n│");
    }

    #[test]
    fn truncate_to_clips_with_ellipsis() {
        let line = Line::new().plain("abcdefghij");
        assert_eq!(render_plain(&[truncate_to(line, 5)]), "abcd…");
    }

    #[test]
    fn width_counts_display_columns() {
        assert_eq!(Line::new().plain("ab").dim("cd").width(), 4);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::live_view::row 2>&1 | tail -20`
Expected: FAIL — module/types not found.

- [ ] **Step 3: Write minimal implementation**

First confirm `unicode-width`: `grep -n 'unicode-width\|unicode_width' Cargo.toml crates/rupu-cli/Cargo.toml`. If absent from `crates/rupu-cli/Cargo.toml`, add `unicode-width.workspace = true` (add to root `[workspace.dependencies]` as `unicode-width = "0.1"` if the workspace lacks it) in this task's commit.

`crates/rupu-cli/src/output/live_view/mod.rs`:
```rust
//! Pure interaction + layout model for the redesigned `workflow run` live
//! view (spec 2026-09-30). No terminal I/O — Plan 3 renders these rows and
//! feeds events in.
pub mod layout;
pub mod nav;
pub mod row;
```
(Declare `layout` and `nav` now even though they land in later tasks — add empty `//! placeholder` files for them in THIS task so the module compiles, per the repo convention for stub modules. Each stub is a single doc-comment line.)

`crates/rupu-cli/src/output/live_view/row.rs`:
```rust
use unicode_width::UnicodeWidthStr;

use crate::output::palette::Status;

/// Color role for a run of text. Plan 3's renderer maps each to ANSI;
/// `render_plain` ignores it (structure-only output for snapshots and
/// `--no-color`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Style {
    Plain,
    Dim,
    Strong,
    Status(Status),
    /// Run-level crew tint; carries the crew word for the renderer to look
    /// up via `rupu_codename::crew_tint`.
    Crew(String),
    /// Per-agent role hue; carries the role word for `rupu_codename::role_badge`.
    Role(String),
    Meter,
    Danger,
    Good,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub segments: Vec<Segment>,
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }
    fn push(mut self, text: impl Into<String>, style: Style) -> Self {
        self.segments.push(Segment { text: text.into(), style });
        self
    }
    pub fn plain(self, s: impl Into<String>) -> Self { self.push(s, Style::Plain) }
    pub fn dim(self, s: impl Into<String>) -> Self { self.push(s, Style::Dim) }
    pub fn strong(self, s: impl Into<String>) -> Self { self.push(s, Style::Strong) }
    pub fn status(self, st: Status, s: impl Into<String>) -> Self { self.push(s, Style::Status(st)) }
    pub fn crew(self, crew: impl Into<String>, s: impl Into<String>) -> Self { self.push(s, Style::Crew(crew.into())) }
    pub fn role(self, role: impl Into<String>, s: impl Into<String>) -> Self { self.push(s, Style::Role(role.into())) }
    pub fn meter(self, s: impl Into<String>) -> Self { self.push(s, Style::Meter) }
    pub fn danger(self, s: impl Into<String>) -> Self { self.push(s, Style::Danger) }
    pub fn good(self, s: impl Into<String>) -> Self { self.push(s, Style::Good) }

    pub fn width(&self) -> usize {
        self.segments.iter().map(|seg| seg.text.width()).sum()
    }
}

pub fn render_plain(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| {
            let mut s = String::new();
            for seg in &l.segments {
                s.push_str(&seg.text);
            }
            s.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clip `line` to `max_cols` display columns, ending with `…` when clipped.
pub fn truncate_to(line: Line, max_cols: usize) -> Line {
    if line.width() <= max_cols {
        return line;
    }
    let budget = max_cols.saturating_sub(1); // room for the ellipsis
    let mut out = Line::new();
    let mut used = 0usize;
    for seg in line.segments {
        let w = seg.text.width();
        if used + w <= budget {
            used += w;
            out.segments.push(seg);
        } else {
            let mut kept = String::new();
            for ch in seg.text.chars() {
                let cw = ch.to_string().width();
                if used + cw > budget {
                    break;
                }
                used += cw;
                kept.push(ch);
            }
            if !kept.is_empty() {
                out.segments.push(Segment { text: kept, style: seg.style });
            }
            break;
        }
    }
    out.segments.push(Segment { text: "…".to_string(), style: Style::Dim });
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli output::live_view::row 2>&1 | tail -20`
Expected: PASS (3). Then `cargo build -p rupu-cli` green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/live_view/ crates/rupu-cli/src/output/mod.rs Cargo.toml crates/rupu-cli/Cargo.toml
git commit -m "feat(cli): live-view Line/Segment/Style row vocabulary + render_plain"
```

---

### Task 2: `NavState` — depth path, selection, `NavKey` transitions, breadcrumb

**Files:**
- Create: `crates/rupu-cli/src/output/live_view/nav.rs` (replacing the Task-1 stub)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `crate::output::run_model::{RunView, StepView, UnitView, UnitStatus}`.
- Produces:
  - `pub enum NavKey { Up, Down, In, Out, Follow, Filter, Quit, Pause }`
  - `pub enum Depth { Run, Step, Unit, SubAgent }`
  - `pub enum UnitFilter { All, Running, Failed, Done }` with `pub fn next(self) -> UnitFilter` cycling `All→Running→Failed→Done→All`.
  - `pub enum NavAction { None, Quit, Pause }` — returned by `apply` so Plan 3 knows when to leave/pause.
  - `pub struct NavState { depth, step_idx, unit_idx, sub_idx, follow: bool, filter: UnitFilter }` (fields private; constructed by `NavState::default()` = `{ depth: Run, follow: true, filter: All, .. }`).
  - `NavState::apply(&mut self, key: NavKey, view: &RunView) -> NavAction` — the pure transition.
  - `NavState::breadcrumb(&self, view: &RunView) -> Vec<String>` — `["mint-tundra", "hunt", "otter#41", "wren#1"]` truncated to the current depth (crew from `view.crew`, step_id, unit codename-or-key, sub-agent codename).
  - `NavState::selected_step<'a>(&self, view: &'a RunView) -> Option<&'a StepView>` and `selected_unit` / `filter()` / `is_following()` / `depth()` accessors for `live_layout`.

Selection rules:
- `Up`/`Down` move the index at the current depth within the in-view list (steps at Run depth; the FILTERED units at Step depth; sub-agents of the selected unit at Unit depth). Moving clears `follow`.
- `In` descends: Run→Step (selects `step_idx`), Step→Unit (only when the selected step has units; selects first unit in the filtered list), Unit→SubAgent (only when the selected unit has sub-agents — dispatches whose `parent_step_id` == the step and which the view associates with this unit; for Plan 2, treat all `view.dispatches` under the selected step as candidates, ordered). No-op at a leaf.
- `Out` ascends one level; at `Run` it is a no-op (does NOT quit).
- `Follow` sets `follow = true` and resets indices to the newest activity (Plan 2: reset to depth `Run`, `follow=true` — the "newest" tracking is applied by `live_layout`/Plan 3 using `follow`).
- `Filter` cycles `filter` (only meaningful at Step/Unit depth over a fan-out).
- `Quit` returns `NavAction::Quit`; `Pause` returns `NavAction::Pause`. Neither mutates depth.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::run_model::RunView;
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::StepKind;

    fn fanout_view() -> RunView {
        let mut v = RunView::default();
        v.crew = Some("mint-tundra".into());
        v.apply(&Event::StepStarted { run_id: "r".into(), step_id: "hunt".into(),
            kind: StepKind::ForEach, agent: None, host: None, codename: None });
        for i in 0..3usize {
            v.apply(&Event::UnitStarted { run_id: "r".into(), step_id: "hunt".into(),
                index: i, unit_key: format!("svc-{i}"), agent: Some("breaker".into()),
                transcript_path: format!("t{i}").into(), host: None,
                codename: Some(format!("otter#{}", i + 1)) });
        }
        v
    }

    #[test]
    fn drill_in_and_out_walks_the_depth_axis() {
        let v = fanout_view();
        let mut nav = NavState::default();
        assert_eq!(nav.depth(), Depth::Run);
        assert_eq!(nav.apply(NavKey::In, &v), NavAction::None);
        assert_eq!(nav.depth(), Depth::Step);
        assert_eq!(nav.apply(NavKey::In, &v), NavAction::None);
        assert_eq!(nav.depth(), Depth::Unit);
        assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra", "hunt", "otter#1"]);
        assert_eq!(nav.apply(NavKey::Out, &v), NavAction::None);
        assert_eq!(nav.depth(), Depth::Step);
        // Out at Run is a no-op, never a quit.
        nav.apply(NavKey::Out, &v);
        assert_eq!(nav.depth(), Depth::Run);
        assert_eq!(nav.apply(NavKey::Out, &v), NavAction::None);
        assert_eq!(nav.depth(), Depth::Run);
    }

    #[test]
    fn quit_and_pause_are_distinct_and_do_not_move() {
        let v = fanout_view();
        let mut nav = NavState::default();
        assert_eq!(nav.apply(NavKey::Quit, &v), NavAction::Quit);
        assert_eq!(nav.depth(), Depth::Run);
        assert_eq!(nav.apply(NavKey::Pause, &v), NavAction::Pause);
        assert_eq!(nav.depth(), Depth::Run);
    }

    #[test]
    fn move_clears_follow_and_filter_cycles() {
        let v = fanout_view();
        let mut nav = NavState::default();
        assert!(nav.is_following());
        nav.apply(NavKey::Down, &v);
        assert!(!nav.is_following());
        nav.apply(NavKey::Follow, &v);
        assert!(nav.is_following());
        nav.apply(NavKey::In, &v); // to Step
        let f0 = nav.filter();
        nav.apply(NavKey::Filter, &v);
        assert_ne!(nav.filter(), f0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::live_view::nav 2>&1 | tail -20`
Expected: FAIL — `NavState` etc. not found.

- [ ] **Step 3: Write minimal implementation**

Implement `NavState` exactly to the selection rules above. Key points the code must get right (each is covered by a test in Step 1 or Task 6):
- `In` from `Step` only advances to `Unit` when `selected_step(view)` has a non-empty `units` map; otherwise no-op.
- `Out` from `Run` is a no-op returning `NavAction::None`.
- `breadcrumb` builds from `view.crew`, the selected step's `step_id`, the selected unit's `codename` (fallback `unit_key`), and the selected sub-agent's `codename`.
- `apply` never panics on an out-of-range index — clamp `step_idx`/`unit_idx` to the current in-view list length each call (a run grows steps/units between ticks).
- Use match guards, not sole-`if`-in-arm (1.95 clippy).

(Write the full implementation here — the reviewer checks it against the rules. It is ~120 lines; keep it pure and panic-free.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli output::live_view::nav 2>&1 | tail -20`
Expected: PASS (3).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/live_view/nav.rs
git commit -m "feat(cli): NavState drill-navigation state machine (run→step→unit→sub-agent)"
```

---

### Task 3: `live_layout` — dashboard zone

**Files:**
- Create: `crates/rupu-cli/src/output/live_view/layout.rs` (replacing the Task-1 stub)
- Test: inline `#[cfg(test)]` snapshot

**Interfaces:**
- Consumes: `RunView`, `fmt_hms`, `step_status`, `RunView::elapsed_ms`; `Line`/`Style` (Task 1); `palette::Status`.
- Produces: `pub fn dashboard(view: &RunView, now: DateTime<Utc>) -> Vec<Line>` (title + crew + status + hours-elapsed; `step N/M` progress bar over completed/total steps; a meter row `⇡in ⇣out $cost · ⚑findings · coverage P%` where each meter is emitted ONLY when the datum exists — cost only when `usage.priced`, findings only when `findings_by_severity` non-empty, coverage omitted in Plan 2 since `RunView` carries none yet). Returns 3–4 `Line`s.

- [ ] **Step 1: Write the failing test** — build a `RunView` (completed, crew `mint-tundra`, one completed + one running step, `usage` with priced cost, some `findings_by_severity`), call `dashboard`, `insta::assert_snapshot!(render_plain(&dashboard(&v, now)))`. Add a second test asserting the cost meter is absent when `usage.priced == false` and the findings meter absent when the map is empty.

- [ ] **Step 2: Run to verify it fails** — `cargo test -p rupu-cli output::live_view::layout 2>&1 | tail -20` → FAIL (`dashboard` missing).

- [ ] **Step 3: Implement `dashboard`** — real code emitting the Lines per the interface; progress bar fill = `completed_steps / total_steps` rendered with `█`/`░` over a fixed ~24-col width; reuse `fmt_hms(view.elapsed_ms(now).unwrap_or(0))`; crew via `.crew(crew, crew)`; status word via the run's status `as_str()`.

- [ ] **Step 4: Run + accept snapshot** — inspect the `.snap.new`, confirm it reads like the mock's dashboard, accept (rename, strip `assertion_line`). The unpriced/empty test passes outright.

- [ ] **Step 5: Commit** — `git add ...layout.rs ...snapshots/` → `feat(cli): live_layout dashboard zone (adaptive meters, no-mock)`.

---

### Task 4: `live_layout` — graph zone (singleton steps, kinds, codenames)

**Files:**
- Modify: `crates/rupu-cli/src/output/live_view/layout.rs`
- Test: inline snapshot

**Interfaces:**
- Produces: `pub fn graph(view: &RunView, nav: &NavState) -> Vec<Line>` for the NON-fan-out rows: one `Line` per step in `view.steps` order, rendered as `<status-glyph> <step_id> · <codename> · <agent> · <provider/model> ···· <status/duration>`. Kind is shown for non-linear kinds (`· gate`, `· action`, `· panel` with `iter r/max`, `· loop` with `iter N`, `· branch`/`· split`/`· join`). The selected step (per `nav`) is marked (a leading `▸` in `Strong`, others `Dim` leader). Codename uses `Style::Crew`/`Style::Role`. Sub-agent dispatch rows for a step render as indented children (`┣━`/`┗━` from `BranchGlyph`) when that step is drilled or selected. Fan-out steps are delegated to Task 5 (`fanout_block`), called from here.

- [ ] Steps 1–5 as above: a failing snapshot test over a `RunView` with a completed linear step (codename `heron#1`, provider/model set), a running gate step, a panel step with `panel_round=2/5`; assert the rendered rows via `render_plain`; implement; accept snapshot; commit `feat(cli): live_layout graph zone — per-kind step rows with codenames`.

---

### Task 5: `live_layout` — fan-out density bar + live movers + filtered expand

**Files:**
- Modify: `crates/rupu-cli/src/output/live_view/layout.rs`
- Test: inline snapshot at 86-unit scale

**Interfaces:**
- Produces: `pub fn fanout_block(step: &StepView, nav: &NavState, selected: bool) -> Vec<Line>`:
  - Always: the step header row + a density line — a `█`/`░` %-complete bar plus `done/total  ✓D ◐R ✗F ○Q` from `step.unit_counts()`, plus the step's aggregated `⇡tokens`/`$cost` when present.
  - Collapsed (default, step not drilled): up to N (e.g. 4) "live movers" — the most-recently-active units (Plan 2: the last N by index whose status is `Running`, else the last N) as indented `┣━`/`┗━` rows `◐ <codename> · <unit_key> · <provider/model> ··· <state>`, then a `┗━ … +K more · [enter] expand` row when `total > shown`.
  - Expanded (step drilled — `nav.depth() >= Step` and this is the selected step): the FULL unit list filtered by `nav.filter()`, each an indented row; a filter indicator in the header (`· running` / `· failed` / `· done`). Selected unit marked.
- The density bar and counts must reconcile with `unit_counts()` exactly (no mock numbers).

- [ ] Steps 1–5: a failing snapshot test building a `RunView` with an 86-unit `ForEach` step (mix of done/running/failed/queued), asserting (a) the collapsed density line + movers + `+K more`, and (b) with a `NavState` drilled into the step and `filter=Failed`, only failed units render. Implement; accept snapshots; commit `feat(cli): live_layout fan-out density bar + movers + filtered expand`.

---

### Task 6: `live_layout` — compose + adaptive bounding (frontier & feed always visible) + breadcrumb + footer

**Files:**
- Modify: `crates/rupu-cli/src/output/live_view/layout.rs`
- Test: inline snapshots at 24-row and 60-row heights; NavState unit tests for edge indices

**Interfaces:**
- Produces: `pub fn live_layout(view: &RunView, nav: &NavState, feed: &[Line], w: usize, h: usize) -> Vec<Line>` — the whole frame:
  1. `dashboard` (Task 3), always.
  2. breadcrumb line (Task 2 `nav.breadcrumb`) when `nav.depth() != Depth::Run`.
  3. graph (Task 4 + Task 5), with **adaptive collapse**: compute the graph's natural height; if `dashboard + breadcrumb + graph + feed(min) + footer > h`, collapse settled regions first — contiguous runs of `Complete`/`Skipped` steps that are NOT the selected step and NOT on the active frontier collapse to a single `✓ preflight  ✓ inventory  … (+N done)` summary line — repeating until it fits or only the frontier + selected + (collapsed-settled) remain. The **frontier** (every step whose `state == Running`/`AwaitingApproval`, and their fan-out blocks) and the **selected** step are never collapsed.
  4. feed region: the given `feed` lines, each `truncate_to(w)`, guaranteed a minimum height (e.g. 4 rows); if space is tight the GRAPH yields to the feed's minimum, never the reverse.
  5. footer (always): a context key legend from `nav` (e.g. `↑↓ move · enter drill · ← back · / filter · p pin · q quit`; at a focused gate: `a approve · r reject · v findings`).
  - Every line is `truncate_to(w)`. Total output height ≤ `h`.
- Invariant (assert in tests): for any `h >= dashboard+footer+feed_min+1`, the output contains at least one frontier row and `feed_min` feed rows.

- [ ] Steps 1–5: failing snapshot tests — a 40-step / 86-unit `RunView` at `h=24` (assert the running frontier step + its density block are present, settled steps collapsed, feed's min rows present) and at `h=60` (more shown); a breadcrumb test when drilled; a footer test at a gate. Add `NavState` edge tests (drill when no units; index clamp when steps grow). Implement; accept snapshots; commit `feat(cli): live_layout adaptive compose — frontier+feed always visible, breadcrumb, footer`.

---

## Self-Review (run against the spec before handing off)

1. **Spec coverage:** adaptive layout / frontier-never-scrolls-off (#2) → Task 6; density-bar fan-out (#4) → Task 5; drill nav + breadcrumb + key scheme → Task 2 + Task 6 footer; codename tints/role badges (#9, the data side) → Tasks 1/4 (`Style::Crew`/`Role`; colors resolved in Plan 3). The firehose feed CONTENT (#5), the diff renderer, in-view gate interaction (#10), and the resume-generation guard are Plan 3 — Task 6 leaves a feed region and a gate-footer hook for them. List any spec zone with no task.
2. **Placeholder scan:** Tasks 3–6 describe the snapshot tests and give the interface + zone contract but defer the exact rendered strings to the accepted snapshot (like Plan 1's Task 6) — that is intentional for snapshot tasks, not a placeholder; every task still has concrete types, signatures, fixtures, and assertions. Tasks 1–2 carry full code. Confirm no "TBD"/"handle edge cases" without specifics.
3. **Type consistency:** `Line`/`Segment`/`Style`, `NavKey`/`Depth`/`NavAction`/`UnitFilter`/`NavState`, `dashboard`/`graph`/`fanout_block`/`live_layout` names are identical across tasks and match Plan 1's `RunView` field/method names.
4. **1.95 clippy hazards:** every match with a conditional arm uses a guard, not a sole inner `if`; no nested collapsible `if`. Called out in Global Constraints.

## Plan 3 preview (not part of this plan)

`transcript_mux` (bounded multi-tail firehose → `Vec<Line>` feed, respecting the fd budget), `live_render` (diff `Vec<Line>` → terminal, replacing the full-screen reprint), and the `live_run.rs` loop rewrite that maps crossterm keys → `NavKey`, drives `live_layout` each tick, renders in-view gate approve/reject, and guards resume-generation terminal events. Plan 3 is where the view becomes live and interactive; it consumes Plan 2's pure model unchanged.
