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
/// Creates parent directory if needed. Serializes the entire line first,
/// then opens `paths.assets` in append mode and writes in a single `write_all` call.
/// A single write under O_APPEND is atomic on local filesystems,
/// making this safe for parallel writers.
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

    // Serialize before opening the file to avoid leaving an empty file on error.
    let mut buf = serde_json::to_vec(&line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    buf.push(b'\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.assets)?;

    file.write_all(&buf)?;

    Ok(())
}

/// Read and fold the assets.jsonl store into an AssetGraph.
///
/// Missing file returns an empty graph. Each line is folded via `AssetGraph::insert`,
/// applying last-write-wins semantics (order-independent, safe for parallel writers).
/// Unparseable lines are skipped with a tracing warning.
pub fn read_asset_graph(paths: &crate::ledger::CoveragePaths) -> AssetGraph {
    let mut graph = AssetGraph::default();

    let content = match std::fs::read_to_string(&paths.assets) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return graph,
        Err(e) => {
            tracing::warn!("failed to read assets.jsonl: {}", e);
            return graph;
        }
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<AssetLine>(trimmed) {
            Ok(asset_line) => graph.insert(asset_line.asset),
            Err(e) => tracing::warn!("failed to parse asset line: {}", e),
        }
    }

    graph
}

/// The label an asset gets when its agent names none: the kind's label
/// template rendered over the locator, else a plain description of the
/// locator, else the kind id itself.
pub(crate) fn default_label(
    kind: &str,
    template: Option<&str>,
    locator: &crate::asset::Locator,
) -> String {
    template
        .and_then(|t| locator.render_label(t))
        .unwrap_or_else(|| {
            let described = locator.describe();
            if described.is_empty() {
                kind.to_string()
            } else {
                described
            }
        })
}

/// Register `asset` in the append-only asset store.
///
/// The store folds last-write-wins on the WHOLE record, so a bare
/// `Asset::new` would erase what an earlier pass recorded (a coverage `depth`,
/// attributes, an agent-given label). The new record is therefore merged over
/// the existing one first, and nothing is appended when that changes nothing,
/// so a hundred findings on one function leave one line, not a hundred.
///
/// `depth` is the one field a caller may SET: `Some(d)` records `d`, `None`
/// keeps whatever depth the asset already has (a finding must never reset it).
pub(crate) fn upsert_asset(
    paths: &crate::ledger::CoveragePaths,
    mut asset: Asset,
    label_given: bool,
    depth: Option<String>,
    attribution: &Attribution,
) -> std::io::Result<()> {
    let graph = read_asset_graph(paths);
    let existing = graph.get(&asset.id);
    if let Some(old) = existing {
        if !label_given {
            asset.label = old.label.clone();
        }
        if asset.parent.is_none() {
            asset.parent = old.parent.clone();
        }
        asset.depth = depth.or_else(|| old.depth.clone());
        asset.attributes = old.attributes.clone();
    } else {
        asset.depth = depth;
    }
    if existing == Some(&asset) {
        return Ok(());
    }
    append_asset(paths, &asset, attribution)
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
        append_asset(&paths, &child_updated, &attribution).expect("reinsert append should succeed");

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

    #[test]
    fn iter_and_from_assets_preserve_order_and_no_duplicate_on_reinsert() {
        // Create three assets
        let a1 = Asset::new(
            "binary:binary",
            Locator(vec![Coordinate::Sha256("aaa".into())]),
            "a1",
        );
        let a2 = Asset::new(
            "binary:function",
            Locator(vec![Coordinate::Address(0x1000)]),
            "a2",
        );
        let a3 = Asset::new(
            "binary:function",
            Locator(vec![Coordinate::Address(0x2000)]),
            "a3",
        );

        let ids = (a1.id.clone(), a2.id.clone(), a3.id.clone());

        // Insert in order
        let graph = crate::asset::AssetGraph::from_assets(vec![a1, a2, a3]);

        // Verify iteration preserves insertion order
        let iter_ids: Vec<_> = graph.iter().map(|a| a.id.clone()).collect();
        assert_eq!(iter_ids.len(), 3);
        assert_eq!(&iter_ids[0], &ids.0);
        assert_eq!(&iter_ids[1], &ids.1);
        assert_eq!(&iter_ids[2], &ids.2);

        // Re-insert middle asset with new label
        let mut graph = graph;
        let a2_updated = Asset::new(
            "binary:function",
            Locator(vec![Coordinate::Address(0x1000)]),
            "a2_updated",
        );
        graph.insert(a2_updated);

        // Verify no duplication: still 3 items
        let iter_ids: Vec<_> = graph.iter().map(|a| a.id.clone()).collect();
        assert_eq!(
            iter_ids.len(),
            3,
            "re-inserted asset should not create duplicate"
        );
        assert_eq!(
            &iter_ids[1], &ids.1,
            "re-inserted item stays in original position"
        );
    }
}
