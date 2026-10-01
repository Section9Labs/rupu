//! Verifies the full Plan 2 flow: include cwe-research with a filter,
//! flatten, auto-select index mode (because catalog > threshold),
//! render prompt section, search by query, fetch details by id.

use rupu_coverage::{
    coverage_concerns_detail, coverage_concerns_search, flatten, render_prompt_section,
    CatalogMode, ConcernFilter, ConcernsBlock, ConcernsEntry, CoverageConcernsDetailInput,
    CoverageConcernsSearchInput, IncludeDirective, Severity, DEFAULT_FULL_MODE_THRESHOLD,
};

#[test]
fn cwe_research_in_index_mode_supports_search_and_detail() {
    let block = ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "cwe-research".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto, // auto-picks Index for ~944 entries
            filter: Some(ConcernFilter {
                severity: vec![Severity::Critical, Severity::High],
                ..Default::default()
            }),
        })],
    };
    let cat = flatten(&block).unwrap();

    // After severity filter, expect a meaningful but smaller subset.
    // CWE-research has many high/critical entries — likely 100-400.
    assert!(
        cat.concerns.len() > 50,
        "expected >50 high/critical concerns, got {}",
        cat.concerns.len()
    );

    // Catalog far exceeds the threshold → index mode.
    let prompt = render_prompt_section(&cat, DEFAULT_FULL_MODE_THRESHOLD);
    assert!(prompt.contains("## Coverage Catalog (index)"));
    assert!(prompt.contains("coverage_concerns_search"));
    assert!(prompt.contains("coverage_concerns_detail"));

    // Search for "injection" — should surface multiple injection CWEs.
    let results = coverage_concerns_search(
        &cat,
        CoverageConcernsSearchInput {
            query: Some("injection".to_string()),
            limit: 50,
            ..Default::default()
        },
    );
    assert!(!results.is_empty(), "expected injection results");

    // Fetch details for the first result. The slug includes the full
    // hyphenated name (data-dependent), so don't hardcode the id — pull
    // it from the search result.
    let first_id = match results.first().unwrap() {
        rupu_coverage::SearchResult::Summary(s) => s.concern_id.clone(),
        rupu_coverage::SearchResult::Full(c) => c.id.clone(),
    };
    let detail = coverage_concerns_detail(
        &cat,
        CoverageConcernsDetailInput {
            concern_ids: vec![first_id.clone()],
        },
    );
    assert_eq!(detail.concerns.len(), 1);
    assert!(
        detail.concerns[0]
            .references
            .iter()
            .any(|r| r.contains("cwe.mitre.org")),
        "expected a cwe.mitre.org reference"
    );
    assert!(detail.not_found.is_empty());
}
