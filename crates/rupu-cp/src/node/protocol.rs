use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Hello {
        node_id: String,
        auth: Auth,
        rupu_version: String,
        capabilities: Vec<String>,
    },
    Welcome {},
    Run {
        run_id: String,
        spec: RunSpec,
    },
    Cancel {
        run_id: String,
    },
    Ping {},
    Pong {},
    Artifact {
        run_id: String,
        file: ArtifactFile,
        line: String,
    },
    RunFinished {
        run_id: String,
        status: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    Token { token: String },
    Mtls {},
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RunSpec {
    pub kind: RunSpecKind,
    pub name: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    pub prompt: Option<String>,
    pub mode: Option<String>,
    pub target: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunSpecKind {
    Workflow,
    Agent,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFile {
    Events,
    StepResults,
    UnitCheckpoints,
    RunJson,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        // Test Frame::Run round-trip
        let run_frame = Frame::Run {
            run_id: "r1".to_string(),
            spec: RunSpec {
                kind: RunSpecKind::Workflow,
                name: "wf".to_string(),
                inputs: BTreeMap::new(),
                prompt: None,
                mode: None,
                target: None,
            },
        };

        let serialized = serde_json::to_string(&run_frame).expect("Failed to serialize Frame::Run");
        let deserialized: Frame =
            serde_json::from_str(&serialized).expect("Failed to deserialize Frame::Run");
        assert_eq!(run_frame, deserialized);

        // Test Frame::Artifact round-trip
        let artifact_frame = Frame::Artifact {
            run_id: "r1".to_string(),
            file: ArtifactFile::Events,
            line: r#"{"type":"event"}"#.to_string(),
        };

        let serialized =
            serde_json::to_string(&artifact_frame).expect("Failed to serialize Frame::Artifact");
        let deserialized: Frame =
            serde_json::from_str(&serialized).expect("Failed to deserialize Frame::Artifact");
        assert_eq!(artifact_frame, deserialized);
    }

    #[test]
    fn hello_token_auth_serialization() {
        let hello_frame = Frame::Hello {
            node_id: "node-1".to_string(),
            auth: Auth::Token {
                token: "secret123".to_string(),
            },
            rupu_version: "0.1.0".to_string(),
            capabilities: vec!["workflow".to_string()],
        };

        let serialized =
            serde_json::to_string(&hello_frame).expect("Failed to serialize Frame::Hello");
        let json: serde_json::Value =
            serde_json::from_str(&serialized).expect("Failed to parse JSON");

        // Assert that auth.kind == "token"
        assert_eq!(json["auth"]["kind"], "token");
        assert_eq!(json["type"], "hello");
    }
}
