use crate::catalog::types::{FlatCatalog, TouchStrength};
use crate::ledger::events::AssertionStatus;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{file_views, read_concern_assertions, read_file_events};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageRemainingInput {
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub min_strength: Option<TouchStrength>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemainingItem {
    pub concern_id: String,
    pub file_path: String,
    pub touch_modes: Vec<TouchStrength>,
    pub reason: String,
}

pub fn coverage_remaining(
    paths: &CoveragePaths,
    catalog: &FlatCatalog,
    input: CoverageRemainingInput,
) -> std::io::Result<Vec<RemainingItem>> {
    let events = read_file_events(paths)?;
    let views = file_views(&events);
    let assertions = read_concern_assertions(paths)?;
    let mut out = Vec::new();
    let concerns_to_check: Vec<_> = catalog
        .concerns
        .iter()
        .filter(|c| input.concern_id.as_deref().is_none_or(|q| c.id == q))
        .collect();
    let min_strength = input.min_strength.unwrap_or(TouchStrength::Read);

    for concern in concerns_to_check {
        // Build glob patterns once.
        let patterns: Vec<glob::Pattern> = concern
            .applicable_globs
            .iter()
            .filter_map(|p| glob::Pattern::new(p).ok())
            .collect();
        for view in &views {
            let matches_glob =
                patterns.is_empty() || patterns.iter().any(|p| p.matches(&view.path));
            if !matches_glob {
                continue;
            }
            let strong_enough = view.strongest >= min_strength;
            let asserted = assertions.iter().any(|a| {
                a.concern_id == concern.id
                    && a.file_path == view.path
                    && a.status != AssertionStatus::NotApplicable
            });
            if asserted {
                continue;
            }
            let reason = if !strong_enough {
                "below_min_strength".to_string()
            } else {
                "no_assertion".to_string()
            };
            out.push(RemainingItem {
                concern_id: concern.id.clone(),
                file_path: view.path.clone(),
                touch_modes: view.touch_modes.clone(),
                reason,
            });
        }
    }
    out.sort_by(|a, b| {
        a.concern_id
            .cmp(&b.concern_id)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{ConcernsBlock, ConcernsEntry, IncludeDirective};
    use crate::ledger::events::{Attribution, FileTouchEvent, Surface};
    use chrono::Utc;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    fn write_events(paths: &CoveragePaths, events: &[FileTouchEvent]) {
        paths.ensure_dir().unwrap();
        let body: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.files, body).unwrap();
    }

    #[test]
    fn remaining_output_is_sorted_by_concern_then_path() {
        // Feed file events whose paths are NOT in sorted order. The output
        // must still come back ordered by (concern_id, file_path), and be
        // identical across two calls — the determinism contract for the
        // live file list the model sees.
        let catalog = flatten(&ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        })
        .unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(dir.path(), "tgt");
        // Events in deliberately reversed path order.
        let events = vec![
            FileTouchEvent::Read {
                path: "src/zeta.rs".to_string(),
                line_range: [1, 10],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            },
            FileTouchEvent::Read {
                path: "src/alpha.rs".to_string(),
                line_range: [1, 10],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            },
        ];
        write_events(&paths, &events);

        let out1 = coverage_remaining(&paths, &catalog, CoverageRemainingInput::default()).unwrap();
        let out2 = coverage_remaining(&paths, &catalog, CoverageRemainingInput::default()).unwrap();
        assert_eq!(out1.len(), out2.len());

        // Identical across calls.
        let key = |r: &RemainingItem| (r.concern_id.clone(), r.file_path.clone());
        let keys1: Vec<_> = out1.iter().map(key).collect();
        let keys2: Vec<_> = out2.iter().map(key).collect();
        assert_eq!(keys1, keys2, "remaining output must be stable across calls");

        // Globally sorted by (concern_id, file_path).
        let mut sorted = keys1.clone();
        sorted.sort();
        assert_eq!(
            keys1, sorted,
            "remaining output must be sorted by (concern_id, file_path)"
        );
    }

    #[test]
    fn lists_touched_files_lacking_assertion() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let block = ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "secrets-in-source".to_string(),
                overrides: vec![],
                mode: crate::catalog::types::CatalogMode::Auto,
                filter: None,
            })],
        };
        let catalog = flatten(&block).unwrap();
        write_events(
            &paths,
            &[FileTouchEvent::Read {
                path: "src/config.rs".to_string(),
                line_range: [1, 50],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: Utc::now(),
            }],
        );
        // No assertions yet → src/config.rs should appear as remaining.
        let remaining =
            coverage_remaining(&paths, &catalog, CoverageRemainingInput::default()).unwrap();
        assert!(remaining
            .iter()
            .any(|r| r.file_path == "src/config.rs" && r.reason == "no_assertion"));
    }
}
