//! Secret-bearing files: created private, replaced atomically.
//!
//! `auth.json` holds tokens, so no byte of it may ever be visible with a
//! mode looser than 0600 — not even its temp file for the few microseconds
//! between creation and a chmod. Every file this module creates gets its
//! mode on the `open(2)` that creates it (`O_CREAT` with mode 0600, under
//! the umask), so there is no window to close.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Open a brand-new file at `path`, private (0600) from the moment it
/// exists. Fails if `path` already exists.
pub(crate) fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Open `path` for locking, creating it private (0600) if it does not
/// exist. An existing file keeps its mode: the lock file holds no secret.
pub(crate) fn open_private_for_lock(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Follow `path` through any symlinks to the file they finally name, so a
/// write replaces the target rather than the link. A dangling link resolves
/// to its (not yet existing) target; a non-link resolves to itself.
pub(crate) fn resolve_symlink(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    // Bounded: a link cycle must not spin forever.
    for _ in 0..32 {
        let is_link = std::fs::symlink_metadata(&current)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_link {
            break;
        }
        let Ok(target) = std::fs::read_link(&current) else {
            break;
        };
        current = if target.is_absolute() {
            target
        } else {
            match current.parent() {
                Some(dir) => dir.join(target),
                None => target,
            }
        };
    }
    current
}

/// Replace the content of the file `path` names (through symlinks) with
/// `body`, atomically: a temp file next to the target, created private
/// (0600) — never any looser, under any umask — written, fsynced and
/// renamed over the target. A reader in any process sees the old file or
/// the new one, never a half-written one. On any failure the temp file is
/// removed and nothing at `path` changes.
///
/// After the write a chmod to 0600 is attempted as well, which only matters
/// under a umask that strips owner bits; on a filesystem without POSIX
/// modes (vfat, exfat, some CIFS mounts) it fails, and that is a warning,
/// not an error — the data is written.
pub(crate) fn write_private_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let target = resolve_symlink(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            std::io::Error::new(e.kind(), format!("mkdir {}: {e}", parent.display()))
        })?;
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "auth.json".into());
    // `create_new` refuses an existing name; a collision (a leftover from a
    // crashed process that had this pid) just moves on to the next sequence
    // number.
    let (tmp, mut file) = loop {
        let tmp = target.with_file_name(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        match create_private(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("create {}: {e}", tmp.display()),
                ))
            }
        }
    };
    let written = file
        .write_all(body)
        .and_then(|()| file.sync_all())
        .map_err(|e| std::io::Error::new(e.kind(), format!("write {}: {e}", tmp.display())))
        .and_then(|()| {
            drop(file);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                {
                    tracing::warn!(
                        path = %tmp.display(),
                        error = %e,
                        "could not chmod the credential file to 0600 (filesystem without POSIX modes?)"
                    );
                }
            }
            std::fs::rename(&tmp, &target).map_err(|e| {
                std::io::Error::new(e.kind(), format!("rename to {}: {e}", target.display()))
            })
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The temp file is 0600 from the `open` that creates it — before any
    /// byte is written, with no chmod to wait for.
    #[cfg(unix)]
    #[test]
    fn a_private_file_is_0600_at_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        let file = create_private(&path).unwrap();
        assert_eq!(mode_of(&path), 0o600, "mode at creation");
        drop(file);
        assert!(
            create_private(&path).is_err(),
            "create_new: an existing file is never reused"
        );
        let lock = dir.path().join("secret.lock");
        drop(open_private_for_lock(&lock).unwrap());
        assert_eq!(mode_of(&lock), 0o600, "the lock file is private too");
    }

    #[test]
    fn an_atomic_write_leaves_a_private_file_and_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_private_atomic(&path, b"{}").unwrap();
        write_private_atomic(&path, b"{\"k\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"k\":1}");
        #[cfg(unix)]
        assert_eq!(mode_of(&path), 0o600);
        let stray: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(stray.is_empty(), "{stray:?}");
    }

    /// A failed write (here: the parent is a file, so no temp can be
    /// created) changes nothing and leaves nothing behind.
    #[test]
    fn a_failed_write_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let path = blocker.join("auth.json");
        let err = write_private_atomic(&path, b"{}").unwrap_err();
        assert!(err.to_string().contains("blocker"), "{err}");
        assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "x");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_resolves_to_its_target_even_when_dangling() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real").join("creds.json");
        let link = dir.path().join("auth.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target, "dangling");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "{}").unwrap();
        assert_eq!(resolve_symlink(&link), target, "existing");
        // Relative links resolve against the link's own directory.
        let rel = dir.path().join("rel.json");
        std::os::unix::fs::symlink("real/creds.json", &rel).unwrap();
        assert_eq!(resolve_symlink(&rel), target);
        assert_eq!(resolve_symlink(&target), target, "a plain file is itself");
    }
}
