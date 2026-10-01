//! The live `workflow run` view — the tick loop behind the in-place terminal
//! surface (spec 2026-09-30).
//!
//! [`run_live_view`] drives, every 100 ms: the run model ([`RunView`], folded
//! from `events.jsonl` plus the few things only `run.json` knows), the drill
//! navigation ([`NavState`]), the bounded transcript firehose
//! ([`TranscriptMux`]), the three-pane dashboard frame
//! ([`dashboard_frame`]) and the diff renderer ([`LiveRenderer`]). Keys are
//! decoded in one place
//! ([`decode_key`]) and an approval gate is *modal*: while one is focused
//! (`NavState::focused_gate` — the same predicate the footer legend uses)
//! `a` approves, `r` rejects and `v` shows what the run found.
//!
//! The terminal loop itself is validated by running it. Everything it
//! decides — key decoding, gate focus, which transcript to pin, when to leave,
//! what a gate decision does to the run store — lives in small pure functions
//! below, and is unit-tested.

use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use rupu_orchestrator::executor::Event as WfEvent;
use rupu_orchestrator::{
    ApprovalDecision, ApprovalError, RunRecord, RunStatus, RunStore, Step, StepKind,
    StepResultRecord, TimeoutAction, Workflow,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::output::jsonl_reader::WfEventTailer;
use crate::output::live_view::gate::{gate_detail_lines, GateFinding};
use crate::output::live_view::layout::printable;
use crate::output::live_view::mux::TranscriptMux;
use crate::output::live_view::nav::{Depth, NavAction, NavKey, NavState, Pane};
use crate::output::live_view::panes::{dashboard_frame, scroll_rows};
use crate::output::live_view::render::{AltScreen, LiveRenderer};
use crate::output::live_view::row::Line;
use crate::output::run_model::{GateView, RunView};

/// Repaint cadence.
const TICK: Duration = Duration::from_millis(100);
/// How often the slow, file-reading parts of the view (usage fold,
/// `step_results.jsonl`) are re-read.
const SLOW_REFRESH_EVERY: Duration = Duration::from_secs(1);
/// How long a one-line notice (pause requested, gate decided, …) stays up.
const NOTICE_TTL: Duration = Duration::from_secs(8);
/// Columns left unwritten at the right edge. A row that fills the last column
/// leaves the terminal in its deferred-wrap state, and the renderer's
/// clear-to-end-of-line then erases that last glyph on common emulators.
const EDGE_GUARD: usize = 1;
/// Tails the mux gets when the fd budget cannot be read.
const DEFAULT_TAILS: usize = 16;
/// Reason recorded when the operator rejects a gate from the view (the CLI's
/// `workflow reject` default).
const REJECT_REASON: &str = "rejected by operator";

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// What a keypress asks the loop to do. Navigation goes through
/// [`NavState::apply`]; the other three are the modal gate keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    Nav(NavKey),
    Approve,
    Reject,
    ToggleDetails,
}

