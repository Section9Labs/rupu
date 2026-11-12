//! Cross-process coordination for supported archived-data lifecycle mutations.
//!
//! The lock lives directly under RUPU_HOME, outside every prunable collection.
//! Acquire it once, before resolving archived ownership, and hold it through
//! the operation. This is NOT reentrant: a holder must not call a prune entry
//! point that acquires it again. It is an advisory lease, not a filesystem
//! transaction and not protection against old binaries or arbitrary writers.
//!
//! Existing session compaction/settings locks retain their own purpose. The
//! lifecycle lease is always outermost; it never waits for another lifecycle
//! holder, so a busy caller fails before changing archived data.
use std::{fs::File, io, path::Path};

/// Dropping the held file releases the advisory lease.
#[derive(Debug)]
pub struct RetentionLease {
    _lock: File,
    _root: File,
}

impl RetentionLease {
    /// Acquire the exact configured global root's lease without waiting.
    ///
    /// The configured root may itself be an intentional filesystem alias; it
    /// is resolved once. Every component of that canonical path and the final
    /// lock file is then opened without following symlinks. The opened file
    /// must be a single-link, empty regular file. No containing directory is
    /// created, no existing content is truncated, and `WouldBlock` is never
    /// converted into permission to continue unlocked.
    #[cfg(unix)]
    pub fn try_acquire(global: &Path) -> io::Result<Self> {
        use rustix::fs::{
            flock, fstat, openat, statat, AtFlags, FileType, FlockOperation, Mode, OFlags,
        };
        let canonical = global.canonicalize()?;
        let root = open_directory(&canonical)?;
        let opened_root = root.metadata()?;
        let fd = openat(
            &root,
            "retention.lifecycle.lock",
            OFlags::WRONLY | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )?;
        let stat = fstat(&fd)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_size != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "retention lifecycle lock must be an empty, single-link regular file",
            ));
        }
        flock(&fd, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            if error == rustix::io::Errno::WOULDBLOCK || error == rustix::io::Errno::AGAIN {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another archived-data lifecycle operation is in progress",
                )
            } else {
                error.into()
            }
        })?;
        // Refuse a lock pathname replaced while its old inode was opened.
        let named = statat(&root, "retention.lifecycle.lock", AtFlags::SYMLINK_NOFOLLOW)?;
        if named.st_dev != stat.st_dev
            || named.st_ino != stat.st_ino
            || FileType::from_raw_mode(named.st_mode) != FileType::RegularFile
            || named.st_nlink != 1
            || named.st_size != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "retention lifecycle lock changed during acquisition",
            ));
        }
        use std::os::unix::fs::MetadataExt;
        let current_root = open_directory(&canonical)?.metadata()?;
        if current_root.dev() != opened_root.dev() || current_root.ino() != opened_root.ino() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "retention lifecycle root changed during acquisition",
            ));
        }
        Ok(Self {
            _lock: File::from(fd),
            _root: root,
        })
    }

    #[cfg(not(unix))]
    pub fn try_acquire(_global: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "guarded retention lifecycle is supported only on Unix",
        ))
    }

    /// Existing CLI/library entry points preserve non-Unix behavior; the new
    /// destructive protocol must remain capability-gated off there. Unix
    /// callers get the same fail-closed lease used by the new executor.
    pub fn for_legacy(global: &Path) -> io::Result<Option<Self>> {
        #[cfg(unix)]
        {
            Self::try_acquire(global).map(Some)
        }
        #[cfg(not(unix))]
        {
            let _ = global;
            Ok(None)
        }
    }
}

