use crate::asset::Coordinate;
use crate::profile::predicate::CompletenessCheck;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("profile TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("kind {kind:?} names unknown coordinate {tag:?}")]
    UnknownCoordinate { kind: String, tag: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetKindDef {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub coordinates: Vec<String>,
    #[serde(default)]
    pub attributes: Vec<String>,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSpec {
    #[serde(default)]
    pub enumerates: Vec<String>,
    #[serde(default)]
    pub depth_ladder: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngagementProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub asset_kinds: Vec<AssetKindDef>,
    #[serde(default)]
    pub evidence_blocks: Vec<String>,
    #[serde(default)]
    pub classification_systems: Vec<String>,
    #[serde(default)]
    pub completeness: Vec<CompletenessCheck>,
    #[serde(default)]
    pub coverage: CoverageSpec,
    #[serde(default)]
    pub bundle: Bundle,
}

pub fn parse_profile(src: &str) -> Result<EngagementProfile, ProfileError> {
    let p: EngagementProfile = toml::from_str(src)?;
    for k in &p.asset_kinds {
        for tag in &k.coordinates {
            if !Coordinate::known_tag(tag) {
                return Err(ProfileError::UnknownCoordinate {
                    kind: k.id.clone(),
                    tag: tag.clone(),
                });
            }
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINARY: &str = include_str!("builtin/binary.toml");

    #[test]
    fn parses_the_builtin_binary_profile() {
        let p = parse_profile(BINARY).unwrap();
        assert_eq!(p.id, "binary");
        assert!(p
            .asset_kinds
            .iter()
            .any(|k| k.id == "function" && k.coordinates.contains(&"address".to_string())));
        assert_eq!(
            p.coverage.depth_ladder.first().map(String::as_str),
            Some("located")
        );
    }

    #[test]
    fn the_builtin_binary_profile_permits_its_blocks_and_systems() {
        // Top-level keys written after a `[[asset_kinds]]` header would be
        // swallowed by that table and silently dropped: pin that they landed.
        let p = parse_profile(BINARY).unwrap();
        assert_eq!(
            p.evidence_blocks,
            ["text", "code_slice", "diff", "hexdump", "disasm"]
        );
        assert_eq!(p.classification_systems, ["CWE", "CVE"]);
    }

    #[test]
    fn unknown_coordinate_is_rejected() {
        let bad = r#"
id = "x"
name = "x"
[[asset_kinds]]
id = "k"
coordinates = ["nonsense"]
label = "l"
[coverage]
enumerates = ["k"]
depth_ladder = ["a"]
[bundle]
"#;
        assert!(matches!(
            parse_profile(bad),
            Err(ProfileError::UnknownCoordinate { .. })
        ));
    }
}
