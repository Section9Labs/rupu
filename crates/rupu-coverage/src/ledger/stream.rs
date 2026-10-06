//! A run's coverage, copied line by line into one run-scoped file so a
//! coordinator can collect a remote unit's coverage (spec
//! 2026-09-30-rupu-remote-findings-transport-design.md §A1).
//!
//! Every ledger write goes through [`append_record`]: the ledger line first,
//! then — when the paths carry a [`RunStream`] — the same record wrapped in a
//! [`StreamLine`] envelope. The stream carries `scope_name`, never
//! `target_id`: the target id hashes the host's workspace path, so the
//! coordinator recomputes it for its own workspace.
//!
//! The async file-touch writer (`ledger::writer`) keeps the same ledger-first
//! rule: it writes AND flushes each ledger line (a buffered file reports a
//! failed write only on a later write or flush) and streams the line only if
//! both succeeded. A ledger failure is logged and the line is not streamed.

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
    /// Finding-tag events (the workspace's `finding_tags.jsonl`), written only by `ledger::tags`.
    Tags,
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
            Ledger::Tags => "tags",
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
    Begin {
        v: u32,
        run_id: String,
    },
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
/// ledger line is the primary record and has already been written (flushed,
/// for the async writer's buffered handle), so a stream failure is logged
/// loudly and not returned: returning it would make a caller retry — and
/// duplicate — a record that was written.
pub(crate) fn stream_json(paths: &CoveragePaths, ledger: Ledger, record_json: &str) {
    stream_json_to(paths.run_stream.as_ref(), ledger, record_json)
}

/// [`stream_json`] for a writer that is not a `CoveragePaths` (the tag log).
pub(crate) fn stream_json_to(run_stream: Option<&RunStream>, ledger: Ledger, record_json: &str) {
    let Some(rs) = run_stream else {
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

/// Write `record` to its ledger under `paths`, then to the run stream. A
/// findings record is appended under the findings ledger lock
/// ([`lock_findings`]), the one ledger an import rewrites.
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
        Ledger::Tags => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "finding-tag events are written by ledger::tags::apply, not append_record",
            ))
        }
    };
    let json = serde_json::to_string(record)?;
    if ledger == Ledger::Findings {
        findings_locked(paths, std::fs::File::lock, || append_line(file, &json))?;
    } else {
        append_line(file, &json)?;
    }
    stream_json(paths, ledger, &json);
    Ok(())
}

/// Run `append`, an append to the findings ledger, holding the ledger lock
/// (taken with `lock`) so it cannot interleave with an import's rewrite
/// (`tools::attach_report`).
///
/// Where the filesystem cannot lock at all (some network and FUSE mounts:
/// see [`lock_unsupported`]), where the lock file cannot be opened (a
/// directory this user may not create files in), or where it could only be
/// opened read-only and an exclusive lock needs it writable (`EBADF`, as
/// NFS's locks do), the line is appended without the lock and a warning is
/// logged, as before the lock existed: losing the finding would be worse.
/// An import needs the same lock, so it either cannot run there or runs as a
/// user who can take it; its length check then almost always catches the
/// unlocked append (not one that lands between that check and its rename).
/// Any other lock failure is an error, and nothing is appended.
fn findings_locked(
    paths: &CoveragePaths,
    lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    append: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    locked_at(
        &paths.root.join("findings.jsonl.lock"),
        &paths.findings,
        lock,
        append,
    )
}

/// Run `f` holding the sidecar lock at `lock_path`, taken with `lock`, with
/// the fallback [`findings_locked`] documents: where the filesystem cannot
/// lock, or the lock file cannot be opened, `f` runs unlocked and a warning
/// names `ledger`; any other lock failure is an error and `f` does not run.
pub(crate) fn locked_at<T, E: From<std::io::Error>>(
    lock_path: &Path,
    ledger: &Path,
    lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let lock_file = match open_lock_at(lock_path) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!(
                error = %e,
                ledger = ?ledger,
                "cannot open the ledger's lock file; writing without the lock"
            );
            None
        }
    };
    if let Some((Err(e), read_only)) = lock_file.as_ref().map(|(f, ro)| (lock(f), *ro)) {
        let needs_write =
            read_only && e.raw_os_error() == Some(rustix::io::Errno::BADF.raw_os_error());
        if !(lock_unsupported(&e) || needs_write) {
            return Err(e.into());
        }
        tracing::warn!(
            error = %e,
            ledger = ?ledger,
            "this filesystem cannot lock the ledger; writing without the lock"
        );
    }
    let out = f();
    // Released after the write.
    drop(lock_file);
    out
}

/// Whether a lock error means the filesystem cannot lock at all, rather than
/// a failure to report: `ErrorKind::Unsupported` (`ENOSYS`, `EOPNOTSUPP`),
/// or one of [`no_lock_errnos`].
fn lock_unsupported(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::Unsupported
        || e.raw_os_error()
            .is_some_and(|n| no_lock_errnos().contains(&n))
}

/// `ENOLCK`, `ENOTSUP` and `EOPNOTSUPP` on this platform (the last two are
/// the same number on Linux).
fn no_lock_errnos() -> [i32; 3] {
    use rustix::io::Errno;
    [
        Errno::NOLCK.raw_os_error(),
        Errno::NOTSUP.raw_os_error(),
        Errno::OPNOTSUPP.raw_os_error(),
    ]
}

