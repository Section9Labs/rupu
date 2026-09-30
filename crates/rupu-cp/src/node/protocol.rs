use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `Welcome` capability: the CP mirrors `ArtifactFile::Usage` lines into the
/// run's `usage.jsonl`. A node only forwards its usage ledger when it sees this.
pub const CAP_USAGE_LEDGER: &str = "usage_ledger";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Hello {
        node_id: String,
        auth: Auth,
        rupu_version: String,
        capabilities: Vec<String>,
    },
    /// CP→node handshake reply. `capabilities` lists optional protocol
    /// features the CP understands (e.g. [`CAP_USAGE_LEDGER`],
    /// [`CAP_MIRROR_COVERAGE`]; see [`cp_capabilities`]); a node must not
    /// send frames gated on a capability the CP did not advertise. An older
    /// CP sends none, so the field defaults empty and the node sends only
    /// what every CP understands. Absent on the wire when empty so an older
    /// node/CP round-trips the frame.
    Welcome {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
    },
    Run {
        run_id: String,
        spec: RunSpec,
    },
    Cancel {
        run_id: String,
    },
    /// CP→node: approve a run paused at an approval gate. `mode` is the
    /// resume mode (`"ask"` | `"bypass"` | `"readonly"`); empty means the
    /// node uses the run's stored mode / default.
    Approve {
        run_id: String,
        mode: String,
    },
    /// CP→node: reject a run paused at an approval gate.
    Reject {
        run_id: String,
        reason: Option<String>,
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
    /// Agent runs only: `rupu run --findings-profile`. Absent on the wire when
    /// `None`, so a job without an override is byte-identical to before.
    ///
    /// An executor predating this field would silently drop it, so a sender
    /// must only set it for an executor known to honour it — see
    /// [`CAP_AGENT_FINDINGS_PROFILE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
}

/// `Hello.capabilities` entry: this node's executor passes
/// [`RunSpec::findings_profile`] through to `rupu run --findings-profile`.
/// A tunnel that has not seen it refuses a launch that carries a profile
/// rather than let an older node run the agent under a different one.
pub const CAP_AGENT_FINDINGS_PROFILE: &str = "agent.findings_profile";

/// `Welcome.capabilities` entry: this CP mirrors [`ArtifactFile::Coverage`].
pub const CAP_MIRROR_COVERAGE: &str = "mirror.coverage";

/// HTTP `/api/host/info` `features` entry: this CP serves
/// `GET /api/runs/:id/coverage`.
pub const CAP_RUN_COVERAGE_STREAM: &str = "run.coverage_stream";

/// What this CP advertises to a node in `Welcome`.
pub fn cp_capabilities() -> Vec<String> {
    vec![
        CAP_USAGE_LEDGER.to_string(),
        CAP_MIRROR_COVERAGE.to_string(),
    ]
}

/// Every capability this build's node executor supports — what `rupu node`
/// advertises in `Hello`.
pub fn node_capabilities() -> Vec<String> {
    vec![CAP_AGENT_FINDINGS_PROFILE.to_string()]
}

/// Host feature: this build's `rupu workflow resume` takes `--if-unfinished`
/// (refuse a run that finished since the resume was requested, instead of
/// retrying it). The SSH connector passes the flag only to a remote whose
/// `rupu __features` lists this: an older remote's clap rejects the flag,
/// and the resume runs detached, so it would fail without a trace.
pub const CAP_WORKFLOW_RESUME_IF_UNFINISHED: &str = "workflow.resume_if_unfinished";

/// Every feature this build honours as a host — what `/api/host/info`
/// serves as `features` and what `rupu __features` prints for an SSH
/// coordinator. Same vocabulary as the tunnel `Hello.capabilities` and the
/// bucket worker markers.
pub fn host_features() -> Vec<String> {
    vec![
        CAP_AGENT_FINDINGS_PROFILE.to_string(),
        CAP_WORKFLOW_RESUME_IF_UNFINISHED.to_string(),
    ]
}

/// The hidden `rupu` subcommand that prints this build's [`FeaturesReport`]
/// — what an SSH coordinator runs on a remote to learn its features.
pub const FEATURES_SUBCOMMAND: &str = "__features";

/// What `rupu __features` prints: one JSON object on stdout. A remote
/// predating the command prints nothing, which reads as no features.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct FeaturesReport {
    #[serde(default)]
    pub features: Vec<String>,
}

impl FeaturesReport {
    /// This build's report.
    pub fn current() -> Self {
        Self {
            features: host_features(),
        }
    }

