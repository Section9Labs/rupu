//! LauncherState — pure data driving the launcher sheet. Mutated by
//! user input + the clone task. Validation re-runs on every keystroke.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rupu_orchestrator::Workflow;

#[derive(Clone)]
pub struct LauncherState {
    pub workflow_path: PathBuf,
    pub workflow: Workflow,
    pub inputs: BTreeMap<String, String>,
    pub mode: LauncherMode,
    pub target: LauncherTarget,
    pub validation: Option<ValidationError>,
    /// One `Entity<TextInput>` per text/int workflow input, keyed by input name.
    /// Plus the reserved key `"__repo_ref"` for the Clone target's repo ref.
    /// Constructed in `WorkspaceWindow::open_launcher` because `cx.new` is
    /// only callable with a mutable `App` context, not at struct-literal time.
    pub text_inputs: BTreeMap<String, gpui::Entity<crate::widget::TextInput>>,
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
            text_inputs: BTreeMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::{InputDef, InputType, Trigger, WorkflowDefaults, Contracts};

    fn workflow_with_input(name: &str, ty: InputType, required: bool) -> Workflow {
        let mut inputs = BTreeMap::new();
        inputs.insert(
            name.to_string(),
            InputDef {
                ty,
                required,
                default: None,
                description: None,
                allowed: vec![],
            },
        );
        Workflow {
            name: "test".to_string(),
            description: None,
            trigger: Trigger::default(),
            inputs,
            defaults: WorkflowDefaults::default(),
            autoflow: None,
            contracts: Contracts {
                outputs: BTreeMap::new(),
            },
            notify_issue: false,
            steps: vec![],
        }
    }

    #[test]
    fn set_input_then_revalidate_clears_error_when_required_input_provided() {
        let wf = workflow_with_input("repo", InputType::String, true);
        let mut state = LauncherState::new("/tmp/wf.yml".into(), wf);
        // Required input is missing → validation should fail.
        assert!(state.validation.is_some(), "expected validation error on init");
        state.set_input("repo", "github:foo/bar");
        state.revalidate();
        assert!(
            state.validation.is_none(),
            "expected validation to clear once required input set, got {:?}",
            state.validation
        );
    }

    #[test]
    fn set_input_with_empty_string_removes_the_entry() {
        let wf = workflow_with_input("repo", InputType::String, false);
        let mut state = LauncherState::new("/tmp/wf.yml".into(), wf);
        state.set_input("repo", "value");
        assert_eq!(state.inputs.get("repo").map(|s| s.as_str()), Some("value"));
        state.set_input("repo", "");
        assert!(state.inputs.get("repo").is_none());
    }
}
