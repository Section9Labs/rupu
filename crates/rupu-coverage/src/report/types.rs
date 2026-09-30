//! Typed finding report. Field names follow the reporting standard the spec
//! was drawn from; where that standard used prose for something with obvious
//! structure (call chain, evidence, patch, tests) this uses the structure.
//!
//! Every struct is `deny_unknown_fields`: a misspelled field is a loud parse
//! error back to the agent, never silently dropped content.

use crate::catalog::types::Severity;
use serde::{Deserialize, Serialize};

/// Prefix of the one sentinel allowed on mandatory-but-sometimes-impossible
/// sections. Must be followed by a non-empty justification.
pub const NOT_PROVIDED_PREFIX: &str = "Not Provided — ";

/// A field that is either real content or a sentinel string. Which sentinel
/// strings are acceptable is per field and enforced by
/// [`crate::report::validate_report`], not by the type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrSentinel<T> {
    Value(T),
    Sentinel(String),
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
}
