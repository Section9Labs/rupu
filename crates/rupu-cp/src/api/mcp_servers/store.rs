//! Bounded global-only reads and an atomic registry-specific CAS writer.
//! Unknown existing fields are refused by Registry::parse before replacement.
use crate::error::{ApiError, ApiResult};
use fs2::FileExt;
use rupu_config::mcp::Registry;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};
const MAX_BYTES: usize = 256 * 1024;
pub(crate) struct Snapshot {
    pub registry: Registry,
    pub revision: String,
}
fn ordinary_file(path: &Path) -> ApiResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => Err(ApiError::conflict(
            "MCP configuration must be an ordinary file",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ApiError::internal("cannot inspect MCP configuration")),
    }
}
fn revision(bytes: Option<&[u8]>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"rupu-mcp-registry-v1\0");
    digest.update([u8::from(bytes.is_some())]);
    if let Some(bytes) = bytes {
        digest.update(bytes);
    }
    format!("sha256:{:x}", digest.finalize())
}
pub(crate) fn read(global: &Path) -> ApiResult<Snapshot> {
    let path = global.join("mcp.toml");
    if !ordinary_file(&path)? {
        return Ok(Snapshot {
            registry: Registry::default(),
            revision: revision(None),
        });
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| ApiError::internal("cannot read MCP configuration"))?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ApiError::internal("cannot read MCP configuration"))?;
    if bytes.len() > MAX_BYTES {
        return Err(ApiError::conflict(
            "existing MCP configuration exceeds its size limit",
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ApiError::conflict("existing MCP configuration is invalid"))?;
    let registry = Registry::parse(text).map_err(|_| {
        ApiError::conflict("existing MCP configuration is invalid; it was not replaced")
    })?;
    Ok(Snapshot {
        registry,
        revision: revision(Some(&bytes)),
    })
}
fn lock_registry(global: &Path) -> ApiResult<fs::File> {
    fs::create_dir_all(global)
        .map_err(|_| ApiError::internal("cannot open MCP configuration directory"))?;
    let lock_path = global.join("mcp.lock");
    ordinary_file(&lock_path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)
        .map_err(|_| ApiError::internal("cannot open MCP configuration lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| ApiError::conflict("MCP configuration is busy; refresh before saving"))?;
    Ok(lock)
}

/// Hold the registry lock while a dependent credential transaction checks its
/// exact owner snapshot. Lock ordering is always registry, then credentials.
pub(crate) fn with_snapshot_locked<T>(
    global: &Path,
    expected: &str,
    action: impl FnOnce(&Snapshot) -> ApiResult<T>,
) -> ApiResult<T> {
    let _lock = lock_registry(global)?;
    let snapshot = read(global)?;
    if snapshot.revision != expected {
        return Err(ApiError::conflict(
            "MCP configuration changed; reload before saving",
        ));
    }
    action(&snapshot)
}

pub(super) fn mutate(
    global: &Path,
    expected: &str,
    edit: impl FnOnce(&mut Registry) -> ApiResult<()>,
) -> ApiResult<Snapshot> {
    with_snapshot_locked(global, expected, |snapshot| {
        let mut current = Snapshot {
            registry: snapshot.registry.clone(),
            revision: snapshot.revision.clone(),
        };
        edit(&mut current.registry)?;
        current
            .registry
            .validate()
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        let contents = toml::to_string_pretty(&current.registry)
            .map_err(|_| ApiError::internal("cannot serialize MCP configuration"))?;
        if contents.len() > MAX_BYTES {
            return Err(ApiError::bad_request(
                "MCP configuration exceeds its size limit",
            ));
        }
        let temp = global.join(format!("mcp-{}.tmp", ulid::Ulid::new()));
        let write = (|| -> ApiResult<()> {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temp)
                .map_err(|_| ApiError::internal("cannot stage MCP configuration"))?;
            file.write_all(contents.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| ApiError::internal("cannot stage MCP configuration"))?;
            // A sibling rename replaces the registry atomically. This is a
            // trusted local root, with static non-file/symlink rejection above;
            // it is not descriptor-anchored protection against hostile FS races.
            ordinary_file(&global.join("mcp.toml"))?;
            fs::rename(&temp, global.join("mcp.toml"))
                .map_err(|_| ApiError::internal("cannot replace MCP configuration"))?;
            Ok(())
        })();
        if write.is_err() {
            let _ = fs::remove_file(&temp);
        }
        write?;
        current.revision = revision(Some(contents.as_bytes()));
        Ok(current)
    })
}
