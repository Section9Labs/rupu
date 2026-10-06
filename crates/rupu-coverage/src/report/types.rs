//! Typed finding report. Field names follow the reporting standard the spec
//! was drawn from; where that standard used prose for something with obvious
//! structure (call chain, evidence, patch, tests) this uses the structure.
//!
//! Every struct is `deny_unknown_fields`: a misspelled field is a loud parse
//! error back to the agent, never silently dropped content.

use crate::catalog::types::Severity;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::marker::PhantomData;

/// Prefix of the one sentinel allowed on mandatory-but-sometimes-impossible
/// sections. Must be followed by a non-empty justification.
pub const NOT_PROVIDED_PREFIX: &str = "Not Provided — ";

/// A field that is either real content or a sentinel string. Which sentinel
/// strings are acceptable is per field and enforced by
/// [`crate::report::validate_report`], not by the type.
///
/// Serializes untagged. Deserializes by hand rather than `untagged`: an
/// untagged enum buffers the input and, when no variant fits, replaces every
/// nested error with "data did not match any variant", so a typo three
/// levels down (`call_chain[0].role`) would surface as a shapeless complaint
/// about the whole field. Here a JSON string is always the sentinel, and
/// anything else is deserialized as `T` in place, propagating `T`'s own
/// error (and its field path, under `serde_path_to_error`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum OrSentinel<T> {
    Value(T),
    Sentinel(String),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OrSentinel<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
        use serde::de::{Error, IntoDeserializer, MapAccess, SeqAccess, Visitor};

        struct OrSentinelVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for OrSentinelVisitor<T> {
            type Value = OrSentinel<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the field's value, or one of its sentinel strings")
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(OrSentinel::Sentinel(v.to_owned()))
            }

            fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(OrSentinel::Sentinel(v))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                T::deserialize(SeqAccessDeserializer::new(seq)).map(OrSentinel::Value)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(OrSentinel::Value)
            }

            // Scalars are never a valid `T` here, but let `T` say so in its
            // own words ("invalid type: boolean `true`, expected a sequence").
            fn visit_bool<E: Error>(self, v: bool) -> Result<Self::Value, E> {
                T::deserialize(v.into_deserializer()).map(OrSentinel::Value)
            }

            fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
                T::deserialize(v.into_deserializer()).map(OrSentinel::Value)
            }

            fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
                T::deserialize(v.into_deserializer()).map(OrSentinel::Value)
            }

            fn visit_f64<E: Error>(self, v: f64) -> Result<Self::Value, E> {
                T::deserialize(v.into_deserializer()).map(OrSentinel::Value)
            }

            fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
                T::deserialize(().into_deserializer()).map(OrSentinel::Value)
            }

            fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }
        }

        d.deserialize_any(OrSentinelVisitor(PhantomData))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl From<RiskLevel> for Severity {
    fn from(r: RiskLevel) -> Self {
        match r {
            RiskLevel::Low => Severity::Low,
            RiskLevel::Medium => Severity::Medium,
            RiskLevel::High => Severity::High,
            RiskLevel::Critical => Severity::Critical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Likelihood {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ownership {
    pub owner: String,
    pub product: String,
    pub affected_component: String,
    pub source_repository: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    #[serde(rename = "type")]
    pub kind: String,
    pub identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rating {
    pub impact: RiskLevel,
    pub likelihood: Likelihood,
    pub risk_rating: RiskLevel,
    pub risk_factor: RiskLevel,
    /// Base score, optionally with the vector string, or `Unknown`.
    pub cvss_v3: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportLocation {
    pub input: String,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HopRole {
    Source,
    Hop,
    Sink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainHop {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_va: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes_because: Option<String>,
    pub role: HopRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceClaim {
    pub claim: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_va: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// SHA-256 of `file` at write time. Set by rupu, not the agent: it is
    /// what lets a viewer flag a claim whose code has since changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Path of an entry in `artifacts` this claim is proven by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

/// A classification spanning any taxonomy, generalizing `cwe`/`cvss_v3`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Classification {
    /// e.g. `"CWE"`, `"CVE"`, `"CAPEC"`, `"ATT&CK"`, `"OWASP"`, `"MASVS"`.
    pub system: String,
    /// e.g. `"CWE-306"`, `"CVE-2024-1234"`, `"T1190"`.
    pub id: String,
    /// e.g. a CVSS vector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<String>,
}

/// One line of a disassembly listing (a [`EvidenceBlock::Disasm`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisasmLine {
    pub address: u64,
    pub bytes: String,
    pub mnemonic: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ops: String,
}

/// A typed evidence block. Carried in [`FindingReport::blocks`] alongside the
/// legacy `evidence` claim list; profile completeness checks the block kinds
/// present via `has_block_kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EvidenceBlock {
    Text {
        text: String,
    },
    CodeSlice {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        excerpt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<String>,
    },
    Diff {
        diff: String,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Image {
        artifact: ArtifactRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
    Hexdump {
        base: u64,
        artifact: ArtifactRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rendered: Option<String>,
    },
    Disasm {
        arch: String,
        listing: Vec<DisasmLine>,
    },
    Decompile {
        lang: String,
        listing: String,
    },
    HttpExchange {
        request: String,
        response: String,
    },
    ScanOutput {
        tool: String,
        output: String,
    },
    PcapRef {
        artifact: ArtifactRef,
        summary: String,
    },
}

impl EvidenceBlock {
    /// The serialized `kind` tag, matching a profile's `has_block_kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            EvidenceBlock::Text { .. } => "text",
            EvidenceBlock::CodeSlice { .. } => "code_slice",
            EvidenceBlock::Diff { .. } => "diff",
            EvidenceBlock::Table { .. } => "table",
            EvidenceBlock::Image { .. } => "image",
            EvidenceBlock::Hexdump { .. } => "hexdump",
            EvidenceBlock::Disasm { .. } => "disasm",
            EvidenceBlock::Decompile { .. } => "decompile",
            EvidenceBlock::HttpExchange { .. } => "http_exchange",
            EvidenceBlock::ScanOutput { .. } => "scan_output",
            EvidenceBlock::PcapRef { .. } => "pcap_ref",
        }
    }

    /// The file this block points at (`image`, `hexdump`, `pcap_ref`).
    pub fn artifact(&self) -> Option<&ArtifactRef> {
        match self {
            EvidenceBlock::Image { artifact, .. }
            | EvidenceBlock::Hexdump { artifact, .. }
            | EvidenceBlock::PcapRef { artifact, .. } => Some(artifact),
            // Listed, not `_`: a new file-bearing kind must fail to compile
            // here rather than silently skip verification.
            EvidenceBlock::Text { .. }
            | EvidenceBlock::CodeSlice { .. }
            | EvidenceBlock::Diff { .. }
            | EvidenceBlock::Table { .. }
            | EvidenceBlock::Disasm { .. }
            | EvidenceBlock::Decompile { .. }
            | EvidenceBlock::HttpExchange { .. }
            | EvidenceBlock::ScanOutput { .. } => None,
        }
    }

    pub fn artifact_mut(&mut self) -> Option<&mut ArtifactRef> {
        match self {
            EvidenceBlock::Image { artifact, .. }
            | EvidenceBlock::Hexdump { artifact, .. }
            | EvidenceBlock::PcapRef { artifact, .. } => Some(artifact),
            // Listed, not `_`: a new file-bearing kind must fail to compile
            // here rather than silently skip verification.
            EvidenceBlock::Text { .. }
            | EvidenceBlock::CodeSlice { .. }
            | EvidenceBlock::Diff { .. }
            | EvidenceBlock::Table { .. }
            | EvidenceBlock::Disasm { .. }
            | EvidenceBlock::Decompile { .. }
            | EvidenceBlock::HttpExchange { .. }
            | EvidenceBlock::ScanOutput { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    /// Unified diff, or a `binary@VA` pseudo-diff for binary-only targets.
    pub diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiDetection {
    /// Where in the pipeline it runs (pre-merge, nightly, release gate).
    pub stage: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub expect: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegressionTest {
    pub body: String,
    pub command: String,
    pub expect_vulnerable: String,
    pub expect_patched: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relation {
    Duplicate,
    Sibling,
    Prerequisite,
    Supersedes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossRef {
    pub finding_id: String,
    pub relation: Relation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Text,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactStorage {
    /// Copied into the content-addressed store; available forever.
    Copied,
    /// Over the size cap (or on a remote host): recorded by path + hash only.
    External,
}

/// An artifact. The agent supplies only `path`; rupu fills in the rest at
/// write time and overwrites anything the agent put there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArtifactKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<ArtifactStorage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerificationStatus {
    Unverified,
    Confirmed,
    Disputed,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub status: VerificationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingReport {
    pub title: String,
    pub ownership: Ownership,
    /// `None Provided` / `Unknown`, or the known tickets.
    pub tickets: OrSentinel<Vec<Ticket>>,
    pub rating: Rating,
    pub category: String,
    pub attack_vector: String,
    #[serde(default)]
    pub cwe: Vec<String>,
    pub description: String,
    pub impact: String,
    pub location: ReportLocation,
    pub root_cause: String,
    pub call_chain: OrSentinel<Vec<ChainHop>>,
    pub evidence: Vec<EvidenceClaim>,
    pub remediation: String,
    pub recommended_patch: OrSentinel<Patch>,
    pub ci_cd_detection: OrSentinel<CiDetection>,
    pub regression_test: OrSentinel<RegressionTest>,
    pub replication_steps: Vec<String>,
    /// `None`, or related findings by `fnd_` id.
    pub cross_references: OrSentinel<Vec<CrossRef>>,
    pub references: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    /// Typed evidence blocks (engagement profiles). Additive to `evidence`:
    /// code findings keep using `evidence`; binary/network/web findings attach
    /// `disasm`/`scan_output`/`http_exchange`/… blocks here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<EvidenceBlock>,
    /// Classifications across taxonomies (engagement profiles). Additive to
    /// `cwe`: legacy `cwe` entries fold in as `CWE` via [`FindingReport::all_classifications`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classifications: Vec<Classification>,
}

impl FindingReport {
    /// Every classification, folding legacy `cwe` entries in as `CWE`. Profile
    /// `has_classification_system` evaluates against this.
    pub fn all_classifications(&self) -> Vec<Classification> {
        let mut out: Vec<Classification> = self
            .cwe
            .iter()
            .filter(|c| !c.trim().is_empty())
            .map(|c| Classification {
                system: "CWE".to_string(),
                id: c.clone(),
                vector: None,
            })
            .collect();
        out.extend(self.classifications.iter().cloned());
        out
    }

    /// The evidence-block kinds present, folding a legacy `evidence` claim with
    /// an `excerpt` in as a `code_slice`. Profile `has_block_kind` evaluates
    /// against this.
    pub fn block_kinds(&self) -> std::collections::BTreeSet<&str> {
        let mut kinds: std::collections::BTreeSet<&str> =
            self.blocks.iter().map(|b| b.kind()).collect();
        if self.evidence.iter().any(|e| e.excerpt.is_some()) {
            kinds.insert("code_slice");
        }
        kinds
    }

    /// Every file this report references: `artifacts`, then the files its
    /// evidence blocks point at. Consumers that mean "any referenced file"
    /// (serving, remote marking, bucket upload) use this, never `artifacts`.
    pub fn artifact_refs(&self) -> impl Iterator<Item = &ArtifactRef> {
        self.artifacts
            .iter()
            .chain(self.blocks.iter().filter_map(EvidenceBlock::artifact))
    }

    pub fn artifact_refs_mut(&mut self) -> impl Iterator<Item = &mut ArtifactRef> {
        self.artifacts.iter_mut().chain(
            self.blocks
                .iter_mut()
                .filter_map(EvidenceBlock::artifact_mut),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    fn parse_error(v: serde_json::Value) -> String {
        serde_path_to_error::deserialize::<_, FindingReport>(v)
            .unwrap_err()
            .to_string()
    }

    fn report_fixture() -> FindingReport {
        serde_json::from_value(fixture()).unwrap()
    }

    #[test]
    fn verification_round_trips_by_agent() {
        let v = Verification {
            status: VerificationStatus::Confirmed,
            by_run: Some("run_B".into()),
            by_agent: Some("exploit-verifier".into()),
            notes: None,
        };
        let j = serde_json::to_string(&v).unwrap();
        assert!(j.contains("\"by_agent\":\"exploit-verifier\""));
        let back: Verification = serde_json::from_str(&j).unwrap();
        assert_eq!(back.by_agent.as_deref(), Some("exploit-verifier"));
        // omitted when None
        let v2 = Verification {
            status: VerificationStatus::Confirmed,
            by_run: None,
            by_agent: None,
            notes: None,
        };
        assert!(!serde_json::to_string(&v2).unwrap().contains("by_agent"));
    }

    #[test]
    fn all_classifications_folds_legacy_cwe() {
        let mut r = report_fixture();
        r.cwe = vec!["CWE-306".into()];
        r.classifications = vec![Classification {
            system: "CVE".into(),
            id: "CVE-2024-1".into(),
            vector: None,
        }];
        let all = r.all_classifications();
        assert!(all.iter().any(|c| c.system == "CWE" && c.id == "CWE-306"));
        assert!(all.iter().any(|c| c.system == "CVE"));
    }

    #[test]
    fn block_kinds_includes_typed_blocks_and_legacy_code_slice() {
        let mut r = report_fixture();
        r.blocks = vec![EvidenceBlock::ScanOutput {
            tool: "nmap".into(),
            output: "open".into(),
        }];
        // the fixture's evidence carries an excerpt → folds in as code_slice
        let kinds = r.block_kinds();
        assert!(kinds.contains("scan_output"));
        assert!(kinds.contains("code_slice"));
    }

    fn art(path: &str) -> ArtifactRef {
        ArtifactRef {
            path: path.into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }
    }

    #[test]
    fn artifact_refs_lists_artifacts_then_block_artifacts() {
        let mut r = report_fixture();
        r.artifacts = vec![art("a.bin")];
        r.blocks = vec![
            EvidenceBlock::Text { text: "t".into() },
            EvidenceBlock::Image {
                artifact: art("x.png"),
                caption: None,
            },
            EvidenceBlock::PcapRef {
                artifact: art("c.pcap"),
                summary: "s".into(),
            },
        ];
        let paths: Vec<&str> = r.artifact_refs().map(|a| a.path.as_str()).collect();
        assert_eq!(paths, ["a.bin", "x.png", "c.pcap"]);
        for a in r.artifact_refs_mut() {
            a.sha256 = "marked".into();
        }
        assert!(r.artifact_refs().all(|a| a.sha256 == "marked"));
        assert_eq!(r.artifact_refs().count(), 3);
        // The block accessors name exactly the three file-bearing kinds.
        assert!(r.blocks[0].artifact().is_none());
        assert_eq!(r.blocks[1].artifact().unwrap().path, "x.png");
        let hex = EvidenceBlock::Hexdump {
            base: 0,
            artifact: art("h.bin"),
            rendered: None,
        };
        assert_eq!(hex.artifact().unwrap().path, "h.bin");
    }

    #[test]
    fn a_nested_bad_variant_keeps_its_path_and_message() {
        let mut v = fixture();
        v["call_chain"][0]["role"] = serde_json::json!("entrypoint");
        let msg = parse_error(v);
        assert!(msg.starts_with("call_chain[0].role: "), "{msg}");
        assert!(msg.contains("unknown variant `entrypoint`"), "{msg}");
        assert!(!msg.contains("did not match any variant"), "{msg}");
    }

    #[test]
    fn a_nested_typo_keeps_its_path_and_names_the_key() {
        let mut v = fixture();
        let hop = v["call_chain"][0].as_object_mut().unwrap();
        let label = hop.remove("label").unwrap();
        hop.insert("lable".into(), label);
        let msg = parse_error(v);
        assert!(msg.starts_with("call_chain[0]"), "{msg}");
        assert!(msg.contains("lable"), "{msg}");
    }

    #[test]
    fn a_string_is_the_sentinel_and_a_value_round_trips_untagged() {
        let s: OrSentinel<Vec<Ticket>> = serde_json::from_str("\"None Provided\"").unwrap();
        assert_eq!(s, OrSentinel::Sentinel("None Provided".into()));
        let v: OrSentinel<Patch> = serde_json::from_str(r#"{"diff":"-a\n+b"}"#).unwrap();
        assert!(matches!(&v, OrSentinel::Value(p) if p.diff == "-a\n+b"));
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"{"diff":"-a\n+b"}"#);
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"None Provided\"");
        let err = serde_json::from_str::<OrSentinel<Vec<Ticket>>>("true").unwrap_err();
        assert!(
            err.to_string().contains("invalid type: boolean `true`"),
            "{err}"
        );
    }
}
