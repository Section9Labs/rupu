//! Walk a project directory's `.rupu/{workflows,agents,autoflows}/`
//! (and the global `~/.rupu/...`) to populate the sidebar.
//!
//! D-1 scope: just filenames + paths. No YAML parsing; later
//! sub-slices (D-2 Graph view, D-7 Agent editor) parse contents on
//! demand when a file is opened. This keeps workspace open fast even
//! for projects with hundreds of agents.

use std::path::{Path, PathBuf};

/// One discovered asset (workflow / agent / autoflow file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// Filename without extension (e.g. `review.yaml` → `"review"`).
    pub name: String,
    /// Absolute path on disk.
    pub path: PathBuf,
}

/// All assets discovered for one location (project dir or global dir).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetSet {
    pub workflows: Vec<Asset>,
    pub agents: Vec<Asset>,
    pub autoflows: Vec<Asset>,
}

/// Walk `<project_dir>/.rupu/{workflows,agents,autoflows}/`. Each is
/// optional — missing dirs yield empty vecs. Files are sorted by
/// name for deterministic UI ordering.
pub fn discover_project(project_dir: &Path) -> AssetSet {
    let rupu = project_dir.join(".rupu");
    AssetSet {
        workflows: list(&rupu.join("workflows"), "yaml"),
        agents:    list(&rupu.join("agents"), "md"),
        autoflows: list(&rupu.join("autoflows"), "yaml"),
    }
}

/// Walk `~/.rupu/{workflows,agents,autoflows}/`. Returns empty if the
/// HOME directory can't be resolved or the dirs are absent.
pub fn discover_global() -> AssetSet {
    let Some(home) = dirs_home() else {
        return AssetSet::default();
    };
    let rupu = home.join(".rupu");
    AssetSet {
        workflows: list(&rupu.join("workflows"), "yaml"),
        agents:    list(&rupu.join("agents"), "md"),
        autoflows: list(&rupu.join("autoflows"), "yaml"),
    }
}

fn dirs_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// List one directory, filtering to a single extension. Sorted by
/// filename.
fn list(dir: &Path, ext: &str) -> Vec<Asset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Asset> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(ext))
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_string();
            Some(Asset { name, path: p })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discover_finds_workflows_agents_autoflows() {
        let tmp = TempDir::new().unwrap();
        let rupu = tmp.path().join(".rupu");
        fs::create_dir_all(rupu.join("workflows")).unwrap();
        fs::create_dir_all(rupu.join("agents")).unwrap();
        fs::create_dir_all(rupu.join("autoflows")).unwrap();
        fs::write(rupu.join("workflows/review.yaml"), "name: review").unwrap();
        fs::write(rupu.join("workflows/dispatch.yaml"), "name: dispatch").unwrap();
        fs::write(rupu.join("agents/sec.md"), "---\nname: sec\n---").unwrap();
        fs::write(rupu.join("autoflows/nightly.yaml"), "name: nightly").unwrap();

        let assets = discover_project(tmp.path());

        assert_eq!(assets.workflows.len(), 2);
        assert!(assets.workflows.iter().any(|a| a.name == "review"));
        assert!(assets.workflows.iter().any(|a| a.name == "dispatch"));
        assert_eq!(assets.agents.len(), 1);
        assert_eq!(assets.agents[0].name, "sec");
        assert_eq!(assets.autoflows.len(), 1);
        assert_eq!(assets.autoflows[0].name, "nightly");
    }

    #[test]
    fn discover_missing_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let assets = discover_project(tmp.path());
        assert!(assets.workflows.is_empty());
        assert!(assets.agents.is_empty());
        assert!(assets.autoflows.is_empty());
    }

    #[test]
    fn discover_ignores_wrong_extensions() {
        let tmp = TempDir::new().unwrap();
        let rupu = tmp.path().join(".rupu");
        fs::create_dir_all(rupu.join("workflows")).unwrap();
        fs::write(rupu.join("workflows/review.yaml"), "").unwrap();
        fs::write(rupu.join("workflows/README.md"), "").unwrap();
        fs::write(rupu.join("workflows/.DS_Store"), "").unwrap();

        let assets = discover_project(tmp.path());
        assert_eq!(assets.workflows.len(), 1);
        assert_eq!(assets.workflows[0].name, "review");
    }
}
