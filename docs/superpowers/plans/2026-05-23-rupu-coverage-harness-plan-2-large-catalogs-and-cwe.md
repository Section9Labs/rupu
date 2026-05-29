# rupu coverage harness — Plan 2: Large catalogs + CWE generator

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add support for catalogs that don't fit in the system prompt: catalog filters (subset large templates), index-mode rendering (one-line-per-concern table), two new agent tools (`coverage_concerns_search` / `coverage_concerns_detail`) for on-demand lookup, and a build-time generator that emits two CWE templates (`cwe-software-development` ~440 entries, `cwe-research` ~930 entries) from MITRE's published CWE XML.

**Architecture:** Filters apply during catalog flatten (per-include subset selection). Render mode (full vs index) is auto-selected from total catalog size, overridable per-include. The two new agent tools query the *snapshot* on disk (not a live catalog instance) so they work uniformly across full and index modes. The CWE generator is a separate binary target (`src/bin/gen_cwe_catalog.rs`), feature-gated under `gen` so non-gen builds don't carry XML/HTTP dependencies.

**Tech Stack:** Rust, `serde_yaml`, `glob` (already in workspace), `quick-xml` (new, feature-gated), `chrono`, `thiserror`. Tests reuse the `tempfile` patterns from Plan 1.

**Spec:** `docs/superpowers/specs/2026-05-23-rupu-coverage-harness-design.md`

**Prior plan:** `docs/superpowers/plans/2026-05-23-rupu-coverage-harness-plan-1-foundation-and-curated-catalog.md` (foundation + curated catalog, complete)

**Out of scope for this plan** (deferred):
- `rupu coverage` CLI subcommand + human-readable audit rendering — Plan 3.
- Session-surface integration + `/coverage` slash command + per-target tool-mappings — Plan 3.
- Plan 1 follow-ups (route `coverage_mark`/`report_finding` through async writer; transcript-error shutdown gap; within-run supersede in derived view; workflow-wins integration test; grep/bash integration tests) — these will land as a small "Plan 1.5" follow-up either before or alongside Plan 2 implementation. They do **not** block Plan 2's correctness.
- Real network downloads in the generator — v1 takes a local XML path; humans/CI download the XML separately (see Task 14 for the workflow).

---

## File structure

```
crates/rupu-coverage/Cargo.toml                                (MODIFY)
└── add quick-xml under [features] gen + reqwest if needed; new [[bin]] target

crates/rupu-coverage/src/
├── catalog/
│   ├── filter.rs                                              (NEW)
│   │   # ConcernFilter type + apply_filter helper
│   ├── render.rs                                              (MODIFY)
│   │   # add render_index_mode, render_prompt_section (mixed)
│   ├── flatten.rs                                             (MODIFY)
│   │   # apply per-include filter; track per-concern mode in FlatCatalog
│   ├── types.rs                                               (MODIFY)
│   │   # add mode, filter to IncludeDirective; CatalogMode enum;
│   │   # add render_mode (per-concern) and chosen_mode (per-catalog) to FlatCatalog
│   ├── mode_selection.rs                                      (NEW)
│   │   # auto-selection of full vs index mode given catalog size
│   └── mod.rs                                                 (MODIFY)
├── tools/
│   ├── coverage_concerns_search.rs                            (NEW)
│   ├── coverage_concerns_detail.rs                            (NEW)
│   └── mod.rs                                                 (MODIFY)
└── lib.rs                                                     (MODIFY)
└── bin/
    └── gen_cwe_catalog.rs                                     (NEW, gen feature)

crates/rupu-coverage/templates/concerns/
├── cwe-software-development.yaml                              (NEW, generated)
├── cwe-software-development.version.txt                       (NEW)
├── cwe-research.yaml                                          (NEW, generated)
└── cwe-research.version.txt                                   (NEW)

crates/rupu-coverage/build/cwe/
├── cwec_v4.13.xml                                             (input, downloaded once)
└── README.md                                                  (NEW)
                                                                # how to refresh the
                                                                # template generation
                                                                # for a new MITRE release

crates/rupu-agent/src/coverage_tools.rs                        (MODIFY)
└── register the 2 new search/detail tools alongside the existing 4
```

---

## Task 1: `ConcernFilter` type and parsing

**Files:**
- Create: `crates/rupu-coverage/src/catalog/filter.rs`
- Modify: `crates/rupu-coverage/src/catalog/types.rs` (add `CatalogMode` enum; extend `IncludeDirective` with `mode` and `filter`)
- Modify: `crates/rupu-coverage/src/catalog/mod.rs`
- Test: inline in `filter.rs`

- [ ] **Step 1: Add `CatalogMode` and extend `IncludeDirective` in `types.rs`**

Append to `crates/rupu-coverage/src/catalog/types.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogMode {
    /// Render every concern's full body into the system prompt.
    Full,
    /// Render a one-line summary table; concerns fetched on demand.
    Index,
    /// Auto-select based on total concern count and config threshold.
    Auto,
}

impl Default for CatalogMode {
    fn default() -> Self {
        Self::Auto
    }
}
```

Modify `IncludeDirective`:

```rust
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
```

(`ConcernFilter` is defined in `filter.rs` in Step 2; add the import: `use crate::catalog::filter::ConcernFilter;` at the top of `types.rs` after the existing imports.)

- [ ] **Step 2: Create `crates/rupu-coverage/src/catalog/filter.rs`**

```rust
use crate::catalog::types::{Concern, Severity};
use serde::{Deserialize, Serialize};

/// Per-include subset selector. All declared filters apply with AND
/// semantics. An empty filter (`None` on the include, or `ConcernFilter::default()`)
/// is a no-op.
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
    /// Useful for "review just this directory" scoping.
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
        // Missing one tag — should fail.
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
        // Both must match.
        assert!(f.matches(&concern("a", Severity::Critical, &["security"], &["**"])));
        // Tag missing.
        assert!(!f.matches(&concern("b", Severity::Critical, &[], &["**"])));
        // Severity wrong.
        assert!(!f.matches(&concern("c", Severity::Low, &["security"], &["**"])));
    }
}
```

- [ ] **Step 3: Wire into module**

Edit `crates/rupu-coverage/src/catalog/mod.rs`:

```rust
pub mod builtin;
pub mod filter;
pub mod flatten;
pub mod parse;
pub mod render;
pub mod snapshot;
pub mod types;
pub use builtin::{builtin_names, resolve_builtin};
pub use filter::ConcernFilter;
pub use flatten::{flatten, flatten_with_resolver, FlattenError};
pub use parse::{parse_template_file, parse_template_str, ParseError};
pub use render::render_full_mode;
pub use snapshot::{read_snapshot, write_snapshot, SnapshotError};
pub use types::{
    CatalogMode, Concern, ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog,
    IncludeDirective, Severity, Template, TouchStrength,
};
```

Edit `crates/rupu-coverage/src/lib.rs` to re-export the new types: add `CatalogMode, ConcernFilter` to the existing `pub use catalog::{...}` line.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: 34 prior + 6 new = 40 tests pass. Clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): catalog filters (severity/tags/ids/applicable_to_path) on includes"
```

---

## Task 2: Apply filters during catalog flatten

**Files:**
- Modify: `crates/rupu-coverage/src/catalog/flatten.rs`
- Test: inline in `flatten.rs`

- [ ] **Step 1: Modify `flatten_with_resolver` to apply each include's filter**

In `crates/rupu-coverage/src/catalog/flatten.rs`, after the line that resolves `template_concerns` and after the nested-include expansion but **before** the override-validation block, insert filter application:

```rust
        // Apply per-include filter (subset selector) before overrides
        // and duplicate-id detection. An empty filter is a no-op.
        if let Some(filter) = &directive.filter {
            if !filter.is_empty() {
                template_concerns.retain(|c| filter.matches(c));
            }
        }
```

The full updated block in pass 2 should look like:

```rust
        let template = resolve(&directive.include)?;
        let mut template_concerns = template.concerns.clone();

        // Recurse into nested includes (composite templates).
        for nested_name in &template.includes {
            let nested = resolve(nested_name)?;
            template_concerns.extend(nested.concerns);
        }

        // Apply per-include filter (subset selector) before overrides
        // and duplicate-id detection. An empty filter is a no-op.
        if let Some(filter) = &directive.filter {
            if !filter.is_empty() {
                template_concerns.retain(|c| filter.matches(c));
            }
        }

        // Apply overrides — must target a concern that exists in the
        // (post-filter, post-nested-include) template.
        let template_ids: std::collections::HashSet<&str> = ...
```

- [ ] **Step 2: Add a test in `flatten.rs`**

In the `tests` module, append:

```rust
    #[test]
    fn filter_subsets_included_template() {
        use crate::catalog::filter::ConcernFilter;
        use crate::catalog::types::Severity;

        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: Some(ConcernFilter {
                    severity: vec![Severity::Critical],
                    ..Default::default()
                }),
            })],
        };
        let cat = flatten(&block).unwrap();
        // Only elevation-of-privilege is Critical in stride.
        assert_eq!(cat.concerns.len(), 1);
        assert_eq!(cat.concerns[0].id, "stride:elevation-of-privilege");
    }

    #[test]
    fn empty_filter_is_no_op() {
        use crate::catalog::filter::ConcernFilter;

        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: Some(ConcernFilter::default()),
            })],
        };
        let cat = flatten(&block).unwrap();
        assert_eq!(cat.concerns.len(), 6);
    }
