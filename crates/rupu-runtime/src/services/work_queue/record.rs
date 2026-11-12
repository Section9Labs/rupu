//! Bounded owner projections. A save between reads is legitimate; replacement
//! during one read is refused. Metadata opens must never wait on a FIFO.
use super::refused;
use rupu_fleet::FleetError;
use serde::de::DeserializeOwned;
use std::path::Path;

pub(super) fn read<T: DeserializeOwned>(
    directory: &Path,
    key: &str,
) -> Result<Option<T>, FleetError> {
    read_with(directory, key, || {})
}

#[cfg(not(unix))]
fn read_with<T: DeserializeOwned>(
    _: &Path,
    _: &str,
    _: impl FnOnce(),
) -> Result<Option<T>, FleetError> {
    Err(refused("Queue owner reads require Unix lifecycle guards"))
}

#[cfg(unix)]
fn read_with<T: DeserializeOwned>(
    directory: &Path,
    key: &str,
    after_check: impl FnOnce(),
) -> Result<Option<T>, FleetError> {
    use crate::retention_lease::open_directory;
    use rustix::fs::{openat, statat, AtFlags, FileType, Mode, OFlags};
    use std::{fs::File, io::Read, os::unix::fs::MetadataExt};
    const MAX_BYTES: u64 = 16 * 1024 * 1024;
    let failure = || refused("Queue owner record changed, is unsafe, or is unavailable");
    let dir = open_directory(directory).map_err(|_| failure())?;
    let checked = match statat(&dir, key, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(failure()),
    };
    if FileType::from_raw_mode(checked.st_mode) != FileType::RegularFile {
        return Err(failure());
    }
    after_check();
    let regular = || -> Result<File, FleetError> {
        let file = File::from(
            openat(
                &dir,
                key,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| failure())?,
        );
        let meta = file.metadata().map_err(|_| failure())?;
        if !meta.is_file() {
            return Err(failure());
        }
        Ok(file)
    };
    let stamp = |file: &File| -> Result<_, FleetError> {
        let m = file.metadata().map_err(|_| failure())?;
        Ok((
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
            m.nlink(),
        ))
    };
    let mut file = regular()?;
    let before = stamp(&file)?;
    if (checked.st_dev as u64, checked.st_ino as u64) != (before.0, before.1) {
        return Err(failure());
    }
    if before.2 > MAX_BYTES {
        return Err(refused("Queue owner record exceeds 16 MiB bound"));
    }
    let mut bounded = (&mut file).take(MAX_BYTES + 1);
    let value = serde_json::from_reader(&mut bounded)
        .map_err(|_| refused("Queue owner record is malformed or exceeds bound"))?;
    if MAX_BYTES + 1 - bounded.limit() > MAX_BYTES
        || stamp(&file)? != before
        || stamp(&regular()?)? != before
    {
        return Err(failure());
    }
    let old_dir = dir.metadata().map_err(|_| failure())?;
    let current_dir = open_directory(directory)
        .map_err(|_| failure())?
        .metadata()
        .map_err(|_| failure())?;
    if (old_dir.dev(), old_dir.ino()) != (current_dir.dev(), current_dir.ino()) {
        return Err(failure());
    }
    Ok(Some(value))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn replacement_between_metadata_and_open_is_refused_without_blocking() {
        for replacement in ["symlink", "fifo", "regular"] {
            let temp = tempfile::tempdir().unwrap();
            let directory = temp.path().canonicalize().unwrap();
            let path = directory.join("session.json");
            let alternate = directory.join("alternate.json");
            fs::write(&path, "{}").unwrap();
            fs::write(&alternate, "{}").unwrap();
            let result: Result<Option<serde_json::Value>, _> =
                read_with(&directory, "session.json", || {
                    fs::remove_file(&path).unwrap();
                    match replacement {
                        "symlink" => std::os::unix::fs::symlink(&alternate, &path).unwrap(),
                        "fifo" => assert!(std::process::Command::new("mkfifo")
                            .arg(&path)
                            .status()
                            .unwrap()
                            .success()),
                        _ => fs::rename(&alternate, &path).unwrap(),
                    }
                });
            assert!(result.is_err(), "must refuse {replacement} replacement");
        }
    }
}
