use crate::asset::{Asset, AssetGraph};
use crate::ledger::Attribution;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetLine {
    #[serde(flatten)]
    pub asset: Asset,
    pub declared_by: Attribution,
}

/// Append a single asset to the assets.jsonl store.
///
/// Creates parent directory if needed. Opens `paths.assets` in append mode,
/// writes one JSON line + newline. This is safe for parallel writers.
pub fn append_asset(
    paths: &crate::ledger::CoveragePaths,
    asset: &Asset,
    declared_by: &Attribution,
) -> std::io::Result<()> {
    std::fs::create_dir_all(&paths.root)?;

    let line = AssetLine {
        asset: asset.clone(),
        declared_by: declared_by.clone(),
    };

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.assets)?;

    let json = serde_json::to_string(&line).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;

    file.write_all(json.as_bytes())?;
    file.write_all(b"\n")?;

    Ok(())
}

/// Read and fold the assets.jsonl store into an AssetGraph.
///
/// Missing file returns an empty graph. Each line is folded via `AssetGraph::insert`,
/// applying last-write-wins semantics (order-independent, safe for parallel writers).
pub fn read_asset_graph(paths: &crate::ledger::CoveragePaths) -> AssetGraph {
    let mut graph = AssetGraph::default();

    let content = match std::fs::read_to_string(&paths.assets) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return graph,
        Err(_) => return graph,
    };

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(asset_line) = serde_json::from_str::<AssetLine>(line) {
            graph.insert(asset_line.asset);
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Coordinate, Locator};
    use crate::ledger::Surface;

    #[test]
    fn append_and_read_asset_graph_with_reinsert() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = crate::ledger::CoveragePaths::new(tmp.path(), "test_target");

        // Create first asset
        let root = Asset::new(
            "binary:binary",
            Locator(vec![Coordinate::Sha256("ab".into())]),
            "blob",
        );
        let root_id = root.id.clone();

        let attribution = Attribution {
            run_id: "run123".to_string(),
            model: "claude-opus".to_string(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        };

        // Append first asset
        append_asset(&paths, &root, &attribution).expect("first append should succeed");

        // Create and append second asset with parent
        let mut child = Asset::new(
            "binary:function",
            Locator(vec![Coordinate::Address(0x1000)]),
            "func",
        );
        child.parent = Some(root_id.clone());
        let child_id = child.id.clone();

        append_asset(&paths, &child, &attribution).expect("child append should succeed");

        // Re-insert child with new depth
        let mut child_updated = child.clone();
        child_updated.depth = Some("analyzed".to_string());
        append_asset(&paths, &child_updated, &attribution)
            .expect("reinsert append should succeed");

        // Read and verify
        let graph = read_asset_graph(&paths);

        // Verify roots
        let roots = graph.roots();
        assert_eq!(roots.len(), 1, "should have one root");
        assert_eq!(roots[0].id, root_id);

        // Verify children
        let children = graph.children(&root_id);
        assert_eq!(children.len(), 1, "should have one child");
        assert_eq!(children[0].id, child_id);

        // Verify re-inserted node has latest depth
        let child_in_graph = graph.get(&child_id).expect("child should exist");
        assert_eq!(
            child_in_graph.depth.as_deref(),
            Some("analyzed"),
            "child should have latest depth"
        );
    }

    #[test]
    fn read_asset_graph_from_missing_file_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = crate::ledger::CoveragePaths::new(tmp.path(), "nonexistent");

        let graph = read_asset_graph(&paths);
        assert_eq!(graph.roots().len(), 0, "empty graph should have no roots");
    }
}
