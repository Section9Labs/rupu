//! One skill directory, loaded and validated.

use crate::frontmatter::{parse_skill_md, Frontmatter, FrontmatterError};
use crate::name::{NameError, SkillName};
use crate::tree::{self, ContentHash, Stamp, TreeEntry, TreeError};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Global,
    Project,
}

impl Layer {
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Global => "global",
            Layer::Project => "project",
        }
    }

    pub fn parse(s: &str) -> Option<Layer> {
        match s {
            "global" => Some(Layer::Global),
            "project" => Some(Layer::Project),
            _ => None,
        }
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub struct Skill {
    pub name: SkillName,
    pub layer: Layer,
    /// The canonical directory: what the fs read tools are scoped to.
    pub root: PathBuf,
    /// The directory as found (a symlinked global skill's link path), so a
    /// reload validates the name against the right directory.
    pub dir: PathBuf,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub entries: Vec<TreeEntry>,
    pub stamp: Stamp,
}

impl Skill {
    pub fn files(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.rel.as_str()).collect()
    }

    pub fn content_hash(&self) -> Result<ContentHash, TreeError> {
        tree::content_hash(&self.root, &self.entries)
    }
}

/// The largest SKILL.md a skill may have. Its body goes whole into a
/// prompt (a preload, `skills.load`, a re-attach), so a bigger one is
/// refused rather than read. The files beside it have no limit.
pub const MAX_SKILL_MD_BYTES: u64 = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("no SKILL.md in {0}")]
    MissingSkillMd(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("SKILL.md is {bytes} bytes; the limit is {limit} bytes")]
    TooLarge { bytes: u64, limit: u64 },
    #[error("SKILL.md: {0}")]
    Frontmatter(#[from] FrontmatterError),
    #[error("name {name:?}: {source}")]
    Name { name: String, source: NameError },
    #[error("name {name:?} does not match its directory {dir:?}")]
    NameMismatch { name: String, dir: String },
    #[error("a project skill directory may not be a symlink")]
    ProjectSymlink,
    #[error("the project skills directory may not be a symlink")]
    ProjectRootSymlink,
    #[error(transparent)]
    Tree(#[from] TreeError),
}

impl SkillError {
    /// The I/O error this load failed on, if it failed on one.
    pub fn io_error(&self) -> Option<&std::io::Error> {
        match self {
            SkillError::Io { source, .. } | SkillError::Tree(TreeError::Io { source, .. }) => {
                Some(source)
            }
            _ => None,
        }
    }
}

pub fn load_skill(dir: &Path, layer: Layer) -> Result<Skill, SkillError> {
    let io = |source| SkillError::Io {
        path: dir.to_owned(),
        source,
    };
    if layer == Layer::Project
        && std::fs::symlink_metadata(dir)
            .map_err(io)?
            .file_type()
            .is_symlink()
    {
        return Err(SkillError::ProjectSymlink);
    }
    let root = dir.canonicalize().map_err(io)?;
    let md_path = root.join("SKILL.md");
    let md_io = |source| SkillError::Io {
        path: md_path.clone(),
        source,
    };
    // Opening a FIFO blocks until a writer appears; refuse anything but a
    // regular file before opening it.
    let too_large = |bytes| SkillError::TooLarge {
        bytes,
        limit: MAX_SKILL_MD_BYTES,
    };
    match std::fs::metadata(&md_path) {
        Ok(m) if m.is_file() && m.len() > MAX_SKILL_MD_BYTES => return Err(too_large(m.len())),
        Ok(m) if m.is_file() => {}
        Ok(_) => {
            return Err(TreeError::NotRegular {
                rel: "SKILL.md".into(),
            }
            .into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SkillError::MissingSkillMd(dir.to_owned()))
        }
        Err(source) => return Err(md_io(source)),
    }
    // At most one byte over the limit, in case the file grew since.
    let mut text = String::new();
    std::fs::File::open(&md_path)
        .and_then(|f| f.take(MAX_SKILL_MD_BYTES + 1).read_to_string(&mut text))
        .map_err(md_io)?;
    if text.len() as u64 > MAX_SKILL_MD_BYTES {
        return Err(too_large(text.len() as u64));
    }
    let (frontmatter, body) = parse_skill_md(&text)?;
    let name: SkillName = frontmatter
        .name
        .parse()
        .map_err(|source| SkillError::Name {
            name: frontmatter.name.clone(),
            source,
        })?;
    let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.as_str() != dir_name {
        return Err(SkillError::NameMismatch {
            name: frontmatter.name.clone(),
            dir: dir_name.to_owned(),
        });
    }
    let entries = tree::walk_tree(&root)?;
    let stamp = tree::stamp(&entries);
    Ok(Skill {
        name,
        layer,
        root,
        dir: dir.to_owned(),
        frontmatter,
        body,
        entries,
        stamp,
    })
}