#[cfg(unix)]
fn open_directory(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, openat, Mode, OFlags};
    use std::path::Component;
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "retention root must be absolute",
        ));
    }
    let flags =
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let mut dir = open("/", flags, Mode::empty())?;
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => {
                dir = openat(&dir, name, flags, Mode::empty())?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid retention root component",
                ))
            }
        }
    }
    Ok(File::from(dir))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn lease_is_exclusive_and_released_without_nested_wait() {
        let tmp = tempfile::tempdir().unwrap();
        let held = RetentionLease::try_acquire(tmp.path()).unwrap();
        assert_eq!(
            RetentionLease::try_acquire(tmp.path()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(held);
        RetentionLease::try_acquire(tmp.path()).unwrap();
    }

    #[test]
    fn lease_refuses_symlink_hardlink_directory_and_nonempty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("retention.lifecycle.lock");
        let outside = tmp.path().join("outside");
        fs::write(&outside, "preserve").unwrap();
        symlink(&outside, &lock).unwrap();
        assert!(RetentionLease::try_acquire(tmp.path()).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"preserve");
        fs::remove_file(&lock).unwrap();
        fs::write(&outside, "").unwrap();
        fs::hard_link(&outside, &lock).unwrap();
        assert!(RetentionLease::try_acquire(tmp.path()).is_err());
        fs::remove_file(&lock).unwrap();
        fs::create_dir(&lock).unwrap();
        assert!(RetentionLease::try_acquire(tmp.path()).is_err());
        fs::remove_dir(&lock).unwrap();
        fs::write(&lock, "not a lock").unwrap();
        assert!(RetentionLease::try_acquire(tmp.path()).is_err());
        assert_eq!(fs::read(&lock).unwrap(), b"not a lock");
    }

    #[test]
    fn lease_refuses_fifo_without_waiting_for_a_reader() {
        let tmp = tempfile::tempdir().unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(tmp.path().join("retention.lifecycle.lock"))
            .status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("mkfifo unavailable; skipping FIFO fixture");
            return;
        }
        let root = tmp.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(RetentionLease::try_acquire(&root).is_err());
        });
        assert!(rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("FIFO open blocked"));
    }

    #[test]
    fn independent_roots_do_not_block_each_other_and_missing_root_is_not_created() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let _one = RetentionLease::try_acquire(first.path()).unwrap();
        let _two = RetentionLease::try_acquire(second.path()).unwrap();
        let missing = first.path().join("missing");
        assert!(RetentionLease::try_acquire(&missing).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn legacy_session_dry_run_does_not_take_the_mutation_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let held = RetentionLease::try_acquire(tmp.path()).unwrap();
        let report =
            crate::sessions::prune_archived_sessions(tmp.path(), Some("0s"), true, &mut |_| {})
                .unwrap();
        assert!(report.is_empty());
        assert!(crate::sessions::prune_archived_sessions(
            tmp.path(),
            Some("0s"),
            false,
            &mut |_| {}
        )
        .is_err());
        drop(held);
        assert!(crate::sessions::prune_archived_sessions(
            tmp.path(),
            Some("0s"),
            false,
            &mut |_| {}
        )
        .unwrap()
        .is_empty());
    }

    // Invoked as a separate test process by the test below. Normal suite runs
    // have no probe root and return immediately, with no filesystem effects.
    #[test]
    fn child_process_lease_probe() {
        let Some(root) = std::env::var_os("RUPU_RETENTION_LEASE_TEST_ROOT") else {
            return;
        };
        let should_block = std::env::var_os("RUPU_RETENTION_LEASE_TEST_BLOCKED").is_some();
        let result = RetentionLease::try_acquire(Path::new(&root));
        if should_block {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
        } else {
            result.unwrap();
        }
    }

    #[test]
    fn lifecycle_lease_coordinates_an_independent_process() {
        let tmp = tempfile::tempdir().unwrap();
        let held = RetentionLease::try_acquire(tmp.path()).unwrap();
        let probe = |blocked: bool| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .arg("--exact")
                .arg("retention_lease::tests::child_process_lease_probe")
                .arg("--nocapture")
                .env("RUPU_RETENTION_LEASE_TEST_ROOT", tmp.path());
            if blocked {
                command.env("RUPU_RETENTION_LEASE_TEST_BLOCKED", "1");
            } else {
                command.env_remove("RUPU_RETENTION_LEASE_TEST_BLOCKED");
            }
            assert!(command.status().unwrap().success());
        };
        probe(true);
        drop(held);
        probe(false);
    }
}
