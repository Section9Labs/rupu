//! Strict, bounded discovery for reviewed report imports. Unlike the ordinary
//! inventory readers, an unreadable or ambiguous ledger aborts the whole scan.
//! All reads use held descriptors; no component of an input path may be a link.

#[cfg(test)]
use rupu_coverage::CoveragePaths;
use rupu_coverage::FindingRecord;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

pub(super) const MAX_REPORTS: usize = 100;
pub(super) const MAX_REPORT_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_REPORT_TOTAL: usize = 16 * 1024 * 1024;
const MAX_VISITS: usize = 5_000;
const MAX_DIRECTORY_DEPTH: usize = 128;
const MAX_WORKSPACES: usize = 1_000;
const MAX_TARGETS: usize = 5_000;
const MAX_REGISTRY_BYTES: usize = 32 * 1024;
const MAX_REGISTRY_TOTAL: usize = 16 * 1024 * 1024;
const MAX_LEDGER_BYTES: usize = 16 * 1024 * 1024;
const MAX_LEDGER_TOTAL: usize = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_PATH_BYTES: usize = 4_096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
    pub len: u64,
    pub mtime_sec: i64,
    pub mtime_nsec: i64,
    pub ctime_sec: i64,
    pub ctime_nsec: i64,
}
impl FileIdentity {
    #[cfg(unix)]
    fn of(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mtime_sec: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            ctime_sec: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        }
    }
    #[cfg(not(unix))]
    fn of(_metadata: &Metadata) -> Self {
        unreachable!("descriptor discovery is Unix-only")
    }
    fn physical(&self) -> (u64, u64) {
        (self.dev, self.ino)
    }
}

#[derive(Debug, Clone)]
pub(super) struct CapturedFile {
    pub original_path: PathBuf,
    pub canonical_path: PathBuf,
    pub identity: FileIdentity,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(super) struct CapturedLedger {
    pub ws_id: String,
    pub target_id: String,
    pub workspace_path: PathBuf,
    pub workspace_identity: FileIdentity,
    pub registry_record: CapturedFile,
    pub ledger: CapturedFile,
    pub records: Vec<FindingRecord>,
    /// Preserve unknown fields and reports the compatibility decoder cannot
    /// represent. Commit code must not serialize the typed records as a ledger.
    pub raw_records: Vec<serde_json::Value>,
}
impl CapturedLedger {
    #[cfg(test)]
    pub fn paths(&self) -> CoveragePaths {
        CoveragePaths::new(&self.workspace_path, &self.target_id)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(super) struct DiscoveryError {
    pub code: &'static str,
    pub path: Option<PathBuf>,
    pub message: String,
}
type Result<T> = std::result::Result<T, DiscoveryError>;
fn fail(code: &'static str, path: &Path, detail: &str) -> DiscoveryError {
    DiscoveryError {
        code,
        path: Some(path.to_path_buf()),
        message: format!("{detail}: {}", path.display()),
    }
}
fn io_failure(path: &Path, error: io::Error) -> DiscoveryError {
    // Do not include file content or provider error text in public errors.
    fail(
        "unreadable",
        path,
        &format!("Cannot safely read input ({:?})", error.kind()),
    )
}

fn check_path(path: &Path) -> Result<()> {
    let Some(text) = path.to_str() else {
        return Err(fail("invalid_path", path, "Path is not UTF-8"));
    };
    if !path.is_absolute()
        || text.len() > MAX_PATH_BYTES
        || text.contains('\0')
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(fail(
            "invalid_path",
            path,
            "A bounded absolute path without parent components is required",
        ));
    }
    Ok(())
}

/// Reusable by the review store: no symlink component, nonblocking regular-file
/// open, no hardlink, bounded held-file read and pre/post identity comparison.
pub(super) fn read_regular_no_follow(path: &Path, limit: usize) -> Result<CapturedFile> {
    check_path(path)?;
    let file = open_absolute(path, false).map_err(|error| io_failure(path, error))?;
    capture_file(file, path, limit)
}

fn capture_file(mut file: File, path: &Path, limit: usize) -> Result<CapturedFile> {
    check_path(path)?;
    let before = file.metadata().map_err(|error| io_failure(path, error))?;
    ensure_regular(&before, path)?;
    if before.len() > limit as u64 {
        return Err(fail("limit", path, "File exceeds byte limit"));
    }
    let identity = FileIdentity::of(&before);
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| io_failure(path, error))?;
    if bytes.len() > limit {
        return Err(fail("limit", path, "File exceeds byte limit"));
    }
    let after = file.metadata().map_err(|error| io_failure(path, error))?;
    ensure_regular(&after, path)?;
    if identity != FileIdentity::of(&after) || bytes.len() as u64 != identity.len {
        return Err(fail("changed", path, "File changed while being read"));
    }
    let canonical_path = verify_locator(path, &identity, false)?;
    Ok(CapturedFile {
        original_path: path.to_path_buf(),
        canonical_path,
        identity,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
    })
}

fn ensure_regular(metadata: &Metadata, path: &Path) -> Result<()> {
    if !metadata.is_file() {
        return Err(fail("not_regular", path, "Input is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(fail("alias", path, "Hard-linked inputs are not supported"));
        }
    }
    Ok(())
}

/// Canonicalization is only a label after the safe open. Both original and
/// canonical paths are independently reopened without following any component,
/// and must still identify the held object. This is not a filesystem lock.
fn verify_locator(path: &Path, identity: &FileIdentity, directory: bool) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(|error| io_failure(path, error))?;
    check_path(&canonical)?;
    for locator in [path, canonical.as_path()] {
        let file = open_absolute(locator, directory).map_err(|error| io_failure(locator, error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_failure(locator, error))?;
        if !directory {
            ensure_regular(&metadata, locator)?;
        }
        if FileIdentity::of(&metadata) != *identity {
            return Err(fail(
                "changed",
                path,
                "Input locator changed during discovery",
            ));
        }
    }
    Ok(canonical)
}

#[cfg(unix)]
fn open_absolute(path: &Path, directory: bool) -> io::Result<File> {
    use rustix::fs::{open, openat, Mode, OFlags};
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "absolute path required",
        ));
    }
    let flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY;
    let mut held = open("/", flags | OFlags::DIRECTORY, Mode::empty())?;
    let parts: Vec<_> = path
        .components()
        .filter(|part| *part != Component::RootDir)
        .collect();
    for (index, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid component",
            ));
        };
        let is_dir = directory || index + 1 < parts.len();
        held = openat(
            &held,
            *name,
            flags
                | if is_dir {
                    OFlags::DIRECTORY
                } else {
                    OFlags::empty()
                },
            Mode::empty(),
        )?;
    }
    Ok(held.into())
}
#[cfg(not(unix))]
fn open_absolute(_path: &Path, _directory: bool) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe discovery requires Unix descriptors",
    ))
}

