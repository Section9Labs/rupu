use crate::catalog::mode_selection::partition_by_mode;
use crate::catalog::text::first_sentence;
use crate::catalog::types::{Concern, FlatCatalog};

/// Render the catalog into the agent's system prompt, splitting
/// concerns by their resolved render mode. Full-mode concerns get
/// their bodies inlined; index-mode concerns appear in a one-line
/// table with instructions to use the search/detail tools.
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
        out.push_str(&format!(
            "You also have access to {} concerns in index mode. Use \
`coverage_concerns_search` to find relevant ones by topic or file, and \
`coverage_concerns_detail` to fetch full descriptions for specific ids.\n\n",
            index.len()
        ));
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

fn severity_str(s: crate::catalog::types::Severity) -> &'static str {
    use crate::catalog::types::Severity::*;
    match s {
        Info => "info",
        Low => "low",
        Medium => "medium",
        High => "high",
        Critical => "critical",
    }
}

/// Render the catalog as a compact one-line-per-concern table for
/// large catalogs. The agent uses `coverage_concerns_search` /
/// `coverage_concerns_detail` to fetch full bodies on demand.
pub fn render_index_mode(catalog: &FlatCatalog) -> String {
    let mut out = String::new();
    out.push_str("## Coverage Catalog (index)\n\n");
    out.push_str(&format!(
        "You have access to a large concern catalog ({} entries). The full \
descriptions are not inlined; use `coverage_concerns_search` to find \
concerns relevant to a topic or file, and `coverage_concerns_detail` \
to fetch full text for any specific concern_id.\n\n",
        catalog.concerns.len()
    ));
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

fn escape_pipes(text: &str) -> String {
    text.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};

    #[test]
    fn renders_section_with_each_concern() {
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let cat = flatten(&block).unwrap();
        let rendered = render_full_mode(&cat);
        assert!(rendered.starts_with("## Coverage Catalog"));
        assert!(rendered.contains("### stride:spoofing"));
        assert!(rendered.contains("**Severity:** high"));
        assert!(rendered.contains("call `coverage_mark`"));
    }

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
    }

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
}
