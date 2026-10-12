//! A skill directory's files and its two fingerprints (spec §1.3). The
//! stamp costs one `stat` per file and is what an index records. The
//! content hash reads every byte and is computed only when a skill loads.

use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Never part of a skill: VCS metadata and Finder litter.
pub const IGNORED_NAMES: &[&str] = &[".git", ".DS_Store"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub rel: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// `mode` is git-style: `0o755` when any exec bit is set, else `0o644`,
    /// so a umask or `chmod g+w` doesn't change the skill's version.
    File {
        len: u64,
        mtime_ns: u128,
        mode: u32,
    },
    Symlink {
        target: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0}: path is not UTF-8")]
    NotUtf8(PathBuf),
    #[error("symlink {rel}: {reason}")]
    BadSymlink { rel: String, reason: String },
    #[error("{rel}: not a regular file, directory or symlink")]
    NotRegular { rel: String },
}

macro_rules! fingerprint {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub [u8; 32]);

        impl $name {
            pub fn hex(&self) -> String {
                hex::encode(self.0)
            }
            pub fn short(&self) -> String {
                self.hex()[..12].to_owned()
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.hex())
            }
        }
    };
}

fingerprint!(Stamp);
fingerprint!(ContentHash);

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> TreeError + '_ {
    move |source| TreeError::Io {
        path: path.to_owned(),
        source,
    }
}

pub fn walk_tree(root: &Path) -> Result<Vec<TreeEntry>, TreeError> {
    let root = root.canonicalize().map_err(io(root))?;
    let walker = walkdir::WalkDir::new(&root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !e.file_name()
                .to_str()
                .is_some_and(|n| IGNORED_NAMES.contains(&n))
        });
    let mut out = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|e| {
            let path = e.path().map(Path::to_owned).unwrap_or_else(|| root.clone());
            TreeError::Io {
                path,
                source: e.into(),
            }
        })?;
        let path = entry.path();
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .components()
            .map(|c| c.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| TreeError::NotUtf8(path.to_owned()))?
            .join("/");
        let ft = entry.file_type();
        if ft.is_dir() {
            continue;
        }
        if ft.is_symlink() {
            let target = std::fs::read_link(path).map_err(io(path))?;
            let target = target
                .to_str()
                .ok_or_else(|| TreeError::NotUtf8(path.to_owned()))?
                .to_owned();
            let resolved = path.canonicalize().map_err(|e| TreeError::BadSymlink {
                rel: rel.clone(),
                reason: format!("does not resolve ({e})"),
            })?;
            let Ok(inside) = resolved.strip_prefix(&root) else {
                return Err(TreeError::BadSymlink {
                    rel,
                    reason: format!("points outside the skill ({})", resolved.display()),
                });
            };
            if let Some(ignored) = inside
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .find(|c| IGNORED_NAMES.contains(c))
            {
                return Err(TreeError::BadSymlink {
                    rel,
                    reason: format!("points into ignored `{ignored}`"),
                });
            }
            out.push(TreeEntry {
                rel,
                kind: EntryKind::Symlink { target },
            });
            continue;
        }
        if !ft.is_file() {
            return Err(TreeError::NotRegular { rel });
        }
        let md = entry.metadata().map_err(|e| TreeError::Io {
            path: path.to_owned(),
            source: e.into(),
        })?;
        let mtime_ns = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        out.push(TreeEntry {
            rel,
            kind: EntryKind::File {
                len: md.len(),
                mtime_ns,
                mode: mode_of(&md),
            },
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

#[cfg(unix)]
fn mode_of(md: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if md.permissions().mode() & 0o111 != 0 {
        0o755
    } else {
        0o644
    }
}

#[cfg(not(unix))]
fn mode_of(_: &std::fs::Metadata) -> u32 {
    0o644
}

pub fn stamp(entries: &[TreeEntry]) -> Stamp {
    let mut h = Sha256::new();
    for e in entries {
        h.update(e.rel.as_bytes());
        h.update([0]);
        match &e.kind {
            EntryKind::File {
                len,
                mtime_ns,
                mode,
            } => {
                h.update(format!("f\0{len}\0{mtime_ns}\0{mode:o}\n").as_bytes());
            }
            EntryKind::Symlink { target } => {
                h.update(b"l\0");
                h.update(target.as_bytes());
                h.update(b"\n");
            }
        }
    }
    Stamp(h.finalize().into())
}

pub fn content_hash(root: &Path, entries: &[TreeEntry]) -> Result<ContentHash, TreeError> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    for e in entries {
        h.update(e.rel.as_bytes());
        h.update([0]);
        match &e.kind {
            EntryKind::File { mode, .. } => {
                let path = root.join(&e.rel);
                let mut f = std::fs::File::open(&path).map_err(io(&path))?;
                let len = f.metadata().map_err(io(&path))?.len();
                h.update(format!("f\0{mode:o}\0{len}\0").as_bytes());
                loop {
                    let n = f.read(&mut buf).map_err(io(&path))?;
                    if n == 0 {
                        break;
                    }
                    h.update(&buf[..n]);
                }
            }
            EntryKind::Symlink { target } => {
                h.update(b"l\0");
                h.update(target.as_bytes());
            }
        }
        h.update(b"\n");
    }
    Ok(ContentHash(h.finalize().into()))
}
