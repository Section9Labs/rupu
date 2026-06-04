//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod audit;
pub mod catalog;
pub mod diff;
pub mod ledger;
pub mod tool_mappings;
pub mod tools;

#[cfg(feature = "gen")]
pub mod cwe_gen;

pub use tools::{
    coverage_concerns_detail, coverage_concerns_search, coverage_mark, coverage_remaining,
    coverage_status, report_finding, CoverageConcernsDetailInput, CoverageConcernsDetailOutput,
    CoverageConcernsSearchInput, CoverageMarkError, CoverageMarkInput, CoverageMarkOutput,
    CoverageRemainingInput, CoverageStatusInput, RemainingItem, ReportFindingError,
    ReportFindingInput, ReportFindingOutput, SearchResult, SearchResultForm, SearchResultSummary,
};

pub use catalog::{
    builtin_names, flatten, partition_by_mode, read_snapshot, render_full_mode, render_index_mode,
    render_prompt_section,
    resolve_builtin, resolve_modes, write_snapshot, CatalogMode, Concern, ConcernFilter,
    ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog, FlattenError, IncludeDirective,
    ParseError, Severity, SnapshotError, Template, TouchStrength, DEFAULT_FULL_MODE_THRESHOLD,
};
pub use audit::{AuditReport, ConcernCoverage, CrossModelEntry, FileCoverage, SerendipitousCluster};
pub use audit::generate::audit as run_audit;
pub use diff::generate::{list_runs, run_diff, DiffError, RunSelector};
pub use diff::{CellRef, FindingThemeRef, RunDiff, RunListEntry, VerdictFlip};
pub use tool_mappings::{load_tool_mappings, ToolMapping, ToolMappings};
pub use ledger::{
    discover_targets, file_views, read_concern_assertions, read_file_events, read_findings,
    target_id, AssertionStatus, Attribution, ConcernAssertion, CoveragePaths, CoverageWriter,
    CoverageWriterHandle, DiscoveredTarget, Evidence, FileTouchEvent, FileView, FindingEvidence,
    FindingRecord, FindingScope, Surface,
};
