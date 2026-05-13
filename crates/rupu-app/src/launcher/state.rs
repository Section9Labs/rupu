//! LauncherState — pure data driving the launcher sheet. Mutated by
//! user input + the clone task. Validation re-runs on every keystroke.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rupu_orchestrator::Workflow;

#[derive(Debug, Clone)]
pub struct LauncherState {
    pub workflow_path: PathBuf,
    pub workflow: Workflow,
    pub inputs: BTreeMap<String, String>,
    pub mode: LauncherMode,
    pub target: LauncherTarget,
    pub validation: Option<ValidationError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherMode {
    Ask,
    Bypass,
    ReadOnly,
}

impl LauncherMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LauncherMode::Ask => "ask",
            LauncherMode::Bypass => "bypass",
            LauncherMode::ReadOnly => "readonly",
        }
    }
}

#[derive(Debug, Clone)]
pub enum LauncherTarget {
    ThisWorkspace,
    Directory(PathBuf),
    Clone {
        repo_ref: String,
        status: CloneStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneStatus {
    NotStarted,
    InProgress,
    Done(PathBuf),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub message: String,
}

impl LauncherState {
    pub fn new(workflow_path: PathBuf, workflow: Workflow) -> Self {
        let mut inputs: BTreeMap<String, String> = BTreeMap::new();
        for (name, def) in &workflow.inputs {
            if let Some(default) = &def.default {
                if let Some(s) = yaml_value_to_string(default) {
                    inputs.insert(name.clone(), s);
                }
            }
        }
        let mut state = Self {
            workflow_path,
            workflow,
            inputs,
            mode: LauncherMode::Ask,
            target: LauncherTarget::ThisWorkspace,
            validation: None,
        };
        state.revalidate();
        state
    }

    pub fn set_input(&mut self, name: &str, value: impl Into<String>) {
        let v = value.into();
        if v.is_empty() {
            self.inputs.remove(name);
        } else {
            self.inputs.insert(name.into(), v);
        }
    }

    pub fn revalidate(&mut self) {
        match rupu_orchestrator::resolve_inputs(&self.workflow, &self.inputs) {
            Ok(_) => self.validation = None,
            Err(e) => {
                self.validation = Some(ValidationError {
                    message: e.to_string(),
                })
            }
        }
    }

    pub fn can_run(&self) -> bool {
        if self.validation.is_some() {
            return false;
        }
        matches!(
            &self.target,
            LauncherTarget::ThisWorkspace
                | LauncherTarget::Directory(_)
                | LauncherTarget::Clone {
                    status: CloneStatus::Done(_) | CloneStatus::NotStarted | CloneStatus::Failed(_),
                    ..
                }
        )
    }
}

fn yaml_value_to_string(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
