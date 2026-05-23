use crate::catalog::types::Template;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("yaml error in {path}: {source}")]
    Yaml {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },
}

pub fn parse_template_str(yaml: &str, source_label: &str) -> Result<Template, ParseError> {
    serde_yaml::from_str(yaml).map_err(|source| ParseError::Yaml {
        path: source_label.to_string(),
        source,
    })
}

pub fn parse_template_file(path: &Path) -> Result<Template, ParseError> {
    let yaml = std::fs::read_to_string(path).map_err(|source| ParseError::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_template_str(&yaml, &path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE_FIXTURE: &str = r#"
name: stride
version: 1
description: STRIDE threat modeling categories
references:
  - https://learn.microsoft.com/en-us/azure/security/develop/threat-modeling-tool-threats

concerns:
  - id: stride:spoofing
    name: Spoofing
    description: Identity-verification threats.
    severity: high
  - id: stride:tampering
    name: Tampering
    description: Data-integrity threats.
    severity: high
"#;

    #[test]
    fn parse_stride_fixture() {
        let template = parse_template_str(STRIDE_FIXTURE, "stride.yaml").unwrap();
        assert_eq!(template.name, "stride");
        assert_eq!(template.concerns.len(), 2);
        assert_eq!(template.concerns[0].id, "stride:spoofing");
        assert_eq!(template.concerns[1].name, "Tampering");
    }

    #[test]
    fn parse_template_missing_required_field_errors() {
        let bad = r#"
name: missing-description
concerns: []
"#;
        let err = parse_template_str(bad, "bad.yaml").unwrap_err();
        assert!(matches!(err, ParseError::Yaml { .. }));
    }
}
