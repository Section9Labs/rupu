//! Immutable operator import primitives; legacy `attach_reports` is unchanged.
//!
//! The caller owns authorization, cross-workspace discovery, manifest persistence,
//! effective-option identity, aggregate batch bounds, expiry and operation status.
//! Preparation writes only a private artifact store. Commit never reopens original
//! artifact paths. A commit is one ledger replacement, not a transaction across
//! ledgers; installed content-addressed blobs can survive a failed ledger write.
//! Cooperative writers share the findings lock. Descriptor-relative replacement
//! prevents a renamed parent from redirecting writes to another directory, but
//! no claim is made of exclusion against writers that ignore that lock. A
//! rename followed by directory-sync failure returns `Unconfirmed`, retaining
//! the backup; it must never be reported as either known success or NotWritten.

use super::attach_report::{
    begin_rewrite, finding_lines, has_raw_report, keep_owner, line_text, terminator, upgraded_line,
    AttachBatch, AttachOutcome, RewriteKind,
};
use super::report_finding::{
    check_full_report, check_stored_size, resolve_block_files, ReportFindingError,
};
use crate::report::{
    is_sha256_hex, sha256_reader, ArtifactStorage, ArtifactStore, FindingProfile,
    FindingWriteOptions,
};
use crate::{CoveragePaths, FindingReport};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAX_CAPTURE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CAPTURE_FILES: usize = 100;
const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PreparedImportError {
    #[error("the reviewed findings ledger or workspace identity changed")]
    LedgerChanged,
    #[error("prepared import changed the ledger but durability is unconfirmed: {reason}")]
    Unconfirmed {
        backup: Option<std::path::PathBuf>,
        reason: String,
    },
    #[error("invalid prepared import: {0}")]
    Invalid(String),
    #[error(transparent)]
    Report(#[from] ReportFindingError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedImportBlob {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedImportReport {
    pub version: u32,
    pub report: FindingReport,
    pub blobs: Vec<PreparedImportBlob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedAttachItem {
    pub finding_id: String,
    pub prepared: PreparedImportReport,
}

/// Physical identity captured in the reviewed manifest, not supplied by UI labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedLedgerIdentity {
    pub workspace_device: u64,
    pub workspace_inode: u64,
    pub parent_device: u64,
    pub parent_inode: u64,
    pub ledger_device: u64,
    pub ledger_inode: u64,
}

fn invalid(message: &str) -> PreparedImportError {
    PreparedImportError::Invalid(message.into())
}

/// One report is capped even when the owning configuration allows more. The CP
/// must additionally account for aggregate prepared reports in its batch.
fn bounded_options(opts: &FindingWriteOptions) -> FindingWriteOptions {
    let mut value = opts.clone();
    value.artifact_max_bytes = value.artifact_max_bytes.min(MAX_CAPTURE_BYTES);
    value.artifact_total_max_bytes = value.artifact_total_max_bytes.min(MAX_CAPTURE_BYTES);
    value.artifact_max_files = value.artifact_max_files.min(MAX_CAPTURE_FILES);
    value
}

pub fn prepare_import_report(
    paths: &CoveragePaths,
    mut report: FindingReport,
    known: &[String],
    opts: &FindingWriteOptions,
    private_artifact_root: &Path,
) -> Result<PreparedImportReport, PreparedImportError> {
    if opts.artifact_root.as_ref().is_some_and(|live| {
        private_artifact_root.starts_with(live) || live.starts_with(private_artifact_root)
    }) {
        return Err(invalid(
            "review artifact store must be separate from the live store",
        ));
    }
    let opts = bounded_options(opts);
    let mut staging = None;
    check_full_report(&report, known, paths.scope_name.as_deref(), &opts)?;
    for claim in &mut report.evidence {
        claim.sha256 = None;
    }
    if report.artifact_refs().next().is_some() {
        // Require a real destination before staging a report that cannot commit.
        if opts.artifact_root.is_none() {
            return Err(invalid("no target artifact store configured"));
        }
        std::fs::create_dir_all(private_artifact_root)?;
        let private = open_no_follow(private_artifact_root)?;
        if !private.metadata()?.is_dir() {
            return Err(invalid("review store is not a directory"));
        }
        if let Some(live) = opts
            .artifact_root
            .as_ref()
            .and_then(|p| std::fs::canonicalize(p).ok())
        {
            let captured = std::fs::canonicalize(private_artifact_root)?;
            if captured.starts_with(&live) || live.starts_with(&captured) {
                return Err(invalid("review store aliases live store"));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            private.set_permissions(std::fs::Permissions::from_mode(0o700))?;
        }
        staging = Some(PrivateCapture::new(private_artifact_root)?);
        let store = ArtifactStore::new(&staging.as_ref().expect("created capture").root);
        report.artifacts = store
            .capture_import(&paths.workspace, &report.artifacts, opts.ingest_limits())
            .map_err(ReportFindingError::from)?;
        resolve_block_files(
            &paths.workspace,
            &mut report,
            opts.ingest_limits(),
            |requested, left| {
                store.capture_import(&paths.workspace, std::slice::from_ref(requested), left)
            },
        )?;
    }
    check_stored_size(&report, &opts)?;
    let blobs = manifest(&report, &opts)?;
    if let Some(staging) = staging.as_mut() {
        staging.publish(private_artifact_root, &blobs)?;
    }
    Ok(PreparedImportReport {
        version: 1,
        report,
        blobs,
    })
}

/// A failed report leaves no partial private blobs behind. Publication is only
/// after the complete report and its shared budget have passed validation.
struct PrivateCapture {
    root: std::path::PathBuf,
    published: Vec<std::path::PathBuf>,
    committed: bool,
}
impl PrivateCapture {
    fn new(parent: &Path) -> std::io::Result<Self> {
        let root = parent.join(format!(".prepare-{}", ulid::Ulid::new()));
        std::fs::create_dir(&root)?;
        private_mode(&root, true)?;
        Ok(Self {
            root,
            published: Vec::new(),
            committed: false,
        })
    }
    fn publish(
        &mut self,
        parent: &Path,
        blobs: &[PreparedImportBlob],
    ) -> Result<(), PreparedImportError> {
        let source = ArtifactStore::new(&self.root);
        let destination = ArtifactStore::new(parent);
        for blob in blobs {
            let dest = destination.blob_path(&blob.sha256);
            if dest.exists() {
                let mut file = open_no_follow(&dest)?;
                require_private_file(&file)?;
                let mut bytes = Vec::new();
                Read::by_ref(&mut file)
                    .take(blob.size + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 != blob.size || digest(&bytes)? != blob.sha256 {
                    return Err(invalid("existing private blob changed"));
                }
                continue;
            }
            let shard = dest.parent().expect("blob shard");
            if !shard.exists() {
                std::fs::create_dir(shard)?;
                private_mode(shard, true)?;
            }
            let dir = open_no_follow(shard)?;
            if !dir.metadata()?.is_dir() {
                return Err(invalid("private blob shard is not a directory"));
            }
            std::fs::rename(source.blob_path(&blob.sha256), &dest)?;
            self.published.push(dest);
        }
        self.committed = true;
        Ok(())
    }
}
impl Drop for PrivateCapture {
    fn drop(&mut self) {
        if !self.committed {
            for file in &self.published {
                let _ = std::fs::remove_file(file);
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[cfg(unix)]
fn private_mode(path: &Path, directory: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )
}
#[cfg(not(unix))]
fn private_mode(_: &Path, _: bool) -> std::io::Result<()> {
    Err(std::io::Error::other("private import requires Unix modes"))
}
fn require_private_file(file: &File) -> Result<(), PreparedImportError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("staged blob is not regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 || metadata.mode() & 0o777 != 0o600 {
            return Err(invalid("staged blob must be single-link mode 0600"));
        }
    }
    Ok(())
}

fn manifest(
    report: &FindingReport,
    opts: &FindingWriteOptions,
) -> Result<Vec<PreparedImportBlob>, PreparedImportError> {
    let mut blobs = BTreeMap::new();
    let mut count = 0usize;
    let mut total = 0u64;
    for artifact in report.artifact_refs() {
        count += 1;
        total = total
            .checked_add(artifact.size)
            .ok_or_else(|| invalid("artifact size overflow"))?;
        if artifact.stored != Some(ArtifactStorage::Copied)
            || artifact.host.is_some()
            || !is_sha256_hex(&artifact.sha256)
            || artifact.size > opts.artifact_max_bytes
            || total > opts.artifact_total_max_bytes
            || count > opts.artifact_max_files
        {
            return Err(invalid(
                "invalid, external or over-budget prepared artifact",
            ));
        }
        if let Some(prior) = blobs.insert(artifact.sha256.clone(), artifact.size) {
            if prior != artifact.size {
                return Err(invalid("one digest has conflicting sizes"));
            }
        }
    }
    Ok(blobs
        .into_iter()
        .map(|(sha256, size)| PreparedImportBlob { sha256, size })
        .collect())
}

/// A bounded exact-byte digest and physical identity for the CP's review. This
/// observation is advisory until the same check succeeds under the write lock.
pub fn capture_import_ledger(
    paths: &CoveragePaths,
) -> Result<(PreparedLedgerIdentity, String), PreparedImportError> {
    let (identity, _, mut ledger) = open_ledger(paths)?;
    let bytes = read_ledger(&mut ledger)?;
    Ok((identity, digest(&bytes)?))
}

fn digest(mut bytes: &[u8]) -> std::io::Result<String> {
    sha256_reader(&mut bytes)
}
fn read_ledger(file: &mut File) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut raw = Vec::new();
    file.take(MAX_LEDGER_BYTES + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_LEDGER_BYTES {
        return Err(std::io::Error::other("reviewed ledger exceeds 64 MiB"));
    }
    Ok(raw)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    use rustix::fs::{open, openat, Mode, OFlags};
    if !path.is_absolute() {
        return Err(std::io::Error::other("expected absolute physical path"));
    }
    let mut parent = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut parts = path
        .components()
        .filter(|c| !matches!(c, std::path::Component::RootDir))
        .peekable();
    while let Some(part) = parts.next() {
        let std::path::Component::Normal(name) = part else {
            return Err(std::io::Error::other("nonphysical path component"));
        };
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        if parts.peek().is_some() {
            flags |= OFlags::DIRECTORY;
        }
        parent = openat(&parent, name, flags, Mode::empty())?;
    }
    Ok(File::from(parent))
}
#[cfg(not(unix))]
fn open_no_follow(_: &Path) -> std::io::Result<File> {
    Err(std::io::Error::other(
        "immutable import requires no-follow file handles",
    ))
}

#[cfg(unix)]
fn open_ledger(
    paths: &CoveragePaths,
) -> Result<(PreparedLedgerIdentity, File, File), PreparedImportError> {
    use rustix::fs::{openat, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;
    if paths.findings != paths.root.join("findings.jsonl")
        || !paths
            .root
            .starts_with(paths.workspace.join(".rupu/coverage"))
    {
        return Err(invalid("ledger is outside captured coverage root"));
    }
    let workspace = open_no_follow(&paths.workspace)?;
    let parent = open_no_follow(&paths.root)?;
    let ledger = File::from(
        openat(
            &parent,
            "findings.jsonl",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    let (wm, pm, lm) = (
        workspace.metadata()?,
        parent.metadata()?,
        ledger.metadata()?,
    );
    if !wm.is_dir() || !pm.is_dir() || !lm.is_file() || lm.nlink() != 1 {
        return Err(invalid(
            "invalid workspace, ledger directory or ledger type",
        ));
    }
    Ok((
        PreparedLedgerIdentity {
            workspace_device: wm.dev(),
            workspace_inode: wm.ino(),
            parent_device: pm.dev(),
            parent_inode: pm.ino(),
            ledger_device: lm.dev(),
            ledger_inode: lm.ino(),
        },
        parent,
        ledger,
    ))
}
#[cfg(not(unix))]
fn open_ledger(
    _: &CoveragePaths,
) -> Result<(PreparedLedgerIdentity, File, File), PreparedImportError> {
    Err(invalid("immutable import requires Unix file identity"))
}

pub fn attach_prepared_reports(
    paths: &CoveragePaths,
    items: &[PreparedAttachItem],
    opts: &FindingWriteOptions,
    expected_ledger_sha256: &str,
    expected_identity: &PreparedLedgerIdentity,
    private_artifact_root: &Path,
) -> Result<AttachBatch, PreparedImportError> {
    if items.is_empty() || items.len() > 100 || !is_sha256_hex(expected_ledger_sha256) {
        return Err(invalid("invalid batch or ledger digest"));
    }
    // Refuse substituted ancestors before even creating a pathname-based lock.
    let (before, _, _) = open_ledger(paths)?;
    if &before != expected_identity {
        return Err(PreparedImportError::LedgerChanged);
    }
    let mut lock = begin_rewrite(paths, RewriteKind::Import, true);
    if let Some(error) = lock.error.take() {
        return Err(error.into());
    }
    let (identity, parent, mut ledger) = open_ledger(paths)?;
    let raw = read_ledger(&mut ledger)?;
    if &identity != expected_identity || digest(&raw)? != expected_ledger_sha256 {
        return Err(PreparedImportError::LedgerChanged);
    }
    if opts.artifact_root.as_ref().is_some_and(|live| {
        std::fs::canonicalize(live).ok() == std::fs::canonicalize(private_artifact_root).ok()
            && live.exists()
    }) {
        return Err(invalid("private store aliases live store"));
    }
    let opts = bounded_options(opts);
    let mut segments: Vec<Vec<u8>> = raw
        .split_inclusive(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    let records: HashMap<_, _> = finding_lines(&segments).collect();
    let mut indices: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, record) in &records {
        indices.entry(record.id.clone()).or_default().push(*index);
    }
    let mut seen = HashSet::new();
    let mut outcomes = Vec::new();
    let mut ready = Vec::new();
    for item in items {
        let outcome = match indices.get(&item.finding_id).map(Vec::as_slice) {
            None => Some(AttachOutcome::NotFound),
            Some([index]) if seen.insert(item.finding_id.clone()) => {
                let record = &records[index];
                let text =
                    line_text(&segments[*index]).ok_or_else(|| invalid("invalid finding line"))?;
                if record.report.is_some()
                    || record.profile == FindingProfile::Full
                    || has_raw_report(text)
                {
                    Some(AttachOutcome::AlreadyHasReport)
                } else {
                    let prepared = &item.prepared;
                    if prepared.version != 1
                        || prepared.report.evidence.iter().any(|c| c.sha256.is_some())
                        || manifest(&prepared.report, &opts)? != prepared.blobs
                    {
                        return Err(invalid(
                            "prepared report manifest or historical claim hashes changed",
                        ));
                    }
                    let known: Vec<String> = indices
                        .keys()
                        .filter(|id| *id != &item.finding_id)
                        .cloned()
                        .collect();
                    check_full_report(
                        &prepared.report,
                        &known,
                        paths.scope_name.as_deref(),
                        &opts,
                    )?;
                    check_stored_size(&prepared.report, &opts)?;
                    ready.push((*index, prepared));
                    None
                }
            }
            Some(_) => Some(AttachOutcome::Duplicate),
        };
        outcomes.push(outcome.unwrap_or(AttachOutcome::Attached));
    }
    if ready.is_empty() {
        return Ok(AttachBatch {
            outcomes,
            backup: None,
            write_error: None,
        });
    }
    // Verify/install only private captured dependencies; never reopen workspace artifacts.
    for (index, prepared) in ready {
        install_blobs(prepared, private_artifact_root, &opts)?;
        let mut text = upgraded_line(
            line_text(&segments[index]).ok_or_else(|| invalid("invalid line"))?,
            &prepared.report,
        )
        .map_err(|e| invalid(&e.to_string()))?;
        text.push_str(terminator(&segments[index]));
        segments[index] = text.into_bytes();
    }
    // Keep original bytes and perform replacement relative to the held directory.
    let result = replace_prepared(
        paths,
        &parent,
        &mut ledger,
        &raw,
        &segments,
        expected_identity,
        expected_ledger_sha256,
        &mut || {},
        &mut |directory| directory.sync_all(),
    );
    match result {
        Ok(backup) => Ok(AttachBatch {
            outcomes,
            backup: Some(backup),
            write_error: None,
        }),
        Err(error @ PreparedImportError::Unconfirmed { .. }) => Err(error),
        Err(PreparedImportError::Io(error)) => {
            for outcome in &mut outcomes {
                if matches!(outcome, AttachOutcome::Attached) {
                    *outcome = AttachOutcome::NotWritten;
                }
            }
            Ok(AttachBatch {
                outcomes,
                backup: None,
                write_error: Some(error),
            })
        }
        Err(error) => Err(error),
    }
}

fn install_blobs(
    prepared: &PreparedImportReport,
    private_root: &Path,
    opts: &FindingWriteOptions,
) -> Result<(), PreparedImportError> {
    if prepared.blobs.is_empty() {
        return Ok(());
    }
    let live = ArtifactStore::new(
        opts.artifact_root
            .as_ref()
            .ok_or_else(|| invalid("no live artifact store"))?,
    );
    let staged = ArtifactStore::new(private_root);
    for blob in &prepared.blobs {
        let mut input = open_no_follow(&staged.blob_path(&blob.sha256))?;
        require_private_file(&input)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut input)
            .take(blob.size + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != blob.size || digest(&bytes)? != blob.sha256 {
            return Err(invalid("staged artifact hash or size changed"));
        }
        // Existing live content must satisfy its address as well.
        let destination = live.blob_path(&blob.sha256);
        if destination.exists() {
            let mut existing = open_no_follow(&destination)?;
            let mut current = Vec::new();
            Read::by_ref(&mut existing)
                .take(blob.size + 1)
                .read_to_end(&mut current)?;
            if current != bytes {
                return Err(invalid(
                    "existing live artifact does not match captured bytes",
                ));
            }
            continue;
        }
        let temp = live.pull_temp_path(&blob.sha256)?;
        let result = (|| {
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
            live.install_verified(&temp, &blob.sha256, blob.size)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result?;
        // install_verified may find an existing concurrent destination; verify
        // that actual destination too before the report can reference it.
        let mut installed = open_no_follow(&destination)?;
        let mut actual = Vec::new();
        Read::by_ref(&mut installed)
            .take(blob.size + 1)
            .read_to_end(&mut actual)?;
        if actual != bytes {
            return Err(invalid("installed artifact does not match captured bytes"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn replace_prepared(
    paths: &CoveragePaths,
    parent: &File,
    ledger: &mut File,
    raw: &[u8],
    segments: &[Vec<u8>],
    expected: &PreparedLedgerIdentity,
    sha: &str,
    before_rename: &mut dyn FnMut(),
    sync_parent: &mut dyn FnMut(&File) -> std::io::Result<()>,
) -> Result<std::path::PathBuf, PreparedImportError> {
    use rustix::fs::{openat, renameat, unlinkat, AtFlags, Mode, OFlags};
    let key = ulid::Ulid::new();
    let backup = format!("findings.jsonl.pre-import-{key}");
    let temp = format!("findings.jsonl.prepared-{key}");
    let metadata = ledger.metadata()?;
    let mut created_backup = false;
    let mut created_temp = false;
    let mut rename_committed = false;
    let result = (|| {
        // A same-size write is a conflict too. Recheck both entry identity and bytes.
        let (identity, now) =
            capture_import_ledger(paths).map_err(|e| std::io::Error::other(e.to_string()))?;
        if &identity != expected || now != sha || read_ledger(ledger)? != raw {
            return Err(std::io::Error::other(
                "reviewed ledger changed before replacement",
            ));
        }
        let create = |name: &str| -> std::io::Result<File> {
            let file = File::from(openat(
                parent,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?);
            Ok(file)
        };
        let mut saved = create(&backup)?;
        created_backup = true;
        saved.set_permissions(metadata.permissions())?;
        keep_owner(&saved, &metadata, RewriteKind::Import)?;
        saved.write_all(raw)?;
        saved.sync_all()?;
        let mut staged = create(&temp)?;
        created_temp = true;
        staged.set_permissions(metadata.permissions())?;
        keep_owner(&staged, &metadata, RewriteKind::Import)?;
        for segment in segments {
            staged.write_all(segment)?;
        }
        staged.sync_all()?;
        // Repeat after staging; all effects are still in the held original parent.
        let (identity, now) =
            capture_import_ledger(paths).map_err(|e| std::io::Error::other(e.to_string()))?;
        if &identity != expected || now != sha {
            return Err(std::io::Error::other(
                "reviewed ledger changed while staging",
            ));
        }
        before_rename();
        renameat(parent, temp.as_str(), parent, "findings.jsonl")?;
        created_temp = false;
        rename_committed = true;
        sync_parent(parent)?;
        Ok(paths.root.join(&backup))
    })();
    if created_temp {
        let _ = unlinkat(parent, temp.as_str(), AtFlags::empty());
    }
    if result.is_err() && created_backup && !rename_committed {
        let _ = unlinkat(parent, backup.as_str(), AtFlags::empty());
    }
    match result {
        Ok(path) => Ok(path),
        Err(error) if rename_committed => Err(PreparedImportError::Unconfirmed {
            backup: Some(paths.root.join(&backup)),
            reason: error.to_string(),
        }),
        Err(error) => Err(PreparedImportError::Io(error)),
    }
}
#[cfg(not(unix))]
fn replace_prepared(
    _: &CoveragePaths,
    _: &File,
    _: &mut File,
    _: &[u8],
    _: &[Vec<u8>],
    _: &PreparedLedgerIdentity,
    _: &str,
    _: &mut dyn FnMut(),
    _: &mut dyn FnMut(&File) -> std::io::Result<()>,
) -> Result<std::path::PathBuf, PreparedImportError> {
    Err(PreparedImportError::Io(std::io::Error::other(
        "immutable import requires descriptor-relative rename",
    )))
}

#[cfg(all(test, unix))]
#[path = "prepared_import_tests.rs"]
mod tests;
