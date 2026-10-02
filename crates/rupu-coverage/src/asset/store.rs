//! Append-only asset store: one [`Asset`] per line in `assets.jsonl`, folded
//! on read (last line for an id wins). Append is the only mutation, so a run
//! that re-marks an asset simply writes a newer line.

use super::Asset;
use std::io::Write;
use std::path::Path;

/// Errors reading or writing the asset store.
#[derive(Debug, thiserror::Error)]
pub enum AssetStoreError {
    #[error("asset store io: {0}")]
    Io(#[from] std::io::Error),
    #[error("asset store parse (line {line}): {source}")]
    Parse {
        line: usize,
        source: serde_json::Error,
    },
}

/// Fold a sequence of recorded asset lines into the current set: later entries
/// for the same id replace earlier ones; first-appearance order is kept.
pub fn from_assets<I: IntoIterator<Item = Asset>>(assets: I) -> Vec<Asset> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, Asset> = std::collections::HashMap::new();
    for a in assets {
        if !by_id.contains_key(&a.id) {
            order.push(a.id.clone());
        }
        by_id.insert(a.id.clone(), a);
    }
    order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect()
}

/// Read and fold the asset store at `path`. A missing file is an empty set.
pub fn read_assets(path: &Path) -> Result<Vec<Asset>, AssetStoreError> {
    Ok(from_assets(read_asset_lines(path)?))
}

/// Every line recorded at `path`, in order and unfolded — the superseded
/// states [`read_assets`] folds away. A missing file is empty.
pub(crate) fn read_asset_lines(path: &Path) -> Result<Vec<Asset>, AssetStoreError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AssetStoreError::Io(e)),
    };
    let mut parsed = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let asset: Asset = serde_json::from_str(line).map_err(|source| AssetStoreError::Parse {
            line: i + 1,
            source,
        })?;
        parsed.push(asset);
    }
    Ok(parsed)
}

/// Append one asset as a JSON line, creating the parent directory and file as
/// needed. Upsert semantics come from fold-on-read, not from rewriting.
pub fn upsert_asset(path: &Path, asset: &Asset) -> Result<(), AssetStoreError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_string(asset)
        .map_err(|source| AssetStoreError::Parse { line: 0, source })?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Asset, Coordinate, Locator};
    use super::*;

    fn asset(kind: &str, host: &str, depth: Option<&str>) -> Asset {
        let mut a = Asset::new(
            kind,
            Locator(vec![Coordinate::Host(host.into())]),
            host,
            None,
        );
        a.depth = depth.map(|d| d.to_string());
        a
    }

    #[test]
    fn fold_is_last_write_wins_in_first_appearance_order() {
        let a1 = asset("network:host", "a", Some("discovered"));
        let b = asset("network:host", "b", Some("discovered"));
        let a2 = asset("network:host", "a", Some("tested")); // same id as a1
        let folded = from_assets([a1.clone(), b.clone(), a2.clone()]);
        assert_eq!(folded.len(), 2);
        assert_eq!(folded[0].id, a1.id);
        assert_eq!(folded[0].depth.as_deref(), Some("tested")); // latest wins
        assert_eq!(folded[1].id, b.id);
    }

    #[test]
    fn append_then_read_round_trips_and_folds() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("assets.jsonl");
        assert!(read_assets(&path).unwrap().is_empty()); // missing = empty

        let a1 = asset("network:host", "a", Some("discovered"));
        upsert_asset(&path, &a1).unwrap();
        let b = asset("network:host", "b", None);
        upsert_asset(&path, &b).unwrap();
        let a2 = asset("network:host", "a", Some("tested"));
        upsert_asset(&path, &a2).unwrap();

        let read = read_assets(&path).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].depth.as_deref(), Some("tested"));
        assert!(read.iter().any(|x| x.id == b.id && x.depth.is_none()));

        // The unfolded reader keeps the superseded line the fold drops.
        let lines = read_asset_lines(&path).unwrap();
        assert_eq!(lines, vec![a1, b, a2]);
    }
}
