//! Merge a remote unit's run stream into a workspace's coverage ledgers
//! (spec 2026-09-30-rupu-remote-findings-transport-design.md §A4).
//!
//! Pure file I/O over one workspace: each line is re-keyed to
//! `target_id(workspace, scope_name)`, de-duplicated (findings by id, every
//! other ledger by exact record equality), and — when the unit ran on another
//! machine — its finding artifacts are recorded `stored: external` with that
//! host, since their blobs live in the host's store, not this one.

use crate::catalog::snapshot::write_snapshot;
use crate::ledger::events::FindingRecord;
use crate::ledger::manifest::read_manifests;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::stream::{append_record, Ledger, StreamLine};
use crate::ledger::target_id::target_id;
use crate::ledger::views::{read_concern_assertions, read_file_events, read_findings};
use crate::report::ArtifactStorage;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

/// Where the stream came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestSource {
    /// The registry id of the host the unit ran on; `None` for this machine
    /// (the artifact store is shared, so artifacts stay `copied`).
    pub host: Option<String>,
}

/// What a merge did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// The stream had its `begin` line — i.e. the host streams at all. When
    /// `false` nothing was written.
    pub begin_seen: bool,
    pub appended: usize,
    pub duplicates: usize,
    pub malformed: usize,
    /// Target ids written to (or checked for duplicates).
    pub targets: BTreeSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("coverage ingest I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("coverage ingest encode: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("coverage ingest catalog snapshot: {0}")]
    Catalog(String),
}

/// Keys already present in one target's ledgers.
#[derive(Default)]
struct Seen {
    findings: HashSet<String>,
    runs: HashSet<String>,
    files: HashSet<String>,
    concerns: HashSet<String>,
}

fn load_seen(paths: &CoveragePaths) -> Result<Seen, IngestError> {
    let mut s = Seen::default();
    for f in read_findings(paths)? {
        s.findings.insert(f.id);
    }
    for m in read_manifests(paths)? {
        s.runs.insert(serde_json::to_string(&m)?);
    }
    for e in read_file_events(paths)? {
        s.files.insert(serde_json::to_string(&e)?);
    }
    for a in read_concern_assertions(paths)? {
        s.concerns.insert(serde_json::to_string(&a)?);
    }
    Ok(s)
}

fn mark_external(record: &mut FindingRecord, host: &str) {
    if let Some(report) = record.report.as_mut() {
        for a in &mut report.artifacts {
            a.stored = Some(ArtifactStorage::External);
            a.host = Some(host.to_string());
        }
    }
}