```

The prior tests construct `IncludeDirective` literally with only `include` and `overrides`. They will fail to compile after the new fields are added in Task 1, so update each prior `IncludeDirective { include: ..., overrides: ... }` literal to also include `mode: CatalogMode::Auto, filter: None`. Search:

```bash
grep -rn "IncludeDirective {" crates/rupu-coverage/src/ crates/rupu-coverage/tests/
```

Update each site to add the two new fields with their defaults.

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: 40 prior + 2 new = 42 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): apply per-include ConcernFilter during catalog flatten"
```

---

## Task 3: Track per-concern render mode on `FlatCatalog`

**Files:**
- Modify: `crates/rupu-coverage/src/catalog/types.rs`
- Modify: `crates/rupu-coverage/src/catalog/flatten.rs`
- Test: inline in `flatten.rs`

- [ ] **Step 1: Extend `FlatCatalog` with a per-concern render-mode map**

In `crates/rupu-coverage/src/catalog/types.rs`, modify `FlatCatalog`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlatCatalog {
    pub concerns: Vec<Concern>,
    /// Source-tracking: for each concern_id, where it came from
    /// (template name or "inline").
    pub sources: std::collections::BTreeMap<String, String>,
    /// Requested render mode per concern_id. Reflects the `mode:` on
    /// the include the concern came from (or `Auto` for inline
    /// concerns and for include directives that didn't set `mode:`).
    /// Actual full-vs-index choice happens in mode_selection.rs.
    #[serde(default)]
    pub render_modes: std::collections::BTreeMap<String, CatalogMode>,
}
```

- [ ] **Step 2: Populate `render_modes` during flatten**

In `crates/rupu-coverage/src/catalog/flatten.rs`'s `flatten_with_resolver`:

Add a `render_modes` BTreeMap alongside `by_id` / `sources` at the top of the function:

```rust
    let mut render_modes: std::collections::BTreeMap<String, CatalogMode> = BTreeMap::new();
```

When inserting inline concerns in pass 1:

```rust
        if let ConcernsEntry::Inline(concern) = entry {
            by_id.insert(concern.id.clone(), concern.clone());
            sources.insert(concern.id.clone(), "inline".to_string());
            render_modes.insert(concern.id.clone(), CatalogMode::Auto);
        }
```

When inserting include concerns in pass 2, record the include's mode:

```rust
            by_id.insert(concern.id.clone(), concern.clone());
            sources
                .entry(concern.id.clone())
                .or_insert_with(|| directive.include.clone());
            render_modes
                .entry(concern.id.clone())
                .or_insert(directive.mode);
```

And include in the returned `FlatCatalog`:

```rust
    Ok(FlatCatalog {
        concerns: by_id.into_values().collect(),
        sources,
        render_modes,
    })
```

Import `CatalogMode` at the top of `flatten.rs`:

```rust
use crate::catalog::types::{
    Concern, ConcernsBlock, ConcernsEntry, FlatCatalog, IncludeDirective, Template,
};
// also add:
use crate::catalog::types::CatalogMode;
```

- [ ] **Step 3: Add a test in `flatten.rs`**

```rust
    #[test]
    fn flatten_records_render_mode_per_concern() {
        use crate::catalog::types::CatalogMode;

        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: CatalogMode::Index,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        assert_eq!(cat.render_modes.get("stride:spoofing"), Some(&CatalogMode::Index));
    }
```

The snapshot round-trip test in `snapshot.rs` may need an update — the prior `FlatCatalog` equality check now compares `render_modes` too. The round-trip should still pass (an empty `render_modes` map round-trips as an empty map), but if the test fixture used a catalog without `render_modes` populated, it'll still work because of `#[serde(default)]`. No code change needed there if you're using `serde_yaml::from_str` and `to_string` round-trip.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: 42 prior + 1 new = 43 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): record requested render mode per concern in FlatCatalog"
```

---

## Task 4: `render_index_mode` function

**Files:**
- Modify: `crates/rupu-coverage/src/catalog/render.rs`
- Test: inline in `render.rs`

- [ ] **Step 1: Add `render_index_mode` to `render.rs`**

```rust
/// Render the catalog as a compact one-line-per-concern table for
/// large catalogs. The agent uses `coverage_concerns_search` /
/// `coverage_concerns_detail` to fetch full bodies on demand.
pub fn render_index_mode(catalog: &FlatCatalog) -> String {
    let mut out = String::new();
    out.push_str("## Coverage Catalog (index)\n\n");
    out.push_str(
        &format!(
            "You have access to a large concern catalog ({} entries). The full \
descriptions are not inlined; use `coverage_concerns_search` to find \
concerns relevant to a topic or file, and `coverage_concerns_detail` \
to fetch full text for any specific concern_id.\n\n",
            catalog.concerns.len()
        ),
    );
    out.push_str("| concern_id | severity | summary |\n");
    out.push_str("| --- | --- | --- |\n");
    for concern in &catalog.concerns {
        let summary = first_sentence(&concern.description);
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            concern.id,
            severity_str(concern.severity),
            escape_pipes(&summary),
        ));
    }
    out
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    // Stop at the first sentence-ending punctuation followed by space
    // or end-of-string. Cap at 200 chars for runaway descriptions.
    let mut end = trimmed.len().min(200);
    if let Some(idx) = trimmed[..end].find(". ") {
        end = idx + 1;
    } else if let Some(idx) = trimmed[..end].find(".\n") {
        end = idx + 1;
    }
    trimmed[..end].replace('\n', " ").trim().to_string()
}

fn escape_pipes(text: &str) -> String {
    text.replace('|', "\\|")
}
```

(The existing `severity_str` helper from Plan 1 stays as-is.)

- [ ] **Step 2: Add a test**

```rust
    #[test]
    fn index_mode_renders_table_with_one_row_per_concern() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        let rendered = render_index_mode(&cat);
        assert!(rendered.starts_with("## Coverage Catalog (index)"));
        assert!(rendered.contains("(6 entries)"));
        assert!(rendered.contains("| concern_id | severity | summary |"));
        assert!(rendered.contains("| stride:spoofing | high |"));
        // No full descriptions inlined.
        assert!(!rendered.contains("Identity-verification threats"));
    }

    #[test]
    fn first_sentence_handles_trailing_period() {
        assert_eq!(first_sentence("Short summary."), "Short summary.");
        assert_eq!(first_sentence("First. Second."), "First.");
        assert_eq!(first_sentence("Multiline\nsummary."), "Multiline summary.");
    }
```

- [ ] **Step 3: Export from `lib.rs`**

Add `render_index_mode` to the `pub use catalog::{...}` line and `pub use render::{render_full_mode, render_index_mode};` to `catalog/mod.rs`.

- [ ] **Step 4: Run tests + clippy**

Expected: 43 prior + 2 new = 45 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): index-mode catalog renderer (one-line-per-concern table)"
```

---

## Task 5: Mode auto-selection logic

**Files:**
- Create: `crates/rupu-coverage/src/catalog/mode_selection.rs`
- Modify: `crates/rupu-coverage/src/catalog/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline in `mode_selection.rs`

- [ ] **Step 1: Create `mode_selection.rs`**

```rust
use crate::catalog::types::{CatalogMode, FlatCatalog};
use std::collections::BTreeMap;

/// Default threshold for auto-selecting full vs index mode. A catalog
/// with more than this many concerns auto-renders in index mode unless
/// explicitly overridden per-include. Tunable via `[coverage].full_mode_max_concerns`
/// in `config.toml`; pulled separately by the agent runner.
pub const DEFAULT_FULL_MODE_THRESHOLD: usize = 80;

/// Resolve each concern's `CatalogMode::Auto` into a concrete `Full`
/// or `Index` choice based on total catalog size and threshold.
///
/// Returns a map from concern_id → resolved mode (always Full or Index;
/// never Auto). Explicit per-include `Full` or `Index` choices are
/// preserved as-is.
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

