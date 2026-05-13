//! LauncherState validation transitions. Each test exercises one
//! InputDef shape against `LauncherState::new` + per-keystroke
//! validation via `revalidate`.

use std::path::PathBuf;

use rupu_app::launcher::{LauncherMode, LauncherState, LauncherTarget};
use rupu_orchestrator::Workflow;

fn parse_wf(yaml: &str) -> Workflow {
    Workflow::parse(yaml).expect("parse")
}

#[test]
fn new_prefills_string_default() {
    let yaml = r#"
name: t
inputs:
  topic:
    type: string
    required: true
    default: "hello"
steps:
  - id: a
    agent: x
    prompt: "{{ input.topic }}"
"#;
    let wf = parse_wf(yaml);
    let state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    assert_eq!(state.inputs.get("topic").map(String::as_str), Some("hello"));
    assert!(matches!(state.mode, LauncherMode::Ask));
    assert!(matches!(state.target, LauncherTarget::ThisWorkspace));
    assert!(state.validation.is_none(), "default pre-fill should validate");
}

#[test]
fn missing_required_input_surfaces_validation_error() {
    let yaml = r#"
name: t
inputs:
  topic:
    type: string
    required: true
steps:
  - id: a
    agent: x
    prompt: "{{ input.topic }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("topic", "");
    state.revalidate();
    assert!(state.validation.is_some(), "empty required input must error");
}

#[test]
fn int_type_mismatch_surfaces_validation_error() {
    let yaml = r#"
name: t
inputs:
  count:
    type: int
    required: false
    default: 1
steps:
  - id: a
    agent: x
    prompt: "{{ input.count }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("count", "not-an-int");
    state.revalidate();
    assert!(state.validation.is_some());
}

#[test]
fn enum_not_allowed_surfaces_validation_error() {
    let yaml = r#"
name: t
inputs:
  mode:
    type: string
    required: true
    default: "fast"
    enum: [fast, slow]
steps:
  - id: a
    agent: x
    prompt: "{{ input.mode }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("mode", "wibble");
    state.revalidate();
    assert!(state.validation.is_some());
}

#[test]
fn valid_inputs_clear_validation() {
    let yaml = r#"
name: t
inputs:
  topic:
    type: string
    required: true
steps:
  - id: a
    agent: x
    prompt: "{{ input.topic }}"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.set_input("topic", "hello");
    state.revalidate();
    assert!(state.validation.is_none());
}

#[test]
fn mode_changes_persist() {
    let yaml = r#"
name: t
steps:
  - id: a
    agent: x
    prompt: "go"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.mode = LauncherMode::Bypass;
    assert!(matches!(state.mode, LauncherMode::Bypass));
}

#[test]
fn target_clone_status_transitions() {
    let yaml = r#"
name: t
steps:
  - id: a
    agent: x
    prompt: "go"
"#;
    let wf = parse_wf(yaml);
    let mut state = LauncherState::new(PathBuf::from("/wf.yaml"), wf);
    state.target = LauncherTarget::Clone {
        repo_ref: "github:foo/bar".into(),
        status: rupu_app::launcher::CloneStatus::NotStarted,
    };
    if let LauncherTarget::Clone { ref mut status, .. } = state.target {
        *status = rupu_app::launcher::CloneStatus::InProgress;
    }
    assert!(matches!(
        state.target,
        LauncherTarget::Clone {
            status: rupu_app::launcher::CloneStatus::InProgress,
            ..
        }
    ));
}
