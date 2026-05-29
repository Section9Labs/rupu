//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod catalog;
pub mod ledger;
pub mod tools;

pub use tools::{
    coverage_concerns_search, coverage_mark, coverage_remaining, coverage_status, report_finding,
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
pub use ledger::{
    file_views, read_concern_assertions, read_file_events, target_id, AssertionStatus, Attribution,
    ConcernAssertion, CoveragePaths, CoverageWriter, CoverageWriterHandle, Evidence, FileTouchEvent,
    FileView, FindingEvidence, FindingRecord, FindingScope, Surface,
};