/// Convenience: returns `(full_concerns, index_concerns)` partition.
pub fn partition_by_mode(
    catalog: &FlatCatalog,
    full_mode_max_concerns: usize,
) -> (Vec<&crate::catalog::types::Concern>, Vec<&crate::catalog::types::Concern>) {
    let modes = resolve_modes(catalog, full_mode_max_concerns);
    let mut full = Vec::new();
    let mut index = Vec::new();
    for c in &catalog.concerns {
        match modes.get(&c.id).copied().unwrap_or(CatalogMode::Full) {
            CatalogMode::Full => full.push(c),
            CatalogMode::Index => index.push(c),
            CatalogMode::Auto => full.push(c), // unreachable but defensive
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
        // Force "large" by using a very low threshold.
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
```

- [ ] **Step 2: Wire into module + lib**

Edit `crates/rupu-coverage/src/catalog/mod.rs` to add:

```rust
pub mod mode_selection;
pub use mode_selection::{partition_by_mode, resolve_modes, DEFAULT_FULL_MODE_THRESHOLD};
```

Edit `crates/rupu-coverage/src/lib.rs` to add these to the `pub use catalog::{...}` line.

- [ ] **Step 3: Run tests + clippy**

Expected: 45 prior + 4 new = 49 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): resolve CatalogMode::Auto using configurable threshold"
```

---

## Task 6: Mixed-mode `render_prompt_section`

**Files:**
- Modify: `crates/rupu-coverage/src/catalog/render.rs`
- Test: inline in `render.rs`

- [ ] **Step 1: Add `render_prompt_section`**

This is the function the agent runner will call. It uses `partition_by_mode` to split the catalog, then renders the full-mode concerns inline and the index-mode concerns as a table.

```rust
use crate::catalog::mode_selection::partition_by_mode;
use crate::catalog::types::Concern;

/// Render the catalog into the agent's system prompt, splitting
/// concerns by their resolved render mode. Full-mode concerns get
/// their bodies inlined; index-mode concerns appear in a one-line
/// table with instructions to use `coverage_concerns_search` /
/// `coverage_concerns_detail` for details.
pub fn render_prompt_section(catalog: &FlatCatalog, full_mode_max_concerns: usize) -> String {
    let (full, index) = partition_by_mode(catalog, full_mode_max_concerns);
    let mut out = String::new();

    if !full.is_empty() {
        out.push_str("## Coverage Catalog\n\n");
        out.push_str(intro_text());
        out.push('\n');
        for c in &full {
            out.push_str(&render_one_full(c));
        }
    }

    if !index.is_empty() {
        if !full.is_empty() {
            out.push('\n');
        }
        out.push_str("## Coverage Catalog (index)\n\n");
        out.push_str(
            &format!(
                "You also have access to {} concerns in index mode. Use \
`coverage_concerns_search` to find relevant ones by topic or file, and \
`coverage_concerns_detail` to fetch full descriptions for specific ids.\n\n",
                index.len()
            ),
        );
        out.push_str("| concern_id | severity | summary |\n");
        out.push_str("| --- | --- | --- |\n");
        for c in &index {
            let summary = first_sentence(&c.description);
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                c.id,
                severity_str(c.severity),
                escape_pipes(&summary),
            ));
        }
    }

    out
}

fn intro_text() -> &'static str {
    "You are reviewing this workspace against the following concerns. \
For each (file × concern) you assess, call `coverage_mark` with the \
appropriate status. For each issue you discover, call `report_finding`. \
Files you read, grep, or edit are tracked automatically — you do not \
need to declare them.\n"
}

fn render_one_full(concern: &Concern) -> String {
    let mut out = String::new();
    out.push_str(&format!("### {}\n", concern.id));
    out.push_str(&format!("**Name:** {}\n", concern.name));
    out.push_str(&format!("**Severity:** {}\n", severity_str(concern.severity)));
    if !concern.applicable_globs.is_empty() {
        out.push_str(&format!(
            "**Applies to:** {}\n",
            concern.applicable_globs.join(", ")
        ));
    }
    out.push('\n');
    out.push_str(concern.description.trim());
    out.push_str("\n\n");
    if !concern.references.is_empty() {
        out.push_str("References:\n");
        for r in &concern.references {
            out.push_str(&format!("- {r}\n"));
        }
        out.push('\n');
    }
    out
}
```

Refactor existing `render_full_mode` to use the new helpers (`intro_text`, `render_one_full`) to avoid duplication. Keep its public signature stable.

```rust
pub fn render_full_mode(catalog: &FlatCatalog) -> String {
    let mut out = String::new();
    out.push_str("## Coverage Catalog\n\n");
    out.push_str(intro_text());
    out.push('\n');
    for concern in &catalog.concerns {
        out.push_str(&render_one_full(concern));
    }
    out
}
```

- [ ] **Step 2: Add tests**

```rust
    #[test]
    fn render_prompt_section_full_only_for_small_catalog() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        let rendered = render_prompt_section(&cat, 80);
        assert!(rendered.contains("## Coverage Catalog\n"));
        assert!(rendered.contains("### stride:spoofing"));
        assert!(!rendered.contains("## Coverage Catalog (index)"));
    }

    #[test]
    fn render_prompt_section_index_only_for_explicit_index_mode() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Index,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        let rendered = render_prompt_section(&cat, 80);
        assert!(rendered.contains("## Coverage Catalog (index)"));
        assert!(rendered.contains("| stride:spoofing | high |"));
        assert!(!rendered.contains("### stride:spoofing"));
    }
```

- [ ] **Step 3: Export from `lib.rs`**

Add `render_prompt_section` to re-exports.

- [ ] **Step 4: Run tests + clippy**

Expected: 49 prior + 2 new = 51 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): render_prompt_section combines full + index sections"
```

---

## Task 7: `coverage_concerns_search` tool

**Files:**
- Create: `crates/rupu-coverage/src/tools/coverage_concerns_search.rs`
- Modify: `crates/rupu-coverage/src/tools/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline

- [ ] **Step 1: Implement the tool function**

```rust
use crate::catalog::filter::ConcernFilter;
use crate::catalog::types::{Concern, FlatCatalog};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchResultForm {
    /// Per-concern record carries id, name, severity, one-line summary only.
    Summary,
    /// Per-concern record carries the full body.
    Full,
}

impl Default for SearchResultForm {
    fn default() -> Self {
        Self::Summary
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    let mut end = trimmed.len().min(200);
    if let Some(idx) = trimmed[..end].find(". ") {
        end = idx + 1;
    } else if let Some(idx) = trimmed[..end].find(".\n") {
        end = idx + 1;
    }
    trimmed[..end].replace('\n', " ").trim().to_string()
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
        // Only elevation-of-privilege is Critical in stride.
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
```

- [ ] **Step 2: Wire into module + lib**

Edit `crates/rupu-coverage/src/tools/mod.rs` to add `pub mod coverage_concerns_search;` and re-exports.

Edit `crates/rupu-coverage/src/lib.rs` to re-export `coverage_concerns_search, CoverageConcernsSearchInput, SearchResult, SearchResultForm, SearchResultSummary`.

- [ ] **Step 3: Run tests + clippy**

Expected: 51 prior + 4 new = 55 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): coverage_concerns_search tool (query + filter against FlatCatalog)"
```

---

## Task 8: `coverage_concerns_detail` tool

**Files:**
- Create: `crates/rupu-coverage/src/tools/coverage_concerns_detail.rs`
- Modify: `crates/rupu-coverage/src/tools/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline

- [ ] **Step 1: Implement the tool function**

```rust
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
```

- [ ] **Step 2: Wire into module + lib**

Edit `crates/rupu-coverage/src/tools/mod.rs` and `lib.rs` to expose `coverage_concerns_detail, CoverageConcernsDetailInput, CoverageConcernsDetailOutput`.

- [ ] **Step 3: Run tests + clippy**

Expected: 55 prior + 2 new = 57 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): coverage_concerns_detail tool (fetch full concern bodies by id)"
```

---

## Task 9: Register the two new tools in the agent runner

**Files:**
- Modify: `crates/rupu-agent/src/coverage_tools.rs`
- Test: extend `crates/rupu-agent/tests/coverage_integration.rs`

- [ ] **Step 1: Add two new `Tool` impls to `crates/rupu-agent/src/coverage_tools.rs`**

After the existing `ReportFindingTool` impl (or wherever the existing 4 tools are defined), add:

```rust
use rupu_coverage::{
    coverage_concerns_detail, coverage_concerns_search, CoverageConcernsDetailInput,
    CoverageConcernsSearchInput, SearchResult,
};

pub struct CoverageConcernsSearchTool {
    catalog: Arc<FlatCatalog>,
}

#[async_trait::async_trait]
impl Tool for CoverageConcernsSearchTool {
    fn name(&self) -> &'static str { "coverage_concerns_search" }

    fn description(&self) -> &'static str {
        "Search the concern catalog by substring and/or filter. Returns up to `limit` \
matching concerns in summary form (id, name, severity, summary) by default, or full \
form if `form: 'full'`. Useful when the catalog is too large to inline in the prompt."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Case-insensitive substring match against id, name, description." },
                "filter": {
                    "type": "object",
                    "properties": {
                        "severity": { "type": "array", "items": { "type": "string", "enum": ["info", "low", "medium", "high", "critical"] } },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "ids": { "type": "array", "items": { "type": "string" } },
                        "applicable_to_path": { "type": "string" }
                    }
                },
                "limit": { "type": "integer", "default": 20 },
                "form": { "type": "string", "enum": ["summary", "full"], "default": "summary" }
            }
        })
    }

    async fn invoke(&self, input: serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input: CoverageConcernsSearchInput = serde_json::from_value(input)
            .map_err(|e| ToolError::input(format!("invalid input: {e}")))?;
        let results = coverage_concerns_search(&self.catalog, input);
        let value = serde_json::to_value(&results)
            .map_err(|e| ToolError::execution(format!("serialize: {e}")))?;
        Ok(ToolOutput::text(value.to_string()))
    }
}

pub struct CoverageConcernsDetailTool {
    catalog: Arc<FlatCatalog>,
}

#[async_trait::async_trait]
impl Tool for CoverageConcernsDetailTool {
    fn name(&self) -> &'static str { "coverage_concerns_detail" }

