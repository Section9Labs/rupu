//! Pack a skill into a tar archive and unpack it on another host (spec
//! `docs/superpowers/specs/2026-10-13-rupu-skills-on-placed-units-design.md`).
//! The pack's hash is the hash of exactly the bytes archived, and unpack
//! verifies it before anything is kept.

use crate::skill::{Layer, Skill};
use crate::tree::{content_hash, walk_tree, ContentHash, ContentHasher, EntryKind, TreeError};
use std::path::{Component, Path, PathBuf};

pub const MAX_SHIPPED_SKILL_BYTES: u64 = 32 << 20;
pub const MAX_BUNDLE_BYTES: u64 = 128 << 20;

#[derive(Debug)]
pub struct PackedSkill {
    pub layer: Layer,
    pub name: String,
    pub hash: ContentHash,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShipError {
    #[error("skill {name}: {source}")]
    Tree { name: String, source: TreeError },
    #[error("skill {name}: symlink {rel} has an absolute target, which can't be shipped")]
    AbsoluteSymlink { name: String, rel: String },
    #[error("skill {name} is {bytes} bytes, over the {limit}-byte limit for a shipped skill")]
    TooLarge {
        name: String,
        bytes: u64,
        limit: u64,
    },
    #[error("the skills to ship total {bytes} bytes, over the {limit}-byte limit for one unit")]
    BundleTooLarge { bytes: u64, limit: u64 },
    #[error("shipped skill archive: {0}")]
    Malformed(String),
    #[error("shipped skill content hash is {got}, expected {expected}")]
    HashMismatch { expected: String, got: String },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("shipped skill {layer}:{name} ({hash}) is not in this host's skill cache")]
    NotCached {
        layer: Layer,
        name: String,
        hash: String,
    },
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> ShipError + '_ {
    move |source| ShipError::Io {
        path: path.to_owned(),
        source,
    }
}

pub fn pack_skill(skill: &Skill) -> Result<PackedSkill, ShipError> {
    let name = skill.name.to_string();
    let tree_err = |source| ShipError::Tree {
        name: name.clone(),
        source,
    };
    let entries = walk_tree(&skill.root).map_err(tree_err)?;
    let mut total = 0u64;
    for e in &entries {
        match &e.kind {
            EntryKind::File { len, .. } => total += len,
            EntryKind::Symlink { target } => {
                // The walk accepts an absolute target that resolves inside the
                // root, but it would point at the wrong place on another host.
                if Path::new(target).is_absolute() {
                    return Err(ShipError::AbsoluteSymlink {
                        name: name.clone(),
                        rel: e.rel.clone(),
                    });
                }
            }
        }
    }
    if total > MAX_SHIPPED_SKILL_BYTES {
        return Err(ShipError::TooLarge {
            name,
            bytes: total,
            limit: MAX_SHIPPED_SKILL_BYTES,
        });
    }
    let mut hasher = ContentHasher::new();
    let mut builder = tar::Builder::new(Vec::new());
    for e in &entries {
        let mut header = tar::Header::new_ustar();
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        match &e.kind {
            EntryKind::File { mode, .. } => {
                let path = skill.root.join(&e.rel);
                let bytes = std::fs::read(&path).map_err(io(&path))?;
                hasher.file(&e.rel, *mode, &bytes);
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(*mode);
                header.set_size(bytes.len() as u64);
                builder
                    .append_data(&mut header, &e.rel, bytes.as_slice())
                    .map_err(io(&path))?;
            }
            EntryKind::Symlink { target } => {
                hasher.symlink(&e.rel, target);
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                builder
                    .append_link(&mut header, &e.rel, target)
                    .map_err(io(&skill.root))?;
            }
        }
    }
    let bytes = builder.into_inner().map_err(io(&skill.root))?;
    Ok(PackedSkill {
        layer: skill.layer,
        name: skill.name.to_string(),
        hash: hasher.finish(),
        bytes,
    })
}

/// Unpack `bytes` into a new directory `dest`, keeping it only if its
/// content hash is `expected`. Any failure leaves no `dest` behind.
pub fn unpack_skill(bytes: &[u8], dest: &Path, expected: &ContentHash) -> Result<(), ShipError> {
    if dest.exists() {
        return Err(ShipError::Malformed(format!(
            "{} already exists",
            dest.display()
        )));
    }
    std::fs::create_dir_all(dest).map_err(io(dest))?;
    let result = unpack_into(bytes, dest, expected);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    result
}

fn unpack_into(bytes: &[u8], dest: &Path, expected: &ContentHash) -> Result<(), ShipError> {
    let bad = |m: String| ShipError::Malformed(m);
    let mut archive = tar::Archive::new(bytes);
    let mut total = 0u64;
    for entry in archive.entries().map_err(io(dest))? {
        let mut entry = entry.map_err(io(dest))?;
        let rel = entry.path().map_err(io(dest))?.into_owned();
        if rel.is_absolute() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(bad(format!("unsafe path {}", rel.display())));
        }
        let kind = entry.header().entry_type();
        let target = dest.join(&rel);
        match kind {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&target).map_err(io(&target))?;
            }
            tar::EntryType::Regular => {
                total += entry.header().size().map_err(io(&target))?;
                if total > MAX_SHIPPED_SKILL_BYTES {
                    return Err(bad(format!(
                        "more than {MAX_SHIPPED_SKILL_BYTES} bytes of content"
                    )));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(io(parent))?;
                }
                let mode = entry.header().mode().unwrap_or(0o644);
                let mut file = std::fs::File::create(&target).map_err(io(&target))?;
                std::io::copy(&mut entry, &mut file).map_err(io(&target))?;
                drop(file);
                set_mode(&target, mode)?;
            }
            tar::EntryType::Symlink => {
                let link = entry
                    .link_name()
                    .map_err(io(&target))?
                    .ok_or_else(|| bad(format!("symlink {} has no target", rel.display())))?
                    .into_owned();
                if link.is_absolute() || !stays_inside(&rel, &link) {
                    return Err(bad(format!(
                        "symlink {} points outside the skill",
                        rel.display()
                    )));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(io(parent))?;
                }
                make_symlink(&link, &target)?;
            }
            other => {
                return Err(bad(format!(
                    "{}: unsupported entry type {other:?}",
                    rel.display()
                )))
            }
        }
    }
    let entries = walk_tree(dest).map_err(|source| ShipError::Tree {
        name: dest.display().to_string(),
        source,
    })?;
    let got = content_hash(dest, &entries).map_err(|source| ShipError::Tree {
        name: dest.display().to_string(),
        source,
    })?;
    if got != *expected {
        return Err(ShipError::HashMismatch {
            expected: expected.hex(),
            got: got.hex(),
        });
    }
    Ok(())
}

/// Whether `link`, resolved lexically from the directory holding `rel`,
/// stays inside the skill root.
fn stays_inside(rel: &Path, link: &Path) -> bool {
    let mut depth = rel.components().count() as i64 - 1;
    for c in link.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::CurDir => {}
            _ => return false,
        }
    }
    true
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ShipError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(io(path))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result<(), ShipError> {
    Ok(())
}

#[cfg(unix)]
fn make_symlink(link: &Path, at: &Path) -> Result<(), ShipError> {
    std::os::unix::fs::symlink(link, at).map_err(io(at))
}

#[cfg(not(unix))]
fn make_symlink(_: &Path, at: &Path) -> Result<(), ShipError> {
    Err(ShipError::Malformed(format!(
        "{}: symlinks can't be unpacked on this platform",
        at.display()
    )))
}
