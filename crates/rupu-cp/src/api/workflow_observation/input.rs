//! One pinned owner directory, bounded content reads and exact metadata witnesses.
use super::*;
use rustix::fs::{Mode, OFlags};
use serde::Serialize;
use sha2::Digest;
use std::{
    fs::{File, Metadata},
    io::{Read as _, Seek, SeekFrom},
    path::{Component, Path as FsPath},
};

pub(super) const MAX_INPUT: usize = 64 * 1024 * 1024;
pub(super) const MAX_FILE: usize = 16 * 1024 * 1024;
#[derive(Clone, PartialEq, Eq, Serialize)]
struct Stamp {
    device: u64,
    inode: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn of(m: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: m.dev(),
            inode: m.ino(),
            len: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}
pub(super) struct Input {
    pub dir: File,
    pub bytes: usize,
    pub hash: Sha256,
    pub window: Window,
    start: Instant,
    witnesses: BTreeMap<String, Option<Stamp>>,
}
pub(super) struct Read {
    pub bytes: Vec<u8>,
    pub read_bytes: usize,
    pub complete: bool,
    pub source_bytes: u64,
    pub start_offset: u64,
    pub head_truncated: bool,
    pub leading_skipped_bytes: usize,
}
fn io_error(e: std::io::Error) -> ApiError {
    if e.kind() == std::io::ErrorKind::NotFound {
        ApiError::not_found("recorded workflow artifact unavailable")
    } else {
        ApiError::conflict("workflow artifact is not a readable anchored regular file")
    }
}
impl Input {
    pub fn open(path: &FsPath) -> ApiResult<Self> {
        if !path.is_absolute() {
            return Err(ApiError::internal("absolute run root required"));
        }
        let flags = OFlags::RDONLY
            | OFlags::DIRECTORY
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC;
        let mut dir =
            rustix::fs::open("/", flags, Mode::empty()).map_err(|e| io_error(e.into()))?;
        for part in path.components().filter(|p| *p != Component::RootDir) {
            let Component::Normal(part) = part else {
                return Err(ApiError::conflict("invalid owner locator"));
            };
            dir = rustix::fs::openat(&dir, part, flags, Mode::empty())
                .map_err(|e| io_error(e.into()))?;
        }
        Ok(Self {
            dir: File::from(dir),
            bytes: 0,
            hash: Sha256::new(),
            window: Window::Prefix,
            start: Instant::now(),
            witnesses: BTreeMap::new(),
        })
    }
    pub fn deadline(&self) -> ApiResult<()> {
        if self.start.elapsed() > Duration::from_secs(5) {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "workflow capture work deadline exceeded".into(),
            ));
        }
        Ok(())
    }
    fn file(&mut self, name: &str) -> ApiResult<Option<(File, Stamp)>> {
        self.deadline()?;
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = match rustix::fs::openat(&self.dir, name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => {
                self.witnesses.entry(name.into()).or_insert(None);
                return Ok(None);
            }
            Err(e) => return Err(io_error(e.into())),
        };
        let file = File::from(fd);
        let m = file.metadata().map_err(io_error)?;
        if !m.is_file() {
            return Err(ApiError::conflict(
                "workflow artifact is not a regular file",
            ));
        }
        Ok(Some((file, Stamp::of(&m))))
    }
    pub fn observe(&mut self, name: &str) -> ApiResult<()> {
        if let Some((_file, stamp)) = self.file(name)? {
            self.witnesses.entry(name.into()).or_insert(Some(stamp));
        }
        self.deadline()
    }
    pub fn frontier(&self, run: &str, workspace: &str, store_kind: &str, epoch: &str) -> String {
        let bytes = serde_json::to_vec(&(run, workspace, store_kind, epoch, &self.witnesses))
            .expect("metadata witnesses");
        format!("sha256:{:x}", Sha256::digest(bytes))
    }
    pub fn read(&mut self, name: &str, max: usize, prefix: bool) -> ApiResult<Option<Read>> {
        let Some((mut file, before)) = self.file(name)? else {
            self.hash.update((name.len() as u64).to_le_bytes());
            self.hash.update(name.as_bytes());
            self.hash.update(b"missing");
            return Ok(None);
        };
        let reserve = if prefix { 512 * 1024 } else { 0 };
        let max = max.min(MAX_INPUT.saturating_sub(self.bytes).saturating_sub(reserve));
        if !prefix && before.len > max as u64 {
            return Err(too_large("workflow owner/source exceeds capture limit"));
        }
        let tail = prefix && self.window == Window::Latest && before.len > max as u64;
        let start = if tail {
            before.len.saturating_sub(max as u64)
        } else {
            0
        };
        file.seek(SeekFrom::Start(start)).map_err(io_error)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(max as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        let after = Stamp::of(&file.metadata().map_err(io_error)?);
        if before != after {
            return Err(ApiError::conflict(
                "workflow artifact changed during capture; refresh explicitly",
            ));
        }
        let read_bytes = bytes.len();
        self.bytes += read_bytes;
        self.hash.update((name.len() as u64).to_le_bytes());
        self.hash.update(name.as_bytes());
        self.hash.update(start.to_le_bytes());
        self.hash.update((bytes.len() as u64).to_le_bytes());
        self.hash.update(&bytes);
        self.witnesses
            .entry(name.into())
            .or_insert(Some(before.clone()));
        let mut start_offset = start;
        let mut leading_skipped_bytes = 0;
        if start > 0 && !bytes.is_empty() {
            // The first captured byte is a boundary probe, not an event byte.
            // A newline proves the next byte begins a record; otherwise skip
            // through the next newline without parsing a cut JSON record.
            let skip = if bytes[0] == b'\n' {
                1
            } else {
                bytes
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(bytes.len(), |n| n + 1)
            };
            leading_skipped_bytes = skip;
            start_offset += skip as u64;
            bytes.drain(..skip);
        }
        self.deadline()?;
        Ok(Some(Read {
            complete: start_offset == 0 && bytes.len() as u64 == before.len,
            source_bytes: before.len,
            bytes,
            read_bytes,
            start_offset,
            head_truncated: start > 0,
            leading_skipped_bytes,
        }))
    }
}
