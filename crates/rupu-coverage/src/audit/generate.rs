use crate::audit::types::{
    AuditReport, ConcernCoverage, CrossModelEntry, FileCoverage, SerendipitousCluster,
};
use crate::catalog::types::FlatCatalog;
use crate::ledger::events::{AssertionStatus, ConcernAssertion, FindingRecord};
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{file_views, read_concern_assertions, read_file_events, read_findings, FileView};
use std::collections::{BTreeMap, BTreeSet};

/// Build a full audit report for a target by joining the three ledgers
/// with the effective-catalog snapshot. A missing snapshot yields an
/// empty catalog (so the audit still reports touched files / findings).
pub fn audit(paths: &CoveragePaths) -> std::io::Result<AuditReport> {
    let catalog = crate::catalog::snapshot::read_snapshot(&paths.catalog).unwrap_or(FlatCatalog {
        concerns: vec![],
        sources: BTreeMap::new(),
        render_modes: BTreeMap::new(),
    });
    let events = read_file_events(paths)?;
    let views = file_views(&events);
    let assertions = read_concern_assertions(paths)?;
    let findings = read_findings(paths)?;

    let target_id = paths
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let concerns = concern_coverage(&catalog, &views, &assertions);
    let files = file_coverage(&catalog, &views, &assertions);
    let cross_model = cross_model(&assertions);
    let serendipitous = serendipitous(&findings);

    let total_concerns = concerns.len();
    let complete_concerns = concerns.iter().filter(|c| c.is_complete()).count();
    let total_gap_files = concerns.iter().map(|c| c.gap_files.len()).sum();

    Ok(AuditReport {
        target_id,
        concerns,
        files,
        cross_model,
        serendipitous,
        total_concerns,
        complete_concerns,
        total_gap_files,
    })
}

fn glob_match(globs: &[String], path: &str) -> bool {
    if globs.is_empty() {
        return true;
    }
    globs
        .iter()
        .any(|g| glob::Pattern::new(g).map(|p| p.matches(path)).unwrap_or(false))
}

fn concern_coverage(
    catalog: &FlatCatalog,
    views: &[FileView],
    assertions: &[ConcernAssertion],
) -> Vec<ConcernCoverage> {
    catalog
        .concerns
        .iter()
        .map(|concern| {
            let in_scope: Vec<String> = views
                .iter()
                .filter(|v| glob_match(&concern.applicable_globs, &v.path))
                .map(|v| v.path.clone())
                .collect();

            let mut asserted: BTreeSet<String> = BTreeSet::new();
            let (mut clean, mut findings, mut examined, mut not_applicable) =
                (0u32, 0u32, 0u32, 0u32);
            for a in assertions.iter().filter(|a| a.concern_id == concern.id) {
                match a.status {
                    AssertionStatus::Clean => clean += 1,
                    AssertionStatus::Finding => findings += 1,
                    AssertionStatus::Examined => examined += 1,
                    AssertionStatus::NotApplicable => not_applicable += 1,
                }
                if a.status != AssertionStatus::NotApplicable {
                    asserted.insert(a.file_path.clone());
                }
            }

            let asserted_files: Vec<String> = asserted.iter().cloned().collect();
            let gap_files: Vec<String> = in_scope
                .iter()
                .filter(|f| !asserted.contains(*f))
                .cloned()
                .collect();

            ConcernCoverage {
                concern_id: concern.id.clone(),
                name: concern.name.clone(),
                severity: concern.severity,
                in_scope_files: in_scope,
                asserted_files,
                gap_files,
                clean,
                findings,
                examined,
                not_applicable,
            }
        })
        .collect()
}

