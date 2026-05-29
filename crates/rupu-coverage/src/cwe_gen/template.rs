use crate::catalog::types::{Concern, Template};

/// Assemble a Template struct ready for YAML serialization.
pub fn build_template(name: &str, view_name: &str, concerns: Vec<Concern>) -> Template {
    Template {
        name: name.to_string(),
        version: 1,
        description: format!("CWE {view_name} view, generated from MITRE CWE XML"),
        references: vec!["https://cwe.mitre.org/data/downloads.html".to_string()],
        concerns,
        includes: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{Severity, TouchStrength};

    #[test]
    fn build_template_sets_metadata_and_concerns() {
        let concerns = vec![Concern {
            id: "cwe-research:cwe-787-out-of-bounds-write".to_string(),
            name: "CWE-787 — Out-of-bounds Write".to_string(),
            description: "desc".to_string(),
            severity: Severity::Critical,
            applicable_globs: vec!["**/*.c".to_string()],
            min_strength: TouchStrength::Read,
            references: vec![],
            tags: vec![],
        }];
        let t = build_template("cwe-research", "Research", concerns);
        assert_eq!(t.name, "cwe-research");
        assert_eq!(t.version, 1);
        assert!(t.description.contains("Research"));
        assert_eq!(t.concerns.len(), 1);
        assert!(t.includes.is_empty());
    }
}
