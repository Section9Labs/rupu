//! Level-1 determinism contract (Slice B Plan 2).
//!
//! Locks the guarantee that everything the coverage harness controls
//! about the model's view is byte-stable and independent of the order
//! catalog inputs are declared in. If any of these fail, prompt
//! construction has become nondeterministic and run-to-run diffs would
//! conflate harness variance with model variance.

use rupu_coverage::{
    flatten, render_prompt_section, write_snapshot, CatalogMode, ConcernsBlock, ConcernsEntry,
    IncludeDirective, DEFAULT_FULL_MODE_THRESHOLD,
};

/// A `ConcernsBlock` that includes `a` then `b`.
fn block_two_includes(a: &str, b: &str) -> ConcernsBlock {
    ConcernsBlock {
        entries: vec![
            ConcernsEntry::Include(IncludeDirective {
                include: a.to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            }),
            ConcernsEntry::Include(IncludeDirective {
                include: b.to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            }),
        ],
    }
}

#[test]
fn render_is_byte_stable_across_repeated_calls() {
    // A purity assertion (no I/O, no global state, no RNG), not a
    // runtime-nondeterminism guard. The substantive order-independence
    // proofs are the two tests below.
    let catalog = flatten(&block_two_includes("stride", "secrets-in-source")).unwrap();
    let first = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
    let second = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
    assert_eq!(
        first, second,
        "render_prompt_section must be a pure function"
    );
}

#[test]
fn render_is_independent_of_include_order() {
    // The SAME logical catalog, declared in two different include orders,
    // must render to identical bytes — proving concern ordering is
    // canonical (sorted by id), not input-order-dependent. Full mode: all
    // 7 concerns are inlined under the default threshold.
    let ab = flatten(&block_two_includes("stride", "secrets-in-source")).unwrap();
    let ba = flatten(&block_two_includes("secrets-in-source", "stride")).unwrap();
    let rendered_ab = render_prompt_section(&ab, DEFAULT_FULL_MODE_THRESHOLD);
    let rendered_ba = render_prompt_section(&ba, DEFAULT_FULL_MODE_THRESHOLD);
    assert_eq!(
        rendered_ab, rendered_ba,
        "render must not depend on the order includes are declared in"
    );
}

#[test]
fn render_is_independent_of_include_order_index_mode() {
    // Same order-independence guarantee for the INDEX-mode render path —
    // the path large CWE catalogs take. A threshold of 0 forces every
    // concern into the one-line index table instead of inlining bodies,
    // exercising the `partition_by_mode` index bucket the full-mode tests
    // never reach.
    let ab = flatten(&block_two_includes("stride", "secrets-in-source")).unwrap();
    let ba = flatten(&block_two_includes("secrets-in-source", "stride")).unwrap();
    let rendered_ab = render_prompt_section(&ab, 0);
    let rendered_ba = render_prompt_section(&ba, 0);
    assert_eq!(
        rendered_ab, rendered_ba,
        "index-mode render must not depend on include order"
    );
}

#[test]
fn catalog_snapshot_is_independent_of_include_order() {
    // The persisted catalog.yaml must also be order-independent so a
    // re-run (B-3) reconstructs an identical effective catalog.
    let ab = flatten(&block_two_includes("stride", "secrets-in-source")).unwrap();
    let ba = flatten(&block_two_includes("secrets-in-source", "stride")).unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    let path_ab = tmp.path().join("ab/catalog.yaml");
    let path_ba = tmp.path().join("ba/catalog.yaml");
    write_snapshot(&ab, &path_ab).unwrap();
    write_snapshot(&ba, &path_ba).unwrap();

    let yaml_ab = std::fs::read_to_string(&path_ab).unwrap();
    let yaml_ba = std::fs::read_to_string(&path_ba).unwrap();
    assert_eq!(
        yaml_ab, yaml_ba,
        "catalog snapshot YAML must not depend on include order"
    );
}

#[test]
fn stride_catalog_render_matches_snapshot() {
    // Pins the exact rendered bytes for a curated catalog. A diff here
    // means the prompt format changed — intentional changes are accepted
    // with `cargo insta review`; unintended ones are caught in review.
    let catalog = flatten(&ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    })
    .unwrap();
    let rendered = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
    insta::assert_snapshot!("stride_catalog_render", rendered);
}
