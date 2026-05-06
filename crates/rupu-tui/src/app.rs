use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{self as cev, DisableMouseCapture, EnableMouseCapture, Event as CtEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};
use tracing::warn;

use rupu_orchestrator::{RunStore, Workflow};

use crate::control::{dispatch, Action};
use crate::source::{EventSource, SourceEvent};
use crate::state::{derive_edges, RunModel};
use crate::view::{canvas::render_canvas, panel::render_panel, tree::render_tree};
use crate::TuiResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Canvas,
    Tree,
}

impl Default for View {
    fn default() -> Self {
        match std::env::var("RUPU_TUI_DEFAULT_VIEW").as_deref() {
            Ok("tree") => View::Tree,
            _ => View::Canvas,
        }
    }
}

pub struct App {
    model: RunModel,
    edges: Vec<(String, String)>,
    view: View,
    focus: String,
    source: Box<dyn EventSource>,
    store: RunStore,
    run_id: String,
}

impl App {
    pub fn new(
        run_id: String,
        source: Box<dyn EventSource>,
        store: RunStore,
        workflow: Option<Workflow>,
    ) -> Self {
        let mut model = RunModel::new();
        let mut edges = Vec::new();
        if let Some(wf) = &workflow {
            for step in &wf.steps {
                model.upsert_node(&step.id, &step.agent.clone().unwrap_or_default());
            }
            edges = derive_edges(wf);
        }
        let focus = model.nodes.keys().next().cloned().unwrap_or_default();
        Self { model, edges, view: View::default(), focus, source, store, run_id }
    }

    pub fn run(mut self) -> TuiResult<()> {
        let mut terminal = setup_terminal()?;
        let result = self.run_loop(&mut terminal);
        teardown_terminal(&mut terminal)?;
        result
    }

    fn run_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> TuiResult<()> {
        loop {
            for ev in self.source.poll() {
                self.apply(ev);
            }
            if cev::poll(Duration::from_millis(33))? {
                if let CtEvent::Key(k) = cev::read()? {
                    match dispatch(k) {
                        Action::Quit => return Ok(()),
                        Action::FocusNext => self.cycle_focus(1),
                        Action::FocusPrev => self.cycle_focus(-1),
                        Action::ToggleView => {
                            self.view = match self.view {
                                View::Canvas => View::Tree,
                                View::Tree => View::Canvas,
                            };
                        }
                        Action::ApproveFocused => self.approve_focused_now(),
                        Action::RejectFocused => self.reject_focused_now(""),
                        _ => {}
                    }
                }
            }
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(40), Constraint::Length(40)])
                    .split(f.area());
                match self.view {
                    View::Canvas => render_canvas(f, chunks[0], &self.model, &self.edges, &self.focus),
                    View::Tree => render_tree(f, chunks[0], &self.model, &self.edges, &self.focus),
                }
                render_panel(f, chunks[1], &self.model, &self.focus);
            })?;
        }
    }

    fn apply(&mut self, ev: SourceEvent) {
        match ev {
            SourceEvent::StepEvent { step_id, event } => self.model.apply_event(&step_id, &event),
            SourceEvent::RunUpdate(rec) => self.model.apply_run_update(rec),
            SourceEvent::Tick => {}
        }
    }

    fn cycle_focus(&mut self, dir: i32) {
        let ids: Vec<String> = self.model.nodes.keys().cloned().collect();
        if ids.is_empty() {
            return;
        }
        let cur = ids.iter().position(|id| id == &self.focus).unwrap_or(0) as i32;
        let next = (cur + dir).rem_euclid(ids.len() as i32) as usize;
        self.focus = ids[next].clone();
    }

    fn approve_focused_now(&mut self) {
        let approver = whoami::username();
        match crate::control::approval::approve_focused(&self.store, &self.run_id, &approver) {
            Ok(_) => {
                if let Some(node) = self.model.nodes.get_mut(&self.focus) {
                    node.status = crate::state::NodeStatus::Working;
                }
            }
            Err(e) => warn!(error = %e, "approve failed"),
        }
    }

    fn reject_focused_now(&mut self, reason: &str) {
        let approver = whoami::username();
        match crate::control::approval::reject_focused(&self.store, &self.run_id, &approver, reason) {
            Ok(_) => {
                if let Some(node) = self.model.nodes.get_mut(&self.focus) {
                    node.status = crate::state::NodeStatus::Failed;
                }
            }
            Err(e) => warn!(error = %e, "reject failed"),
        }
    }
}

fn setup_terminal() -> TuiResult<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let term = Terminal::new(CrosstermBackend::new(out))?;
    install_panic_hook();
    Ok(term)
}

fn teardown_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> TuiResult<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original(info);
    }));
}
