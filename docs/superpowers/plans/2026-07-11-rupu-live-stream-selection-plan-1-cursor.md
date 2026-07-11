# Live-view stream selection — Plan 1 (Part A: node-selection cursor)

> **For agentic workers:** subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add a keyboard-driven **node-selection cursor** to the live three-zone workflow view so the user can choose which concurrent node's stream the focus zone follows (instead of today's auto-follow-latest). Covers `for_each` / `parallel` / panel concurrency now; sub-agent dispatch children (Plan 2) inherit it for free.

**Spec:** `docs/superpowers/specs/2026-07-11-rupu-live-stream-selection-design.md` (Part A; Q1–Q4 resolved: cursor scope = active step + its concurrent children; wrap at ends; release-to-auto-follow on step change).

**Scope:** entirely `crates/rupu-cli/src/output/live_run.rs` (the alt-screen TUI). No orchestrator/other-crate changes. TTY-only; the non-interactive/`--plain` path is untouched.

## Global Constraints
- **Backward compatible:** default (no manual selection) preserves today's auto-follow exactly — `selected: None` behaves as current code. Every existing `live_run.rs` test stays green.
- Esc still pauses (keep `handle_live_run_keypress`'s existing behavior). New keys are additive.
- Cursor scope (Q1) = the **active step + its concurrent children** (the `units` of the active step); completed/other-step nodes are not navigable. Wrap at ends (Q2). Release to auto-follow on step change (Q4) and on an explicit release key.
- Pure-render functions stay pure (`render_graph`/`render_dashboard`/`render_focus` take `&LiveRunState` → `Vec<String>`, unit-testable). `#![deny(clippy::all)]`; no `unsafe`. Per-file `rustfmt --edition 2021 crates/rupu-cli/src/output/live_run.rs` (it is NOT a mod-root). Never cargo fmt. `git status --short` before commit.

## Grounded shapes (verified — file:line in live_run.rs)
- `LiveRunState` (:155-215): `steps: Vec<StepState>`, `active: ActiveFocus` (:171). `ActiveFocus` (:142-153): `step_id`, `agent`, `unit_key`, `active_unit_transcript: Option<PathBuf>`. `StepState` (:111-127) has `units: Vec<UnitState>`; `UnitState` (:65-70) has `key`, status, tokens (+ a `transcript_path`, from `UnitStarted`). `ensure_unit_slot` (:507-520) grows units by `index`.
- `apply` (:239-393): `StepStarted` (:245-257) sets `active`, clears unit; `UnitStarted` (:296-322) overwrites `active.unit_key` + `active.active_unit_transcript` ("re-focus on this unit"); `UnitCompleted` (:323-354).
- Loop (:1239-1407): `handle_live_run_keypress` (:1201-1220, only Esc→pause); `desired_transcript` computed (:1284-1317) = `active.active_unit_transcript` else linear step transcript; opens/switches a `TranscriptTailer` when the path changes (:1305-1317).
- Render: `render_view` (:1078-1136) stacks `render_dashboard` (:588), `render_graph` (:709-836), `render_focus` (:871). Fan-out unit rows drawn at :771-787.
- Tests: `mod tests` (:1474+) — fan-out expansion (:1675,1695,1718), UnitStarted/Completed state machine (:1831,1855,1888,1905), token accounting (:1927,2055), keypress (Esc:2279, terminal-noop:2312, non-esc-ignored:2337), render_view composition (:2087,2135,2155).
- **Precedent selection UIs to mirror** (structure only): `crates/rupu-cli/src/cmd/autoflow.rs:2215-2241` (`switch_focus()`, `selected_index`, `sync_selection`); `crates/rupu-cli/src/cmd/session.rs` (pane selection). Borrow the cursor/index pattern; do not import.

---

## Task 1: Selection state + keyboard navigation

**Files:** `crates/rupu-cli/src/output/live_run.rs`. Test: same file `mod tests`.

**Interfaces — Produces:** `LiveRunState.selected: Option<NodeRef>` (+ `NodeRef` type identifying a navigable node: the active linear step, or a concurrent child keyed `(step_id, index)`); navigation methods; extended `handle_live_run_keypress`.

- [ ] **Step 1: Failing tests** (mirror the keypress + apply tests):
  - `nav_down_selects_first_concurrent_unit_then_next` — with an active fan-out step having N units, `↓`/`j` moves the cursor over the active step + its units in order.
  - `nav_wraps_at_ends` — moving past the last node wraps to the first (Q2).
  - `release_key_clears_selection_to_auto_follow` — a release key sets `selected = None`.
  - `step_change_releases_selection` — applying a new `StepStarted` while a child of the old step is selected clears `selected` (Q4).
  - `esc_still_pauses_with_selection_active` — Esc keeps its pause behavior regardless of selection (don't regress the existing Esc tests).
  - `nav_ignored_when_no_concurrent_nodes` — with only a single linear step and no units, nav keys are a no-op (or select the lone step) — pick + assert.
- [ ] **Step 2:** `cargo test -p rupu-cli --lib -- live_run` → new tests FAIL.
- [ ] **Step 3: Implement.**
  - Add `NodeRef` (e.g. `enum NodeRef { Step, Unit { index: usize } }` scoped to the active step — the active step_id is implicit from `active`) and `selected: Option<NodeRef>` to `LiveRunState` (default `None`).
  - Add a helper computing the ordered **navigable node set** for the current active step: `[the active linear step] + active step's units in index order` (Q1 scope). Only the active step's children are navigable.
  - Add `select_next()`/`select_prev()` (wrap, Q2) that move `selected` over that set (seeding from `None` → first on first press), and `clear_selection()`.
  - In `apply`, on `StepStarted` (a new active step), call `clear_selection()` (Q4). (Do NOT change the existing auto-follow assignment in `UnitStarted` — that stays as the default when `selected` is `None`.)
  - Extend `handle_live_run_keypress`: `Down`/`j` → `select_next`; `Up`/`k` → `select_prev`; `Tab` → `select_next`, `BackTab`/`Shift-Tab` → `select_prev`; a release key (`a`) → `clear_selection`; keep `Esc` → pause unchanged. Return whatever signal the loop expects (mirror the current return type).
- [ ] **Step 4:** tests pass; full `cargo test -p rupu-cli --lib -- live_run` green.
- [ ] **Step 5:** rustfmt live_run.rs; `cargo clippy -p rupu-cli --no-deps` clean for changed code (pre-existing unrelated lints noted in repo memory are not yours); commit `feat(cli): live-view node-selection state + keyboard navigation`.

## Task 2: Focus resolution + spine highlight + keymap hint

**Files:** `crates/rupu-cli/src/output/live_run.rs`. Test: same file.

**Interfaces — Consumes:** Task 1's `selected` + `NodeRef` + navigable-set helper.

- [ ] **Step 1: Failing tests:**
  - `desired_transcript_follows_selection_when_pinned` — with `selected = Some(Unit{index})`, the focus zone's target transcript is that unit's `transcript_path`, NOT the auto-follow latest.
  - `desired_transcript_auto_follows_when_selection_none` — `selected = None` → today's behavior (auto-follow), unchanged.
  - `pinned_completed_node_stays_focused` — a selected unit that then completes keeps being the focus target (don't auto-jump away) until the user moves/releases.
  - `render_graph_highlights_selected_node` — `render_graph` marks the selected node (a cursor glyph / marker substring) and only that one.
  - `dashboard_shows_selection_keymap_hint` — the dashboard zone includes a short key hint (e.g. `↑↓/Tab select · a auto`) when concurrent nodes exist.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement.**
  - In the loop's `desired_transcript` computation (:1284-1317): if `selected` is `Some(node)`, resolve that node's transcript path (Unit → its `units[index].transcript_path`; Step → the linear step transcript) and use it; else keep the existing auto-follow logic verbatim. When a pinned node completes, keep its path as the target (don't fall back) until `selected` changes.
  - In `render_graph` (:709-836): render a selection marker on the node matching `selected` (reverse-video or a `▸`/cursor prefix); leave all rows otherwise as today.
  - In `render_dashboard` (:588): add a one-line keymap hint for selection when the active step has concurrent nodes (keep it out when there's nothing to select, to avoid noise).
- [ ] **Step 4:** `cargo test -p rupu-cli --lib -- live_run` green (new + all existing).
- [ ] **Step 5:** rustfmt; clippy `-p rupu-cli --no-deps`; commit `feat(cli): focus follows selected node + spine highlight + keymap hint`.

---

## Self-Review
Coverage: selection state + nav (T1); focus-follows-selection + highlight + hint (T2). Q1 (active-step scope), Q2 (wrap), Q4 (release on step change) in T1; pinned-completed + render in T2. Backward compat: `selected: None` = today's auto-follow, existing tests green (asserted in both tasks). Q3 (dispatch nesting) is Plan 2. Type flow: `NodeRef`/`selected` (T1) → `desired_transcript`/`render_graph` (T2). All in one file, one crate — sequential.

## Execution
Subagent-driven: T1 → review → T2 → review → final whole-branch review → PR to main (no self-merge). **matt validates the TUI at runtime** (alt-screen: run a workflow with a fan-out, navigate with ↑↓/Tab, confirm the focus zone follows the selected node + highlight + release) before merge — subagents cannot validate alt-screen rendering. Plan 2 (dispatch nodes) follows as a separate plan/PR.
