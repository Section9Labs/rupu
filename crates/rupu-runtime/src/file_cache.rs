//! Process-wide parse cache for small record files, validated by `stat`.
//!
//! The stores behind the control plane's list views (`RunStore::list`, the
//! autoflow history) re-read thousands of small JSON files per request, and
//! almost none of them ever change again. Opening a file costs far more than
//! statting it — on a host with an endpoint-security agent, ~300µs against
//! ~4µs — so [`FileCache`] keeps each file's last parse and hands it back
//! while a fresh `stat` shows the same [`FileStamp`]. Nothing is ever served
//! without that `stat`: a file that changed, moved or vanished is re-read (or
//! reported missing), never answered from memory.
//!
//! **Racily-clean files** (git's term). Timestamps are coarse on some
//! filesystems and inodes get reused, so a file rewritten moments after it was
//! read could come back with an identical stamp. A parse is therefore only
//! cached once the file has been quiet for the cache's settle window: any
//! later write moves `ctime` — which no writer can set back, unlike `mtime` —
//! past the cached value. A file that is still being written (a live run's
//! `run.json`) is simply re-read every time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

/// How long a file must have been unchanged before its parse is cached.
/// Generous against coarse (1–2s) filesystem timestamp granularity.
pub const SETTLE: Duration = Duration::from_secs(2);

/// What a cached parse is validated against: `(len, mtime, ctime, inode)`.
/// `ctime` and the inode are unix-only (`None` / `0` elsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    mtime: Option<SystemTime>,
    ctime: Option<(i64, i64)>,
    ino: u64,
}

impl FileStamp {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            len: meta.len(),
            mtime: meta.modified().ok(),
            ctime: ctime(meta),
            ino: inode(meta),
        }
    }

    /// When the file last changed: its `ctime` on unix, else its `mtime`.
    fn changed_at(&self) -> Option<SystemTime> {
        match self.ctime {
            Some((secs, nanos)) => {
                let secs = u64::try_from(secs).ok()?;
                let nanos = u32::try_from(nanos).ok()?;
                SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs, nanos))
            }
            None => self.mtime,
        }
    }
}

#[cfg(unix)]
fn ctime(m: &std::fs::Metadata) -> Option<(i64, i64)> {
    use std::os::unix::fs::MetadataExt as _;
    Some((m.ctime(), m.ctime_nsec()))
}

#[cfg(not(unix))]
fn ctime(_: &std::fs::Metadata) -> Option<(i64, i64)> {
    None
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    m.ino()
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0
}

/// A [`FileCache::read`] failure: the file could not be read, or `parse`
/// rejected it. Neither is cached.
#[derive(Debug)]
pub enum FileCacheError<E> {
    Io(std::io::Error),
    Parse(E),
}

/// Last parse of each file, keyed by path. See the module docs.
pub struct FileCache<T> {
    entries: Mutex<HashMap<PathBuf, (FileStamp, Arc<T>)>>,
    settle: Duration,
}

impl<T> Default for FileCache<T> {
    fn default() -> Self {
        Self::with_settle(SETTLE)
    }
}

impl<T> FileCache<T> {
    /// A cache that only keeps parses of files unchanged for `settle`.
    pub fn with_settle(settle: Duration) -> Self {
        Self {
            entries: Mutex::default(),
            settle,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, (FileStamp, Arc<T>)>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `path` parsed by `parse` — the cached parse while a fresh `stat` of
    /// `path` matches the one it was read from, else a read from disk.
    /// `Ok(None)` when `path` is not a regular file (missing, a directory,
    /// or unstattable), exactly when `Path::is_file` is false.
    pub fn read<E>(
        &self,
        path: &Path,
        parse: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<Option<Arc<T>>, FileCacheError<E>> {
        let Ok(meta) = std::fs::metadata(path) else {
            return Ok(None);
        };
        if !meta.is_file() {
            return Ok(None);
        }
        let stamp = FileStamp::of(&meta);
        if let Some((cached, value)) = self.lock().get(path) {
            if *cached == stamp {
                return Ok(Some(Arc::clone(value)));
            }
        }

        // Stamp the bytes we actually read (the open handle), not the path:
        // a rename landing between the stat above and this open must leave
        // the cache keyed to the content it holds.
        use std::io::Read as _;
        let mut file = std::fs::File::open(path).map_err(FileCacheError::Io)?;
        let read_stamp = FileStamp::of(&file.metadata().map_err(FileCacheError::Io)?);
        let mut body = Vec::new();
        file.read_to_end(&mut body).map_err(FileCacheError::Io)?;
        let value = Arc::new(parse(&body).map_err(FileCacheError::Parse)?);

        let settled = read_stamp
            .changed_at()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age >= self.settle);
        let mut entries = self.lock();
        if settled {
            entries.insert(path.to_path_buf(), (read_stamp, Arc::clone(&value)));
        } else {
            entries.remove(path);
        }
        Ok(Some(value))
    }

    /// Drop every entry whose path `keep` rejects (e.g. records a listing no
    /// longer saw), so deleted files don't pin memory.
    pub fn retain(&self, mut keep: impl FnMut(&Path) -> bool) {
        self.lock().retain(|p, _| keep(p));
    }

    /// Number of cached parses.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn parse_counting<'a>(
        calls: &'a Cell<u32>,
    ) -> impl FnOnce(&[u8]) -> Result<String, std::str::Utf8Error> + 'a {
        move |b| {
            calls.set(calls.get() + 1);
            std::str::from_utf8(b).map(str::to_owned)
        }
    }

