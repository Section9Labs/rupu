use crate::asset::{Asset, AssetId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetGraph {
    /// Insertion-preserving map keyed by id (last write wins on re-insert).
    nodes: BTreeMap<String, Asset>,
    order: Vec<String>,
}

impl AssetGraph {
    pub fn insert(&mut self, asset: Asset) {
        if !self.nodes.contains_key(&asset.id.0) {
            self.order.push(asset.id.0.clone());
        }
        self.nodes.insert(asset.id.0.clone(), asset);
    }

    pub fn get(&self, id: &AssetId) -> Option<&Asset> {
        self.nodes.get(&id.0)
    }

    pub fn roots(&self) -> Vec<&Asset> {
        self.order
            .iter()
            .filter_map(|k| self.nodes.get(k))
            .filter(|a| a.parent.is_none())
            .collect()
    }

    pub fn children(&self, parent: &AssetId) -> Vec<&Asset> {
        self.order
            .iter()
            .filter_map(|k| self.nodes.get(k))
            .filter(|a| a.parent.as_ref() == Some(parent))
            .collect()
    }

    pub fn set_depth(&mut self, id: &AssetId, depth: impl Into<String>) -> bool {
        match self.nodes.get_mut(&id.0) {
            Some(a) => {
                a.depth = Some(depth.into());
                true
            }
            None => false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Asset> {
        self.order
            .iter()
            .filter_map(|k| self.nodes.get(k))
    }

    pub fn from_assets(it: impl IntoIterator<Item = Asset>) -> Self {
        let mut graph = AssetGraph::default();
        for asset in it {
            graph.insert(asset);
        }
        graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Asset, Coordinate, Locator};

    #[test]
    fn builds_tree_and_updates_depth() {
        let root = Asset::new(
            "binary:binary",
            Locator(vec![Coordinate::Sha256("ab".into())]),
            "blob",
        );
        let mut child = Asset::new(
            "binary:function",
            Locator(vec![Coordinate::Address(0x1000)]),
            "f",
        );
        child.parent = Some(root.id.clone());
        let (rid, cid) = (root.id.clone(), child.id.clone());

        let mut g = AssetGraph::default();
        g.insert(root);
        g.insert(child);

        assert_eq!(g.roots().len(), 1);
        assert_eq!(g.children(&rid).len(), 1);
        assert!(g.set_depth(&cid, "analyzed"));
        assert_eq!(g.get(&cid).unwrap().depth.as_deref(), Some("analyzed"));
    }
}
