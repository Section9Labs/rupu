//! Live workflow run view — pure state + renderers.
//!
//! `LiveRunState` accumulates step / unit / panel status, run totals,
//! and the active agent's rolling activity feed from two event streams:
//!   - workflow step events ([`rupu_orchestrator::executor::Event`],
//!     applied via [`LiveRunState::apply`]) drive the dashboard + graph,
//!   - the active step's transcript events (pushed via
//!     [`LiveRunState::push_activity`]) drive the focus feed.
//!
//! The three zone renderers ([`render_dashboard`], [`render_graph`],
//! [`render_focus`]) and the combined [`render_view`] are PURE — they
//! take a state + dimensions and return ANSI-colored `Vec<String>`. They
//! are the unit-tested core. The live loop (cursor control, the render
//! tick) lives in `cmd/workflow.rs` and is validated by running it.

use chrono::{DateTime, Utc};
use rupu_app_canvas::NodeStatus;
use rupu_orchestrator::executor::Event as WfEvent;
use rupu_orchestrator::{RunStatus, StepKind, Workflow};

use crate::output::fmt::{format_cost_compact, format_token_compact};
use crate::output::palette::{self, BRAND, COMPLETE, DIM, FAILED, RUNNING};
use crate::output::printer::visible_len;

/// Braille spinner frames cycled by the render tick.
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Cap on the number of activity lines retained for the focus feed.
/// The renderer windows to the available height; this just bounds memory.
const ACTIVITY_CAP: usize = 200;

/// Return the spinner glyph for `tick`, cycling the braille frames.
pub fn spinner_frame(tick: u64) -> char {
    SPINNER_FRAMES[(tick as usize) % SPINNER_FRAMES.len()]
}

/// Map a [`NodeStatus`] to its live-view glyph. Distinct from the
/// canvas `NodeStatus::glyph` so pending/never-reached uses `◌`.
fn node_glyph(status: NodeStatus) -> char {
    match status {
        NodeStatus::Waiting => '○',
        NodeStatus::Active | NodeStatus::Working => '◐',
        NodeStatus::Complete => '✓',
        NodeStatus::Failed => '✗',
        NodeStatus::SoftFailed => '✗',
        NodeStatus::Awaiting => '⏸',
        NodeStatus::Retrying => '↻',
        NodeStatus::Skipped => '◌',
    }
}

fn node_color(status: NodeStatus) -> owo_colors::Rgb {
    match status {
        NodeStatus::Waiting | NodeStatus::Skipped => DIM,
        NodeStatus::Active | NodeStatus::Working => RUNNING,
        NodeStatus::Complete => COMPLETE,
        NodeStatus::Failed | NodeStatus::SoftFailed => FAILED,
        NodeStatus::Awaiting => palette::AWAITING,
        NodeStatus::Retrying => palette::RETRYING,
    }
}

/// One unit (a `for_each` item or `parallel` sub-step) of a fan-out step.
#[derive(Debug, Clone)]
pub struct UnitState {
    pub key: String,
    pub status: NodeStatus,
    pub tokens: u64,
    pub elapsed_secs: u64,
}

/// One line in the active agent's rolling activity feed.
#[derive(Debug, Clone)]
pub struct ActivityLine {
    pub ts: DateTime<Utc>,
    pub kind: ActivityKind,
    pub text: String,
}

/// The visual class of an activity line — picks the leading glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    ToolCall,
    Finding,
    Coverage,
    Text,
}

impl ActivityKind {
    fn glyph(self) -> char {
        match self {
            ActivityKind::ToolCall => '▸',
            ActivityKind::Finding => '⚑',
            ActivityKind::Coverage => '✓',
            ActivityKind::Text => '·',
        }
    }

    fn color(self) -> owo_colors::Rgb {
        match self {
            ActivityKind::ToolCall => palette::TOOL_ARROW,
            ActivityKind::Finding => palette::SEV_HIGH,
            ActivityKind::Coverage => COMPLETE,
            ActivityKind::Text => DIM,
        }
    }
}

/// Per-step accumulated state.
#[derive(Debug, Clone)]
pub struct StepState {
    pub id: String,
    pub kind: StepKind,
    pub agent: Option<String>,
    pub status: NodeStatus,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub elapsed_secs: u64,
    /// Fan-out (`for_each` / `parallel`) units. Empty for linear/panel.
    pub units: Vec<UnitState>,
    /// `for_each`/`parallel` completed-unit count summary (done, total).
    pub fanout_total: Option<usize>,
    /// Panel iteration counters: (current_iteration, max_iterations).
    pub panel_iter: Option<(u32, u32)>,
    /// Panel findings observed so far.
    pub panel_findings: usize,
}

impl StepState {
    fn done_units(&self) -> usize {
        self.units
            .iter()
            .filter(|u| matches!(u.status, NodeStatus::Complete | NodeStatus::Failed))
            .count()
    }
}

/// The currently-active focus: which step + (optional) unit is running,
/// plus the rolling transcript feed and the last-event timestamp used by
/// the heartbeat.
#[derive(Debug, Clone, Default)]
pub struct ActiveFocus {
    pub step_id: Option<String>,
    pub unit_key: Option<String>,
    pub agent: Option<String>,
    /// Transcript of the active fan-out UNIT (set by `UnitStarted`). The
    /// live loop tails this in preference to `run.json`'s active-step
    /// transcript, which is null during a fan-out. `None` for linear
    /// steps (the loop falls back to the active-step transcript).
    pub active_unit_transcript: Option<std::path::PathBuf>,
    pub feed: Vec<ActivityLine>,
    pub last_event_at: Option<DateTime<Utc>>,
}

/// Pure, accumulating state for the live run view.
#[derive(Debug, Clone)]
pub struct LiveRunState {
    pub workflow_name: String,
    pub run_id: String,
    pub status: RunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Steps in declared order; index aligns with the workflow's steps.
    pub steps: Vec<StepState>,
    /// Run totals.
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: Option<f64>,
    pub findings_count: Option<usize>,
    pub coverage_pct: Option<u8>,
    pub active: ActiveFocus,
}

