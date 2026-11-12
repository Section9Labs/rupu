use super::*;
use rustix::fs::{Mode, OFlags};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct Identity {
    dev: u64,
    ino: u64,
}
impl Identity {
    pub fn of(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct Stamp {
    identity: Identity,
    bytes: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
    links: u64,
    mode: u32,
}
impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            identity: Identity::of(meta),
            bytes: meta.len(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_nsec: meta.ctime_nsec(),
            links: meta.nlink(),
            mode: meta.mode(),
        }
    }
}
pub(super) fn directory(path: &Path) -> ApiResult<File> {
    if !path.is_absolute() {
        return Err(ApiError::bad_request("agent directory must be absolute"));
    }
    let mut fd = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(storage)?;
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => {
                fd = rustix::fs::openat(
                    &fd,
                    name,
                    OFlags::RDONLY
                        | OFlags::DIRECTORY
                        | OFlags::NOFOLLOW
                        | OFlags::NONBLOCK
                        | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(unsafe_source)?
            }
            _ => {
                return Err(ApiError::bad_request(
                    "agent directory has unsupported path components",
                ))
            }
        }
    }
    Ok(File::from(fd))
}
fn unsafe_source(_: impl std::fmt::Display) -> ApiError {
    ApiError::conflict("agent source is missing, redirected or not a supported regular file")
}
pub(super) fn read(path: &Path, cap: usize) -> ApiResult<(Vec<u8>, Stamp)> {
    let mut file = match crate::api::fs_open::open_anchored_regular_file(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found(
                "exact agent source or project registration is missing",
            ))
        }
        Err(e) => return Err(unsafe_source(e)),
    };
    let before = file.metadata().map_err(storage)?;
    if before.nlink() != 1 {
        return Err(ApiError::conflict(
            "agent source has ambiguous hard-link ownership",
        ));
    }
    if before.len() > cap as u64 {
        return Err(large("agent source exceeds its byte budget"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(storage)?;
    let after = file.metadata().map_err(storage)?;
    if bytes.len() > cap {
        return Err(large("agent source exceeds its byte budget"));
    }
    let stamp = Stamp::of(&before);
    if stamp != Stamp::of(&after) || bytes.len() != before.len() as usize {
        return Err(changed());
    }
    let named = crate::api::fs_open::open_anchored_regular_file(path).map_err(unsafe_source)?;
    if Stamp::of(&named.metadata().map_err(storage)?) != stamp {
        return Err(changed());
    }
    Ok((bytes, stamp))
}
pub(super) fn lock(dir: &Path) -> ApiResult<File> {
    let directory = directory(dir)?;
    let fd = rustix::fs::openat(
        &directory,
        ".agent-definitions.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(unsafe_source)?;
    let file = File::from(fd);
    let metadata = file.metadata().map_err(storage)?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() != 0 {
        return Err(ApiError::conflict(
            "agent writer lock is not a supported lock file",
        ));
    }
    fs2::FileExt::try_lock_exclusive(&file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            ApiError(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "agent definition writer is busy".into(),
            )
        } else {
            storage(e)
        }
    })?;
    let check =
        crate::api::fs_open::open_anchored_regular_file(&dir.join(".agent-definitions.lock"))
            .map_err(unsafe_source)?;
    if Identity::of(&check.metadata().map_err(storage)?) != Identity::of(&metadata) {
        return Err(changed());
    }
    Ok(file)
}
/// The caller holds the single directory writer lock. No lock is reacquired.
/// Recheck runs after durable staging but BEFORE rename; errors there have no
/// target effect. Errors after rename are deliberately reported as unknown.
pub(super) fn replace(
    owner: &Owner,
    old: &Stamp,
    raw: &[u8],
    recheck: impl FnOnce() -> ApiResult<()>,
    fault: Fault,
) -> ApiResult<()> {
    let dir = directory(&owner.dir)?;
    if Identity::of(&dir.metadata().map_err(storage)?) != owner.directory_identity {
        return Err(changed());
    }
    let temp = format!(".agent-repair-{}.tmp", ulid::Ulid::new());
    let fd = rustix::fs::openat(
        &dir,
        temp.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(storage)?;
    let result = (|| {
        let mut staged = File::from(fd);
        staged
            .set_permissions(fs::Permissions::from_mode(old.mode & 0o777))
            .map_err(storage)?;
        staged.write_all(raw).map_err(storage)?;
        staged.sync_all().map_err(storage)?;
        recheck()?;
        if fault == Fault::BeforeRename {
            return Err(storage("injected pre-replacement fault"));
        }
        rustix::fs::renameat(&dir, temp.as_str(), &dir, owner.filename.as_str())
            .map_err(storage)?;
        if fault == Fault::AfterRename {
            return Err(unknown());
        }
        dir.sync_all().map_err(|_| unknown())?;
        Ok(())
    })();
    // Cleanup only the exclusive staging name; never the selected source.
    let _ = rustix::fs::unlinkat(&dir, temp.as_str(), rustix::fs::AtFlags::empty());
    result
}
