//! Merge a remote unit's run stream into a workspace's coverage ledgers
//! (spec 2026-09-30-rupu-remote-findings-transport-design.md §A4).
//!
//! Pure file I/O over one workspace: each line is re-keyed to
//! `target_id(workspace, scope_name)`, de-duplicated (findings by id, every
//! other ledger by exact record equality — asset lines by how many times each
//! line occurs, since the store folds last-line-wins and a state may
//! legitimately recur), and — when the unit ran on another machine — its
//! finding artifacts are recorded `stored: external` with that host, since
//! their blobs live in the host's store, not this one.

use crate::asset::store::read_asset_lines;
use crate::asset::AssetStoreError;
use crate::catalog::snapshot::write_snapshot;
use crate::ledger::events::FindingRecord;
use crate::ledger::manifest::read_manifests;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::stream::{append_record, Ledger, StreamLine};
use crate::ledger::target_id::target_id;
use crate::ledger::views::{read_concern_assertions, read_file_events, read_findings};
use crate::report::ArtifactStorage;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    #[error("coverage ingest asset ledger: {0}")]
    Assets(#[from] AssetStoreError),
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
    assets: Occurrences,
}

/// Whether a serialized ledger line is new to the target being merged into.
trait SeenLines {
    /// Record one more sighting of `line` in the stream being merged; `true`
    /// when it must be appended.
    fn is_new(&mut self, line: String) -> bool;
}

/// Exact equality, once: a repeat of a line is a duplicate. Right for ledgers
/// that are read back as a set or in time order from the records themselves.
impl SeenLines for HashSet<String> {
    fn is_new(&mut self, line: String) -> bool {
        self.insert(line)
    }
}

/// Exact equality by occurrence. The asset store folds last-line-wins, so
/// order and multiplicity matter: a unit that stamps an asset, marks it, then
/// stamps it again recorded the same first line twice, and dropping the
/// repeat would leave the mark as the coordinator's last word. The k-th
/// occurrence of a line in the stream is new only when the target already
/// holds fewer than k copies of it - so re-merging a stream, merging one that
/// grew since an earlier collection, or retrying an interrupted merge all
/// converge on the unit's own fold without duplicating a line.
#[derive(Default)]
struct Occurrences {
    /// Copies of each line in the target's ledger before this merge.
    on_disk: HashMap<String, usize>,
    /// Occurrences of each line seen so far in the stream being merged.
    merged: HashMap<String, usize>,
}

impl SeenLines for Occurrences {
    fn is_new(&mut self, line: String) -> bool {
        let on_disk = self.on_disk.get(&line).copied().unwrap_or(0);
        let nth = self.merged.entry(line).or_insert(0);
        *nth += 1;
        on_disk < *nth
    }
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
    for a in read_asset_lines(&paths.assets)? {
        *s.assets
            .on_disk
            .entry(serde_json::to_string(&a)?)
            .or_insert(0) += 1;
    }
    Ok(s)
}

/// Append `record` to `ledger` unless `seen` says the target already has it.
/// Whether the record was new.
fn append_unseen(
    paths: &CoveragePaths,
    ledger: Ledger,
    seen: &mut impl SeenLines,
    record: &impl Serialize,
) -> Result<bool, IngestError> {
    let fresh = seen.is_new(serde_json::to_string(record)?);
    if fresh {
        append_record(paths, ledger, record)?;
    }
    Ok(fresh)
}

