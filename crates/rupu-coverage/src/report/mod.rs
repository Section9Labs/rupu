//! Structured finding reports.
//!
//! A finding under the `full` profile carries a [`FindingReport`]: the whole
//! assessment write-up as typed data, and the source of truth for the
//! finding, so the agent writes it exactly once. Design:
//! docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md

pub mod artifacts;
pub mod guidance;
pub mod options;
pub mod profile;
pub mod schema;
pub mod summary;
pub mod types;
pub mod validate;

pub use artifacts::{
    is_sha256_hex, sha256_file, sha256_reader, ArtifactError, ArtifactInstallError, ArtifactStore,
    IngestLimits,
};
pub use guidance::guidance;
pub use options::{
    FindingWriteOptions, DEFAULT_ARTIFACT_MAX_BYTES, DEFAULT_ARTIFACT_MAX_FILES,
    DEFAULT_ARTIFACT_TOTAL_MAX_BYTES, DEFAULT_REPORT_MAX_BYTES,
};
pub use profile::FindingProfile;
pub use summary::{completeness, summarize, Completeness, ReportSummary};
pub use types::*;
pub use validate::{validate_report, FieldError, ReportValidationError, ValidateCtx};
