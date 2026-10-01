# Workflow Live View Redesign — Plan 3: live integration (transcript_mux, live_render, loop rewrite, in-view gate)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the redesigned live view actually live and interactive. Wire Plan 1's `RunView` and Plan 2's `NavState`/`live_layout` into `rupu workflow run`'s in-place terminal surface: a bounded multi-transcript firehose feed (`transcript_mux`), a diff renderer that kills the full-screen-reprint flicker (`live_render`), a rewritten `live_run.rs` tick loop that drives them with crossterm key input → `NavKey`, in-view gate approve/reject, and a resume-generation guard on terminal exit.

**Architecture:** Two new modules under `crates/rupu-cli/src/output/live_view/`: `mux.rs` (`TranscriptMux` — tails a bounded set of the most-recently-active transcripts within the fd budget, merges them into one time-ordered `Vec<FeedLine>`, each tagged with its codename; a pure `project_event` widening of the old `map_transcript_event`) and `render.rs` (`LiveRenderer` — diffs `Vec<Line>` vs the last frame and emits only changed rows via crossterm, mapping each `Style` to ANSI via the existing `palette`/`rupu_codename` color surfaces; owns alt-screen/raw-mode/resize). Then `live_run.rs`'s `run_live_view` loop is rewritten to drive `RunView::apply` + `TranscriptMux` + `NavState` + `live_layout` + `LiveRenderer` each 100 ms tick, decode keys to `NavKey`, act on `NavAction`, render the in-view gate, and exit on a terminal event of the *current* run generation only. The old full-reprint `render_view`/`render_dashboard`/`render_graph`/`render_focus`/`LiveRunState`/`map_transcript_event`/`focused_transcript` path is deleted. The sibling-task wiring in `cmd/workflow.rs` (`run_workflow_with_live_view` + the Plan 1 `print_completion_summary` convergence) is unchanged except for the `run_live_view` body.

**Tech Stack:** Rust 2021, `rupu-cli`. `crossterm` 0.28 (events + queue/execute; already a dep), `tokio` interval, `insta` snapshots. Reuses Plan 1 `run_model` (`RunView`, `apply`, `from_run_dir`, `generation`), Plan 2 `live_view::{row, nav, layout}`, `jsonl_reader::{WfEventTailer, TranscriptTailer}`, `rupu_transcript::Event`, `rupu_cp::usage::{run_usage, run_transcript_paths}`, `rupu_orchestrator::runs::{RunStore, AwaitingGate, known_transcript_from_event_line}`, `rupu_agent::fd_budget::fd_usage`, `palette::{Status, write_colored, active_palette}`, `rupu_codename::{crew_tint, role_badge}`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-workflow-live-view-redesign-design.md`

**Depends on:** Plan 1 (merged, PR #686) and Plan 2 (its own PR). Cut Plan 3's branch from the main commit that has BOTH merged.

## Global Constraints

- Workspace deps only; never pin a version in a crate `Cargo.toml`.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- **CI lints with `cargo clippy --workspace --all-targets --locked -- -D warnings` on the pinned 1.95 toolchain; the worktree runs 1.97 and CANNOT verify 1.95 lints.** Write lint-clean proactively: match guards not sole-`if`-in-arm, no nested collapsible `if`, no match-on-bool, no `needless_return`, no manual clamp/min-max. A pre-existing `completers.rs:127` `question_mark` is a 1.97-only false alarm — ignore it. Lint is CI-arbitrated; expect one round-trip.
- **No mock features:** the firehose shows only real transcript events; usage/cost stays truthful; a transcript the mux cannot open degrades to "read on complete", never a fabricated line.
- **GUI/rendering rule:** `cargo test` green ≠ rendering green. Plan 3 is the first time the redesign draws on a real terminal, so a human tty smoke (run a real workflow; watch the live view, drill nav, a gate, a wide fan-out) is a REQUIRED pre-merge verification — flag it; it is not a code change.
- Per-file `rustfmt` only; never package-wide `cargo fmt`.
- The view must restore the terminal on every exit path (alt-screen guard Drop) — a panic mid-render must not leave the user's terminal in raw mode.
- Every change lands on a feature branch via PR.

## Plan 2 carry-forward entry conditions (must be honored in Plan 3)

- **I1 — follow-mode gate focus:** today the gate footer/keys activate only when the *chosen* step is `AwaitingApproval`. In follow mode `NavState` parks the cursor at step 0, so a parked gate would show no `a/r/v` until the operator navigates to it. Plan 3 decides focus: when the run is `AwaitingApproval` and the operator has not manually selected, auto-focus the (first) gate step, and the key-dispatch predicate MUST match the footer predicate exactly.
- **I2 — wire the gate keys:** the footer legend advertises `a approve · r reject · v findings`; these are not `NavKey`s. Plan 3 MUST wire the `a`/`r`/`v` handlers in the SAME change that makes the footer reachable (Task 4). They must never be dead advertising.
- **M4 — right-state clip for non-leader rows:** `clip_row` protects the right-hand state only for rows carrying the ` ···· ` leader. `live_render` (or the layout) must not re-clip; the renderer emits rows as the layout produced them (already width-bounded). Do not double-clip.

---

### Task 1: `project_event` — widen the transcript→feed projection

**Files:**
- Create: `crates/rupu-cli/src/output/live_view/mux.rs`
- Modify: `crates/rupu-cli/src/output/live_view/mod.rs` (add `pub mod mux;`)
- Test: inline `#[cfg(test)]` in `mux.rs`