/// Serializes the writers of one target's findings ledger. The lock is a
/// sidecar file, not the ledger itself: `attach_reports` replaces the ledger
/// by rename, and a lock held on the replaced file would not exclude the
/// next writer. Released when the returned handle drops.
pub(crate) fn lock_findings(paths: &CoveragePaths) -> std::io::Result<std::fs::File> {
    let (f, _) = open_lock_file(paths)?;
    f.lock()?;
    Ok(f)
}

fn open_lock_file(paths: &CoveragePaths) -> std::io::Result<(std::fs::File, bool)> {
    open_lock_at(&paths.root.join("findings.jsonl.lock"))
}

/// The lock sidecar at `path` (its directory created if missing), and
/// whether it was opened read-only.
///
/// A lock needs only a handle on the file on most filesystems, so one this
/// user may not write (created by another user, say an import run with
/// `sudo`) is opened read-only instead.
fn open_lock_at(path: &Path) -> std::io::Result<(std::fs::File, bool)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
    {
        Ok(f) => Ok((f, false)),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            std::fs::File::open(path).map(|f| (f, true)).map_err(|_| e)
        }
        Err(e) => Err(e),
    }
}

/// Stream the catalog snapshot a run writes at start. No-op without a stream.
/// Infallible like every stream write: a problem here is logged, never
/// returned, so it cannot fail the run that is only mirroring its coverage.
pub fn stream_catalog(paths: &CoveragePaths, catalog: &FlatCatalog) {
    match serde_json::to_string(catalog) {
        Ok(json) => stream_json(paths, Ledger::Catalog, &json),
        Err(e) => tracing::error!(
            error = %e,
            ledger = Ledger::Catalog.as_str(),
            "coverage catalog serialization failed; a coordinator will not see this record"
        ),
    }
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

    /// An append to the findings ledger through its lock, as
    /// `append_record` makes one.
    fn append_line_locked(
        paths: &CoveragePaths,
        line: &str,
        lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        findings_locked(paths, lock, || append_line(&paths.findings, line))
    }

    #[test]
    fn an_append_goes_ahead_unlocked_only_where_the_filesystem_cannot_lock() {
        use std::io::{Error, ErrorKind};
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        // A filesystem that cannot lock: the finding is still recorded.
        append_line_locked(&paths, "{\"n\":1}", |_| {
            Err(Error::from(ErrorKind::Unsupported))
        })
        .expect("unsupported locking falls back to an unlocked append");
        let mut expected = "{\"n\":1}\n".to_string();
        for errno in no_lock_errnos() {
            append_line_locked(&paths, "{\"n\":2}", |_| {
                Err(Error::from_raw_os_error(errno))
            })
            .unwrap_or_else(|e| panic!("errno {errno}: {e}"));
            expected.push_str("{\"n\":2}\n");
        }
        assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), expected);
        // Any other lock failure is an error, and nothing is appended.
        let err = append_line_locked(&paths, "{\"n\":3}", |_| {
            Err(Error::from(ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), expected);
        // And the real lock is taken when it can be.
        append_line_locked(&paths, "{\"n\":4}", std::fs::File::lock).unwrap();
        assert!(std::fs::read_to_string(&paths.findings)
            .unwrap()
            .ends_with("{\"n\":4}\n"));
    }

    #[test]
    fn an_append_goes_ahead_unlocked_when_the_lock_file_cannot_be_opened() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        // Something that cannot be opened as the lock file.
        std::fs::create_dir_all(paths.root.join("findings.jsonl.lock")).unwrap();
        append_line_locked(&paths, "{\"n\":1}", std::fs::File::lock)
            .expect("the finding is still recorded");
        assert_eq!(
            std::fs::read_to_string(&paths.findings).unwrap(),
            "{\"n\":1}\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_file_this_user_cannot_write_is_locked_through_a_read_only_handle() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return; // root can write any file: nothing to fall back from
        }
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        paths.ensure_dir().unwrap();
        let sidecar = paths.root.join("findings.jsonl.lock");
        std::fs::write(&sidecar, "").unwrap();
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o444)).unwrap();

        let held = lock_findings(&paths).expect("locked through a read-only handle");
        let other = std::fs::File::open(&sidecar).unwrap();
        assert!(
            matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            "the read-only handle's lock excludes other writers"
        );
        drop(held);
        append_line_locked(&paths, "{\"n\":1}", std::fs::File::lock).unwrap();
        assert_eq!(
            std::fs::read_to_string(&paths.findings).unwrap(),
            "{\"n\":1}\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_lock_handle_refused_as_unwritable_appends_unlocked() {
        use std::io::{Error, ErrorKind};
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return; // root opens the lock file writable: no read-only handle
        }
        let ebadf = || Error::from_raw_os_error(rustix::io::Errno::BADF.raw_os_error());
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        paths.ensure_dir().unwrap();
        // A writable handle refused with EBADF is a real error.
        let err = append_line_locked(&paths, "{\"n\":1}", |_| Err(ebadf())).unwrap_err();
        assert_ne!(err.kind(), ErrorKind::Unsupported);
        assert!(!paths.findings.exists());
        // A read-only one (the lock file is another user's) is NFS refusing
        // an exclusive lock it cannot take: the finding is still recorded.
        let sidecar = paths.root.join("findings.jsonl.lock");
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o444)).unwrap();
        append_line_locked(&paths, "{\"n\":1}", |_| Err(ebadf())).unwrap();
        assert_eq!(
            std::fs::read_to_string(&paths.findings).unwrap(),
            "{\"n\":1}\n"
        );
    }

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
            tags: Vec::new(),
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
        stream_catalog(&paths, &catalog);

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
