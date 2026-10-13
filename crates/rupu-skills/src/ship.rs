//! Pack a skill into a tar archive and unpack it on another host (spec
//! `docs/superpowers/specs/2026-10-13-rupu-skills-on-placed-units-design.md`).
//! The pack's hash is the hash of exactly the bytes archived, and unpack
//! verifies it before anything is kept.

use crate::catalog::OverlaySkill;
use crate::name::SkillName;
use crate::skill::{Layer, Skill};
use crate::tree::{
    content_hash, walk_tree, ContentHash, ContentHasher, EntryKind, TreeError, IGNORED_NAMES,
};
use std::io::{BufRead, Read};
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
    #[error("skill {name}: {a} and {b} differ only by letter case, which can't be shipped")]
    CaseCollision { name: String, a: String, b: String },
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
    #[error("skill bundle manifest: {0}")]
    Manifest(String),
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
    let rels: Vec<&str> = entries.iter().map(|e| e.rel.as_str()).collect();
    if let Some((a, b)) = case_collision(&rels) {
        return Err(ShipError::CaseCollision { name, a, b });
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

/// Two paths that are one on a case-insensitive filesystem: equal once
/// case-folded, or one below the other's folded path.
pub fn case_collision(rels: &[&str]) -> Option<(String, String)> {
    let mut seen = std::collections::HashMap::new();
    for r in rels {
        if let Some(prev) = seen.insert(fold(Path::new(r)), *r) {
            return Some(((*prev).to_owned(), (*r).to_owned()));
        }
    }
    for r in rels {
        for a in Path::new(r).ancestors().skip(1) {
            if let Some(prev) = seen.get(&fold(a)) {
                return Some(((*prev).to_owned(), (*r).to_owned()));
            }
        }
    }
    None
}

/// Unpack `bytes` into a new directory `dest` (its parent must exist),
/// keeping it only if its content hash is `expected`. Any failure after
/// `dest` is created leaves no `dest` behind; an existing `dest` is never
/// touched.
pub fn unpack_skill(bytes: &[u8], dest: &Path, expected: &ContentHash) -> Result<(), ShipError> {
    match std::fs::create_dir(dest) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ShipError::Malformed(format!(
                "{} already exists",
                dest.display()
            )));
        }
        Err(e) => return Err(io(dest)(e)),
    }
    let result = unpack_into(bytes, dest, expected);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    result
}

enum Item {
    Dir,
    File { mode: u32, data: Vec<u8> },
    Link(PathBuf),
}

