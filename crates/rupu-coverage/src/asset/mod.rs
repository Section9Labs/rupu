//! The asset model: typed locator coordinates, the asset graph, and its store.
//!
//! A finding is *about an asset*, and the asset has a profile-namespaced
//! `kind`. The kind drives everything the finding/coverage layer used to
//! hardcode for "code": the locator shape, evidence shape, taxonomy, and what
//! coverage counts. See the engagement-profiles spec.

pub mod coordinate;
pub mod store;

pub use coordinate::{Coordinate, Locator, Proto};
pub use store::{from_assets, read_assets, upsert_asset, AssetStoreError};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A stable id for an asset: `"<kind>:<16-hex of sha256(kind | canonical locator)>"`.
/// Stable across re-runs so the same subject folds to one node.
pub type AssetId = String;

/// A node in the engagement asset graph. A finding is *about* one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    /// Profile-namespaced kind, e.g. `"network:service"`, `"binary:function"`.
    pub kind: String,
    /// Parent in the graph; `None` for a root (a top-level asset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AssetId>,
    pub locator: Locator,
    /// Rendered from the kind's `label` template.
    pub label: String,
    /// Profile-declared coverage depth state (a rung of the kind's ladder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<String>,
    /// Escape hatch for profile-specific data the core need not understand.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

impl Asset {
    /// Derive the stable [`AssetId`] for a `kind` + `locator`.
    pub fn derive_id(kind: &str, locator: &Locator) -> AssetId {
        use sha2::{Digest, Sha256};
        // Canonical locator: sort coordinates by their serialized form so the
        // id does not depend on insertion order.
        let mut parts: Vec<String> = locator
            .0
            .iter()
            .map(|c| serde_json::to_string(c).unwrap_or_default())
            .collect();
        parts.sort();
        let mut hasher = Sha256::new();
        hasher.update(kind.as_bytes());
        hasher.update([0u8]);
        hasher.update(parts.join("\u{1f}").as_bytes());
        let digest = hasher.finalize();
        let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        format!("{kind}:{hex}")
    }

    /// Build an asset with a derived id.
    pub fn new(
        kind: impl Into<String>,
        locator: Locator,
        label: impl Into<String>,
        parent: Option<AssetId>,
    ) -> Self {
        let kind = kind.into();
        let id = Self::derive_id(&kind, &locator);
        Asset {
            id,
            kind,
            parent,
            locator,
            label: label.into(),
            depth: None,
            attributes: BTreeMap::new(),
        }
    }

    /// Take over the graph state `current` (this asset's present fold) holds
    /// and a line built from just a kind and locator does not: its parent,
    /// depth and attributes. The store folds last-line-wins, so a line that
    /// omits them erases them; a writer that only means to restate the asset
    /// (a finding's stamp) or advance one field (a depth mark) carries the
    /// rest forward with this. The label stays the writer's own.
    pub(crate) fn carry_state_from(&mut self, current: Asset) {
        debug_assert_eq!(self.id, current.id, "state carries between one asset's lines");
        self.parent = current.parent;
        self.depth = current.depth;
        self.attributes = current.attributes;
    }
}

/// `"network:service"` → `"network"`; a bare kind with no namespace maps to
/// itself.
pub fn profile_of(namespaced_kind: &str) -> &str {
    namespaced_kind
        .split_once(':')
        .map(|(p, _)| p)
        .unwrap_or(namespaced_kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_stable_and_order_independent() {
        let a = Locator(vec![
            Coordinate::Host("h".into()),
            Coordinate::Port {
                number: 22,
                proto: Proto::Tcp,
            },
        ]);
        let b = Locator(vec![
            Coordinate::Port {
                number: 22,
                proto: Proto::Tcp,
            },
            Coordinate::Host("h".into()),
        ]);
        assert_eq!(
            Asset::derive_id("network:service", &a),
            Asset::derive_id("network:service", &b),
            "id must not depend on coordinate order"
        );
        assert_ne!(
            Asset::derive_id("network:service", &a),
            Asset::derive_id("network:host", &a),
            "kind is part of the id"
        );
        assert!(Asset::derive_id("network:service", &a).starts_with("network:service:"));
    }

    #[test]
    fn profile_of_splits_the_namespace() {
        assert_eq!(profile_of("network:service"), "network");
        assert_eq!(profile_of("binary:function"), "binary");
        assert_eq!(profile_of("code"), "code");
    }
}
