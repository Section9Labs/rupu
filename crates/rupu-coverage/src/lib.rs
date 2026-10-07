//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod asset;
pub mod audit;
pub mod catalog;
pub mod diff;
pub mod ledger;
pub mod profile;
pub mod report;
pub mod rerun;
pub mod tool_mappings;
pub mod tools;

#[cfg(feature = "gen")]
pub mod cwe_gen;

pub use tools::{
    asset_mark, coverage_concerns_detail, coverage_concerns_search, coverage_mark,
    coverage_remaining, coverage_status, report_finding, AssetMarkError, AssetMarkInput,
    AssetMarkOutput, AssetRef, CoverageConcernsDetailInput, CoverageConcernsDetailOutput,
    CoverageConcernsSearchInput, CoverageMarkError, CoverageMarkInput, CoverageMarkOutput,
    CoverageRemainingInput, CoverageStatusInput, RemainingItem, ReportFindingError,
    ReportFindingInput, ReportFindingOutput, SearchResult, SearchResultForm, SearchResultSummary,
};

pub use asset::{
    from_assets, profile_of, read_assets, Asset, AssetId, AssetStoreError, Coordinate, Locator,
    Proto,
};

pub use profile::{
    builtin_profiles, builtin_registry, registry_with_overlay, ActiveSet, AssetKindDef, Bundle,
    CompletenessCheck, Coverage, EngagementProfile, Predicate, ProfileRegistry, DEFAULT_PROFILE,
};

pub use audit::generate::audit as run_audit;
pub use audit::{
    AuditReport, ConcernCoverage, CrossModelEntry, FileCoverage, SerendipitousCluster,
};
pub use catalog::{
    builtin_names, flatten, partition_by_mode, read_snapshot, render_full_mode, render_index_mode,
    render_prompt_section, resolve_builtin, resolve_modes, write_snapshot, CatalogMode, Concern,
    ConcernFilter, ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog, FlattenError,
    IncludeDirective, ParseError, Severity, SnapshotError, Template, TouchStrength,
    DEFAULT_FULL_MODE_THRESHOLD,
};
pub use diff::generate::{list_runs, run_diff, DiffError, RunSelector};
pub use diff::{CellRef, FindingThemeRef, RunDiff, RunListEntry, VerdictFlip};
pub use ledger::{
    append_manifest, append_record, apply, discover_targets, file_views, find_manifest, fold_tags,
    ingest_tag_events, ingest_unit_stream, merge_tag_log_copy, parse_tags, read_concern_assertions,
    read_declared_findings, read_declared_workspace_findings, read_file_events, read_findings,
    read_manifests, read_tag_events, read_workspace_findings, stream_catalog, stream_path,
    tag_history, tag_input_schema, tags_schema_property, target_id, write_stream_begin,
    AssertionStatus, Attribution, ConcernAssertion, CoveragePaths, CoverageWriter,
    CoverageWriterHandle, DiscoveredTarget, Evidence, FileTouchEvent, FileView, FindingEvidence,
    FindingRecord, FindingScope, IngestError, IngestReport, IngestSource, Ledger,
    OperatorAttribution, OperatorSurface, RunManifest, RunStream, StreamLine, Surface, Tag,
    TagActor, TagChange, TagChangeInput, TagError, TagEvent, TagLog, TagLogMerge, TagOp,
    TagOutcome, TagParseError, MAX_TAGS_PER_FINDING, STREAM_FILE, STREAM_VERSION, TAG_LOG_FILE,
};
pub use ledger::{
    check_available, facets, run_values, select, FacetValue, FindingView, RunScopes, Unavailable,
};
pub use ledger::{
    field, parse_query, ErrorCode, FieldDef, FieldKind, Key, Op, ParseError as QueryParseError,
    ParsedQuery, Term, FIELDS, SEVERITIES,
};
pub use ledger::{
    query, query_input_schema, query_response, severity_rank, tags_in_use, FindingQuery,
    FindingRow, Page, QueryError, TagCount, DEFAULT_LIMIT, MAX_LIMIT,
};
pub use report::{
    Classification, DisasmLine, EvidenceBlock, FindingProfile, FindingReport, FindingWriteOptions,
    Verification, VerificationStatus,
};
pub use rerun::{plan_rerun, RerunError, RerunInvocation};
pub use tool_mappings::{load_tool_mappings, ToolMapping, ToolMappings};
