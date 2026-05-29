use crate::catalog::types::{Concern, Severity};
use serde::{Deserialize, Serialize};

/// Per-include subset selector. All declared filters apply with AND
/// semantics. An empty filter (`None` on the include, or
/// `ConcernFilter::default()`) is a no-op.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernFilter {
    /// Keep only concerns whose severity is in this set.
    #[serde(default)]
    pub severity: Vec<Severity>,
    /// Keep only concerns whose tags include all of these.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Keep only concerns whose id matches at least one of these glob
    /// patterns (e.g. "cwe-research:cwe-2[0-9][0-9]-*").
    #[serde(default)]
    pub ids: Vec<String>,
    /// Keep only concerns whose `applicable_globs` match this path.
    #[serde(default)]
    pub applicable_to_path: Option<String>,
}

impl ConcernFilter {
    /// Returns true when the filter is empty / no-op.
    pub fn is_empty(&self) -> bool {
        self.severity.is_empty()
            && self.tags.is_empty()
            && self.ids.is_empty()
            && self.applicable_to_path.is_none()
    }

    /// Returns true when `concern` passes all filters.
    pub fn matches(&self, concern: &Concern) -> bool {
        if !self.severity.is_empty() && !self.severity.contains(&concern.severity) {
            return false;
        }
        if !self.tags.is_empty() && !self.tags.iter().all(|t| concern.tags.contains(t)) {
            return false;
        }
        if !self.ids.is_empty() {
            let id_match = self.ids.iter().any(|pat| {
                glob::Pattern::new(pat)
                    .map(|p| p.matches(&concern.id))
                    .unwrap_or(false)
            });
            if !id_match {
                return false;
            }
        }
        if let Some(path) = &self.applicable_to_path {
            let path_match = concern.applicable_globs.iter().any(|g| {
                glob::Pattern::new(g)
                    .map(|p| p.matches(path))
                    .unwrap_or(false)
            });
            if !path_match {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{Severity, TouchStrength};

    fn concern(id: &str, sev: Severity, tags: &[&str], globs: &[&str]) -> Concern {
        Concern {
            id: id.to_string(),
            name: id.to_string(),
            description: "x".to_string(),
            severity: sev,
            applicable_globs: globs.iter().map(|s| s.to_string()).collect(),
            min_strength: TouchStrength::Read,
            references: vec![],
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn empty_filter_is_empty_and_matches_everything() {
        let f = ConcernFilter::default();
        assert!(f.is_empty());
        assert!(f.matches(&concern("a", Severity::Low, &[], &["**"])));
    }

    #[test]
    fn severity_filter_keeps_only_listed() {
        let f = ConcernFilter {
            severity: vec![Severity::High, Severity::Critical],
            ..Default::default()
        };
        assert!(f.matches(&concern("a", Severity::High, &[], &["**"])));
        assert!(!f.matches(&concern("b", Severity::Low, &[], &["**"])));
    }

    #[test]
    fn tags_filter_requires_all() {
        let f = ConcernFilter {
            tags: vec!["lang:rust".to_string(), "owasp".to_string()],
            ..Default::default()
        };
        assert!(f.matches(&concern("a", Severity::High, &["lang:rust", "owasp"], &["**"])));
        assert!(!f.matches(&concern("b", Severity::High, &["lang:rust"], &["**"])));
    }

    #[test]
    fn ids_filter_matches_glob() {
        let f = ConcernFilter {
            ids: vec!["cwe-*:cwe-78-*".to_string()],
            ..Default::default()
        };
        assert!(f.matches(&concern(
            "cwe-research:cwe-78-os-command-injection",
            Severity::Critical,
            &[],
            &["**"],
        )));
        assert!(!f.matches(&concern(
            "cwe-research:cwe-79-xss",
            Severity::High,
            &[],
            &["**"],
        )));
    }

    #[test]
    fn applicable_to_path_keeps_only_matching_globs() {
        let f = ConcernFilter {
            applicable_to_path: Some("src/db/queries.rs".to_string()),
            ..Default::default()
        };
        assert!(f.matches(&concern("a", Severity::High, &[], &["src/db/**"])));
        assert!(!f.matches(&concern("b", Severity::High, &[], &["src/handlers/**"])));
    }

    #[test]
    fn multiple_filters_apply_with_and_semantics() {
        let f = ConcernFilter {
            severity: vec![Severity::Critical],
            tags: vec!["security".to_string()],
            ..Default::default()
        };
        assert!(f.matches(&concern("a", Severity::Critical, &["security"], &["**"])));
        assert!(!f.matches(&concern("b", Severity::Critical, &[], &["**"])));
        assert!(!f.matches(&concern("c", Severity::Low, &["security"], &["**"])));
    }
}
