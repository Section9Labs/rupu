use crate::catalog::types::{CatalogMode, FlatCatalog};
use std::collections::BTreeMap;

/// Default threshold for auto-selecting full vs index mode. A catalog
/// with more than this many concerns auto-renders in index mode unless
/// explicitly overridden per-include. Tunable via
/// `[coverage].full_mode_max_concerns` in config.toml; the agent runner
/// pulls that value separately.
pub const DEFAULT_FULL_MODE_THRESHOLD: usize = 80;

/// Resolve each concern's `CatalogMode::Auto` into a concrete `Full`
/// or `Index` choice based on total catalog size and threshold.
/// Explicit per-include `Full`/`Index` choices are preserved.
pub fn resolve_modes(
    catalog: &FlatCatalog,
    full_mode_max_concerns: usize,
) -> BTreeMap<String, CatalogMode> {
    let total = catalog.concerns.len();
    let auto_choice = if total > full_mode_max_concerns {
        CatalogMode::Index
    } else {
        CatalogMode::Full
    };

    catalog
        .concerns
        .iter()
        .map(|c| {
            let requested = catalog
                .render_modes
                .get(&c.id)
                .copied()
                .unwrap_or(CatalogMode::Auto);
            let resolved = match requested {
                CatalogMode::Auto => auto_choice,
                explicit => explicit,
            };
            (c.id.clone(), resolved)
        })
        .collect()
}

/// Convenience: partition the catalog's concerns into
/// `(full_concerns, index_concerns)` by resolved mode.
pub fn partition_by_mode(
    catalog: &FlatCatalog,
    full_mode_max_concerns: usize,
) -> (
    Vec<&crate::catalog::types::Concern>,
    Vec<&crate::catalog::types::Concern>,
) {
    let modes = resolve_modes(catalog, full_mode_max_concerns);
    let mut full = Vec::new();
    let mut index = Vec::new();
    for c in &catalog.concerns {
        match modes.get(&c.id).copied().unwrap_or(CatalogMode::Full) {
            CatalogMode::Index => index.push(c),
            // Full and (defensively) Auto both render full.
            _ => full.push(c),
        }
    }
    (full, index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};

    fn stride_block(mode: CatalogMode) -> ConcernsBlock {
        ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode,
                filter: None,
            })],
        }
    }

    #[test]
    fn small_catalog_auto_picks_full() {
        let cat = flatten(&stride_block(CatalogMode::Auto)).unwrap();
        let modes = resolve_modes(&cat, 80);
        assert!(modes.values().all(|m| *m == CatalogMode::Full));
    }

    #[test]
    fn explicit_index_overrides_auto_for_small_catalogs() {
        let cat = flatten(&stride_block(CatalogMode::Index)).unwrap();
        let modes = resolve_modes(&cat, 80);
        assert!(modes.values().all(|m| *m == CatalogMode::Index));
    }

    #[test]
    fn large_catalog_auto_picks_index() {
        let cat = flatten(&stride_block(CatalogMode::Auto)).unwrap();
        // Force "large" with a very low threshold.
        let modes = resolve_modes(&cat, 3);
        assert!(modes.values().all(|m| *m == CatalogMode::Index));
    }

    #[test]
    fn partition_separates_full_and_index() {
        let cat = flatten(&stride_block(CatalogMode::Index)).unwrap();
        let (full, index) = partition_by_mode(&cat, 80);
        assert_eq!(full.len(), 0);
        assert_eq!(index.len(), 6);
    }
}
