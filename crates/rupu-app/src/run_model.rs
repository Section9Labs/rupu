//! RunModel — mutable per-run state in the app. Built by applying
//! Events from the executor stream. `apply()` is a pure function so
//! tests can drive it deterministically.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rupu_app_canvas::NodeStatus;
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::RunStatus;

#[derive(Debug, Clone)]
pub struct RunModel {
    pub run_id: String,
    pub workflow_path: PathBuf,
    pub run_status: RunStatus,
    pub nodes: BTreeMap<String, NodeStatus>,
    pub active_step: Option<String>,
    pub focused_step: Option<String>,
    pub focused_step_last_set: Option<chrono::DateTime<chrono::Utc>>,
}

impl RunModel {
    pub fn new(run_id: String, workflow_path: PathBuf) -> Self {
        Self {
            run_id,
            workflow_path,
            run_status: RunStatus::Pending,
            nodes: BTreeMap::new(),
            active_step: None,
            focused_step: None,
            focused_step_last_set: None,
        }
    }

    pub fn apply(mut self, ev: &Event) -> Self {
        match ev {
            Event::RunStarted { .. } => {
                self.run_status = RunStatus::Running;
            }
            Event::StepStarted { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Active);
                self.active_step = Some(step_id.clone());
            }
            Event::StepWorking { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Working);
            }
            Event::StepAwaitingApproval { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Awaiting);
                self.run_status = RunStatus::AwaitingApproval;
                let should_auto_focus = self
                    .focused_step_last_set
                    .map(|t| chrono::Utc::now().signed_duration_since(t).num_seconds() >= 10)
                    .unwrap_or(true);
                if should_auto_focus {
                    self.focused_step = Some(step_id.clone());
                    self.focused_step_last_set = Some(chrono::Utc::now());
                }
            }
            Event::StepCompleted { step_id, success, .. } => {
                let status = if *success { NodeStatus::Complete } else { NodeStatus::SoftFailed };
                self.nodes.insert(step_id.clone(), status);
                if self.active_step.as_deref() == Some(step_id) {
                    self.active_step = None;
                }
            }
            Event::StepFailed { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Failed);
                if self.active_step.as_deref() == Some(step_id) {
                    self.active_step = None;
                }
            }
            Event::StepSkipped { step_id, .. } => {
                self.nodes.insert(step_id.clone(), NodeStatus::Skipped);
            }
            Event::RunCompleted { status, .. } => {
                self.run_status = *status;
                self.active_step = None;
            }
            Event::RunFailed { .. } => {
                self.run_status = RunStatus::Failed;
                self.active_step = None;
            }
        }
        self
    }

    /// Called when the user clicks a node — overrides auto-focus.
    pub fn set_user_focus(&mut self, step_id: Option<String>) {
        self.focused_step = step_id;
        self.focused_step_last_set = Some(chrono::Utc::now());
    }
}
