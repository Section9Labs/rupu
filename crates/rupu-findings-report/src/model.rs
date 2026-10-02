use chrono::{DateTime, Utc};
use rupu_coverage::FindingRecord;

/// One finding as the caller collected it.
#[derive(Debug, Clone)]
pub struct ExportInput {
    pub ws_id: String,
    pub project: String,
    pub workflow_name: Option<String>,
    pub record: FindingRecord,
}

/// A finding with its display number (e.g. `SEC-004`).
#[derive(Debug, Clone)]
pub struct ExportFinding {
    pub number: String,
    pub input: ExportInput,
}

#[derive(Debug, Clone)]
pub struct ReportMeta {
    pub title: String,
    pub generated_at: DateTime<Utc>,
    /// Human description of what was selected, e.g. "Project notebin · severity ≥ high".
    pub scope: String,
}

/// Read access to the local finding-artifact store, handed in by the caller
/// so the renderers never touch the disk themselves. The reader is called as
/// `read(sha256, max_bytes)` and returns the blob whose contents hash to
/// `sha256`, or `None` when it is absent, unreadable, larger than
/// `max_bytes`, or does not match (`ArtifactStore::read_verified` is exactly
/// that). The renderers only ask for a well-formed sha256 of a file recorded
/// `stored: copied` with no `host`, and never fetch anything remote.
#[derive(Clone, Copy)]
pub struct Blobs<'a> {
    read: Option<&'a BlobReader<'a>>,
}

/// `read(sha256, max_bytes)`: see [`Blobs`].
pub type BlobReader<'a> = dyn Fn(&str, u64) -> Option<Vec<u8>> + 'a;

impl<'a> Blobs<'a> {
    /// No store: every evidence-block file is shown by reference only.
    pub const NONE: Blobs<'static> = Blobs { read: None };

    pub fn new(read: &'a BlobReader<'a>) -> Self {
        Blobs { read: Some(read) }
    }

    pub(crate) fn is_available(&self) -> bool {
        self.read.is_some()
    }

    pub(crate) fn read(&self, sha256: &str, max_bytes: u64) -> Option<Vec<u8>> {
        self.read.and_then(|r| r(sha256, max_bytes))
    }
}

impl std::fmt::Debug for Blobs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blobs")
            .field("available", &self.is_available())
            .finish()
    }
}
