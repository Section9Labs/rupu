pub mod discover;
pub mod events;
pub mod ingest;
pub mod manifest;
pub mod paths;
pub mod stream;
pub mod tags;
pub mod target_id;
pub mod views;
pub mod writer;
pub use discover::{discover_targets, DiscoveredTarget};
pub use events::{
    AssertionStatus, Attribution, ConcernAssertion, Evidence, FileTouchEvent, FindingEvidence,
    FindingRecord, FindingScope, Surface,
};
pub use ingest::{ingest_unit_stream, IngestError, IngestReport, IngestSource};
pub use manifest::{append_manifest, find_manifest, read_manifests, RunManifest};
pub use paths::CoveragePaths;
pub use stream::{
    append_record, stream_catalog, stream_path, write_stream_begin, Ledger, RunStream, StreamLine,
    STREAM_FILE, STREAM_VERSION,
};
pub use tags::{
    apply, fold_tags, ingest_tag_events, parse_tags, read_declared_workspace_findings,
    read_tag_events, read_workspace_findings, tag_history, OperatorAttribution, OperatorSurface,
    Tag, TagActor, TagChange, TagError, TagEvent, TagLog, TagOp, TagOutcome, TagParseError,
    MAX_TAGS_PER_FINDING, TAG_LOG_FILE,
};
pub use target_id::target_id;
pub use views::{
    file_views, read_concern_assertions, read_declared_findings, read_file_events, read_findings,
    FileView,
};
pub use writer::{CoverageWriter, CoverageWriterHandle};
