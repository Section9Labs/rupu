use super::*;
use rustix::fs::{AtFlags, Mode, OFlags};
use std::{
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    pub fn of(m: &fs::Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Stamp {
    pub identity: Identity,
    pub bytes: u64,
    pub(super) modified: (i64, i64),
    changed: (i64, i64),
    pub links: u64,
}
impl Stamp {
    pub fn of(m: &fs::Metadata) -> Self {
        Self {
            identity: Identity::of(m),
            bytes: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
            links: m.nlink(),
        }
    }
}
pub(super) fn directory(path: &Path) -> ApiResult<fs::File> {
    if !path.is_absolute() {
        return Err(ApiError::bad_request("retention paths must be absolute"));
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut dir = fs::File::from(rustix::fs::open("/", flags, Mode::empty()).map_err(internal)?);
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => {
                dir = fs::File::from(
                    rustix::fs::openat(&dir, name, flags, Mode::empty()).map_err(internal)?,
                );
            }
            _ => return Err(ApiError::bad_request("noncanonical retention path")),
        }
    }
    Ok(dir)
}
pub(super) fn identity(path: &Path) -> ApiResult<Identity> {
    Ok(Identity::of(
        &directory(path)?.metadata().map_err(internal)?,
    ))
}
pub(super) fn present(path: &Path) -> ApiResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(internal(e)),
    }
}
pub(super) fn stamp(path: &Path, is_dir: bool) -> ApiResult<Stamp> {
    let f = if is_dir {
        directory(path)?
    } else {
        crate::api::fs_open::open_anchored_regular_file(path).map_err(internal)?
    };
    let m = f.metadata().map_err(internal)?;
    if !is_dir && m.nlink() != 1 {
        return Err(ApiError::conflict(
            "ambiguous hard-linked retention artifact",
        ));
    }
    Ok(Stamp::of(&m))
}
pub(super) fn read(path: &Path, cap: usize) -> ApiResult<Vec<u8>> {
    let file = crate::api::fs_open::open_anchored_regular_file(path).map_err(internal)?;
    let before = Stamp::of(&file.metadata().map_err(internal)?);
    if before.bytes > cap as u64 {
        return Err(large("retention metadata exceeds its byte limit"));
    }
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(internal)?;
    if bytes.len() > cap {
        return Err(large("retention metadata exceeds its byte limit"));
    }
    let after = stamp(path, false)?;
    if before != after {
        return Err(ApiError::conflict("retention source changed during read"));
    }
    Ok(bytes)
}
pub(super) fn atomic(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    if bytes.len() > MAX_BYTES {
        return Err(large("retention journal exceeds its byte limit"));
    }
    let parent = directory(path.parent().ok_or_else(|| internal("missing parent"))?)?;
    let name = path
        .file_name()
        .ok_or_else(|| internal("missing filename"))?;
    let temporary = format!(".{}.tmp", ulid::Ulid::new());
    let mut file = fs::File::from(
        rustix::fs::openat(
            &parent,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(internal)?,
    );
    file.write_all(bytes).map_err(internal)?;
    file.sync_all().map_err(internal)?;
    rustix::fs::renameat(&parent, temporary.as_str(), &parent, name).map_err(internal)?;
    parent.sync_all().map_err(internal)
}
pub(super) fn create_dir(parent: &Path, name: &str) -> ApiResult<()> {
    valid_id(name)?;
    let fd = directory(parent)?;
    match rustix::fs::mkdirat(&fd, name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
        Ok(()) => fd.sync_all().map_err(internal)?,
        Err(rustix::io::Errno::EXIST) => (),
        Err(e) => return Err(internal(e)),
    }
    directory(&parent.join(name))?;
    Ok(())
}
pub(super) fn remove(root: &Root, artifact: &Artifact) -> ApiResult<bool> {
    if identity(&root.path)? != root.identity {
        return Err(ApiError::conflict("retention root changed"));
    }
    let path = root.path.join(&artifact.relative);
    if !present(&path)? {
        return Ok(false);
    }
    let observed = stamp(&path, artifact.directory)?;
    if if artifact.directory {
        observed.identity != artifact.stamp.identity
    } else {
        observed != artifact.stamp
    } {
        return Err(ApiError::conflict("retention artifact changed"));
    }
    let parent = directory(path.parent().ok_or_else(|| internal("invalid artifact"))?)?;
    rustix::fs::unlinkat(
        &parent,
        path.file_name()
            .ok_or_else(|| internal("invalid artifact"))?,
        if artifact.directory {
            AtFlags::REMOVEDIR
        } else {
            AtFlags::empty()
        },
    )
    .map_err(internal)?;
    parent.sync_all().map_err(internal)?;
    Ok(true)
}

/// Single reviewed regular file, anchored to one opened parent. The shared
/// lifecycle lease coordinates supported owner restore/archive operations;
/// this is not an atomic compare-and-unlink against arbitrary writers.
pub(super) fn remove_exact_file(
    root: &Root,
    artifact: &Artifact,
    lease: &rupu_netflow::ledger::lease::LedgerLease,
) -> ApiResult<bool> {
    if artifact.directory
        || identity(&root.path)? != root.identity
        || lease.path() != root.path.join(&artifact.relative)
    {
        return Err(ApiError::conflict("reviewed ledger root changed"));
    }
    let fd = match lease.read() {
        Ok(fd) => fd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(internal(e)),
    };
    let metadata = fd.metadata().map_err(internal)?;
    if Stamp::of(&metadata) != artifact.stamp || identity(&root.path)? != root.identity {
        return Err(ApiError::conflict("reviewed ledger changed before removal"));
    }
    lease.unlink(&metadata).map_err(internal)?;
    Ok(true)
}
