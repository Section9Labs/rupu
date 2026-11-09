//! Private, bounded operator import reviews and operation receipts.
//!
//! The lease covers the caller's entire preview/commit operation. A receipt is
//! reserved durably BEFORE ledger mutation. Reusing its ID never authorizes a
//! second execution. An error after publication is deliberately ambiguous: the
//! caller must read the existing operation, never remove it and retry.
//! Unix only; the HTTP capability must not be advertised on other platforms.
#![cfg(unix)]

use fs2::FileExt;
use rustix::fs::{AtFlags, Mode, OFlags};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) const MAX_REVIEWS: usize = 8;
pub(super) const MAX_OPERATIONS: usize = 1024;
const REVIEW_BYTES: u64 = 24 * 1024 * 1024;
const OPERATION_BYTES: u64 = 2 * 1024 * 1024;
const STAGE_BYTES: u64 = 112 * 1024 * 1024;
const MAX_STAGE_ENTRIES: usize = 4096;
const MAX_REVIEW_LIFETIME: u64 = 24 * 60 * 60;
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Review<T> {
    pub review_id: String,
    pub digest: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub payload: T,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operation<T> {
    pub operation_id: String,
    pub review_id: String,
    pub review_digest: String,
    pub created_at: u64,
    #[serde(default)]
    pub review_releasable: bool,
    pub payload: T,
}
#[derive(Debug)]
pub(super) enum Reservation<T> {
    Reserved(Operation<T>),
    Existing(Operation<T>),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    review_id: String,
    created_at: u64,
    expires_at: u64,
}

pub(super) struct Store {
    path: PathBuf,
    root: File,
    reviews: File,
    operations: File,
    // Drop releases the cooperative cross-process lease.
    _lease: File,
}

impl Store {
    /// The configured global root may itself be a supported canonical alias;
    /// all private descendants are opened descriptor-relative with NOFOLLOW.
    /// Does not read configuration or credential contents.
    pub fn open(global: &Path) -> io::Result<Self> {
        #[cfg(not(unix))]
        {
            let _ = global;
            return Err(invalid("import review store requires Unix"));
        }
        #[cfg(unix)]
        {
            let global = global.canonicalize()?;
            let owner = open_directory(&global)?;
            let root = private_directory(&owner, "finding-import-reviews")?;
            let lease = File::from(rustix::fs::openat(
                &root,
                "lease",
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            private_file(&lease)?;
            if lease.metadata()?.len() != 0 {
                return Err(invalid("invalid import review lease"));
            }
            FileExt::try_lock_exclusive(&lease)?;
            // A replaced lock inode must not create a second authority.
            same_entry(&root, "lease", &lease)?;
            lease.sync_all()?;
            root.sync_all()?;
            let reviews = private_directory(&root, "reviews")?;
            let operations = private_directory(&root, "operations")?;
            let result = Self {
                path: global.join("finding-import-reviews"),
                root,
                reviews,
                operations,
                _lease: lease,
            };
            result.check_location()?;
            Ok(result)
        }
    }

    /// Create a bounded private staging directory. Failed previews keep their
    /// creation record and can be cleaned after expiry; no source is deleted.
    pub fn create_review(&self, id: &str, expires_at: u64) -> io::Result<PathBuf> {
        self.check_location()?;
        valid_id(id)?;
        let created_at = now()?;
        if expires_at <= created_at || expires_at - created_at > MAX_REVIEW_LIFETIME {
            return Err(invalid("invalid review expiry"));
        }
        if names(&self.reviews, MAX_REVIEWS)?.len() >= MAX_REVIEWS {
            return Err(capacity(
                "import review capacity reached; expired unreferenced reviews must be cleaned",
            ));
        }
        rustix::fs::mkdirat(&self.reviews, id, Mode::from_bits_truncate(0o700))?;
        self.reviews.sync_all()?;
        let dir = open_private_directory(&self.reviews, id)?;
        let stage = Stage {
            review_id: id.into(),
            created_at,
            expires_at,
        };
        publish(
            &dir,
            "stage.json",
            &stage,
            OPERATION_BYTES,
            false,
            &mut |_| Ok(()),
        )?;
        private_directory(&dir, "artifacts")?;
        private_directory(&dir, "inputs")?;
        Ok(self.path.join("reviews").join(id))
    }

    /// Snapshot original Markdown bytes; immutable even before review publication.
    /// Names are generated from bounded ordinals, never user-provided paths.
    pub fn write_review_input(
        &self,
        id: &str,
        ordinal: usize,
        bytes: &[u8],
    ) -> io::Result<PathBuf> {
        self.check_location()?;
        valid_id(id)?;
        if ordinal >= 100 || bytes.len() > 4 * 1024 * 1024 {
            return Err(capacity("report input bound exceeded"));
        }
        let dir = open_private_directory(&self.reviews, id)?;
        if exists(&dir, "review.json")? {
            return Err(invalid("review is already immutable"));
        }
        let stage: Stage = read_json(&dir, "stage.json", OPERATION_BYTES)?;
        if stage.review_id != id || stage.expires_at <= now()? {
            return Err(invalid("review is expired or foreign"));
        }
        let inputs = open_private_directory(&dir, "inputs")?;
        let mut size = bytes.len() as u64;
        for name in names(&inputs, 100)? {
            let file = File::from(rustix::fs::openat(
                &inputs,
                name.as_str(),
                READ_FLAGS,
                Mode::empty(),
            )?);
            private_file(&file)?;
            size = size
                .checked_add(file.metadata()?.len())
                .ok_or_else(|| capacity("report input overflow"))?;
        }
        if size > 16 * 1024 * 1024 {
            return Err(capacity("aggregate report input bound exceeded"));
        }
        let name = format!("{ordinal:03}.md");
        let mut file = File::from(rustix::fs::openat(
            &inputs,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        private_file(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        inputs.sync_all()?;
        Ok(self.path.join("reviews").join(id).join("inputs").join(name))
    }

    /// Immutable publication. Even a successful second call with identical
    /// bytes is rejected; a review identity always names its original content.
    pub fn save_review<T: Serialize>(&self, id: &str, digest: &str, payload: &T) -> io::Result<()> {
        self.check_location()?;
        valid_id(id)?;
        valid_digest(digest)?;
        let dir = open_private_directory(&self.reviews, id)?;
        let stage: Stage = read_json(&dir, "stage.json", OPERATION_BYTES)?;
        if stage.review_id != id || stage.expires_at <= now()? {
            return Err(invalid("review is expired or foreign"));
        }
        let mut budget = TreeBudget::default();
        inspect_tree(&dir, &mut budget, 0)?;
        // Reserve the full permitted manifest size before publication, so the
        // complete stage is bounded even if its JSON uses the entire allowance.
        if budget.bytes > STAGE_BYTES - REVIEW_BYTES {
            return Err(capacity(
                "staged content leaves no room for review manifest",
            ));
        }
        let review = Review {
            review_id: id.into(),
            digest: digest.into(),
            created_at: stage.created_at,
            expires_at: stage.expires_at,
            payload,
        };
        publish(
            &dir,
            "review.json",
            &review,
            REVIEW_BYTES,
            false,
            &mut |_| Ok(()),
        )
    }

    /// Expiry is returned, not silently deleted. The caller refuses commits
    /// after expiry; existing operation status remains readable forever.
    pub fn load_review<T: DeserializeOwned>(&self, id: &str) -> io::Result<Review<T>> {
        self.check_location()?;
        valid_id(id)?;
        let dir = open_private_directory(&self.reviews, id)?;
        let review: Review<T> = read_json(&dir, "review.json", REVIEW_BYTES)?;
        valid_digest(&review.digest)?;
        if review.review_id != id
            || review.created_at >= review.expires_at
            || review.expires_at - review.created_at > MAX_REVIEW_LIFETIME
        {
            return Err(invalid("invalid review binding"));
        }
        Ok(review)
    }

    pub fn review_path(&self, id: &str) -> io::Result<PathBuf> {
        self.check_location()?;
        valid_id(id)?;
        let _ = open_private_directory(&self.reviews, id)?;
        Ok(self.path.join("reviews").join(id))
    }

    pub fn reserve_operation<T: Serialize + DeserializeOwned>(
        &self,
        id: &str,
        review_id: &str,
        digest: &str,
        payload: T,
    ) -> io::Result<Reservation<T>> {
        self.reserve_with(id, review_id, digest, payload, &mut |_| Ok(()))
    }

    fn reserve_with<T: Serialize + DeserializeOwned>(
        &self,
        id: &str,
        review_id: &str,
        digest: &str,
        payload: T,
        hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
    ) -> io::Result<Reservation<T>> {
        self.check_location()?;
        valid_id(id)?;
        valid_id(review_id)?;
        valid_digest(digest)?;
        if let Some(existing) = self.load_operation::<T>(id)? {
            if existing.review_id != review_id || existing.review_digest != digest {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "operation ID belongs to another review",
                ));
            }
            return Ok(Reservation::Existing(existing));
        }
        // A review is a one-attempt authority. Its retained receipt is a
        // tombstone even after known-terminal snapshot storage is expired.
        for name in names(&self.operations, MAX_OPERATIONS)? {
            let existing_id = name
                .strip_suffix(".json")
                .ok_or_else(|| invalid("unknown operation file"))?;
            let existing = self
                .load_operation::<serde_json::Value>(existing_id)?
                .ok_or_else(|| invalid("operation disappeared"))?;
            if existing.review_id == review_id {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "review already belongs to another operation",
                ));
            }
        }
        let review: Review<serde_json::Value> = self.load_review(review_id)?;
        if review.digest != digest || review.expires_at <= now()? {
            return Err(invalid("review changed or expired"));
        }
        if names(&self.operations, MAX_OPERATIONS)?.len() >= MAX_OPERATIONS {
            return Err(capacity("import operation receipt capacity reached"));
        }
        let op = Operation {
            operation_id: id.into(),
            review_id: review_id.into(),
            review_digest: digest.into(),
            created_at: now()?,
            review_releasable: false,
            payload,
        };
        publish(
            &self.operations,
            &format!("{id}.json"),
            &op,
            OPERATION_BYTES,
            false,
            hook,
        )?;
        Ok(Reservation::Reserved(op))
    }

    /// No public remove/reset API. An unknown operation cannot be made new by
    /// polling, closing a sheet, or clearing a preview.
    pub fn load_operation<T: DeserializeOwned>(
        &self,
        id: &str,
    ) -> io::Result<Option<Operation<T>>> {
        self.check_location()?;
        valid_id(id)?;
        let op: Operation<T> =
            match read_json(&self.operations, &format!("{id}.json"), OPERATION_BYTES) {
                Ok(value) => value,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
            };
        valid_id(&op.review_id)?;
        valid_digest(&op.review_digest)?;
        if op.operation_id != id {
            return Err(invalid("invalid operation binding"));
        }
        Ok(Some(op))
    }

    pub fn update_operation<T: Serialize>(&self, id: &str, payload: T) -> io::Result<()> {
        self.update_inner(id, payload, false)
    }

    /// Caller certifies every ledger outcome is known and the payload retains
    /// the public reviewed manifest plus outcomes/backups. Never call this for
    /// reserved, running, partially unknown, or uncertain persistence states.
    /// Only the private input/blob snapshot may later expire; the operation's
    /// identity/digest and receipt remain as a permanent replay tombstone.
    pub fn finish_operation<T: Serialize>(&self, id: &str, payload: T) -> io::Result<()> {
        self.update_inner(id, payload, true)
    }

    fn update_inner<T: Serialize>(&self, id: &str, payload: T, finish: bool) -> io::Result<()> {
        self.check_location()?;
        valid_id(id)?;
        let old = self
            .load_operation::<serde_json::Value>(id)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "operation was not reserved"))?;
        // Final receipts cannot be revised into a new uncertain operation.
        if old.review_releasable {
            return Err(invalid("terminal operation receipt is immutable"));
        }
        let op = Operation {
            operation_id: old.operation_id,
            review_id: old.review_id,
            review_digest: old.review_digest,
            created_at: old.created_at,
            review_releasable: finish,
            payload,
        };
        publish(
            &self.operations,
            &format!("{id}.json"),
            &op,
            OPERATION_BYTES,
            true,
            &mut |_| Ok(()),
        )
    }

    /// Caller supplies wall-clock seconds for deterministic lifecycle tests.
    /// Unfinished/unknown operations pin their review. Explicitly finalized
    /// receipts retain identity/digest after only their expired snapshot is removed.
    /// Missing/malformed stage records are never guessed to be safe to delete.
    pub fn cleanup_expired_reviews(&self, at: u64) -> io::Result<usize> {
        self.check_location()?;
        let mut referenced = HashSet::new();
        for name in names(&self.operations, MAX_OPERATIONS)? {
            let id = name
                .strip_suffix(".json")
                .ok_or_else(|| invalid("unknown operation file"))?;
            let op = self
                .load_operation::<serde_json::Value>(id)?
                .ok_or_else(|| invalid("operation disappeared"))?;
            if !op.review_releasable {
                referenced.insert(op.review_id);
            }
        }
        let mut removed = 0;
        for id in names(&self.reviews, MAX_REVIEWS)? {
            valid_id(&id)?;
            if referenced.contains(&id) {
                continue;
            }
            let dir = open_private_directory(&self.reviews, &id)?;
            let stage: Stage = read_json(&dir, "stage.json", OPERATION_BYTES)?;
            if stage.review_id != id
                || stage.created_at >= stage.expires_at
                || stage.expires_at - stage.created_at > MAX_REVIEW_LIFETIME
            {
                return Err(invalid("invalid staging lifetime"));
            }
            if stage.expires_at > at {
                continue;
            }
            // Validate the complete bounded tree before deleting any entries.
            inspect_tree(&dir, &mut TreeBudget::default(), 0)?;
            remove_tree_contents(&dir)?;
            same_entry(&self.reviews, &id, &dir)?;
            rustix::fs::unlinkat(&self.reviews, id.as_str(), AtFlags::REMOVEDIR)?;
            self.reviews.sync_all()?;
            removed += 1;
        }
        Ok(removed)
    }

    fn check_location(&self) -> io::Result<()> {
        let current = open_directory(&self.path)?;
        private_dir_metadata(&current)?;
        same_file(&current, &self.root)?;
        same_entry(&self.root, "reviews", &self.reviews)?;
        same_entry(&self.root, "operations", &self.operations)?;
        same_entry(&self.root, "lease", &self._lease)
    }
}

