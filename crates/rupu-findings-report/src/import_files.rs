//! Attach finding reports written before the full profile to their findings:
//! the run behind `rupu findings import` and `POST /api/findings/import`.
//!
//! [`ImportPlan::find`] finds the report files a request names;
//! [`ImportPlan::run`] parses each with [`crate::import::parse_report`] and
//! attaches it to the summary finding its `Finding ID:` line names. A report
//! attaches whole or not at all, a finding that already has a report is left
//! alone, and every ledger write goes through
//! [`rupu_coverage::tools::attach_reports`], which takes the findings lock
//! and backs the ledger up first (`findings.jsonl.pre-import-<time>`).

use crate::import::{parse_report, retain_known_cross_references, Parsed};
use rupu_coverage::report::FindingWriteOptions;
use rupu_coverage::tools::{attach_reports, AttachItem, AttachOutcome};
use rupu_coverage::{CoveragePaths, FindingReport};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Files larger than this are not finding reports.
pub const IMPORT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// What to import.
#[derive(Debug, Clone)]
pub struct ImportRequest {
    /// Report files, or directories to search for `*.md` files.
    pub paths: Vec<PathBuf>,
    /// The finding a single report belongs to, for a report with no
    /// `Finding ID:` line. Only with one file.
    pub id: Option<String>,
    /// Parse and check everything; write nothing (no ledger change, backup
    /// or lock file).
    pub dry_run: bool,
    /// How the caller spells [`Self::id`] in messages: `--id` on the CLI.
    pub id_flag: &'static str,
    /// The most directory entries a search may visit; `None` = no limit.
    /// Past it the search stops and the import is refused
    /// ([`ImportRefused::TooManyEntries`]).
    pub max_entries: Option<usize>,
}

/// What became of one file named for import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FileOutcome {
    /// Attached (or, on a dry run, would be).
    Attached {
        finding_id: String,
    },
    Skipped {
        reason: String,
    },
    Failed {
        reason: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        details: Vec<String>,
    },
}

impl FileOutcome {
    fn failed(why: impl Into<String>) -> Self {
        Self::Failed {
            reason: why.into(),
            details: Vec::new(),
        }
    }
}

/// One file's line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportedFile {
    pub path: PathBuf,
    #[serde(flatten)]
    pub outcome: FileOutcome,
}