**Interfaces:**
- Consumes: `rupu_transcript::Event`; Plan 2 `row::{Line, Style}`; `palette::Status`.
- Produces:
  - `pub struct FeedLine { pub ts: Option<DateTime<Utc>>, pub codename: Option<String>, pub line: Line }` — one projected feed row (the `line` is a Plan 2 `Line` so `live_render` colors it uniformly; `codename` tags the source unit/sub-agent; `ts` orders the merge).
  - `pub fn project_event(ev: &rupu_transcript::Event, codename: Option<&str>) -> Option<FeedLine>` — the widened map. Returns `None` for events with no feed representation (RunStart/TurnStart/Seed/etc.). Covers, at minimum: `ToolCall` → `▸ <tool> <arg-summary>`; `Thinking` → `◇ thinking <trunc>` (the streamed `ThinkingDelta` returns `None` — the committed `Thinking` carries the whole block); `AssistantMessage` → `▪ <trunc>` (the streamed `AssistantDelta` returns `None`); every projected row is scrubbed of control characters (reuse Plan 2's `printable`) so untrusted model/tool/repo text cannot break the row or inject terminal escapes; `FileEdit` → `✎ <kind> <path>`; `CommandRun` → `$ <argv0> (exit N)`; `ActionEmitted` with `kind.contains("finding")` → `⚑ <SEV> <title>` (Style::Danger/Meter by severity); `ToolAudit{blocked:true}` → `✕ <tool> blocked` (Style::Danger); `Notice` → `! <message>` (Style::Dim). Each builds a `Line` with the codename as a `Style::Role` lead segment.

- [ ] **Step 1: Write the failing test** — construct representative `rupu_transcript::Event` values (ToolCall, Thinking, a finding ActionEmitted, a blocked ToolAudit) and assert `render_plain(&[project_event(&ev, Some("otter#3")).unwrap().line])` contains the expected glyph + codename + summary; assert `project_event(&Event::TurnStart{..}, None)` is `None`.
- [ ] **Step 2: Run to verify it fails** — `cargo test -p rupu-cli output::live_view::mux 2>&1 | tail` → FAIL (module missing).
- [ ] **Step 3: Implement** `FeedLine` + `project_event` per the interface. Reuse the old `map_transcript_event` logic (`live_run.rs:1891`) as a starting point but widen it and emit `Line`s (not `(ActivityKind, String)`). Truncate long text with the Plan 2 `truncate_to` or a char budget. Use match guards; the big `match ev` is exhaustive with a final `_ => None`.
- [ ] **Step 4: Run to verify it passes.**
- [ ] **Step 5: Commit** — `feat(cli): transcript-event → feed-line projection (widened firehose map)`.

---

### Task 2: `TranscriptMux` — bounded multi-tail firehose

**Files:**
- Modify: `crates/rupu-cli/src/output/live_view/mux.rs`
- Test: inline (writes temp JSONL transcripts)

**Interfaces:**
- Consumes: `jsonl_reader::TranscriptTailer`; `project_event`/`FeedLine` (Task 1); `rupu_agent::fd_budget::fd_usage`.
- Produces:
  - `pub struct TranscriptMux { /* open tailers keyed by path, LRU of last-activity, pinned path, ring buffer of FeedLine, codename-by-path map, budget */ }`
  - `TranscriptMux::new(max_tails: usize) -> Self` — `max_tails` derived by the caller from `fd_budget::fd_usage()` (leave headroom; see Task 3). Clamp to a sane floor/ceiling (e.g. 4..=64).
  - `pub fn observe(&mut self, path: PathBuf, codename: Option<String>)` — register a transcript the run has produced (from a `UnitStarted`/`AgentStarted`/`DispatchStarted`/`StepWorking` event). Does not necessarily open it yet.
  - `pub fn pin(&mut self, path: Option<PathBuf>)` — the drilled unit's transcript; pinned is exempt from eviction and always tailed.
  - `pub fn drain(&mut self) -> &[FeedLine]` — advance the open tailers (the pinned one + the N most-recently-active within `max_tails`), project new events via `project_event`, append to the capped ring (e.g. 500), and return the ring slice. Opening a tail that fails (fd exhaustion / missing file) is swallowed (the unit still contributes to totals elsewhere); never panic.
  - `pub fn pinned_lines(&self) -> Vec<Line>` and `pub fn firehose_lines(&self) -> Vec<Line>` — the feed region content for `live_layout`: when a unit is pinned, its stream; else the merged cross-unit firehose (newest last).

Eviction: when the open set exceeds `max_tails`, close the least-recently-active unpinned tail. A newly-active observed path re-opens on demand. Tokens/usage are NOT computed here (Plan 1/`run_usage` owns totals); the mux is display-only.

- [ ] **Step 1: Write the failing test** — write 3 temp transcript files with a few events each; `observe` all 3; `drain`; assert the merged firehose contains lines from all 3 tagged with their codenames; set `max_tails=1`, `observe` a 4th and `drain`, assert the least-recently-active was evicted (still ≤1 open) but a `pin` on one keeps it open; assert no panic when a path does not exist.
- [ ] **Step 2–5:** fail → implement (LRU + pin + ring + fd-aware open) → pass → commit `feat(cli): TranscriptMux — bounded multi-transcript firehose within the fd budget`.

---

### Task 3: `LiveRenderer` — diff-render `Vec<Line>` to the terminal

**Files:**
- Create: `crates/rupu-cli/src/output/live_view/render.rs`
- Modify: `crates/rupu-cli/src/output/live_view/mod.rs` (`pub mod render;`)
- Test: inline (render to an in-memory buffer)

**Interfaces:**
- Consumes: Plan 2 `row::{Line, Segment, Style}`; `palette::{Status, write_colored, active_palette}`; `rupu_codename::{crew_tint, role_badge}`; `crossterm`.
- Produces:
  - `pub fn style_ansi(buf: &mut String, seg: &Segment)` — map one `Segment` to colored text: `Style::Status(s)` → `palette`'s color for `s`; `Crew(w)` → `crew_tint(w).map(|t| t.dark_rgb())`; `Role(w)` → `role_badge(w).tint.dark_rgb()`; `Dim`/`Strong`/`Meter`/`Danger`/`Good`/`Plain` → fixed palette roles. Honors `NO_COLOR`/`active_palette()` (colorless → raw text). This is the pure, unit-testable core.
  - `pub struct LiveRenderer { last: Vec<String> }` with `pub fn new() -> Self` and `pub fn draw(&mut self, out: &mut impl Write, frame: &[Line]) -> io::Result<()>` — render each `Line` to an ANSI string (via `style_ansi`), DIFF against `self.last` row by row, and emit `MoveTo(0,row)` + the row + clear-to-EOL only for rows that changed (and clear trailing rows when the new frame is shorter). No full-screen `Clear(All)`.
  - `pub struct AltScreen` — RAII guard: `enter()` does `enable_raw_mode` + `EnterAlternateScreen` + `Hide`; `Drop` restores (`Show` + `LeaveAlternateScreen` + `disable_raw_mode`). (Lift the existing `AltScreenGuard` from `live_run.rs`.)

- [ ] **Step 1: Write the failing test** — `style_ansi` colorless-mode test (set the colorless palette) asserts a `Status`/`Crew`/`Role` segment renders as its raw text with no escapes; a `LiveRenderer` diff test: `draw` frame A to a buffer, then frame B differing in one row, assert the second `draw`'s output contains a `MoveTo` to that row and NOT the unchanged rows' text; a shorter-frame test asserts trailing rows are cleared.
- [ ] **Step 2–5:** fail → implement (diff + ANSI map + guard) → pass → commit `feat(cli): LiveRenderer — diff-render Line frames, no full-screen reprint`.

---

### Task 4: rewrite `run_live_view` — drive the model, nav, mux, renderer, in-view gate

**Files:**
- Modify: `crates/rupu-cli/src/output/live_run.rs` (rewrite `run_live_view`; delete the dead full-reprint path it replaces)
- Test: a headless integration test of the loop's pure tick (see Step 1)

**Interfaces:**
- Consumes everything above + `RunView::{apply, from_run_dir, generation}`, `NavState::{apply, sync, breadcrumb, selected_step, depth}`, `NavKey`, `NavAction`, `live_layout`, `RunStore::{load, approve_gate, reject_gate, pause}`, `crate::resume::resume_run`, `WfEventTailer`.
- Produces: the rewritten `pub async fn run_live_view(workflow, runs_dir, run_id, pricing) -> io::Result<()>` (signature unchanged) with this tick:
  1. drain `WfEventTailer` → `RunView::apply`; for each event carrying a `transcript_path` (`known_transcript_from_event_line` shapes), `mux.observe(path, codename)`.
  2. `nav.sync(&view)`; decode pending crossterm keys → `NavKey`; `match nav.apply(key, &view)` for `NavAction::{Quit → break, Pause → pause + break}`; the gate keys (I2) are handled before nav when a gate is focused (I1): `a`→`store.approve_gate`, `r`→`store.reject_gate` (+ spawn `resume::resume_run`), `v`→toggle findings.
  3. `mux.pin(nav.selected_unit(&view).map(|u| u.transcript_path))`; `mux.drain()`; `feed = if pinned { mux.pinned_lines() } else { mux.firehose_lines() }`.
  4. refresh usage ≤1 Hz via `run_usage` → fold into `view` (reuse Plan 1's path).
  5. `(w,h)=terminal::size()`; `frame = live_layout(&view, &nav, &feed, Utc::now(), w, h)`; `renderer.draw(&mut stdout, &frame)`.
  6. terminal-exit: break only when `view.status.is_terminal()||Paused` AND the terminal event belongs to the CURRENT `view.generation` (resume-generation guard — do not exit on a prior generation's terminal event replayed from `events.jsonl`).
- `max_tails` for the mux = derived from `fd_budget::fd_usage()` headroom at startup.

- [ ] **Step 1: Write the failing test** — extract the per-tick logic into a pure helper `tick_frame(view, nav, feed, now, w, h) -> Vec<Line>` (or test `live_layout` composition is invoked) and an integration test that: writes a fixture run dir (run.json + events.jsonl with a run through a gate), builds a `RunView` via `from_run_dir`, builds a `NavState`, and asserts the frame (via `render_plain`) shows the gate footer when the gate is focused and the resume-generation guard does not exit on a stale terminal event. (The real crossterm/stdout loop is covered by the manual tty smoke, not CI.)
- [ ] **Step 2: Run to verify it fails.**
- [ ] **Step 3: Implement** the rewrite; DELETE `render_view`/`render_dashboard`/`render_graph`/`render_focus`/`LiveRunState`/`map_transcript_event`/`focused_transcript`/`handle_live_run_keypress` and the `AltScreenGuard` (moved to `render.rs`). Keep `refresh_run_usage`. Map crossterm `KeyCode` → `NavKey` in one `decode_key` fn (↑/k→Up, ↓/j→Down, Enter/→/l→In, ←/Backspace/h→Out, Tab→Down, a→Follow-or-Approve (gate-modal), q→Quit, Esc→Pause, /→Filter, r/v gate-modal). Gate-modal `a`/`r`/`v` take precedence only when a gate is focused.
- [ ] **Step 4: Run tests + `cargo build -p rupu-cli`.**
- [ ] **Step 5: Commit** — `feat(cli): live-view loop drives RunView+NavState+mux+renderer, in-view gate, resume guard`.

---

### Task 5: delete dead code + reconcile the spend footer + snapshot the frame shapes

**Files:**
- Modify: `crates/rupu-cli/src/output/live_run.rs` (remove now-unused imports/helpers), `workflow_printer.rs` only if a shared helper moved.
- Test: snapshot tests of representative full frames (`render_plain(&live_layout(...))`) at the gate, wide-fan-out, and completed states.

- [ ] Confirm no dead code remains (CI `-D warnings` would catch unused fns/imports after the rewrite — this is the likely failure if a helper was left behind). Run `cargo build -p rupu-cli` and grep for now-orphaned `render_dashboard`/`LiveRunState` references across the crate (the non-live retained-view path in `workflow_printer.rs` must NOT have depended on them — verify; if it did, keep what it needs).
- [ ] Add 2–3 whole-frame snapshot tests through the real `run_live_view` composition (headless) for the gate-parked, 86-unit fan-out, and completed frames; accept snapshots.
- [ ] Commit — `refactor(cli): remove the old full-reprint live-view path; snapshot the new frames`.

---

### Task 6: wiring + manual tty smoke gate

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` only if the `run_live_view` call signature changed (it should not).

- [ ] Confirm `run_workflow_with_live_view` still spawns the view as a sibling, aborts+awaits on the runner-done timeout (alt-screen restored before the Plan 1 `print_completion_summary` convergence prints), and that `live_view_enabled`/`RUPU_LIVE_VIEW` still gates it. No behavior change expected beyond the new `run_live_view` body.
- [ ] `cargo test -p rupu-cli` green; `cargo build` clean.
- [ ] **Manual tty smoke (REQUIRED pre-merge, human):** run a real short workflow and a wide-fan-out one; verify the live view renders without flicker, drill nav (enter/←) works, the breadcrumb tracks, the firehose ticks with codenames, a gate shows `a/r/v` and approve/reject resumes, `q` quits without pausing and `Esc` pauses, and the completion summary prints once after teardown. Record the result in the PR. (This is the "green ≠ rendering green" gate; it is matt's to run.)
- [ ] Commit any wiring tweak — `chore(cli): wire the rewritten live view into workflow run/resume`.

---

## Self-Review (run against the spec before handoff)

1. **Spec coverage:** firehose feed + pin (#5) → Tasks 1,2,4; diff renderer / flicker (part of #2 fix set) → Task 3; drill loop + keys + breadcrumb live → Task 4; in-view gate approve/reject (#10) → Task 4 (I1/I2); resume-generation guard → Task 4; fd-budget-bounded tails → Task 2. Every correctness fix not closed by Plans 1–2 is here. List any spec zone with no task.
2. **Placeholder scan:** Tasks 1–3 carry concrete interfaces + test shapes; Tasks 4–6 are integration with explicit delete-lists and a required human smoke. No "TBD".
3. **Type consistency:** `FeedLine`/`project_event`/`TranscriptMux`/`LiveRenderer`/`style_ansi`/`AltScreen` names are stable across tasks and match Plan 1/2 (`RunView`, `NavState`, `live_layout`, `Line`, `Style`).
4. **1.95 clippy hazards:** the `project_event` and `decode_key` matches and the diff loop are the hazards — guards not sole-ifs, no match-on-bool, no manual clamp. Called out in Global Constraints.
5. **Terminal safety:** the alt-screen RAII guard restores on every path including panic (Task 3).

## Done definition

Plans 1, 2, 3 merged; the CLI live view is the new drill-in global view with a real firehose, diff rendering, in-view gates, and correct accounting; the old full-reprint path is gone; and matt has run the tty smoke. This completes the workflow-CLI-UI redesign.