    fn description(&self) -> &'static str {
        "Fetch full concern records by id. Use after `coverage_concerns_search` finds \
a relevant concern and you need its full description, applicable_globs, or references."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "concern_ids": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["concern_ids"]
        })
    }

    async fn invoke(&self, input: serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input: CoverageConcernsDetailInput = serde_json::from_value(input)
            .map_err(|e| ToolError::input(format!("invalid input: {e}")))?;
        let out = coverage_concerns_detail(&self.catalog, input);
        let value = serde_json::to_value(&out)
            .map_err(|e| ToolError::execution(format!("serialize: {e}")))?;
        Ok(ToolOutput::text(value.to_string()))
    }
}
```

(Names like `ToolOutput::text`, `ToolError::input`, `ToolError::execution`, and the `Tool` trait signature must match the actual `rupu-tools` definitions — match them by reading the existing 4 coverage tool impls.)

Update `register(...)` to also push these two:

```rust
pub fn register(
    tools: &mut std::collections::BTreeMap<String, Arc<dyn Tool>>,
    catalog: FlatCatalog,
    paths: CoveragePaths,
) {
    let catalog = Arc::new(catalog);
    // ...existing 4 inserts...
    tools.insert(
        "coverage_concerns_search".to_string(),
        Arc::new(CoverageConcernsSearchTool { catalog: catalog.clone() }),
    );
    tools.insert(
        "coverage_concerns_detail".to_string(),
        Arc::new(CoverageConcernsDetailTool { catalog: catalog.clone() }),
    );
}
```

- [ ] **Step 2: Switch the runner to call `render_prompt_section`**

In `crates/rupu-agent/src/runner.rs`, where the prompt section is rendered (currently calls `render_full_mode`), switch to:

```rust
use rupu_coverage::{render_prompt_section, DEFAULT_FULL_MODE_THRESHOLD};

let prompt_section = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
```

(Later: pull the threshold from `[coverage].full_mode_max_concerns` in config.toml. For Plan 2 v1, the constant default is fine.)

- [ ] **Step 3: Add an integration test**

Append to `crates/rupu-agent/tests/coverage_integration.rs`:

```rust
#[tokio::test]
async fn agent_run_with_index_mode_catalog_exposes_search_tools() {
    // Build a synthetic concerns block that forces index mode on stride.
    // After the run, verify the agent's tool list includes both
    // coverage_concerns_search and coverage_concerns_detail.
    // ... follow the pattern of the existing agent_run_with_concerns_writes_catalog_snapshot test
}
```

(Adapt to existing test pattern.)

- [ ] **Step 4: Run tests + workspace build**

```bash
cargo test -p rupu-agent
cargo build --workspace
cargo clippy -p rupu-agent --tests -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-agent
git commit -m "feat(agent): inject coverage_concerns_search/detail tools; render via render_prompt_section"
```

---

## Task 10: CWE XML reader (build script foundation)

**Files:**
- Modify: `crates/rupu-coverage/Cargo.toml` (add `gen` feature with quick-xml dep + binary target)
- Create: `crates/rupu-coverage/src/bin/gen_cwe_catalog.rs`
- Create: `crates/rupu-coverage/build/cwe/README.md`

This task and the next several (10-13) build up the CWE generator. The generator is a separate binary feature-gated under `gen` so production builds don't carry quick-xml or its dependencies.

- [ ] **Step 1: Add `gen` feature to Cargo.toml**

Edit `crates/rupu-coverage/Cargo.toml`:

```toml
[features]
gen = ["dep:quick-xml"]

[dependencies]
# ... existing ...
quick-xml = { workspace = true, optional = true }

[[bin]]
name = "gen_cwe_catalog"
path = "src/bin/gen_cwe_catalog.rs"
required-features = ["gen"]
```

If `quick-xml` is not yet a workspace dep, add it to root `Cargo.toml` `[workspace.dependencies]`:

```toml
quick-xml = { version = "0.36", features = ["serialize"] }
```

(Use whatever recent stable 0.x version makes sense; check that `serialize` feature is included.)

- [ ] **Step 2: Create `crates/rupu-coverage/build/cwe/README.md`**

```markdown
# CWE catalog generator

`cwe-software-development.yaml` and `cwe-research.yaml` under `../../templates/concerns/`
are generated from MITRE's published CWE XML. To refresh:

```bash
# 1. Download the latest CWE XML release from MITRE:
#    https://cwe.mitre.org/data/downloads.html
curl -L -o build/cwe/cwec_v4.13.xml.zip \
  https://cwe.mitre.org/data/xml/cwec_v4.13.xml.zip
unzip -o build/cwe/cwec_v4.13.xml.zip -d build/cwe/

# 2. Run the generator for each view:
cargo run --features gen --bin gen_cwe_catalog -- \
  --xml build/cwe/cwec_v4.13.xml \
  --view 699 \
  --release 4.13 \
  --out templates/concerns/cwe-software-development.yaml

cargo run --features gen --bin gen_cwe_catalog -- \
  --xml build/cwe/cwec_v4.13.xml \
  --view 1000 \
  --release 4.13 \
  --out templates/concerns/cwe-research.yaml
```

