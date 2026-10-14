//! The engagement-profile package format (TOML) and its parser.
//!
//! A profile is pure data. serde `deny_unknown_fields` everywhere so a typo in
//! an authored profile is a loud parse error, never a silently dropped field.
//! Authoring note: profile-level scalar/array keys MUST precede the first
//! `[[asset_kinds]]` table, or TOML attributes them to that kind and the parse
//! fails.

use super::predicate::CompletenessCheck;
use serde::{Deserialize, Serialize};

/// Errors parsing a profile package.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("profile parse: {0}")]
    Parse(#[from] toml::de::Error),
}

/// A complete engagement profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngagementProfile {
    pub id: String,
    pub name: String,
    /// Composite expansion: the profiles this one includes (unioned).
    #[serde(default)]
    pub includes: Vec<String>,
    /// Declared evidence block kinds this engagement expects (advisory).
    #[serde(default)]
    pub evidence_blocks: Vec<String>,
    /// Declared classification systems this engagement uses (advisory).
    #[serde(default)]
    pub classification_systems: Vec<String>,
    #[serde(default)]
    pub asset_kinds: Vec<AssetKindDef>,
    #[serde(default)]
    pub completeness: Vec<CompletenessCheck>,
    #[serde(default)]
    pub coverage: Coverage,
    #[serde(default)]
    pub bundle: Bundle,
}

/// One asset kind a profile declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetKindDef {
    pub id: String,
    /// Parent kind id (within the same profile) — `None` for a root.
    #[serde(default)]
    pub parent: Option<String>,
    /// Coordinate tags an asset of this kind carries.
    #[serde(default)]
    pub coordinates: Vec<String>,
    /// Label template, e.g. `"{host}:{port}"`.
    pub label: String,
}

/// The coverage model: which kinds are enumerated, and the depth ladder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    #[serde(default)]
    pub enumerates: Vec<String>,
    #[serde(default)]
    pub depth_ladder: Vec<String>,
}

/// Launcher prefill only (never a tool or skill grant): the agents, tools,
/// workflows and skills an engagement typically uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
    /// The skills `rupu fleet install` installs for this profile's agents,
    /// each pinned to a commit.
    #[serde(default)]
    pub skills: Vec<BundleSkill>,
}

/// One bundled skill: what `rupu skill install` would be given, pinned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSkill {
    /// The skill's directory name in the source.
    pub name: String,
    /// `owner/repo` or an https git URL.
    pub source: String,
    /// The release the commit belongs to. Shown, never fetched.
    pub tag: String,
    /// What is fetched: a full commit, so a moved tag changes nothing.
    #[serde(deserialize_with = "full_commit")]
    pub commit: String,
}

/// Accept only a full 40-character lowercase hex SHA: a tag or branch can move.
fn full_commit<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(s)
    } else {
        Err(serde::de::Error::custom(format!(
            "commit {s:?} must be a full 40-character lowercase hex SHA; a tag or branch can move"
        )))
    }
}

/// Parse a profile from TOML source.
pub fn parse_profile(toml_src: &str) -> Result<EngagementProfile, ProfileError> {
    Ok(toml::from_str(toml_src)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_profile() {
        let p = parse_profile(
            r#"
id = "network"
name = "Network security assessment"
evidence_blocks = ["scan_output"]
classification_systems = ["CVE"]

[[asset_kinds]]
id = "host"
coordinates = ["host"]
label = "{host}"

[[asset_kinds]]
id = "service"
parent = "host"
coordinates = ["host", "port"]
label = "{host}:{port}"

[[completeness]]
id = "service_identified"
label = "pinned to host + port"
required = true
satisfied_when = { all = [ { locator_has_coordinate = "host" }, { locator_has_coordinate = "port" } ] }

[coverage]
enumerates = ["host", "service"]
depth_ladder = ["discovered", "enumerated"]
"#,
        )
        .unwrap();
        assert_eq!(p.id, "network");
        assert_eq!(p.asset_kinds.len(), 2);
        assert_eq!(p.asset_kinds[1].parent.as_deref(), Some("host"));
        assert_eq!(p.completeness.len(), 1);
        assert_eq!(p.coverage.depth_ladder, ["discovered", "enumerated"]);
    }

    #[test]
    fn unknown_field_is_a_loud_error() {
        let err = parse_profile(
            r#"
id = "x"
name = "x"
bogus = true
"#,
        );
        assert!(err.is_err(), "deny_unknown_fields must reject `bogus`");
    }

    #[test]
    fn top_level_key_after_asset_kinds_is_rejected() {
        // The classic authoring trap: a profile-level key placed AFTER a
        // `[[asset_kinds]]` is attributed to that kind, which rejects it.
        let err = parse_profile(
            r#"
id = "x"
name = "x"

[[asset_kinds]]
id = "k"
label = "l"

classification_systems = ["CWE"]
"#,
        );
        assert!(
            err.is_err(),
            "a key after [[asset_kinds]] must fail to parse"
        );
    }

    #[test]
    fn composite_declares_includes() {
        let p = parse_profile(
            r#"
id = "pentest"
name = "Penetration test"
includes = ["network", "web"]
"#,
        )
        .unwrap();
        assert_eq!(p.includes, ["network", "web"]);
        assert!(p.asset_kinds.is_empty());
    }

    const PIN: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn bundle_skills_parse_with_a_full_commit() {
        let p = parse_profile(&format!(
            r#"
id = "x"
name = "x"

[bundle]
agents = ["recon"]
skills = [
  {{ name = "network-recon", source = "Section9Labs/rupu-skills", tag = "v1.0.0", commit = "{PIN}" }},
]
"#
        ))
        .unwrap();
        assert_eq!(p.bundle.skills.len(), 1);
        let s = &p.bundle.skills[0];
        assert_eq!(s.name, "network-recon");
        assert_eq!(s.source, "Section9Labs/rupu-skills");
        assert_eq!(s.tag, "v1.0.0");
        assert_eq!(s.commit, PIN);
    }

    #[test]
    fn bundle_skills_default_to_empty() {
        let p = parse_profile("id = \"x\"\nname = \"x\"\n[bundle]\nagents = [\"a\"]\n").unwrap();
        assert!(p.bundle.skills.is_empty());
    }

    #[test]
    fn a_bundle_skill_commit_must_be_a_full_lowercase_sha() {
        for bad in [
            "v1.0.0",
            "main",
            "0123456",
            "0123456789ABCDEF0123456789ABCDEF01234567",
            "0123456789abcdef0123456789abcdef0123456z",
        ] {
            let err = parse_profile(&format!(
                "id = \"x\"\nname = \"x\"\n[bundle]\nskills = [{{ name = \"a\", source = \"o/r\", tag = \"t\", commit = \"{bad}\" }}]\n"
            ));
            let msg = format!("{}", err.expect_err(bad));
            assert!(msg.contains("40-character"), "{bad}: {msg}");
        }
    }

    #[test]
    fn a_bundle_skill_rejects_unknown_keys() {
        let err = parse_profile(&format!(
            "id = \"x\"\nname = \"x\"\n[bundle]\nskills = [{{ name = \"a\", source = \"o/r\", tag = \"t\", commit = \"{PIN}\", ref = \"main\" }}]\n"
        ));
        assert!(err.is_err(), "deny_unknown_fields must reject `ref`");
    }
}