    pub fn supports(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
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
    /// The run's agent transcript (`$RUPU_HOME/transcripts/<run_id>.jsonl` on
    /// the executing host — note: NOT under `runs/<run_id>/`). Written by
    /// placed/standalone agent runs; workflow runs write per-step transcripts
    /// elsewhere and never produce this artifact.
    Transcript,
    /// The run's usage ledger (`runs/<id>/usage.jsonl`, spec 2026-09-29 §3).
    /// Only sent to a tunnel CP that advertised [`CAP_USAGE_LEDGER`].
    Usage,
    /// The run's coverage stream (`runs/<run_id>/coverage.jsonl`). Sent only
    /// to a CP that advertised [`CAP_MIRROR_COVERAGE`] — an older CP fails to
    /// parse an unknown variant.
    Coverage,
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
                findings_profile: None,
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

    #[test]
    fn approve_frame_round_trips() {
        let f = Frame::Approve {
            run_id: "run_01ABC".to_string(),
            mode: "bypass".to_string(),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"approve""#));
        assert!(json.contains(r#""run_id":"run_01ABC""#));
        assert!(json.contains(r#""mode":"bypass""#));
        let back: Frame = serde_json::from_str(&json).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn reject_frame_round_trips_with_and_without_reason() {
        let with = Frame::Reject {
            run_id: "run_01ABC".to_string(),
            reason: Some("not now".to_string()),
        };
        let json = serde_json::to_string(&with).unwrap();
        assert!(json.contains(r#""type":"reject""#));
        assert!(json.contains(r#""reason":"not now""#));
        assert_eq!(serde_json::from_str::<Frame>(&json).unwrap(), with);

        let without = Frame::Reject {
            run_id: "run_01ABC".to_string(),
            reason: None,
        };
        let json2 = serde_json::to_string(&without).unwrap();
        assert_eq!(serde_json::from_str::<Frame>(&json2).unwrap(), without);
    }

    #[test]
    fn artifact_file_usage_round_trips_as_usage() {
        let json = serde_json::to_string(&ArtifactFile::Usage).unwrap();
        assert_eq!(json, r#""usage""#);
        let back: ArtifactFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ArtifactFile::Usage);
    }

    #[test]
    fn welcome_from_an_old_cp_has_no_capabilities() {
        // A pre-`usage_ledger` CP sends a bare `{"type":"welcome"}`.
        let f: Frame = serde_json::from_str(r#"{"type":"welcome"}"#).unwrap();
        assert_eq!(
            f,
            Frame::Welcome {
                capabilities: vec![]
            }
        );
        // An empty capability list is not serialized, so a CP that
        // advertises nothing stays byte-identical on the wire.
        assert_eq!(serde_json::to_string(&f).unwrap(), r#"{"type":"welcome"}"#);
    }

    #[test]
    fn welcome_capabilities_round_trip() {
        let f = Frame::Welcome {
            capabilities: vec![CAP_USAGE_LEDGER.to_string()],
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""capabilities":["usage_ledger"]"#));
        assert_eq!(serde_json::from_str::<Frame>(&json).unwrap(), f);
    }

    #[test]
    fn host_features_advertise_resume_if_unfinished() {
        let features = host_features();
        assert!(
            features
                .iter()
                .any(|f| f == CAP_WORKFLOW_RESUME_IF_UNFINISHED),
            "{features:?}"
        );
        assert!(
            features.iter().any(|f| f == CAP_AGENT_FINDINGS_PROFILE),
            "{features:?}"
        );
    }

    #[test]
    fn welcome_carries_cp_capabilities_and_old_welcomes_still_parse() {
        let w = Frame::Welcome {
            capabilities: cp_capabilities(),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains(CAP_MIRROR_COVERAGE), "{json}");
        assert!(json.contains(CAP_USAGE_LEDGER), "{json}");
        // An older CP sends `{"type":"welcome"}`.
        let old: Frame = serde_json::from_str(r#"{"type":"welcome"}"#).unwrap();
        assert_eq!(
            old,
            Frame::Welcome {
                capabilities: vec![]
            }
        );
    }

    #[test]
    fn features_report_round_trips_this_builds_features() {
        let json = serde_json::to_string(&FeaturesReport::current()).unwrap();
        let back: FeaturesReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.features, host_features());
        assert!(back.supports(CAP_WORKFLOW_RESUME_IF_UNFINISHED));
    }

    #[test]
    fn features_report_ignores_fields_it_does_not_know() {
        // A newer peer may add fields; an older reader still sees the list.
        let back: FeaturesReport =
            serde_json::from_str(r#"{"features":["agent.findings_profile"],"later":1}"#).unwrap();
        assert!(back.supports(CAP_AGENT_FINDINGS_PROFILE));
        assert!(!back.supports(CAP_WORKFLOW_RESUME_IF_UNFINISHED));
    }

    #[test]
    fn coverage_artifact_frame_round_trips() {
        let f = Frame::Artifact {
            run_id: "run_1".into(),
            file: ArtifactFile::Coverage,
            line: r#"{"ledger":"begin","v":1,"run_id":"run_1"}"#.into(),
        };
        let back: Frame = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }
}