The XML file is gitignored (it's large and re-downloadable). The generated YAML files
and their `.version.txt` sidecars ARE committed.
```

Add `crates/rupu-coverage/build/cwe/*.xml` and `crates/rupu-coverage/build/cwe/*.zip` to `.gitignore`.

- [ ] **Step 3: Create the skeleton `crates/rupu-coverage/src/bin/gen_cwe_catalog.rs`**

```rust
//! Generator: parses MITRE's published CWE XML and emits a rupu-coverage
//! concerns YAML template. See `crates/rupu-coverage/build/cwe/README.md`
//! for the refresh workflow.

use std::path::PathBuf;

#[derive(Debug)]
struct Args {
    xml: PathBuf,
    view: u32,
    release: String,
    out: PathBuf,
}

fn parse_args() -> Args {
    let mut xml = None;
    let mut view = None;
    let mut release = None;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--xml" => xml = args.next().map(PathBuf::from),
            "--view" => view = args.next().and_then(|s| s.parse().ok()),
            "--release" => release = args.next(),
            "--out" => out = args.next().map(PathBuf::from),
            other => eprintln!("unknown flag: {other}"),
        }
    }
    Args {
        xml: xml.expect("--xml required"),
        view: view.expect("--view required (e.g. 699 or 1000)"),
        release: release.expect("--release required (e.g. 4.13)"),
        out: out.expect("--out required"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();
    eprintln!(
        "generating CWE catalog: view={} release={} xml={} out={}",
        args.view,
        args.release,
        args.xml.display(),
        args.out.display(),
    );
    // Subsequent tasks (11-13) fill this in:
    // 1. Parse XML into raw weakness records.
    // 2. Resolve view membership.
    // 3. Map to Concern records (severity, applicable_globs heuristics).
    // 4. Serialize as Template YAML.
    // 5. Write .version.txt sidecar.
    eprintln!("(generator skeleton — body implemented in subsequent tasks)");
    Ok(())
}
```

- [ ] **Step 4: Verify it builds under the gen feature**

```bash
cargo build --features gen --bin gen_cwe_catalog 2>&1 | tail -5
```

Expected: clean build. (No tests yet — this is a binary skeleton; behavior tests come in subsequent tasks.)

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage Cargo.toml Cargo.lock .gitignore
git commit -m "feat(coverage): scaffold gen_cwe_catalog binary (gen feature, CLI args, README)"
```

---

## Task 11: Parse CWE XML into raw weakness records

**Files:**
- Create: `crates/rupu-coverage/src/cwe_gen/mod.rs`
- Create: `crates/rupu-coverage/src/cwe_gen/xml.rs`
- Modify: `crates/rupu-coverage/src/lib.rs` (add `#[cfg(feature = "gen")] pub mod cwe_gen;`)
- Test: `crates/rupu-coverage/src/cwe_gen/xml.rs` (inline)

The XML parsing is feature-gated so it doesn't bloat normal builds.

- [ ] **Step 1: Create `crates/rupu-coverage/src/cwe_gen/mod.rs`**

```rust
//! CWE generator: parses MITRE CWE XML into a rupu-coverage concerns template.
//! Compiled only under the `gen` feature.

#![cfg(feature = "gen")]

pub mod xml;
```

- [ ] **Step 2: Create `crates/rupu-coverage/src/cwe_gen/xml.rs`**

```rust
//! XML parsing layer. Reads MITRE's CWE XML into intermediate
//! `RawWeakness` and `RawView` structs that the mapper layer (Task 12)
//! transforms into our Concern type.

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct RawWeakness {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub extended_description: Option<String>,
    /// E.g. `["Critical", "High"]`. Empty when MITRE doesn't classify.
    pub impact_tags: Vec<String>,
    /// E.g. `["C", "C++", "Rust"]`. Empty when language-agnostic.
    pub applicable_languages: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RawView {
    pub id: u32,
    pub name: String,
    /// Weakness IDs that are members of this view (transitively
    /// through categories — already flattened).
    pub member_weakness_ids: Vec<u32>,
}

#[derive(Debug)]
pub struct ParsedCwe {
    pub weaknesses: Vec<RawWeakness>,
    pub views: Vec<RawView>,
}

#[derive(Debug, thiserror::Error)]
pub enum CweXmlError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("xml: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("malformed: {0}")]
    Malformed(String),
}

pub fn parse_cwe_xml(path: &Path) -> Result<ParsedCwe, CweXmlError> {
    let xml = std::fs::read_to_string(path)?;
    parse_cwe_xml_str(&xml)
}

pub fn parse_cwe_xml_str(xml: &str) -> Result<ParsedCwe, CweXmlError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut weaknesses = Vec::new();
    let mut views = Vec::new();

    // CWE XML structure (simplified):
    //   <Weakness_Catalog>
    //     <Weaknesses>
    //       <Weakness ID="787" Name="...">
    //         <Description>...</Description>
    //         <Extended_Description>...</Extended_Description>
    //         <Common_Consequences>
    //           <Consequence>
    //             <Scope>...</Scope>
    //             <Impact>...</Impact>
    //           </Consequence>
    //         </Common_Consequences>
    //         <Applicable_Platforms>
    //           <Language Name="C" />
    //           <Language Name="C++" />
    //         </Applicable_Platforms>
    //       </Weakness>
    //     </Weaknesses>
    //     <Categories>
    //       <Category ID="119" Name="...">
    //         <Relationships>
    //           <Has_Member CWE_ID="787" />
    //         </Relationships>
    //       </Category>
    //     </Categories>
    //     <Views>
    //       <View ID="699" Name="Software Development">
    //         <Members>
    //           <Has_Member CWE_ID="119" />  <!-- category ref -->
    //           <Has_Member CWE_ID="20" />   <!-- direct weakness ref -->
    //         </Members>
    //       </View>
    //     </Views>
    //   </Weakness_Catalog>

    let mut state = ParseState::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => state.on_start(&reader, e)?,
            Event::End(e) => state.on_end(&reader, e, &mut weaknesses, &mut views)?,
            Event::Text(e) => state.on_text(&reader, e)?,
            Event::Empty(e) => state.on_empty(&reader, e)?,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Flatten view membership: each view's `<Has_Member>` may point at
    // either a Weakness or a Category. Resolve category refs by
    // pulling in all weakness members of the referenced categories.
    let categories = state.into_categories();
    let resolved_views: Vec<RawView> = views
        .into_iter()
        .map(|mut v| {
            let mut weakness_ids: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
            for member_id in v.member_weakness_ids.drain(..) {
                if let Some(cat) = categories.get(&member_id) {
                    // Member was actually a category; expand to its weakness members.
                    weakness_ids.extend(cat.iter().copied());
                } else {
                    // Member is a weakness directly.
                    weakness_ids.insert(member_id);
                }
            }
            v.member_weakness_ids = weakness_ids.into_iter().collect();
            v
        })
        .collect();

    Ok(ParsedCwe {
        weaknesses,
        views: resolved_views,
    })
}

// ParseState is a small state machine that tracks which element we're
// inside (weakness vs category vs view), accumulates field values, and
// emits a complete record when the closing tag fires.
#[derive(Debug, Default)]
struct ParseState {
    // Implementation in next step (deferred — would balloon this task too much).
    // For now, treat as a placeholder that compiles.
}

impl ParseState {
    fn on_start(&mut self, _r: &Reader<&[u8]>, _e: quick_xml::events::BytesStart) -> Result<(), CweXmlError> {
        Ok(())
    }
    fn on_end(
        &mut self,
        _r: &Reader<&[u8]>,
        _e: quick_xml::events::BytesEnd,
        _weaknesses: &mut Vec<RawWeakness>,
        _views: &mut Vec<RawView>,
    ) -> Result<(), CweXmlError> {
        Ok(())
    }
    fn on_text(&mut self, _r: &Reader<&[u8]>, _e: quick_xml::events::BytesText) -> Result<(), CweXmlError> {
        Ok(())
    }
    fn on_empty(&mut self, _r: &Reader<&[u8]>, _e: quick_xml::events::BytesStart) -> Result<(), CweXmlError> {
        Ok(())
    }
    fn into_categories(self) -> std::collections::HashMap<u32, Vec<u32>> {
        std::collections::HashMap::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Weakness_Catalog>
  <Weaknesses>
    <Weakness ID="787" Name="Out-of-bounds Write">
      <Description>The code writes data past the end of the intended buffer.</Description>
      <Extended_Description>This typically occurs when...</Extended_Description>
      <Common_Consequences>
        <Consequence>
          <Scope>Integrity</Scope>
          <Impact>Modify Memory</Impact>
        </Consequence>
      </Common_Consequences>
      <Applicable_Platforms>
        <Language Name="C" />
        <Language Name="C++" />
      </Applicable_Platforms>
    </Weakness>
  </Weaknesses>
  <Views>
    <View ID="699" Name="Software Development">
      <Members>
        <Has_Member CWE_ID="787" />
      </Members>
    </View>
  </Views>
</Weakness_Catalog>
"#;

    #[test]
    fn parses_minimal_fixture_without_panicking() {
        // Skeleton just verifies the parser runs cleanly on a tiny fixture.
        // Full parsing is implemented in Task 12 (which replaces ParseState's
        // placeholder body).
        let parsed = parse_cwe_xml_str(TINY_FIXTURE).expect("parses");
        // After Task 12 lands the real parser, this test will assert:
        // assert_eq!(parsed.weaknesses.len(), 1);
        // assert_eq!(parsed.weaknesses[0].id, 787);
        // For now, just sanity-check the call doesn't panic.
        let _ = parsed.weaknesses.len();
        let _ = parsed.views.len();
    }
}
```

- [ ] **Step 3: Wire feature-gated module in `lib.rs`**

In `crates/rupu-coverage/src/lib.rs`, add:

```rust
#[cfg(feature = "gen")]
pub mod cwe_gen;
```

- [ ] **Step 4: Verify**

```bash
cargo build -p rupu-coverage 2>&1 | tail -5
cargo build --features gen -p rupu-coverage 2>&1 | tail -5
cargo test --features gen -p rupu-coverage cwe_gen 2>&1 | tail -5
```

Expected: both builds clean; the placeholder test passes.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): CWE XML reader scaffold (parse skeleton + RawWeakness/RawView types)"
```

---

## Task 12: Implement the CWE XML state machine

**Files:**
- Modify: `crates/rupu-coverage/src/cwe_gen/xml.rs`

This task fills in the `ParseState` body that Task 11 stubbed out. It's mechanical state-machine code; the structure follows the CWE schema commented at the top of Task 11.

- [ ] **Step 1: Replace `ParseState` with the real implementation**

```rust
#[derive(Debug, Default)]
struct ParseState {
    in_weakness: bool,
    current_weakness: Option<PartialWeakness>,
    in_category: bool,
    current_category: Option<PartialCategory>,
    in_view: bool,
    current_view: Option<PartialView>,
    /// Stack of element names currently open. Helps disambiguate
    /// nested `<Description>` between Weakness, Category, etc.
    open_elements: Vec<String>,
    categories: std::collections::HashMap<u32, Vec<u32>>,
}

#[derive(Debug, Default)]
struct PartialWeakness {
    id: u32,
    name: String,
    description: String,
    extended_description: Option<String>,
    impact_tags: Vec<String>,
    applicable_languages: Vec<String>,
    in_description: bool,
    in_extended_description: bool,
    in_impact: bool,
}

#[derive(Debug, Default)]
struct PartialCategory {
    id: u32,
    member_weakness_ids: Vec<u32>,
}

#[derive(Debug, Default)]
struct PartialView {
    id: u32,
    name: String,
    member_ids: Vec<u32>,
}

impl ParseState {
    fn on_start(&mut self, r: &Reader<&[u8]>, e: quick_xml::events::BytesStart) -> Result<(), CweXmlError> {
        let name = std::str::from_utf8(e.name().as_ref())
            .map_err(|_| CweXmlError::Malformed("non-utf8 element name".into()))?
            .to_string();
        self.open_elements.push(name.clone());

        match name.as_str() {
            "Weakness" => {
                let mut pw = PartialWeakness::default();
                for attr in e.attributes().flatten() {
                    let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or_default();
                    let val = attr
                        .decode_and_unescape_value(r.decoder())
                        .map_err(|e| CweXmlError::Malformed(format!("attr: {e}")))?
                        .into_owned();
                    match key {
                        "ID" => pw.id = val.parse().unwrap_or(0),
                        "Name" => pw.name = val,
                        _ => {}
                    }
                }
                self.in_weakness = true;
                self.current_weakness = Some(pw);
            }
            "Description" if self.in_weakness => {
                if let Some(w) = self.current_weakness.as_mut() {
                    w.in_description = true;
                }
            }
            "Extended_Description" if self.in_weakness => {
                if let Some(w) = self.current_weakness.as_mut() {
                    w.in_extended_description = true;
                }
            }
            "Impact" if self.in_weakness => {
                if let Some(w) = self.current_weakness.as_mut() {
                    w.in_impact = true;
                }
            }
            "Category" => {
                let mut pc = PartialCategory::default();
                for attr in e.attributes().flatten() {
                    let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or_default();
                    let val = attr
                        .decode_and_unescape_value(r.decoder())
                        .map_err(|e| CweXmlError::Malformed(format!("attr: {e}")))?
                        .into_owned();
                    if key == "ID" {
                        pc.id = val.parse().unwrap_or(0);
                    }
                }
                self.in_category = true;
                self.current_category = Some(pc);
            }
            "View" => {
                let mut pv = PartialView::default();
                for attr in e.attributes().flatten() {
                    let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or_default();
                    let val = attr
                        .decode_and_unescape_value(r.decoder())
                        .map_err(|e| CweXmlError::Malformed(format!("attr: {e}")))?
                        .into_owned();
                    match key {
                        "ID" => pv.id = val.parse().unwrap_or(0),
                        "Name" => pv.name = val,
                        _ => {}
                    }
                }
                self.in_view = true;
                self.current_view = Some(pv);
            }
            _ => {}
        }
        Ok(())
    }

    fn on_end(
        &mut self,
        _r: &Reader<&[u8]>,
        e: quick_xml::events::BytesEnd,
        weaknesses: &mut Vec<RawWeakness>,
        views: &mut Vec<RawView>,
    ) -> Result<(), CweXmlError> {
        let name = std::str::from_utf8(e.name().as_ref())
            .map_err(|_| CweXmlError::Malformed("non-utf8 element name".into()))?
            .to_string();
        self.open_elements.pop();
        match name.as_str() {
            "Weakness" => {
                if let Some(pw) = self.current_weakness.take() {
                    weaknesses.push(RawWeakness {
                        id: pw.id,
                        name: pw.name,
                        description: pw.description.trim().to_string(),
                        extended_description: if pw.extended_description.is_some() {
                            pw.extended_description.map(|s| s.trim().to_string())
                        } else {
                            None
                        },
                        impact_tags: pw.impact_tags,
                        applicable_languages: pw.applicable_languages,
                    });
                }
                self.in_weakness = false;
            }
            "Description" if self.in_weakness => {
                if let Some(w) = self.current_weakness.as_mut() {
                    w.in_description = false;
                }
            }
            "Extended_Description" if self.in_weakness => {
                if let Some(w) = self.current_weakness.as_mut() {
                    w.in_extended_description = false;
                }
            }
            "Impact" if self.in_weakness => {
                if let Some(w) = self.current_weakness.as_mut() {
                    w.in_impact = false;
                }
            }
            "Category" => {
                if let Some(pc) = self.current_category.take() {
                    self.categories.insert(pc.id, pc.member_weakness_ids);
                }
                self.in_category = false;
            }
            "View" => {
                if let Some(pv) = self.current_view.take() {
                    views.push(RawView {
                        id: pv.id,
                        name: pv.name,
                        member_weakness_ids: pv.member_ids,
                    });
                }
                self.in_view = false;
            }
            _ => {}
        }
        Ok(())
    }

    fn on_text(&mut self, _r: &Reader<&[u8]>, e: quick_xml::events::BytesText) -> Result<(), CweXmlError> {
        let text = e
            .unescape()
            .map_err(|err| CweXmlError::Malformed(format!("text: {err}")))?
            .into_owned();
        if let Some(w) = self.current_weakness.as_mut() {
            if w.in_description {
                w.description.push_str(&text);
            } else if w.in_extended_description {
                w.extended_description.get_or_insert_with(String::new).push_str(&text);
            } else if w.in_impact {
                w.impact_tags.push(text.trim().to_string());
            }
        }
        Ok(())
    }

    fn on_empty(&mut self, r: &Reader<&[u8]>, e: quick_xml::events::BytesStart) -> Result<(), CweXmlError> {
        let name = std::str::from_utf8(e.name().as_ref())
            .map_err(|_| CweXmlError::Malformed("non-utf8 element name".into()))?
            .to_string();
        match name.as_str() {
            "Language" if self.in_weakness => {
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == b"Name" {
                        let val = attr
                            .decode_and_unescape_value(r.decoder())
                            .map_err(|e| CweXmlError::Malformed(format!("attr: {e}")))?
                            .into_owned();
                        if let Some(w) = self.current_weakness.as_mut() {
                            w.applicable_languages.push(val);
                        }
                    }
                }
            }
            "Has_Member" if self.in_category || self.in_view => {
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == b"CWE_ID" {
                        let val = attr
                            .decode_and_unescape_value(r.decoder())
                            .map_err(|e| CweXmlError::Malformed(format!("attr: {e}")))?
                            .into_owned();
                        if let Ok(id) = val.parse::<u32>() {
                            if let Some(c) = self.current_category.as_mut() {
                                c.member_weakness_ids.push(id);
                            } else if let Some(v) = self.current_view.as_mut() {
                                v.member_ids.push(id);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn into_categories(self) -> std::collections::HashMap<u32, Vec<u32>> {
        self.categories
    }
}
```

- [ ] **Step 2: Update the test**

Replace the placeholder assertions:

```rust
    #[test]
    fn parses_minimal_fixture() {
        let parsed = parse_cwe_xml_str(TINY_FIXTURE).expect("parses");
        assert_eq!(parsed.weaknesses.len(), 1);
        assert_eq!(parsed.weaknesses[0].id, 787);
        assert_eq!(parsed.weaknesses[0].name, "Out-of-bounds Write");
        assert!(parsed.weaknesses[0].description.contains("writes data past the end"));
        assert_eq!(parsed.weaknesses[0].applicable_languages, vec!["C", "C++"]);
        assert_eq!(parsed.weaknesses[0].impact_tags, vec!["Modify Memory"]);

        assert_eq!(parsed.views.len(), 1);
        assert_eq!(parsed.views[0].id, 699);
        assert_eq!(parsed.views[0].member_weakness_ids, vec![787]);
    }
```

- [ ] **Step 3: Verify**

```bash
cargo test --features gen -p rupu-coverage cwe_gen
cargo clippy --features gen -p rupu-coverage --tests -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): CWE XML state-machine parser (Weakness/Category/View)"
```

---

## Task 13: Map raw CWE records to `Concern` records

**Files:**
- Create: `crates/rupu-coverage/src/cwe_gen/mapping.rs`
- Modify: `crates/rupu-coverage/src/cwe_gen/mod.rs`
- Test: inline in `mapping.rs`

- [ ] **Step 1: Create `mapping.rs`**

```rust
//! Maps `RawWeakness` + view membership into `Concern` records that
//! the rupu-coverage catalog system can consume.

use crate::catalog::types::{Concern, Severity, TouchStrength};
use crate::cwe_gen::xml::{ParsedCwe, RawWeakness, RawView};

/// Map a parsed CWE corpus + a view ID into a list of Concern records.
/// Returns `None` if the view ID isn't found in the parsed corpus.
pub fn map_view_to_concerns(
    parsed: &ParsedCwe,
    view_id: u32,
    namespace: &str,
) -> Option<Vec<Concern>> {
    let view = parsed.views.iter().find(|v| v.id == view_id)?;
    let weakness_by_id: std::collections::HashMap<u32, &RawWeakness> =
        parsed.weaknesses.iter().map(|w| (w.id, w)).collect();
    let mut concerns: Vec<Concern> = view
        .member_weakness_ids
        .iter()
        .filter_map(|id| weakness_by_id.get(id).copied())
        .map(|w| map_weakness(w, namespace))
        .collect();
    concerns.sort_by(|a, b| a.id.cmp(&b.id));
    Some(concerns)
}

fn map_weakness(w: &RawWeakness, namespace: &str) -> Concern {
    let id = format!("{namespace}:cwe-{}-{}", w.id, slug(&w.name));
    let description = compose_description(w);
    Concern {
        id,
        name: format!("CWE-{} — {}", w.id, w.name),
        description,
        severity: severity_from_impact(&w.impact_tags),
        applicable_globs: globs_from_languages(&w.applicable_languages),
        min_strength: TouchStrength::Read,
        references: vec![format!("https://cwe.mitre.org/data/definitions/{}.html", w.id)],
        tags: tags_from_languages(&w.applicable_languages),
    }
}

fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn compose_description(w: &RawWeakness) -> String {
    let mut out = w.description.clone();
    if let Some(extended) = &w.extended_description {
        if !extended.is_empty() {
            out.push_str("\n\n");
            out.push_str(extended);
        }
    }
    // Cap to 600 chars; full body available via coverage_concerns_detail.
    if out.len() > 600 {
        out.truncate(597);
        out.push_str("...");
    }
    out
}

fn severity_from_impact(impact_tags: &[String]) -> Severity {
    let s: String = impact_tags.join(" ").to_lowercase();
    if s.contains("execute unauthorized code")
        || s.contains("gain privileges")
        || s.contains("bypass protection")
        || s.contains("modify memory")
    {
        Severity::Critical
    } else if s.contains("read application data")
        || s.contains("modify application data")
        || s.contains("hide activities")
    {
        Severity::High
    } else if s.contains("dos")
        || s.contains("denial of service")
        || s.contains("resource consumption")
    {
        Severity::Medium
    } else if impact_tags.is_empty() {
        Severity::Medium
    } else {
        Severity::Medium
    }
}

fn globs_from_languages(langs: &[String]) -> Vec<String> {
    if langs.is_empty() || langs.iter().any(|l| l.eq_ignore_ascii_case("Not Language-Specific")) {
        return vec!["**".to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for lang in langs {
        for glob in language_to_globs(lang) {
            if seen.insert(glob.to_string()) {
                out.push(glob.to_string());
            }
        }
    }
    if out.is_empty() {
        vec!["**".to_string()]
    } else {
        out
    }
}

fn language_to_globs(lang: &str) -> &'static [&'static str] {
    match lang.to_lowercase().as_str() {
        "c" | "c++" => &["**/*.c", "**/*.cpp", "**/*.h", "**/*.hpp", "**/*.cc", "**/*.cxx"],
        "rust" => &["**/*.rs"],
        "python" => &["**/*.py"],
        "java" => &["**/*.java"],
        "javascript" => &["**/*.js", "**/*.jsx", "**/*.mjs"],
        "typescript" => &["**/*.ts", "**/*.tsx"],
        "go" => &["**/*.go"],
        "ruby" => &["**/*.rb"],
        "php" => &["**/*.php"],
        "c#" => &["**/*.cs"],
        "swift" => &["**/*.swift"],
        "kotlin" => &["**/*.kt", "**/*.kts"],
        _ => &[],
    }
}

