//! SCM and issue-tracker configuration. Spec §7c.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScmSection {
    pub default: Option<ScmDefault>,
    /// Per-platform overrides: `[scm.github]`, `[scm.gitlab]`.
    /// Keyed by lower-case platform name.
    #[serde(flatten, with = "platforms_serde")]
    pub platforms: BTreeMap<String, ScmPlatformConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssuesSection {
    pub default: Option<IssuesDefault>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmDefault {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuesDefault {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmPlatformConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
    /// "https" or "ssh"; default chosen by the connector at clone time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_protocol: Option<String>,
}

mod platforms_serde {
    //! Serialize/deserialize `BTreeMap<String, ScmPlatformConfig>` as
    //! flattened sub-tables, but EXCLUDING the reserved `default` key
    //! (which is its own typed field on `ScmSection`).
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::ScmPlatformConfig;

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<String, ScmPlatformConfig>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        map.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, ScmPlatformConfig>, D::Error> {
        let mut raw: BTreeMap<String, ScmPlatformConfig> = BTreeMap::deserialize(d)?;
        // Drop the reserved key if it slipped through (it's typed
        // separately on ScmSection.default).
        raw.remove("default");
        Ok(raw)
    }
}