/// Read and vet the whole archive before anything is written.
fn read_archive(bytes: &[u8]) -> Result<Vec<(PathBuf, Item)>, ShipError> {
    let bad = |m: String| ShipError::Malformed(m);
    let tar_err = |e: std::io::Error| ShipError::Malformed(format!("unreadable archive: {e}"));
    let mut archive = tar::Archive::new(bytes);
    let mut items = Vec::new();
    let mut total = 0u64;
    for entry in archive.entries().map_err(tar_err)? {
        let mut entry = entry.map_err(tar_err)?;
        let rel = entry.path().map_err(tar_err)?.into_owned();
        if rel.is_absolute() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(bad(format!("unsafe path {}", rel.display())));
        }
        // `walk_tree` never looks under these, so the final hash wouldn't
        // cover what an archive put there.
        if rel.components().any(|c| {
            let c = c.as_os_str().to_string_lossy().to_lowercase();
            IGNORED_NAMES.iter().any(|n| n.to_lowercase() == c)
        }) {
            return Err(bad(format!("{}: not part of a skill", rel.display())));
        }
        let item = match entry.header().entry_type() {
            tar::EntryType::Directory => Item::Dir,
            tar::EntryType::Regular => {
                total += entry.header().size().map_err(tar_err)?;
                if total > MAX_SHIPPED_SKILL_BYTES {
                    return Err(bad(format!(
                        "more than {MAX_SHIPPED_SKILL_BYTES} bytes of content"
                    )));
                }
                let mode = entry.header().mode().unwrap_or(0o644);
                let mut data = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut data).map_err(tar_err)?;
                Item::File { mode, data }
            }
            tar::EntryType::Symlink => {
                let link = entry
                    .link_name()
                    .map_err(tar_err)?
                    .ok_or_else(|| bad(format!("symlink {} has no target", rel.display())))?
                    .into_owned();
                if link.is_absolute() || !stays_inside(&rel, &link) {
                    return Err(bad(format!(
                        "symlink {} points outside the skill",
                        rel.display()
                    )));
                }
                Item::Link(link)
            }
            other => {
                return Err(bad(format!(
                    "{}: unsupported entry type {other:?}",
                    rel.display()
                )))
            }
        };
        items.push((rel, item));
    }
    // Entries are later written through directories, so no path may repeat
    // and none may sit below a symlink or a file.
    // Compared case-folded too: on a case-insensitive filesystem `A` and `a`
    // are one path, and a skill differing only by case can't ship portably.
    let mut seen = std::collections::HashSet::new();
    for (rel, _) in &items {
        if !seen.insert(fold(rel)) {
            return Err(bad(format!("duplicate entry {}", rel.display())));
        }
    }
    let blockers: std::collections::HashSet<PathBuf> = items
        .iter()
        .filter(|(_, i)| !matches!(i, Item::Dir))
        .map(|(r, _)| fold(r))
        .collect();
    for (rel, _) in &items {
        if rel.ancestors().skip(1).any(|a| blockers.contains(&fold(a))) {
            return Err(bad(format!("{} is below a file or symlink", rel.display())));
        }
    }
    Ok(items)
}

fn fold(p: &Path) -> PathBuf {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// An entry that collides with another path (a name that folds onto an
/// existing one, say) is a bad archive, not an I/O fault.
fn write_err(path: &Path) -> impl Fn(std::io::Error) -> ShipError + '_ {
    move |e| match e.kind() {
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::NotADirectory => {
            ShipError::Malformed(format!("{}: collides with another entry", path.display()))
        }
        _ => ShipError::Io {
            path: path.to_owned(),
            source: e,
        },
    }
}

fn unpack_into(bytes: &[u8], dest: &Path, expected: &ContentHash) -> Result<(), ShipError> {
    let items = read_archive(bytes)?;
    // Phase 1, while `dest` holds no symlink: every directory (including each
    // link's parent), then the files.
    for (rel, item) in &items {
        let target = dest.join(rel);
        match item {
            Item::Dir => std::fs::create_dir_all(&target).map_err(write_err(&target))?,
            Item::File { .. } | Item::Link(_) => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(write_err(parent))?;
                }
            }
        }
    }
    for (rel, item) in &items {
        if let Item::File { mode, data } = item {
            let target = dest.join(rel);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(write_err(&target))?;
            std::io::Write::write_all(&mut file, data).map_err(io(&target))?;
            drop(file);
            set_mode(&target, *mode)?;
        }
    }
    // Phase 2: only symlink creation, which resolves nothing but the parent
    // that phase 1 made. A path that folds onto anything already there is
    // EEXIST.
    for (rel, item) in &items {
        if let Item::Link(link) = item {
            let target = dest.join(rel);
            make_symlink(link, &target).map_err(|e| match e {
                ShipError::Io { path, source } => write_err(&path)(source),
                other => other,
            })?;
        }
    }
    let tree_err = |e: TreeError| ShipError::Malformed(format!("unpacked skill is invalid: {e}"));
    let entries = walk_tree(dest).map_err(tree_err)?;
    let got = content_hash(dest, &entries).map_err(tree_err)?;
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShippedSkillRef {
    pub layer: Layer,
    pub name: String,
    /// 64 lowercase hex digits.
    pub content_hash: String,
}