fn tags_from_languages(langs: &[String]) -> Vec<String> {
    langs
        .iter()
        .map(|l| format!("lang:{}", l.to_lowercase().replace(' ', "-")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_weakness() -> RawWeakness {
        RawWeakness {
            id: 787,
            name: "Out-of-bounds Write".to_string(),
            description: "The code writes data past the end of the intended buffer.".to_string(),
            extended_description: None,
            impact_tags: vec!["Modify Memory".to_string()],
            applicable_languages: vec!["C".to_string(), "C++".to_string()],
        }
    }

    #[test]
    fn map_weakness_produces_expected_concern() {
        let c = map_weakness(&fixture_weakness(), "cwe-research");
        assert_eq!(c.id, "cwe-research:cwe-787-out-of-bounds-write");
        assert_eq!(c.name, "CWE-787 — Out-of-bounds Write");
        assert_eq!(c.severity, Severity::Critical); // "modify memory" → Critical
        assert!(c.applicable_globs.iter().any(|g| g == "**/*.c"));
        assert!(c.references[0].contains("787"));
        assert!(c.tags.iter().any(|t| t == "lang:c"));
    }

    #[test]
    fn slug_handles_special_chars() {
        assert_eq!(slug("Out-of-bounds Write"), "out-of-bounds-write");
        assert_eq!(slug("OS Command Injection"), "os-command-injection");
        assert_eq!(slug("XSS / Cross-site Scripting"), "xss-cross-site-scripting");
    }

    #[test]
    fn severity_heuristics_cover_known_impacts() {
        assert_eq!(
            severity_from_impact(&["Execute Unauthorized Code".to_string()]),
            Severity::Critical
        );
        assert_eq!(
            severity_from_impact(&["Read Application Data".to_string()]),
            Severity::High
        );
        assert_eq!(
            severity_from_impact(&["DoS: Resource Consumption".to_string()]),
            Severity::Medium
        );
        assert_eq!(severity_from_impact(&[]), Severity::Medium);
    }

    #[test]
    fn globs_default_to_double_star_when_unknown_language() {
        let g = globs_from_languages(&["NotALanguage".to_string()]);
        assert_eq!(g, vec!["**".to_string()]);
    }
}
```

- [ ] **Step 2: Wire into module**

Edit `crates/rupu-coverage/src/cwe_gen/mod.rs`:

```rust
#![cfg(feature = "gen")]

pub mod mapping;
pub mod xml;
```

- [ ] **Step 3: Verify**

```bash
cargo test --features gen -p rupu-coverage cwe_gen::mapping
cargo clippy --features gen -p rupu-coverage --tests -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): map RawWeakness records into Concern records (severity, globs, tags)"
```

---

## Task 14: Generator main + emit YAML + version sidecar

**Files:**
- Modify: `crates/rupu-coverage/src/bin/gen_cwe_catalog.rs`
- Modify: `crates/rupu-coverage/src/cwe_gen/mod.rs` (add `template.rs` for Template assembly)
- Create: `crates/rupu-coverage/src/cwe_gen/template.rs`

- [ ] **Step 1: Create `crates/rupu-coverage/src/cwe_gen/template.rs`**

```rust
use crate::catalog::types::Template;

/// Assemble a Template struct ready for YAML serialization.
pub fn build_template(name: &str, view_name: &str, concerns: Vec<crate::catalog::types::Concern>) -> Template {
    Template {
        name: name.to_string(),
        version: 1,
        description: format!("CWE {view_name} view, generated from MITRE CWE XML"),
        references: vec![
            "https://cwe.mitre.org/data/downloads.html".to_string(),
        ],
        concerns,
        includes: vec![],
    }
}
```

Add `pub mod template;` to `crates/rupu-coverage/src/cwe_gen/mod.rs`.

- [ ] **Step 2: Implement the generator main**

Replace `crates/rupu-coverage/src/bin/gen_cwe_catalog.rs`:

```rust
//! Generator: parses MITRE's published CWE XML and emits a rupu-coverage
//! concerns YAML template.

use rupu_coverage::cwe_gen::{mapping::map_view_to_concerns, template::build_template, xml::parse_cwe_xml};
use std::path::PathBuf;

#[derive(Debug)]
struct Args {
    xml: PathBuf,
    view: u32,
    release: String,
    out: PathBuf,
}

fn parse_args() -> Args {
    let mut xml = None;
    let mut view = None;
    let mut release = None;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--xml" => xml = args.next().map(PathBuf::from),
            "--view" => view = args.next().and_then(|s| s.parse().ok()),
            "--release" => release = args.next(),
            "--out" => out = args.next().map(PathBuf::from),
            other => eprintln!("unknown flag: {other}"),
        }
    }
    Args {
        xml: xml.expect("--xml required"),
        view: view.expect("--view required (e.g. 699 or 1000)"),
        release: release.expect("--release required (e.g. 4.13)"),
        out: out.expect("--out required"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();
    eprintln!(
        "Parsing CWE XML: view={} release={} xml={}",
        args.view,
        args.release,
        args.xml.display()
    );

    let parsed = parse_cwe_xml(&args.xml)?;
    eprintln!(
        "Parsed {} weaknesses, {} views, {} categories",
        parsed.weaknesses.len(),
        parsed.views.len(),
        // categories aren't directly exposed but folded into views;
        // just print the view membership counts.
        parsed.views.iter().map(|v| v.member_weakness_ids.len()).sum::<usize>(),
    );

    let (namespace, view_name) = match args.view {
        699 => ("cwe-software-development", "Software Development"),
        1000 => ("cwe-research", "Research"),
        other => return Err(format!("unsupported view {other}; supported: 699, 1000").into()),
    };

    let concerns = map_view_to_concerns(&parsed, args.view, namespace)
        .ok_or_else(|| format!("view {} not found in XML", args.view))?;
    eprintln!(
        "Mapped {} concerns for view {} ({})",
        concerns.len(),
        args.view,
        view_name
    );

    let template = build_template(namespace, view_name, concerns);
    let yaml = serde_yaml::to_string(&template)?;

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, yaml)?;
    eprintln!("Wrote {}", args.out.display());

    // Sidecar: <out>.version.txt
    let mut version_path = args.out.clone();
    let new_name = format!(
        "{}.version.txt",
        version_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("cwe")
    );
    version_path.set_file_name(new_name);
    let stamp = chrono::Utc::now().to_rfc3339();
    std::fs::write(&version_path, format!("cwe_release: {}\ngenerated_at: {}\n", args.release, stamp))?;
    eprintln!("Wrote {}", version_path.display());

    Ok(())
}
```

- [ ] **Step 3: Verify the generator runs against a fixture**

The full MITRE XML is too big to commit. For a smoke test, use the `TINY_FIXTURE` from Task 11's tests via a one-off shell command:

```bash
mkdir -p /tmp/cwe-test
cat > /tmp/cwe-test/tiny.xml <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<Weakness_Catalog>
  <Weaknesses>
    <Weakness ID="787" Name="Out-of-bounds Write">
      <Description>The code writes data past the end of the intended buffer.</Description>
      <Common_Consequences>
        <Consequence>
          <Scope>Integrity</Scope>
          <Impact>Modify Memory</Impact>
        </Consequence>
      </Common_Consequences>
      <Applicable_Platforms>
        <Language Name="C" />
      </Applicable_Platforms>
    </Weakness>
  </Weaknesses>
  <Views>
    <View ID="699" Name="Software Development">
      <Members>
        <Has_Member CWE_ID="787" />
      </Members>
    </View>
  </Views>
