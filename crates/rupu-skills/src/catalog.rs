//! Every skill installed in the global and project layers. Both layers are
//! kept: a project skill never shadows a global one (spec §1.2). Grants
//! decide which one binds (`grant.rs`).

use crate::record::{read_record, RecordError, SourceRecord};
use crate::skill::{load_skill, Layer, Skill, SkillError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The two `skills/` directories. `project` is `None` when the run has no
/// project layer (see `rupu_workspace::project_layer_dir`).
#[derive(Debug, Clone, Default)]
pub struct LayerDirs {
    pub global: Option<PathBuf>,
    pub project: Option<PathBuf>,
}

impl LayerDirs {
    /// The `skills/` directories under the global `.rupu` and, when the run
    /// has one, the project's. A project `.rupu` that IS the global dir
    /// (`project_root_for` resolved to `$HOME`) is no project layer (R5).
    pub fn for_layers(global_rupu_dir: &Path, project_rupu_dir: Option<&Path>) -> Self {
        let same = |p: &Path| match (p.canonicalize(), global_rupu_dir.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => p == global_rupu_dir,
        };
        Self {
            global: Some(global_rupu_dir.join("skills")),
            project: project_rupu_dir
                .filter(|p| !same(p))
                .map(|p| p.join("skills")),
        }
    }

    pub fn skills_dir(&self, layer: Layer) -> Option<&Path> {
        match layer {
            Layer::Global => self.global.as_deref(),
            Layer::Project => self.project.as_deref(),
        }
    }
}

/// One directory found in a layer's `skills/`. Discovery only lists it; the
/// skill loads (SKILL.md read, tree walked) on the first `skill()` call and
/// stays cached for the catalog's life, so a run pays only for the skills
/// its grants name.
#[derive(Debug)]
pub struct CatalogEntry {
    pub dir_name: String,
    pub layer: Layer,
    pub path: PathBuf,
    in_linked_root: bool,
    loaded: OnceLock<Result<Arc<Skill>, Arc<SkillError>>>,
}

impl CatalogEntry {
    pub fn skill(&self) -> Result<&Arc<Skill>, &SkillError> {
        self.load().as_ref().map_err(|e| &**e)
    }

    pub fn is_valid(&self) -> bool {
        self.skill().is_ok()
    }

    /// Where the skill was installed from, if it has a record. A skill in
    /// a symlinked project `skills/` has its record refused unread.
    pub fn record(&self) -> Result<Option<SourceRecord>, RecordError> {
        if self.in_linked_root {
            return Err(RecordError::LinkedRoot);
        }
        read_record(self.path.parent().unwrap_or(&self.path), &self.dir_name)
    }

    pub(crate) fn load(&self) -> &Result<Arc<Skill>, Arc<SkillError>> {
        self.loaded.get_or_init(|| {
            if self.in_linked_root {
                return Err(Arc::new(SkillError::ProjectRootSymlink));
            }
            load_skill(&self.path, self.layer)
                .map(Arc::new)
                .map_err(Arc::new)
        })
    }
}

#[derive(Debug, Default)]
pub struct SkillCatalog {
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DiscoverError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: Arc<std::io::Error>,
    },
}

impl SkillCatalog {
    pub fn discover(dirs: &LayerDirs) -> Result<Self, DiscoverError> {
        let (catalog, failed) = Self::discover_readable(dirs);
        match failed.into_iter().next() {
            Some((_, e)) => Err(e),
            None => Ok(catalog),
        }
    }

    /// The skills of every layer that can be read, and why each other
    /// layer can't: a listing shows what it can.
    pub fn discover_readable(dirs: &LayerDirs) -> (Self, Vec<(Layer, DiscoverError)>) {
        let mut entries = Vec::new();
        let mut failed = Vec::new();
        for (dir, layer) in [
            (&dirs.global, Layer::Global),
            (&dirs.project, Layer::Project),
        ] {
            let Some(dir) = dir else { continue };
            let mut found = Vec::new();
            match scan(dir, layer, &mut found) {
                Ok(()) => entries.append(&mut found),
                Err(e) => failed.push((layer, e)),
            }
        }
        entries.sort_by(|a, b| (&a.dir_name, a.layer).cmp(&(&b.dir_name, b.layer)));
        (Self { entries }, failed)
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
        source: Arc::new(source),
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
        out.push(CatalogEntry {
            dir_name,
            layer,
            path,
            in_linked_root: root_is_link,
            loaded: OnceLock::new(),
        });
    }
    Ok(())
}

/// A directory, or a symlink that is not known to point at a file. A
/// dangling or unreadable link is kept so `load_skill` reports its error.
fn may_be_skill(item: &std::fs::DirEntry) -> Result<bool, DiscoverError> {
    let kind = item.file_type().map_err(|source| DiscoverError::Io {
        path: item.path(),
        source: Arc::new(source),
    })?;
    Ok(kind.is_dir()
        || (kind.is_symlink() && std::fs::metadata(item.path()).map_or(true, |m| m.is_dir())))
}
