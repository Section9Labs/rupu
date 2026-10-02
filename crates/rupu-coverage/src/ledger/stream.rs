//! A run's coverage, copied line by line into one run-scoped file so a
//! coordinator can collect a remote unit's coverage (spec
//! 2026-09-30-rupu-remote-findings-transport-design.md §A1).
//!
//! Every ledger write goes through [`append_record`]: the ledger line first,
//! then — when the paths carry a [`RunStream`] — the same record wrapped in a
//! [`StreamLine`] envelope. The stream carries `scope_name`, never
//! `target_id`: the target id hashes the host's workspace path, so the
//! coordinator recomputes it for its own workspace.

use crate::asset::Asset;
use crate::catalog::types::FlatCatalog;
use crate::ledger::events::{ConcernAssertion, FileTouchEvent, FindingRecord};
use crate::ledger::manifest::RunManifest;
use crate::ledger::paths::CoveragePaths;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The stream's file name inside `runs/<run_id>/`.
pub const STREAM_FILE: &str = "coverage.jsonl";
/// Written into the begin line; bump on an incompatible envelope change.
pub const STREAM_VERSION: u32 = 1;

/// Where a run's coverage is streamed, and the scope its records belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStream {
    pub path: PathBuf,
    pub scope_name: String,
}

/// Which coverage file a record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ledger {
    Runs,
    Files,
    Concerns,
    Findings,
    /// The engagement asset graph (`assets.jsonl`, folded last-line-wins on
    /// read, so a streamed line is the append, never a rewrite).
    Assets,
    /// The catalog snapshot (`catalog.yaml`); streamed, never appended.
    Catalog,
}

impl Ledger {
    pub fn as_str(self) -> &'static str {
        match self {
            Ledger::Runs => "runs",
            Ledger::Files => "files",
            Ledger::Concerns => "concerns",
            Ledger::Findings => "findings",
            Ledger::Assets => "assets",
            Ledger::Catalog => "catalog",
        }
    }
}

/// One line of a run stream.
// A line is parsed, handled and dropped one at a time, so the size gap
// between a findings record and a begin line costs nothing; boxing `record`
// would only make every consumer pattern-match through a `Box`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ledger", rename_all = "lowercase")]
pub enum StreamLine {
    /// First line of every stream: proves the host streams at all.
    Begin { v: u32, run_id: String },
    Runs {
        scope_name: String,
        record: RunManifest,
    },
    Files {
        scope_name: String,
        record: FileTouchEvent,
    },
    Concerns {
        scope_name: String,
        record: ConcernAssertion,
    },
    Findings {
        scope_name: String,
        record: FindingRecord,
    },
    Assets {
        scope_name: String,
        record: Asset,
    },
    Catalog {
        scope_name: String,
        record: FlatCatalog,
    },
}

/// `<runs_root>/<run_id>/coverage.jsonl`.
pub fn stream_path(runs_root: &Path, run_id: &str) -> PathBuf {
    runs_root.join(run_id).join(STREAM_FILE)
}

/// Append `line` and its newline in ONE write: with two writes, a concurrent
/// appender (a parallel `dispatch_agent` child sharing the stream, or a second
/// writer on the same ledger) could land its line between ours and our
/// newline, fusing two records into one unparseable line.
pub(crate) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(buf.as_bytes())?;
    f.flush()
}

/// `{"ledger":"<ledger>","scope_name":<json>,"record":<record_json>}` —
/// built around the already-serialized record so it is encoded once.
pub(crate) fn envelope(
    ledger: Ledger,
    scope_name: &str,
    record_json: &str,
) -> std::io::Result<String> {
    Ok(format!(
        "{{\"ledger\":\"{}\",\"scope_name\":{},\"record\":{}}}",
        ledger.as_str(),
        serde_json::to_string(scope_name)?,
        record_json
    ))
}

/// Mirror an already-serialized record into the run stream, if any. The
/// ledger line is the primary record and has already landed, so a stream
/// failure is logged loudly and not returned: returning it would make a
/// caller retry — and duplicate — a record that was written.
pub(crate) fn stream_json(paths: &CoveragePaths, ledger: Ledger, record_json: &str) {
    let Some(rs) = &paths.run_stream else {
        return;
    };
    if let Err(e) =
        envelope(ledger, &rs.scope_name, record_json).and_then(|l| append_line(&rs.path, &l))
    {
        tracing::error!(
            error = %e,
            path = %rs.path.display(),
            ledger = ledger.as_str(),
            "coverage stream write failed; a coordinator will not see this record"
        );
    }
}

/// Write `record` to its ledger under `paths`, then to the run stream.
pub fn append_record(
    paths: &CoveragePaths,
    ledger: Ledger,
    record: &impl Serialize,
) -> std::io::Result<()> {
    let file = match ledger {
        Ledger::Runs => &paths.runs,
        Ledger::Files => &paths.files,
        Ledger::Concerns => &paths.concerns,
        Ledger::Findings => &paths.findings,
        Ledger::Assets => &paths.assets,
        Ledger::Catalog => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the catalog is a snapshot, not a ledger; use stream_catalog",
            ))
        }
    };
    let json = serde_json::to_string(record)?;
    append_line(file, &json)?;
    stream_json(paths, ledger, &json);
    Ok(())
}

