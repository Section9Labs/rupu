//! `NavState` — the pure drill-navigation state machine for the redesigned
//! `workflow run` live view (spec 2026-09-30).
//!
//! Depth axis: run → step → unit → sub-agent. The cursor at each depth moves
//! over the in-view list for that depth (steps at `Run`, the filtered units
//! at `Step`, sub-agents at `Unit`/`SubAgent`). No terminal I/O: `output::live_run`
//! maps crossterm key events onto [`NavKey`] and feeds them to [`NavState::apply`]
//! together with the current [`RunView`]. A run grows steps / units /
//! dispatches between ticks, so every call re-clamps the cursors to the lists
//! that exist *now* and never indexes out of range.
//!
//! The dashboard adds a second axis: which [`Pane`] has focus (structure |
//! stream | firehose). Drill / move / filter keys act only on the structure
//! pane; with the stream or firehose focused the arrows scroll that pane
//! instead. The structure selection keeps driving the stream pane from any
//! focus, so changing focus never changes what the stream shows.

use crate::output::run_model::{DispatchView, GateView, RunView, StepView, UnitStatus, UnitView};
use rupu_orchestrator::runs::RunStatus;

/// A navigation input, already decoded from the terminal by Plan 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    /// Drill one level deeper.
    In,
    /// Ascend one level. A no-op at `Run` (never quits).
    Out,
    /// Resume following the newest activity (and return to `Run`).
    Follow,
    /// Cycle the fan-out unit filter.
    Filter,
    Quit,
    Pause,
    /// Focus the next pane (structure → stream → firehose → structure).
    PaneNext,
    /// Focus the previous pane.
    PanePrev,
    /// Scroll the focused stream / firehose pane toward older lines by a
    /// page. A no-op with the structure pane focused (it windows itself).
    ScrollUp,
    /// Scroll the focused stream / firehose pane toward the live tail by a
    /// page. A no-op with the structure pane focused.
    ScrollDown,
}

/// Which pane of the dashboard has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    /// The workflow structure graph — the drill/selection pane.
    #[default]
    Structure,
    /// The transcript of the structure selection.
    Stream,
    /// The run-wide event firehose.
    Firehose,
}

impl Pane {
    /// The next pane in `Structure → Stream → Firehose → Structure` order,
    /// or the previous one when `fwd` is false.
    fn cycled(self, fwd: bool) -> Pane {
        match (self, fwd) {
            (Pane::Structure, true) | (Pane::Firehose, false) => Pane::Stream,
            (Pane::Stream, true) | (Pane::Structure, false) => Pane::Firehose,
            (Pane::Firehose, true) | (Pane::Stream, false) => Pane::Structure,
        }
    }
}

/// Lines moved by one arrow press on a scrolling pane.
const SCROLL_LINE: usize = 1;
/// Lines moved by one [`NavKey::ScrollUp`] / [`NavKey::ScrollDown`].
const SCROLL_PAGE: usize = 10;

/// How deep the operator has drilled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Run,
    Step,
    Unit,
    SubAgent,
}

impl Depth {
    /// Position on the depth axis (`Run` = 0). Private: keeps `Depth`'s
    /// derive list to the contract without needing `Ord`.
    fn rank(self) -> u8 {
        match self {
            Depth::Run => 0,
            Depth::Step => 1,
            Depth::Unit => 2,
            Depth::SubAgent => 3,
        }
    }
}

/// Which fan-out units are in view at `Step` depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitFilter {
    All,
    Running,
    Failed,
    Done,
}

impl UnitFilter {
    /// Cycle `All → Running → Failed → Done → All`.
    pub fn next(self) -> UnitFilter {
        match self {
            UnitFilter::All => UnitFilter::Running,
            UnitFilter::Running => UnitFilter::Failed,
            UnitFilter::Failed => UnitFilter::Done,
            UnitFilter::Done => UnitFilter::All,
        }
    }

    fn admits(self, status: UnitStatus) -> bool {
        match self {
            UnitFilter::All => true,
            UnitFilter::Running => status == UnitStatus::Running,
            UnitFilter::Failed => status == UnitStatus::Failed,
            UnitFilter::Done => status == UnitStatus::Done,
        }
    }
}

/// What the caller must do after a key, beyond redrawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavAction {
    None,
    Quit,
    Pause,
}

/// Drill position + cursors + pane focus. Fields are private; construct with
/// [`NavState::default`] (`Run` depth, following, `All` filter, cursors 0,
/// structure pane focused, both scrolling panes at their live tail).
///
/// A scroll offset is the number of lines the pane is held *above its live
/// tail*: `0` shows the newest lines, larger values show older ones. Nav does
/// not know the buffer lengths, so the offset is only bounded below here; the
/// renderer, which does, must call [`NavState::clamp_scroll`] each frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavState {
    depth: Depth,
    step_idx: usize,
    unit_idx: usize,
    sub_idx: usize,
    follow: bool,
    filter: UnitFilter,
    pane: Pane,
    stream_scroll: usize,
    firehose_scroll: usize,
}

impl Default for NavState {
    fn default() -> Self {
        Self {
            depth: Depth::Run,
            step_idx: 0,
            unit_idx: 0,
            sub_idx: 0,
            follow: true,
            filter: UnitFilter::All,
            pane: Pane::Structure,
            stream_scroll: 0,
            firehose_scroll: 0,
        }
    }
}

/// Clamp `idx` into `0..len` (0 when the list is empty).
fn clamp_idx(idx: usize, len: usize) -> usize {
    idx.min(len.saturating_sub(1))
}

/// Move a cursor one slot, stopping at the ends (no wrap).
fn step_cursor(idx: usize, len: usize, down: bool) -> usize {
    if down {
        clamp_idx(idx.saturating_add(1), len)
    } else {
        clamp_idx(idx.saturating_sub(1), len)
    }
}

