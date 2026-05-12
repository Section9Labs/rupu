//! WorkspaceWindow — the GPUI view for one workspace's window.

pub mod sidebar;
pub mod titlebar;

use crate::executor::AppExecutor;
use crate::menu::app_menu::{ApproveFocused, RejectFocused};
use crate::palette;
use crate::view::transcript_tail::{TranscriptLine, TranscriptTail};
use crate::view::{ApproveCallback, RejectCallback};
use crate::window::sidebar::ActiveRunMap;
use crate::workspace::Workspace;
use gpui::{
    div, prelude::*, px, size, AnyElement, App, Bounds, Context, IntoElement, Render, WeakEntity,
    Window, WindowBounds, WindowHandle, WindowOptions,
};
use std::path::PathBuf;
use std::sync::Arc;

pub struct WorkspaceWindow {
    pub workspace: Workspace,
    /// Executor singleton — used to check for active runs and subscribe
    /// to their event streams. Populated at window-open time.
    pub app_executor: Arc<AppExecutor>,
    /// Live run state for the currently displayed workflow. `None`
    /// when no run is active (D-2 static display). Populated by
    /// `on_workflow_clicked` from the AppExecutor event stream.
    pub run_model: Option<crate::run_model::RunModel>,
    /// Buffered lines from the focused step's transcript JSONL.
    /// Reset when a new transcript tail is started.
    /// Simplification (D-3): the tail is spawned for the first focused
    /// step only and not re-spawned on focus changes. A follow-up task
    /// will add cancellation + re-spawn.
    pub transcript_lines: Vec<TranscriptLine>,
}

