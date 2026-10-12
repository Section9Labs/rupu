//! Every skill installed in the global and project layers. Both layers are
//! kept: a project skill never shadows a global one (spec §1.2). Grants
//! decide which one binds (`grant.rs`).

use crate::skill::{load_skill, Layer, Skill, SkillError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The two `skills/` directories. `project` is `None` when the run has no
/// project layer (see `rupu_workspace::project_layer_dir`).
#[derive(Debug, Clone, Default)]
pub struct LayerDirs {
    pub global: Option<PathBuf>,
    pub project: Option<PathBuf>,
}

impl LayerDirs {
    /// The `skills/` directories under the global `.rupu` and, when the run
    /// has one, the project's.
    pub fn for_layers(global_rupu_dir: &Path, project_rupu_dir: Option<&Path>) -> Self {
        Self {
            global: Some(global_rupu_dir.join("skills")),
            project: project_rupu_dir.map(|p| p.join("skills")),
        }
    }

    pub fn skills_dir(&self, layer: Layer) -> Option<&Path> {
        match layer {
            Layer::Global => self.global.as_deref(),
            Layer::Project => self.project.as_deref(),
        }
    }
}

#[derive(Debug)]
pub struct CatalogEntry {
    pub dir_name: String,
    pub layer: Layer,
    pub path: PathBuf,
    pub skill: Result<Arc<Skill>, SkillError>,
}

impl CatalogEntry {
    pub fn is_valid(&self) -> bool {
        self.skill.is_ok()
    }
}

#[derive(Debug, Default)]
pub struct SkillCatalog {
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoverError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl SkillCatalog {
    pub fn discover(dirs: &LayerDirs) -> Result<Self, DiscoverError> {
        let mut entries = Vec::new();
        for (dir, layer) in [
            (&dirs.global, Layer::Global),
            (&dirs.project, Layer::Project),
        ] {
            if let Some(dir) = dir {
                scan(dir, layer, &mut entries)?;
            }
        }
        entries.sort_by(|a, b| (&a.dir_name, a.layer).cmp(&(&b.dir_name, b.layer)));
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a CatalogEntry> {
        self.entries.iter().filter(move |e| e.dir_name == name)
    }

    pub fn get(&self, layer: Layer, name: &str) -> Option<&CatalogEntry> {
        self.entries
            .iter()
            .find(|e| e.layer == layer && e.dir_name == name)
    }
}

fn scan(dir: &Path, layer: Layer, out: &mut Vec<CatalogEntry>) -> Result<(), DiscoverError> {
    let io = |source| DiscoverError::Io {
        path: dir.to_owned(),
        source,
    };
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io(source)),
    };
    // A symlinked project `skills/` would let a repo point every project
    // skill's read root anywhere on the machine (spec §1.1).
    let root_is_link = layer == Layer::Project
        && std::fs::symlink_metadata(dir)
            .map_err(io)?
            .file_type()
            .is_symlink();
    for item in read {
        let item = item.map_err(io)?;
        let path = item.path();
        let Some(dir_name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if dir_name.starts_with('.') || !may_be_skill(&item)? {
            continue;
        }
        let skill = if root_is_link {
            Err(SkillError::ProjectRootSymlink)
        } else {
            load_skill(&path, layer).map(Arc::new)
        };
        out.push(CatalogEntry {
            dir_name,
            layer,
            path,
            skill,
        });
    }
    Ok(())
}

/// A directory, or a symlink that is not known to point at a file. A
/// dangling or unreadable link is kept so `load_skill` reports its error.
fn may_be_skill(item: &std::fs::DirEntry) -> Result<bool, DiscoverError> {
    let kind = item.file_type().map_err(|source| DiscoverError::Io {
        path: item.path(),
        source,
    })?;
    Ok(kind.is_dir()
        || (kind.is_symlink() && std::fs::metadata(item.path()).map_or(true, |m| m.is_dir())))
}
