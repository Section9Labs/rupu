use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    WorkflowRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowBinding {
    pub name: String,
    pub source_path: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RepoBinding {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub project_root: Option<PathBuf>,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTriggerSource {
    WorkflowCli,
    IssueCommand,
    EventDispatch,
    CronEvent,
    Autoflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTrigger {
    pub source: RunTriggerSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub wake_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunContext {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub event_present: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub issue_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRequest {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub backend: Option<String>,
    pub permission_mode: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workspace_strategy: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict_templates: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub attach_ui: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoflowEnvelope {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claim_id: Option<String>,
    pub priority: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunCorrelation {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dispatch_group_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkerRequest {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub requested_worker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub assigned_worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEnvelope {
    pub version: u32,
    pub run_id: String,
    pub kind: RunKind,
    pub workflow: WorkflowBinding,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo: Option<RepoBinding>,
    pub trigger: RunTrigger,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub context: Option<RunContext>,
    pub execution: ExecutionRequest,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub autoflow: Option<AutoflowEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub correlation: Option<RunCorrelation>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker: Option<WorkerRequest>,
}

impl RunEnvelope {
    pub const VERSION: u32 = 1;
}

const fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_envelope_round_trips_json() {
        let envelope = RunEnvelope {
            version: RunEnvelope::VERSION,
            run_id: "run_01JXYZ".into(),
            kind: RunKind::WorkflowRun,
            workflow: WorkflowBinding {
                name: "phase-delivery-cycle".into(),
                source_path: PathBuf::from(".rupu/workflows/phase-delivery-cycle.yaml"),
                fingerprint: "sha256:abc123".into(),
            },
            repo: Some(RepoBinding {
                repo_ref: Some("github:Section9Labs/rupu".into()),
                project_root: Some(PathBuf::from("/tmp/repo")),
                workspace_id: "ws_01".into(),
                workspace_path: PathBuf::from("/tmp/repo"),
            }),
            trigger: RunTrigger {
                source: RunTriggerSource::Autoflow,
                wake_id: Some("wake_01".into()),
                event_id: Some("github.pull_request.merged".into()),
            },
            inputs: BTreeMap::from([(String::from("phase"), String::from("phase-2"))]),
            context: Some(RunContext {
                issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
                target: Some("github:Section9Labs/rupu/issues/42".into()),
                event_present: true,
                issue_present: true,
            }),
            execution: ExecutionRequest {
                backend: Some("local_worktree".into()),
                permission_mode: "bypass".into(),
                workspace_strategy: Some("managed_worktree".into()),
                strict_templates: true,
                attach_ui: false,
                use_canvas: false,
            },
            autoflow: Some(AutoflowEnvelope {
                name: "issue-supervisor-dispatch".into(),
                claim_id: Some("github:Section9Labs/rupu/issues/42".into()),
                priority: 100,
            }),
            correlation: Some(RunCorrelation {
                parent_run_id: Some("run_parent".into()),
                dispatch_group_id: Some("dispatch_01".into()),
            }),
            worker: Some(WorkerRequest {
                requested_worker: Some("local".into()),
                assigned_worker_id: Some("worker_local_cli".into()),
            }),
        };

        let encoded = serde_json::to_string_pretty(&envelope).unwrap();
        let decoded: RunEnvelope = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn run_envelope_version_is_stable() {
        assert_eq!(RunEnvelope::VERSION, 1);
    }
}
