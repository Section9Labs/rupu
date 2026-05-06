use std::io::{self, Stdout};
use std::time::{Duration, Instant};

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

use rupu_orchestrator::{RunStore, Workflow};

use crate::control::{dispatch, Action};
use crate::source::{EventSource, SourceEvent};
use crate::state::{derive_edges, RunModel};
use crate::view::{
    canvas::render_canvas_with_warning, panel::render_panel, toast::render_toast,
    toast::Toast, tree::render_tree,
};
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
    toast: Option<Toast>,
    /// Last instant the user pressed a focus-changing key. Used to
    /// debounce auto-focus-on-awaiting (5s window).
    last_user_focus: Option<Instant>,
    /// When Some(buffer), App is in reject-reason input mode and
    /// keystrokes append to the buffer instead of dispatching.
    reject_buffer: Option<String>,
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
        Self {
            model,
            edges,
            view: View::default(),
            focus,
            source,
            store,
            run_id,
            toast: None,
            last_user_focus: None,
            reject_buffer: None,
        }
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
                    if let Some(buf) = self.reject_buffer.as_mut() {
                        match k.code {
                            cev::KeyCode::Esc => { self.reject_buffer = None; }
                            cev::KeyCode::Enter => self.finish_reject(),
                            cev::KeyCode::Backspace => { buf.pop(); }
                            cev::KeyCode::Char(c) if buf.len() < 200 => buf.push(c),
                            _ => {}
                        }
                    } else {
                        match dispatch(k) {
                            Action::Quit => return Ok(()),
                            Action::FocusNext => {
                                self.cycle_focus(1);
                                self.last_user_focus = Some(Instant::now());
                            }
                            Action::FocusPrev => {
                                self.cycle_focus(-1);
                                self.last_user_focus = Some(Instant::now());
                            }
                            Action::ToggleView => {
                                self.view = match self.view {
                                    View::Canvas => View::Tree,
                                    View::Tree => View::Canvas,
                                };
                            }
                            Action::ApproveFocused => self.approve_focused_now(),
                            Action::RejectFocused => self.begin_reject(),
                            _ => {}
                        }
                    }
                }
            }
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(
                            if self.toast.is_some() || self.reject_buffer.is_some() { 2 } else { 0 },
                        ),
                    ])
                    .split(f.area());
                let main = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(40), Constraint::Length(40)])
                    .split(chunks[0]);
                match self.view {
                    View::Canvas => render_canvas_with_warning(
                        f, main[0], &self.model, &self.edges, &self.focus,
                    ),
                    View::Tree => render_tree(f, main[0], &self.model, &self.edges, &self.focus),
                }
                render_panel(f, main[1], &self.model, &self.focus);
                if let Some(buf) = &self.reject_buffer {
                    let para = ratatui::widgets::Paragraph::new(format!("reject reason: {buf}_"))
                        .style(ratatui::style::Style::default().fg(ratatui::style::Color::Red));
                    f.render_widget(para, chunks[1]);
                } else if let Some(toast) = &self.toast {
                    render_toast(f, chunks[1], toast);
                }
            })?;

            // Expire non-gate toasts.
            if let Some(t) = &self.toast {
                if t.expired(Instant::now()) {
                    self.toast = None;
                }
            }
        }
    }

    fn apply(&mut self, ev: SourceEvent) {
        match ev {
            SourceEvent::StepEvent { step_id, event } => {
                self.model.apply_event(&step_id, &event);
                if let Some(node) = self.model.node(&step_id) {
                    if node.status == crate::state::NodeStatus::Awaiting {
                        let allow_steal = self
                            .last_user_focus
                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(5));
                        if allow_steal {
                            self.focus = step_id.clone();
                        }
                        let prompt = node.gate_prompt.clone().unwrap_or_default();
                        self.toast = Some(Toast::gate(format!(
                            "\u{23f8} {step_id}: {prompt}  [a] approve  [r] reject  [enter] expand"
                        )));
                    }
                }
            }
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

    fn focused_is_awaiting(&self) -> bool {
        self.model
            .node(&self.focus)
            .map(|n| n.status == crate::state::NodeStatus::Awaiting)
            .unwrap_or(false)
    }

    fn approve_focused_now(&mut self) {
        if !self.focused_is_awaiting() {
            self.toast = Some(Toast::warn("not awaiting approval \u{2014} focus a \u{23f8} node"));
            return;
        }
        let approver = whoami::username();
        match crate::control::approval::approve_focused(&self.store, &self.run_id, &approver) {
            Ok(_) => {
                if let Some(node) = self.model.nodes.get_mut(&self.focus) {
                    node.status = crate::state::NodeStatus::Working;
                }
                self.toast = Some(Toast::ok("\u{2713} approved"));
            }
            Err(e) => self.toast = Some(Toast::err(format!("approve failed: {e}"))),
        }
    }

    fn begin_reject(&mut self) {
        if !self.focused_is_awaiting() {
            self.toast = Some(Toast::warn("not awaiting approval \u{2014} focus a \u{23f8} node"));
            return;
        }
        self.reject_buffer = Some(String::new());
    }

    fn finish_reject(&mut self) {
        let Some(reason) = self.reject_buffer.take() else { return };
        let approver = whoami::username();
        match crate::control::approval::reject_focused(
            &self.store,
            &self.run_id,
            &approver,
            &reason,
        ) {
            Ok(_) => {
                if let Some(node) = self.model.nodes.get_mut(&self.focus) {
                    node.status = crate::state::NodeStatus::Failed;
                }
                self.toast = Some(Toast::ok("\u{2717} rejected"));
            }
            Err(e) => self.toast = Some(Toast::err(format!("reject failed: {e}"))),
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