impl LiveRunState {
    /// Build the initial state from the parsed workflow + a run id. Every
    /// step starts `Waiting`; the first `StepStarted` event activates one.
    pub fn from_workflow(workflow: &Workflow, run_id: impl Into<String>) -> Self {
        let steps = workflow
            .steps
            .iter()
            .map(|step| {
                let kind = step_kind(step);
                StepState {
                    id: step.id.clone(),
                    kind,
                    agent: step.agent.clone(),
                    status: NodeStatus::Waiting,
                    tokens_in: 0,
                    tokens_out: 0,
                    elapsed_secs: 0,
                    units: Vec::new(),
                    fanout_total: None,
                    panel_iter: step
                        .panel
                        .as_ref()
                        .and_then(|p| p.gate.as_ref().map(|g| (0, g.max_iterations))),
                    panel_findings: 0,
                }
            })
            .collect();
        Self {
            workflow_name: workflow.name.clone(),
            run_id: run_id.into(),
            status: RunStatus::Running,
            started_at: None,
            finished_at: None,
            steps,
            tokens_in: 0,
            tokens_out: 0,
            cost: None,
            findings_count: None,
            coverage_pct: None,
            active: ActiveFocus::default(),
        }
    }

    fn step_mut(&mut self, step_id: &str) -> Option<&mut StepState> {
        self.steps.iter_mut().find(|s| s.id == step_id)
    }

    /// Number of completed steps (for the progress bar).
    pub fn completed_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| matches!(s.status, NodeStatus::Complete | NodeStatus::Skipped))
            .count()
    }

    /// Status lookup for the graph renderer, keyed by step id.
    pub fn status_for(&self, step_id: &str) -> NodeStatus {
        self.steps
            .iter()
            .find(|s| s.id == step_id)
            .map(|s| s.status)
            .unwrap_or(NodeStatus::Waiting)
    }

    /// Apply a workflow step event, mutating step / run status.
    pub fn apply(&mut self, event: &WfEvent) {
        match event {
            WfEvent::RunStarted { started_at, .. } => {
                self.started_at = Some(*started_at);
                self.status = RunStatus::Running;
            }
            WfEvent::StepStarted { step_id, agent, .. } => {
                self.active.step_id = Some(step_id.clone());
                self.active.unit_key = None;
                self.active.agent = agent.clone();
                self.active.feed.clear();
                self.active.last_event_at = None;
                if let Some(step) = self.step_mut(step_id) {
                    step.status = NodeStatus::Active;
                    if agent.is_some() {
                        step.agent = agent.clone();
                    }
                }
            }
            WfEvent::StepWorking { step_id, .. } => {
                if let Some(step) = self.step_mut(step_id) {
                    if !matches!(step.status, NodeStatus::Complete | NodeStatus::Failed) {
                        step.status = NodeStatus::Working;
                    }
                }
            }
            WfEvent::StepAwaitingApproval { step_id, .. } => {
                if let Some(step) = self.step_mut(step_id) {
                    step.status = NodeStatus::Awaiting;
                }
            }
            WfEvent::StepCompleted {
                step_id,
                success,
                duration_ms,
                ..
            } => {
                let secs = duration_ms / 1000;
                if let Some(step) = self.step_mut(step_id) {
                    step.status = if *success {
                        NodeStatus::Complete
                    } else {
                        NodeStatus::Failed
                    };
                    step.elapsed_secs = secs;
                }
            }
            WfEvent::StepFailed { step_id, .. } => {
                if let Some(step) = self.step_mut(step_id) {
                    step.status = NodeStatus::Failed;
                }
            }
            WfEvent::StepSkipped { step_id, .. } => {
                if let Some(step) = self.step_mut(step_id) {
                    step.status = NodeStatus::Skipped;
                }
            }
            WfEvent::UnitStarted {
                step_id,
                index,
                unit_key,
                agent,
                transcript_path,
                ..
            } => {
                // Re-focus on this unit: its transcript drives the feed.
                self.active.step_id = Some(step_id.clone());
                self.active.unit_key = Some(unit_key.clone());
                if agent.is_some() {
                    self.active.agent = agent.clone();
                }
                self.active.active_unit_transcript = Some(transcript_path.clone());
                self.active.feed.clear();
                self.active.last_event_at = None;
                if let Some(step) = self.step_mut(step_id) {
                    if !matches!(step.status, NodeStatus::Complete | NodeStatus::Failed) {
                        step.status = NodeStatus::Working;
                    }
                    ensure_unit_slot(&mut step.units, *index);
                    let unit = &mut step.units[*index];
                    unit.key = unit_key.clone();
                    unit.status = NodeStatus::Working;
                }
            }
            WfEvent::UnitCompleted {
                step_id,
                index,
                unit_key,
                success,
                tokens_in,
                tokens_out,
                ..
            } => {
                if let Some(step) = self.step_mut(step_id) {
                    ensure_unit_slot(&mut step.units, *index);
                    let unit = &mut step.units[*index];
                    if unit.key.is_empty() {
                        unit.key = unit_key.clone();
                    }
                    unit.status = if *success {
                        NodeStatus::Complete
                    } else {
                        NodeStatus::Failed
                    };
                    unit.tokens += tokens_in + tokens_out;
                    step.tokens_in += tokens_in;
                    step.tokens_out += tokens_out;
                    // Keep the collapsed `done/total` summary coherent: at
                    // minimum the step has as many units as the highest
                    // index seen so far.
                    let seen = step.units.len();
                    step.fanout_total = Some(step.fanout_total.unwrap_or(0).max(seen));
                }
                self.tokens_in += tokens_in;
                self.tokens_out += tokens_out;
            }
            WfEvent::RunCompleted {
                status,
                finished_at,
                ..
            } => {
                self.status = *status;
                self.finished_at = Some(*finished_at);
                self.active.step_id = None;
            }
            WfEvent::RunFailed { finished_at, .. } => {
                self.status = RunStatus::Failed;
                self.finished_at = Some(*finished_at);
            }
        }
    }

    /// Append a line to the active agent's rolling feed and bump the
    /// heartbeat timestamp. Caps the feed at [`ACTIVITY_CAP`].
    pub fn push_activity(
        &mut self,
        ts: DateTime<Utc>,
        kind: ActivityKind,
        text: impl Into<String>,
    ) {
        self.active.last_event_at = Some(ts);
        self.active.feed.push(ActivityLine {
            ts,
            kind,
            text: text.into(),
        });
        if self.active.feed.len() > ACTIVITY_CAP {
            let overflow = self.active.feed.len() - ACTIVITY_CAP;
            self.active.feed.drain(0..overflow);
        }
    }

    /// Record per-step token deltas (from transcript `Usage` events).
    pub fn add_step_tokens(&mut self, step_id: &str, tokens_in: u64, tokens_out: u64) {
        if let Some(step) = self.step_mut(step_id) {
            step.tokens_in += tokens_in;
            step.tokens_out += tokens_out;
        }
        self.tokens_in += tokens_in;
        self.tokens_out += tokens_out;
    }

    /// Seconds since the run started, given a `now`.
    pub fn elapsed_secs(&self, now: DateTime<Utc>) -> i64 {
        self.started_at
            .map(|s| (now - s).num_seconds().max(0))
            .unwrap_or(0)
    }

    fn status_label(&self) -> (&'static str, owo_colors::Rgb) {
        match self.status {
            RunStatus::Running => ("running", RUNNING),
            RunStatus::Completed => ("completed", COMPLETE),
            RunStatus::Failed => ("failed", FAILED),
            RunStatus::AwaitingApproval => ("awaiting", palette::AWAITING),
            RunStatus::Rejected => ("rejected", FAILED),
            RunStatus::Pending => ("pending", DIM),
        }
    }
}