impl NavState {
    pub fn depth(&self) -> Depth {
        self.depth
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    pub fn filter(&self) -> UnitFilter {
        self.filter
    }

    /// The pane that currently has keyboard focus.
    pub fn pane(&self) -> Pane {
        self.pane
    }

    /// Move focus to the next pane, or the previous one when `fwd` is false
    /// (`Structure → Stream → Firehose → Structure`). Touches nothing else:
    /// selection, depth and follow are unaffected.
    pub fn cycle_pane(&mut self, fwd: bool) {
        self.pane = self.pane.cycled(fwd);
    }

    /// Lines `pane` is held above its live tail (`0` = pinned to the newest
    /// line). The structure pane windows itself and always answers `0`.
    pub fn scroll_offset(&self, pane: Pane) -> usize {
        match pane {
            Pane::Structure => 0,
            Pane::Stream => self.stream_scroll,
            Pane::Firehose => self.firehose_scroll,
        }
    }

    /// Bound `pane`'s offset to `max_offset` — the most lines the renderer
    /// can actually scroll back given the buffer it just drew (typically
    /// `total_lines - visible_rows`, `0` for an empty or short buffer). Call
    /// once per frame so an offset never runs past the real scrollback, and a
    /// pane whose buffer shrank snaps back instead of drawing blank. A no-op
    /// for the structure pane.
    pub fn clamp_scroll(&mut self, pane: Pane, max_offset: usize) {
        match pane {
            Pane::Structure => {}
            Pane::Stream => self.stream_scroll = self.stream_scroll.min(max_offset),
            Pane::Firehose => self.firehose_scroll = self.firehose_scroll.min(max_offset),
        }
    }

    /// Everything that decides which transcript the stream pane shows: the
    /// drill depth, the three cursors and the unit filter (a filter change
    /// can swap the unit under an unchanged cursor index).
    fn selection_key(&self) -> (Depth, usize, usize, usize, UnitFilter) {
        (
            self.depth,
            self.step_idx,
            self.unit_idx,
            self.sub_idx,
            self.filter,
        )
    }

    /// Scroll the focused pane `lines` toward older output (`older`) or the
    /// live tail. Saturates at the tail; the far end is the renderer's
    /// [`NavState::clamp_scroll`]. A no-op with the structure pane focused.
    fn scroll_focused(&mut self, older: bool, lines: usize) {
        let offset = match self.pane {
            Pane::Structure => return,
            Pane::Stream => &mut self.stream_scroll,
            Pane::Firehose => &mut self.firehose_scroll,
        };
        *offset = if older {
            offset.saturating_add(lines)
        } else {
            offset.saturating_sub(lines)
        };
    }

    /// The step under the cursor (valid at every depth).
    pub fn selected_step<'a>(&self, view: &'a RunView) -> Option<&'a StepView> {
        view.steps.get(clamp_idx(self.step_idx, view.steps.len()))
    }

    /// The step the operator has *chosen*, if any. While the view is
    /// following the newest activity at `Run` depth the step cursor is just
    /// parked on step 0 — that is not a selection, so nothing is chosen. Once
    /// the operator has moved the cursor or drilled in, the cursor step is
    /// the choice. While following, a parked run's first gate stands in for a
    /// choice ([`NavState::gate_step`]), so what `a` / `r` would act on is
    /// the thing marked. The one predicate behind every selection marker.
    pub fn chosen_step<'a>(&self, view: &'a RunView) -> Option<&'a StepView> {
        let chosen = !self.follow || self.depth != Depth::Run;
        self.selected_step(view)
            .filter(|_| chosen)
            .or_else(|| self.gate_step(view))
    }

    /// The unit under the cursor within the selected step's *filtered* list.
    pub fn selected_unit<'a>(&self, view: &'a RunView) -> Option<&'a UnitView> {
        let units = self.unit_list(view);
        units.get(clamp_idx(self.unit_idx, units.len())).copied()
    }

    /// The sub-agent under the cursor.
    pub fn selected_sub_agent<'a>(&self, view: &'a RunView) -> Option<&'a DispatchView> {
        let subs = self.sub_list(view);
        subs.get(clamp_idx(self.sub_idx, subs.len())).copied()
    }

    /// The parked-gate step an approve / reject would act on, if any. The ONE
    /// predicate behind both the footer legend (`a approve · r reject · …`) and
    /// the key dispatch, so the two can never disagree.
    ///
    /// Only an `AwaitingApproval` run has one, and only a step with an entry in
    /// the run's awaiting set (`view.gates`) is actionable — a step that merely
    /// looks parked is not. Once the operator has chosen a step (moved the
    /// cursor or drilled in) the gate is that step, if it is parked. While
    /// following the newest activity nothing is chosen, so the first parked
    /// gate in step order is focused automatically — a parked run must not
    /// need navigation before its keys appear.
    pub fn gate_step<'a>(&self, view: &'a RunView) -> Option<&'a StepView> {
        if view.status != RunStatus::AwaitingApproval {
            return None;
        }
        let parked = |s: &&StepView| view.gates.iter().any(|g| g.step_id == s.step_id);
        let chosen = !self.follow || self.depth != Depth::Run;
        if chosen {
            self.selected_step(view).filter(parked)
        } else {
            view.steps.iter().find(parked)
        }
    }

    /// The gate behind [`NavState::gate_step`]: what `a` / `r` / `v` act on.
    pub fn focused_gate<'a>(&self, view: &'a RunView) -> Option<&'a GateView> {
        let step = self.gate_step(view)?;
        view.gates.iter().find(|g| g.step_id == step.step_id)
    }

    /// The unit under the cursor within `step`'s *filtered* list. Same answer
    /// as [`NavState::selected_unit`] when `step` is the selected step; for
    /// renderers that hold only the step (the structure pane's fan-out block).
    pub fn selected_unit_in<'a>(&self, step: &'a StepView) -> Option<&'a UnitView> {
        let units = self.filtered_units(step);
        units.get(clamp_idx(self.unit_idx, units.len())).copied()
    }

    /// `step`'s units admitted by the current filter, in unit-index order —
    /// the list the unit cursor walks. One predicate for nav and renderers.
    pub fn filtered_units<'a>(&self, step: &'a StepView) -> Vec<&'a UnitView> {
        step.units
            .values()
            .filter(|u| self.filter.admits(u.status))
            .collect()
    }

    /// Filtered units of the selected step, in unit-index order.
    fn unit_list<'a>(&self, view: &'a RunView) -> Vec<&'a UnitView> {
        match self.selected_step(view) {
            Some(step) => self.filtered_units(step),
            None => Vec::new(),
        }
    }

    /// Sub-agents of the selected step: dispatches attributed to it. Plan 2
    /// treats every such dispatch as a candidate for the selected unit.
    fn sub_list<'a>(&self, view: &'a RunView) -> Vec<&'a DispatchView> {
        match self.selected_step(view) {
            Some(step) => view
                .dispatches
                .values()
                .filter(|d| d.parent_step_id.as_deref() == Some(step.step_id.as_str()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Re-clamp every cursor to the lists that exist now and pull `depth`
    /// back to the deepest level that still has something selectable. Only
    /// reacts to the view (or a filter change) — never to the key itself.
    fn settle(&mut self, view: &RunView) {
        self.step_idx = clamp_idx(self.step_idx, view.steps.len());
        let units_len = self.unit_list(view).len();
        self.unit_idx = clamp_idx(self.unit_idx, units_len);
        let subs_len = self.sub_list(view).len();
        self.sub_idx = clamp_idx(self.sub_idx, subs_len);

        // Clamp to the deepest level that is actually available. A level is
        // only reachable through its parent, so this cascades: e.g. a
        // sub-agent depth with no selectable unit drops all the way to Step,
        // never leaving a parentless sub-agent selected.
        let deepest = if view.steps.is_empty() {
            Depth::Run
        } else if units_len == 0 {
            Depth::Step
        } else if subs_len == 0 {
            Depth::Unit
        } else {
            Depth::SubAgent
        };
        if self.depth.rank() > deepest.rank() {
            self.depth = deepest;
        }
    }

    /// Reconcile depth/cursors against a possibly-changed view (call once per
    /// render tick before reading `depth()` / `selected_*`). Same logic as the
    /// internal settle applied on every [`NavState::apply`].
    pub fn sync(&mut self, view: &RunView) {
        self.settle(view);
    }

    /// The pure transition. Returns [`NavAction::Quit`] / [`NavAction::Pause`]
    /// for the two keys the caller must act on; everything else is
    /// [`NavAction::None`] after mutating `self`.
    ///
    /// Focus routing: `Quit` / `Pause` / `Follow` and the pane-cycle keys work
    /// from any pane. The drill / move / filter keys act only with the
    /// structure pane focused; with the stream or firehose focused `Up` /
    /// `Down` scroll that pane a line and `ScrollUp` / `ScrollDown` a page, and
    /// `In` / `Out` / `Filter` do nothing.
    pub fn apply(&mut self, key: NavKey, view: &RunView) -> NavAction {
        self.settle(view);
        let structure = self.pane == Pane::Structure;
        // What the stream pane is showing: any change to it invalidates the
        // stream's scroll offset (it indexed into the previous transcript).
        let shown = self.selection_key();
        let action = match key {
            NavKey::Quit => NavAction::Quit,
            NavKey::Pause => NavAction::Pause,
            NavKey::PaneNext => {
                self.cycle_pane(true);
                NavAction::None
            }
            NavKey::PanePrev => {
                self.cycle_pane(false);
                NavAction::None
            }
            NavKey::ScrollUp => {
                self.scroll_focused(true, SCROLL_PAGE);
                NavAction::None
            }
            NavKey::ScrollDown => {
                self.scroll_focused(false, SCROLL_PAGE);
                NavAction::None
            }
            NavKey::Up | NavKey::Down => {
                let down = key == NavKey::Down;
                if structure {
                    self.move_cursor(down, view);
                } else {
                    self.scroll_focused(!down, SCROLL_LINE);
                }
                NavAction::None
            }
            NavKey::In => {
                if structure {
                    self.drill_in(view);
                }
                NavAction::None
            }
            NavKey::Out => {
                if structure {
                    self.drill_out();
                }
                NavAction::None
            }
            NavKey::Follow => {
                self.follow = true;
                self.depth = Depth::Run;
                self.step_idx = 0;
                self.unit_idx = 0;
                self.sub_idx = 0;
                // Following means "back to live" everywhere.
                self.stream_scroll = 0;
                self.firehose_scroll = 0;
                NavAction::None
            }
            NavKey::Filter => {
                if structure {
                    self.filter = self.filter.next();
                    // The filtered list was rebuilt; start from its top.
                    self.unit_idx = 0;
                    self.sub_idx = 0;
                }
                NavAction::None
            }
        };
        // A filter change can empty the list the depth points into.
        self.settle(view);
        if self.selection_key() != shown {
            self.stream_scroll = 0;
        }
        action
    }

    /// Up/Down: move the cursor of the current depth's list. Any move —
    /// even one that stops at an end — stops following the newest activity.
    fn move_cursor(&mut self, down: bool, view: &RunView) {
        self.follow = false;
        match self.depth {
            Depth::Run => {
                self.step_idx = step_cursor(self.step_idx, view.steps.len(), down);
                // Cursors below belong to the previously selected step.
                self.unit_idx = 0;
                self.sub_idx = 0;
            }
            Depth::Step => {
                self.unit_idx = step_cursor(self.unit_idx, self.unit_list(view).len(), down);
                self.sub_idx = 0;
            }
            Depth::Unit | Depth::SubAgent => {
                self.sub_idx = step_cursor(self.sub_idx, self.sub_list(view).len(), down);
            }
        }
    }

    /// In: descend one level when there is something to descend into; a
    /// no-op at a leaf or when the next level is empty.
    fn drill_in(&mut self, view: &RunView) {
        match self.depth {
            Depth::Run if !view.steps.is_empty() => {
                self.depth = Depth::Step;
                self.unit_idx = 0;
                self.sub_idx = 0;
            }
            Depth::Step if !self.unit_list(view).is_empty() => {
                self.depth = Depth::Unit;
                self.sub_idx = 0;
            }
            Depth::Unit if !self.sub_list(view).is_empty() => {
                self.depth = Depth::SubAgent;
            }
            _ => {}
        }
    }

    /// Out: ascend one level; a no-op at `Run` (never quits).
    fn drill_out(&mut self) {
        self.depth = match self.depth {
            Depth::Run | Depth::Step => Depth::Run,
            Depth::Unit => Depth::Step,
            Depth::SubAgent => Depth::Unit,
        };
        if self.depth == Depth::Step {
            // Leaving a unit: the sub-agent cursor no longer applies.
            self.sub_idx = 0;
        }
    }

    /// Path to the current selection, truncated to the current depth:
    /// `[crew, step_id, unit codename-or-key, sub-agent codename]`. The crew
    /// crumb is omitted until the run's codename is known.
    pub fn breadcrumb(&self, view: &RunView) -> Vec<String> {
        let rank = self.depth.rank();
        let mut crumbs: Vec<String> = view.crew.iter().cloned().collect();

        let Some(step) = self.selected_step(view).filter(|_| rank >= 1) else {
            return crumbs;
        };
        crumbs.push(step.step_id.clone());

        let Some(unit) = self.selected_unit(view).filter(|_| rank >= 2) else {
            return crumbs;
        };
        crumbs.push(
            unit.codename
                .clone()
                .unwrap_or_else(|| unit.unit_key.clone()),
        );

        let Some(sub) = self.selected_sub_agent(view).filter(|_| rank >= 3) else {
            return crumbs;
        };
        crumbs.push(
            sub.codename
                .clone()
                .or_else(|| sub.agent.clone())
                .unwrap_or_else(|| sub.sub_run_id.clone()),
        );
        crumbs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::run_model::RunView;
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::{RunStatus, StepKind};

    fn fanout_view() -> RunView {
        let mut v = RunView::default();
        v.crew = Some("mint-tundra".into());
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            kind: StepKind::ForEach,
            agent: None,
            host: None,
            codename: None,
        });
        for i in 0..3usize {
            v.apply(&Event::UnitStarted {
                run_id: "r".into(),
                step_id: "hunt".into(),
                index: i,
                unit_key: format!("svc-{i}"),
                agent: Some("breaker".into()),
                transcript_path: format!("t{i}").into(),
                host: None,
                codename: Some(format!("otter#{}", i + 1)),
            });
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

    fn start_step(v: &mut RunView, step_id: &str, kind: StepKind) {
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: step_id.into(),
            kind,
            agent: None,
            host: None,
            codename: None,
        });
    }

    fn complete_unit(v: &mut RunView, step_id: &str, index: usize, success: bool) {
        v.apply(&Event::UnitCompleted {
            run_id: "r".into(),
            step_id: step_id.into(),
            index,
            unit_key: format!("svc-{index}"),
            success,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
            cause: None,
        });
    }

    fn dispatch(v: &mut RunView, sub_run_id: &str, codename: &str) {
        v.apply(&Event::DispatchStarted {
            run_id: "r".into(),
            sub_run_id: sub_run_id.into(),
            agent: Some("scout".into()),
            transcript_path: format!("t-{sub_run_id}").into(),
            codename: Some(codename.into()),
            provider: None,
            model: None,
        });
    }

    #[test]
    fn in_is_a_noop_when_there_is_nothing_to_descend_into() {
        // Empty run: In at Run stays at Run.
        let empty = RunView::default();
        let mut nav = NavState::default();
        assert_eq!(nav.apply(NavKey::In, &empty), NavAction::None);
        assert_eq!(nav.depth(), Depth::Run);

        // A linear step has no units: Run -> Step works, Step -> Unit does not.
        let mut v = RunView::default();
        start_step(&mut v, "plan", StepKind::Linear);
        assert_eq!(nav.apply(NavKey::In, &v), NavAction::None);
        assert_eq!(nav.depth(), Depth::Step);
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Step);
        assert!(nav.selected_unit(&v).is_none());

        // A unit with no sub-agents is a leaf for In.
        let f = fanout_view();
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &f);
        nav.apply(NavKey::In, &f);
        assert_eq!(nav.depth(), Depth::Unit);
        nav.apply(NavKey::In, &f);
        assert_eq!(nav.depth(), Depth::Unit);
    }

    #[test]
    fn stale_indices_clamp_when_the_view_changes_underneath() {
        let mut v = RunView::default();
        for id in ["a", "b", "c"] {
            start_step(&mut v, id, StepKind::Linear);
        }
        let mut nav = NavState::default();
        nav.apply(NavKey::Down, &v);
        nav.apply(NavKey::Down, &v);
        assert_eq!(nav.selected_step(&v).map(|s| s.step_id.as_str()), Some("c"));

        // The view is replaced by one with a single step (e.g. a resumed
        // generation replaying from scratch): no panic, cursor clamps.
        let mut shrunk = RunView::default();
        start_step(&mut shrunk, "a", StepKind::Linear);
        assert_eq!(
            nav.selected_step(&shrunk).map(|s| s.step_id.as_str()),
            Some("a")
        );
        nav.apply(NavKey::Up, &shrunk);
        nav.apply(NavKey::Down, &shrunk);
        assert_eq!(
            nav.selected_step(&shrunk).map(|s| s.step_id.as_str()),
            Some("a")
        );

        // Unit cursor clamps too, and an emptied view pulls depth back to Run.
        let f = fanout_view();
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &f);
        nav.apply(NavKey::Down, &f);
        nav.apply(NavKey::Down, &f);
        nav.apply(NavKey::In, &f);
        assert_eq!(nav.depth(), Depth::Unit);
        assert_eq!(
            nav.selected_unit(&f).map(|u| u.unit_key.as_str()),
            Some("svc-2")
        );
        let empty = RunView::default();
        assert_eq!(nav.apply(NavKey::Down, &empty), NavAction::None);
        assert_eq!(nav.depth(), Depth::Run);
        assert!(nav.selected_step(&empty).is_none());
        assert!(nav.selected_unit(&empty).is_none());
    }

    #[test]
    fn unit_cursor_walks_the_filtered_list_and_empty_filter_pops_depth() {
        let mut v = fanout_view();
        complete_unit(&mut v, "hunt", 0, true); // svc-0 Done
        complete_unit(&mut v, "hunt", 1, false); // svc-1 Failed; svc-2 Running
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &v);
        let key = |nav: &NavState| {
            nav.selected_unit(&v)
                .map(|u| u.unit_key.clone())
                .unwrap_or_default()
        };
        assert_eq!(key(&nav), "svc-0");
        nav.apply(NavKey::Down, &v);
        assert_eq!(key(&nav), "svc-1");

        nav.apply(NavKey::Filter, &v); // Running
        assert_eq!(nav.filter(), UnitFilter::Running);
        assert_eq!(key(&nav), "svc-2");
        nav.apply(NavKey::Filter, &v); // Failed
        assert_eq!(key(&nav), "svc-1");
        nav.apply(NavKey::Filter, &v); // Done
        assert_eq!(key(&nav), "svc-0");
        nav.apply(NavKey::Down, &v); // stops at the end of a one-item list
        assert_eq!(key(&nav), "svc-0");
        nav.apply(NavKey::Filter, &v); // back to All
        assert_eq!(nav.filter(), UnitFilter::All);

        // Drilled into a unit, then a filter that matches nothing pops back
        // to Step rather than leaving the cursor on a vanished unit.
        let mut all_running = fanout_view();
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &all_running);
        nav.apply(NavKey::In, &all_running);
        assert_eq!(nav.depth(), Depth::Unit);
        nav.apply(NavKey::Filter, &all_running); // Running: all 3 match
        assert_eq!(nav.depth(), Depth::Unit);
        nav.apply(NavKey::Filter, &all_running); // Failed: none match
        assert_eq!(nav.depth(), Depth::Step);
        assert!(nav.selected_unit(&all_running).is_none());
        // In is a no-op while the filtered list is empty.
        nav.apply(NavKey::In, &all_running);
        assert_eq!(nav.depth(), Depth::Step);
        // Units growing into the filter makes it descendable again.
        complete_unit(&mut all_running, "hunt", 0, false);
        nav.apply(NavKey::In, &all_running);
        assert_eq!(nav.depth(), Depth::Unit);
    }

    #[test]
    fn sub_agents_are_the_dispatches_of_the_selected_step() {
        let mut v = fanout_view();
        dispatch(&mut v, "sub1", "wren#1");
        dispatch(&mut v, "sub2", "wren#2");
        // A later step's dispatch is not a sub-agent of `hunt`.
        start_step(&mut v, "later", StepKind::Linear);
        dispatch(&mut v, "sub3", "kite#1");

        let mut nav = NavState::default();
        for _ in 0..3 {
            nav.apply(NavKey::In, &v);
        }
        assert_eq!(nav.depth(), Depth::SubAgent);
        assert_eq!(
            nav.breadcrumb(&v),
            vec!["mint-tundra", "hunt", "otter#1", "wren#1"]
        );
        nav.apply(NavKey::Down, &v);
        assert_eq!(
            nav.breadcrumb(&v),
            vec!["mint-tundra", "hunt", "otter#1", "wren#2"]
        );
        nav.apply(NavKey::Down, &v); // only two sub-agents: clamps
        assert_eq!(
            nav.breadcrumb(&v).last().map(String::as_str),
            Some("wren#2")
        );
        // In at a leaf is a no-op.
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::SubAgent);
        nav.apply(NavKey::Out, &v);
        assert_eq!(nav.depth(), Depth::Unit);
        assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra", "hunt", "otter#1"]);
    }

    #[test]
    fn follow_returns_to_run_depth_and_breadcrumb_truncates() {
        let v = fanout_view();
        let mut nav = NavState::default();
        assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra"]);
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra", "hunt"]);
        nav.apply(NavKey::In, &v);
        nav.apply(NavKey::Down, &v);
        assert!(!nav.is_following());
        nav.apply(NavKey::Follow, &v);
        assert!(nav.is_following());
        assert_eq!(nav.depth(), Depth::Run);
        assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra"]);

        // Unit with no codename falls back to its key; no crew omits the crumb.
        let mut bare = RunView::default();
        start_step(&mut bare, "hunt", StepKind::ForEach);
        bare.apply(&Event::UnitStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 0,
            unit_key: "svc-0".into(),
            agent: None,
            transcript_path: "t".into(),
            host: None,
            codename: None,
        });
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &bare);
        nav.apply(NavKey::In, &bare);
        assert_eq!(nav.breadcrumb(&bare), vec!["hunt", "svc-0"]);
    }

    #[test]
    fn unit_filter_cycles_through_all_four_and_back() {
        let mut f = UnitFilter::All;
        let mut seen = Vec::new();
        for _ in 0..5 {
            f = f.next();
            seen.push(f);
        }
        assert_eq!(
            seen,
            vec![
                UnitFilter::Running,
                UnitFilter::Failed,
                UnitFilter::Done,
                UnitFilter::All,
                UnitFilter::Running
            ]
        );
    }

    #[test]
    fn settle_demotes_subagent_when_units_empty() {
        let mut v = fanout_view();
        dispatch(&mut v, "sub1", "wren#1");
        let mut nav = NavState::default();
        for _ in 0..3 {
            nav.apply(NavKey::In, &v);
        }
        assert_eq!(nav.depth(), Depth::SubAgent);

        // Running: all three units still match, sub-agent stays selectable.
        nav.apply(NavKey::Filter, &v);
        assert_eq!(nav.filter(), UnitFilter::Running);
        assert_eq!(nav.depth(), Depth::SubAgent);

        // Failed: no unit matches while a sub-agent still exists. Depth must
        // cascade all the way to Step, not stay on a parentless sub-agent.
        nav.apply(NavKey::Filter, &v);
        assert_eq!(nav.filter(), UnitFilter::Failed);
        assert!(nav.selected_unit(&v).is_none());
        assert_eq!(nav.depth(), Depth::Step);
        assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra", "hunt"]);

        // Every running unit completing under filter=Running does the same
        // without any keypress changing the filter.
        let mut v = fanout_view();
        dispatch(&mut v, "sub1", "wren#1");
        let mut nav = NavState::default();
        for _ in 0..3 {
            nav.apply(NavKey::In, &v);
        }
        nav.apply(NavKey::Filter, &v); // Running
        assert_eq!(nav.depth(), Depth::SubAgent);
        for i in 0..3 {
            complete_unit(&mut v, "hunt", i, true);
        }
        nav.sync(&v);
        assert_eq!(nav.depth(), Depth::Step);
        assert!(nav.selected_unit(&v).is_none());
    }

    #[test]
    fn step_scoped_accessors_agree_with_the_view_scoped_ones() {
        // The structure pane's fan-out block only holds the step, so nav must
        // answer from it.
        let mut v = fanout_view();
        complete_unit(&mut v, "hunt", 0, true); // svc-0 Done
        complete_unit(&mut v, "hunt", 1, false); // svc-1 Failed; svc-2 Running
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &v);
        let step = &v.steps[0];
        let keys = |nav: &NavState| -> Vec<String> {
            nav.filtered_units(step)
                .iter()
                .map(|u| u.unit_key.clone())
                .collect()
        };
        assert_eq!(keys(&nav), vec!["svc-0", "svc-1", "svc-2"]);
        nav.apply(NavKey::Down, &v);
        assert_eq!(
            nav.selected_unit_in(step).map(|u| u.index),
            nav.selected_unit(&v).map(|u| u.index)
        );
        assert_eq!(nav.selected_unit_in(step).map(|u| u.index), Some(1));

        nav.apply(NavKey::Filter, &v); // Running
        assert_eq!(keys(&nav), vec!["svc-2"]);
        nav.apply(NavKey::Filter, &v); // Failed
        assert_eq!(keys(&nav), vec!["svc-1"]);
        assert_eq!(nav.selected_unit_in(step).map(|u| u.index), Some(1));

        // A stale cursor clamps instead of indexing out of range, and a
        // step with no matching units has no selection.
        nav.apply(NavKey::Filter, &v); // Done
        nav.apply(NavKey::Filter, &v); // All
        let empty = RunView::default();
        let mut bare = RunView::default();
        start_step(&mut bare, "plan", StepKind::Linear);
        assert!(nav.selected_unit_in(&bare.steps[0]).is_none());
        assert!(nav.filtered_units(&bare.steps[0]).is_empty());
        assert!(empty.steps.is_empty());
    }

    #[test]
    fn sync_reconciles_depth_on_view_change() {
        let f = fanout_view();
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &f);
        nav.apply(NavKey::In, &f);
        assert_eq!(nav.depth(), Depth::Unit);

        // Same step id, but the new view's step has no units.
        let mut no_units = RunView::default();
        start_step(&mut no_units, "hunt", StepKind::Linear);
        nav.sync(&no_units);
        assert_eq!(nav.depth(), Depth::Step);
        assert!(nav.selected_unit(&no_units).is_none());

        // A view with no steps at all pulls depth back to Run.
        nav.sync(&RunView::default());
        assert_eq!(nav.depth(), Depth::Run);

        // sync never promotes: a consistent view leaves depth alone.
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &f);
        nav.sync(&f);
        assert_eq!(nav.depth(), Depth::Step);
    }

    // ── gate focus (Plan 3, I1) ─────────────────────────────────────────

    /// `build` done, `gate_a` / `gate_b` parked, `deploy` pending; both
    /// gates are in `view.gates` (run.json's awaiting set) when `parked`.
    fn parked_view(two_gates: bool) -> RunView {
        use crate::output::run_model::{GateView, StepState};
        let mut v = RunView::default();
        v.status = RunStatus::AwaitingApproval;
        start_step(&mut v, "build", StepKind::Linear);
        v.step_mut("build").state = StepState::Complete;
        let mut gates = vec!["gate_a"];
        if two_gates {
            gates.push("gate_b");
        }
        for id in &gates {
            let s = v.step_mut(id);
            s.kind = StepKind::ApprovalGate;
            s.state = StepState::AwaitingApproval;
        }
        v.step_mut("deploy");
        v.gates = gates
            .into_iter()
            .map(|id| GateView {
                step_id: id.into(),
                prompt: Some("publish?".into()),
                since: chrono::Utc::now(),
                expires_at: None,
            })
            .collect();
        v
    }

    fn gate_id(nav: &NavState, v: &RunView) -> Option<String> {
        nav.focused_gate(v).map(|g| g.step_id.clone())
    }

    #[test]
    fn a_parked_gate_is_auto_focused_while_following() {
        let v = parked_view(true);
        let nav = NavState::default();
        assert!(nav.is_following());
        // No manual selection: the FIRST parked gate (step order) is focused.
        assert_eq!(gate_id(&nav, &v).as_deref(), Some("gate_a"));
        assert_eq!(
            nav.gate_step(&v).map(|s| s.step_id.as_str()),
            Some("gate_a")
        );
    }

    #[test]
    fn a_manual_selection_decides_the_focused_gate() {
        let v = parked_view(true);
        // Down x1 -> `gate_a`, x2 -> `gate_b`; x0..: following parks on step 0.
        let mut nav = NavState::default();
        nav.apply(NavKey::Down, &v); // step 1 = gate_a (a move stops following)
        assert!(!nav.is_following());
        assert_eq!(gate_id(&nav, &v).as_deref(), Some("gate_a"));
        nav.apply(NavKey::Down, &v);
        assert_eq!(gate_id(&nav, &v).as_deref(), Some("gate_b"));

        // Selecting a step that is NOT a parked gate: nothing is focused —
        // the navigation keys apply, not approve/reject.
        nav.apply(NavKey::Down, &v);
        assert_eq!(
            nav.selected_step(&v).map(|s| s.step_id.as_str()),
            Some("deploy")
        );
        assert_eq!(gate_id(&nav, &v), None);
        nav.apply(NavKey::Up, &v);
        nav.apply(NavKey::Up, &v);
        nav.apply(NavKey::Up, &v);
        assert_eq!(
            nav.selected_step(&v).map(|s| s.step_id.as_str()),
            Some("build")
        );
        assert_eq!(gate_id(&nav, &v), None);

        // `a` (Follow) drops the manual selection and re-focuses the first gate.
        nav.apply(NavKey::Follow, &v);
        assert_eq!(gate_id(&nav, &v).as_deref(), Some("gate_a"));
    }

    #[test]
    fn chosen_step_is_only_a_real_choice_or_the_parked_gate() {
        let chosen = |nav: &NavState, v: &RunView| nav.chosen_step(v).map(|s| s.step_id.clone());

        // Following with nothing parked: the cursor idles on step 0 — that is
        // not a choice.
        let mut v = parked_view(true);
        v.status = RunStatus::Running;
        v.gates.clear();
        assert_eq!(chosen(&NavState::default(), &v), None);

        // Following a parked run: the first gate stands in for a choice.
        let v = parked_view(true);
        assert_eq!(chosen(&NavState::default(), &v).as_deref(), Some("gate_a"));

        // A manual move chooses the cursor step, parked or not.
        let mut nav = NavState::default();
        nav.apply(NavKey::Up, &v); // stops following, cursor stays on `build`
        assert_eq!(chosen(&nav, &v).as_deref(), Some("build"));
    }

    #[test]
    fn a_drilled_gate_stays_focused_without_following() {
        let v = parked_view(false);
        let mut nav = NavState::default();
        nav.apply(NavKey::Down, &v); // gate_a
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Step);
        assert_eq!(gate_id(&nav, &v).as_deref(), Some("gate_a"));
    }

    #[test]
    fn no_gate_is_focused_unless_the_run_is_awaiting_and_the_gate_is_actionable() {
        // The run is not awaiting approval (e.g. a pause left a stale record):
        let mut v = parked_view(true);
        v.status = RunStatus::Running;
        assert_eq!(gate_id(&NavState::default(), &v), None);

        // A step that merely LOOKS parked but has no entry in run.json's
        // awaiting set is not actionable: no approve/reject advertised.
        let mut v = parked_view(true);
        v.gates.clear();
        assert_eq!(gate_id(&NavState::default(), &v), None);
        let mut nav = NavState::default();
        nav.apply(NavKey::Down, &v);
        assert_eq!(gate_id(&nav, &v), None);

        // A decided gate drops out of the set: the other one takes the focus.
        let mut v = parked_view(true);
        v.gates.retain(|g| g.step_id != "gate_a");
        assert_eq!(gate_id(&NavState::default(), &v).as_deref(), Some("gate_b"));
    }

    // ── pane focus + per-pane scroll (dashboard Task 6) ─────────────────

    fn three_steps() -> RunView {
        let mut v = RunView::default();
        for id in ["a", "b", "c"] {
            start_step(&mut v, id, StepKind::Linear);
        }
        v
    }

    /// A fresh `NavState` with `pane` focused.
    fn focused(pane: Pane) -> NavState {
        NavState {
            pane,
            ..NavState::default()
        }
    }

    fn sel(nav: &NavState, v: &RunView) -> Option<String> {
        nav.selected_step(v).map(|s| s.step_id.clone())
    }

    #[test]
    fn tab_cycles_structure_stream_firehose_and_back() {
        let v = fanout_view();
        let mut nav = NavState::default();
        assert_eq!(nav.pane(), Pane::Structure);

        for want in [Pane::Stream, Pane::Firehose, Pane::Structure] {
            assert_eq!(nav.apply(NavKey::PaneNext, &v), NavAction::None);
            assert_eq!(nav.pane(), want);
        }
        // Reverse walks the same ring the other way.
        for want in [Pane::Firehose, Pane::Stream, Pane::Structure] {
            assert_eq!(nav.apply(NavKey::PanePrev, &v), NavAction::None);
            assert_eq!(nav.pane(), want);
        }

        // The direct accessor agrees with the keys.
        nav.cycle_pane(true);
        assert_eq!(nav.pane(), Pane::Stream);
        nav.cycle_pane(false);
        assert_eq!(nav.pane(), Pane::Structure);
    }

    #[test]
    fn pane_focus_does_not_touch_selection_follow_or_depth() {
        let v = three_steps();
        let mut nav = NavState::default();
        assert!(nav.is_following());
        nav.apply(NavKey::PaneNext, &v);
        nav.apply(NavKey::PaneNext, &v);
        nav.apply(NavKey::PanePrev, &v);
        assert!(nav.is_following());
        assert_eq!(nav.depth(), Depth::Run);
        assert_eq!(sel(&nav, &v).as_deref(), Some("a"));
    }

    #[test]
    fn structure_keys_move_and_drill_only_in_structure_focus() {
        let v = three_steps();
        let mut nav = NavState::default();
        nav.apply(NavKey::Down, &v);
        assert_eq!(sel(&nav, &v).as_deref(), Some("b"));

        for pane in [Pane::Stream, Pane::Firehose] {
            nav.pane = pane;
            let before = nav;
            for key in [
                NavKey::Down,
                NavKey::Up,
                NavKey::In,
                NavKey::Out,
                NavKey::Filter,
            ] {
                nav.apply(key, &v);
                assert_eq!(sel(&nav, &v).as_deref(), Some("b"), "{key:?} in {pane:?}");
                assert_eq!(nav.depth(), before.depth(), "{key:?} in {pane:?}");
                assert_eq!(nav.filter(), before.filter(), "{key:?} in {pane:?}");
            }
        }

        // Back in Structure focus the same keys act again.
        nav.pane = Pane::Structure;
        nav.apply(NavKey::Down, &v);
        assert_eq!(sel(&nav, &v).as_deref(), Some("c"));
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Step);
        nav.apply(NavKey::Filter, &v);
        assert_ne!(nav.filter(), UnitFilter::All);
    }

    #[test]
    fn out_ascends_only_with_the_structure_pane_focused() {
        // `Out` at `Run` is a no-op anyway, so the gate must be proven at a
        // depth where `Out` really ascends: drill to `Unit`, then move focus
        // off the structure pane. Removing the `structure` gate on `Out`
        // makes this fail (the depth would drop to `Step`).
        let v = fanout_view();
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &v);
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Unit);

        for pane in [Pane::Stream, Pane::Firehose] {
            nav.pane = pane;
            assert_eq!(nav.apply(NavKey::Out, &v), NavAction::None);
            assert_eq!(nav.depth(), Depth::Unit, "Out in {pane:?} must not ascend");
            assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra", "hunt", "otter#1"]);
        }

        // Back on the structure pane the same key ascends.
        nav.pane = Pane::Structure;
        nav.apply(NavKey::Out, &v);
        assert_eq!(nav.depth(), Depth::Step);
    }

    #[test]
    fn a_move_key_outside_structure_focus_does_not_stop_following() {
        // `move_cursor` clears `follow`; an arrow that only scrolls the
        // stream must not, or the run view would stop tracking the frontier.
        let v = three_steps();
        let mut nav = NavState::default();
        nav.apply(NavKey::PaneNext, &v);
        nav.apply(NavKey::Down, &v);
        nav.apply(NavKey::Up, &v);
        assert!(nav.is_following());
    }

    #[test]
    fn arrows_scroll_the_focused_pane_and_stop_at_the_live_tail() {
        let v = three_steps();
        let mut nav = NavState::default();
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
        assert_eq!(nav.scroll_offset(Pane::Firehose), 0);

        nav.apply(NavKey::PaneNext, &v); // Stream
        nav.apply(NavKey::Up, &v); // toward older lines
        nav.apply(NavKey::Up, &v);
        assert_eq!(nav.scroll_offset(Pane::Stream), 2);
        assert_eq!(nav.scroll_offset(Pane::Firehose), 0);
        nav.apply(NavKey::Down, &v); // toward the tail
        assert_eq!(nav.scroll_offset(Pane::Stream), 1);
        nav.apply(NavKey::Down, &v);
        nav.apply(NavKey::Down, &v); // already at the tail: stays put
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);

        // Firehose keeps its own offset, and offsets survive a focus change.
        nav.apply(NavKey::Up, &v);
        nav.apply(NavKey::PaneNext, &v); // Firehose
        nav.apply(NavKey::Up, &v);
        nav.apply(NavKey::Up, &v);
        nav.apply(NavKey::Up, &v);
        assert_eq!(nav.scroll_offset(Pane::Firehose), 3);
        assert_eq!(nav.scroll_offset(Pane::Stream), 1);
        // Structure has no scroll offset of its own.
        assert_eq!(nav.scroll_offset(Pane::Structure), 0);
    }

    #[test]
    fn scroll_keys_scroll_only_the_focused_pane() {
        let v = three_steps();
        let mut nav = NavState::default();

        // Structure focus: its windowing is its own, so scroll keys are inert.
        nav.apply(NavKey::ScrollUp, &v);
        nav.apply(NavKey::ScrollDown, &v);
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
        assert_eq!(nav.scroll_offset(Pane::Firehose), 0);
        assert_eq!(sel(&nav, &v).as_deref(), Some("a"));
        assert!(nav.is_following());

        nav.apply(NavKey::PaneNext, &v); // Stream
        nav.apply(NavKey::ScrollUp, &v);
        let up = nav.scroll_offset(Pane::Stream);
        assert!(up > 0, "ScrollUp raises the stream offset");
        assert_eq!(nav.scroll_offset(Pane::Firehose), 0);
        nav.apply(NavKey::ScrollUp, &v);
        assert!(nav.scroll_offset(Pane::Stream) > up);
        nav.apply(NavKey::ScrollDown, &v);
        assert_eq!(nav.scroll_offset(Pane::Stream), up);
        // ScrollDown saturates at the tail.
        for _ in 0..4 {
            nav.apply(NavKey::ScrollDown, &v);
        }
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);

        nav.apply(NavKey::PaneNext, &v); // Firehose
        nav.apply(NavKey::ScrollUp, &v);
        assert!(nav.scroll_offset(Pane::Firehose) > 0);
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
        // Scrolling never moves the structure selection.
        assert_eq!(sel(&nav, &v).as_deref(), Some("a"));
    }

    #[test]
    fn clamp_scroll_bounds_the_offset_to_the_real_buffer() {
        let v = three_steps();
        let mut nav = NavState::default();
        nav.apply(NavKey::PaneNext, &v);
        for _ in 0..50 {
            nav.apply(NavKey::ScrollUp, &v);
        }
        nav.clamp_scroll(Pane::Stream, 7);
        assert_eq!(nav.scroll_offset(Pane::Stream), 7);
        // Already within bounds: untouched. Other panes: untouched.
        nav.clamp_scroll(Pane::Stream, 100);
        assert_eq!(nav.scroll_offset(Pane::Stream), 7);
        nav.clamp_scroll(Pane::Firehose, 0);
        assert_eq!(nav.scroll_offset(Pane::Stream), 7);
        // An emptied buffer snaps back to the tail.
        nav.clamp_scroll(Pane::Stream, 0);
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
        // Structure never carries an offset.
        nav.clamp_scroll(Pane::Structure, 5);
        assert_eq!(nav.scroll_offset(Pane::Structure), 0);
    }

    #[test]
    fn changing_the_selection_returns_the_stream_to_the_tail() {
        // The stream pane follows the structure selection, so an offset into
        // one transcript means nothing against another.
        let v = three_steps();
        let mut nav = NavState::default();
        nav.apply(NavKey::PaneNext, &v); // Stream
        nav.apply(NavKey::ScrollUp, &v);
        nav.apply(NavKey::ScrollUp, &v);
        assert!(nav.scroll_offset(Pane::Stream) > 0);
        nav.apply(NavKey::PanePrev, &v); // Structure
        assert!(nav.scroll_offset(Pane::Stream) > 0, "focus alone keeps it");
        nav.apply(NavKey::Down, &v);
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);

        // A move that stops at the end of the list changes nothing, so the
        // operator's scroll position is kept.
        nav.apply(NavKey::Down, &v); // c
        nav.apply(NavKey::PaneNext, &v);
        nav.apply(NavKey::ScrollUp, &v);
        let held = nav.scroll_offset(Pane::Stream);
        nav.apply(NavKey::PanePrev, &v);
        nav.apply(NavKey::Down, &v); // already on c
        assert_eq!(nav.scroll_offset(Pane::Stream), held);
    }

    #[test]
    fn drilling_or_filtering_also_returns_the_stream_to_the_tail() {
        // Neither changes a cursor index on a one-step run, but both change
        // which transcript the stream pane is showing.
        let v = fanout_view();
        let mut nav = NavState::default();
        let scrolled = |nav: &mut NavState| {
            nav.apply(NavKey::PaneNext, &v);
            nav.apply(NavKey::ScrollUp, &v);
            nav.apply(NavKey::PanePrev, &v);
            assert!(nav.scroll_offset(Pane::Stream) > 0);
        };
        scrolled(&mut nav);
        nav.apply(NavKey::In, &v); // Run -> Step: cursors unchanged
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
        scrolled(&mut nav);
        nav.apply(NavKey::Filter, &v);
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
        scrolled(&mut nav);
        nav.apply(NavKey::Out, &v);
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
    }

    #[test]
    fn selection_still_drives_the_stream_from_any_focus() {
        let v = fanout_view();
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &v);
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Unit);
        let unit = |nav: &NavState| nav.selected_unit(&v).map(|u| u.unit_key.clone());
        assert_eq!(unit(&nav).as_deref(), Some("svc-0"));

        // Focus the stream: the unit it would show is unchanged, and arrows
        // scroll rather than walk to svc-1.
        nav.apply(NavKey::PaneNext, &v);
        nav.apply(NavKey::Down, &v);
        nav.apply(NavKey::Up, &v);
        assert_eq!(unit(&nav).as_deref(), Some("svc-0"));
        assert_eq!(nav.depth(), Depth::Unit);
        assert_eq!(nav.breadcrumb(&v), vec!["mint-tundra", "hunt", "otter#1"]);

        // Return to Structure: drilling and moving work as before (the unit
        // cursor walks units at Step depth).
        nav.apply(NavKey::PanePrev, &v);
        nav.apply(NavKey::Out, &v);
        assert_eq!(nav.depth(), Depth::Step);
        nav.apply(NavKey::Down, &v);
        assert_eq!(unit(&nav).as_deref(), Some("svc-1"));
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Unit);
    }

    #[test]
    fn follow_returns_every_pane_to_the_tail_without_changing_focus() {
        let v = three_steps();
        let mut nav = NavState::default();
        nav.apply(NavKey::PaneNext, &v); // Stream
        nav.apply(NavKey::ScrollUp, &v);
        nav.apply(NavKey::PaneNext, &v); // Firehose
        nav.apply(NavKey::ScrollUp, &v);
        assert!(nav.scroll_offset(Pane::Stream) > 0);
        assert!(nav.scroll_offset(Pane::Firehose) > 0);

        nav.apply(NavKey::Follow, &v);
        assert!(nav.is_following());
        assert_eq!(nav.scroll_offset(Pane::Stream), 0);
        assert_eq!(nav.scroll_offset(Pane::Firehose), 0);
        assert_eq!(nav.pane(), Pane::Firehose);
    }

    #[test]
    fn quit_and_pause_work_from_every_pane() {
        let v = three_steps();
        for pane in [Pane::Structure, Pane::Stream, Pane::Firehose] {
            let mut nav = focused(pane);
            assert_eq!(nav.apply(NavKey::Quit, &v), NavAction::Quit);
            assert_eq!(nav.apply(NavKey::Pause, &v), NavAction::Pause);
        }
    }

    #[test]
    fn every_key_in_every_pane_is_panic_free_on_empty_and_shrunken_views() {
        let keys = [
            NavKey::Up,
            NavKey::Down,
            NavKey::In,
            NavKey::Out,
            NavKey::Follow,
            NavKey::Filter,
            NavKey::Quit,
            NavKey::Pause,
            NavKey::PaneNext,
            NavKey::PanePrev,
            NavKey::ScrollUp,
            NavKey::ScrollDown,
        ];
        let views = [RunView::default(), three_steps(), fanout_view()];
        for view in &views {
            for pane in [Pane::Structure, Pane::Stream, Pane::Firehose] {
                for key in keys {
                    let mut nav = focused(pane);
                    nav.apply(key, view);
                    // Drive it on a different view afterwards.
                    nav.apply(key, &RunView::default());
                    nav.sync(&RunView::default());
                }
            }
        }
    }
}