impl ShippedSkillRef {
    pub fn validate(&self) -> Result<(), ShipError> {
        let bad = |m: String| ShipError::Manifest(m);
        self.name
            .parse::<SkillName>()
            .map_err(|e| bad(format!("skill name {:?}: {e}", self.name)))?;
        let hex_ok = self.content_hash.len() == 64
            && self
                .content_hash
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !hex_ok {
            return Err(bad(format!(
                "{}:{}: content hash is not 64 lowercase hex digits",
                self.layer, self.name
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SkillBundleManifest {
    pub version: u32,
    pub skills: Vec<ShippedSkillRef>,
}

impl SkillBundleManifest {
    pub const VERSION: u32 = 1;

    /// Every name is a valid `SkillName`, every hash 64 lowercase hex, no
    /// (layer, name) twice, version == 1.
    pub fn validate(&self) -> Result<(), ShipError> {
        if self.version != Self::VERSION {
            return Err(ShipError::Manifest(format!(
                "version {} is not supported (expected {})",
                self.version,
                Self::VERSION
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for r in &self.skills {
            r.validate()?;
            if !seen.insert((r.layer, r.name.as_str())) {
                return Err(ShipError::Manifest(format!(
                    "{}:{} listed twice",
                    r.layer, r.name
                )));
            }
        }
        Ok(())
    }
}

impl PackedSkill {
    pub fn reference(&self) -> ShippedSkillRef {
        ShippedSkillRef {
            layer: self.layer,
            name: self.name.clone(),
            content_hash: self.hash.hex(),
        }
    }
}

/// `<global>/cache/skills`.
pub fn cache_dir(global: &Path) -> PathBuf {
    global.join("cache").join("skills")
}

/// `<cache>/<hash>/<name>`.
pub fn cached_path(cache: &Path, r: &ShippedSkillRef) -> PathBuf {
    cache.join(&r.content_hash).join(&r.name)
}

fn is_cached(cache: &Path, r: &ShippedSkillRef) -> bool {
    cached_path(cache, r).join("SKILL.md").is_file()
}

/// The refs whose skill isn't in the cache. A leftover temp directory
/// (dot-prefixed) is never looked at, so it never counts as cached.
pub fn missing(cache: &Path, refs: &[ShippedSkillRef]) -> Vec<ShippedSkillRef> {
    refs.iter()
        .filter(|r| !is_cached(cache, r))
        .cloned()
        .collect()
}

/// Unpack `bytes` into the cache under `r`'s hash and name, verifying the
/// hash first. Storing what is already cached is a no-op; of concurrent
/// stores of one skill, one wins and the rest return its directory.
pub fn store(cache: &Path, r: &ShippedSkillRef, bytes: &[u8]) -> Result<PathBuf, ShipError> {
    r.validate()?;
    let dest = cached_path(cache, r);
    if is_cached(cache, r) {
        return Ok(dest);
    }
    let expected = ContentHash::from_hex(&r.content_hash)
        .ok_or_else(|| ShipError::Manifest(format!("{}: bad content hash", r.name)))?;
    std::fs::create_dir_all(cache).map_err(io(cache))?;
    private_dir(cache)?;
    let tmp = tempfile::Builder::new()
        .prefix(".tmp-")
        .tempdir_in(cache)
        .map_err(io(cache))?;
    let staged = tmp.path().join(&r.name);
    unpack_skill(bytes, &staged, &expected)?;
    let hash_dir = cache.join(&r.content_hash);
    // Two skills with identical content share a hash directory, so when it
    // already exists the staged skill moves into it instead.
    let first = if hash_dir.exists() {
        std::fs::rename(&staged, &dest).map_err(|e| (dest.clone(), e))
    } else {
        match std::fs::rename(tmp.path(), &hash_dir) {
            Ok(()) => {
                // The temp dir is the hash dir now; nothing is left to remove.
                let _ = tmp.keep();
                Ok(())
            }
            Err(e) if hash_dir.exists() => {
                // Another store created the hash dir first; join it.
                std::fs::rename(&staged, &dest).map_err(|_| (hash_dir.clone(), e))
            }
            Err(e) => Err((hash_dir.clone(), e)),
        }
    };
    match first {
        Ok(()) => {
            private_dir(&hash_dir)?;
            Ok(dest)
        }
        // Another store won the race.
        Err(_) if is_cached(cache, r) => Ok(dest),
        Err((path, e)) => Err(io(&path)(e)),
    }
}

#[cfg(unix)]
fn private_dir(path: &Path) -> Result<(), ShipError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io(path))
}

#[cfg(not(unix))]
fn private_dir(_: &Path) -> Result<(), ShipError> {
    Ok(())
}

/// The overlay a unit's skills resolve against, or `NotCached` for the first gap.
pub fn overlay_for(cache: &Path, m: &SkillBundleManifest) -> Result<Vec<OverlaySkill>, ShipError> {
    m.validate()?;
    m.skills
        .iter()
        .map(|r| {
            if is_cached(cache, r) {
                Ok(OverlaySkill {
                    layer: r.layer,
                    name: r.name.clone(),
                    dir: cached_path(cache, r),
                })
            } else {
                Err(ShipError::NotCached {
                    layer: r.layer,
                    name: r.name.clone(),
                    hash: r.content_hash.clone(),
                })
            }
        })
        .collect()
}

/// stdin framing for `rupu __skills receive`: per skill, a line
/// `<hash> <layer> <name> <len>\n` then `<len>` bytes.
pub fn encode_stream(skills: &[(&ShippedSkillRef, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (r, bytes) in skills {
        out.extend_from_slice(
            format!(
                "{} {} {} {}\n",
                r.content_hash,
                r.layer,
                r.name,
                bytes.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(bytes);
    }
    out
}

const MAX_HEADER_BYTES: u64 = 512;

pub fn decode_stream(
    r: &mut impl std::io::BufRead,
    total_cap: u64,
) -> Result<Vec<(ShippedSkillRef, Vec<u8>)>, ShipError> {
    let bad = |m: String| ShipError::Malformed(m);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    loop {
        let mut line = Vec::new();
        let n = std::io::Read::take(&mut *r, MAX_HEADER_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| bad(format!("stream: {e}")))?;
        if n == 0 {
            return Ok(out);
        }
        if line.last() != Some(&b'\n') {
            return Err(bad("stream header is unterminated or too long".into()));
        }
        line.pop();
        let line = String::from_utf8(line).map_err(|_| bad("stream header is not UTF-8".into()))?;
        let parts: Vec<&str> = line.split(' ').collect();
        let [hash, layer, name, len] = parts[..] else {
            return Err(bad(format!(
                "stream header {line:?} is not `<hash> <layer> <name> <len>`"
            )));
        };
        let layer = Layer::parse(layer).ok_or_else(|| bad(format!("unknown layer {layer:?}")))?;
        let len: u64 = len
            .parse()
            .map_err(|_| bad(format!("bad length {len:?}")))?;
        let sref = ShippedSkillRef {
            layer,
            name: name.to_owned(),
            content_hash: hash.to_owned(),
        };
        sref.validate()?;
        if !seen.insert((sref.layer, sref.name.clone())) {
            return Err(bad(format!("{}:{} streamed twice", sref.layer, sref.name)));
        }
        total = total.saturating_add(len);
        if total > total_cap {
            return Err(ShipError::BundleTooLarge {
                bytes: total,
                limit: total_cap,
            });
        }
        let mut data = Vec::new();
        let got = std::io::Read::take(&mut *r, len)
            .read_to_end(&mut data)
            .map_err(|e| bad(format!("stream: {e}")))?;
        if got as u64 != len {
            return Err(bad(format!(
                "{name}: stream ended after {got} of {len} bytes"
            )));
        }
        out.push((sref, data));
    }
}