/// Grow `units` so that `index` is addressable, filling any gap with
/// `Waiting` placeholder units. Fan-out events arrive per-unit (in real
/// start/finish order under concurrency), so the vector may be sparse
/// until every unit has started.
fn ensure_unit_slot(units: &mut Vec<UnitState>, index: usize) {
    if units.len() <= index {
        units.resize_with(index + 1, || UnitState {
            key: String::new(),
            status: NodeStatus::Waiting,
            tokens: 0,
            elapsed_secs: 0,
        });
    }
}

fn step_kind(step: &rupu_orchestrator::Step) -> StepKind {
    if step.panel.is_some() {
        StepKind::Panel
    } else if step.parallel.is_some() {
        StepKind::Parallel
    } else if step.for_each.is_some() {
        StepKind::ForEach
    } else {
        StepKind::Linear
    }
}

/// Format an `Ns` / `Nm Ss` elapsed string.
fn fmt_elapsed(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

// ---------------------------------------------------------------------------
// Zone 1 — Dashboard
// ---------------------------------------------------------------------------

/// Render Zone 1: title + status + elapsed, a rule, the `step N/M`
/// progress bar, and the adaptive meters row.
pub fn render_dashboard(state: &LiveRunState, now: DateTime<Utc>, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let (label, color) = state.status_label();
    let elapsed = fmt_elapsed(state.elapsed_secs(now));

    // Title line: brand name left, `status · elapsed` right.
    let right = format!("{label} · {elapsed}");
    let mut title = String::new();
    let _ = palette::write_bold_colored(&mut title, &state.workflow_name, BRAND);
    let used = visible_len(&title) + visible_len(&right);
    let pad = width.saturating_sub(used);
    title.push_str(&" ".repeat(pad));
    let _ = palette::write_colored(&mut title, &right, color);
    rows.push(title);

    // Horizontal rule.
    let mut rule = String::new();
    let _ = palette::write_colored(&mut rule, &"─".repeat(width), palette::SEPARATOR);
    rows.push(rule);

    // Progress bar: `step N/M  [bar]  active_step`.
    let total = state.steps.len().max(1);
    let done = state.completed_steps();
    let active_name = state
        .active
        .step_id
        .as_deref()
        .or_else(|| {
            state
                .steps
                .iter()
                .find(|s| matches!(s.status, NodeStatus::Active | NodeStatus::Working))
                .map(|s| s.id.as_str())
        })
        .unwrap_or("");
    let bar_w = 24usize;
    let filled = (done * bar_w).checked_div(total).unwrap_or(0).min(bar_w);
    let mut prog = String::new();
    let _ = palette::write_colored(&mut prog, &format!("step {done}/{total}   "), DIM);
    let _ = palette::write_colored(&mut prog, &"█".repeat(filled), RUNNING);
    let _ = palette::write_colored(&mut prog, &"░".repeat(bar_w - filled), DIM);
    if !active_name.is_empty() {
        prog.push_str("   ");
        let _ = palette::write_bold_colored(&mut prog, active_name, BRAND);
    }
    rows.push(prog);

    // Meters row, adaptive.
    let mut meters = String::new();
    let _ = palette::write_colored(
        &mut meters,
        &format!("⇡ {}   ", format_token_compact(state.tokens_in)),
        DIM,
    );
    let _ = palette::write_colored(
        &mut meters,
        &format!("⇣ {}   ", format_token_compact(state.tokens_out)),
        DIM,
    );
    let cost = state.cost.unwrap_or(0.0);
    let _ = palette::write_colored(&mut meters, &format_cost_compact(cost), COMPLETE);
    if let Some(findings) = state.findings_count.filter(|n| *n > 0) {
        let _ = palette::write_colored(&mut meters, "          ", DIM);
        let _ = palette::write_colored(
            &mut meters,
            &format!("findings {findings}"),
            palette::SEV_HIGH,
        );
    }
    if let Some(pct) = state.coverage_pct {
        let _ = palette::write_colored(&mut meters, "          ", DIM);
        let _ = palette::write_colored(&mut meters, "coverage ", DIM);
        let cov_w = 8usize;
        let cov_filled = (pct as usize * cov_w) / 100;
        let _ = palette::write_colored(&mut meters, &"█".repeat(cov_filled), COMPLETE);
        let _ = palette::write_colored(&mut meters, &"░".repeat(cov_w - cov_filled), DIM);
        let _ = palette::write_colored(&mut meters, &format!(" {pct}%"), DIM);
    }
    rows.push(meters);

    rows
}

// ---------------------------------------------------------------------------
// Zone 2 — Git-graph spine
// ---------------------------------------------------------------------------

/// A dotted-leader right-aligned line: `<glyph> <label> ····· <status>`.
fn leader_line(
    prefix: &str,
    prefix_status: NodeStatus,
    label: &str,
    label_color: owo_colors::Rgb,
    right: &str,
    right_color: owo_colors::Rgb,
    width: usize,
) -> String {
    // Compose the plain (uncolored) skeleton first to measure widths.
    let left_plain = format!("{prefix} {label} ");
    let right_plain = format!(" {right}");
    let dots = width
        .saturating_sub(visible_len(&left_plain) + visible_len(&right_plain))
        .max(1);

    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, prefix, node_color(prefix_status));
    buf.push(' ');
    let _ = palette::write_colored(&mut buf, label, label_color);
    buf.push(' ');
    let _ = palette::write_colored(&mut buf, &"·".repeat(dots), DIM);
    buf.push(' ');
    let _ = palette::write_colored(&mut buf, right, right_color);
    buf
}