/// Merge `stream` (a unit's `coverage.jsonl`) into `workspace`'s ledgers.
pub fn ingest_unit_stream(
    workspace: &Path,
    source: &IngestSource,
    stream: &[u8],
) -> Result<IngestReport, IngestError> {
    let mut report = IngestReport::default();
    let text = String::from_utf8_lossy(stream);
    let mut lines = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<StreamLine>(raw) {
            Ok(l) => lines.push(l),
            Err(_) => report.malformed += 1,
        }
    }
    report.begin_seen = lines.iter().any(|l| matches!(l, StreamLine::Begin { .. }));
    if !report.begin_seen {
        return Ok(report);
    }

    let mut targets: BTreeMap<String, (CoveragePaths, Seen)> = BTreeMap::new();
    for line in lines {
        let scope = match &line {
            StreamLine::Begin { .. } => continue,
            StreamLine::Runs { scope_name, .. }
            | StreamLine::Files { scope_name, .. }
            | StreamLine::Concerns { scope_name, .. }
            | StreamLine::Findings { scope_name, .. }
            | StreamLine::Catalog { scope_name, .. } => scope_name.clone(),
        };
        if !targets.contains_key(&scope) {
            let tid = target_id(workspace, &scope);
            let paths = CoveragePaths::new(workspace, &tid);
            paths.ensure_dir()?;
            let seen = load_seen(&paths)?;
            report.targets.insert(tid);
            targets.insert(scope.clone(), (paths, seen));
        }
        let (paths, seen) = targets.get_mut(&scope).expect("inserted above");
        let fresh = match line {
            StreamLine::Begin { .. } => continue,
            StreamLine::Runs { record, .. } => {
                let fresh = seen.runs.insert(serde_json::to_string(&record)?);
                if fresh {
                    append_record(paths, Ledger::Runs, &record)?;
                }
                fresh
            }
            StreamLine::Files { record, .. } => {
                let fresh = seen.files.insert(serde_json::to_string(&record)?);
                if fresh {
                    append_record(paths, Ledger::Files, &record)?;
                }
                fresh
            }
            StreamLine::Concerns { record, .. } => {
                let fresh = seen.concerns.insert(serde_json::to_string(&record)?);
                if fresh {
                    append_record(paths, Ledger::Concerns, &record)?;
                }
                fresh
            }
            StreamLine::Findings { mut record, .. } => {
                let fresh = seen.findings.insert(record.id.clone());
                if fresh {
                    if let Some(host) = source.host.as_deref() {
                        mark_external(&mut record, host);
                    }
                    append_record(paths, Ledger::Findings, &record)?;
                }
                fresh
            }
            StreamLine::Catalog { record, .. } => {
                // Latest run wins — the same rule the agent runner applies at
                // run start. Not counted as appended or duplicate.
                write_snapshot(&record, &paths.catalog)
                    .map_err(|e| IngestError::Catalog(e.to_string()))?;
                continue;
            }
        };
        if fresh {
            report.appended += 1;
        } else {
            report.duplicates += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{
        Attribution, FindingEvidence, FindingRecord, FindingScope, Surface,
    };
    use crate::ledger::stream::{StreamLine, STREAM_VERSION};
    use crate::report::{ArtifactKind, ArtifactRef, ArtifactStorage, FindingReport};
    use chrono::Utc;

    fn finding(id: &str, with_artifact: bool) -> FindingRecord {
        let report = with_artifact.then(|| {
            let mut r: FindingReport = serde_json::from_str(include_str!(
                "../../tests/fixtures/finding_report/valid_full.json"
            ))
            .unwrap();
            r.artifacts = vec![ArtifactRef {
                path: "poc/exploit.py".into(),
                sha256: "ab".repeat(32),
                size: 12,
                kind: Some(ArtifactKind::Text),
                stored: Some(ArtifactStorage::Copied),
                host: None,
            }];
            r
        });
        FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::File,
            summary: "s".into(),
            severity: crate::Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: "run_U1".into(),
                model: "m".into(),
                surface: Surface::Agent,
            },
            declared_at: Utc::now(),
            profile: if with_artifact {
                crate::report::FindingProfile::Full
            } else {
                crate::report::FindingProfile::Summary
            },
            report,
        }
    }

    fn stream(lines: &[StreamLine]) -> Vec<u8> {
        let mut out = String::new();
        for l in lines {
            out.push_str(&serde_json::to_string(l).unwrap());
            out.push('\n');
        }
        out.into_bytes()
    }

    fn begin() -> StreamLine {
        StreamLine::Begin {
            v: STREAM_VERSION,
            run_id: "run_U1".into(),
        }
    }

    fn findings_line(scope: &str, f: FindingRecord) -> StreamLine {
        StreamLine::Findings {
            scope_name: scope.into(),
            record: f,
        }
    }

    fn remote() -> IngestSource {
        IngestSource {
            host: Some("host_01REMOTE".into()),
        }
    }

    #[test]
    fn rekeys_by_scope_into_the_coordinator_workspace() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", false))]);
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert!(rep.begin_seen);
        assert_eq!(rep.appended, 1);
        let tid = target_id(ws.path(), "sec");
        assert!(rep.targets.contains(&tid));
        let got = read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "f1");
    }

    #[test]
    fn a_remote_hosts_artifacts_become_external_with_host() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", true))]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let tid = target_id(ws.path(), "sec");
        let got = read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
        let a = &got[0].report.as_ref().unwrap().artifacts[0];
        assert_eq!(a.stored, Some(ArtifactStorage::External));
        assert_eq!(a.host.as_deref(), Some("host_01REMOTE"));
        assert_eq!(
            a.sha256,
            "ab".repeat(32),
            "sha256 is kept for the pull's verification"
        );
        assert_eq!(a.size, 12);
        assert_eq!(a.path, "poc/exploit.py");
    }

    #[test]
    fn a_local_hosts_artifacts_stay_copied() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", true))]);
        ingest_unit_stream(ws.path(), &IngestSource { host: None }, &s).unwrap();
        let tid = target_id(ws.path(), "sec");
        let got = read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
        let a = &got[0].report.as_ref().unwrap().artifacts[0];
        assert_eq!(a.stored, Some(ArtifactStorage::Copied));
        assert_eq!(a.host, None);
    }

    #[test]
    fn reingesting_the_same_stream_appends_nothing() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", false))]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!(rep.appended, 0);
        assert_eq!(rep.duplicates, 1);
        let tid = target_id(ws.path(), "sec");
        assert_eq!(
            read_findings(&CoveragePaths::new(ws.path(), &tid))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn without_a_begin_line_nothing_is_written() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[findings_line("sec", finding("f1", false))]);
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert!(!rep.begin_seen);
        assert_eq!(rep.appended, 0);
        assert!(rep.targets.is_empty());
        assert!(!ws.path().join(".rupu").exists());
    }

    #[test]
    fn malformed_lines_are_skipped_and_counted() {
        let ws = tempfile::tempdir().unwrap();
        let mut s = stream(&[begin(), findings_line("sec", finding("f1", false))]);
        s.extend_from_slice(b"{not json\n{\"ledger\":\"nope\"}\n");
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!(rep.appended, 1);
        assert_eq!(rep.malformed, 2);
    }

    #[test]
    fn an_empty_stream_is_not_begun() {
        let ws = tempfile::tempdir().unwrap();
        let rep = ingest_unit_stream(ws.path(), &remote(), b"").unwrap();
        assert!(!rep.begin_seen);
    }

    #[test]
    fn catalog_lines_overwrite_the_targets_snapshot() {
        let ws = tempfile::tempdir().unwrap();
        // `FlatCatalog` has no `Default`; an empty one is built field by field.
        let catalog = crate::FlatCatalog {
            concerns: vec![],
            sources: std::collections::BTreeMap::new(),
            render_modes: std::collections::BTreeMap::new(),
        };
        let s = stream(&[
            begin(),
            StreamLine::Catalog {
                scope_name: "sec".into(),
                record: catalog.clone(),
            },
        ]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let tid = target_id(ws.path(), "sec");
        let got = crate::read_snapshot(&CoveragePaths::new(ws.path(), &tid).catalog).unwrap();
        assert_eq!(got, catalog);
    }
}