</Weakness_Catalog>
EOF

cargo run --features gen --bin gen_cwe_catalog -- \
  --xml /tmp/cwe-test/tiny.xml \
  --view 699 \
  --release "test-4.13" \
  --out /tmp/cwe-test/out.yaml

cat /tmp/cwe-test/out.yaml
cat /tmp/cwe-test/out.version.txt
```

Expected: the generator runs without error; `out.yaml` is a valid Template YAML with one concern; `out.version.txt` records the release stamp.

(You can clean up `/tmp/cwe-test/` after.)

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): gen_cwe_catalog main — parse, map, emit YAML + version sidecar"
```

---

## Task 15: Run the generator against real MITRE XML; commit two CWE templates

This task **requires manual download** of the real CWE XML. The implementer runs the generator twice (once per view) and commits the resulting four files (two `.yaml`, two `.version.txt`).

**Files:**
- Create: `crates/rupu-coverage/templates/concerns/cwe-software-development.yaml` (generated)
- Create: `crates/rupu-coverage/templates/concerns/cwe-software-development.version.txt`
- Create: `crates/rupu-coverage/templates/concerns/cwe-research.yaml` (generated)
- Create: `crates/rupu-coverage/templates/concerns/cwe-research.version.txt`

- [ ] **Step 1: Download the CWE XML**

