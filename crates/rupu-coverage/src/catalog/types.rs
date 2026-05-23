use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Default for Severity {
    fn default() -> Self {
        Severity::Medium
    }
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
}

fn default_template_version() -> u32 {
    1
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