/// Render Zone 2: the heavy-edged git-graph spine. Walks the workflow
/// steps and the live `LiveRunState`. The active fan-out step expands
/// its units as branch rows; inactive fan-outs collapse to `done/total`.
pub fn render_graph(state: &LiveRunState, _workflow: &Workflow, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let total = state.steps.len();

    for (i, step) in state.steps.iter().enumerate() {
        // Spine connector before each step (except the first).
        if i > 0 {
            let mut pipe = String::new();
            let _ = palette::write_colored(&mut pipe, "┃", DIM);
            rows.push(pipe);
        }

        let glyph = node_glyph(step.status).to_string();
        let agent = step.agent.clone().unwrap_or_default();

        match step.kind {
            StepKind::Panel => {
                let (iter, max) = step.panel_iter.unwrap_or((0, 0));
                let right = if matches!(step.status, NodeStatus::Active | NodeStatus::Working) {
                    format!("⟲ iter {iter}/{max} · {} found", step.panel_findings)
                } else {
                    status_word(step.status)
                };
                let label = format!("{} · panel", step.id);
                rows.push(leader_line(
                    &glyph,
                    step.status,
                    &label,
                    BRAND,
                    &right,
                    node_color(step.status),
                    width,
                ));
            }
            StepKind::ForEach | StepKind::Parallel => {
                let is_active = matches!(step.status, NodeStatus::Active | NodeStatus::Working);
                let total_units = step.fanout_total.unwrap_or(step.units.len());
                let done = step.done_units();
                let kind_word = if step.kind == StepKind::ForEach {
                    "for_each"
                } else {
                    "parallel"
                };
                let label = format!("{} · {kind_word}", step.id);

                if is_active && !step.units.is_empty() {
                    // Header line with the summary + total tokens.
                    let right = format!(
                        "{}/{} ⇡{}",
                        done,
                        total_units,
                        format_token_compact(step.tokens_in)
                    );
                    rows.push(leader_line(
                        &glyph,
                        step.status,
                        &label,
                        BRAND,
                        &right,
                        node_color(step.status),
                        width,
                    ));
                    // One branch row per unit.
                    let last = step.units.len() - 1;
                    for (ui, unit) in step.units.iter().enumerate() {
                        let branch = if ui == last { "┗━" } else { "┣━" };
                        let uglyph = node_glyph(unit.status);
                        let prefix = format!("{branch}{uglyph}");
                        let right = unit_right(unit);
                        rows.push(leader_line(
                            &prefix,
                            unit.status,
                            &unit.key,
                            node_color(unit.status),
                            &right,
                            node_color(unit.status),
                            width,
                        ));
                    }
                } else {
                    // Collapsed one-line summary.
                    let right = if total_units > 0 {
                        format!("{done}/{total_units}")
                    } else {
                        status_word(step.status)
                    };
                    rows.push(leader_line(
                        &glyph,
                        step.status,
                        &label,
                        BRAND,
                        &right,
                        node_color(step.status),
                        width,
                    ));
                }
            }
            StepKind::Linear => {
                let right = if matches!(step.status, NodeStatus::Complete) {
                    format!(
                        "{} {} ⇡{}",
                        node_glyph(step.status),
                        fmt_elapsed(step.elapsed_secs as i64),
                        format_token_compact(step.tokens_in)
                    )
                } else {
                    status_word(step.status)
                };
                let label = if agent.is_empty() {
                    step.id.clone()
                } else {
                    format!("{} · {agent}", step.id)
                };
                rows.push(leader_line(
                    &glyph,
                    step.status,
                    &label,
                    BRAND,
                    &right,
                    node_color(step.status),
                    width,
                ));
            }
        }
        let _ = total;
    }
    rows
}

fn unit_right(unit: &UnitState) -> String {
    match unit.status {
        NodeStatus::Complete => format!("✓  ⇡{}", format_token_compact(unit.tokens)),
        NodeStatus::Failed => format!("✗  ⇡{}", format_token_compact(unit.tokens)),
        NodeStatus::Active | NodeStatus::Working => {
            format!("working ⇡{}", format_token_compact(unit.tokens))
        }
        _ => status_word(unit.status),
    }
}

fn status_word(status: NodeStatus) -> String {
    match status {
        NodeStatus::Waiting => "queued".into(),
        NodeStatus::Active => "active".into(),
        NodeStatus::Working => "working".into(),
        NodeStatus::Complete => "done".into(),
        NodeStatus::Failed => "failed".into(),
        NodeStatus::SoftFailed => "soft-failed".into(),
        NodeStatus::Awaiting => "awaiting".into(),
        NodeStatus::Retrying => "retrying".into(),
        NodeStatus::Skipped => "pending".into(),
    }
}

// ---------------------------------------------------------------------------
// Zone 3 — Focus feed
// ---------------------------------------------------------------------------

/// Render Zone 3: a bordered panel for the active agent. Header shows
/// `unit · agent  ⇡tokens  ◐ active Ns ago`; body is the rolling feed,
/// windowed to `height` rows (including the two border rows). On a
/// failed run, the last body line is the `↳ rupu workflow resume` hint.
pub fn render_focus(
    state: &LiveRunState,
    now: DateTime<Utc>,
    width: usize,
    height: usize,
) -> Vec<String> {
    let inner = width.saturating_sub(4); // borders + padding
    let mut rows = Vec::new();

    // Header text.
    let agent = state
        .active
        .agent
        .clone()
        .unwrap_or_else(|| "—".to_string());
    let unit = state
        .active
        .unit_key
        .clone()
        .or_else(|| state.active.step_id.clone())
        .unwrap_or_else(|| "—".to_string());
    let header_left = format!("{unit} · {agent}");

    let heartbeat = match state.active.last_event_at {
        Some(ts) => {
            let secs = (now - ts).num_seconds().max(0);
            format!("◐ active {secs}s ago")
        }
        None => "◐ idle".to_string(),
    };

    // Top border: ╭ header_left ... heartbeat ╮
    let mut top = String::new();
    let _ = palette::write_colored(&mut top, "╭ ", palette::SEPARATOR);
    let _ = palette::write_bold_colored(&mut top, &header_left, BRAND);
    let mid_used = visible_len(&header_left) + visible_len(&heartbeat) + 4;
    let fill = width.saturating_sub(mid_used).max(1);
    let _ = palette::write_colored(&mut top, &" ".repeat(fill), palette::SEPARATOR);
    let _ = palette::write_colored(&mut top, &heartbeat, RUNNING);
    let _ = palette::write_colored(&mut top, " ╮", palette::SEPARATOR);
    rows.push(top);

    // Body window. Reserve 2 rows for borders.
    let body_rows = height.saturating_sub(2).max(1);
    let failed = matches!(state.status, RunStatus::Failed | RunStatus::Rejected);
    let resume_hint = failed.then(|| format!("↳ rupu workflow resume {}", state.run_id));
    let reserve = if resume_hint.is_some() { 1 } else { 0 };
    let feed_rows = body_rows.saturating_sub(reserve);

    let feed = &state.active.feed;
    let start = feed.len().saturating_sub(feed_rows);
    for line in &feed[start..] {
        rows.push(border_body_line(&format_activity(line, inner), width));
    }
    // Pad with blank body rows so the panel keeps a fixed height.
    let used = feed[start..].len() + reserve;
    for _ in used..body_rows {
        rows.push(border_body_line("", width));
    }
    if let Some(hint) = resume_hint {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, &hint, FAILED);
        rows.push(border_body_line_colored(buf, &hint, width));
    }

    // Bottom border.
    let mut bottom = String::new();
    let _ = palette::write_colored(
        &mut bottom,
        &format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
        palette::SEPARATOR,
    );
    rows.push(bottom);

    rows
}

