use crate::asset::Locator;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    /// Profile-namespaced kind id, e.g. `binary:function`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<AssetId>,
    pub locator: Locator,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

impl Asset {
    pub fn new(kind: impl Into<String>, locator: Locator, label: impl Into<String>) -> Self {
        let kind = kind.into();
        let mut h = Sha256::new();
        h.update(kind.as_bytes());
        h.update([0u8]);
        // Locator serialization is deterministic (ordered Vec) ⇒ stable id.
        h.update(serde_json::to_vec(&locator).expect("locator serializes"));
        let hex = format!("{:x}", h.finalize());
        let id = AssetId(format!("ast_{}", &hex[..16]));
        Asset {
            id,
            kind,
            parent: None,
            locator,
            label: label.into(),
            depth: None,
            attributes: BTreeMap::new(),
        }
    }
}

/// The owning profile of a namespaced kind (`binary:function` ⇒ `binary`).
pub fn profile_of(kind: &str) -> &str {
    kind.split_once(':').map(|(p, _)| p).unwrap_or(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::Coordinate;

    #[test]
    fn id_is_stable_and_namespace_parses() {
        let loc = Locator(vec![Coordinate::Address(0x401000), Coordinate::Symbol("main".into())]);
        let a = Asset::new("binary:function", loc.clone(), "main @ 0x401000");
        let b = Asset::new("binary:function", loc, "main @ 0x401000");
        assert_eq!(a.id, b.id, "same kind+locator ⇒ same id");
        assert_eq!(profile_of("binary:function"), "binary");
        assert_eq!(profile_of("code"), "code");
    }
}
