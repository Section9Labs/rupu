//! rupu coverage harness — exhaustive-coverage ledgers, concern catalogs, and agent tools.

#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod catalog;
pub mod ledger;

pub use catalog::{
    builtin_names, flatten, read_snapshot, resolve_builtin, write_snapshot, Concern, ConcernOverride,
    ConcernsBlock, ConcernsEntry, FlatCatalog, FlattenError, IncludeDirective, ParseError, Severity,
    SnapshotError, Template, TouchStrength,
};
pub use ledger::{
    file_views, read_concern_assertions, read_file_events, target_id, AssertionStatus, Attribution,
    ConcernAssertion, CoveragePaths, CoverageWriter, CoverageWriterHandle, Evidence, FileTouchEvent,
    FileView, FindingEvidence, FindingRecord, FindingScope, Surface,
};