impl WorkspaceWindow {
    /// Open a new top-level window for the given workspace. The
    /// window owns the workspace handle for its lifetime.
    pub fn open(
        workspace: Workspace,
        app_executor: Arc<AppExecutor>,
        cx: &mut App,
    ) -> WindowHandle<Self> {
        let bounds = Bounds::centered(None, size(px(1240.0), px(800.0)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None, // we draw our own titlebar inside the view
            ..Default::default()
        };
        cx.open_window(opts, |_window, cx| {
            cx.new(|_cx| WorkspaceWindow {
                workspace,
                app_executor,
                run_model: None,
                transcript_lines: Vec::new(),
            })
        })
        .expect("open workspace window")
    }

    /// Called when the user selects a workflow in the sidebar. Checks
    /// for an active run; if one exists, subscribes to its event stream
    /// and updates `run_model` as events arrive.
    pub fn on_workflow_clicked(&mut self, workflow_path: PathBuf, cx: &mut Context<Self>) {
        let active = self.app_executor.list_active_runs(Some(workflow_path.clone()));
        if let Some(run) = active.into_iter().next() {
            self.run_model = Some(crate::run_model::RunModel::new(
                run.id.clone(),
                workflow_path,
            ));
            let app_executor = self.app_executor.clone();
            let run_id = run.id.clone();
            let run_store = self.app_executor.run_store().clone();
            cx.spawn(async move |this, cx| {
                match app_executor.attach(&run_id).await {
                    Ok(mut stream) => {
                        use futures_util::StreamExt;
                        while let Some(ev) = stream.next().await {
                            // Apply the event and capture whether focused_step
                            // was just set for the first time.
                            let maybe_transcript_path = this.update(cx, |this, cx| {
                                let prev_focused = this
                                    .run_model
                                    .as_ref()
                                    .and_then(|m| m.focused_step.clone());
                                if let Some(m) = this.run_model.take() {
                                    this.run_model = Some(m.apply(&ev));
                                }
                                let new_focused = this
                                    .run_model
                                    .as_ref()
                                    .and_then(|m| m.focused_step.clone());
                                cx.notify();
                                // Return the transcript path only if focus was
                                // just set for the first time (prev None, now Some).
                                // D-3 simplification: no re-spawn on subsequent
                                // focus changes; that polish is deferred.
                                if prev_focused.is_none() {
                                    if let Some(step_id) = &new_focused {
                                        // Use active_step_transcript_path from the
                                        // RunRecord when present; otherwise derive
                                        // from convention: <runs_root>/<run_id>/
                                        // transcripts/<step_id>.jsonl.
                                        let authoritative = run_store
                                            .load(&run_id)
                                            .ok()
                                            .and_then(|r| r.active_step_transcript_path);
                                        let path = authoritative.unwrap_or_else(|| {
                                            run_store
                                                .run_json_path(&run_id)
                                                .parent()
                                                .expect("run dir exists")
                                                .join("transcripts")
                                                .join(format!("{step_id}.jsonl"))
                                        });
                                        return Some(path);
                                    }
                                }
                                None
                            });

                            match maybe_transcript_path {
                                Err(_) => break, // window closed
                                Ok(Some(path)) => {
                                    // Spawn the transcript tail for this step.
                                    // Clone the weak handle and the AsyncApp so the
                                    // inner future can push lines to the UI.
                                    let this2 = this.clone();
                                    let mut cx2 = cx.clone();
                                    cx.spawn(async move |_cx| {
                                        match TranscriptTail::open(&path).await {
                                            Ok(mut tail) => {
                                                use futures_util::StreamExt;
                                                while let Some(line) = tail.next().await {
                                                    let res = this2.update(&mut cx2, |this, cx| {
                                                        this.transcript_lines.push(line);
                                                        cx.notify();
                                                    });
                                                    if res.is_err() {
                                                        break;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!(%e, path = ?path, "transcript tail open failed");
                                            }
                                        }
                                    })
                                    .detach();
                                }
                                Ok(None) => {} // no new focus
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(run_id = %run_id, %e, "failed to attach to run stream");
                    }
                }
            })
            .detach();
        } else {
            self.run_model = None;
        }
        cx.notify();
    }
}

impl WorkspaceWindow {
    /// Approve the awaiting step in the currently active run.
    /// `step_id` is passed for forward-compatibility (the executor currently
    /// operates at run granularity — the awaiting step is unambiguous).
    pub fn handle_approve(&mut self, step_id: String, cx: &mut Context<Self>) {
        let Some(model) = &self.run_model else { return };
        // Guard: only fire when the step is actually Awaiting.
        if model
            .nodes
            .get(&step_id)
            .copied()
            .unwrap_or(rupu_app_canvas::NodeStatus::Waiting)
            != rupu_app_canvas::NodeStatus::Awaiting
        {
            return;
        }
        let run_id = model.run_id.clone();
        let app_exec = self.app_executor.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = app_exec.approve(&run_id, "rupu.app").await {
                tracing::error!(error = %e, run_id = %run_id, "approve failed");
            }
        })
        .detach();
    }

    /// Reject the awaiting step in the currently active run.
    pub fn handle_reject(&mut self, step_id: String, reason: String, cx: &mut Context<Self>) {
        let Some(model) = &self.run_model else { return };
        // Guard: only fire when the step is actually Awaiting.
        if model
            .nodes
            .get(&step_id)
            .copied()
            .unwrap_or(rupu_app_canvas::NodeStatus::Waiting)
            != rupu_app_canvas::NodeStatus::Awaiting
        {
            return;
        }
        let run_id = model.run_id.clone();
        let app_exec = self.app_executor.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = app_exec.reject(&run_id, &reason).await {
                tracing::error!(error = %e, run_id = %run_id, "reject failed");
            }
        })
        .detach();
    }

    /// Returns the path of the workflow currently displayed in the main area,
    /// i.e. the first workflow in the project asset list. D-3 simplification:
    /// the app always shows `workflows.first()`; a later task will add
    /// per-row click selection.
    pub fn current_workflow_path(&self) -> Option<PathBuf> {
        self.workspace
            .project_assets
            .workflows
            .first()
            .map(|a| a.path.clone())
    }

    /// Called when the user clicks the Run button in the toolbar.
    /// Starts a new workflow run and subscribes to its event stream.
    pub fn handle_run_clicked(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.current_workflow_path() else { return };
        let app_exec = self.app_executor.clone();
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            match app_exec.start_workflow(path.clone()).await {
                Ok(run_id) => {
                    let _ = this.update(cx, |this, cx| {
                        this.run_model = Some(crate::run_model::RunModel::new(
                            run_id.clone(),
                            path.clone(),
                        ));
                        cx.notify();
                    });
                    // Subscribe to the new run's event stream and apply events.
                    if let Ok(mut stream) = app_exec.attach(&run_id).await {
                        use futures_util::StreamExt;
                        while let Some(ev) = stream.next().await {
                            let res = this.update(cx, |this, cx| {
                                if let Some(m) = this.run_model.take() {
                                    this.run_model = Some(m.apply(&ev));
                                }
                                cx.notify();
                            });
                            if res.is_err() {
                                break; // window closed
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "start_workflow failed");
                }
            }
        })
        .detach();
    }
}

impl Render for WorkspaceWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let run_model = self.run_model.clone();
        let transcript_lines = self.transcript_lines.clone();

        // Build approve / reject callbacks via WeakEntity so the closures
        // can update `self` when invoked from click handlers. WeakEntity
        // is Send + 'static, making it suitable for Arc<dyn Fn(...)> callbacks.
        let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let weak2 = weak.clone();
        let on_approve: ApproveCallback =
            Arc::new(move |step_id: String, window: &mut Window, cx: &mut App| {
                let _ = window;
                weak.update(cx, |this, cx| {
                    this.handle_approve(step_id, cx);
                })
                .ok();
            });
        let on_reject: RejectCallback =
            Arc::new(move |step_id: String, reason: String, window: &mut Window, cx: &mut App| {
                let _ = window;
                weak2
                    .update(cx, |this, cx| {
                        this.handle_reject(step_id, reason, cx);
                    })
                    .ok();
            });

