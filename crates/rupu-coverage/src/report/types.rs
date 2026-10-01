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
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Classification {
    pub system: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classifications: Vec<Classification>,
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
}

impl FindingReport {
    pub fn all_classifications(&self) -> Vec<Classification> {
        let mut out = self.classifications.clone();
        for id in &self.cwe {
            let c = Classification {
                system: "CWE".into(),
                id: id.clone(),
                vector: None,
            };
            if !out.contains(&c) {
                out.push(c);
            }
        }
        let cvss = self.rating.cvss_v3.trim();
        if !cvss.is_empty() && !cvss.eq_ignore_ascii_case("unknown") {
            let c = Classification {
                system: "CVSS".into(),
                id: cvss.to_string(),
                vector: None,
            };
            if !out.contains(&c) {
                out.push(c);
            }
        }
        out
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

    #[test]
    fn cwe_and_explicit_classifications_fold_together() {
        let mut v = fixture();
        v["cwe"] = serde_json::json!(["CWE-306"]);
        v["classifications"] = serde_json::json!([{"system":"CVE","id":"CVE-2026-0001","vector":"AV:N"}]);
        let r: FindingReport = serde_json::from_value(v).unwrap();
        let all = r.all_classifications();
        assert!(all.iter().any(|c| c.system == "CWE" && c.id == "CWE-306"));
        assert!(all.iter().any(|c| c.system == "CVE" && c.vector.as_deref() == Some("AV:N")));
    }
}