    fn write_atomic(path: &Path, body: &str) {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, body).unwrap();
        std::fs::rename(&tmp, path).unwrap();
    }

    #[test]
    fn a_settled_file_is_parsed_once() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.json");
        std::fs::write(&p, "one").unwrap();
        let cache = FileCache::with_settle(Duration::ZERO);
        let calls = Cell::new(0);

        for _ in 0..3 {
            let v = cache.read(&p, parse_counting(&calls)).unwrap().unwrap();
            assert_eq!(*v, "one");
        }
        assert_eq!(calls.get(), 1, "unchanged file must come from the cache");
    }

    #[test]
    fn a_rewrite_is_reread_even_at_the_same_length() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.json");
        std::fs::write(&p, "one").unwrap();
        let cache = FileCache::with_settle(Duration::ZERO);
        let calls = Cell::new(0);
        assert_eq!(
            *cache.read(&p, parse_counting(&calls)).unwrap().unwrap(),
            "one"
        );

        // Same length, replaced by rename (how every rupu store writes).
        write_atomic(&p, "two");
        assert_eq!(
            *cache.read(&p, parse_counting(&calls)).unwrap().unwrap(),
            "two"
        );
        // In-place overwrite, same length again.
        std::fs::write(&p, "six").unwrap();
        assert_eq!(
            *cache.read(&p, parse_counting(&calls)).unwrap().unwrap(),
            "six"
        );
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn a_rewrite_that_restores_the_old_mtime_is_still_reread() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.json");
        std::fs::write(&p, "one").unwrap();
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        let cache = FileCache::with_settle(Duration::ZERO);
        let calls = Cell::new(0);
        cache.read(&p, parse_counting(&calls)).unwrap();

        // A mirror pull that preserves the source's mtime (`rsync -t`).
        std::fs::write(&p, "two").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_eq!(
            *cache.read(&p, parse_counting(&calls)).unwrap().unwrap(),
            "two"
        );
    }

    #[test]
    fn a_fresh_file_is_never_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.json");
        std::fs::write(&p, "one").unwrap();
        let cache = FileCache::with_settle(Duration::from_secs(3600));
        let calls = Cell::new(0);
        cache.read(&p, parse_counting(&calls)).unwrap();
        cache.read(&p, parse_counting(&calls)).unwrap();
        assert_eq!(calls.get(), 2, "a racily-clean file is re-read");
        assert!(cache.is_empty());
    }

    #[test]
    fn missing_and_non_files_are_none_and_parse_errors_are_not_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let cache: FileCache<String> = FileCache::with_settle(Duration::ZERO);
        let calls = Cell::new(0);
        assert!(cache
            .read(&tmp.path().join("nope"), parse_counting(&calls))
            .unwrap()
            .is_none());
        assert!(cache
            .read(tmp.path(), parse_counting(&calls))
            .unwrap()
            .is_none());

        let p = tmp.path().join("bad.json");
        std::fs::write(&p, [0xff, 0xfe]).unwrap();
        assert!(matches!(
            cache.read(&p, parse_counting(&calls)),
            Err(FileCacheError::Parse(_))
        ));
        assert!(cache.is_empty());

        // A cached file that disappears reads as missing, not as its parse.
        let q = tmp.path().join("gone.json");
        std::fs::write(&q, "here").unwrap();
        cache.read(&q, parse_counting(&calls)).unwrap();
        std::fs::remove_file(&q).unwrap();
        assert!(cache.read(&q, parse_counting(&calls)).unwrap().is_none());
    }

    #[test]
    fn retain_drops_rejected_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = FileCache::with_settle(Duration::ZERO);
        let calls = Cell::new(0);
        for name in ["a", "b"] {
            let p = tmp.path().join(name);
            std::fs::write(&p, name).unwrap();
            cache.read(&p, parse_counting(&calls)).unwrap();
        }
        let a = tmp.path().join("a");
        cache.retain(|p| p == a);
        assert_eq!(cache.len(), 1);
    }
}
