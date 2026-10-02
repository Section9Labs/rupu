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

/// Write the snapshot atomically: a reader, or a concurrent writer (two
/// fan-out unit ingests, two local runs), sees one whole catalog, never a
/// truncated or interleaved one. The bytes go to a temp file unique to this
/// call in the target's own directory, then `rename` swaps it over `path`;
/// the temp file is removed if either step fails.
pub fn write_snapshot(catalog: &FlatCatalog, path: &Path) -> Result<(), SnapshotError> {
    let yaml = serde_yaml::to_string(catalog)?;
    let io_err = |p: &Path| {
        let path = p.display().to_string();
        move |source| SnapshotError::Io { path, source }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", ulid::Ulid::new()));
    let landed = std::fs::write(&tmp, yaml)
        .map_err(io_err(&tmp))
        .and_then(|()| std::fs::rename(&tmp, path).map_err(io_err(path)));
    if landed.is_err() {
        // Best effort: the error to report is the write/rename one.
        let _ = std::fs::remove_file(&tmp);
    }
    landed
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
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let original = flatten(&block).unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested/catalog.yaml");
        write_snapshot(&original, &path).unwrap();

        let loaded = read_snapshot(&path).unwrap();
        assert_eq!(original, loaded);
    }

    /// A catalog with `n` concerns, so different `n` give different lengths.
    fn catalog_of(n: usize) -> FlatCatalog {
        use crate::catalog::types::Concern;
        FlatCatalog {
            concerns: (0..n)
                .map(|i| Concern {
                    id: format!("c{i}"),
                    name: format!("concern {i}"),
                    description: "x".repeat(40 * (i + 1)),
                    severity: Default::default(),
                    applicable_globs: vec!["**".to_string()],
                    min_strength: crate::TouchStrength::Read,
                    references: vec![],
                    tags: vec![],
                })
                .collect(),
            sources: Default::default(),
            render_modes: Default::default(),
        }
    }

    #[test]
    fn concurrent_writers_never_leave_a_corrupt_or_torn_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("catalog.yaml");
        // Different lengths: a truncate-then-write interleave would leave a
        // shorter catalog's bytes spliced onto a longer one's tail.
        let catalogs: Vec<FlatCatalog> = (1..=8).map(|n| catalog_of(n * 3)).collect();

        let done = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            // A reader racing the writers: with rename-over-target it only
            // ever sees a whole catalog, never a truncated or spliced one.
            let reader = s.spawn(|| {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    match read_snapshot(&path) {
                        Ok(seen) => assert!(catalogs.contains(&seen), "read a torn catalog"),
                        Err(SnapshotError::Io { .. }) => {} // not written yet
                        Err(e) => panic!("read a corrupt snapshot: {e}"),
                    }
                }
            });
            let writers: Vec<_> = catalogs
                .iter()
                .map(|catalog| {
                    let path = &path;
                    s.spawn(move || {
                        for _ in 0..50 {
                            write_snapshot(catalog, path).unwrap();
                        }
                    })
                })
                .collect();
            for w in writers {
                w.join().unwrap();
            }
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            reader.join().unwrap();
        });

        let loaded = read_snapshot(&path).expect("the snapshot must parse");
        assert!(
            catalogs.contains(&loaded),
            "the snapshot must equal one of the written catalogs"
        );
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["catalog.yaml".to_string()], "no temp files");
    }

    #[test]
    fn a_failed_write_leaves_no_temp_file_behind() {
        let tmp = tempfile::TempDir::new().unwrap();
        // The rename target is a directory: the temp file is written, then
        // the rename fails, and the temp file must not outlive the call.
        let target = tmp.path().join("catalog.yaml");
        std::fs::create_dir(&target).unwrap();
        write_snapshot(&catalog_of(2), &target).unwrap_err();
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["catalog.yaml".to_string()]);

        // The parent is a file: nothing can be created, nothing is left.
        let file = tmp.path().join("plain");
        std::fs::write(&file, "x").unwrap();
        write_snapshot(&catalog_of(2), &file.join("catalog.yaml")).unwrap_err();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
    }
}