#[cfg(unix)]
fn open_child(parent: &File, name: &std::ffi::OsStr, directory: bool) -> io::Result<File> {
    use rustix::fs::{openat, Mode, OFlags};
    let flags = OFlags::RDONLY
        | OFlags::CLOEXEC
        | OFlags::NOFOLLOW
        | OFlags::NONBLOCK
        | OFlags::NOCTTY
        | if directory {
            OFlags::DIRECTORY
        } else {
            OFlags::empty()
        };
    Ok(openat(parent, name, flags, Mode::empty())?.into())
}
#[cfg(not(unix))]
fn open_child(_parent: &File, _name: &std::ffi::OsStr, _directory: bool) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe discovery requires Unix descriptors",
    ))
}

#[derive(Clone)]
struct Entry {
    name: std::ffi::OsString,
    directory: bool,
    regular: bool,
}
#[cfg(unix)]
fn entries(
    directory: &File,
    path: &Path,
    visits: &mut usize,
    maximum: usize,
) -> Result<Vec<Entry>> {
    use rustix::fs::{AtFlags, Dir, FileType};
    use std::os::unix::ffi::OsStringExt;
    let mut found = Vec::new();
    let stream = Dir::read_from(directory).map_err(|error| io_failure(path, error.into()))?;
    for entry in stream {
        let entry = entry.map_err(|error| io_failure(path, error.into()))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        *visits += 1;
        if *visits > maximum {
            return Err(fail("limit", path, "Directory visit limit exceeded"));
        }
        let stat = rustix::fs::statat(directory, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| io_failure(path, error.into()))?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        if kind == FileType::Symlink {
            return Err(fail(
                "symlink",
                &path.join(std::ffi::OsString::from_vec(name.to_vec())),
                "Symlink entry refused",
            ));
        }
        found.push(Entry {
            name: std::ffi::OsString::from_vec(name.to_vec()),
            directory: kind == FileType::Directory,
            regular: kind == FileType::RegularFile,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}
#[cfg(not(unix))]
fn entries(
    _directory: &File,
    path: &Path,
    _visits: &mut usize,
    _maximum: usize,
) -> Result<Vec<Entry>> {
    Err(fail(
        "unsupported",
        path,
        "Safe discovery requires Unix descriptors",
    ))
}

pub(super) fn discover_reports(
    files: &[PathBuf],
    directory: Option<&Path>,
) -> Result<Vec<CapturedFile>> {
    if (files.is_empty() && directory.is_none())
        || (!files.is_empty() && directory.is_some())
        || files.len() > MAX_REPORTS
    {
        return Err(fail(
            "selection",
            Path::new(""),
            "Choose 1–100 explicit files or one directory",
        ));
    }
    let mut reports: Vec<CapturedFile> = Vec::new();
    let mut total = 0;
    let mut canonical: HashMap<PathBuf, usize> = HashMap::new();
    let mut physical = HashSet::new();
    let mut add = |held: File, path: &Path| -> Result<()> {
        check_path(path)?;
        let metadata = held.metadata().map_err(|error| io_failure(path, error))?;
        ensure_regular(&metadata, path)?;
        let identity = FileIdentity::of(&metadata);
        let locator = verify_locator(path, &identity, false)?;
        if let Some(index) = canonical.get(&locator) {
            if reports[*index].identity != identity {
                return Err(fail(
                    "changed",
                    path,
                    "Duplicate report changed during discovery",
                ));
            }
            return Ok(());
        }
        if reports.len() >= MAX_REPORTS {
            return Err(fail("limit", path, "Report count limit exceeded"));
        }
        // Check the remaining aggregate budget before allocating or reading
        // another file. Repeated canonical locators do not consume it twice.
        let file = capture_file(held, path, MAX_REPORT_BYTES.min(MAX_REPORT_TOTAL - total))?;
        if !physical.insert(file.identity.physical()) {
            return Err(fail(
                "alias",
                &file.original_path,
                "Distinct report locators share a physical file",
            ));
        }
        total += file.bytes.len();
        canonical.insert(file.canonical_path.clone(), reports.len());
        reports.push(file);
        Ok(())
    };
    if let Some(path) = directory {
        check_path(path)?;
        let held = open_absolute(path, true).map_err(|error| io_failure(path, error))?;
        walk_reports(&held, path, 0, &mut 0, &mut HashSet::new(), &mut add)?;
    } else {
        for path in files {
            check_path(path)?;
            let held = open_absolute(path, false).map_err(|error| io_failure(path, error))?;
            add(held, path)?;
        }
    }
    if reports.is_empty() {
        return Err(fail(
            "selection",
            directory.unwrap_or(Path::new("")),
            "No Markdown reports found",
        ));
    }
    Ok(reports)
}

fn walk_reports(
    directory: &File,
    path: &Path,
    depth: usize,
    visits: &mut usize,
    seen: &mut HashSet<(u64, u64)>,
    add: &mut impl FnMut(File, &Path) -> Result<()>,
) -> Result<()> {
    check_path(path)?;
    if depth > MAX_DIRECTORY_DEPTH {
        return Err(fail("limit", path, "Directory depth limit exceeded"));
    }
    let identity = FileIdentity::of(
        &directory
            .metadata()
            .map_err(|error| io_failure(path, error))?,
    );
    if !seen.insert(identity.physical()) {
        return Err(fail("alias", path, "Directory alias refused"));
    }
    for entry in entries(directory, path, visits, MAX_VISITS)? {
        let child = path.join(&entry.name);
        if entry.directory {
            let held = open_child(directory, &entry.name, true)
                .map_err(|error| io_failure(&child, error))?;
            walk_reports(&held, &child, depth + 1, visits, seen, add)?;
        } else if child
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            if !entry.regular {
                return Err(fail("not_regular", &child, "Report is not a regular file"));
            }
            let held = open_child(directory, &entry.name, false)
                .map_err(|error| io_failure(&child, error))?;
            add(held, &child)?;
        }
    }
    verify_locator(path, &identity, true)?;
    Ok(())
}

/// Complete declared-ledger scan, including every registered workspace. No
/// tolerant inventory helper is used: malformed records and aliases fail closed.
/// A genuinely missing coverage directory or findings ledger is an empty target;
/// commit must rediscover the entire inventory to detect newly added duplicates.
pub(super) fn discover_ledgers(global_dir: &Path) -> Result<Vec<CapturedLedger>> {
    check_path(global_dir)?;
    let global = open_absolute(global_dir, true).map_err(|error| io_failure(global_dir, error))?;
    let global_identity = FileIdentity::of(
        &global
            .metadata()
            .map_err(|error| io_failure(global_dir, error))?,
    );
    let registry_path = global_dir.join("workspaces");
    let Some(registry) = optional_directory(&global, Path::new("workspaces"), &registry_path)?
    else {
        verify_locator(global_dir, &global_identity, true)?;
        return Ok(Vec::new());
    };
    let registry_identity = FileIdentity::of(
        &registry
            .metadata()
            .map_err(|error| io_failure(&registry_path, error))?,
    );
    let mut visits = 0;
    let mut workspace_count = 0;
    let mut target_count = 0;
    let mut target_visits = 0;
    let mut registry_bytes = 0;
    let mut ledger_bytes = 0;
    let mut record_count = 0;
    let mut ws_ids = HashSet::new();
    let mut workspace_paths = HashSet::new();
    let mut workspace_objects = HashSet::new();
    let mut ledger_paths = HashSet::new();
    let mut ledger_objects = HashSet::new();
    let mut output = Vec::new();
    for entry in entries(&registry, &registry_path, &mut visits, MAX_VISITS)? {
        let record_path = registry_path.join(&entry.name);
        if record_path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        workspace_count += 1;
        if workspace_count > MAX_WORKSPACES {
            return Err(fail(
                "limit",
                &registry_path,
                "Workspace count limit exceeded",
            ));
        }
        let record_file = open_child(&registry, &entry.name, false)
            .map_err(|error| io_failure(&record_path, error))?;
        let record = capture_file(
            record_file,
            &record_path,
            MAX_REGISTRY_BYTES.min(MAX_REGISTRY_TOTAL - registry_bytes),
        )?;
        registry_bytes += record.bytes.len();
        if registry_bytes > MAX_REGISTRY_TOTAL {
            return Err(fail(
                "limit",
                &registry_path,
                "Registry byte limit exceeded",
            ));
        }
        let text = std::str::from_utf8(&record.bytes).map_err(|_| {
            fail(
                "invalid_registry",
                &record_path,
                "Registry record is not UTF-8",
            )
        })?;
        let workspace: rupu_workspace::Workspace = toml::from_str(text).map_err(|_| {
            fail(
                "invalid_registry",
                &record_path,
                "Malformed workspace registry record",
            )
        })?;
        if rupu_workspace::customers::validate_ws_id(&workspace.id).is_err()
            || record_path.file_stem().and_then(|stem| stem.to_str()) != Some(workspace.id.as_str())
            || !ws_ids.insert(workspace.id.clone())
        {
            return Err(fail(
                "alias",
                &record_path,
                "Workspace record identity is ambiguous",
            ));
        }
        let workspace_path = PathBuf::from(&workspace.path);
        check_path(&workspace_path)?;
        let held_workspace = open_absolute(&workspace_path, true)
            .map_err(|error| io_failure(&workspace_path, error))?;
        let workspace_identity = FileIdentity::of(
            &held_workspace
                .metadata()
                .map_err(|error| io_failure(&workspace_path, error))?,
        );
        let canonical_workspace = verify_locator(&workspace_path, &workspace_identity, true)?;
        if !workspace_paths.insert(canonical_workspace.clone())
            || !workspace_objects.insert(workspace_identity.physical())
        {
            return Err(fail(
                "alias",
                &workspace_path,
                "Multiple registrations name the same workspace",
            ));
        }
        let rupu_path = workspace_path.join(".rupu");
        if let Some(rupu) = optional_directory(&held_workspace, Path::new(".rupu"), &rupu_path)? {
            let rupu_identity = FileIdentity::of(
                &rupu
                    .metadata()
                    .map_err(|error| io_failure(&rupu_path, error))?,
            );
            let coverage_path = rupu_path.join("coverage");
            if let Some(coverage) =
                optional_directory(&rupu, Path::new("coverage"), &coverage_path)?
            {
                let coverage_identity = FileIdentity::of(
                    &coverage
                        .metadata()
                        .map_err(|error| io_failure(&coverage_path, error))?,
                );
                for target in entries(&coverage, &coverage_path, &mut target_visits, MAX_VISITS)? {
                    if !target.directory {
                        continue;
                    }
                    target_count += 1;
                    if target_count > MAX_TARGETS {
                        return Err(fail(
                            "limit",
                            &coverage_path,
                            "Coverage target limit exceeded",
                        ));
                    }
                    let target_path = coverage_path.join(&target.name);
                    let target_id = target
                        .name
                        .to_str()
                        .ok_or_else(|| {
                            fail("invalid_path", &target_path, "Target name is not UTF-8")
                        })?
                        .to_owned();
                    check_path(&target_path)?;
                    let held_target = open_child(&coverage, &target.name, true)
                        .map_err(|error| io_failure(&target_path, error))?;
                    let target_identity = FileIdentity::of(
                        &held_target
                            .metadata()
                            .map_err(|error| io_failure(&target_path, error))?,
                    );
                    let ledger_path = target_path.join("findings.jsonl");
                    let held_ledger = match open_child(
                        &held_target,
                        std::ffi::OsStr::new("findings.jsonl"),
                        false,
                    ) {
                        Ok(file) => Some(file),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                        Err(error) => return Err(io_failure(&ledger_path, error)),
                    };
                    if let Some(held_ledger) = held_ledger {
                        let ledger = capture_file(
                            held_ledger,
                            &ledger_path,
                            MAX_LEDGER_BYTES.min(MAX_LEDGER_TOTAL - ledger_bytes),
                        )?;
                        ledger_bytes += ledger.bytes.len();
                        if ledger_bytes > MAX_LEDGER_TOTAL {
                            return Err(fail(
                                "limit",
                                &ledger_path,
                                "Ledger aggregate byte limit exceeded",
                            ));
                        }
                        if !ledger_paths.insert(ledger.canonical_path.clone())
                            || !ledger_objects.insert(ledger.identity.physical())
                        {
                            return Err(fail(
                                "alias",
                                &ledger_path,
                                "Multiple targets name the same physical ledger",
                            ));
                        }
                        let (records, raw_records) = ledger_records(&ledger, &mut record_count)?;
                        output.push(CapturedLedger {
                            ws_id: workspace.id.clone(),
                            target_id,
                            workspace_path: canonical_workspace.clone(),
                            workspace_identity: workspace_identity.clone(),
                            registry_record: record.clone(),
                            ledger,
                            records,
                            raw_records,
                        });
                    }
                    verify_locator(&target_path, &target_identity, true)?;
                }
                verify_locator(&coverage_path, &coverage_identity, true)?;
            }
            verify_locator(&rupu_path, &rupu_identity, true)?;
        }
        verify_locator(&workspace_path, &workspace_identity, true)?;
        verify_locator(&record_path, &record.identity, false)?;
    }
    verify_locator(&registry_path, &registry_identity, true)?;
    Ok(output)
}

#[cfg(all(test, unix))]
#[path = "discovery_tests.rs"]
mod tests;

fn optional_directory(parent: &File, name: &Path, path: &Path) -> Result<Option<File>> {
    match open_child(parent, name.as_os_str(), true) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure(path, error)),
    }
}

fn ledger_records(
    ledger: &CapturedFile,
    total: &mut usize,
) -> Result<(Vec<FindingRecord>, Vec<serde_json::Value>)> {
    let text = std::str::from_utf8(&ledger.bytes).map_err(|_| {
        fail(
            "invalid_ledger",
            &ledger.original_path,
            "Ledger is not UTF-8",
        )
    })?;
    let mut records = Vec::new();
    let mut raw_records = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        *total += 1;
        if *total > MAX_RECORDS {
            return Err(fail(
                "limit",
                &ledger.original_path,
                "Ledger record count limit exceeded",
            ));
        }
        let raw: serde_json::Value = serde_json::from_str(line).map_err(|_| {
            fail(
                "invalid_ledger",
                &ledger.original_path,
                "Malformed ledger line",
            )
        })?;
        let record = serde_json::from_value(raw.clone()).map_err(|_| {
            fail(
                "invalid_ledger",
                &ledger.original_path,
                "Unrecognized finding record",
            )
        })?;
        records.push(record);
        raw_records.push(raw);
    }
    Ok((records, raw_records))
}