/// Format one activity line as `HH:MM:SS  <glyph> <text>` (uncolored
/// content sized to `inner`), returning the colored string.
fn format_activity(line: &ActivityLine, inner: usize) -> String {
    let ts = line.ts.format("%H:%M:%S").to_string();
    let glyph = line.kind.glyph();
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, &ts, DIM);
    buf.push_str("  ");
    let _ = palette::write_colored(&mut buf, &glyph.to_string(), line.kind.color());
    buf.push(' ');
    let body = truncate_plain(&line.text, inner.saturating_sub(visible_len(&ts) + 4));
    let _ = palette::write_colored(&mut buf, &body, DIM);
    buf
}

/// Wrap a pre-colored content string in `│ … │` borders, padding to width.
fn border_body_line_colored(content_colored: String, content_plain: &str, width: usize) -> String {
    let inner = width.saturating_sub(4);
    let pad = inner.saturating_sub(visible_len(content_plain));
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "│ ", palette::SEPARATOR);
    buf.push_str(&content_colored);
    buf.push_str(&" ".repeat(pad));
    let _ = palette::write_colored(&mut buf, " │", palette::SEPARATOR);
    buf
}

/// Wrap an already-colored body string in borders. The body is measured
/// by stripping ANSI to compute padding.
fn border_body_line(content_colored: &str, width: usize) -> String {
    let inner = width.saturating_sub(4);
    let pad = inner.saturating_sub(visible_len(content_colored));
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "│ ", palette::SEPARATOR);
    buf.push_str(content_colored);
    buf.push_str(&" ".repeat(pad));
    let _ = palette::write_colored(&mut buf, " │", palette::SEPARATOR);
    buf
}

fn truncate_plain(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max <= 1 {
        "…".to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------------------
// Combined view
// ---------------------------------------------------------------------------

/// Stack the three zones into a single fixed-height block. Zone 3 grows
/// to fill the remaining terminal height below Zones 1 + 2.
pub fn render_view(
    state: &LiveRunState,
    workflow: &Workflow,
    now: DateTime<Utc>,
    term_width: usize,
    term_height: usize,
) -> Vec<String> {
    let width = term_width.max(20);
    let mut out = Vec::new();

    out.extend(render_dashboard(state, now, width));
    out.push(String::new());
    out.extend(render_graph(state, workflow, width));
    out.push(String::new());

    let used = out.len();
    let focus_height = term_height.saturating_sub(used).max(3);
    out.extend(render_focus(state, now, width, focus_height));

    out
}

// ---------------------------------------------------------------------------
// Live loop (best-effort; cursor control validated by running it)
// ---------------------------------------------------------------------------

/// Drive the live three-zone view in place until the run reaches a
/// terminal state. Tails `events.jsonl` for step status, reads
/// `run.json` for the active step's transcript path, and tails that
/// transcript for the focus feed. Repaints on a ~100ms tick and
/// immediately after each event batch. No alt-screen — the final frame
/// stays on the terminal.
///
/// Best-effort: any I/O hiccup degrades to the next tick. The caller
/// guards entry behind a tty check; non-tty falls back to the existing
/// line printer.
pub async fn run_live_view(
    workflow: Workflow,
    runs_dir: std::path::PathBuf,
    run_id: String,
) -> std::io::Result<()> {
    use crossterm::cursor::{Hide, MoveToColumn, MoveToPreviousLine, Show};
    use crossterm::style::Print;
    use crossterm::terminal::{self, Clear, ClearType};
    use crossterm::{execute, queue};
    use std::io::Write;

    let store = rupu_orchestrator::RunStore::new(runs_dir.clone());
    let events_path = runs_dir.join(&run_id).join("events.jsonl");
    let mut events_tailer = crate::output::jsonl_reader::WfEventTailer::new(&events_path);
    let mut state = LiveRunState::from_workflow(&workflow, &run_id);

    let mut transcript_tailer: Option<(std::path::PathBuf, crate::output::TranscriptTailer)> = None;
    let mut tick: u64 = 0;
    let mut prev_lines: u16 = 0;
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, Hide);

    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        interval.tick().await;
        tick += 1;

        // Drain workflow events. `UnitStarted` may re-point the focus to
        // a fan-out unit's transcript (see below).
        for ev in events_tailer.drain_events() {
            state.apply(&ev);
        }

        // Decide which transcript drives the focus feed. During a
        // fan-out the active UNIT's transcript (from the most recent
        // `UnitStarted`) wins; `run.json`'s active_step_transcript_path
        // is null then. For a linear step, fall back to that path.
        if let Ok(record) = store.load(&run_id) {
            if let Some(step_id) = record.active_step_id.clone() {
                // Don't clobber a fan-out unit focus set by UnitStarted.
                if state.active.active_unit_transcript.is_none() {
                    state.active.step_id = Some(step_id.clone());
                    if record.active_step_agent.is_some() {
                        state.active.agent = record.active_step_agent.clone();
                    }
                }
            }
        }
        let desired_transcript = state.active.active_unit_transcript.clone().or_else(|| {
            store
                .load(&run_id)
                .ok()
                .and_then(|r| r.active_step_transcript_path)
        });
        if let Some(path) = desired_transcript {
            let need_new = transcript_tailer
                .as_ref()
                .map(|(p, _)| p != &path)
                .unwrap_or(true);
            if need_new {
                transcript_tailer =
                    Some((path.clone(), crate::output::TranscriptTailer::new(path)));
                // `apply(UnitStarted)` already cleared the feed; clearing
                // again here is harmless and covers the linear re-point.
                state.active.feed.clear();
            }
        }
        if let Some((_, tailer)) = transcript_tailer.as_mut() {
            for ev in tailer.drain() {
                let now = Utc::now();
                if let Some((kind, text)) = map_transcript_event(&ev, now) {
                    state.push_activity(now, kind, text);
                }
            }
        }

        // Repaint.
        let now = Utc::now();
        let (cols, rows) = terminal::size().unwrap_or((100, 30));
        let _ = tick; // spinner advance is handled via heartbeat / glyphs
        let frame = render_view(
            &state,
            &workflow,
            now,
            cols as usize,
            (rows as usize).min(40),
        );

        if prev_lines > 0 {
            let _ = queue!(stdout, MoveToPreviousLine(prev_lines));
        }
        let _ = queue!(stdout, MoveToColumn(0), Clear(ClearType::FromCursorDown));
        for line in &frame {
            let _ = queue!(stdout, Print(line), Print("\r\n"));
        }
        let _ = stdout.flush();
        prev_lines = frame.len() as u16;

        if state.status.is_terminal() {
            break;
        }
    }

    // Final repaint already reflects the terminal state; leave it on
    // screen and restore the cursor.
    let _ = execute!(stdout, MoveToColumn(0), Show);
    Ok(())
}

