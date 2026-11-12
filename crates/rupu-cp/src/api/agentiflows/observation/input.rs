//! Anchored, non-following file reads. Never reopen a checked path by name.
use super::*;
use serde_json::json;
use sha2::Digest;
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Component, Path},
};

const MAX_INPUT: usize = 32 * 1024 * 1024;
pub(super) const MAX_FILE: usize = 16 * 1024 * 1024;
const MAX_LINE: usize = 256 * 1024;
const MAX_ENTRIES: usize = 50_000;
const MAX_ROWS: usize = 100_000;

pub(super) struct Entry {
    pub name: OsString,
    pub is_dir: bool,
    pub is_file: bool,
}
pub(super) struct Input {
    root: fs::File,
    pub mirror: tempfile::TempDir,
    pub bytes: usize,
    entries: usize,
    rows: usize,
    pub hash: Sha256,
    pub malformed: usize,
    pub partial: usize,
}
impl Input {
    pub fn new(global: &Path, id: &str) -> ApiResult<Self> {
        let root = anchored_root(global, id)?;
        Ok(Self {
            root,
            mirror: tempfile::tempdir()
                .map_err(|_| ApiError::internal("cannot create observation capture"))?,
            bytes: 0,
            entries: 0,
            rows: 0,
            hash: Sha256::new(),
            malformed: 0,
            partial: 0,
        })
    }
    pub fn copy(&mut self, relative: &Path, limit: usize) -> ApiResult<Option<Vec<u8>>> {
        let Some(mut file) = open_relative(&self.root, relative, false)? else {
            return Ok(None);
        };
        let before = file
            .metadata()
            .map_err(|_| ApiError::internal("observation metadata unavailable"))?;
        if !before.is_file() {
            return Err(ApiError::conflict(
                "observation source is not a regular file",
            ));
        }
        let allowed = limit.min(MAX_INPUT.saturating_sub(self.bytes));
        if before.len() > allowed as u64 {
            return Err(too_large(
                "observation input exceeds its file or remaining capture budget",
            ));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(allowed as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ApiError::internal("observation input read failed"))?;
        let after = file
            .metadata()
            .map_err(|_| ApiError::internal("observation metadata unavailable"))?;
        if !same_file(&before, &after) {
            return Err(ApiError::conflict(
                "observation source changed during capture; refresh explicitly",
            ));
        }
        if bytes.len() > allowed {
            return Err(too_large("observation source grew past capture budget"));
        }
        self.bytes += bytes.len();
        self.rows += 1;
        if self.rows > MAX_ROWS {
            return Err(too_large("observation source record budget exceeded"));
        }
        self.hash.update(relative.to_string_lossy().as_bytes());
        self.hash.update(&bytes);
        let target = self.mirror.path().join(relative);
        fs::create_dir_all(target.parent().expect("capture path parent"))
            .map_err(|_| ApiError::internal("capture directory unavailable"))?;
        fs::write(target, &bytes).map_err(|_| ApiError::internal("capture write failed"))?;
        Ok(Some(bytes))
    }
    /// Read only the latest lead's first line, including at most 256KiB of
    /// actual IO. A large transcript remainder is never read or size-rejected.
    /// Appends after this complete line do not invalidate its read-time proof.
    pub fn first_line(&mut self, relative: &Path) -> ApiResult<(String, Option<Vec<u8>>)> {
        let Some(file) = open_relative(&self.root, relative, false)? else {
            return Ok(("missing".into(), None));
        };
        let before = file
            .metadata()
            .map_err(|_| ApiError::internal("lead prologue metadata unavailable"))?;
        if !before.is_file() {
            return Err(ApiError::conflict("lead prologue is not a regular file"));
        }
        let allowed = MAX_LINE.min(MAX_INPUT.saturating_sub(self.bytes));
        if allowed == 0 {
            return Err(too_large(
                "observation prologue exceeds remaining input budget",
            ));
        }
        let mut reader = BufReader::with_capacity(4096, file.take(allowed as u64));
        let mut line = Vec::new();
        reader
            .read_until(b'\n', &mut line)
            .map_err(|_| ApiError::internal("lead prologue read failed"))?;
        let limited = reader.into_inner();
        let actual = allowed - limited.limit() as usize;
        let after = limited
            .into_inner()
            .metadata()
            .map_err(|_| ApiError::internal("lead prologue metadata unavailable"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if before.dev() != after.dev() || before.ino() != after.ino() {
                return Err(ApiError::conflict(
                    "lead prologue identity changed during capture",
                ));
            }
        }
        if !after.is_file() || after.len() < line.len() as u64 {
            return Err(ApiError::conflict(
                "lead prologue was truncated during capture",
            ));
        }
        self.bytes += actual;
        self.hash.update(relative.to_string_lossy().as_bytes());
        self.hash.update(&line);
        if line.ends_with(b"\n") {
            return Ok(("complete".into(), Some(line)));
        }
        if line.len() >= MAX_LINE {
            return Ok(("too_large".into(), None));
        }
        if actual == allowed && allowed < MAX_LINE {
            return Err(too_large(
                "observation prologue exceeds remaining input budget",
            ));
        }
        Ok(("incomplete".into(), None))
    }
    pub fn jsonl(&mut self, relative: &str) -> ApiResult<Vec<Value>> {
        let Some(bytes) = self.copy(Path::new(relative), MAX_FILE)? else {
            return Ok(Vec::new());
        };
        let rows = self.lines(&bytes)?;
        let mut clean = Vec::new();
        for row in &rows {
            serde_json::to_writer(&mut clean, &row["value"])
                .map_err(|_| ApiError::internal("invalid captured line"))?;
            clean.push(b'\n');
        }
        fs::write(self.mirror.path().join(relative), clean)
            .map_err(|_| ApiError::internal("capture write failed"))?;
        Ok(rows)
    }
    pub fn entries(&mut self, relative: &Path) -> ApiResult<Vec<Entry>> {
        let Some(dir) = open_relative(&self.root, relative, true)? else {
            return Ok(Vec::new());
        };
        directory_entries(&dir, &mut self.entries)
    }
    pub fn lines(&mut self, bytes: &[u8]) -> ApiResult<Vec<Value>> {
        let mut values = Vec::new();
        for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            self.rows += 1;
            if self.rows > MAX_ROWS {
                return Err(too_large("observation source record budget exceeded"));
            }
            if line.len() > MAX_LINE {
                return Err(too_large("observation record exceeds the line budget"));
            }
            if !line.ends_with(b"\n") {
                self.partial += 1;
                continue;
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<Value>(line) {
                Ok(value) if value.is_object() => {
                    values.push(json!({"line": index + 1, "value": value}))
                }
                _ => self.malformed += 1,
            }
        }
        Ok(values)
    }
}
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return false;
        }
    }
    a.is_file() && b.is_file() && a.len() == b.len() && a.modified().ok() == b.modified().ok()
}
#[cfg(unix)]
fn anchored_root(global: &Path, id: &str) -> ApiResult<fs::File> {
    use rustix::fs::{open, openat, Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let global = open(global, flags, Mode::empty())
        .map_err(|_| ApiError::conflict("observation storage root unavailable"))?;
    let flows = openat(&global, "agentiflows", flags, Mode::empty()).map_err(anchor_error)?;
    let run = openat(&flows, id, flags, Mode::empty()).map_err(anchor_error)?;
    Ok(run.into())
}
#[cfg(unix)]
fn anchor_error(error: rustix::io::Errno) -> ApiError {
    if error == rustix::io::Errno::NOENT {
        ApiError::not_found("Agentiflow observation source unavailable")
    } else {
        ApiError::conflict("Agentiflow observation directory unavailable or symlinked")
    }
}
#[cfg(unix)]
fn open_relative(root: &fs::File, relative: &Path, directory: bool) -> ApiResult<Option<fs::File>> {
    use rustix::fs::{openat, Mode, OFlags};
    let parts: Vec<_> = relative.components().collect();
    if parts.is_empty() || parts.iter().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(ApiError::bad_request("invalid observation input path"));
    }
    let mut parent =
        rustix::io::dup(root).map_err(|_| ApiError::internal("observation anchor unavailable"))?;
    for (index, component) in parts.iter().enumerate() {
        let dir = if directory || index + 1 < parts.len() {
            OFlags::DIRECTORY
        } else {
            OFlags::empty()
        };
        parent = match openat(
            &parent,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK | dir,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(e) if e == rustix::io::Errno::NOENT => return Ok(None),
            Err(_) => {
                return Err(ApiError::conflict(
                    "observation source unavailable or symlinked",
                ))
            }
        };
    }
    Ok(Some(parent.into()))
}
#[cfg(unix)]
fn directory_entries(dir: &fs::File, count: &mut usize) -> ApiResult<Vec<Entry>> {
    use rustix::fs::{Dir, FileType};
    use std::os::unix::ffi::OsStringExt;
    let mut out = Vec::new();
    for entry in
        Dir::read_from(dir).map_err(|_| ApiError::internal("observation directory unavailable"))?
    {
        let entry = entry.map_err(|_| ApiError::internal("observation directory unreadable"))?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err(too_large("observation directory entry budget exceeded"));
        }
        // fstatat is relative to the already-open directory and does not follow
        // links. Directory enumeration never re-resolves a checked pathname.
        let stat = rustix::fs::statat(
            dir,
            entry.file_name(),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|_| ApiError::conflict("observation entry changed during capture"))?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        out.push(Entry {
            name: OsString::from_vec(bytes.to_vec()),
            is_dir: kind == FileType::Directory,
            is_file: kind == FileType::RegularFile,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
#[cfg(not(unix))]
fn anchored_root(_global: &Path, _id: &str) -> ApiResult<fs::File> {
    Err(ApiError::not_available(
        "anchored observations require a Unix host",
    ))
}
#[cfg(not(unix))]
fn open_relative(
    _root: &fs::File,
    _relative: &Path,
    _directory: bool,
) -> ApiResult<Option<fs::File>> {
    Err(ApiError::not_available(
        "anchored observations require a Unix host",
    ))
}
#[cfg(not(unix))]
fn directory_entries(_dir: &fs::File, _count: &mut usize) -> ApiResult<Vec<Entry>> {
    Err(ApiError::not_available(
        "anchored observations require a Unix host",
    ))
}
