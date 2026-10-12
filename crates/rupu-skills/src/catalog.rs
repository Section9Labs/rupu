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
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DiscoverError::Io {
                path: dir.to_owned(),
                source,
            })
        }
    };
    for item in read {
        let item = item.map_err(|source| DiscoverError::Io {
            path: dir.to_owned(),
            source,
        })?;
        let path = item.path();
        let Some(dir_name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if dir_name.starts_with('.') || !path.is_dir() {
            continue;
        }
        let skill = load_skill(&path, layer).map(Arc::new);
        out.push(CatalogEntry {
            dir_name,
            layer,
            path,
            skill,
        });
    }
    Ok(())
}