fn file_coverage(
    catalog: &FlatCatalog,
    views: &[FileView],
    assertions: &[ConcernAssertion],
) -> Vec<FileCoverage> {
    views
        .iter()
        .map(|v| {
            let asserted: Vec<String> = assertions
                .iter()
                .filter(|a| a.file_path == v.path)
                .map(|a| a.concern_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let asserted_set: BTreeSet<&String> = asserted.iter().collect();
            let missing: Vec<String> = catalog
                .concerns
                .iter()
                .filter(|c| glob_match(&c.applicable_globs, &v.path))
                .map(|c| c.id.clone())
                .filter(|id| !asserted_set.contains(id))
                .collect();
            FileCoverage {
                path: v.path.clone(),
                strongest_touch: format!("{:?}", v.strongest).to_lowercase(),
                asserted_concerns: asserted,
                missing_concerns: missing,
            }
        })
        .collect()
}

fn cross_model(assertions: &[ConcernAssertion]) -> Vec<CrossModelEntry> {
    let mut cells: BTreeMap<(String, String), BTreeMap<String, AssertionStatus>> = BTreeMap::new();
    for a in assertions {
        cells
            .entry((a.concern_id.clone(), a.file_path.clone()))
            .or_default()
            .insert(a.declared_by.model.clone(), a.status);
    }
    cells
        .into_iter()
        .filter(|(_, models)| models.len() > 1)
        .map(|((concern_id, file_path), models)| {
            let distinct: BTreeSet<AssertionStatus> = models.values().copied().collect();
            let model_statuses: Vec<(String, AssertionStatus)> = models.into_iter().collect();
            CrossModelEntry {
                concern_id,
                file_path,
                disagreement: distinct.len() > 1,
                model_statuses,
            }
        })
        .collect()
}

fn serendipitous(findings: &[FindingRecord]) -> Vec<SerendipitousCluster> {
    let mut by_theme: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in findings.iter().filter(|f| f.concern_id.is_none()) {
        by_theme
            .entry(theme_key(&f.summary))
            .or_default()
            .push(f.id.clone());
    }
    by_theme
        .into_iter()
        .map(|(theme, ids)| SerendipitousCluster {
            theme,
            count: ids.len() as u32,
            finding_ids: ids,
        })
        .collect()
}

fn theme_key(summary: &str) -> String {
    summary
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{CatalogMode, ConcernsBlock, ConcernsEntry, IncludeDirective};
    use crate::ledger::events::{Attribution, Evidence, FileTouchEvent, Surface};
    use chrono::Utc;

    fn attribution(model: &str) -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: model.to_string(),
            surface: Surface::Workflow,
        }
    }

    fn read_event(path: &str) -> FileTouchEvent {
        FileTouchEvent::Read {
            path: path.to_string(),
            line_range: [1, 50],
            tool: "read_file".to_string(),
            attribution: attribution("m1"),
            at: Utc::now(),
        }
    }

    fn assertion(
        concern: &str,
        file: &str,
        status: AssertionStatus,
        model: &str,
    ) -> ConcernAssertion {
        ConcernAssertion {
            concern_id: concern.to_string(),
            file_path: file.to_string(),
            status,
            evidence: Evidence {
                summary: "s".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: attribution(model),
            declared_at: Utc::now(),
        }
    }

    fn stride_catalog() -> FlatCatalog {
        flatten(&ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            })],
        })
        .unwrap()
    }

    #[test]
    fn concern_coverage_computes_gaps() {
        let catalog = stride_catalog();
        let views = file_views(&[read_event("src/a.rs"), read_event("src/b.rs")]);
        let assertions = vec![assertion(
            "stride:spoofing",
            "src/a.rs",
            AssertionStatus::Clean,
            "m1",
        )];
        let cov = concern_coverage(&catalog, &views, &assertions);
        let spoofing = cov
            .iter()
            .find(|c| c.concern_id == "stride:spoofing")
            .unwrap();
        assert_eq!(spoofing.clean, 1);
        // Any in-scope file other than the asserted one is a gap.
        for f in &spoofing.in_scope_files {
            if f != "src/a.rs" {
                assert!(spoofing.gap_files.contains(f));
            }
        }
        assert!(spoofing.asserted_files.contains(&"src/a.rs".to_string()));
    }

    #[test]
    fn cross_model_flags_disagreement() {
        let assertions = vec![
            assertion(
                "stride:spoofing",
                "src/a.rs",
                AssertionStatus::Clean,
                "m1",
            ),
            assertion(
                "stride:spoofing",
                "src/a.rs",
                AssertionStatus::Finding,
                "m2",
            ),
        ];
        let xm = cross_model(&assertions);
        assert_eq!(xm.len(), 1);
        assert!(xm[0].disagreement);
        assert_eq!(xm[0].model_statuses.len(), 2);
    }

    #[test]
    fn cross_model_agreement_not_flagged() {
        let assertions = vec![
            assertion(
                "stride:spoofing",
                "src/a.rs",
                AssertionStatus::Clean,
                "m1",
            ),
            assertion(
                "stride:spoofing",
                "src/a.rs",
                AssertionStatus::Clean,
                "m2",
            ),
        ];
        let xm = cross_model(&assertions);
        assert_eq!(xm.len(), 1);
        assert!(!xm[0].disagreement);
    }

    #[test]
    fn single_model_cell_not_in_cross_model() {
        let assertions = vec![assertion(
            "stride:spoofing",
            "src/a.rs",
            AssertionStatus::Clean,
            "m1",
        )];
        assert!(cross_model(&assertions).is_empty());
    }
}
