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
}

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

/// Drill position + cursors. Fields are private; construct with
/// [`NavState::default`] (`Run` depth, following, `All` filter, cursors 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavState {
    depth: Depth,
    step_idx: usize,
    unit_idx: usize,
    sub_idx: usize,
    follow: bool,
    filter: UnitFilter,
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

    /// The step under the cursor (valid at every depth).
    pub fn selected_step<'a>(&self, view: &'a RunView) -> Option<&'a StepView> {
        view.steps.get(clamp_idx(self.step_idx, view.steps.len()))
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
    /// renderers that hold only the step (`layout::fanout_block`).
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
    pub fn apply(&mut self, key: NavKey, view: &RunView) -> NavAction {
        self.settle(view);
        let action = match key {
            NavKey::Quit => NavAction::Quit,
            NavKey::Pause => NavAction::Pause,
            NavKey::Up => {
                self.move_cursor(false, view);
                NavAction::None
            }
            NavKey::Down => {
                self.move_cursor(true, view);
                NavAction::None
            }
            NavKey::In => {
                self.drill_in(view);
                NavAction::None
            }
            NavKey::Out => {
                self.drill_out();
                NavAction::None
            }
            NavKey::Follow => {
                self.follow = true;
                self.depth = Depth::Run;
                self.step_idx = 0;
                self.unit_idx = 0;
                self.sub_idx = 0;
                NavAction::None
            }
            NavKey::Filter => {
                self.filter = self.filter.next();
                // The filtered list was rebuilt; start from its top.
                self.unit_idx = 0;
                self.sub_idx = 0;
                NavAction::None
            }
        };
        // A filter change can empty the list the depth points into.
        self.settle(view);
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
        // `fanout_block` only holds the step, so nav must answer from it.
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
}