        // Build active-run map for the sidebar status dots. We query the
        // executor once per frame for all workflows in the project asset list.
        let active_run_map: ActiveRunMap = self
            .workspace
            .project_assets
            .workflows
            .iter()
            .chain(self.workspace.global_assets.workflows.iter())
            .filter_map(|asset| {
                let runs = self.app_executor.list_active_runs(Some(asset.path.clone()));
                runs.into_iter().next().map(|r| (asset.path.clone(), r.status))
            })
            .collect();

        let main_area = match self.workspace.project_assets.workflows.first() {
            Some(asset) => {
                let wf_path = asset.path.clone();
                let has_active = active_run_map.contains_key(&wf_path);
                let weak_run = cx.weak_entity();
                render_main_for_workflow(
                    asset,
                    run_model.as_ref(),
                    &transcript_lines,
                    on_approve.clone(),
                    on_reject.clone(),
                    has_active,
                    weak_run,
                )
            }
            None => render_main_placeholder(),
        };

        // Keyboard approval shortcuts — `a` to approve, `r` to reject.
        // Only fire when `focused_step` is Some and Awaiting; the guard is
        // inside handle_approve / handle_reject.
        let focused_for_approve = self
            .run_model
            .as_ref()
            .and_then(|m| m.focused_step.clone());
        let focused_for_reject = focused_for_approve.clone();
        let on_approve_kb = cx.listener(move |this, _: &ApproveFocused, _window, cx| {
            if let Some(step) = &focused_for_approve {
                this.handle_approve(step.clone(), cx);
            }
        });
        let on_reject_kb = cx.listener(move |this, _: &RejectFocused, _window, cx| {
            if let Some(step) = &focused_for_reject {
                this.handle_reject(step.clone(), "rejected via keyboard".into(), cx);
            }
        });

        div()
            .size_full()
            .bg(palette::BG_PRIMARY)
            .text_color(palette::TEXT_PRIMARY)
            .flex()
            .flex_col()
            .on_action(on_approve_kb)
            .on_action(on_reject_kb)
            .child(titlebar::render(&self.workspace))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(sidebar::render(&self.workspace, &active_run_map))
                    .child(main_area),
            )
    }
}

fn render_main_placeholder() -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::TEXT_DIMMEST)
        .child("Open a workflow from the sidebar.")
        .into_any_element()
}

fn render_main_for_workflow(
    asset: &crate::workspace::Asset,
    run_model: Option<&crate::run_model::RunModel>,
    transcript_lines: &[TranscriptLine],
    on_approve: ApproveCallback,
    on_reject: RejectCallback,
    has_active_run: bool,
    weak: WeakEntity<WorkspaceWindow>,
) -> AnyElement {
    use rupu_orchestrator::Workflow;

    let body = match std::fs::read_to_string(&asset.path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(path = ?asset.path, %e, "read workflow");
            return render_main_error(format!("failed to read {}: {e}", asset.path.display()));
        }
    };
    let wf = match Workflow::parse(&body) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(path = ?asset.path, %e, "parse workflow");
            return render_main_error(format!("failed to parse {}: {e}", asset.path.display()));
        }
    };

    let default_model = crate::run_model::RunModel::new(String::new(), asset.path.clone());
    let model = run_model.unwrap_or(&default_model);

    // Toolbar: workflow name on the left, Run button on the right.
    // The Run button is greyed and non-interactive when a run is already active.
    let run_btn_bg = if has_active_run {
        palette::BG_SIDEBAR
    } else {
        palette::RUNNING
    };
    let mut run_btn = div()
        .id("run-workflow")
        .px(px(12.0))
        .py(px(6.0))
        .bg(run_btn_bg)
        .text_color(palette::TEXT_PRIMARY)
        .child("Run");
    if !has_active_run {
        run_btn = run_btn.cursor_pointer().on_click(move |_ev, _window, cx| {
            weak.update(cx, |this, cx| {
                this.handle_run_clicked(cx);
            })
            .ok();
        });
    }

    let toolbar = div()
        .flex()
        .flex_row()
        .items_center()
        .px(px(24.0))
        .py(px(8.0))
        .bg(palette::BG_PRIMARY)
        .border_b_1()
        .border_color(palette::BORDER)
        .child(
            div()
                .flex_1()
                .text_color(palette::TEXT_PRIMARY)
                .child(wf.name.clone()),
        )
        .child(run_btn);

    div()
        .flex_1()
        .flex()
        .flex_col()
        .child(toolbar)
        .child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .child(crate::view::graph::render(&wf, model, on_approve.clone()))
                .child(crate::view::drilldown::render(
                    model,
                    transcript_lines,
                    on_approve,
                    on_reject,
                )),
        )
        .into_any_element()
}

fn render_main_error(msg: String) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::FAILED)
        .child(msg)
        .into_any_element()
}
