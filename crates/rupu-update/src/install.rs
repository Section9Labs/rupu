use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// `~/.rupu/backups`.
pub fn backup_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".rupu").join("backups")
}

/// Atomically replace `target` with `new_bytes`. Writes a temp file in the same
/// directory (same filesystem → atomic `rename`), sets 0755, optionally backs up
/// the existing target first. The caller must have write access to `target`'s dir.
pub fn swap_in_place(
    new_bytes: &[u8],
    target: &Path,
    backup: Option<&Path>,
) -> Result<(), crate::UpdateError> {
    let dir = target
        .parent()
        .ok_or_else(|| crate::UpdateError::Install("target has no parent dir".into()))?;
    // Unique temp name in the target directory.
    let pid = std::process::id();
    let tmp = dir.join(format!(".rupu-update.{pid}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(new_bytes)?;
        f.flush()?;
        let mut perms = f.metadata()?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp, perms)?;
        f.sync_all()?;
    }
    if let Some(bak) = backup {
        if target.exists() {
            if let Some(bp) = bak.parent() {
                fs::create_dir_all(bp)?;
            }
            fs::copy(target, bak)?;
        }
    }
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        crate::UpdateError::Install(format!("atomic rename failed: {e}"))
    })?;
    Ok(())
}

/// Restore `target` from `backup`.
pub fn rollback(backup: &Path, target: &Path) -> Result<(), crate::UpdateError> {
    if !backup.exists() {
        return Err(crate::UpdateError::Install(format!(
            "no backup at {}",
            backup.display()
        )));
    }
    let bytes = fs::read(backup)?;
    swap_in_place(&bytes, target, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("rupu");
        fs::write(&target, b"OLD").unwrap();
        let bak = dir.path().join("bak").join("rupu-old");
        swap_in_place(b"NEW", &target, Some(&bak)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"NEW");
        assert_eq!(fs::read(&bak).unwrap(), b"OLD");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn rollback_restores_previous() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("rupu");
        fs::write(&target, b"OLD").unwrap();
        let bak = dir.path().join("rupu-old");
        swap_in_place(b"NEW", &target, Some(&bak)).unwrap();
        rollback(&bak, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"OLD");
    }
}
