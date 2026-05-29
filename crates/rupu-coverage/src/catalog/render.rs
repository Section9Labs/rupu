use crate::catalog::types::FlatCatalog;

pub fn render_full_mode(catalog: &FlatCatalog) -> String {
    let mut out = String::new();
    out.push_str("## Coverage Catalog\n\n");
    out.push_str(
        "You are reviewing this workspace against the following concerns. \
For each (file × concern) you assess, call `coverage_mark` with the \
appropriate status. For each issue you discover, call `report_finding`. \
Files you read, grep, or edit are tracked automatically — you do not \
need to declare them.\n\n",
    );
    for concern in &catalog.concerns {
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

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    // Cap at 200 bytes, walking back to a UTF-8 char boundary so we
    // never slice through a multi-byte char (CWE descriptions contain
    // non-ASCII punctuation).
    let mut end_cap = trimmed.len().min(200);
    while end_cap < trimmed.len() && !trimmed.is_char_boundary(end_cap) {
        end_cap -= 1;
    }
    let mut end = end_cap;
    if let Some(idx) = trimmed[..end_cap].find(". ") {
        end = idx + 1;
    } else if let Some(idx) = trimmed[..end_cap].find(".\n") {
        end = idx + 1;
    }
    trimmed[..end].replace('\n', " ").trim().to_string()
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
    fn first_sentence_handles_trailing_period() {
        assert_eq!(first_sentence("Short summary."), "Short summary.");
        assert_eq!(first_sentence("First. Second."), "First.");
        assert_eq!(first_sentence("Multiline\nsummary."), "Multiline summary.");
    }
}
