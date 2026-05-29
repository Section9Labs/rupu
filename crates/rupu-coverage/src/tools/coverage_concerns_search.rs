use crate::catalog::filter::ConcernFilter;
use crate::catalog::text::first_sentence;
use crate::catalog::types::{Concern, FlatCatalog};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchResultForm {
    /// Per-concern record carries id, name, severity, one-line summary only.
    #[default]
    Summary,
    /// Per-concern record carries the full body.
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageConcernsSearchInput {
    /// Case-insensitive substring match against name + description + id.
    #[serde(default)]
    pub query: Option<String>,
    /// Optional filter — same shape as the include directive's filter.
    #[serde(default)]
    pub filter: Option<ConcernFilter>,
    /// Maximum results. Defaults to 20.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Summary (default) or full record.
    #[serde(default)]
    pub form: SearchResultForm,
}

fn default_limit() -> usize {
    20
}

impl Default for CoverageConcernsSearchInput {
    fn default() -> Self {
        Self {
            query: None,
            filter: None,
            limit: default_limit(),
            form: SearchResultForm::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultSummary {
    pub concern_id: String,
    pub name: String,
    pub severity: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SearchResult {
    Summary(SearchResultSummary),
    Full(Concern),
}

pub fn coverage_concerns_search(
    catalog: &FlatCatalog,
    input: CoverageConcernsSearchInput,
) -> Vec<SearchResult> {
    let needle = input.query.as_deref().map(|q| q.to_lowercase());
    let empty_filter = ConcernFilter::default();
    let filter = input.filter.as_ref().unwrap_or(&empty_filter);

    catalog
        .concerns
        .iter()
        .filter(|c| {
            if let Some(n) = &needle {
                let in_id = c.id.to_lowercase().contains(n);
                let in_name = c.name.to_lowercase().contains(n);
                let in_desc = c.description.to_lowercase().contains(n);
                if !(in_id || in_name || in_desc) {
                    return false;
                }
            }
            filter.matches(c)
        })
        .take(input.limit)
        .map(|c| match input.form {
            SearchResultForm::Summary => SearchResult::Summary(SearchResultSummary {
                concern_id: c.id.clone(),
                name: c.name.clone(),
                severity: severity_label(c.severity),
                summary: first_sentence(&c.description),
            }),
            SearchResultForm::Full => SearchResult::Full(c.clone()),
        })
        .collect()
}

fn severity_label(s: crate::catalog::types::Severity) -> String {
    use crate::catalog::types::Severity::*;
    match s {
        Info => "info",
        Low => "low",
        Medium => "medium",
        High => "high",
        Critical => "critical",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{
        CatalogMode, ConcernsBlock, ConcernsEntry, IncludeDirective, Severity,
    };

    fn stride_catalog() -> FlatCatalog {
        flatten(&ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: CatalogMode::Index,
                filter: None,
            })],
        })
        .unwrap()
    }

    #[test]
    fn query_substring_matches_name_or_description() {
        let cat = stride_catalog();
        let results = coverage_concerns_search(
            &cat,
            CoverageConcernsSearchInput {
                query: Some("spoofing".to_string()),
                ..Default::default()
            },
        );
        assert!(results.iter().any(|r| matches!(r, SearchResult::Summary(s) if s.concern_id == "stride:spoofing")));
    }

    #[test]
    fn filter_subset_applies() {
        let cat = stride_catalog();
        let results = coverage_concerns_search(
            &cat,
            CoverageConcernsSearchInput {
                filter: Some(ConcernFilter {
                    severity: vec![Severity::Critical],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn empty_query_and_filter_returns_all_up_to_limit() {
        let cat = stride_catalog();
        let results = coverage_concerns_search(&cat, CoverageConcernsSearchInput::default());
        assert_eq!(results.len(), 6);
    }

    #[test]
    fn full_form_returns_complete_concern() {
        let cat = stride_catalog();
        let results = coverage_concerns_search(
            &cat,
            CoverageConcernsSearchInput {
                query: Some("spoofing".to_string()),
                form: SearchResultForm::Full,
                ..Default::default()
            },
        );
        match results.first() {
            Some(SearchResult::Full(c)) => assert_eq!(c.id, "stride:spoofing"),
            _ => panic!("expected Full variant"),
        }
    }
}
