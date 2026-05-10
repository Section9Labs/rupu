use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RunRecord,
    RunEnvelope,
    WorkflowSnapshot,
    StepTranscript,
    StepOutput,
    Summary,
    ExternalLink,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: String,
    pub kind: ArtifactKind,
    pub name: String,
    pub producer: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub local_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub inline_json: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactManifest {
    pub version: u32,
    pub run_id: String,
    pub backend_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
    pub generated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
}

impl ArtifactManifest {
    pub const VERSION: u32 = 1;

    pub fn new(run_id: impl Into<String>, backend_id: impl Into<String>) -> Self {
        Self {
            version: Self::VERSION,
            run_id: run_id.into(),
            backend_id: backend_id.into(),
            worker_id: None,
            generated_at: Utc::now().to_rfc3339(),
            artifacts: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_manifest_round_trips_json() {
        let manifest = ArtifactManifest {
            version: ArtifactManifest::VERSION,
            run_id: "run_01JXYZ".into(),
            backend_id: "local_worktree".into(),
            worker_id: Some("worker_local_cli".into()),
            generated_at: "2026-05-09T17:00:00Z".into(),
            artifacts: vec![
                ArtifactRef {
                    id: "art_run".into(),
                    kind: ArtifactKind::RunRecord,
                    name: "run-record".into(),
                    producer: "run".into(),
                    local_path: Some(PathBuf::from("/tmp/runs/run_01/run.json")),
                    uri: None,
                    inline_json: None,
                },
                ArtifactRef {
                    id: "art_outcome".into(),
                    kind: ArtifactKind::Summary,
                    name: "autoflow-outcome".into(),
                    producer: "step.finalize".into(),
                    local_path: None,
                    uri: None,
                    inline_json: Some(serde_json::json!({
                        "status": "await_human",
                        "summary": "draft PR opened"
                    })),
                },
            ],
        };

        let encoded = serde_json::to_string_pretty(&manifest).unwrap();
        let decoded: ArtifactManifest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, manifest);
    }
}