/// Stream the catalog snapshot a run writes at start. No-op without a stream.
pub fn stream_catalog(paths: &CoveragePaths, catalog: &FlatCatalog) -> std::io::Result<()> {
    let json = serde_json::to_string(catalog)?;
    stream_json(paths, Ledger::Catalog, &json);
    Ok(())
}

/// Write a stream's first line (creating the file and its directory).
pub fn write_stream_begin(path: &Path, run_id: &str) -> std::io::Result<()> {
    let line = serde_json::to_string(&StreamLine::Begin {
        v: STREAM_VERSION,
        run_id: run_id.to_string(),
    })?;
    append_line(path, &line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{
        AssertionStatus, Attribution, ConcernAssertion, Evidence, FindingEvidence, FindingRecord,
        FindingScope, Surface,
    };
    use crate::ledger::paths::CoveragePaths;
    use chrono::Utc;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_S1".into(),
            model: "m".into(),
            surface: Surface::Agent,
            codename: None,
            agent: None,
            provider: None,
        }
    }

    pub(crate) fn finding(id: &str) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::File,
            summary: "s".into(),
            severity: crate::Severity::Medium,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
        }
    }

    fn lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn append_record_writes_the_ledger_and_the_stream() {
        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join("runs/run_S1/coverage.jsonl");
        let paths = CoveragePaths::new(tmp.path(), "t1").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "sec".into(),
        }));
        paths.ensure_dir().unwrap();

        append_record(&paths, Ledger::Findings, &finding("f1")).unwrap();

        let ledger = lines(&paths.findings);
        assert_eq!(ledger.len(), 1);
        let back: FindingRecord = serde_json::from_str(&ledger[0]).unwrap();
        assert_eq!(back.id, "f1");

        let streamed = lines(&stream);
        assert_eq!(streamed.len(), 1);
        match serde_json::from_str::<StreamLine>(&streamed[0]).unwrap() {
            StreamLine::Findings { scope_name, record } => {
                assert_eq!(scope_name, "sec");
                assert_eq!(record.id, "f1");
            }
            other => panic!("expected a findings line, got {other:?}"),
        }
    }

    #[test]
    fn append_record_streams_an_asset_line_that_parses_back() {
        use crate::asset::{read_assets, Asset, Coordinate, Locator};

        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join("runs/run_S1/coverage.jsonl");
        let paths = CoveragePaths::new(tmp.path(), "t1").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "net".into(),
        }));
        let asset = Asset::new(
            "network:host",
            Locator(vec![Coordinate::Host("10.0.0.7".into())]),
            "10.0.0.7",
            None,
        );

        append_record(&paths, Ledger::Assets, &asset).unwrap();

        assert_eq!(read_assets(&paths.assets).unwrap(), vec![asset.clone()]);
        let streamed = lines(&stream);
        assert_eq!(streamed.len(), 1);
        assert!(
            streamed[0].starts_with(r#"{"ledger":"assets","scope_name":"net","record":"#),
            "got {}",
            streamed[0]
        );
        assert_eq!(
            serde_json::from_str::<StreamLine>(&streamed[0]).unwrap(),
            StreamLine::Assets {
                scope_name: "net".into(),
                record: asset
            }
        );
    }

    #[test]
    fn without_a_run_stream_only_the_ledger_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t1");
        paths.ensure_dir().unwrap();
        let a = ConcernAssertion {
            concern_id: "c1".into(),
            file_path: "a.rs".into(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "ok".into(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
        };
        append_record(&paths, Ledger::Concerns, &a).unwrap();
        assert_eq!(lines(&paths.concerns).len(), 1);
        assert!(paths.run_stream.is_none());
    }

    #[test]
    fn catalog_is_a_snapshot_not_a_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t1");
        let err = append_record(&paths, Ledger::Catalog, &"x").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn stream_catalog_and_begin_lines_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let stream = stream_path(&tmp.path().join("runs"), "run_S1");
        assert!(stream.ends_with("runs/run_S1/coverage.jsonl"));
        write_stream_begin(&stream, "run_S1").unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t1").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "sec".into(),
        }));
        let catalog = crate::FlatCatalog {
            concerns: vec![],
            sources: Default::default(),
            render_modes: Default::default(),
        };
        stream_catalog(&paths, &catalog).unwrap();

        let got: Vec<StreamLine> = lines(&stream)
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            got,
            vec![
                StreamLine::Begin {
                    v: STREAM_VERSION,
                    run_id: "run_S1".into()
                },
                StreamLine::Catalog {
                    scope_name: "sec".into(),
                    record: catalog
                },
            ]
        );
        assert_eq!(
            lines(&stream)[0],
            r#"{"ledger":"begin","v":1,"run_id":"run_S1"}"#
        );
    }
}
