pub mod discover;
pub mod events;
pub mod ingest;
pub mod manifest;
pub mod paths;
pub mod stream;
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
pub use target_id::target_id;
pub use views::{file_views, read_concern_assertions, read_file_events, read_findings, FileView};
pub use writer::{CoverageWriter, CoverageWriterHandle};
