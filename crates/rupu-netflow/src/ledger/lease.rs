//! Advisory per-ledger coordination. Persistent lock inodes are never removed.
//! Writers share a lease; retention takes it exclusively without waiting.
//! All ledger I/O stays relative to the pinned directory, never a later alias.
#[cfg(unix)]
use rustix::fs::{fstat, openat, statat, AtFlags, FileType, Mode, OFlags};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct LedgerLease {
    path: PathBuf,
    #[cfg(unix)]
    root: File,
    #[cfg(unix)]
    locks: File,
    #[cfg(unix)]
    lock: File,
    #[cfg(unix)]
    exclusive: bool,
}
impl LedgerLease {
    pub fn shared(path: &Path) -> io::Result<Self> {
        Self::acquire(path, false)
    }
    pub fn exclusive(path: &Path) -> io::Result<Self> {
        Self::acquire(path, true)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn root(&self) -> &Path {
        self.path.parent().expect("validated ledger parent")
    }
    fn name(&self) -> &std::ffi::OsStr {
        self.path.file_name().expect("validated ledger filename")
    }

    #[cfg(unix)]
    fn acquire(path: &Path, exclusive: bool) -> io::Result<Self> {
        use rustix::fs::{flock, mkdirat, FlockOperation};
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(invalid)?;
        if name.len() > 192
            || !name.ends_with(".jsonl")
            || name.starts_with('.')
            || !name
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"_-.".contains(&v))
        {
            return Err(invalid());
        }
        let parent = path.parent().ok_or_else(invalid)?.canonicalize()?;
        let root = directory(&parent)?;
        match mkdirat(&root, ".leases", Mode::RWXU) {
            Ok(()) => (),
            Err(e) if e == rustix::io::Errno::EXIST => (),
            Err(e) => return Err(e.into()),
        }
        let locks = File::from(openat(&root, ".leases", dir_flags(), Mode::empty())?);
        let lock_name = format!("{name}.lock");
        let lock = File::from(openat(
            &locks,
            &lock_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        regular(&lock, true)?;
        flock(
            &lock,
            if exclusive {
                FlockOperation::NonBlockingLockExclusive
            } else {
                FlockOperation::NonBlockingLockShared
            },
        )
        .map_err(|e| {
            if e == rustix::io::Errno::WOULDBLOCK || e == rustix::io::Errno::AGAIN {
                io::Error::new(io::ErrorKind::WouldBlock, "network ledger is in use")
            } else {
                e.into()
            }
        })?;
        let value = Self {
            path: parent.join(name),
            root,
            locks,
            lock,
            exclusive,
        };
        value.validate()?;
        Ok(value)
    }
    #[cfg(not(unix))]
    fn acquire(path: &Path, exclusive: bool) -> io::Result<Self> {
        if exclusive {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "guarded ledger retention requires Unix",
            ));
        }
        if path.parent().is_none() || path.file_name().is_none() {
            return Err(invalid());
        }
        Ok(Self {
            path: path.to_owned(),
        })
    }
    #[cfg(unix)]
    pub fn validate(&self) -> io::Result<()> {
        let named_root = directory(self.root())?;
        if !same(&fstat(&named_root)?, &fstat(&self.root)?) {
            return Err(changed());
        }
        let named_locks = statat(&self.root, ".leases", AtFlags::SYMLINK_NOFOLLOW)?;
        if !same(&named_locks, &fstat(&self.locks)?)
            || FileType::from_raw_mode(named_locks.st_mode) != FileType::Directory
        {
            return Err(changed());
        }
        let lock_name = format!("{}.lock", self.name().to_string_lossy());
        let named = statat(&self.locks, &lock_name, AtFlags::SYMLINK_NOFOLLOW)?;
        let held = fstat(&self.lock)?;
        regular(&self.lock, true)?;
        if !same(&named, &held)
            || named.st_nlink != 1
            || named.st_size != 0
            || FileType::from_raw_mode(named.st_mode) != FileType::RegularFile
        {
            return Err(changed());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    pub fn validate(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn read(&self) -> io::Result<File> {
        self.validate()?;
        #[cfg(unix)]
        {
            let file = File::from(openat(
                &self.root,
                self.name(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            regular(&file, false)?;
            Ok(file)
        }
        #[cfg(not(unix))]
        {
            File::open(&self.path)
        }
    }
    pub fn append(&self) -> io::Result<File> {
        self.validate()?;
        #[cfg(unix)]
        {
            let file = File::from(openat(
                &self.root,
                self.name(),
                OFlags::WRONLY
                    | OFlags::APPEND
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?);
            regular(&file, false)?;
            Ok(file)
        }
        #[cfg(not(unix))]
        {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        }
    }
    /// Replace a landed ledger using a private, newly created temporary inode.
    pub fn replace(&self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.validate()?;
        #[cfg(unix)]
        {
            let temporary = format!(".landing-{}.tmp", ulid::Ulid::new());
            let mut file = File::from(openat(
                &self.root,
                &temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?);
            let result = (|| {
                file.write_all(bytes)?;
                file.sync_all()?;
                self.validate()?;
                rustix::fs::renameat(&self.root, &temporary, &self.root, self.name())?;
                self.root.sync_all()
            })();
            if result.is_err() {
                let _ = rustix::fs::unlinkat(&self.root, &temporary, AtFlags::empty());
            }
            result
        }
        #[cfg(not(unix))]
        {
            let temporary = self
                .root()
                .join(format!(".landing-{}.tmp", ulid::Ulid::new()));
            let result = std::fs::write(&temporary, bytes)
                .and_then(|_| std::fs::rename(&temporary, &self.path));
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            result
        }
    }
    /// Caller supplies the last opened exact artifact metadata. No atomic-CAS
    /// guarantee against arbitrary writers is implied; cooperating writers are locked.
    #[cfg(unix)]
    pub fn unlink(&self, expected: &std::fs::Metadata) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        if !self.exclusive {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "exclusive ledger lease required",
            ));
        }
        self.validate()?;
        let named = statat(&self.root, self.name(), AtFlags::SYMLINK_NOFOLLOW)?;
        if named.st_dev as u64 != expected.dev()
            || named.st_ino as u64 != expected.ino()
            || named.st_nlink != 1
            || FileType::from_raw_mode(named.st_mode) != FileType::RegularFile
            || named.st_size as u64 != expected.len()
        {
            return Err(changed());
        }
        rustix::fs::unlinkat(&self.root, self.name(), AtFlags::empty())?;
        self.root.sync_all()
    }
}
#[cfg(unix)]
fn same(a: &rustix::fs::Stat, b: &rustix::fs::Stat) -> bool {
    a.st_dev == b.st_dev && a.st_ino == b.st_ino
}
#[cfg(unix)]
fn regular(file: &File, empty: bool) -> io::Result<()> {
    let s = fstat(file)?;
    if FileType::from_raw_mode(s.st_mode) != FileType::RegularFile
        || s.st_nlink != 1
        || (empty && s.st_size != 0)
    {
        return Err(invalid());
    }
    Ok(())
}
#[cfg(unix)]
fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
}
#[cfg(unix)]
fn directory(path: &Path) -> io::Result<File> {
    use std::path::Component;
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut dir = rustix::fs::open("/", dir_flags(), Mode::empty())?;
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => dir = openat(&dir, name, dir_flags(), Mode::empty())?,
            _ => return Err(invalid()),
        }
    }
    Ok(File::from(dir))
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid network ledger coordination file",
    )
}
fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "network ledger coordination identity changed",
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::unix::fs::symlink};
    #[test]
    fn shared_writers_exclude_cleanup_and_other_ledgers_remain_independent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("run_fixture.jsonl");
        let a = LedgerLease::shared(&path).unwrap();
        let b = LedgerLease::shared(&path).unwrap();
        a.append().unwrap().write_all(b"fixture\n").unwrap();
        assert_eq!(
            LedgerLease::exclusive(&path).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let _other = LedgerLease::exclusive(&temp.path().join("run_other.jsonl")).unwrap();
        drop(a);
        assert!(LedgerLease::exclusive(&path).is_err());
        drop(b);
        let held = LedgerLease::exclusive(&path).unwrap();
        assert!(LedgerLease::shared(&path).is_err());
        let meta = held.read().unwrap().metadata().unwrap();
        held.unlink(&meta).unwrap();
        assert!(!path.exists());
        assert!(temp.path().join(".leases/run_fixture.jsonl.lock").exists());
    }
    #[test]
    fn alias_and_directory_replacement_never_redirect_ledger_io() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let alias = temp.path().join("alias");
        symlink(&a, &alias).unwrap();
        let lease = LedgerLease::shared(&alias.join("run_fixture.jsonl")).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&b, &alias).unwrap();
        lease.append().unwrap().write_all(b"original only").unwrap();
        assert!(!b.join("run_fixture.jsonl").exists());
        fs::rename(&a, temp.path().join("moved")).unwrap();
        fs::create_dir(&a).unwrap();
        assert!(lease.append().is_err());
        assert!(lease.replace(b"refused").is_err());
        assert!(!a.join("run_fixture.jsonl").exists());
        assert_eq!(
            fs::read(temp.path().join("moved/run_fixture.jsonl")).unwrap(),
            b"original only"
        );
    }
    #[test]
    fn invalid_or_replaced_coordination_files_never_unlock_a_write() {
        for kind in ["symlink", "hardlink", "fifo", "nonempty", "directory"] {
            let temp = tempfile::tempdir().unwrap();
            fs::create_dir(temp.path().join(".leases")).unwrap();
            let lock = temp.path().join(".leases/run_fixture.jsonl.lock");
            let target = temp.path().join("outside");
            fs::write(&target, b"").unwrap();
            match kind {
                "symlink" => symlink(&target, &lock).unwrap(),
                "hardlink" => fs::hard_link(&target, &lock).unwrap(),
                "fifo" => assert!(std::process::Command::new("mkfifo")
                    .arg(&lock)
                    .status()
                    .unwrap()
                    .success()),
                "nonempty" => fs::write(&lock, b"preserve").unwrap(),
                _ => fs::create_dir(&lock).unwrap(),
            }
            assert!(
                LedgerLease::shared(&temp.path().join("run_fixture.jsonl")).is_err(),
                "{kind}"
            );
            assert_eq!(fs::read(target).unwrap(), b"");
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("run_fixture.jsonl");
        let held = LedgerLease::shared(&path).unwrap();
        let lock = temp.path().join(".leases/run_fixture.jsonl.lock");
        fs::rename(&lock, temp.path().join("old-lock")).unwrap();
        fs::write(&lock, b"").unwrap();
        assert!(held.append().is_err());
        assert!(!path.exists());
    }
}
