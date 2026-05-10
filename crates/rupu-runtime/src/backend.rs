use crate::{ArtifactManifest, RunEnvelope};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub trait ExecutionBackend {
    fn id(&self) -> &'static str;
    fn can_execute(&self, envelope: &RunEnvelope) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRun {
    pub version: u32,
    pub run_id: String,
    pub backend_id: String,
    pub workspace_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub project_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workspace_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
}

impl PreparedRun {
    pub const VERSION: u32 = 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResultStatus {
    Completed,
    Failed,
    AwaitingApproval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub version: u32,
    pub run_id: String,
    pub backend_id: String,
    pub status: RunResultStatus,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_wake_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub artifact_manifest: Option<ArtifactManifest>,
}

impl RunResult {
    pub const VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionRequest, RunKind, RunTrigger, RunTriggerSource, WorkflowBinding};
    use std::collections::BTreeMap;

    struct LocalWorktreeBackend;

    impl ExecutionBackend for LocalWorktreeBackend {
        fn id(&self) -> &'static str {
            "local_worktree"
        }

        fn can_execute(&self, envelope: &RunEnvelope) -> bool {
            envelope
                .execution
                .backend
                .as_deref()
                .unwrap_or("local_worktree")
                == self.id()
        }
    }

    #[test]
    fn prepared_run_and_result_round_trip_json() {
        let prepared = PreparedRun {
            version: PreparedRun::VERSION,
            run_id: "run_01JXYZ".into(),
            backend_id: "local_worktree".into(),
            workspace_path: PathBuf::from("/tmp/repo"),
            project_root: Some(PathBuf::from("/tmp/repo")),
            repo_ref: Some("github:Section9Labs/rupu".into()),
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            workspace_strategy: Some("managed_worktree".into()),
            worker_id: Some("worker_local_cli".into()),
        };
        let encoded = serde_json::to_string_pretty(&prepared).unwrap();
        let decoded: PreparedRun = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, prepared);

        let result = RunResult {
            version: RunResult::VERSION,
            run_id: "run_01JXYZ".into(),
            backend_id: "local_worktree".into(),
            status: RunResultStatus::Completed,
            worker_id: Some("worker_local_cli".into()),
            source_wake_id: Some("wake_01".into()),
            artifact_manifest: Some(ArtifactManifest::new("run_01JXYZ", "local_worktree")),
        };
        let encoded = serde_json::to_string_pretty(&result).unwrap();
        let decoded: RunResult = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn local_worktree_backend_accepts_local_worktree_envelopes() {
        let envelope = RunEnvelope {
            version: 1,
            run_id: "run_01JXYZ".into(),
            kind: RunKind::WorkflowRun,
            workflow: WorkflowBinding {
                name: "hello".into(),
                source_path: PathBuf::from("hello.yaml"),
                fingerprint: "sha256:abc".into(),
            },
            repo: None,
            trigger: RunTrigger {
                source: RunTriggerSource::WorkflowCli,
                wake_id: None,
                event_id: None,
            },
            inputs: BTreeMap::new(),
            context: None,
            execution: ExecutionRequest {
                backend: Some("local_worktree".into()),
                permission_mode: "bypass".into(),
                workspace_strategy: Some("managed_worktree".into()),
                strict_templates: false,
                attach_ui: false,
                use_canvas: false,
            },
            autoflow: None,
            correlation: None,
            worker: None,
        };

        let backend = LocalWorktreeBackend;
        assert!(backend.can_execute(&envelope));
        assert_eq!(backend.id(), "local_worktree");
    }
}