/// What an import did, file by file in path order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub files: Vec<ImportedFile>,
    /// The ledger backups written before each change.
    pub backups: Vec<PathBuf>,
    pub attached: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Problems that did not stop the import (an unreadable global config,
    /// whose default limits were used instead).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Why nothing was imported at all.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImportRefused {
    #[error("{0} needs exactly one report file")]
    IdNeedsOneFile(&'static str),
    #[error("no Markdown reports found under {0}")]
    NoReports(String),
    /// A named path could not be looked up, or a named directory listed.
    #[error("cannot read {}: {error}", path.display())]
    Unreadable { path: PathBuf, error: String },
    /// The search visited more than [`ImportRequest::max_entries`] entries.
    #[error("the search passed {0} directory entries; name narrower paths")]
    TooManyEntries(usize),
}

/// The report files an [`ImportRequest`] names, found and checked; nothing
/// is read or written yet.
#[derive(Debug)]
pub struct ImportPlan {
    req: ImportRequest,
    found: Found,
}

impl ImportPlan {
    /// Find the report files `req` names: the files named, plus every `*.md`
    /// under the directories named. Refused when a named path can't be read,
    /// `req.id` comes with more than one file, the search passes
    /// `req.max_entries`, or there is nothing to import.
    pub fn find(req: ImportRequest) -> Result<Self, ImportRefused> {
        let found = find_reports(&req.paths, req.max_entries)?;
        let files = &found.files;
        if req.id.is_some() && (files.len() != 1 || req.paths.iter().any(|p| !p.is_file())) {
            return Err(ImportRefused::IdNeedsOneFile(req.id_flag));
        }
        if files.is_empty() && found.unreadable.is_empty() {
            let under: Vec<String> = req.paths.iter().map(|p| p.display().to_string()).collect();
            return Err(ImportRefused::NoReports(under.join(", ")));
        }
        Ok(Self { req, found })
    }

    /// Import the reports into the findings ledgers: `ledgers` maps each
    /// finding id to the ledgers holding it, `opts` are the write limits.
    /// One ledger failing fails its own files and the others go on.
    pub fn run(
        self,
        ledgers: &HashMap<String, Vec<CoveragePaths>>,
        opts: &FindingWriteOptions,
    ) -> ImportReport {
        let Self { req, found } = self;
        let files = &found.files;
        // ledger file → the ids in it. A ledger only accepts cross-references to
        // its own findings.
        let mut ids_by_ledger: BTreeMap<PathBuf, HashSet<String>> = BTreeMap::new();
        for (id, homes) in ledgers {
            for home in homes {
                ids_by_ledger
                    .entry(home.findings.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }

        let mut lines: BTreeMap<PathBuf, FileOutcome> = BTreeMap::new();
        // Found while searching but not readable: one failed line each.
        for (path, why) in &found.unreadable {
            lines.insert(path.clone(), FileOutcome::failed(why.clone()));
        }
        // finding id → the files whose report is for it
        let mut wanted: BTreeMap<String, Vec<(PathBuf, FindingReport)>> = BTreeMap::new();
        for file in files {
            let parsed =
                read_report_file(file).and_then(|md| parse_report(&md).map_err(|e| e.to_string()));
            match parsed {
                Err(e) => {
                    lines.insert(file.clone(), FileOutcome::failed(e));
                }
                // Named with an id, the file was meant as that finding's report.
                Ok(Parsed::NotAReport) if req.id.is_some() => {
                    lines.insert(
                        file.clone(),
                        FileOutcome::failed(
                            "not a finding report (fewer than three of the report layout's sections)",
                        ),
                    );
                }
                Ok(Parsed::NotAReport) => {
                    lines.insert(
                        file.clone(),
                        FileOutcome::Skipped {
                            reason: "not a finding report".into(),
                        },
                    );
                }
                Ok(Parsed::Report { report, own_ids }) => {
                    match report_id(req.id.as_deref(), &own_ids, req.id_flag) {
                        Ok(id) => wanted.entry(id).or_default().push((file.clone(), report)),
                        Err(why) => {
                            lines.insert(file.clone(), FileOutcome::failed(why));
                        }
                    }
                }
            }
        }

        // Group by ledger; refuse an id two files claim, or one not found in
        // exactly one ledger.
        let mut by_ledger: BTreeMap<PathBuf, (CoveragePaths, Vec<(PathBuf, AttachItem)>)> =
            BTreeMap::new();
        for (id, reports) in wanted {
            if reports.len() > 1 {
                let names: Vec<String> = reports
                    .iter()
                    .map(|(f, _)| f.display().to_string())
                    .collect();
                for (f, _) in reports {
                    lines.insert(
                        f,
                        FileOutcome::failed(format!(
                            "{} reports cite {id}: {}",
                            names.len(),
                            names.join(", ")
                        )),
                    );
                }
                continue;
            }
            let (file, mut report) = reports.into_iter().next().expect("one report");
            // The same ledger may be listed twice (a workspace registered twice,
            // an id on two lines of it); only distinct ledger files count.
            let mut homes: Vec<&CoveragePaths> = Vec::new();
            for home in ledgers.get(&id).into_iter().flatten() {
                if !homes.iter().any(|h| h.findings == home.findings) {
                    homes.push(home);
                }
            }
            let paths = match homes.as_slice() {
                [one] => (*one).clone(),
                [] => {
                    lines.insert(
                        file,
                        FileOutcome::failed(format!("no finding {id} in any registered project")),
                    );
                    continue;
                }
                _ => {
                    lines.insert(
                        file,
                        FileOutcome::failed(format!("{id} is in more than one ledger")),
                    );
                    continue;
                }
            };
            // Keep cross-references to the ledger's other findings only: one to
            // the finding the report is being attached to is a self-reference.
            let known = ids_by_ledger.entry(paths.findings.clone()).or_default();
            let own = known.remove(&id);
            retain_known_cross_references(&mut report, known);
            if own {
                known.insert(id.clone());
            }
            by_ledger
                .entry(paths.findings.clone())
                .or_insert_with(|| (paths.clone(), Vec::new()))
                .1
                .push((
                    file,
                    AttachItem {
                        finding_id: id,
                        report,
                    },
                ));
        }

        let mut backups = Vec::new();
        for (_, (paths, items)) in by_ledger {
            let (files, items): (Vec<PathBuf>, Vec<AttachItem>) = items.into_iter().unzip();
            let ids: Vec<String> = items.iter().map(|i| i.finding_id.clone()).collect();
            // A ledger that cannot be written (it cannot be locked, is a
            // symlink, or changed under us) still has every report assessed:
            // those with problems list them, and only those that would have
            // attached fail with the write error.
            let batch = match attach_reports(&paths, items, opts, req.dry_run) {
                Ok(b) => b,
                Err(e) => {
                    for file in files {
                        lines.insert(
                            file,
                            FileOutcome::failed(format!(
                                "cannot read {}: {e}",
                                paths.findings.display()
                            )),
                        );
                    }
                    continue;
                }
            };
            backups.extend(batch.backup);
            let not_written = batch
                .write_error
                .map(|e| format!("cannot update {}: {e}", paths.findings.display()));
            for ((file, id), outcome) in files.into_iter().zip(ids).zip(batch.outcomes) {
                lines.insert(file, outcome_line(id, outcome, not_written.as_deref()));
            }
        }

        let count = |f: fn(&FileOutcome) -> bool| lines.values().filter(|l| f(l)).count();
        ImportReport {
            dry_run: req.dry_run,
            attached: count(|l| matches!(l, FileOutcome::Attached { .. })),
            skipped: count(|l| matches!(l, FileOutcome::Skipped { .. })),
            failed: count(|l| matches!(l, FileOutcome::Failed { .. })),
            files: lines
                .into_iter()
                .map(|(path, outcome)| ImportedFile { path, outcome })
                .collect(),
            backups,
            warnings: Vec::new(),
        }
    }
}

/// The finding a report belongs to: the one id its labelled id lines give
/// (`own`), else the id flag's. Several labelled ids, a flagged id that
/// disagrees with the labelled one, or neither, fail.
fn report_id(flag: Option<&str>, own: &[String], id_flag: &str) -> Result<String, String> {
    match (own, flag) {
        ([_, _, ..], _) => Err(format!(
            "cites several finding ids on Finding ID lines ({})",
            own.join(", ")
        )),
        ([one], Some(id)) if one != id => {
            Err(format!("the report says {one}; {id_flag} says {id}"))
        }
        ([one], _) => Ok(one.clone()),
        ([], Some(id)) => Ok(id.to_string()),
        ([], None) => Err(format!(
            "no Finding ID line; import it alone with {id_flag}"
        )),
    }
}

/// A report file's text; anything over [`IMPORT_MAX_BYTES`] is refused
/// without being read whole.
fn read_report_file(file: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .and_then(|f| f.take(IMPORT_MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() > IMPORT_MAX_BYTES {
        return Err("larger than 4 MiB".into());
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// What an [`AttachOutcome`] means for the file whose report was for `id`.
/// `not_written` is the failure line for a report that would have attached
/// to a ledger that was not written.
fn outcome_line(id: String, outcome: AttachOutcome, not_written: Option<&str>) -> FileOutcome {
    use rupu_coverage::tools::report_finding::ReportFindingError;
    match outcome {
        AttachOutcome::Attached => FileOutcome::Attached { finding_id: id },
        AttachOutcome::NotWritten => {
            FileOutcome::failed(not_written.unwrap_or("the ledger was not written"))
        }
        AttachOutcome::AlreadyHasReport => FileOutcome::Skipped {
            reason: format!("{id} already has a report"),
        },
        AttachOutcome::NotFound => {
            FileOutcome::failed(format!("no parseable finding {id} in its ledger"))
        }
        AttachOutcome::Duplicate => {
            FileOutcome::failed(format!("{id} appears more than once in its ledger"))
        }
        AttachOutcome::Rejected(ReportFindingError::Report(v)) => FileOutcome::Failed {
            reason: format!("{} problem(s) in the report", v.0.len()),
            details: v
                .0
                .iter()
                .map(|e| format!("{}: {}", e.path, e.message))
                .collect(),
        },
        AttachOutcome::Rejected(e) => FileOutcome::failed(e.to_string()),
    }
}

/// What a search for report files found.
#[derive(Debug, Default)]
struct Found {
    /// The report files, in path order, each file once however it was named.
    files: Vec<PathBuf>,
    /// Paths found while searching a directory that could not be read, with
    /// the OS error. One failed line each; the search went on without them.
    unreadable: Vec<(PathBuf, String)>,
    /// Directory entries visited so far, and the most allowed.
    visited: usize,
    max_entries: Option<usize>,
}

impl Found {
    /// Count one more entry visited; `false` once past the budget, when the
    /// search must stop.
    fn visit(&mut self) -> bool {
        self.visited += 1;
        self.max_entries.is_none_or(|max| self.visited <= max)
    }

    fn over_budget(&self) -> bool {
        self.max_entries.is_some_and(|max| self.visited > max)
    }
}

/// The files named, plus every `*.md` under the directories named, in path
/// order. While searching, hidden files and directories are skipped and
/// symlinks are not followed (a symlinked file or directory is not found).
///
/// A named path that does not exist (or cannot be looked up), and a named
/// directory that cannot be listed, fail the search, naming the path and the
/// OS error. A path found *while searching* that cannot be read is recorded
/// in [`Found::unreadable`] and the search goes on. A named *file* is only
/// looked up here: one that cannot be read fails when it is read, as its own
/// failed line.
///
/// With `max_entries`, the search stops once it has visited that many
/// directory entries and fails ([`ImportRefused::TooManyEntries`]).
fn find_reports(paths: &[PathBuf], max_entries: Option<usize>) -> Result<Found, ImportRefused> {
    fn walk(dir: &Path, found: &mut Found) {
        match std::fs::read_dir(dir) {
            Ok(entries) => walk_entries(dir, entries, found),
            Err(e) => found.unreadable.push((dir.to_path_buf(), e.to_string())),
        }
    }
    fn walk_entries(dir: &Path, entries: std::fs::ReadDir, found: &mut Found) {
        for entry in entries {
            if !found.visit() {
                return;
            }
            let entry = match entry {
                Ok(e) => e,
                // The entry itself is unknown: it is the directory that
                // could not be listed in full.
                Err(e) => {
                    found.unreadable.push((dir.to_path_buf(), e.to_string()));
                    continue;
                }
            };
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(k) => k,
                Err(e) => {
                    found.unreadable.push((path, e.to_string()));
                    continue;
                }
            };
            if kind.is_dir() {
                walk(&path, found);
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                found.files.push(path);
            }
        }
    }
    let unreadable = |p: &Path, e: std::io::Error| ImportRefused::Unreadable {
        path: p.to_path_buf(),
        error: e.to_string(),
    };
    let mut found = Found {
        max_entries,
        ..Found::default()
    };
    for p in paths {
        let meta = std::fs::metadata(p).map_err(|e| unreadable(p, e))?;
        if meta.is_dir() {
            let entries = std::fs::read_dir(p).map_err(|e| unreadable(p, e))?;
            walk_entries(p, entries, &mut found);
        } else {
            found.files.push(p.clone());
        }
        if found.over_budget() {
            return Err(ImportRefused::TooManyEntries(found.visited - 1));
        }
    }
    // One file however it was spelled (`a.md` and `./a.md`, a directory and
    // a file inside it): the first spelling found is the one kept.
    let mut seen = HashSet::new();
    found
        .files
        .retain(|f| seen.insert(std::fs::canonicalize(f).unwrap_or_else(|_| f.clone())));
    found.files.sort();
    found.unreadable.sort();
    found.unreadable.dedup();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_belongs_to_its_labelled_id_else_the_flagged_one() {
        let own = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let id = |flag, own: &[String]| report_id(flag, own, "--id");
        assert_eq!(id(None, &own(&["fnd_1"])).unwrap(), "fnd_1");
        // An id flag that agrees, or names the finding of a report with no
        // id line.
        assert_eq!(id(Some("fnd_1"), &own(&["fnd_1"])).unwrap(), "fnd_1");
        assert_eq!(id(Some("fnd_9"), &[]).unwrap(), "fnd_9");
        // The flag never overrides the report's own id line.
        assert_eq!(
            id(Some("fnd_9"), &own(&["fnd_1"])).unwrap_err(),
            "the report says fnd_1; --id says fnd_9"
        );
        assert_eq!(
            id(None, &[]).unwrap_err(),
            "no Finding ID line; import it alone with --id"
        );
        // The caller's spelling of the flag.
        assert_eq!(
            report_id(None, &[], "`id`").unwrap_err(),
            "no Finding ID line; import it alone with `id`"
        );
        for flag in [None, Some("fnd_1")] {
            assert_eq!(
                id(flag, &own(&["fnd_1", "fnd_2"])).unwrap_err(),
                "cites several finding ids on Finding ID lines (fnd_1, fnd_2)"
            );
        }
    }

    /// A path whose mode is changed for a test and put back when dropped, so
    /// the temp dir can be cleaned up even when an assertion fails first.
    #[cfg(unix)]
    struct Mode {
        path: PathBuf,
        original: std::fs::Permissions,
    }

    #[cfg(unix)]
    impl Mode {
        fn set(path: &Path, mode: u32) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let original = std::fs::metadata(path).unwrap().permissions();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
            Self {
                path: path.to_path_buf(),
                original,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for Mode {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.path, self.original.clone());
        }
    }

    fn names(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_str().unwrap().to_string())
            .collect()
    }

    fn find_reports_all(paths: &[PathBuf]) -> Result<Found, ImportRefused> {
        find_reports(paths, None)
    }

    #[test]
    fn a_search_stops_at_its_entry_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        for f in ["a.md", "b.md", "sub/c.md"] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        // a.md, b.md, sub, sub/c.md: four entries.
        assert_eq!(
            find_reports(&[root.to_path_buf()], Some(4))
                .unwrap()
                .files
                .len(),
            3
        );
        assert_eq!(
            find_reports(&[root.to_path_buf()], Some(3)).unwrap_err(),
            ImportRefused::TooManyEntries(3)
        );
        assert_eq!(
            ImportRefused::TooManyEntries(3).to_string(),
            "the search passed 3 directory entries; name narrower paths"
        );
        // A named file is not a directory entry.
        assert!(find_reports(&[root.join("a.md")], Some(0)).is_ok());
    }

    #[test]
    fn find_reports_walks_directories_for_markdown_in_path_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        for f in [
            "b.md",
            "a.MD",
            "notes.txt",
            "sub/c.md",
            "sub/deeper/d.md",
            ".hidden/e.md",
            ".dot.md",
        ] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        let found = find_reports_all(&[root.to_path_buf()]).unwrap();
        assert_eq!(
            names(root, &found.files),
            ["a.MD", "b.md", "sub/c.md", "sub/deeper/d.md"]
        );
        assert!(found.unreadable.is_empty());
        // A file named outright is taken as is (whatever its extension, and
        // even a hidden one); naming it twice, or its directory as well,
        // lists it once.
        let txt = root.join("notes.txt");
        let hidden = root.join(".dot.md");
        let found = find_reports_all(&[txt.clone(), root.join("sub"), txt.clone(), hidden.clone()])
            .unwrap();
        assert_eq!(found.files.len(), 4, "{:?}", found.files);
        assert!(found.files.contains(&txt) && found.files.contains(&hidden));
    }

    #[test]
    fn a_file_is_listed_once_however_it_is_spelled() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        let found = find_reports_all(&[
            root.join("a.md"),
            root.join("sub").join("..").join("a.md"),
            root.join(".").join("a.md"),
            root.to_path_buf(),
            root.join("sub").join("..").to_path_buf(),
        ])
        .unwrap();
        assert_eq!(found.files.len(), 1, "{:?}", found.files);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_found_while_searching_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("reports");
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(root.join("real.md"), "x").unwrap();
        std::fs::write(elsewhere.join("outside.md"), "x").unwrap();
        std::os::unix::fs::symlink(elsewhere.join("outside.md"), root.join("linked.md")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("linked_dir")).unwrap();
        // A link back to the directory being searched must not loop.
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();

        let found = find_reports_all(std::slice::from_ref(&root)).unwrap();
        assert_eq!(names(&root, &found.files), ["real.md"]);
        assert!(found.unreadable.is_empty());
        // Named outright, a link is just the file it points to.
        let found = find_reports_all(&[root.join("linked.md")]).unwrap();
        assert_eq!(found.files, [root.join("linked.md")]);
    }

    #[test]
    fn a_named_path_that_cannot_be_read_names_the_path_and_the_cause() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");
        let err = find_reports_all(&[tmp.path().to_path_buf(), missing.clone()]).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&missing.display().to_string()), "{text}");
        assert!(text.contains("No such file or directory"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_path_found_while_searching_is_recorded_and_the_search_goes_on() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("locked")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        std::fs::write(root.join("locked/b.md"), "x").unwrap();
        let _mode = Mode::set(&root.join("locked"), 0o000);
        if std::fs::read_dir(root.join("locked")).is_ok() {
            eprintln!("skipped: directory permissions are not enforced for this user");
            return;
        }

        let found = find_reports_all(&[root.to_path_buf()]).unwrap();
        assert_eq!(names(root, &found.files), ["a.md"]);
        assert_eq!(found.unreadable.len(), 1, "{:?}", found.unreadable);
        assert_eq!(found.unreadable[0].0, root.join("locked"));
        assert!(
            found.unreadable[0].1.contains("Permission denied"),
            "{:?}",
            found.unreadable
        );
        // The same directory named outright fails the search, with its path
        // and the cause.
        let err = find_reports_all(&[root.join("locked")])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&root.join("locked").display().to_string()),
            "{err}"
        );
        assert!(err.contains("Permission denied"), "{err}");
    }

    #[test]
    fn an_oversized_report_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let big = tmp.path().join("big.md");
        std::fs::write(&big, vec![b'a'; IMPORT_MAX_BYTES + 1]).unwrap();
        let err = read_report_file(&big).unwrap_err();
        assert!(err.contains("larger than 4 MiB"), "{err}");
        let ok = tmp.path().join("ok.md");
        std::fs::write(&ok, vec![b'a'; IMPORT_MAX_BYTES]).unwrap();
        assert_eq!(read_report_file(&ok).unwrap().len(), IMPORT_MAX_BYTES);
        let bin = tmp.path().join("bin.md");
        std::fs::write(&bin, [0xff, 0xfe]).unwrap();
        assert!(read_report_file(&bin).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_report_file_reports_the_os_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("locked.md");
        std::fs::write(&file, "x").unwrap();
        let _mode = Mode::set(&file, 0o000);
        if std::fs::File::open(&file).is_ok() {
            eprintln!("skipped: file permissions are not enforced for this user");
            return;
        }
        let err = read_report_file(&file).unwrap_err();
        assert!(err.contains("Permission denied"), "{err}");
    }

    #[test]
    fn each_attach_outcome_has_its_wording() {
        use rupu_coverage::report::{ArtifactError, FieldError, ReportValidationError};
        use rupu_coverage::tools::report_finding::ReportFindingError;
        let id = || "fnd_1".to_string();
        assert_eq!(
            outcome_line(id(), AttachOutcome::Attached, None),
            FileOutcome::Attached { finding_id: id() }
        );
        assert_eq!(
            outcome_line(id(), AttachOutcome::AlreadyHasReport, None),
            FileOutcome::Skipped {
                reason: "fnd_1 already has a report".into()
            }
        );
        assert_eq!(
            outcome_line(id(), AttachOutcome::NotFound, None),
            FileOutcome::failed("no parseable finding fnd_1 in its ledger")
        );
        assert_eq!(
            outcome_line(id(), AttachOutcome::Duplicate, None),
            FileOutcome::failed("fnd_1 appears more than once in its ledger")
        );
        let problems = ReportValidationError(vec![
            FieldError {
                path: "report.artifacts[0].path".into(),
                message: "must not contain `..`".into(),
            },
            FieldError {
                path: "report.impact".into(),
                message: "must not be empty".into(),
            },
        ]);
        assert_eq!(
            outcome_line(
                id(),
                AttachOutcome::Rejected(ReportFindingError::Report(problems)),
                None
            ),
            FileOutcome::Failed {
                reason: "2 problem(s) in the report".into(),
                details: vec![
                    "report.artifacts[0].path: must not contain `..`".into(),
                    "report.impact: must not be empty".into(),
                ]
            }
        );
        assert_eq!(
            outcome_line(
                id(),
                AttachOutcome::Rejected(ReportFindingError::Artifact(ArtifactError::Missing {
                    path: "out/x.txt".into()
                })),
                None
            ),
            FileOutcome::failed("artifact `out/x.txt` does not exist in the workspace")
        );
        // A report that would have attached to a ledger that was not written
        // fails with the write error.
        assert_eq!(
            outcome_line(
                id(),
                AttachOutcome::NotWritten,
                Some("cannot update l: locked")
            ),
            FileOutcome::failed("cannot update l: locked")
        );
    }

    #[test]
    fn a_file_line_serializes_its_status_beside_its_path() {
        let line = |outcome| {
            serde_json::to_value(ImportedFile {
                path: "/r/a.md".into(),
                outcome,
            })
            .unwrap()
        };
        assert_eq!(
            line(FileOutcome::Attached {
                finding_id: "fnd_1".into()
            }),
            serde_json::json!({"path": "/r/a.md", "status": "attached", "finding_id": "fnd_1"})
        );
        assert_eq!(
            line(FileOutcome::failed("x")),
            serde_json::json!({"path": "/r/a.md", "status": "failed", "reason": "x"})
        );
    }
}