fn now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| invalid("invalid system clock"))
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn capacity(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, message)
}
fn valid_id(id: &str) -> io::Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        Err(invalid("invalid import identity"))
    } else {
        Ok(())
    }
}
fn valid_digest(digest: &str) -> io::Result<()> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        Ok(())
    } else {
        Err(invalid("invalid review digest"))
    }
}
fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(invalid("absolute directory required"));
    }
    let mut file = File::from(rustix::fs::open("/", DIR_FLAGS, Mode::empty())?);
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => {
                file = File::from(rustix::fs::openat(&file, name, DIR_FLAGS, Mode::empty())?);
            }
            _ => return Err(invalid("normal directory components required")),
        }
    }
    Ok(file)
}
fn private_directory(parent: &File, name: &str) -> io::Result<File> {
    match rustix::fs::mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) => parent.sync_all()?,
        Err(rustix::io::Errno::EXIST) => (),
        Err(e) => return Err(e.into()),
    }
    open_private_directory(parent, name)
}
fn open_private_directory(parent: &File, name: &str) -> io::Result<File> {
    let file = File::from(rustix::fs::openat(parent, name, DIR_FLAGS, Mode::empty())?);
    private_dir_metadata(&file)?;
    Ok(file)
}
fn private_dir_metadata(file: &File) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_dir()
        || meta.mode() & 0o077 != 0
        || meta.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(invalid("review directory is not private"));
    }
    Ok(())
}
fn private_file(file: &File) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(invalid(
            "review file must be private, regular and single-link",
        ));
    }
    Ok(())
}
fn same_file(a: &File, b: &File) -> io::Result<()> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    if a.dev() == b.dev() && a.ino() == b.ino() {
        Ok(())
    } else {
        Err(invalid("review store identity changed"))
    }
}
fn same_entry(parent: &File, name: &str, expected: &File) -> io::Result<()> {
    let actual = File::from(rustix::fs::openat(parent, name, READ_FLAGS, Mode::empty())?);
    if expected.metadata()?.is_dir() {
        private_dir_metadata(&actual)?;
    } else {
        private_file(&actual)?;
    }
    same_file(&actual, expected)
}
fn names(dir: &File, max: usize) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in rustix::fs::Dir::read_from(dir)? {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        if names.len() >= max {
            return Err(capacity("review store entry limit exceeded"));
        }
        names.push(
            String::from_utf8(bytes.to_vec())
                .map_err(|_| invalid("non UTF-8 private store entry"))?,
        );
    }
    names.sort();
    Ok(names)
}
fn exists(dir: &File, name: &str) -> io::Result<bool> {
    match rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty()) {
        Ok(file) => {
            private_file(&File::from(file))?;
            Ok(true)
        }
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(e) => Err(e.into()),
    }
}
struct CappedWriter {
    bytes: Vec<u8>,
    max: u64,
}
impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.max.saturating_sub(self.bytes.len() as u64) {
            return Err(capacity("review record byte bound exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn read_json<T: DeserializeOwned>(dir: &File, name: &str, max: u64) -> io::Result<T> {
    let file = File::from(rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty())?);
    private_file(&file)?;
    if file.metadata()?.len() > max {
        return Err(capacity("review record is too large"));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(capacity("review record grew beyond limit"));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid("invalid review record"))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublishStage {
    FileSynced,
    Published,
    DirectorySynced,
}
fn publish<T: Serialize>(
    dir: &File,
    name: &str,
    value: &T,
    max: u64,
    replace: bool,
    hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
) -> io::Result<()> {
    let mut encoded = CappedWriter {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut encoded, value)
        .map_err(|_| invalid("review record is invalid or exceeds its byte bound"))?;
    let bytes = encoded.bytes;
    if replace {
        let old = File::from(rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty())?);
        private_file(&old)?;
    }
    let tmp = format!("pending-{}", ulid::Ulid::new());
    let mut file = File::from(rustix::fs::openat(
        dir,
        tmp.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    let result = (|| {
        private_file(&file)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        hook(PublishStage::FileSynced)?;
        if replace {
            rustix::fs::renameat(dir, tmp.as_str(), dir, name)?;
        } else {
            // Link publishes without replacing an existing immutable record.
            rustix::fs::linkat(dir, tmp.as_str(), dir, name, AtFlags::empty())?;
            rustix::fs::unlinkat(dir, tmp.as_str(), AtFlags::empty())?;
        }
        hook(PublishStage::Published)?;
        dir.sync_all()?;
        hook(PublishStage::DirectorySynced)
    })();
    // Never remove the published name, including errors after rename/link.
    let _ = rustix::fs::unlinkat(dir, tmp.as_str(), AtFlags::empty());
    result
}
#[derive(Default)]
struct TreeBudget {
    entries: usize,
    bytes: u64,
}
fn inspect_tree(dir: &File, budget: &mut TreeBudget, depth: usize) -> io::Result<()> {
    if depth > 16 {
        return Err(capacity("staged directory depth exceeded"));
    }
    for name in names(dir, MAX_STAGE_ENTRIES)? {
        budget.entries += 1;
        if budget.entries > MAX_STAGE_ENTRIES {
            return Err(capacity("too many staged files"));
        }
        let child = File::from(rustix::fs::openat(
            dir,
            name.as_str(),
            READ_FLAGS,
            Mode::empty(),
        )?);
        let meta = child.metadata()?;
        if meta.is_dir() {
            if meta.mode() & 0o077 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
                return Err(invalid("nonprivate staged directory"));
            }
            inspect_tree(&child, budget, depth + 1)?;
        } else {
            private_file(&child)?;
            budget.bytes = budget
                .bytes
                .checked_add(meta.len())
                .ok_or_else(|| capacity("staged size overflow"))?;
            if budget.bytes > STAGE_BYTES {
                return Err(capacity("staged review byte budget exceeded"));
            }
        }
    }
    Ok(())
}
fn remove_tree_contents(dir: &File) -> io::Result<()> {
    for name in names(dir, MAX_STAGE_ENTRIES)? {
        let child = File::from(rustix::fs::openat(
            dir,
            name.as_str(),
            READ_FLAGS,
            Mode::empty(),
        )?);
        if child.metadata()?.is_dir() {
            remove_tree_contents(&child)?;
            same_entry(dir, &name, &child)?;
            rustix::fs::unlinkat(dir, name.as_str(), AtFlags::REMOVEDIR)?;
        } else {
            private_file(&child)?;
            same_entry(dir, &name, &child)?;
            rustix::fs::unlinkat(dir, name.as_str(), AtFlags::empty())?;
        }
    }
    dir.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::os::unix::fs::{symlink, PermissionsExt};
    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn ready(store: &Store, id: &str) -> PathBuf {
        let path = store.create_review(id, now().unwrap() + 300).unwrap();
        store
            .write_review_input(id, 0, b"# captured original report")
            .unwrap();
        store
            .save_review(id, DIGEST, &json!({"targets":["ws1/target1/fnd1"]}))
            .unwrap();
        path
    }

    #[test]
    fn immutable_review_and_original_bytes_are_private_and_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let path = ready(&store, "review-1");
        let review: Review<Value> = store.load_review("review-1").unwrap();
        assert_eq!(review.digest, DIGEST);
        assert_eq!(review.payload["targets"][0], "ws1/target1/fnd1");
        assert_eq!(
            std::fs::read(path.join("inputs/000.md")).unwrap(),
            b"# captured original report"
        );
        assert_eq!(
            std::fs::metadata(path.join("review.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(store.save_review("review-1", OTHER, &json!({})).is_err());
        assert!(store.write_review_input("review-1", 1, b"late").is_err());
        assert_eq!(
            store.load_review::<Value>("review-1").unwrap().digest,
            DIGEST
        );
    }

    #[test]
    fn operation_reservation_replay_preserves_binding_and_updated_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        ready(&store, "review-1");
        assert!(matches!(
            store
                .reserve_operation("op-1", "review-1", DIGEST, json!({"status":"submitting"}))
                .unwrap(),
            Reservation::Reserved(_)
        ));
        store
            .update_operation(
                "op-1",
                json!({"status":"unknown", "completed":["ledger-a"]}),
            )
            .unwrap();
        match store
            .reserve_operation("op-1", "review-1", DIGEST, json!({"status":"new"}))
            .unwrap()
        {
            Reservation::Existing(op) => {
                assert_eq!(op.payload["status"], "unknown");
                assert_eq!(op.payload["completed"][0], "ledger-a");
                assert_eq!(op.review_digest, DIGEST);
            }
            Reservation::Reserved(_) => panic!("existing operation authorized another mutation"),
        }
        assert!(store
            .reserve_operation("op-1", "review-1", OTHER, json!({}))
            .is_err());
        assert!(store.update_operation("absent", json!({})).is_err());
        drop(store);
        let reopened = Store::open(tmp.path()).unwrap();
        assert_eq!(
            reopened
                .load_operation::<Value>("op-1")
                .unwrap()
                .unwrap()
                .payload["status"],
            "unknown"
        );
    }

    #[test]
    fn publication_faults_never_erase_a_published_reservation() {
        for stage in [
            PublishStage::FileSynced,
            PublishStage::Published,
            PublishStage::DirectorySynced,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let store = Store::open(tmp.path()).unwrap();
            ready(&store, "review-1");
            let result = store.reserve_with(
                "op-1",
                "review-1",
                DIGEST,
                json!({"status":"submitting"}),
                &mut |observed| {
                    if observed == stage {
                        Err(io::Error::other("injected publication failure"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            let captured = store.load_operation::<Value>("op-1").unwrap();
            if stage == PublishStage::FileSynced {
                assert!(captured.is_none());
            } else {
                assert_eq!(captured.unwrap().payload["status"], "submitting");
                assert!(matches!(
                    store
                        .reserve_operation("op-1", "review-1", DIGEST, json!({}))
                        .unwrap(),
                    Reservation::Existing(_)
                ));
            }
        }
    }

    #[test]
    fn update_failure_after_publication_retains_new_known_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        ready(&store, "review-1");
        store
            .reserve_operation("op-1", "review-1", DIGEST, json!({"status":"submitting"}))
            .unwrap();
        let mut op = store.load_operation::<Value>("op-1").unwrap().unwrap();
        op.payload = json!({"status":"partial","attached":["ws1/target1/fnd1"]});
        let result = publish(
            &store.operations,
            "op-1.json",
            &op,
            OPERATION_BYTES,
            true,
            &mut |stage| {
                if stage == PublishStage::Published {
                    Err(io::Error::other("directory sync failure"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(
            store
                .load_operation::<Value>("op-1")
                .unwrap()
                .unwrap()
                .payload,
            op.payload
        );
    }

    #[test]
    fn expired_unreferenced_staging_is_cleaned_but_any_operation_pins_review() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let a = ready(&store, "unused");
        let b = ready(&store, "unknown");
        store
            .reserve_operation("op-1", "unknown", DIGEST, json!({"status":"unknown"}))
            .unwrap();
        let at = now().unwrap() + 301;
        assert_eq!(store.cleanup_expired_reviews(at).unwrap(), 1);
        assert!(!a.exists());
        assert!(b.exists());
        assert_eq!(store.cleanup_expired_reviews(at).unwrap(), 0);
        assert!(store.load_operation::<Value>("op-1").unwrap().is_some());
    }

    #[test]
    fn no_symlink_hardlink_or_public_store_file_is_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("unrelated");
        std::fs::write(&secret, "unchanged").unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let path = ready(&store, "review-1");
        std::fs::remove_file(path.join("review.json")).unwrap();
        symlink(&secret, path.join("review.json")).unwrap();
        assert!(store.load_review::<Value>("review-1").is_err());
        assert!(store.cleanup_expired_reviews(now().unwrap() + 301).is_err());
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "unchanged");
        std::fs::remove_file(path.join("review.json")).unwrap();
        std::fs::hard_link(&secret, path.join("review.json")).unwrap();
        assert!(store.load_review::<Value>("review-1").is_err());
        std::fs::remove_file(path.join("review.json")).unwrap();
        std::fs::write(path.join("review.json"), "{}").unwrap();
        std::fs::set_permissions(
            path.join("review.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(store.load_review::<Value>("review-1").is_err());
    }

    #[test]
    fn store_root_and_lock_replacement_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let lock = store.path.join("lease");
        std::fs::rename(&lock, store.path.join("old-lease")).unwrap();
        std::fs::write(&lock, "").unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store
            .create_review("review-1", now().unwrap() + 300)
            .is_err());
    }

    #[test]
    fn concurrent_store_lease_refuses_without_waiting_and_releases_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let first = Store::open(tmp.path()).unwrap();
        assert!(Store::open(tmp.path()).is_err());
        drop(first);
        assert!(Store::open(tmp.path()).is_ok());
    }

    #[test]
    fn explicit_bounds_and_invalid_identifiers_fail_before_publication() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        for bad in ["", "..", "../x", "a/b", "a\\b", "a\0b"] {
            assert!(store.create_review(bad, now().unwrap() + 300).is_err());
        }
        let path = store
            .create_review("review-1", now().unwrap() + 300)
            .unwrap();
        assert!(store.write_review_input("review-1", 100, b"x").is_err());
        assert!(store
            .write_review_input("review-1", 0, &vec![0; 4 * 1024 * 1024 + 1])
            .is_err());
        for n in 0..4 {
            store
                .write_review_input("review-1", n, &vec![0; 4 * 1024 * 1024])
                .unwrap();
        }
        assert!(store.write_review_input("review-1", 4, b"x").is_err());
        assert!(!path.join("inputs/004.md").exists());
        assert!(publish(
            &store.operations,
            "too-large.json",
            &"a".repeat(65),
            32,
            false,
            &mut |_| Ok(())
        )
        .is_err());
        assert!(!store.path.join("operations/too-large.json").exists());
        for n in 1..MAX_REVIEWS {
            store
                .create_review(&format!("review-{n}-other"), now().unwrap() + 300)
                .unwrap();
        }
        assert!(store
            .create_review("overflow", now().unwrap() + 300)
            .is_err());
    }

    #[test]
    fn malformed_operation_blocks_cleanup_instead_of_losing_unknown_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let path = ready(&store, "review-1");
        let op = store.path.join("operations/op-1.json");
        std::fs::write(&op, "broken").unwrap();
        std::fs::set_permissions(&op, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store.cleanup_expired_reviews(now().unwrap() + 301).is_err());
        assert!(path.exists());
    }
    #[test]
    fn known_terminal_cleanup_releases_capacity_but_keeps_replay_tombstone() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        for n in 0..MAX_REVIEWS {
            let review = format!("review-{n}");
            let op = format!("op-{n}");
            ready(&store, &review);
            store
                .reserve_operation(&op, &review, DIGEST, json!({"status":"running"}))
                .unwrap();
            store
                .finish_operation(
                    &op,
                    json!({"status":"completed", "reviewed_targets":["ws/target/finding"]}),
                )
                .unwrap();
            assert!(store
                .update_operation(&op, json!({"status":"unknown"}))
                .is_err());
        }
        assert!(store
            .create_review("overflow", now().unwrap() + 300)
            .is_err());
        assert_eq!(
            store.cleanup_expired_reviews(now().unwrap() + 301).unwrap(),
            MAX_REVIEWS
        );
        assert!(store.create_review("next", now().unwrap() + 300).is_ok());
        let existing: Reservation<Value> = store
            .reserve_operation("op-0", "review-0", DIGEST, json!({}))
            .unwrap();
        assert!(matches!(existing, Reservation::Existing(_)));
        assert!(store
            .reserve_operation("different-op", "review-0", DIGEST, json!({}))
            .is_err());
        assert!(
            store
                .load_operation::<Value>("op-0")
                .unwrap()
                .unwrap()
                .review_releasable
        );
    }
    #[test]
    fn lease_child_probe() {
        let Some(path) = std::env::var_os("RUPU_IMPORT_REVIEW_LEASE_TEST_ROOT") else {
            return;
        };
        assert!(
            Store::open(Path::new(&path)).is_err(),
            "independent process acquired an already-held lease"
        );
    }

    #[test]
    fn lease_excludes_an_independent_process() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "api::findings_import_review::store::tests::lease_child_probe",
                "--nocapture",
            ])
            .env("RUPU_IMPORT_REVIEW_LEASE_TEST_ROOT", tmp.path())
            .status()
            .unwrap();
        assert!(status.success());
        drop(store);
        assert!(Store::open(tmp.path()).is_ok());
    }
    #[test]
    fn changed_private_directory_permissions_refuse_subsequent_access() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        ready(&store, "review-1");
        std::fs::set_permissions(
            store.path.join("reviews"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(store.load_review::<Value>("review-1").is_err());
        assert!(store
            .reserve_operation("op-1", "review-1", DIGEST, json!({}))
            .is_err());
    }
}
