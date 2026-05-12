//! WorkspaceWindow — the GPUI view for one workspace's window.

pub mod sidebar;
pub mod titlebar;

use crate::executor::AppExecutor;
use crate::palette;
use crate::view::transcript_tail::{TranscriptLine, TranscriptTail};
use crate::workspace::Workspace;
use gpui::{
    div, prelude::*, px, size, AnyElement, App, Bounds, Context, IntoElement, Render, Window,
    WindowBounds, WindowHandle, WindowOptions,
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

impl Render for WorkspaceWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let run_model = self.run_model.clone();
        let transcript_lines = self.transcript_lines.clone();
        let main_area = match self.workspace.project_assets.workflows.first() {
            Some(asset) => render_main_for_workflow(asset, run_model.as_ref(), &transcript_lines),
            None => render_main_placeholder(),
        };

        div()
            .size_full()
            .bg(palette::BG_PRIMARY)
            .text_color(palette::TEXT_PRIMARY)
            .flex()
            .flex_col()
            .child(titlebar::render(&self.workspace))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(sidebar::render(&self.workspace))
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

    div()
        .flex_1()
        .flex()
        .flex_row()
        .child(crate::view::graph::render(&wf, model))
        .child(crate::view::drilldown::render(model, transcript_lines))
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
