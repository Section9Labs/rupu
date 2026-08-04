use crate::catalog::types::{Concern, FlatCatalog};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageConcernsDetailInput {
    pub concern_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageConcernsDetailOutput {
    pub concerns: Vec<Concern>,
    pub not_found: Vec<String>,
}

pub fn coverage_concerns_detail(
    catalog: &FlatCatalog,
    input: CoverageConcernsDetailInput,
) -> CoverageConcernsDetailOutput {
    let mut concerns = Vec::new();
    let mut not_found = Vec::new();
    for id in &input.concern_ids {
        match catalog.concerns.iter().find(|c| &c.id == id) {
            Some(c) => concerns.push(c.clone()),
            None => not_found.push(id.clone()),
        }
    }
    CoverageConcernsDetailOutput { concerns, not_found }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{CatalogMode, ConcernsBlock, ConcernsEntry, IncludeDirective};

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
    fn returns_concerns_by_id() {
        let cat = stride_catalog();
        let out = coverage_concerns_detail(
            &cat,
            CoverageConcernsDetailInput {
                concern_ids: vec![
                    "stride:spoofing".to_string(),
                    "stride:tampering".to_string(),
                ],
            },
        );
        assert_eq!(out.concerns.len(), 2);
        assert!(out.not_found.is_empty());
    }

    #[test]
    fn reports_unknown_ids() {
        let cat = stride_catalog();
        let out = coverage_concerns_detail(
            &cat,
            CoverageConcernsDetailInput {
                concern_ids: vec![
                    "stride:spoofing".to_string(),
                    "stride:not-real".to_string(),
                ],
            },
        );
        assert_eq!(out.concerns.len(), 1);
        assert_eq!(out.not_found, vec!["stride:not-real".to_string()]);
    }
}