```bash
# From the repo root:
mkdir -p crates/rupu-coverage/build/cwe
curl -L -o crates/rupu-coverage/build/cwe/cwec_v4.13.xml.zip \
  https://cwe.mitre.org/data/xml/cwec_v4.13.xml.zip
unzip -o crates/rupu-coverage/build/cwe/cwec_v4.13.xml.zip -d crates/rupu-coverage/build/cwe/
```

The unzipped file is `cwec_v4.13.xml` under `build/cwe/`. The XML is ~5MB.

- [ ] **Step 2: Generate `cwe-software-development.yaml`**

```bash
cargo run --features gen --bin gen_cwe_catalog -- \
  --xml crates/rupu-coverage/build/cwe/cwec_v4.13.xml \
  --view 699 \
  --release 4.13 \
  --out crates/rupu-coverage/templates/concerns/cwe-software-development.yaml
```

Verify it produced ~440 concerns:

```bash
grep -c "^  - id:" crates/rupu-coverage/templates/concerns/cwe-software-development.yaml
```

Expected: number in the 400-500 range (Software Development view has ~440 members at last count).

- [ ] **Step 3: Generate `cwe-research.yaml`**

```bash
cargo run --features gen --bin gen_cwe_catalog -- \
  --xml crates/rupu-coverage/build/cwe/cwec_v4.13.xml \
  --view 1000 \
  --release 4.13 \
  --out crates/rupu-coverage/templates/concerns/cwe-research.yaml
```

Verify ~930 concerns:

```bash
grep -c "^  - id:" crates/rupu-coverage/templates/concerns/cwe-research.yaml
```

- [ ] **Step 4: Verify both parse cleanly**

```bash
cargo test -p rupu-coverage catalog::parse::tests::all_curated_templates_parse 2>&1 | tail -5
```

(The next task adds them to the parse-all test; for this task, also spot-check by hand-parsing:)

```bash
cargo run --features gen --bin gen_cwe_catalog 2>&1 || true
# Then a quick rust one-off:
cat > /tmp/parse_check.rs <<'EOF'
use rupu_coverage::parse_template_file;
use std::path::Path;
fn main() {
    for p in &[
        "crates/rupu-coverage/templates/concerns/cwe-software-development.yaml",
        "crates/rupu-coverage/templates/concerns/cwe-research.yaml",
    ] {
        let t = parse_template_file(Path::new(p)).expect("parses");
        eprintln!("{}: {} concerns", p, t.concerns.len());
    }
}
EOF
# (Skip the one-off if the next task's parse-all test covers it.)
```

- [ ] **Step 5: Commit the two YAML files + sidecars**

```bash
git add crates/rupu-coverage/templates/concerns/cwe-software-development.yaml \
        crates/rupu-coverage/templates/concerns/cwe-software-development.version.txt \
        crates/rupu-coverage/templates/concerns/cwe-research.yaml \
        crates/rupu-coverage/templates/concerns/cwe-research.version.txt
git commit -m "feat(coverage): generate cwe-software-development (~440) + cwe-research (~930) from MITRE CWE 4.13"
```

The committed YAMLs are large (multi-MB each). If git complains about file size, they're still well within Git's normal limits (no LFS needed at MB scale).

---

## Task 16: Register the CWE templates in the builtin registry

**Files:**
- Modify: `crates/rupu-coverage/src/catalog/builtin.rs`
- Modify: `crates/rupu-coverage/src/catalog/parse.rs` (extend `all_curated_templates_parse` test)
- Test: inline in `builtin.rs`

- [ ] **Step 1: Add the two CWE templates to `BUILTIN_TEMPLATES`**

Edit `crates/rupu-coverage/src/catalog/builtin.rs`, add to the `BUILTIN_TEMPLATES` array (alphabetical position):

```rust
    (
        "cwe-research",
        include_str!("../../templates/concerns/cwe-research.yaml"),
    ),
    (
        "cwe-software-development",
        include_str!("../../templates/concerns/cwe-software-development.yaml"),
    ),
```

These should go between `cwe-top25-2023` and `secrets-in-source` to maintain alphabetical order.

- [ ] **Step 2: Update `all_curated_templates_parse` in `parse.rs`**

Add the two new filenames to the list:

```rust
    let expected = [
        "api-security-default.yaml",
        "code-smells.yaml",
        "cwe-research.yaml",
        "cwe-software-development.yaml",
        "cwe-top25-2023.yaml",
        "owasp-api-top10-2023.yaml",
        "owasp-top10-2021.yaml",
        "secrets-in-source.yaml",
        "stride.yaml",
        "web-security-default.yaml",
    ];
```

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: all prior tests + the `all_curated_templates_parse` test now exercises the two CWE templates. The `each_builtin_resolves_to_template_with_matching_name` test also exercises them. The binary may grow significantly (~5MB) from the `include_str!`'d YAML — acceptable per spec.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): register cwe-software-development and cwe-research as builtin templates"
```

---

## Task 17: End-to-end test — CWE catalog in index mode with search

**Files:**
- Create: `crates/rupu-coverage/tests/cwe_index_mode_end_to_end.rs`

- [ ] **Step 1: Write the e2e test**

```rust
//! Verifies the full Plan 2 flow: include cwe-research with a filter,
//! flatten, auto-select index mode (because catalog > 80 entries),
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
            mode: CatalogMode::Auto, // will auto-pick Index for ~930 entries
            filter: Some(ConcernFilter {
                severity: vec![Severity::Critical, Severity::High],
                ..Default::default()
            }),
        })],
    };
    let cat = flatten(&block).unwrap();

    // After severity filter, expect a meaningful but smaller subset
    // (CWE-research has many low/medium entries; high+critical is a
    // significant chunk — likely 100-300 concerns).
    assert!(cat.concerns.len() > 50);

    let prompt = render_prompt_section(&cat, DEFAULT_FULL_MODE_THRESHOLD);
    assert!(prompt.contains("## Coverage Catalog (index)"));
    assert!(prompt.contains("coverage_concerns_search"));
    assert!(prompt.contains("coverage_concerns_detail"));

    // Search for "injection" — expect at least CWE-78 OS Command Injection
    // and CWE-89 SQL Injection to surface.
    let results = coverage_concerns_search(
        &cat,
        CoverageConcernsSearchInput {
            query: Some("injection".to_string()),
            limit: 50,
            ..Default::default()
        },
    );
    assert!(!results.is_empty());

    // Fetch details for CWE-78. Note: the slug includes the full
    // hyphenated name, so we can't hardcode the exact id — pick the
    // first injection result.
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
    assert!(detail.concerns[0].references.iter().any(|r| r.contains("cwe.mitre.org")));
    assert!(detail.not_found.is_empty());
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p rupu-coverage --test cwe_index_mode_end_to_end
```

Expected: pass.

- [ ] **Step 3: Final workspace verification**

```bash
cargo build --workspace
cargo test --workspace --no-fail-fast 2>&1 | grep -E "^test result|FAILED" | tail -30
cargo clippy --workspace --tests -- -D warnings
```

Expected: workspace builds; new tests pass; same pre-existing rupu-cli printer failures as before (unrelated to this plan).

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage/tests/cwe_index_mode_end_to_end.rs
git commit -m "test(coverage): e2e — CWE research in index mode with filter + search + detail"
```

---

## Self-review

**1. Spec coverage:**

| Spec requirement | Task |
| --- | --- |
| Catalog filters (severity / tags / ids / applicable_to_path) on `include:` | 1, 2 |
| Per-concern render mode tracked on FlatCatalog | 3 |
| Index-mode rendering function | 4 |
| Mode auto-selection logic with configurable threshold | 5 |
| Mixed-mode `render_prompt_section` (full + index combined) | 6 |
| `coverage_concerns_search` tool | 7 |
| `coverage_concerns_detail` tool | 8 |
| Both new tools auto-injected when index mode is used | 9 |
| `gen_cwe_catalog` build-time generator | 10, 11, 12, 13, 14 |
| `cwe-software-development.yaml` (~440 entries) shipped | 15 |
| `cwe-research.yaml` (~930 entries) shipped | 15 |
| `.version.txt` sidecars recording CWE release | 14, 15 |
| Both CWE templates resolvable via builtin registry | 16 |
| End-to-end test exercising the full new surface | 17 |

**2. Placeholder scan:** All steps contain concrete code or commands. Task 11's `ParseState` is intentionally stubbed (placeholder body) — that's flagged in the comment and Task 12 fills it in. Task 17's "pick the first injection result" handles the fact that the slug includes the full name (which is data-dependent on the MITRE XML release), avoiding hardcoded ids.

**3. Type consistency:** `CatalogMode`, `ConcernFilter`, `FlatCatalog.render_modes`, `IncludeDirective.{mode, filter}` are introduced in Task 1, referenced consistently in Tasks 2-9. `RawWeakness` / `RawView` / `ParsedCwe` from Task 11 are consumed by Tasks 12-14. `map_view_to_concerns` signature in Task 13 matches the call in Task 14's generator main.

**4. Spec gaps:** Plan 1 follow-ups (async/sync write inconsistency, transcript shutdown gap, within-run supersede, workflow-wins integration test, grep/bash tests) are intentionally deferred per the "Out of scope" note at the top. Plan 3 work (CLI subcommand, audit rendering, session integration) is also explicitly out of scope.

---

## Execution

Plan complete and saved to `docs/superpowers/plans/2026-05-23-rupu-coverage-harness-plan-2-large-catalogs-and-cwe.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
