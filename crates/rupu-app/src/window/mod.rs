//! WorkspaceWindow — the GPUI view for one workspace's window.

pub mod sidebar;
pub mod titlebar;

use crate::executor::AppExecutor;
use crate::menu::app_menu::{ApproveFocused, DismissContextMenu, LaunchSelected, RejectFocused, ToggleSidebar};
use crate::palette;
use crate::view::transcript_tail::{TranscriptLine, TranscriptTail};
use crate::view::{ApproveCallback, RejectCallback};
use crate::window::sidebar::{ActiveRunMap, SectionToggleCb, WorkflowClickCb};
use crate::workspace::Workspace;
use gpui::{
    div, point, prelude::*, px, size, AnyElement, App, Bounds, Context, IntoElement, Render,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions,
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
    /// `Some` when the launcher sheet is open. None otherwise.
    pub launcher: Option<crate::launcher::LauncherState>,
    /// The workflow row most recently focused in the sidebar.
    /// ⌘R uses this. `None` means no row has focus.
    pub focused_workflow: Option<PathBuf>,
    /// `Some` while a right-click context menu is open. Cleared by item
    /// selection, click-outside, or Esc.
    pub context_menu: Option<crate::widget::ContextMenuState>,
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
            // appears_transparent = true keeps the custom titlebar we draw in
            // `titlebar::render` (color chip + name + in-flight badge) while
            // restoring the native macOS traffic-light controls. The 80px
            // left-padding on the custom titlebar leaves room for the lights.
            // Position mirrors Zed (crates/zed/src/zed.rs:352).
            titlebar: Some(TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(9.0), px(9.0))),
            }),
            ..Default::default()
        };
        let handle = cx
            .open_window(opts, |_window, cx| {
                cx.new(|_cx| WorkspaceWindow {
                    workspace,
                    app_executor,
                    run_model: None,
                    transcript_lines: Vec::new(),
                    launcher: None,
                    focused_workflow: None,
                    context_menu: None,
                })
            })
            .expect("open workspace window");

        // Register keyboard-action handlers globally once at window-open
        // time via WeakEntity. Registering them per-frame in Render::render
        // (via cx.listener + .on_action) causes a RefCell borrow conflict
        // because the entity is already borrowed during render.
        if let Ok(entity) = handle.entity(cx) {
            let weak_a = entity.downgrade();
            cx.on_action(move |_: &ApproveFocused, cx| {
                let _ = weak_a.update(cx, |this, cx| {
                    if let Some(step) = this.run_model.as_ref().and_then(|m| m.focused_step.clone())
                    {
                        this.handle_approve(step, cx);
                    }
                });
            });
            let weak_r = entity.downgrade();
            cx.on_action(move |_: &RejectFocused, cx| {
                let _ = weak_r.update(cx, |this, cx| {
                    if let Some(step) = this.run_model.as_ref().and_then(|m| m.focused_step.clone())
                    {
                        this.handle_reject(step, "rejected via keyboard".into(), cx);
                    }
                });
            });
            let weak_t = entity.downgrade();
            cx.on_action(move |_: &ToggleSidebar, cx| {
                let _ = weak_t.update(cx, |this, cx| {
                    // Toggle every section as a proxy for sidebar hide/show.
                    // A future task may add a dedicated `sidebar_hidden` flag.
                    for section in ["workflows", "runs", "repos", "agents", "issues"] {
                        this.handle_section_toggle(section, cx);
                    }
                });
            });
            let weak_l = entity.downgrade();
            cx.on_action(move |_: &LaunchSelected, cx| {
                let _ = weak_l.update(cx, |this, cx| {
                    if let Some(path) = this.focused_workflow.clone() {
                        this.open_launcher(path, cx);
                    }
                });
            });
            let weak_dctx = entity.downgrade();
            cx.on_action(move |_: &DismissContextMenu, cx| {
                let _ = weak_dctx.update(cx, |this, cx| {
                    this.handle_dismiss_context_menu(cx);
                });
            });
        }

        handle
    }

    /// Called when the user selects a workflow in the sidebar. Checks
    /// for an active run; if one exists, subscribes to its event stream
    /// and updates `run_model` as events arrive.
    pub fn on_workflow_clicked(&mut self, workflow_path: PathBuf, cx: &mut Context<Self>) {
        let active = self
            .app_executor
            .list_active_runs(Some(workflow_path.clone()));
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

    /// Returns the path of the workflow currently displayed in the main area.
    /// Prefers `focused_workflow` (last sidebar click); falls back to the
    /// first project workflow when no row has been clicked yet (workspace
    /// just opened) or the focused workflow has been removed from disk.
    pub fn current_workflow_path(&self) -> Option<PathBuf> {
        if let Some(p) = &self.focused_workflow {
            // Confirm it still exists in the asset list — if a workflow file
            // was deleted mid-session we don't want to keep pointing at it.
            let known = self
                .workspace
                .project_assets
                .workflows
                .iter()
                .chain(self.workspace.global_assets.workflows.iter())
                .any(|a| &a.path == p);
            if known {
                return Some(p.clone());
            }
        }
        self.workspace
            .project_assets
            .workflows
            .first()
            .map(|a| a.path.clone())
    }

    /// Sets `focused_workflow` to `path` and notifies the view.
    /// Called when the user left-clicks a workflow row in the sidebar.
    pub fn handle_workflow_clicked(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.focused_workflow = Some(path);
        cx.notify();
    }

    /// Sets `focused_workflow` to `path` and immediately opens the launcher.
    /// Called when the user right-clicks a workflow row in the sidebar.
    pub fn handle_workflow_right_clicked(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.focused_workflow = Some(path.clone());
        self.open_launcher(path, cx);
    }

    /// Open a context menu at `position` with workflow-row actions.
    pub fn open_workflow_context_menu(
        &mut self,
        path: PathBuf,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let weak_run: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let path_for_run = path.clone();
        let run_item = crate::widget::ContextMenuItem {
            label: "Run\u{2026}".into(),
            on_select: Arc::new(move |_w, cx| {
                let weak = weak_run.clone();
                let path = path_for_run.clone();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.handle_workflow_right_clicked(path, cx);
                    });
                });
            }),
        };
        let weak_reveal: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let path_for_reveal = path.clone();
        let reveal_item = crate::widget::ContextMenuItem {
            label: "Reveal in Finder".into(),
            on_select: Arc::new(move |_w, cx| {
                let weak = weak_reveal.clone();
                let path = path_for_reveal.clone();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.handle_workflow_reveal(path, cx);
                    });
                });
            }),
        };
        self.context_menu = Some(crate::widget::ContextMenuState {
            position,
            items: vec![run_item, reveal_item],
        });
        cx.notify();
    }

    /// Open a context menu at `position` with agent-row actions.
    pub fn open_agent_context_menu(
        &mut self,
        path: PathBuf,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let weak_open: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let path_for_open = path.clone();
        let open_item = crate::widget::ContextMenuItem {
            label: "Open in Editor".into(),
            on_select: Arc::new(move |_w, cx| {
                let weak = weak_open.clone();
                let path = path_for_open.clone();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.handle_agent_clicked(path, cx);
                    });
                });
            }),
        };
        let weak_reveal: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let path_for_reveal = path.clone();
        let reveal_item = crate::widget::ContextMenuItem {
            label: "Reveal in Finder".into(),
            on_select: Arc::new(move |_w, cx| {
                let weak = weak_reveal.clone();
                let path = path_for_reveal.clone();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.handle_agent_reveal(path, cx);
                    });
                });
            }),
        };
        self.context_menu = Some(crate::widget::ContextMenuState {
            position,
            items: vec![open_item, reveal_item],
        });
        cx.notify();
    }

    /// Toggle a sidebar section's collapsed state and persist.
    pub fn handle_section_toggle(&mut self, section: &'static str, cx: &mut Context<Self>) {
        self.workspace
            .manifest
            .ui
            .toggle_section_collapsed(section);
        if let Err(e) = crate::workspace::storage::save(&self.workspace.manifest) {
            tracing::warn!(%e, "persist sidebar collapse state");
        }
        cx.notify();
    }

    /// Close the context menu. Idempotent — safe to call when no menu is open.
    pub fn handle_dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.is_some() {
            self.context_menu = None;
            cx.notify();
        }
    }

    /// Open an agent's `.md` source file in the user's default app.
    pub fn handle_agent_clicked(&mut self, path: PathBuf, _cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = std::process::Command::new("open").arg(&path).spawn() {
                tracing::warn!(?path, %e, "open agent file");
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            tracing::info!(?path, "agent click: no-op on non-macOS");
        }
    }

    /// Reveal the agent file in Finder. macOS: `open -R <path>`.
    pub fn handle_agent_reveal(&mut self, path: PathBuf, _cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = std::process::Command::new("open")
                .args(["-R"])
                .arg(&path)
                .spawn()
            {
                tracing::warn!(?path, %e, "reveal agent in Finder");
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            tracing::info!(?path, "reveal: no-op on non-macOS");
        }
    }

    /// Reveal a workflow's YAML file in Finder.
    pub fn handle_workflow_reveal(&mut self, path: PathBuf, _cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = std::process::Command::new("open")
                .args(["-R"])
                .arg(&path)
                .spawn()
            {
                tracing::warn!(?path, %e, "reveal workflow in Finder");
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            tracing::info!(?path, "reveal: no-op on non-macOS");
        }
    }

    /// Called when the user clicks the Run button in the toolbar.
    /// Opens the launcher modal with the current workflow.
    pub fn handle_run_clicked(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.current_workflow_path() else {
            return;
        };
        self.open_launcher(path, cx);
    }

    pub fn open_launcher(&mut self, workflow_path: PathBuf, cx: &mut Context<Self>) {
        let yaml = match std::fs::read_to_string(&workflow_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, path = %workflow_path.display(), "open_launcher: read_to_string failed");
                return;
            }
        };
        let workflow = match rupu_orchestrator::Workflow::parse(&yaml) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error = %e, path = %workflow_path.display(), "open_launcher: parse failed");
                return;
            }
        };
        let mut state = crate::launcher::LauncherState::new(workflow_path, workflow);

        // Construct one TextInput entity per string/int input and seed it with
        // the current value (which may be the workflow's default).
        for (name, def) in &state.workflow.inputs {
            let is_text =
                matches!(def.ty, rupu_orchestrator::InputType::String) && def.allowed.is_empty();
            let is_int = matches!(def.ty, rupu_orchestrator::InputType::Int);
            if !is_text && !is_int {
                continue;
            }
            let initial = state.inputs.get(name).cloned().unwrap_or_default();
            let placeholder = format!("{name}…");
            let name_owned = name.clone();
            let entity = cx.new(|cx| {
                let mut t = crate::widget::TextInput::new(cx, placeholder);
                if !initial.is_empty() {
                    t.set_content(initial, cx);
                }
                t
            });
            // Subscribe to ContentChanged → mirror into LauncherState.inputs.
            let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
            cx.subscribe(&entity, move |_, _entity, ev: &crate::widget::ContentChanged, cx| {
                let name = name_owned.clone();
                let new_value = ev.content.to_string();
                let weak = weak.clone();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if let Some(s) = this.launcher.as_mut() {
                            s.set_input(&name, new_value);
                            s.revalidate();
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
            state.text_inputs.insert(name.clone(), entity);
        }

        // Reserved entity for Clone target's repo_ref. Always constructed even when
        // the user hasn't selected the Clone target yet, so switching to Clone
        // doesn't have to wire it lazily.
        let weak_clone: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let repo_ref_entity = cx.new(|cx| crate::widget::TextInput::new(cx, "owner/repo"));
        cx.subscribe(
            &repo_ref_entity,
            move |_, _entity, ev: &crate::widget::ContentChanged, cx| {
                let new_value = ev.content.to_string();
                let weak = weak_clone.clone();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if let Some(s) = this.launcher.as_mut() {
                            if let crate::launcher::LauncherTarget::Clone { repo_ref, .. } =
                                &mut s.target
                            {
                                *repo_ref = new_value;
                                cx.notify();
                            }
                        }
                    });
                });
            },
        )
        .detach();
        state
            .text_inputs
            .insert("__repo_ref".to_string(), repo_ref_entity);

        self.launcher = Some(state);
        cx.notify();
    }

    pub fn close_launcher(&mut self, cx: &mut Context<Self>) {
        self.launcher = None;
        cx.notify();
    }

    pub fn handle_launcher_input_change(
        &mut self,
        name: String,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.launcher.as_mut() {
            state.set_input(&name, value);
            state.revalidate();
            cx.notify();
        }
    }

    pub fn handle_launcher_mode_change(
        &mut self,
        mode: crate::launcher::LauncherMode,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.launcher.as_mut() {
            state.mode = mode;
            cx.notify();
        }
    }

    pub fn handle_launcher_target_change(
        &mut self,
        target: crate::launcher::LauncherTarget,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.launcher.as_mut() {
            state.target = target;
            cx.notify();
        }
    }

    pub fn handle_launcher_pick_directory(&mut self, cx: &mut Context<Self>) {
        let Some(path) = crate::menu::app_menu::pick_directory_modal("Pick target directory")
        else {
            return; // User cancelled
        };
        if let Some(state) = self.launcher.as_mut() {
            state.target = crate::launcher::LauncherTarget::Directory(path);
            cx.notify();
        }
    }

    pub fn handle_launcher_run(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.launcher.clone() else {
            return;
        };
        if !state.can_run() {
            return;
        }
        // Extract what we need from target before consuming state.
        enum DispatchKind {
            Direct(PathBuf),
            CloneFirst(String),
            NoOp,
        }
        let kind = match &state.target {
            crate::launcher::LauncherTarget::ThisWorkspace => {
                DispatchKind::Direct(PathBuf::from(&self.workspace.manifest.path))
            }
            crate::launcher::LauncherTarget::Directory(path) => DispatchKind::Direct(path.clone()),
            crate::launcher::LauncherTarget::Clone { repo_ref, status } => match status {
                crate::launcher::CloneStatus::Done(path) => DispatchKind::Direct(path.clone()),
                crate::launcher::CloneStatus::NotStarted
                | crate::launcher::CloneStatus::Failed(_) => {
                    DispatchKind::CloneFirst(repo_ref.clone())
                }
                crate::launcher::CloneStatus::InProgress => DispatchKind::NoOp,
            },
        };
        match kind {
            DispatchKind::Direct(target) => self.spawn_run(state, target, cx),
            DispatchKind::CloneFirst(repo_ref) => {
                self.spawn_clone_then_run(state, repo_ref, cx);
            }
            DispatchKind::NoOp => {}
        }
    }

    fn spawn_clone_then_run(
        &mut self,
        state: crate::launcher::LauncherState,
        repo_ref: String,
        cx: &mut Context<Self>,
    ) {
        // Flip status to InProgress immediately so the UI reflects the
        // change before the spawn fires.
        if let Some(s) = self.launcher.as_mut() {
            if let crate::launcher::LauncherTarget::Clone { status, .. } = &mut s.target {
                *status = crate::launcher::CloneStatus::InProgress;
            }
        }
        cx.notify();

        let registry = self.app_executor.config_mcp_registry();
        let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let clone_result = crate::launcher::clone::clone_repo_ref(registry, &repo_ref).await;
            match clone_result {
                Ok(path) => {
                    let _ = weak.update(cx, |this, cx| {
                        if let Some(s) = this.launcher.as_mut() {
                            if let crate::launcher::LauncherTarget::Clone { status, .. } =
                                &mut s.target
                            {
                                *status = crate::launcher::CloneStatus::Done(path.clone());
                            }
                        }
                        cx.notify();
                        this.spawn_run(state.clone(), path, cx);
                    });
                }
                Err(e) => {
                    let _ = weak.update(cx, |this, cx| {
                        if let Some(s) = this.launcher.as_mut() {
                            if let crate::launcher::LauncherTarget::Clone { status, .. } =
                                &mut s.target
                            {
                                *status = crate::launcher::CloneStatus::Failed(e.to_string());
                            }
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn spawn_run(
        &mut self,
        state: crate::launcher::LauncherState,
        target_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let app_exec = self.app_executor.clone();
        let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let workflow_path = state.workflow_path.clone();
        let inputs = state.inputs.clone();
        let mode = state.mode;
        cx.spawn(async move |_, cx| {
            match app_exec
                .start_workflow_with_opts(workflow_path.clone(), inputs, mode, target_dir)
                .await
            {
                Ok(run_id) => {
                    let _ = weak.update(cx, |this, cx| {
                        this.launcher = None;
                        this.run_model = Some(crate::run_model::RunModel::new(
                            run_id.clone(),
                            workflow_path.clone(),
                        ));
                        cx.notify();
                    });
                    // Subscribe to the new run's event stream (mirrors
                    // handle_run_clicked's existing pattern).
                    if let Ok(mut stream) = app_exec.attach(&run_id).await {
                        use futures_util::StreamExt;
                        while let Some(ev) = stream.next().await {
                            let res = weak.update(cx, |this, cx| {
                                if let Some(m) = this.run_model.take() {
                                    this.run_model = Some(m.apply(&ev));
                                }
                                cx.notify();
                            });
                            if res.is_err() {
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = weak.update(cx, |this, cx| {
                        if let Some(s) = this.launcher.as_mut() {
                            s.validation = Some(crate::launcher::ValidationError {
                                message: format!("start failed: {e}"),
                            });
                        }
                        cx.notify();
                    });
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

        // One cx.weak_entity() call for the whole render frame. All
        // callback closures share clones of this WeakEntity so we never
        // call cx.weak_entity() again mid-render (each extra call would
        // re-borrow the context and risk a RefCell conflict).
        let weak: WeakEntity<WorkspaceWindow> = cx.weak_entity();
        let weak_sidebar_click = weak.clone();
        let weak_sidebar_right = weak.clone();
        let weak_section_toggle = weak.clone();
        let weak_agent_click = weak.clone();
        let weak_agent_reveal = weak.clone();
        let weak2 = weak.clone();
        // Pre-cloned for the launcher overlay branch below.
        let weak_launcher = weak.clone();
        // Pre-cloned for the context menu dismiss callback.
        let weak_context_dismiss = weak.clone();
        let on_approve: ApproveCallback =
            Arc::new(move |step_id: String, window: &mut Window, cx: &mut App| {
                let _ = window;
                let weak = weak.clone();
                cx.defer(move |cx| {
                    weak.update(cx, |this, cx| {
                        this.handle_approve(step_id, cx);
                    })
                    .ok();
                });
            });
        let on_reject: RejectCallback = Arc::new(
            move |step_id: String, reason: String, window: &mut Window, cx: &mut App| {
                let _ = window;
                let weak2 = weak2.clone();
                cx.defer(move |cx| {
                    weak2
                        .update(cx, |this, cx| {
                            this.handle_reject(step_id, reason, cx);
                        })
                        .ok();
                });
            },
        );

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
                runs.into_iter()
                    .next()
                    .map(|r| (asset.path.clone(), r.status))
            })
            .collect();

        // Display whichever workflow the user most recently clicked in the
        // sidebar; fall back to the first project workflow if none has been
        // clicked yet. `current_workflow_path` handles both cases.
        let main_area = match self.current_workflow_path().and_then(|path| {
            self.workspace
                .project_assets
                .workflows
                .iter()
                .chain(self.workspace.global_assets.workflows.iter())
                .find(|a| a.path == path)
                .cloned()
        }) {
            Some(asset) => {
                let wf_path = asset.path.clone();
                let has_active = active_run_map.contains_key(&wf_path);
                render_main_for_workflow(
                    &asset,
                    run_model.as_ref(),
                    &transcript_lines,
                    on_approve.clone(),
                    on_reject.clone(),
                    has_active,
                    weak_sidebar_click.clone(),
                )
            }
            None => render_main_placeholder(),
        };

        // Keyboard approval shortcuts (`a` / `r` / ⌘R) are registered
        // globally at window-open time in WorkspaceWindow::open via WeakEntity
        // + cx.on_action. Registering them here per-frame caused a RefCell
        // already-borrowed panic at ~120Hz on 120Hz displays.

        let main_layout = div()
            .size_full()
            .bg(palette::BG_PRIMARY)
            .text_color(palette::TEXT_PRIMARY)
            .flex()
            .flex_col()
            .child(titlebar::render(
                &self.workspace,
                titlebar::in_flight_count(&active_run_map),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child({
                        let on_workflow_click: WorkflowClickCb = Arc::new(move |path, _w, cx| {
                            let weak_sidebar_click = weak_sidebar_click.clone();
                            cx.defer(move |cx| {
                                let _ = weak_sidebar_click
                                    .update(cx, |this, cx| this.handle_workflow_clicked(path, cx));
                            });
                        });
                        let on_workflow_right_click: sidebar::RightClickCb =
                            Arc::new(move |path, position, _w, cx| {
                                let weak_sidebar_right = weak_sidebar_right.clone();
                                cx.defer(move |cx| {
                                    let _ = weak_sidebar_right.update(cx, |this, cx| {
                                        this.open_workflow_context_menu(path, position, cx)
                                    });
                                });
                            });
                        let on_section_toggle: SectionToggleCb =
                            Arc::new(move |section, _w, cx| {
                                let weak = weak_section_toggle.clone();
                                cx.defer(move |cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.handle_section_toggle(section, cx)
                                    });
                                });
                            });
                        let on_agent_click: sidebar::AgentClickCb =
                            Arc::new(move |path, _w, cx| {
                                let weak = weak_agent_click.clone();
                                cx.defer(move |cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.handle_agent_clicked(path, cx)
                                    });
                                });
                            });
                        let on_agent_right_click: sidebar::RightClickCb =
                            Arc::new(move |path, position, _w, cx| {
                                let weak = weak_agent_reveal.clone();
                                cx.defer(move |cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.open_agent_context_menu(path, position, cx)
                                    });
                                });
                            });
                        sidebar::render(
                            &self.workspace,
                            &active_run_map,
                            self.focused_workflow.as_ref(),
                            on_workflow_click,
                            on_workflow_right_click,
                            on_agent_click,
                            on_agent_right_click,
                            on_section_toggle,
                        )
                    })
                    .child(main_area),
            );

        // Render the launcher overlay when present. KEY FIX 1: borrow
        // self.launcher via as_ref() rather than cloning it. KEY FIX 2:
        // launcher callbacks defer the entity update via cx.defer so the
        // WeakEntity is not re-entered while GPUI's click-dispatch already
        // holds a borrow → eliminates the RefCell-already-borrowed flood.
        let body: AnyElement = if let Some(state) = self.launcher.as_ref() {
            let weak2 = weak_launcher.clone();
            let on_input_change: crate::view::launcher::InputChangeCb =
                Arc::new(move |name, value, _w, cx| {
                    let weak3 = weak2.clone();
                    cx.defer(move |cx| {
                        let _ = weak3.update(cx, |this, cx| {
                            this.handle_launcher_input_change(name, value, cx);
                        });
                    });
                });

            let weak2 = weak_launcher.clone();
            let on_mode_change: crate::view::launcher::ModeChangeCb =
                Arc::new(move |mode, _w, cx| {
                    let weak3 = weak2.clone();
                    cx.defer(move |cx| {
                        let _ = weak3.update(cx, |this, cx| {
                            this.handle_launcher_mode_change(mode, cx);
                        });
                    });
                });

            let weak2 = weak_launcher.clone();
            let on_target_change: crate::view::launcher::TargetChangeCb =
                Arc::new(move |target, _w, cx| {
                    let weak3 = weak2.clone();
                    cx.defer(move |cx| {
                        let _ = weak3.update(cx, |this, cx| {
                            this.handle_launcher_target_change(target, cx);
                        });
                    });
                });

            let weak2 = weak_launcher.clone();
            let on_run: crate::view::launcher::RunCb = Arc::new(move |_w, cx| {
                let weak3 = weak2.clone();
                cx.defer(move |cx| {
                    let _ = weak3.update(cx, |this, cx| this.handle_launcher_run(cx));
                });
            });

            let weak2 = weak_launcher.clone();
            let on_close: crate::view::launcher::CloseCb = Arc::new(move |_w, cx| {
                let weak3 = weak2.clone();
                cx.defer(move |cx| {
                    let _ = weak3.update(cx, |this, cx| this.close_launcher(cx));
                });
            });

            let weak2 = weak_launcher.clone();
            let on_pick_dir: crate::view::launcher::PickDirCb = Arc::new(move |_w, cx| {
                let weak3 = weak2.clone();
                cx.defer(move |cx| {
                    let _ = weak3.update(cx, |this, cx| this.handle_launcher_pick_directory(cx));
                });
            });

            let _ = weak;
            div()
                .relative()
                .size_full()
                .child(main_layout)
                .child(crate::view::launcher::render(
                    state,
                    on_input_change,
                    on_mode_change,
                    on_target_change,
                    on_pick_dir,
                    on_run,
                    on_close,
                ))
                .into_any_element()
        } else {
            let _ = weak;
            let _ = weak_launcher;
            main_layout.into_any_element()
        };

        // Context menu overlay — paints on top of everything (including the
        // launcher sheet) when state.is_some(). Built as `deferred(...)` inside
        // the widget so paint order is correct.
        let on_dismiss: crate::widget::DismissCb = Arc::new(move |_w, cx| {
            let weak = weak_context_dismiss.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| this.handle_dismiss_context_menu(cx));
            });
        });

        let body_with_context: AnyElement = if let Some(menu_state) = self.context_menu.as_ref() {
            div()
                .relative()
                .size_full()
                .child(body)
                .child(crate::widget::context_menu::render(menu_state, on_dismiss))
                .into_any_element()
        } else {
            body
        };
        body_with_context
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
            let weak = weak.clone();
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| {
                    this.handle_run_clicked(cx);
                })
                .ok();
            });
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