fn mark_external(record: &mut FindingRecord, host: &str) {
    if let Some(report) = record.report.as_mut() {
        // Every file the report references, evidence-block files included.
        for a in report.artifact_refs_mut() {
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
            | StreamLine::Assets { scope_name, .. }
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
                append_unseen(paths, Ledger::Runs, &mut seen.runs, &record)?
            }
            StreamLine::Files { record, .. } => {
                append_unseen(paths, Ledger::Files, &mut seen.files, &record)?
            }
            StreamLine::Concerns { record, .. } => {
                append_unseen(paths, Ledger::Concerns, &mut seen.concerns, &record)?
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
            StreamLine::Assets { record, .. } => {
                append_unseen(paths, Ledger::Assets, &mut seen.assets, &record)?
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
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: Utc::now(),
            profile: if with_artifact {
                crate::report::FindingProfile::Full
            } else {
                crate::report::FindingProfile::Summary
            },
            report,
            tags: Vec::new(),
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
    fn remote_ingest_marks_block_artifacts_external_with_the_host() {
        let ws = tempfile::tempdir().unwrap();
        let mut f = finding("f1", true);
        f.report.as_mut().unwrap().blocks = vec![crate::report::EvidenceBlock::Image {
            artifact: ArtifactRef {
                path: "shots/login.png".into(),
                sha256: "cd".repeat(32),
                size: 7,
                kind: Some(ArtifactKind::Binary),
                stored: Some(ArtifactStorage::Copied),
                host: None,
            },
            caption: None,
        }];
        let s = stream(&[begin(), findings_line("sec", f)]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let tid = target_id(ws.path(), "sec");
        let got = read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
        let rep = got[0].report.as_ref().unwrap();
        let a = rep.blocks[0].artifact().expect("an image block has a file");
        assert_eq!(a.stored, Some(ArtifactStorage::External));
        assert_eq!(a.host.as_deref(), Some("host_01REMOTE"));
        assert_eq!(
            a.sha256,
            "cd".repeat(32),
            "sha256 is kept for the pull's verification"
        );
        assert_eq!(a.size, 7);
        // The PoC artifact is still marked too, and block files did not join it.
        assert_eq!(rep.artifacts.len(), 1);
        assert_eq!(rep.artifacts[0].host.as_deref(), Some("host_01REMOTE"));
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

    fn asset(host: &str, depth: Option<&str>) -> crate::asset::Asset {
        let mut a = crate::asset::Asset::new(
            "network:host",
            crate::asset::Locator(vec![crate::asset::Coordinate::Host(host.into())]),
            host,
            None,
        );
        a.depth = depth.map(str::to_string);
        a
    }

    fn assets_line(scope: &str, a: crate::asset::Asset) -> StreamLine {
        StreamLine::Assets {
            scope_name: scope.into(),
            record: a,
        }
    }

    #[test]
    fn asset_lines_land_in_the_coordinator_targets_asset_ledger() {
        let ws = tempfile::tempdir().unwrap();
        let a = asset("10.0.0.7", Some("discovered"));
        let s = stream(&[begin(), assets_line("net", a.clone())]);
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!((rep.appended, rep.duplicates), (1, 0));
        let tid = target_id(ws.path(), "net");
        assert!(rep.targets.contains(&tid));
        let paths = CoveragePaths::new(ws.path(), &tid);
        assert_eq!(crate::asset::read_assets(&paths.assets).unwrap(), vec![a]);
        assert!(
            files_named(ws.path(), crate::ledger::stream::STREAM_FILE).is_empty(),
            "an ingest never re-streams what it merges"
        );
    }

    /// Every file called `name` anywhere under `dir`.
    fn files_named(dir: &Path, name: &str) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(files_named(&path, name));
            } else if path.file_name().is_some_and(|n| n == name) {
                found.push(path);
            }
        }
        found
    }

    #[test]
    fn reingesting_an_asset_stream_leaves_the_ledger_byte_identical() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[
            begin(),
            assets_line("net", asset("10.0.0.7", Some("discovered"))),
            assets_line("net", asset("10.0.0.8", None)),
        ]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let tid = target_id(ws.path(), "net");
        let file = CoveragePaths::new(ws.path(), &tid).assets;
        let before = std::fs::read(&file).unwrap();
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!((rep.appended, rep.duplicates), (0, 2));
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }

    #[test]
    fn a_recollected_stream_appends_only_the_lines_the_ledger_lacks() {
        // Attempt 1 collected the unit's stream while it held [A0, At]; the
        // unit then re-asserted A0 (a stamp that erased the depth), and a
        // retry collects [A0, At, A0]. The third line is a state the
        // coordinator has never seen, so it must land - a plain "is this line
        // anywhere in the ledger" check would drop it and leave `At` as the
        // last word while the unit's own fold ends at A0.
        let ws = tempfile::tempdir().unwrap();
        let first = stream(&[
            begin(),
            assets_line("net", asset("10.0.0.7", None)),
            assets_line("net", asset("10.0.0.7", Some("tested"))),
        ]);
        let again = stream(&[
            begin(),
            assets_line("net", asset("10.0.0.7", None)),
            assets_line("net", asset("10.0.0.7", Some("tested"))),
            assets_line("net", asset("10.0.0.7", None)),
        ]);
        ingest_unit_stream(ws.path(), &remote(), &first).unwrap();
        let rep = ingest_unit_stream(ws.path(), &remote(), &again).unwrap();
        assert_eq!((rep.appended, rep.duplicates), (1, 2));
        let tid = target_id(ws.path(), "net");
        let file = CoveragePaths::new(ws.path(), &tid).assets;
        assert_eq!(
            crate::asset::read_assets(&file).unwrap(),
            vec![asset("10.0.0.7", None)]
        );

        // Re-collecting the full stream once more is a byte-identical no-op.
        let before = std::fs::read(&file).unwrap();
        let rep = ingest_unit_stream(ws.path(), &remote(), &again).unwrap();
        assert_eq!((rep.appended, rep.duplicates), (0, 3));
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }

    #[test]
    fn an_interrupted_asset_merge_converges_on_retry() {
        // The first merge died after the first two lines reached the ledger.
        let ws = tempfile::tempdir().unwrap();
        let full = [
            assets_line("net", asset("10.0.0.7", None)),
            assets_line("net", asset("10.0.0.7", Some("tested"))),
            assets_line("net", asset("10.0.0.7", None)),
        ];
        let tid = target_id(ws.path(), "net");
        let paths = CoveragePaths::new(ws.path(), &tid);
        for line in &full[..2] {
            if let StreamLine::Assets { record, .. } = line {
                append_record(&paths, Ledger::Assets, record).unwrap();
            }
        }
        let s = stream(&[&[begin()], &full[..]].concat());
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!((rep.appended, rep.duplicates), (1, 2));
        assert_eq!(
            crate::asset::read_assets(&paths.assets).unwrap(),
            vec![asset("10.0.0.7", None)]
        );
    }

    #[test]
    fn a_repeated_asset_state_keeps_the_streams_last_line_wins_fold() {
        // The unit marked the host, stamped it again with no depth (a later
        // finding about it), and the store folds last-line-wins: no depth.
        // The merge must preserve that order, not drop the repeat as a
        // duplicate of the first line and leave the mark as the last word.
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[
            begin(),
            assets_line("net", asset("10.0.0.7", None)),
            assets_line("net", asset("10.0.0.7", Some("tested"))),
            assets_line("net", asset("10.0.0.7", None)),
        ]);
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!((rep.appended, rep.duplicates), (3, 0));
        let tid = target_id(ws.path(), "net");
        let paths = CoveragePaths::new(ws.path(), &tid);
        let folded = crate::asset::read_assets(&paths.assets).unwrap();
        assert_eq!(folded, vec![asset("10.0.0.7", None)]);

        // And re-merging is still idempotent against the unfolded lines: the
        // already-superseded first state is not re-appended on top.
        let again = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!((again.appended, again.duplicates), (0, 3));
        assert_eq!(crate::asset::read_assets(&paths.assets).unwrap(), folded);
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
