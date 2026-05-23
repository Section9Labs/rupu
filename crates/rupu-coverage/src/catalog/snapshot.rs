use crate::catalog::types::FlatCatalog;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("io error writing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("yaml serialization failed: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

pub fn write_snapshot(catalog: &FlatCatalog, path: &Path) -> Result<(), SnapshotError> {
    let yaml = serde_yaml::to_string(catalog)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SnapshotError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(path, yaml).map_err(|source| SnapshotError::Io {
        path: path.display().to_string(),
        source,
    })
}

pub fn read_snapshot(path: &Path) -> Result<FlatCatalog, SnapshotError> {
    let yaml = std::fs::read_to_string(path).map_err(|source| SnapshotError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let catalog = serde_yaml::from_str(&yaml)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};

    #[test]
    fn snapshot_round_trips_through_yaml() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
            })],
        };
        let original = flatten(&block).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested/catalog.yaml");
        write_snapshot(&original, &path).unwrap();

        let loaded = read_snapshot(&path).unwrap();
        assert_eq!(original, loaded);
    }
}
