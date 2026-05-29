use crate::catalog::filter::ConcernFilter;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concern {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default = "default_applicable_globs")]
    pub applicable_globs: Vec<String>,
    #[serde(default = "default_min_strength")]
    pub min_strength: TouchStrength,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_applicable_globs() -> Vec<String> {
    vec!["**".to_string()]
}

fn default_min_strength() -> TouchStrength {
    TouchStrength::Read
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchStrength {
    Glob,
    Cmd,
    Grep,
    Read,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    #[serde(default = "default_template_version")]
    pub version: u32,
    pub description: String,
    #[serde(default)]
    pub references: Vec<String>,
    pub concerns: Vec<Concern>,
    #[serde(default)]
    pub includes: Vec<String>,
}

fn default_template_version() -> u32 {
    1
}

/// A user-declared concerns block — appears in agent frontmatter or
/// workflow YAML. A list of entries, each either an inline concern or
/// an include of a named template.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConcernsBlock {
    pub entries: Vec<ConcernsEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConcernsEntry {
    Include(IncludeDirective),
    Inline(Concern),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogMode {
    /// Render every concern's full body into the system prompt.
    Full,
    /// Render a one-line summary table; concerns fetched on demand.
    Index,
    /// Auto-select based on total concern count and config threshold.
    #[default]
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncludeDirective {
    pub include: String,
    #[serde(default)]
    pub overrides: Vec<ConcernOverride>,
    #[serde(default)]
    pub mode: CatalogMode,
    #[serde(default)]
    pub filter: Option<ConcernFilter>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernOverride {
    pub id: String,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub applicable_globs: Option<Vec<String>>,
    #[serde(default)]
    pub min_strength: Option<TouchStrength>,
    #[serde(default)]
    pub references: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub description: Option<String>,
}

/// The flattened catalog — what the harness actually uses. All includes
/// resolved, all overrides applied, all duplicates rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlatCatalog {
    pub concerns: Vec<Concern>,
    /// Source-tracking: for each concern_id, where it came from (template name or "inline").
    pub sources: std::collections::BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concern_yaml_round_trip_with_defaults() {
        let yaml = r#"
id: secrets-in-source
name: Secrets in source
description: Find hardcoded credentials.
"#;
        let concern: Concern = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(concern.id, "secrets-in-source");
        assert_eq!(concern.severity, Severity::Medium);
        assert_eq!(concern.applicable_globs, vec!["**".to_string()]);
        assert_eq!(concern.min_strength, TouchStrength::Read);
    }

    #[test]
    fn touch_strength_orders_glob_below_edit() {
        assert!(TouchStrength::Glob < TouchStrength::Read);
        assert!(TouchStrength::Read < TouchStrength::Edit);
    }
}
