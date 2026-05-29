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
}
