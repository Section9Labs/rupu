//! One skill directory, loaded and validated.

use crate::frontmatter::{parse_skill_md, Frontmatter, FrontmatterError};
use crate::name::{NameError, SkillName};
use crate::tree::{self, ContentHash, Stamp, TreeEntry, TreeError};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
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

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("no SKILL.md in {0}")]
    MissingSkillMd(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
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
    let text = match std::fs::read_to_string(&md_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SkillError::MissingSkillMd(dir.to_owned()))
        }
        Err(source) => {
            return Err(SkillError::Io {
                path: md_path,
                source,
            })
        }
    };
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
