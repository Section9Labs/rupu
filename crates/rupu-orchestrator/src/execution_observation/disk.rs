//! Descriptor-anchored, bounded I/O. Lock errors never permit unlocked access.
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};

#[cfg(unix)]
use rustix::fs::{self, AtFlags, FileType, FlockOperation, Mode, OFlags};

#[cfg(unix)]
pub(super) fn open_run_directory(path: &Path) -> io::Result<File> {
    use std::path::Component;
    // A configured store root may be an intentional alias. The run directory
    // itself and every component after that root must not be a symlink.
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("run directory missing"))?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("run store missing"))?
        .canonicalize()?;
    let flags =
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let mut fd = fs::open("/", flags, Mode::empty())?;
    for component in parent.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(part) => fd = fs::openat(&fd, part, flags, Mode::empty())?,
            _ => return Err(io::Error::other("invalid run store path")),
        }
    }
    Ok(File::from(fs::openat(&fd, name, flags, Mode::empty())?))
}

#[cfg(unix)]
fn regular(dir: &File, name: &str, create: bool, exclusive: bool) -> io::Result<File> {
    let mut flags = OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    flags |= if create {
        OFlags::WRONLY | OFlags::CREATE
    } else {
        OFlags::RDONLY
    };
    if exclusive {
        flags |= OFlags::EXCL;
    }
    let file = File::from(fs::openat(dir, name, flags, Mode::RUSR | Mode::WUSR)?);
    verify(dir, name, &file, false)?;
    Ok(file)
}

#[cfg(unix)]
fn verify(dir: &File, name: &str, file: &File, empty: bool) -> io::Result<()> {
    let opened = fs::fstat(file)?;
    let named = fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if opened.st_dev != named.st_dev
        || opened.st_ino != named.st_ino
        || FileType::from_raw_mode(opened.st_mode) != FileType::RegularFile
        || FileType::from_raw_mode(named.st_mode) != FileType::RegularFile
        || opened.st_nlink != 1
        || named.st_nlink != 1
        || (empty && opened.st_size != 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "execution file identity invalid",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn lock(dir: &File, name: &str, create: bool) -> io::Result<File> {
    let file = regular(dir, name, create, false)?;
    verify(dir, name, &file, true)?;
    fs::flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    verify(dir, name, &file, true)?;
    Ok(file)
}

/// Producers tolerate short observation reads, but never continue unlocked.
pub(super) fn mutation_lock(dir: &File, create: bool) -> io::Result<File> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match lock(dir, super::MUTATION_LOCK_FILE, create) {
            Ok(file) => return Ok(file),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
pub(super) fn new_lock(dir: &File, name: &str) -> io::Result<File> {
    let file = regular(dir, name, true, true)?;
    fs::flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    verify(dir, name, &file, true)?;
    Ok(file)
}

#[cfg(unix)]
pub(super) fn is_locked(dir: &File, name: &str) -> io::Result<bool> {
    let file = regular(dir, name, false, false)?;
    verify(dir, name, &file, true)?;
    let locked = match fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => false,
        Err(error)
            if error == rustix::io::Errno::WOULDBLOCK || error == rustix::io::Errno::AGAIN =>
        {
            true
        }
        Err(error) => return Err(error.into()),
    };
    verify(dir, name, &file, true)?;
    Ok(locked)
}

#[cfg(unix)]
pub(super) fn exists(dir: &File, name: &str) -> io::Result<bool> {
    match fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "execution file is not regular",
                ));
            }
            Ok(true)
        }
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(unix)]
pub(super) fn marker(dir: &File, name: &str) -> io::Result<()> {
    match regular(dir, name, true, true) {
        Ok(file) => {
            file.sync_all()?;
            dir.sync_all()
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let file = regular(dir, name, false, false)?;
            verify(dir, name, &file, true)
        }
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
pub(super) fn read(
    dir: &File,
    name: &str,
    max: usize,
    consumed: &mut usize,
) -> io::Result<Option<Vec<u8>>> {
    use std::os::unix::fs::MetadataExt;
    let mut file = match regular(dir, name, false, false) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let before = file.metadata()?;
    if before.len() > max as u64 {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "execution file exceeds limit",
        ));
    }
    let mut data = Vec::new();
    let read = (&mut file).take(max as u64 + 1).read_to_end(&mut data);
    *consumed = consumed.saturating_add(data.len());
    read?;
    if data.len() > max {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "execution file exceeds limit",
        ));
    }
    let after = file.metadata()?;
    if before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(io::Error::other("execution file changed during read"));
    }
    verify(dir, name, &file, false)?;
    Ok(Some(data))
}

#[cfg(unix)]
pub(super) fn atomic_write(dir: &File, name: &str, data: &[u8]) -> io::Result<()> {
    let temporary = format!("execution-{}.tmp", ulid::Ulid::new());
    let result = (|| {
        let mut file = regular(dir, &temporary, true, true)?;
        file.write_all(data)?;
        file.sync_all()?;
        verify(dir, &temporary, &file, false)?;
        // Existing destinations must also be ordinary single-link files.
        exists(dir, name)?;
        fs::renameat(dir, &temporary, dir, name)?;
        dir.sync_all()
    })();
    if result.is_err() {
        let _ = fs::unlinkat(dir, &temporary, AtFlags::empty());
    }
    result
}

#[cfg(unix)]
pub(super) fn remove(dir: &File, name: &str) -> io::Result<()> {
    if exists(dir, name)? {
        fs::unlinkat(dir, name, AtFlags::empty())?;
        dir.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "execution observations require Unix local locks",
    ))
}
#[cfg(not(unix))]
pub(super) fn open_run_directory(_: &Path) -> io::Result<File> {
    unsupported()
}
#[cfg(not(unix))]
pub(super) fn lock(_: &File, _: &str, _: bool) -> io::Result<File> {
    unsupported()
}
#[cfg(not(unix))]
pub(super) fn new_lock(_: &File, _: &str) -> io::Result<File> {
    unsupported()
}
#[cfg(not(unix))]
pub(super) fn is_locked(_: &File, _: &str) -> io::Result<bool> {
    unsupported()
}
#[cfg(not(unix))]
pub(super) fn exists(_: &File, _: &str) -> io::Result<bool> {
    unsupported()
}
#[cfg(not(unix))]
pub(super) fn marker(_: &File, _: &str) -> io::Result<()> {
    unsupported()
}
#[cfg(not(unix))]
pub(super) fn read(_: &File, _: &str, _: usize, _: &mut usize) -> io::Result<Option<Vec<u8>>> {
    unsupported()
}
#[cfg(not(unix))]
pub(super) fn atomic_write(_: &File, _: &str, _: &[u8]) -> io::Result<()> {
    unsupported()
}