/// Decode one key press. `gate_focused` is `NavState::focused_gate(..)
/// .is_some()` — the SAME predicate the footer legend uses, so the keys the
/// legend advertises are exactly the keys that act.
///
/// A gate is modal: `a` approves, `r` rejects, `v` / `enter` show details.
/// With none focused, `a` is auto-follow, `enter` drills in, and `r` / `v` do
/// nothing. `tab` / shift-`tab` cycle the pane focus and `PgUp` / `PgDn` page
/// the focused scrolling pane (both from any focus, gate included). `q` and
/// Ctrl-C quit (raw mode turns the tty's SIGINT off, so Ctrl-C arrives as a
/// key and must be handled here); only `Esc` pauses. A control- or
/// alt-modified letter is never a plain command — Ctrl-A must not approve a
/// gate.
fn decode_key(code: KeyCode, mods: KeyModifiers, gate_focused: bool) -> Option<KeyAction> {
    if mods.contains(KeyModifiers::CONTROL) {
        return (code == KeyCode::Char('c')).then_some(KeyAction::Nav(NavKey::Quit));
    }
    if mods.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char(_)) {
        return None;
    }
    let nav = |key: NavKey| Some(KeyAction::Nav(key));
    match code {
        KeyCode::Char('a') if gate_focused => Some(KeyAction::Approve),
        KeyCode::Char('r') if gate_focused => Some(KeyAction::Reject),
        KeyCode::Char('v') | KeyCode::Enter if gate_focused => Some(KeyAction::ToggleDetails),
        KeyCode::Tab => nav(NavKey::PaneNext),
        KeyCode::BackTab => nav(NavKey::PanePrev),
        KeyCode::PageUp => nav(NavKey::ScrollUp),
        KeyCode::PageDown => nav(NavKey::ScrollDown),
        KeyCode::Up | KeyCode::Char('k') => nav(NavKey::Up),
        KeyCode::Down | KeyCode::Char('j') => nav(NavKey::Down),
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => nav(NavKey::In),
        KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => nav(NavKey::Out),
        KeyCode::Char('a') => nav(NavKey::Follow),
        KeyCode::Char('/') => nav(NavKey::Filter),
        KeyCode::Char('q') => nav(NavKey::Quit),
        KeyCode::Esc => nav(NavKey::Pause),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Leaving
// ---------------------------------------------------------------------------

/// A run is finished for the view's purposes when it is terminal or
/// cooperatively paused (a paused run is resumable, so `is_terminal()` alone
/// would spin forever on it: the process driving the view stops with it).
fn finished(status: RunStatus) -> bool {
    status.is_terminal() || status == RunStatus::Paused
}

/// The generation whose terminal / paused status was already in the log when
/// the view attached — a *previous* generation's outcome, replayed. `None`
/// for a run that had not finished (every fresh run).
fn stale_terminal_gen(replayed: &RunView) -> Option<u64> {
    finished(replayed.status).then_some(replayed.generation)
}

/// Whether the loop should leave on this tick. `events.jsonl` is append-only
/// across resumes, so on attach the model replays the previous generation's
/// `RunFailed` / `RunPaused` before the resumed run's `RunStarted` is written;
/// exiting on that would tear the view down the instant it opened. Each
/// `RunStarted` bumps `generation`, so a finished status only counts once it
/// belongs to a generation other than the stale one.
fn should_exit(status: RunStatus, generation: u64, stale: Option<u64>) -> bool {
    finished(status) && stale != Some(generation)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    /// `q` / Ctrl-C: leave the viewer; the run keeps going.
    Quit,
    /// The run finished (current generation).
    RunFinished,
}

// ---------------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------------

/// Where each unit / sub-agent's transcript lives, learned from the events
/// that announce them. `UnitView` / `DispatchView` carry no path, so this is
/// how a drilled selection resolves to a file to pin.
///
/// A path is stored in the exact spelling its event used and handed to the
/// mux in that same spelling, so `observe` and `pin` can never disagree
/// (canonicalising would not do: a transcript that does not exist yet cannot
/// be canonicalised).
#[derive(Debug, Default)]
struct TranscriptIndex {
    /// A leaf step's own transcript (a linear / `run:` step), keyed by step id.
    /// A fan-out step has none of its own — its units do.
    steps: HashMap<String, PathBuf>,
    units: HashMap<(String, usize), PathBuf>,
    dispatches: HashMap<String, PathBuf>,
}

impl TranscriptIndex {
    /// Remember the transcript `ev` announces (if any) and return the
    /// `(path, codename)` to register with the mux. Call after
    /// `view.apply(ev)`: a `StepWorking` path has no codename of its own, it
    /// takes its step's, which `StepStarted` set.
    fn note(&mut self, view: &RunView, ev: &WfEvent) -> Option<(PathBuf, Option<String>)> {
        let (path, codename) = match ev {
            WfEvent::UnitStarted {
                transcript_path,
                codename,
                ..
            }
            | WfEvent::AgentStarted {
                transcript_path,
                codename,
                ..
            }
            | WfEvent::DispatchStarted {
                transcript_path,
                codename,
                ..
            } => (transcript_path, codename.clone()),
            WfEvent::StepWorking {
                step_id,
                transcript_path: Some(path),
                ..
            } => {
                let codename = view
                    .steps
                    .iter()
                    .find(|s| &s.step_id == step_id)
                    .and_then(|s| s.codename.clone());
                (path, codename)
            }
            _ => return None,
        };
        // A remote unit whose transcript is not mirrored yet can announce an
        // empty path; there is nothing to tail.
        if path.as_os_str().is_empty() {
            return None;
        }
        match ev {
            WfEvent::UnitStarted { step_id, index, .. } => {
                self.units.insert((step_id.clone(), *index), path.clone());
            }
            WfEvent::DispatchStarted { sub_run_id, .. } => {
                self.dispatches.insert(sub_run_id.clone(), path.clone());
            }
            // A step's own transcript: the agent of a non-fan-out step
            // (`unit_index` is `None`), or the path `StepWorking` carries.
            WfEvent::AgentStarted {
                step_id,
                unit_index: None,
                ..
            }
            | WfEvent::StepWorking { step_id, .. } => {
                self.steps.insert(step_id.clone(), path.clone());
            }
            _ => {}
        }
        Some((path.clone(), codename))
    }
}

/// The transcript the stream pane pins — the ONE stream-target predicate,
/// kept in lockstep with what the stream title names (`panes::selection_label`
/// reads the SAME nav selection), so the title can never name a transcript the
/// stream is not showing (Task 5 review, Minor 7):
///
/// * `SubAgent` / `Unit` depth: the drilled sub-agent / unit's transcript —
///   the breadcrumb leaf the title shows.
/// * `Run` / `Step` depth: the *chosen* step's own transcript — a manual
///   selection, or (while following a parked run) its gate. A fan-out step has
///   no transcript of its own, so the pin is `None` there and the stream waits
///   until the operator drills into a unit.
/// * Following at `Run` depth with nothing chosen: `None` — the stream is idle
///   and its title is the neutral `run`.
fn pinned_path(view: &RunView, nav: &NavState, index: &TranscriptIndex) -> Option<PathBuf> {
    match nav.depth() {
        Depth::SubAgent => {
            let sub = nav.selected_sub_agent(view)?;
            index.dispatches.get(&sub.sub_run_id).cloned()
        }
        Depth::Unit => {
            let step = nav.selected_step(view)?;
            let unit = nav.selected_unit(view)?;
            index
                .units
                .get(&(step.step_id.clone(), unit.index))
                .cloned()
        }
        Depth::Run | Depth::Step => {
            let step = nav.chosen_step(view)?;
            index.steps.get(&step.step_id).cloned()
        }
    }
}

/// Concurrent tails the mux may use, from `(open descriptors, limit)`: a
/// quarter of the headroom past a reserve. The mux clamps the result to its
/// own floor / ceiling.
fn tail_budget(usage: Option<(u64, u64)>) -> usize {
    match usage {
        Some((open, limit)) => {
            let spare = limit.saturating_sub(open).saturating_sub(16) / 4;
            usize::try_from(spare).unwrap_or(usize::MAX)
        }
        None => DEFAULT_TAILS,
    }
}

// ---------------------------------------------------------------------------
// The model: seeding + the parts only run.json knows
// ---------------------------------------------------------------------------

/// The kind a not-yet-started step is shown as. Mirrors the orchestrator's
/// step classification (`step_kind_for_run_record`, crate-private there); it
/// only labels a *pending* row, since `StepStarted` carries the real kind.
fn seed_kind(step: &Step) -> StepKind {
    if rupu_orchestrator::is_approval_gate(step) {
        StepKind::ApprovalGate
    } else if step.run.is_some() {
        StepKind::Run
    } else if step.branch.is_some() {
        StepKind::Branch
    } else if step.split.is_some() {
        StepKind::Split
    } else if step.join.is_some() {
        StepKind::Join
    } else if step.panel.is_some() {
        StepKind::Panel
    } else if step.parallel.is_some() {
        StepKind::Parallel
    } else if step.for_each.is_some() {
        StepKind::ForEach
    } else if step.action.is_some() {
        StepKind::Action
    } else {
        StepKind::Linear
    }
}

/// The view before any event: every workflow step pending, in declaration
/// order, so the graph shows the whole run up front and the layout can fold
/// what has not started.
fn seed_view(workflow: &Workflow, run_id: &str) -> RunView {
    let mut view = RunView::default();
    view.run_id = run_id.to_string();
    view.workflow_name = workflow.name.clone();
    for step in &workflow.steps {
        let seeded = view.step_mut(&step.id);
        seeded.kind = seed_kind(step);
        seeded.agent = step.agent.clone();
    }
    view
}

/// Fold what only `run.json` knows into the view: the crew, the parked-gate
/// set, and the *parked* status. A gate park emits no run-level event (the
/// runner just stops), so without this a parked run would read `running`
/// forever. Applied every tick.
///
/// Gates are taken only while the record is `AwaitingApproval`: a cooperative
/// pause also fills the awaiting fields (`RunStore::pause` records the active
/// step there), and that is not a gate anyone can approve. Only the parked
/// status is overlaid — terminal and paused stay event-derived, so the
/// resume-generation guard keeps its meaning.
fn overlay_record(view: &mut RunView, rec: &RunRecord) {
    if view.workflow_name.is_empty() {
        view.workflow_name.clone_from(&rec.workflow_name);
    }
    if let Some(crew) = &rec.codename {
        view.crew = Some(crew.clone());
    }
    view.gates = if rec.status == RunStatus::AwaitingApproval {
        rec.awaiting_gates()
            .into_iter()
            .map(|g| GateView {
                step_id: g.step_id,
                prompt: g.prompt,
                since: g.since,
                expires_at: g.expires_at,
            })
            .collect()
    } else {
        Vec::new()
    };
    match (rec.status, view.status) {
        (RunStatus::AwaitingApproval, _) => view.status = RunStatus::AwaitingApproval,
        // The gate was decided (here or elsewhere): follow the record out.
        (settled, RunStatus::AwaitingApproval) => view.status = settled,
        _ => {}
    }
}

/// Fold `step_results.jsonl` into the view: loop iteration + host onto the
/// steps that exist, and the findings-by-severity tally. Idempotent — the
/// tally is recounted from scratch, so re-reading every second never stacks.
/// A result for a step the view does not know (a loop super-node) is counted
/// for findings but never invents a row.
fn overlay_step_results(view: &mut RunView, records: &[StepResultRecord]) {
    view.findings_by_severity.clear();
    for r in records {
        if let Some(step) = view.steps.iter_mut().find(|s| s.step_id == r.step_id) {
            if r.loop_iteration.is_some() {
                step.loop_iteration = r.loop_iteration;
            }
            if r.host.is_some() {
                step.host.clone_from(&r.host);
            }
        }
        for f in &r.findings {
            *view
                .findings_by_severity
                .entry(f.severity.to_lowercase())
                .or_insert(0) += 1;
        }
    }
}

/// The findings the run has recorded so far, for the gate-details panel.
fn gate_findings(records: &[StepResultRecord]) -> Vec<GateFinding> {
    records
        .iter()
        .flat_map(|r| r.findings.iter())
        .map(|f| GateFinding {
            severity: f.severity.clone(),
            title: f.title.clone(),
            who: f.codename.clone().or_else(|| Some(f.source.clone())),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Gate decisions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateVerb {
    Approve,
    Reject,
}

/// What a decided gate asks the loop to run next. The decision itself (the
/// run store flip) is already recorded when one of these is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GateFollowUp {
    /// Approved: re-enter the run from this gate (`crate::resume::resume_run`,
    /// exactly what `rupu workflow approve` runs after recording).
    Resume {
        step_id: String,
        approver: String,
        via_timeout: bool,
    },
    /// Rejected (or auto-rejected by the gate's own timeout policy): run the
    /// gate's `on_reject` cleanup chain, exactly what `rupu workflow reject`
    /// runs after recording.
    Cleanup {
        step_id: String,
        reason: String,
        via: &'static str,
        approver: String,
    },
}

/// What the gate's own `on_timeout` policy had already decided, when the gate
/// is overdue. Lets an approve / reject that lands on an overdue gate be
/// attributed to the policy (`via: "timeout"`), as the CLI does.
fn overdue_policy(
    workflow: &Workflow,
    gate: &GateView,
    now: DateTime<Utc>,
) -> Option<TimeoutAction> {
    let overdue = gate.expires_at.is_some_and(|exp| now > exp);
    overdue
        .then(|| rupu_orchestrator::gate_timeout_action(workflow, &gate.step_id))
        .flatten()
}

/// Record an operator's decision on `gate` (phase 1 of `workflow approve` /
/// `reject`: the same `RunStore` calls, none of the CLI's `println!`s, which
/// would corrupt the alternate screen). `Err` carries a one-line message for
/// the notice row; nothing was recorded.
fn decide_gate(
    store: &RunStore,
    workflow: &Workflow,
    run_id: &str,
    gate: &GateView,
    verb: GateVerb,
    approver: &str,
    now: DateTime<Utc>,
) -> Result<GateFollowUp, String> {
    let policy = overdue_policy(workflow, gate, now);
    match verb {
        GateVerb::Approve => match store.approve_gate(run_id, approver, now, Some(&gate.step_id)) {
            Ok(ApprovalDecision::Approved { step_id, .. }) => Ok(GateFollowUp::Resume {
                step_id,
                approver: approver.to_string(),
                via_timeout: policy == Some(TimeoutAction::Approve),
            }),
            // The gate's own `on_timeout: reject` fired before this approve
            // landed; the store already finalized the run `Rejected`.
            Err(ApprovalError::ExpiredRejected { step_id, reason }) => Ok(GateFollowUp::Cleanup {
                step_id,
                reason,
                via: "timeout",
                approver: approver.to_string(),
            }),
            Err(e) => Err(format!("approve failed: {e}")),
            Ok(other) => Err(format!("approve: unexpected decision {other:?}")),
        },
        GateVerb::Reject => {
            match store.reject_gate(run_id, approver, REJECT_REASON, now, Some(&gate.step_id)) {
                Ok(ApprovalDecision::Rejected {
                    step_id, reason, ..
                }) => Ok(GateFollowUp::Cleanup {
                    step_id,
                    reason,
                    via: if policy == Some(TimeoutAction::Reject) {
                        "timeout"
                    } else {
                        "human"
                    },
                    approver: approver.to_string(),
                }),
                Err(e) => Err(format!("reject failed: {e}")),
                Ok(other) => Err(format!("reject: unexpected decision {other:?}")),
            }
        }
    }
}

/// Run a decided gate's follow-up in this process: the resume for an approve,
/// the `on_reject` chain for a reject. `Err` is the message for the notice
/// row.
async fn run_follow_up(
    runs_dir: &Path,
    run_id: &str,
    follow_up: GateFollowUp,
) -> Result<(), String> {
    let store = RunStore::new(runs_dir.to_path_buf());
    match follow_up {
        GateFollowUp::Resume {
            step_id,
            approver,
            via_timeout,
        } => crate::resume::resume_run(&store, run_id, &step_id, None, &approver, via_timeout)
            .await
            .map(|_| ())
            .map_err(|e| format!("resume failed: {e:#}")),
        GateFollowUp::Cleanup {
            step_id,
            reason,
            via,
            approver,
        } => {
            // The store lives at `<global>/runs`; the chain reads its config
            // and writes its records under that same home.
            let global = runs_dir.parent().ok_or_else(|| {
                format!(
                    "on_reject cleanup unavailable: {} has no parent rupu home",
                    runs_dir.display()
                )
            })?;
            let (opts, _chain_len) = crate::resume::build_reject_cleanup_opts(
                &store, global, run_id, &step_id, &reason, None,
            )
            .await
            .map_err(|e| format!("on_reject cleanup unavailable: {e:#}"))?;
            rupu_orchestrator::runner::run_reject_cleanup(
                opts,
                &step_id,
                &reason,
                via,
                Some(&approver),
            )
            .await
            .map_err(|e| format!("on_reject cleanup failed: {e}"))?;
            // The chain just ran synchronously: clear the pending marker so
            // `cp serve`'s gate sweep does not run it a second time.
            // Best-effort — a failure to clear must not hide a success.
            let _ = store.clear_reject_cleanup(run_id);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

/// A one-line message under the feed (pause requested, gate decided, a
/// failure). Replaced by the next one and gone after [`NOTICE_TTL`].
struct Notice {
    line: Line,
    until: Instant,
    /// A "run parked at a gate" pointer: dropped as soon as it is acted on.
    pointer: bool,
}

fn notice_line(text: &str, bad: bool) -> Line {
    let text = printable(text);
    let line = Line::new().dim("» ");
    if bad {
        line.danger(text)
    } else {
        line.plain(text)
    }
}

/// Everything the loop owns between ticks.
struct Live {
    workflow: Workflow,
    store: RunStore,
    runs_dir: PathBuf,
    run_id: String,
    pricing: rupu_config::PricingConfig,
    approver: String,
    view: RunView,
    nav: NavState,
    mux: TranscriptMux,
    renderer: LiveRenderer,
    tailer: WfEventTailer,
    paths: TranscriptIndex,
    /// The previous generation's finished outcome, replayed on attach.
    stale_gen: Option<u64>,
    /// `v` / `enter` at a focused gate: show its details instead of the feed.
    show_details: bool,
    gate_findings: Vec<GateFinding>,
    notice: Option<Notice>,
    /// The gate the "run parked" notice was last shown for, so it fires once
    /// per park rather than every tick.
    parked_hint: Option<String>,
    /// Follow-ups of in-view gate decisions (resume / on_reject cleanup).
    /// Joined on exit so leaving the view never kills the run it resumed.
    background: Vec<JoinHandle<()>>,
    msgs_tx: UnboundedSender<String>,
    msgs_rx: UnboundedReceiver<String>,
    last_slow: Option<Instant>,
}

impl Live {
    /// Seed the view from the workflow, replay `events.jsonl` and read
    /// `run.json`, so the first frame is already true. The slow files (usage
    /// fold, step results) are read after it is painted.
    fn new(
        workflow: Workflow,
        runs_dir: PathBuf,
        run_id: String,
        pricing: rupu_config::PricingConfig,
    ) -> Self {
        let store = RunStore::new(runs_dir.clone());
        let (msgs_tx, msgs_rx) = unbounded_channel();
        let mut live = Self {
            view: seed_view(&workflow, &run_id),
            nav: NavState::default(),
            mux: TranscriptMux::new(tail_budget(rupu_agent::fd_budget::fd_usage())),
            renderer: LiveRenderer::new(),
            tailer: WfEventTailer::new(runs_dir.join(&run_id).join("events.jsonl")),
            paths: TranscriptIndex::default(),
            stale_gen: None,
            show_details: false,
            gate_findings: Vec::new(),
            notice: None,
            parked_hint: None,
            background: Vec::new(),
            msgs_tx,
            msgs_rx,
            last_slow: None,
            approver: whoami::username(),
            workflow,
            store,
            runs_dir,
            run_id,
            pricing,
        };
        live.drain_events();
        live.stale_gen = stale_terminal_gen(&live.view);
        live.overlay();
        live
    }

    /// Fold newly appended events into the model and register their
    /// transcripts with the mux.
    fn drain_events(&mut self) {
        for ev in self.tailer.drain_events() {
            self.view.apply(&ev);
            if let Some((path, codename)) = self.paths.note(&self.view, &ev) {
                self.mux.observe(path, codename);
            }
        }
    }

    /// Re-read `run.json` into the view (crew, gates, parked status).
    fn overlay(&mut self) {
        if let Ok(rec) = self.store.load(&self.run_id) {
            overlay_record(&mut self.view, &rec);
        }
    }

    /// The per-tick intake: events, `run.json`, follow-up results, and a nav
    /// reconcile against the (possibly grown / replaced) view.
    fn ingest(&mut self) {
        self.drain_events();
        self.overlay();
        while let Ok(msg) = self.msgs_rx.try_recv() {
            self.set_notice(&msg, true);
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|n| Instant::now() >= n.until)
        {
            self.notice = None;
        }
        self.nav.sync(&self.view);
        let gate_focused = self.nav.focused_gate(&self.view).is_some();
        if !gate_focused {
            self.show_details = false;
        }
        self.hint_parked_gate(gate_focused);
    }

    /// A run that parks while the operator is looking elsewhere (drilled into
    /// a unit, say) keeps their selection — the gate keys only apply once it
    /// is focused — so say once that it is waiting and how to reach it.
    fn hint_parked_gate(&mut self, gate_focused: bool) {
        // The pointer has done its job once the gate is focused.
        if gate_focused && self.notice.as_ref().is_some_and(|n| n.pointer) {
            self.notice = None;
        }
        // The gate `a` (follow) will land on: a default nav is following, so
        // `gate_step` yields the first parked gate in step order — the same
        // one the key focuses, not merely the first in run.json's set.
        let parked = NavState::default()
            .gate_step(&self.view)
            .map(|s| s.step_id.clone());
        match (parked, gate_focused) {
            (Some(step), false) if self.parked_hint.as_ref() != Some(&step) => {
                self.set_notice(
                    &format!(
                        "run parked at gate {step} — press a to jump to it, then a approve / r reject"
                    ),
                    false,
                );
                if let Some(n) = self.notice.as_mut() {
                    n.pointer = true;
                }
                self.parked_hint = Some(step);
            }
            (None, _) => self.parked_hint = None,
            _ => {}
        }
    }

    /// The slow, file-reading parts, at most once a second (`force` skips the
    /// rate limit): the usage fold (incremental, cheap on a long run) and
    /// `step_results.jsonl` (findings, loop iteration, host).
    fn refresh_slow(&mut self, force: bool) {
        if !force
            && self
                .last_slow
                .is_some_and(|t| t.elapsed() < SLOW_REFRESH_EVERY)
        {
            return;
        }
        self.last_slow = Some(Instant::now());
        self.view.usage = Some(rupu_cp::usage::summarize_run(
            &self.store,
            &self.run_id,
            &self.pricing,
        ));
        if let Ok(records) = self.store.read_step_results(&self.run_id) {
            overlay_step_results(&mut self.view, &records);
            self.gate_findings = gate_findings(&records);
        }
    }

    fn set_notice(&mut self, text: &str, bad: bool) {
        self.notice = Some(Notice {
            line: notice_line(text, bad),
            until: Instant::now() + NOTICE_TTL,
            pointer: false,
        });
    }

    /// Drain pending key presses. `Some` when one asks to leave.
    async fn poll_keys(&mut self) -> Option<Exit> {
        while event::poll(Duration::ZERO).unwrap_or(false) {
            let key = match event::read() {
                Ok(TermEvent::Key(key)) => key,
                Ok(_) => continue,
                Err(_) => break,
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // Re-derived per key: an approve in this very drain may have
            // just retired the gate.
            let gate_focused = self.nav.focused_gate(&self.view).is_some();
            let Some(action) = decode_key(key.code, key.modifiers, gate_focused) else {
                continue;
            };
            if let Some(exit) = self.apply_action(action).await {
                return Some(exit);
            }
        }
        None
    }

    async fn apply_action(&mut self, action: KeyAction) -> Option<Exit> {
        match action {
            KeyAction::Nav(key) => match self.nav.apply(key, &self.view) {
                NavAction::Quit => return Some(Exit::Quit),
                NavAction::Pause => self.request_pause().await,
                NavAction::None => {}
            },
            KeyAction::Approve => self.decide(GateVerb::Approve),
            KeyAction::Reject => self.decide(GateVerb::Reject),
            KeyAction::ToggleDetails => self.show_details = !self.show_details,
        }
        None
    }

    /// `Esc`: ask the run to pause cooperatively — the exact primitive
    /// `rupu workflow pause` uses, which writes to the store rather than
    /// stdout so it is safe on the alternate screen. The view stays up until
    /// the run actually stops (`RunPaused`), so the operator watches it land.
    async fn request_pause(&mut self) {
        // A run resumed from this view runs without a pause channel
        // (`resume_run` wires none); flipping its record to `Paused` would
        // claim a pause nothing will honor.
        if self.background.iter().any(|h| !h.is_finished()) {
            self.set_notice(
                "pause is not available for a run resumed from this view \
                 (q leaves it running; rupu workflow cancel stops it)",
                true,
            );
            return;
        }
        match crate::cmd::workflow::pause_with_store(&self.store, &self.run_id).await {
            Ok(()) => {
                let msg = format!(
                    "pause requested — will stop at next safe boundary \
                     (resume: rupu workflow resume {})",
                    self.run_id
                );
                self.set_notice(&msg, false);
            }
            Err(e) => self.set_notice(&format!("{e:#}"), true),
        }
    }

    /// `a` / `r` at the focused gate: record the decision, then run its
    /// follow-up in the background while the view keeps following the run.
    fn decide(&mut self, verb: GateVerb) {
        let Some(gate) = self.nav.focused_gate(&self.view).cloned() else {
            return;
        };
        match decide_gate(
            &self.store,
            &self.workflow,
            &self.run_id,
            &gate,
            verb,
            &self.approver,
            Utc::now(),
        ) {
            Ok(follow_up) => {
                let (what, step) = match verb {
                    GateVerb::Approve => ("approved", "resuming the run"),
                    GateVerb::Reject => ("rejected", "running its on_reject cleanup"),
                };
                self.set_notice(&format!("{what} {} — {step}", gate.step_id), false);
                self.spawn_follow_up(follow_up);
            }
            Err(msg) => self.set_notice(&msg, true),
        }
        // The decision rewrote run.json: reflect it now, so the gate stops
        // being focusable (and advertised) before the next key.
        self.overlay();
        self.nav.sync(&self.view);
    }

    fn spawn_follow_up(&mut self, follow_up: GateFollowUp) {
        let runs_dir = self.runs_dir.clone();
        let run_id = self.run_id.clone();
        let tx = self.msgs_tx.clone();
        self.background.push(tokio::spawn(async move {
            if let Err(msg) = run_follow_up(&runs_dir, &run_id, follow_up).await {
                let _ = tx.send(msg);
            }
        }));
    }

    /// The stream pane's content: a focused gate's details when `v` is on,
    /// else the pinned (drilled / chosen) transcript — empty when nothing is
    /// pinned, since the firehose now has its own pane. The transient notice
    /// rides last. Pins + drains the mux, so it is called once per frame.
    fn build_feed(&mut self) -> Vec<Line> {
        let pinned = pinned_path(&self.view, &self.nav, &self.paths);
        self.mux.pin(pinned);
        self.mux.drain();
        let details = self
            .nav
            .focused_gate(&self.view)
            .filter(|_| self.show_details);
        let mut feed = match details {
            Some(gate) => gate_detail_lines(gate, &self.gate_findings, Utc::now()),
            None => self.mux.pinned_lines(),
        };
        let notice = self
            .notice
            .as_ref()
            .filter(|n| Instant::now() < n.until)
            .map(|n| n.line.clone());
        feed.extend(notice);
        feed
    }

    /// Compose one frame at `size` (`(cols, rows)`): the three-pane dashboard
    /// built from the run model, the pinned stream and the merged firehose.
    /// Clamps each scrolling pane's offset against the real buffer first
    /// (nav windows but never clamps), using [`scroll_rows`] so the clamp and
    /// the frame measure the body the same way.
    fn frame(&mut self, size: (u16, u16)) -> Vec<Line> {
        let stream = self.build_feed();
        let firehose = self.mux.firehose_lines();
        let w = usize::from(size.0).saturating_sub(EDGE_GUARD).max(1);
        let h = usize::from(size.1);
        let now = Utc::now();
        let rows = scroll_rows(&self.view, &self.nav, now, w, h);
        self.nav
            .clamp_scroll(Pane::Stream, stream.len().saturating_sub(rows.stream));
        self.nav
            .clamp_scroll(Pane::Firehose, firehose.len().saturating_sub(rows.firehose));
        dashboard_frame(
            &self.view,
            &self.workflow,
            &self.nav,
            &stream,
            &firehose,
            now,
            w,
            h,
        )
    }

    /// Compose and paint one frame at `size` (`(cols, rows)`).
    fn draw(&mut self, out: &mut impl Write, size: (u16, u16)) {
        let frame = self.frame(size);
        // A failed write leaves the screen in an unknown state: forget what
        // was drawn so the next frame repaints in full.
        if self.renderer.draw(out, &frame).is_err() {
            self.renderer.invalidate();
        }
    }

    fn run_finished(&self) -> bool {
        should_exit(self.view.status, self.view.generation, self.stale_gen)
    }

    /// Wait for the follow-ups of in-view gate decisions. They run in this
    /// process, so leaving before they finish would kill the run they
    /// resumed. Called with the terminal already restored.
    async fn join_background(&mut self) {
        let pending: Vec<JoinHandle<()>> = self
            .background
            .drain(..)
            .filter(|h| !h.is_finished())
            .collect();
        if pending.is_empty() {
            return;
        }
        eprintln!(
            "rupu: finishing run {} before exit (Ctrl-C aborts)",
            self.run_id
        );
        for handle in pending {
            let _ = handle.await;
        }
    }
}

/// Drive the live view on the alternate screen until the run finishes (this
/// generation's terminal / paused outcome) or the operator leaves. Returns
/// with the normal screen restored; the caller prints the shared completion
/// summary there.
///
/// Every [`TICK`]: fold new `events.jsonl` lines + `run.json` into the
/// [`RunView`]; reconcile [`NavState`]; decode keys (`q` / Ctrl-C quit and
/// leave the run running, `Esc` pauses, an approval gate is modal); pin the
/// drilled transcript and drain the [`TranscriptMux`]; refresh the usage fold
/// at most once a second; and diff-render [`dashboard_frame`]'s frame. A terminal
/// resize repaints in full. [`AltScreen`] restores the terminal on every exit
/// path, a panic included.
///
/// A parked approval gate does not end the view: `a` / `r` record the decision
/// through the run store and run its follow-up (resume / `on_reject`
/// cleanup) in this process, the same code `rupu workflow approve` / `reject`
/// run. The follow-up is joined before returning.
///
/// Best-effort: an I/O hiccup degrades to the next tick. The caller guards
/// entry behind a tty check; non-tty falls back to the line printer.
pub async fn run_live_view(
    workflow: Workflow,
    runs_dir: PathBuf,
    run_id: String,
    pricing: rupu_config::PricingConfig,
) -> io::Result<()> {
    let mut live = Live::new(workflow, runs_dir, run_id, pricing);
    let screen = AltScreen::enter()?;
    // The renderer flushes once per frame; buffering turns a frame's many
    // small queued writes into one syscall (a bare stdout lock flashes).
    let mut out = BufWriter::new(io::stdout());

    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_size: Option<(u16, u16)> = None;
    let mut size = (100, 30);
    let exit = loop {
        interval.tick().await;
        size = terminal::size().unwrap_or(size);
        if last_size.replace(size) != Some(size) {
            // A resize can reflow or drop rows behind the renderer's back.
            live.renderer.invalidate();
        }
        live.ingest();
        if let Some(exit) = live.poll_keys().await {
            break exit;
        }
        live.draw(&mut out, size);
        live.refresh_slow(false);
        if live.run_finished() {
            break Exit::RunFinished;
        }
    };

    if exit == Exit::RunFinished {
        // One last frame carrying the final spend, not up-to-a-second-stale
        // figures.
        live.ingest();
        live.refresh_slow(true);
        live.draw(&mut out, size);
    }
    drop(out);
    drop(screen);
    live.join_background().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::executor::Event;

    const NONE: KeyModifiers = KeyModifiers::NONE;

    fn nav(k: NavKey) -> Option<KeyAction> {
        Some(KeyAction::Nav(k))
    }

    #[test]
    fn decode_key_truth_table_without_a_gate() {
        let d = |c: KeyCode| decode_key(c, NONE, false);
        for (code, want) in [
            (KeyCode::Up, nav(NavKey::Up)),
            (KeyCode::Char('k'), nav(NavKey::Up)),
            (KeyCode::Down, nav(NavKey::Down)),
            (KeyCode::Char('j'), nav(NavKey::Down)),
            // Tab / shift-Tab cycle the pane focus; PgUp/PgDn page a feed.
            (KeyCode::Tab, nav(NavKey::PaneNext)),
            (KeyCode::BackTab, nav(NavKey::PanePrev)),
            (KeyCode::PageUp, nav(NavKey::ScrollUp)),
            (KeyCode::PageDown, nav(NavKey::ScrollDown)),
            (KeyCode::Enter, nav(NavKey::In)),
            (KeyCode::Right, nav(NavKey::In)),
            (KeyCode::Char('l'), nav(NavKey::In)),
            (KeyCode::Left, nav(NavKey::Out)),
            (KeyCode::Backspace, nav(NavKey::Out)),
            (KeyCode::Char('h'), nav(NavKey::Out)),
            (KeyCode::Char('a'), nav(NavKey::Follow)),
            (KeyCode::Char('/'), nav(NavKey::Filter)),
            (KeyCode::Char('q'), nav(NavKey::Quit)),
            (KeyCode::Esc, nav(NavKey::Pause)),
            // The gate keys do nothing without a focused gate.
            (KeyCode::Char('r'), None),
            (KeyCode::Char('v'), None),
            (KeyCode::Char('x'), None),
            (KeyCode::F(5), None),
        ] {
            assert_eq!(d(code), want, "{code:?}");
        }
    }

    #[test]
    fn a_focused_gate_makes_a_r_v_enter_modal_and_leaves_the_rest_alone() {
        let d = |c: KeyCode| decode_key(c, NONE, true);
        assert_eq!(d(KeyCode::Char('a')), Some(KeyAction::Approve));
        assert_eq!(d(KeyCode::Char('r')), Some(KeyAction::Reject));
        assert_eq!(d(KeyCode::Char('v')), Some(KeyAction::ToggleDetails));
        assert_eq!(d(KeyCode::Enter), Some(KeyAction::ToggleDetails));
        // Movement, quitting and pausing are unchanged at a gate.
        assert_eq!(d(KeyCode::Down), nav(NavKey::Down));
        assert_eq!(d(KeyCode::Char('j')), nav(NavKey::Down));
        assert_eq!(d(KeyCode::Char('q')), nav(NavKey::Quit));
        assert_eq!(d(KeyCode::Esc), nav(NavKey::Pause));
        assert_eq!(d(KeyCode::Right), nav(NavKey::In));
    }

    #[test]
    fn ctrl_c_always_quits_and_other_chords_never_act() {
        for gate in [false, true] {
            assert_eq!(
                decode_key(KeyCode::Char('c'), KeyModifiers::CONTROL, gate),
                nav(NavKey::Quit),
                "gate={gate}"
            );
            // Ctrl-A / Ctrl-R / Ctrl-Q are not plain commands: Ctrl-A must
            // never approve a gate.
            for c in ['a', 'r', 'v', 'q', 'j'] {
                assert_eq!(
                    decode_key(KeyCode::Char(c), KeyModifiers::CONTROL, gate),
                    None,
                    "ctrl-{c} gate={gate}"
                );
                assert_eq!(
                    decode_key(KeyCode::Char(c), KeyModifiers::ALT, gate),
                    None,
                    "alt-{c} gate={gate}"
                );
            }
            // Uppercase is a different key: `A` is not approve.
            assert_eq!(
                decode_key(KeyCode::Char('A'), KeyModifiers::SHIFT, gate),
                None
            );
        }
    }

    fn started() -> Event {
        Event::RunStarted {
            event_version: 1,
            run_id: "r".into(),
            workflow_path: "wf".into(),
            started_at: Utc::now(),
        }
    }

    fn failed() -> Event {
        Event::RunFailed {
            run_id: "r".into(),
            error: "boom".into(),
            finished_at: Utc::now(),
        }
    }

    fn completed() -> Event {
        Event::RunCompleted {
            run_id: "r".into(),
            status: RunStatus::Completed,
            finished_at: Utc::now(),
        }
    }

    #[test]
    fn should_exit_waits_for_a_finished_status_of_a_fresh_generation() {
        // Live statuses never exit.
        for s in [
            RunStatus::Pending,
            RunStatus::Running,
            RunStatus::AwaitingApproval,
        ] {
            assert!(!should_exit(s, 1, None), "{s:?}");
        }
        // Terminal / paused with nothing stale: exit.
        for s in [
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Rejected,
            RunStatus::Cancelled,
            RunStatus::Paused,
        ] {
            assert!(should_exit(s, 1, None), "{s:?}");
        }
        // The stale generation's outcome does not count; a later one does.
        assert!(!should_exit(RunStatus::Failed, 1, Some(1)));
        assert!(!should_exit(RunStatus::Paused, 3, Some(3)));
        assert!(should_exit(RunStatus::Completed, 2, Some(1)));
    }

    #[test]
    fn a_resumed_runs_replayed_terminal_event_does_not_close_the_view() {
        // events.jsonl of a run that failed and is now being resumed: the
        // previous generation's RunFailed is replayed on attach.
        let mut view = RunView::default();
        view.apply(&started());
        view.apply(&failed());
        let stale = stale_terminal_gen(&view);
        assert_eq!(stale, Some(1));
        assert!(finished(view.status));
        assert!(
            !should_exit(view.status, view.generation, stale),
            "must not exit on the replayed RunFailed"
        );

        // The resumed run starts: a new generation, no longer finished.
        view.apply(&started());
        assert_eq!(view.generation, 2);
        assert!(!should_exit(view.status, view.generation, stale));

        // ...and finishing it IS honored.
        view.apply(&completed());
        assert!(should_exit(view.status, view.generation, stale));

        // A fresh run (nothing finished at attach) exits on its first outcome.
        let mut fresh = RunView::default();
        fresh.apply(&started());
        assert_eq!(stale_terminal_gen(&fresh), None);
        fresh.apply(&completed());
        assert!(should_exit(
            fresh.status,
            fresh.generation,
            stale_terminal_gen(&RunView::default())
        ));
    }

    // ── transcripts ─────────────────────────────────────────────────────

    fn step_started(step: &str, kind: StepKind, codename: Option<&str>) -> Event {
        Event::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            kind,
            agent: None,
            host: None,
            codename: codename.map(str::to_string),
        }
    }

    fn unit_started(step: &str, index: usize, path: &str, codename: &str) -> Event {
        Event::UnitStarted {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            unit_key: format!("svc-{index}"),
            agent: Some("breaker".into()),
            transcript_path: path.into(),
            host: None,
            codename: Some(codename.into()),
        }
    }

    fn dispatch_started(sub: &str, path: &str, codename: &str) -> Event {
        Event::DispatchStarted {
            run_id: "r".into(),
            sub_run_id: sub.into(),
            agent: Some("scout".into()),
            transcript_path: path.into(),
            codename: Some(codename.into()),
            provider: None,
            model: None,
        }
    }

    /// Fold `events` the way the loop does: apply, then note.
    fn fold(events: &[Event]) -> (RunView, TranscriptIndex, Vec<(PathBuf, Option<String>)>) {
        let mut view = RunView::default();
        let mut index = TranscriptIndex::default();
        let mut observed = Vec::new();
        for ev in events {
            view.apply(ev);
            observed.extend(index.note(&view, ev));
        }
        (view, index, observed)
    }

    #[test]
    fn events_that_name_a_transcript_register_it_with_its_codename() {
        let (_, index, observed) = fold(&[
            step_started("hunt", StepKind::ForEach, None),
            unit_started("hunt", 0, "/t/u0.jsonl", "otter#1"),
            Event::AgentStarted {
                run_id: "r".into(),
                step_id: "hunt".into(),
                unit_index: Some(0),
                codename: Some("otter#1".into()),
                agent: "breaker".into(),
                provider: None,
                model: None,
                agent_run_id: "a0".into(),
                transcript_path: "/t/u0.jsonl".into(),
            },
            dispatch_started("sub1", "/t/sub1.jsonl", "wren#1"),
            step_started("report", StepKind::Linear, Some("heron#1")),
            Event::StepWorking {
                run_id: "r".into(),
                step_id: "report".into(),
                note: None,
                transcript_path: Some("/t/report.jsonl".into()),
            },
            // No path: nothing to register.
            Event::StepWorking {
                run_id: "r".into(),
                step_id: "report".into(),
                note: Some("tool call".into()),
                transcript_path: None,
            },
            // A remote unit whose transcript is not mirrored yet: empty path.
            unit_started("hunt", 1, "", "otter#2"),
        ]);
        let got: Vec<(String, Option<String>)> = observed
            .iter()
            .map(|(p, c)| (p.display().to_string(), c.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("/t/u0.jsonl".to_string(), Some("otter#1".to_string())),
                ("/t/u0.jsonl".to_string(), Some("otter#1".to_string())),
                ("/t/sub1.jsonl".to_string(), Some("wren#1".to_string())),
                // A step's own transcript takes the step's codename.
                ("/t/report.jsonl".to_string(), Some("heron#1".to_string())),
            ]
        );
        assert_eq!(
            index.units.get(&("hunt".to_string(), 0)),
            Some(&PathBuf::from("/t/u0.jsonl"))
        );
        // The empty-path unit is not indexed (nothing to pin).
        assert!(!index.units.contains_key(&("hunt".to_string(), 1)));
        assert_eq!(
            index.dispatches.get("sub1"),
            Some(&PathBuf::from("/t/sub1.jsonl"))
        );
    }

    #[test]
    fn a_drilled_selection_resolves_to_the_exact_path_the_mux_was_given() {
        let (view, index, observed) = fold(&[
            step_started("hunt", StepKind::ForEach, None),
            unit_started("hunt", 0, "/t/u0.jsonl", "otter#1"),
            unit_started("hunt", 1, "/t/u1.jsonl", "otter#2"),
            dispatch_started("sub1", "/t/sub1.jsonl", "wren#1"),
        ]);
        let mut nav = NavState::default();
        // Run and Step depth pin nothing: the feed is the firehose.
        assert_eq!(pinned_path(&view, &nav, &index), None);
        nav.apply(NavKey::In, &view);
        assert_eq!(nav.depth(), Depth::Step);
        assert_eq!(pinned_path(&view, &nav, &index), None);
        // Unit depth pins the selected unit...
        nav.apply(NavKey::In, &view);
        assert_eq!(nav.depth(), Depth::Unit);
        assert_eq!(
            pinned_path(&view, &nav, &index),
            Some(PathBuf::from("/t/u0.jsonl"))
        );
        // The unit cursor moves at Step depth: back out, step down, drill in.
        nav.apply(NavKey::Out, &view);
        nav.apply(NavKey::Down, &view);
        nav.apply(NavKey::In, &view);
        assert_eq!(nav.depth(), Depth::Unit);
        assert_eq!(
            pinned_path(&view, &nav, &index),
            Some(PathBuf::from("/t/u1.jsonl"))
        );
        // ...and the pin is spelled exactly like an observed path, so the
        // mux's pin lands on an entry it already knows.
        assert!(observed
            .iter()
            .any(|(p, _)| Some(p) == pinned_path(&view, &nav, &index).as_ref()));
        // Sub-agent depth pins the dispatched child.
        nav.apply(NavKey::In, &view);
        assert_eq!(nav.depth(), Depth::SubAgent);
        assert_eq!(
            pinned_path(&view, &nav, &index),
            Some(PathBuf::from("/t/sub1.jsonl"))
        );
        // Popping back out unpins.
        nav.apply(NavKey::Out, &view);
        nav.apply(NavKey::Out, &view);
        assert_eq!(pinned_path(&view, &nav, &index), None);
    }

    #[test]
    fn the_tail_budget_is_a_quarter_of_the_spare_descriptors() {
        assert_eq!(tail_budget(Some((100, 10_240))), (10_240 - 100 - 16) / 4);
        // No headroom: zero, which the mux lifts to its own floor.
        assert_eq!(tail_budget(Some((10_240, 10_240))), 0);
        assert_eq!(tail_budget(Some((300, 256))), 0);
        assert_eq!(tail_budget(Some((250, 256))), 0);
        // Unreadable: a modest default.
        assert_eq!(tail_budget(None), DEFAULT_TAILS);
    }

    // ── the model ───────────────────────────────────────────────────────

    const GATES_YAML: &str =
        "name: g\nsteps:\n  - id: gate_a\n    approval: {}\n  - id: gate_b\n    approval: {}\n";

    fn parse(yaml: &str) -> Workflow {
        Workflow::parse(yaml).unwrap_or_else(|e| panic!("fixture workflow: {e}"))
    }

    /// A run record in `status`, with `gates` parked when awaiting approval.
    fn record(id: &str, status: &str, gates: &[(&str, Option<DateTime<Utc>>)]) -> RunRecord {
        let awaiting: Vec<serde_json::Value> = gates
            .iter()
            .map(|(step, expires)| {
                serde_json::json!({
                    "step_id": step,
                    "prompt": format!("approve {step}?"),
                    "since": "2026-09-30T11:00:00Z",
                    "expires_at": expires,
                })
            })
            .collect();
        let mut rec: RunRecord = serde_json::from_value(serde_json::json!({
            "id": id,
            "workflow_name": "g",
            "status": status,
            "inputs": {},
            "workspace_id": "ws",
            "workspace_path": "/tmp/ws",
            "transcript_dir": "/tmp/tr",
            "started_at": "2026-09-30T10:00:00Z",
            "codename": "mint-tundra",
            "awaiting": awaiting,
        }))
        .unwrap();
        rec.sync_awaiting_compat();
        rec
    }

    #[test]
    fn the_seeded_view_lists_every_step_pending_in_order() {
        let wf = parse(
            "name: seeded\nsteps:\n  \
             - id: plan\n    agent: planner\n    prompt: go\n  \
             - id: hunt\n    agent: breaker\n    for_each: \"{{ inputs.items }}\"\n    prompt: x\n  \
             - id: ship\n    approval: {}\n",
        );
        let view = seed_view(&wf, "run_1");
        assert_eq!(view.run_id, "run_1");
        assert_eq!(view.workflow_name, "seeded");
        let got: Vec<(&str, StepKind, bool)> = view
            .steps
            .iter()
            .map(|s| {
                (
                    s.step_id.as_str(),
                    s.kind,
                    s.state == crate::output::run_model::StepState::Pending,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("plan", StepKind::Linear, true),
                ("hunt", StepKind::ForEach, true),
                ("ship", StepKind::ApprovalGate, true),
            ]
        );
        assert_eq!(view.steps[0].agent.as_deref(), Some("planner"));
        // Events then update the seeded rows in place rather than adding new.
        let mut view = view;
        view.apply(&step_started("hunt", StepKind::ForEach, Some("heron#1")));
        assert_eq!(view.steps.len(), 3);
        assert_eq!(
            view.steps[1].state,
            crate::output::run_model::StepState::Running
        );
    }

    #[test]
    fn overlay_record_takes_gates_only_while_awaiting_and_follows_the_record_out() {
        let mut view = seed_view(&parse(GATES_YAML), "r");
        // Events alone never say "parked": a gate park emits no run-level event.
        view.apply(&started());
        assert_eq!(view.status, RunStatus::Running);

        let parked = record(
            "r",
            "awaiting_approval",
            &[("gate_a", None), ("gate_b", None)],
        );
        overlay_record(&mut view, &parked);
        assert_eq!(view.status, RunStatus::AwaitingApproval);
        assert_eq!(view.crew.as_deref(), Some("mint-tundra"));
        let ids: Vec<&str> = view.gates.iter().map(|g| g.step_id.as_str()).collect();
        assert_eq!(ids, vec!["gate_a", "gate_b"]);
        assert_eq!(view.gates[0].prompt.as_deref(), Some("approve gate_a?"));

        // One gate decided: it leaves the set, the run stays parked.
        let one_left = record("r", "awaiting_approval", &[("gate_b", None)]);
        overlay_record(&mut view, &one_left);
        assert_eq!(view.status, RunStatus::AwaitingApproval);
        assert_eq!(view.gates.len(), 1);

        // The last gate decided and the run resumed: no gates, status follows.
        overlay_record(&mut view, &record("r", "running", &[]));
        assert_eq!(view.status, RunStatus::Running);
        assert!(view.gates.is_empty());
    }

    #[test]
    fn a_manual_pause_record_is_not_a_gate_and_never_overwrites_event_status() {
        let mut view = seed_view(&parse(GATES_YAML), "r");
        view.apply(&started());
        // `RunStore::pause` stores the active step in the awaiting fields.
        let mut paused = record("r", "paused", &[]);
        paused.awaiting_step_id = Some("gate_a".into());
        overlay_record(&mut view, &paused);
        assert!(view.gates.is_empty(), "a pause is not an approvable gate");
        // Terminal / paused stay event-derived (the generation guard's input).
        assert_eq!(view.status, RunStatus::Running);
        overlay_record(&mut view, &record("r", "failed", &[]));
        assert_eq!(view.status, RunStatus::Running);
    }

    fn step_result(step: &str, findings: &[&str]) -> StepResultRecord {
        let mut r: StepResultRecord = serde_json::from_value(serde_json::json!({
            "step_id": step,
            "run_id": "r",
            "transcript_path": "/t/x.jsonl",
            "output": "",
            "success": true,
            "skipped": false,
            "rendered_prompt": "",
            "finished_at": "2026-09-30T11:00:00Z",
        }))
        .unwrap();
        r.findings = findings
            .iter()
            .map(|sev| rupu_orchestrator::FindingRecord {
                source: "panel".into(),
                severity: (*sev).into(),
                title: format!("{sev} thing"),
                body: String::new(),
                codename: None,
            })
            .collect();
        r
    }

    #[test]
    fn step_results_overlay_is_idempotent_and_never_invents_a_step() {
        let mut view = seed_view(&parse(GATES_YAML), "r");
        let mut a = step_result("gate_a", &["High", "high", "low"]);
        a.loop_iteration = Some(2);
        a.host = Some("kuki".into());
        // A loop super-node result: findings count, but no row is invented.
        let phantom = step_result("loop:fix", &["low"]);
        let records = vec![a, phantom];

        overlay_step_results(&mut view, &records);
        overlay_step_results(&mut view, &records);
        assert_eq!(view.findings_by_severity.get("high"), Some(&2));
        assert_eq!(view.findings_by_severity.get("low"), Some(&2));
        assert_eq!(view.steps.len(), 2, "no phantom `loop:fix` row");
        assert_eq!(view.steps[0].loop_iteration, Some(2));
        assert_eq!(view.steps[0].host.as_deref(), Some("kuki"));

        let listed = gate_findings(&records);
        assert_eq!(listed.len(), 4);
        assert_eq!(listed[0].who.as_deref(), Some("panel"));
    }

    // ── gate decisions ──────────────────────────────────────────────────

    fn gate_view(step: &str, expires_at: Option<DateTime<Utc>>) -> GateView {
        GateView {
            step_id: step.into(),
            prompt: None,
            since: Utc::now(),
            expires_at,
        }
    }

    fn parked_store(tmp: &Path, yaml: &str, gates: &[(&str, Option<DateTime<Utc>>)]) -> RunStore {
        let store = RunStore::new(tmp.join("runs"));
        store
            .create(record("run_t", "awaiting_approval", gates), yaml)
            .unwrap();
        store
    }

    #[test]
    fn approving_one_gate_records_it_and_asks_for_a_resume_of_that_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let store = parked_store(
            tmp.path(),
            GATES_YAML,
            &[("gate_a", None), ("gate_b", None)],
        );
        let wf = parse(GATES_YAML);

        let out = decide_gate(
            &store,
            &wf,
            "run_t",
            &gate_view("gate_b", None),
            GateVerb::Approve,
            "op",
            Utc::now(),
        )
        .unwrap();
        assert_eq!(
            out,
            GateFollowUp::Resume {
                step_id: "gate_b".into(),
                approver: "op".into(),
                via_timeout: false,
            }
        );
        // Only gate_b left the set; the run stays parked on gate_a.
        let rec = store.load("run_t").unwrap();
        assert_eq!(rec.status, RunStatus::AwaitingApproval);
        let left: Vec<String> = rec
            .awaiting_gates()
            .into_iter()
            .map(|g| g.step_id)
            .collect();
        assert_eq!(left, vec!["gate_a".to_string()]);
    }

    #[test]
    fn rejecting_a_gate_records_it_and_asks_for_the_on_reject_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let store = parked_store(
            tmp.path(),
            GATES_YAML,
            &[("gate_a", None), ("gate_b", None)],
        );
        let wf = parse(GATES_YAML);

        let out = decide_gate(
            &store,
            &wf,
            "run_t",
            &gate_view("gate_a", None),
            GateVerb::Reject,
            "op",
            Utc::now(),
        )
        .unwrap();
        assert_eq!(
            out,
            GateFollowUp::Cleanup {
                step_id: "gate_a".into(),
                reason: REJECT_REASON.into(),
                via: "human",
                approver: "op".into(),
            }
        );
        let left: Vec<String> = store
            .load("run_t")
            .unwrap()
            .awaiting_gates()
            .into_iter()
            .map(|g| g.step_id)
            .collect();
        assert_eq!(left, vec!["gate_b".to_string()]);
    }

    #[test]
    fn a_decision_that_cannot_be_recorded_is_a_one_line_message_and_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = parked_store(
            tmp.path(),
            GATES_YAML,
            &[("gate_a", None), ("gate_b", None)],
        );
        let wf = parse(GATES_YAML);
        let try_it = |step: &str, verb| {
            decide_gate(
                &store,
                &wf,
                "run_t",
                &gate_view(step, None),
                verb,
                "op",
                Utc::now(),
            )
        };

        let err = try_it("no_such_gate", GateVerb::Approve).unwrap_err();
        assert!(
            err.starts_with("approve failed:") && err.contains("no_such_gate"),
            "{err}"
        );
        let err = try_it("no_such_gate", GateVerb::Reject).unwrap_err();
        assert!(err.starts_with("reject failed:"), "{err}");
        assert!(!err.contains('\n'), "one line: {err:?}");
        assert_eq!(store.load("run_t").unwrap().awaiting_gates().len(), 2);

        // A second approve of an already-decided gate is refused, not applied.
        try_it("gate_a", GateVerb::Approve).unwrap();
        let err = try_it("gate_a", GateVerb::Approve).unwrap_err();
        assert!(err.starts_with("approve failed:"), "{err}");
    }

    const TIMED_YAML: &str = "name: g\nsteps:\n  - id: gate_a\n    approval:\n      \
         timeout_seconds: 60\n      on_timeout: approve\n  - id: gate_b\n    approval:\n      \
         timeout_seconds: 60\n      on_timeout: reject\n";

    #[test]
    fn a_decision_on_an_overdue_gate_is_attributed_to_its_timeout_policy() {
        let wf = parse(TIMED_YAML);
        let now = Utc::now();
        let past = now - chrono::Duration::seconds(5);
        let future = now + chrono::Duration::seconds(50);

        assert_eq!(
            overdue_policy(&wf, &gate_view("gate_a", Some(past)), now),
            Some(TimeoutAction::Approve)
        );
        assert_eq!(
            overdue_policy(&wf, &gate_view("gate_b", Some(past)), now),
            Some(TimeoutAction::Reject)
        );
        // Not overdue, no deadline, or not a gate: no policy has fired.
        assert_eq!(
            overdue_policy(&wf, &gate_view("gate_a", Some(future)), now),
            None
        );
        assert_eq!(overdue_policy(&wf, &gate_view("gate_a", None), now), None);
        assert_eq!(
            overdue_policy(&wf, &gate_view("nope", Some(past)), now),
            None
        );

        // Approving the overdue `on_timeout: approve` gate resumes it as a
        // timeout decision (`via: "timeout"`), as the CLI records it.
        let tmp = tempfile::tempdir().unwrap();
        let store = parked_store(tmp.path(), TIMED_YAML, &[("gate_a", Some(past))]);
        let out = decide_gate(
            &store,
            &wf,
            "run_t",
            &gate_view("gate_a", Some(past)),
            GateVerb::Approve,
            "op",
            now,
        )
        .unwrap();
        assert!(
            matches!(
                &out,
                GateFollowUp::Resume {
                    via_timeout: true,
                    ..
                }
            ),
            "{out:?}"
        );

        // Rejecting the overdue `on_timeout: reject` gate is the policy's call.
        let tmp = tempfile::tempdir().unwrap();
        let store = parked_store(tmp.path(), TIMED_YAML, &[("gate_b", Some(past))]);
        let out = decide_gate(
            &store,
            &wf,
            "run_t",
            &gate_view("gate_b", Some(past)),
            GateVerb::Reject,
            "op",
            now,
        )
        .unwrap();
        assert!(
            matches!(&out, GateFollowUp::Cleanup { via: "timeout", .. }),
            "{out:?}"
        );
    }

    // ── the footer and the keys agree ───────────────────────────────────

    #[test]
    fn a_parked_run_replayed_from_disk_shows_the_gate_keys_and_they_act() {
        use crate::output::live_view::layout::footer_line;
        use crate::output::live_view::row::render_plain;
        use std::io::Write as _;

        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join("runs");
        let run_dir = runs.join("run_P");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("run.json"),
            serde_json::json!({
                "id": "run_P",
                "workflow_name": "publish",
                "status": "awaiting_approval",
                "inputs": {},
                "workspace_id": "ws",
                "workspace_path": tmp.path(),
                "transcript_dir": tmp.path(),
                "started_at": "2026-09-30T10:00:00Z",
                "codename": "mint-tundra",
                "awaiting": [{
                    "step_id": "approve",
                    "prompt": "publish the report?",
                    "since": "2026-09-30T11:00:00Z"
                }]
            })
            .to_string(),
        )
        .unwrap();
        let mut f = std::fs::File::create(run_dir.join("events.jsonl")).unwrap();
        for line in [
            r#"{"type":"run_started","event_version":1,"run_id":"run_P","workflow_path":"wf","started_at":"2026-09-30T10:00:00Z"}"#,
            r#"{"type":"step_started","run_id":"run_P","step_id":"build","kind":"run","agent":null}"#,
            r#"{"type":"step_completed","run_id":"run_P","step_id":"build","success":true,"duration_ms":18000}"#,
            r#"{"type":"step_started","run_id":"run_P","step_id":"approve","kind":"approval_gate","agent":null}"#,
            r#"{"type":"step_awaiting_approval","run_id":"run_P","step_id":"approve","reason":"publish?"}"#,
            r#"{"type":"step_started","run_id":"run_P","step_id":"deploy","kind":"run","agent":null}"#,
        ] {
            writeln!(f, "{line}").unwrap();
        }
        drop(f);

        let store = RunStore::new(runs);
        let view = RunView::from_run_dir(&store, "run_P", &rupu_config::PricingConfig::default());
        assert_eq!(view.status, RunStatus::AwaitingApproval);
        let at = |nav: &NavState| -> (String, bool) {
            let footer = render_plain(std::slice::from_ref(&footer_line(&view, nav, 80)));
            let acts = decode_key(KeyCode::Char('a'), NONE, nav.focused_gate(&view).is_some())
                == Some(KeyAction::Approve);
            (footer, acts)
        };

        // Following a parked run: the gate is focused with no navigation,
        // the footer advertises approve, and `a` really approves.
        let (footer, acts) = at(&NavState::default());
        assert!(footer.starts_with("a approve · r reject"), "{footer}");
        assert!(acts);

        // Select a step that is not the gate: the legend is the navigation
        // set and `a` is auto-follow — the two can never disagree.
        let mut nav = NavState::default();
        nav.apply(NavKey::Up, &view); // step 0 = `build`, chosen by hand
        let (footer, acts) = at(&nav);
        assert!(footer.starts_with("↑↓ move"), "{footer}");
        assert!(!acts);
        // `a` there is Follow, which returns to the auto-focused gate.
        assert_eq!(
            decode_key(KeyCode::Char('a'), NONE, false),
            nav_key(NavKey::Follow)
        );
        nav.apply(NavKey::Follow, &view);
        assert!(at(&nav).1);
    }

    fn nav_key(k: NavKey) -> Option<KeyAction> {
        Some(KeyAction::Nav(k))
    }

    #[test]
    fn notices_are_single_scrubbed_rows() {
        use crate::output::live_view::row::render_plain;
        let line = notice_line("pause\u{1b}[2J requested\nnow", false);
        let plain = render_plain(&[line]);
        assert!(plain.starts_with("» pause"), "{plain:?}");
        assert!(
            !plain.contains('\u{1b}') && !plain.contains('\n'),
            "{plain:?}"
        );
    }

    // ── the loop's state, driven without a terminal ─────────────────────

    const RUN_AND_GATES_YAML: &str = "name: g\nsteps:\n  - id: build\n    run:\n      \
         cmd: echo\n  - id: gate_a\n    approval: {}\n  - id: gate_b\n    approval: {}\n";

    /// A `Live` over a run parked at `gate_a` + `gate_b` (after a `build`
    /// step), plus its store. No terminal is touched.
    fn parked_live(tmp: &Path) -> (Live, RunStore) {
        let store = parked_store(
            tmp,
            RUN_AND_GATES_YAML,
            &[("gate_a", None), ("gate_b", None)],
        );
        let live = Live::new(
            parse(RUN_AND_GATES_YAML),
            tmp.join("runs"),
            "run_t".into(),
            rupu_config::PricingConfig::default(),
        );
        (live, store)
    }

    fn notice_text(live: &Live) -> Option<String> {
        use crate::output::live_view::row::render_plain;
        live.notice
            .as_ref()
            .map(|n| render_plain(std::slice::from_ref(&n.line)))
    }

    #[tokio::test]
    async fn a_parked_run_focuses_its_first_gate_and_a_wandering_operator_is_told_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut live, store) = parked_live(tmp.path());
        live.ingest();
        assert_eq!(live.view.status, RunStatus::AwaitingApproval);
        assert_eq!(
            live.nav
                .focused_gate(&live.view)
                .map(|g| g.step_id.as_str()),
            Some("gate_a")
        );
        // Following a parked run needs no hint: the gate is already focused.
        assert!(notice_text(&live).is_none());

        // The operator moves off the gates (onto `build`): they keep their
        // selection, and are told once that the run is waiting.
        live.apply_action(KeyAction::Nav(NavKey::Up)).await;
        live.ingest();
        assert_eq!(live.nav.focused_gate(&live.view).map(|_| ()), None);
        let hint = notice_text(&live).expect("a parked-run notice");
        assert!(
            hint.contains("gate_a") && hint.contains("press a"),
            "{hint}"
        );
        // Once per park: clearing it and ticking again does not re-announce.
        live.notice = None;
        live.ingest();
        assert!(notice_text(&live).is_none());

        // `a` (follow) brings the gate back into focus, and the pointer
        // that said how to get there goes away.
        live.ingest();
        live.notice = None;
        live.parked_hint = None;
        live.ingest();
        assert!(notice_text(&live).is_some());
        live.apply_action(KeyAction::Nav(NavKey::Follow)).await;
        live.ingest();
        assert!(live.nav.focused_gate(&live.view).is_some());
        assert!(notice_text(&live).is_none());

        // Both gates decided elsewhere: the run is no longer parked and the
        // next park would be announced afresh.
        store
            .approve_gate("run_t", "op", Utc::now(), Some("gate_a"))
            .unwrap();
        store
            .approve_gate("run_t", "op", Utc::now(), Some("gate_b"))
            .unwrap();
        live.ingest();
        assert_ne!(live.view.status, RunStatus::AwaitingApproval);
        assert!(live.view.gates.is_empty());
        assert_eq!(live.parked_hint, None);
    }

    #[tokio::test]
    async fn the_parked_hint_names_the_gate_the_follow_key_lands_on() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut live, _store) = parked_live(tmp.path());
        live.ingest();
        // The operator wanders off the gates (onto `build`).
        live.apply_action(KeyAction::Nav(NavKey::Up)).await;
        // run.json's awaiting set lists `gate_b` first, but step order — what
        // `a` (follow) focuses — puts `gate_a` first: the hint must agree
        // with the key, not with the set's order.
        live.view.gates.reverse();
        live.notice = None;
        live.parked_hint = None;
        live.hint_parked_gate(false);
        let hint = notice_text(&live).expect("a parked-run notice");
        assert!(
            hint.contains("gate_a") && !hint.contains("gate_b"),
            "{hint}"
        );
        live.apply_action(KeyAction::Nav(NavKey::Follow)).await;
        assert_eq!(
            live.nav
                .focused_gate(&live.view)
                .map(|g| g.step_id.as_str()),
            Some("gate_a")
        );
    }

    #[tokio::test]
    async fn quit_leaves_pause_only_asks_and_a_stale_gate_key_is_a_notice_not_an_action() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut live, store) = parked_live(tmp.path());
        live.ingest();

        // `q` / Ctrl-C leave; neither touches the run.
        assert_eq!(
            live.apply_action(KeyAction::Nav(NavKey::Quit)).await,
            Some(Exit::Quit)
        );
        assert_eq!(
            store.load("run_t").unwrap().status,
            RunStatus::AwaitingApproval
        );

        // `Esc` on a parked run cannot pause it: a one-line notice says why,
        // and the record is untouched.
        assert_eq!(live.apply_action(KeyAction::Nav(NavKey::Pause)).await, None);
        let note = notice_text(&live).expect("a notice");
        assert!(note.contains("only a running run can be paused"), "{note}");
        assert_eq!(
            store.load("run_t").unwrap().status,
            RunStatus::AwaitingApproval
        );

        // Someone else decides `gate_a` first; this view still shows it. The
        // approve is refused (nothing spawned) and the gate leaves the view.
        store
            .approve_gate("run_t", "other", Utc::now(), Some("gate_a"))
            .unwrap();
        live.apply_action(KeyAction::Approve).await;
        let note = notice_text(&live).expect("a notice");
        assert!(note.starts_with("» approve failed:"), "{note}");
        assert!(
            live.background.is_empty(),
            "a refused decision spawns nothing"
        );
        assert_eq!(
            live.nav
                .focused_gate(&live.view)
                .map(|g| g.step_id.as_str()),
            Some("gate_b"),
            "the view dropped the decided gate and focuses the next"
        );
    }

    #[tokio::test]
    async fn the_gate_panel_replaces_the_feed_while_v_is_on_and_the_notice_stays_last() {
        use crate::output::live_view::row::render_plain;
        let tmp = tempfile::tempdir().unwrap();
        let (mut live, _store) = parked_live(tmp.path());
        live.ingest();
        let plain = |lines: &[Line]| render_plain(lines);

        assert!(plain(&live.build_feed()).is_empty());
        live.apply_action(KeyAction::ToggleDetails).await;
        live.set_notice("approved nothing", false);
        let feed = plain(&live.build_feed());
        assert!(feed.contains("⏸ gate_a · parked"), "{feed}");
        assert!(feed.contains("approve gate_a?"), "{feed}");
        assert!(feed.ends_with("» approved nothing"), "{feed}");

        // Toggling off — or losing the gate's focus — restores the feed.
        live.apply_action(KeyAction::ToggleDetails).await;
        assert!(!plain(&live.build_feed()).contains("gate_a"));
        live.apply_action(KeyAction::ToggleDetails).await;
        live.apply_action(KeyAction::Nav(NavKey::Up)).await; // off the gates
        live.ingest();
        assert!(!live.show_details);
    }

    #[test]
    fn a_frame_is_painted_in_full_then_only_what_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut live, _store) = parked_live(tmp.path());
        live.ingest();
        let mut first: Vec<u8> = Vec::new();
        live.draw(&mut first, (80, 24));
        let first = String::from_utf8_lossy(&first).to_string();
        assert!(first.contains("approve"), "{first:?}");
        assert!(first.contains("gate_a"), "{first:?}");

        // Nothing but the clock can have changed: the unchanged rows (the
        // gate's, the footer's) are not written again.
        let mut second: Vec<u8> = Vec::new();
        live.draw(&mut second, (80, 24));
        let second = String::from_utf8_lossy(&second).to_string();
        assert!(!second.contains("gate_a"), "{second:?}");
        assert!(!second.contains("approve"), "{second:?}");

        // A resize invalidates the renderer: the next frame clears and
        // repaints everything.
        live.renderer.invalidate();
        let mut third: Vec<u8> = Vec::new();
        live.draw(&mut third, (60, 20));
        let third = String::from_utf8_lossy(&third).to_string();
        assert!(third.contains("gate_a"), "{third:?}");
    }

    #[test]
    fn the_loops_frame_composes_three_panes_and_the_gate_footer() {
        use crate::output::live_view::row::render_plain;
        let tmp = tempfile::tempdir().unwrap();
        let (mut live, _store) = parked_live(tmp.path());
        live.ingest();
        // Drive the exact path the draw loop uses, at a wide size.
        let frame = live.frame((100, 28));
        let s = render_plain(&frame);

        // The three panes compose: a structure column, a stream pane, and the
        // firehose rule under it.
        assert!(s.contains("─ structure"), "{s}");
        assert!(s.contains("─ stream · "), "{s}");
        assert!(s.contains("├─ live · "), "{s}");
        // The parked gate is on screen in the structure column.
        assert!(s.contains("gate_a"), "{s}");
        // Following a parked run auto-focuses its gate, so the footer is the
        // modal approve / reject legend.
        assert_eq!(
            render_plain(&frame[frame.len() - 1..]),
            "a approve · r reject · v findings · Esc pause · q quit",
            "{s}"
        );
    }
}