// ---------------------------------------------------------------------------
// Transcript-event → activity mapping
// ---------------------------------------------------------------------------

/// Map a transcript event to a focus-feed activity line, if it should
/// surface. Returns `None` for events that don't add signal (turn
/// boundaries, run start/stop, deltas). Pure + unit-tested.
pub fn map_transcript_event(
    ev: &rupu_transcript::Event,
    now: DateTime<Utc>,
) -> Option<(ActivityKind, String)> {
    use rupu_transcript::Event as Tx;
    let _ = now;
    match ev {
        Tx::ToolCall { tool, input, .. } => {
            let arg = tool_key_arg(tool, input);
            let text = if arg.is_empty() {
                tool.clone()
            } else {
                format!("{tool}  {arg}")
            };
            Some((ActivityKind::ToolCall, text))
        }
        Tx::ActionEmitted { kind, payload, .. } => {
            // `report_finding` actions surface as findings; coverage
            // marks surface as coverage; others are skipped.
            if kind.contains("finding") {
                let sev = payload
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_uppercase();
                let title = payload
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or(kind);
                let text = if sev.is_empty() {
                    title.to_string()
                } else {
                    format!("{sev}  {title}")
                };
                Some((ActivityKind::Finding, text))
            } else if kind.contains("coverage") {
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("control_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(kind);
                Some((ActivityKind::Coverage, id.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract the single most informative argument for a tool call.
fn tool_key_arg(tool: &str, input: &serde_json::Value) -> String {
    let candidate = match tool {
        "read_file" | "write_file" | "edit_file" => input.get("path"),
        "grep" => input.get("pattern"),
        "bash" => input.get("command"),
        _ => None,
    };
    candidate
        .or_else(|| input.get("path"))
        .or_else(|| input.get("query"))
        .and_then(|v| v.as_str())
        .map(|s| truncate_plain(s, 48))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for inner in chars.by_ref() {
                    if inner == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn ts(sec: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 17, 17, 42, sec).unwrap()
    }

    fn fanout_state(active: bool) -> LiveRunState {
        let mut units = Vec::new();
        units.push(UnitState {
            key: "conf-manager".into(),
            status: NodeStatus::Complete,
            tokens: 210_000,
            elapsed_secs: 12,
        });
        units.push(UnitState {
            key: "tlb-agent".into(),
            status: NodeStatus::Complete,
            tokens: 180_000,
            elapsed_secs: 11,
        });
        units.push(UnitState {
            key: "app-gw".into(),
            status: NodeStatus::Working,
            tokens: 120_000,
            elapsed_secs: 8,
        });
        units.push(UnitState {
            key: "rtc".into(),
            status: NodeStatus::Waiting,
            tokens: 0,
            elapsed_secs: 0,
        });
        units.push(UnitState {
            key: "auth".into(),
            status: NodeStatus::Waiting,
            tokens: 0,
            elapsed_secs: 0,
        });
        let assess = StepState {
            id: "assess".into(),
            kind: StepKind::ForEach,
            agent: Some("for_each".into()),
            status: if active {
                NodeStatus::Working
            } else {
                NodeStatus::Complete
            },
            tokens_in: 890_000,
            tokens_out: 0,
            elapsed_secs: 0,
            units,
            fanout_total: Some(5),
            panel_iter: None,
            panel_findings: 0,
        };
        LiveRunState {
            workflow_name: "oracle-assessor-workflow".into(),
            run_id: "run_01ABC".into(),
            status: RunStatus::Running,
            started_at: Some(ts(0)),
            finished_at: None,
            steps: vec![
                StepState {
                    id: "understand".into(),
                    kind: StepKind::Linear,
                    agent: Some("oracle-recon".into()),
                    status: NodeStatus::Complete,
                    tokens_in: 102_000,
                    tokens_out: 0,
                    elapsed_secs: 18,
                    units: Vec::new(),
                    fanout_total: None,
                    panel_iter: None,
                    panel_findings: 0,
                },
                assess,
                StepState {
                    id: "sweep".into(),
                    kind: StepKind::Panel,
                    agent: None,
                    status: NodeStatus::Working,
                    tokens_in: 0,
                    tokens_out: 0,
                    elapsed_secs: 0,
                    units: Vec::new(),
                    fanout_total: None,
                    panel_iter: Some((2, 10)),
                    panel_findings: 2,
                },
                StepState {
                    id: "report".into(),
                    kind: StepKind::Linear,
                    agent: None,
                    status: NodeStatus::Waiting,
                    tokens_in: 0,
                    tokens_out: 0,
                    elapsed_secs: 0,
                    units: Vec::new(),
                    fanout_total: None,
                    panel_iter: None,
                    panel_findings: 0,
                },
            ],
            tokens_in: 1_200_000,
            tokens_out: 45_000,
            cost: Some(3.40),
            findings_count: Some(12),
            coverage_pct: Some(78),
            active: ActiveFocus {
                step_id: Some("assess".into()),
                unit_key: Some("app-gw".into()),
                agent: Some("oracle-assessor".into()),
                active_unit_transcript: None,
                feed: Vec::new(),
                last_event_at: Some(ts(31)),
            },
        }
    }

    fn empty_workflow() -> Workflow {
        // The graph renderer drives off LiveRunState.steps, not the
        // workflow itself, so an empty workflow is sufficient.
        serde_yaml::from_str("name: w\nsteps: []\n").unwrap()
    }

    fn stripped(rows: Vec<String>) -> Vec<String> {
        rows.iter().map(|r| strip_ansi(r)).collect()
    }

    #[test]
    fn spinner_cycles() {
        assert_eq!(spinner_frame(0), '⠋');
        assert_eq!(spinner_frame(1), '⠙');
        assert_eq!(spinner_frame(10), '⠋');
        assert_eq!(spinner_frame(19), '⠏');
    }

    #[test]
    fn dashboard_progress_bar_fills_completed_over_total() {
        let state = fanout_state(true);
        let rows = stripped(render_dashboard(&state, ts(31), 79));
        // 1 of 4 steps complete (understand). Bar is 24 wide → 6 filled.
        let prog = rows.iter().find(|r| r.contains("step ")).unwrap();
        assert!(prog.contains("step 1/4"), "got {prog:?}");
        assert_eq!(prog.matches('█').count(), 6, "got {prog:?}");
        assert_eq!(prog.matches('░').count(), 18, "got {prog:?}");
    }

    #[test]
    fn dashboard_meters_always_show_tokens_and_cost() {
        let state = fanout_state(true);
        let rows = stripped(render_dashboard(&state, ts(31), 79));
        let meters = rows.iter().find(|r| r.contains("⇡")).unwrap();
        assert!(meters.contains("⇡ 1.2M"), "got {meters:?}");
        assert!(meters.contains("⇣ 45K"), "got {meters:?}");
        assert!(meters.contains("$3.40"), "got {meters:?}");
    }

    #[test]
    fn dashboard_findings_and_coverage_shown_when_present() {
        let state = fanout_state(true);
        let rows = stripped(render_dashboard(&state, ts(31), 79));
        let meters = rows.iter().find(|r| r.contains("⇡")).unwrap();
        assert!(meters.contains("findings 12"), "got {meters:?}");
        assert!(meters.contains("78%"), "got {meters:?}");
    }

    #[test]
    fn dashboard_findings_and_coverage_hidden_when_absent() {
        let mut state = fanout_state(true);
        state.findings_count = None;
        state.coverage_pct = None;
        state.findings_count = Some(0); // zero also hides findings
        let rows = stripped(render_dashboard(&state, ts(31), 79));
        let meters = rows.iter().find(|r| r.contains("⇡")).unwrap();
        assert!(!meters.contains("findings"), "got {meters:?}");
        assert!(!meters.contains("coverage"), "got {meters:?}");
    }

    #[test]
    fn graph_active_fanout_expands_units() {
        let state = fanout_state(true);
        let rows = stripped(render_graph(&state, &empty_workflow(), 79));
        // Active assess expands all 5 units.
        assert!(rows.iter().any(|r| r.contains("conf-manager")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("tlb-agent")));
        assert!(rows.iter().any(|r| r.contains("app-gw")));
        assert!(rows.iter().any(|r| r.contains("rtc")));
        assert!(rows.iter().any(|r| r.contains("auth")));
        // Mid branches use ┣━, last uses ┗━.
        assert!(rows.iter().any(|r| r.starts_with("┣━")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.starts_with("┗━")), "{rows:#?}");
        // Completed units carry ✓; working unit reads "working".
        let app_gw = rows.iter().find(|r| r.contains("app-gw")).unwrap();
        assert!(app_gw.contains("working"), "{app_gw:?}");
        let queued = rows.iter().find(|r| r.contains("rtc")).unwrap();
        assert!(queued.contains("queued"), "{queued:?}");
    }

    #[test]
    fn graph_inactive_fanout_collapses_to_done_over_total() {
        let state = fanout_state(false);
        let rows = stripped(render_graph(&state, &empty_workflow(), 79));
        // assess is complete (not active) → collapsed, no unit rows.
        assert!(
            !rows.iter().any(|r| r.contains("conf-manager")),
            "{rows:#?}"
        );
        let assess = rows.iter().find(|r| r.contains("assess")).unwrap();
        // done_units counts Complete+Failed: conf-manager + tlb-agent = 2 of 5.
        assert!(assess.contains("2/5"), "{assess:?}");
    }

    #[test]
    fn graph_panel_shows_iteration() {
        let state = fanout_state(true);
        let rows = stripped(render_graph(&state, &empty_workflow(), 79));
        let panel = rows.iter().find(|r| r.contains("sweep")).unwrap();
        assert!(panel.contains("⟲ iter 2/10"), "{panel:?}");
        assert!(panel.contains("2 found"), "{panel:?}");
    }

    #[test]
    fn graph_failed_unit_shows_cross() {
        let mut state = fanout_state(true);
        state.steps[1].units[2].status = NodeStatus::Failed;
        let rows = stripped(render_graph(&state, &empty_workflow(), 79));
        let app_gw = rows.iter().find(|r| r.contains("app-gw")).unwrap();
        assert!(app_gw.contains('✗'), "{app_gw:?}");
    }

    #[test]
    fn graph_uses_heavy_spine() {
        let state = fanout_state(true);
        let rows = stripped(render_graph(&state, &empty_workflow(), 79));
        assert!(rows.iter().any(|r| r == "┃"), "{rows:#?}");
    }

    #[test]
    fn focus_heartbeat_formats_seconds_ago() {
        let mut state = fanout_state(true);
        state.active.last_event_at = Some(ts(31));
        let rows = stripped(render_focus(&state, ts(33), 79, 8));
        let header = &rows[0];
        assert!(header.contains("active 2s ago"), "{header:?}");
        assert!(header.contains("app-gw · oracle-assessor"), "{header:?}");
    }

    #[test]
    fn focus_feed_respects_height_cap() {
        let mut state = fanout_state(true);
        for i in 0..50u32 {
            state.push_activity(ts(i % 60), ActivityKind::ToolCall, format!("event {i}"));
        }
        // height 8 → 6 body rows.
        let rows = render_focus(&state, ts(59), 79, 8);
        assert_eq!(rows.len(), 8, "1 top + 6 body + 1 bottom");
        let stripped_rows = stripped(rows);
        // Most recent event (49) is shown; an old one (10) is not.
        assert!(stripped_rows.iter().any(|r| r.contains("event 49")));
        assert!(!stripped_rows.iter().any(|r| r.contains("event 10")));
    }

    #[test]
    fn focus_failed_run_shows_resume_hint() {
        let mut state = fanout_state(true);
        state.status = RunStatus::Failed;
        let rows = stripped(render_focus(&state, ts(33), 79, 8));
        assert!(
            rows.iter()
                .any(|r| r.contains("↳ rupu workflow resume run_01ABC")),
            "{rows:#?}"
        );
    }

    #[test]
    fn apply_step_started_activates_and_resets_feed() {
        let mut state = fanout_state(true);
        state.push_activity(ts(1), ActivityKind::Text, "stale");
        state.apply(&WfEvent::StepStarted {
            run_id: "run_01ABC".into(),
            step_id: "report".into(),
            kind: StepKind::Linear,
            agent: Some("reporter".into()),
        });
        assert_eq!(state.active.step_id.as_deref(), Some("report"));
        assert_eq!(state.active.agent.as_deref(), Some("reporter"));
        assert!(state.active.feed.is_empty());
        assert_eq!(state.status_for("report"), NodeStatus::Active);
    }

    #[test]
    fn apply_step_completed_sets_status_and_elapsed() {
        let mut state = fanout_state(true);
        state.apply(&WfEvent::StepCompleted {
            run_id: "run_01ABC".into(),
            step_id: "understand".into(),
            success: true,
            duration_ms: 18_000,
        });
        let step = state.steps.iter().find(|s| s.id == "understand").unwrap();
        assert_eq!(step.status, NodeStatus::Complete);
        assert_eq!(step.elapsed_secs, 18);
    }

    #[test]
    fn map_transcript_tool_call_surfaces_key_arg() {
        let ev = rupu_transcript::Event::ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            input: serde_json::json!({"path": "services/app-gw/handler.go"}),
        };
        let (kind, text) = map_transcript_event(&ev, ts(0)).unwrap();
        assert_eq!(kind, ActivityKind::ToolCall);
        assert!(text.contains("read_file"), "{text:?}");
        assert!(text.contains("services/app-gw/handler.go"), "{text:?}");
    }

    #[test]
    fn map_transcript_finding_action_surfaces_severity() {
        let ev = rupu_transcript::Event::ActionEmitted {
            kind: "report_finding".into(),
            payload: serde_json::json!({"severity": "high", "title": "path traversal"}),
            allowed: true,
            applied: true,
            reason: None,
        };
        let (kind, text) = map_transcript_event(&ev, ts(0)).unwrap();
        assert_eq!(kind, ActivityKind::Finding);
        assert!(text.starts_with("HIGH"), "{text:?}");
        assert!(text.contains("path traversal"), "{text:?}");
    }

    #[test]
    fn apply_unit_started_marks_unit_working_and_sets_active_transcript() {
        let mut state = fanout_state(true);
        // Pre-seed stale feed to prove the unit switch resets it.
        state.push_activity(ts(1), ActivityKind::Text, "stale");
        let path = std::path::PathBuf::from("/runs/run_unit2.jsonl");
        state.apply(&WfEvent::UnitStarted {
            run_id: "run_01ABC".into(),
            step_id: "assess".into(),
            index: 2,
            unit_key: "app-gw".into(),
            agent: Some("oracle-assessor".into()),
            transcript_path: path.clone(),
        });
        let step = state.steps.iter().find(|s| s.id == "assess").unwrap();
        assert_eq!(step.units[2].status, NodeStatus::Working);
        assert_eq!(step.units[2].key, "app-gw");
        assert_eq!(state.active.step_id.as_deref(), Some("assess"));
        assert_eq!(state.active.unit_key.as_deref(), Some("app-gw"));
        assert_eq!(state.active.active_unit_transcript.as_ref(), Some(&path));
        assert!(state.active.feed.is_empty(), "feed reset on unit switch");
    }

    #[test]
    fn apply_unit_completed_marks_done_and_adds_tokens() {
        let mut state = fanout_state(true);
        let step_tokens_before = state
            .steps
            .iter()
            .find(|s| s.id == "assess")
            .unwrap()
            .tokens_in;
        let run_in_before = state.tokens_in;
        let run_out_before = state.tokens_out;
        // Unit 3 starts Waiting in the fixture; complete it.
        state.apply(&WfEvent::UnitCompleted {
            run_id: "run_01ABC".into(),
            step_id: "assess".into(),
            index: 3,
            unit_key: "rtc".into(),
            success: true,
            tokens_in: 1000,
            tokens_out: 250,
        });
        let step = state.steps.iter().find(|s| s.id == "assess").unwrap();
        assert_eq!(step.units[3].status, NodeStatus::Complete);
        assert_eq!(step.units[3].tokens, 1250);
        assert_eq!(step.tokens_in, step_tokens_before + 1000);
        assert_eq!(step.tokens_out, 250);
        assert_eq!(state.tokens_in, run_in_before + 1000);
        assert_eq!(state.tokens_out, run_out_before + 250);
        // done_units now counts conf-manager + tlb-agent + rtc = 3.
        assert_eq!(step.done_units(), 3);
    }

    #[test]
    fn apply_unit_completed_failure_marks_failed() {
        let mut state = fanout_state(true);
        state.apply(&WfEvent::UnitCompleted {
            run_id: "run_01ABC".into(),
            step_id: "assess".into(),
            index: 2,
            unit_key: "app-gw".into(),
            success: false,
            tokens_in: 0,
            tokens_out: 0,
        });
        let step = state.steps.iter().find(|s| s.id == "assess").unwrap();
        assert_eq!(step.units[2].status, NodeStatus::Failed);
    }

    #[test]
    fn apply_unit_started_grows_units_for_fresh_fanout() {
        // A fresh fan-out step (no pre-seeded units) should grow its
        // unit vector to address the started index, filling gaps with
        // Waiting placeholders.
        let mut state = fanout_state(true);
        state.steps[1].units.clear();
        state.apply(&WfEvent::UnitStarted {
            run_id: "run_01ABC".into(),
            step_id: "assess".into(),
            index: 3,
            unit_key: "rtc".into(),
            agent: Some("oracle-assessor".into()),
            transcript_path: std::path::PathBuf::from("/runs/u3.jsonl"),
        });
        let step = state.steps.iter().find(|s| s.id == "assess").unwrap();
        assert_eq!(step.units.len(), 4);
        assert_eq!(step.units[3].status, NodeStatus::Working);
        assert_eq!(step.units[0].status, NodeStatus::Waiting);
    }

    #[test]
    fn map_transcript_delta_is_ignored() {
        let ev = rupu_transcript::Event::AssistantDelta {
            content: "thinking...".into(),
        };
        assert!(map_transcript_event(&ev, ts(0)).is_none());
    }

    #[test]
    fn render_view_stacks_zones_and_fills_height() {
        let state = fanout_state(true);
        let rows = render_view(&state, &empty_workflow(), ts(31), 79, 30);
        // The view should be at most term_height rows.
        assert!(rows.len() <= 30, "got {}", rows.len());
        let plain = stripped(rows);
        // Sanity: dashboard title + graph + focus border all present.
        assert!(plain.iter().any(|r| r.contains("oracle-assessor-workflow")));
        assert!(plain.iter().any(|r| r.contains("understand")));
        assert!(plain.iter().any(|r| r.starts_with("╭ ")));
        assert!(plain.iter().any(|r| r.starts_with("╰")));
    }
}
