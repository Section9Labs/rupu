//! `rupu findings import` end to end: a temp `RUPU_HOME` with one registered
//! workspace whose coverage target holds a findings ledger of summary
//! findings, and Markdown reports imported through the real binary.

use assert_cmd::Command;
use rupu_coverage::{
    Attribution, CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope,
    Severity, Surface,
};
use std::path::{Path, PathBuf};

const PLAIN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../rupu-findings-report/tests/fixtures/import/notebin_plain.md"
));
const EXPORTED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../rupu-findings-report/tests/fixtures/import/notebin_markdown.md"
));
const ID1: &str = "fnd_01J00000000000000000000001";
const ID2: &str = "fnd_01J00000000000000000000002";
const ID9: &str = "fnd_01J00000000000000000000009";

/// A finding as recorded before the full profile: a summary, no report.
fn summary_record(id: &str) -> FindingRecord {
    FindingRecord {
        id: id.to_string(),
        file_path: Some("src/routes/notes.rs".to_string()),
        line_range: Some([40, 58]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: "note lookup skips the owner check".to_string(),
        severity: Severity::Medium,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "why".to_string(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: "run_1".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: "2026-09-29T00:00:00Z".parse().unwrap(),
        profile: FindingProfile::Summary,
        report: None,
    }
}

/// Register a workspace `ws1` at `<home>/repo` and write `records` as the
/// findings ledger of one coverage target under it. Returns the repo path.
fn seed(home: &Path, records: &[FindingRecord]) -> PathBuf {
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.canonicalize().unwrap();
    let ws = rupu_workspace::Workspace {
        id: "ws1".to_string(),
        path: repo.to_str().unwrap().to_string(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        last_run_at: None,
    };
    let dir = home.join("workspaces");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ws1.toml"), toml::to_string(&ws).unwrap()).unwrap();
    write_ledger(&repo, "tgt1", records);
    repo
}

/// (Re)write the findings ledger of coverage target `target` under `repo`.
fn write_ledger(repo: &Path, target: &str, records: &[FindingRecord]) -> CoveragePaths {
    let paths = CoveragePaths::new(repo, target);
    paths.ensure_dir().unwrap();
    let jsonl: String = records
        .iter()
        .map(|r| serde_json::to_string(r).unwrap() + "\n")
        .collect();
    std::fs::write(&paths.findings, jsonl).unwrap();
    paths
}

fn rupu(home: &Path) -> Command {
    rupu_in(home, home)
}

/// Like [`rupu`], run from `cwd`. The cwd matters: rupu layers the
/// `.rupu/config.toml` found above it, and the test binary itself runs inside
/// the rupu checkout.
fn rupu_in(home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.env("RUPU_HOME", home)
        .env("NO_COLOR", "1")
        .current_dir(cwd)
        .write_stdin("");
    cmd
}

fn ledger(repo: &Path) -> String {
    std::fs::read_to_string(CoveragePaths::new(repo, "tgt1").findings).unwrap()
}

/// The finding with `id` on `ledger_text`, as JSON.
fn line_of(ledger_text: &str, id: &str) -> serde_json::Value {
    ledger_text
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {ledger_text}"))
}

fn stdout_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

#[test]
fn imports_a_directory_and_attaches_by_cited_id() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let reports = home.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(reports.join("NB-001.md"), PLAIN).unwrap();
    std::fs::write(reports.join("README.md"), "# Reports\n\nOne per finding.\n").unwrap();
    let before = ledger(&repo);

    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&reports)
            .assert()
            .success(),
    );
    assert!(out.contains("attached"), "{out}");
    assert!(out.contains(ID1), "{out}");
    assert!(out.contains("README.md: not a finding report"), "{out}");
    assert!(out.contains("1 attached, 1 skipped, 0 failed"), "{out}");

    let after = ledger(&repo);
    let first: serde_json::Value = serde_json::from_str(after.lines().next().unwrap()).unwrap();
    assert_eq!(first["profile"], "full");
    assert_eq!(
        first["report"]["title"],
        "Notes API returns another user's note by id"
    );
    // The other finding's line is untouched.
    assert_eq!(after.lines().nth(1), before.lines().nth(1));
    let backups: Vec<_> = std::fs::read_dir(CoveragePaths::new(&repo, "tgt1").root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("pre-import"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), before);
    assert!(out.contains("backup"), "{out}");
}

#[test]
fn a_dry_run_changes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let before = ledger(&repo);
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import", "--dry-run"])
            .arg(&file)
            .assert()
            .success(),
    );
    assert!(out.contains("would attach"), "{out}");
    assert!(out.contains("1 would attach, 0 skipped, 0 failed"), "{out}");
    assert_eq!(ledger(&repo), before);
    // No backup either: nothing was written.
    assert!(std::fs::read_dir(CoveragePaths::new(&repo, "tgt1").root)
        .unwrap()
        .all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("pre-import")));
}

#[test]
fn a_report_for_an_unknown_finding_fails_the_command() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), &[summary_record(ID9)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let assert = rupu(home.path())
        .args(["findings", "import"])
        .arg(&file)
        .assert()
        .failure();
    let out = stdout_of(assert);
    assert!(out.contains("no finding"), "{out}");
    assert!(out.contains("0 attached, 0 skipped, 1 failed"), "{out}");
}

/// PLAIN without its `Finding ID:` line.
fn plain_unlabelled() -> String {
    PLAIN.replace(&format!("Finding ID: {ID1}\n"), "")
}

#[test]
fn id_names_the_finding_of_a_report_with_no_id_line() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID9)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, plain_unlabelled()).unwrap();
    rupu(home.path())
        .args(["findings", "import", "--id", ID9])
        .arg(&file)
        .assert()
        .success();
    let first: serde_json::Value =
        serde_json::from_str(ledger(&repo).lines().next().unwrap()).unwrap();
    assert_eq!(first["id"], ID9);
    assert_eq!(first["profile"], "full");
}

#[test]
fn id_that_disagrees_with_the_reports_own_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let before = ledger(&repo);
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import", "--id", ID9])
            .arg(&file)
            .assert()
            .failure(),
    );
    assert!(
        out.contains(&format!("the report says {ID1}; --id says {ID9}")),
        "{out}"
    );
    assert_eq!(ledger(&repo), before);
    // An `--id` that agrees is fine.
    rupu(home.path())
        .args(["findings", "import", "--id", ID1])
        .arg(&file)
        .assert()
        .success();
    assert_eq!(line_of(&ledger(&repo), ID1)["profile"], "full");
}

#[test]
fn a_report_that_only_mentions_another_finding_in_prose_is_refused() {
    // The final review's probe: with no id line, the one id the prose names
    // (another finding) used to be taken for the report's own, and that
    // finding was overwritten.
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(
        &file,
        plain_unlabelled().replace(
            "The get_note handler loads",
            &format!("Like {ID9}, the get_note handler loads"),
        ),
    )
    .unwrap();
    let before = ledger(&repo);
    for dry in [true, false] {
        let mut cmd = rupu(home.path());
        cmd.args(["findings", "import"]);
        if dry {
            cmd.arg("--dry-run");
        }
        let out = stdout_of(cmd.arg(&file).assert().failure());
        assert!(
            out.contains("no Finding ID line; import it alone with --id"),
            "dry={dry}: {out}"
        );
        assert!(out.contains("0 skipped, 1 failed"), "dry={dry}: {out}");
        assert_eq!(ledger(&repo), before, "dry={dry}");
    }
}

#[test]
fn a_native_finding_label_names_the_finding() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(
        &file,
        PLAIN
            .replace("Finding ID:", "Native Finding:")
            // A prose mention of another finding changes nothing.
            .replace(
                "Root Cause\n",
                &format!("Root Cause\nSee also {ID9} for the same store.\n"),
            ),
    )
    .unwrap();
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&file)
            .assert()
            .success(),
    );
    assert!(out.contains(&format!("→ {ID1}")), "{out}");
    let after = ledger(&repo);
    assert_eq!(line_of(&after, ID1)["profile"], "full");
    assert_eq!(line_of(&after, ID9)["profile"], "summary");
}

#[test]
fn id_needs_exactly_one_file() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), &[summary_record(ID1)]);
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    rupu(home.path())
        .args(["findings", "import", "--id", ID1])
        .arg(&dir)
        .assert()
        .failure();
}

#[test]
fn a_finding_that_already_has_a_report_is_skipped_and_the_ledger_is_untouched() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    rupu(home.path())
        .args(["findings", "import"])
        .arg(&file)
        .assert()
        .success();
    let once = ledger(&repo);

    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&file)
            .assert()
            .success(),
    );
    assert!(out.contains("already has a report"), "{out}");
    assert!(out.contains("0 attached, 1 skipped, 0 failed"), "{out}");
    assert_eq!(ledger(&repo), once);
}

#[test]
fn a_report_with_no_id_line_or_several_fails() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let before = ledger(&repo);
    let none = home.path().join("none.md");
    std::fs::write(&none, plain_unlabelled()).unwrap();
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&none)
            .assert()
            .failure(),
    );
    assert!(
        out.contains("none.md: no Finding ID line; import it alone with --id"),
        "{out}"
    );

    let several = home.path().join("several.md");
    std::fs::write(
        &several,
        PLAIN.replace(
            "Owner: Unknown\n",
            &format!("Owner: Unknown\nNative Finding: {ID9}\n"),
        ),
    )
    .unwrap();
    for flag in [None, Some(ID1)] {
        let mut cmd = rupu(home.path());
        cmd.args(["findings", "import"]);
        if let Some(id) = flag {
            cmd.args(["--id", id]);
        }
        let out = stdout_of(cmd.arg(&several).assert().failure());
        assert!(
            out.contains(&format!(
                "cites several finding ids on Finding ID lines ({ID1}, {ID9})"
            )),
            "{flag:?}: {out}"
        );
    }
    assert_eq!(ledger(&repo), before);
}

#[test]
fn two_files_for_one_finding_are_both_refused() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), PLAIN).unwrap();
    std::fs::write(dir.join("b.md"), PLAIN).unwrap();
    let before = ledger(&repo);
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&dir)
            .assert()
            .failure(),
    );
    assert!(out.contains("2 reports cite"), "{out}");
    assert!(out.contains("0 attached, 0 skipped, 2 failed"), "{out}");
    assert_eq!(ledger(&repo), before);
}

#[test]
fn a_file_that_is_not_a_report_is_skipped_and_one_that_cannot_parse_fails() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let before = ledger(&repo);
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.md"), "# Index\n\n- NB-001\n").unwrap();
    // Reads as a report (it has the sections) but the Impact rating is not
    // one of the allowed values.
    std::fs::write(
        dir.join("bad.md"),
        PLAIN.replace("Impact: High\n", "Impact: Enormous\n"),
    )
    .unwrap();
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&dir)
            .assert()
            .failure(),
    );
    assert!(out.contains("index.md: not a finding report"), "{out}");
    assert!(out.contains("bad.md"), "{out}");
    assert!(out.contains("0 attached, 1 skipped, 1 failed"), "{out}");
    assert_eq!(ledger(&repo), before);
}

#[test]
fn a_cross_reference_to_another_finding_in_the_ledger_is_kept() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID2)]);
    let file = home.path().join("NB-002.md");
    std::fs::write(&file, EXPORTED).unwrap();
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&file)
            .assert()
            .success(),
    );
    assert!(out.contains(ID2), "{out}");
    let refs = &line_of(&ledger(&repo), ID2)["report"]["cross_references"];
    assert_eq!(refs.as_array().map(Vec::len), Some(1), "{refs}");
    assert_eq!(refs[0]["finding_id"], ID1);
    assert_eq!(refs[0]["relation"], "prerequisite");
}

#[test]
fn a_cross_reference_outside_the_ledger_is_dropped_but_its_text_is_kept() {
    let home = tempfile::tempdir().unwrap();
    // The ledger holds the finding but not the one the report points to.
    let repo = seed(home.path(), &[summary_record(ID2)]);
    let file = home.path().join("NB-002.md");
    std::fs::write(&file, EXPORTED).unwrap();
    rupu(home.path())
        .args(["findings", "import"])
        .arg(&file)
        .assert()
        .success();
    let report = &line_of(&ledger(&repo), ID2)["report"];
    assert_eq!(report["cross_references"], "None", "{report}");
    let references = report["references"].as_str().unwrap();
    assert!(references.contains(ID1), "{references}");
}

#[test]
fn a_self_cross_reference_is_dropped() {
    let home = tempfile::tempdir().unwrap();
    // A second finding is in the ledger so that dropping is not just
    // "nothing else is known".
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(
        &file,
        PLAIN.replace(
            "Cross-References\nNone\n",
            &format!("Cross-References\n{ID1} is this finding itself.\n{ID9} is a sibling.\n"),
        ),
    )
    .unwrap();
    rupu(home.path())
        .args(["findings", "import"])
        .arg(&file)
        .assert()
        .success();
    let refs = &line_of(&ledger(&repo), ID1)["report"]["cross_references"];
    let ids: Vec<&str> = refs
        .as_array()
        .unwrap_or_else(|| panic!("{refs}"))
        .iter()
        .map(|r| r["finding_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [ID9], "{refs}");

    // Also when the id comes from `--id` rather than from the report.
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID9)]);
    let file = home.path().join("x.md");
    std::fs::write(
        &file,
        plain_unlabelled().replace(
            "Cross-References\nNone\n",
            &format!("Cross-References\n{ID9} is this finding itself.\n"),
        ),
    )
    .unwrap();
    rupu(home.path())
        .args(["findings", "import", "--id", ID9])
        .arg(&file)
        .assert()
        .success();
    let refs = &line_of(&ledger(&repo), ID9)["report"]["cross_references"];
    assert_eq!(refs, "None", "{refs}");
}

/// A path whose mode is changed for a test and put back when dropped, so the
/// temp dir can be cleaned up even when an assertion fails first.
#[cfg(unix)]
struct Restrict {
    path: PathBuf,
    original: std::fs::Permissions,
}

#[cfg(unix)]
impl Restrict {
    fn new(path: &Path, mode: u32) -> Self {
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
impl Drop for Restrict {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.original.clone());
    }
}

/// Whether a read-only directory refuses new files to this user (root is
/// not bound by it). Probes by trying, rather than reading the uid.
#[cfg(unix)]
fn creation_is_denied(dir: &Path) -> bool {
    let probe = dir.join("probe");
    match std::fs::write(&probe, "x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(_) => true,
    }
}

/// One ledger that cannot be updated fails its own files; the other ledgers
/// are still imported.
#[cfg(unix)]
#[test]
fn a_ledger_that_cannot_be_updated_fails_its_files_and_the_rest_go_on() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let locked = write_ledger(&repo, "tgt2", &[summary_record(ID9)]);
    let locked_before = std::fs::read_to_string(&locked.findings).unwrap();
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), PLAIN).unwrap();
    std::fs::write(dir.join("b.md"), PLAIN.replace(ID1, ID9)).unwrap();

    // A read-only target directory: the ledger lock cannot be created.
    let _restore = Restrict::new(&locked.root, 0o555);
    if !creation_is_denied(&locked.root) {
        eprintln!("skipped: directory permissions are not enforced for this user");
        return;
    }
    let assert = rupu(home.path())
        .args(["findings", "import"])
        .arg(&dir)
        .assert()
        .failure();

    let out = stdout_of(assert);
    assert!(out.contains("1 attached, 0 skipped, 1 failed"), "{out}");
    assert!(out.contains("cannot update"), "{out}");
    assert!(out.contains("b.md"), "{out}");
    assert_eq!(line_of(&ledger(&repo), ID1)["profile"], "full");
    assert_eq!(
        std::fs::read_to_string(&locked.findings).unwrap(),
        locked_before
    );
}

/// A ledger that cannot be written still has each report assessed: one with
/// problems lists them, and only one that would have attached fails with the
/// write error.
#[cfg(unix)]
#[test]
fn a_ledger_that_cannot_be_updated_still_lists_each_reports_problems() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let paths = CoveragePaths::new(&repo, "tgt1");
    let before = ledger(&repo);
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), PLAIN).unwrap();
    std::fs::write(
        dir.join("b.md"),
        format!("{}\nArtifacts\n- ../secrets.txt\n", PLAIN.replace(ID1, ID9)),
    )
    .unwrap();

    let _restore = Restrict::new(&paths.root, 0o555);
    if !creation_is_denied(&paths.root) {
        eprintln!("skipped: directory permissions are not enforced for this user");
        return;
    }
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&dir)
            .assert()
            .failure(),
    );
    assert!(
        out.contains(&format!(
            "a.md: cannot update {}:",
            paths.findings.display()
        )),
        "{out}"
    );
    assert!(out.contains("b.md: 1 problem(s) in the report"), "{out}");
    assert!(
        out.contains("report.artifacts[0].path: must not contain `..`"),
        "{out}"
    );
    assert!(out.contains("0 attached, 0 skipped, 2 failed"), "{out}");
    assert_eq!(ledger(&repo), before);
}

#[cfg(unix)]
#[test]
fn a_named_file_that_cannot_be_read_is_a_failed_line_and_the_rest_go_on() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let good = home.path().join("NB-001.md");
    std::fs::write(&good, PLAIN).unwrap();
    let locked = home.path().join("locked.md");
    std::fs::write(&locked, PLAIN.replace(ID1, ID9)).unwrap();
    let _restore = Restrict::new(&locked, 0o000);
    if std::fs::File::open(&locked).is_ok() {
        eprintln!("skipped: file permissions are not enforced for this user");
        return;
    }
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&good)
            .arg(&locked)
            .assert()
            .failure(),
    );
    assert!(out.contains("locked.md: Permission denied"), "{out}");
    assert!(out.contains("1 attached, 0 skipped, 1 failed"), "{out}");
    assert_eq!(line_of(&ledger(&repo), ID1)["profile"], "full");
}

#[cfg(unix)]
#[test]
fn a_dry_run_works_on_a_read_only_ledger_directory() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let paths = CoveragePaths::new(&repo, "tgt1");
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let before = ledger(&repo);

    let _restore = Restrict::new(&paths.root, 0o555);
    if !creation_is_denied(&paths.root) {
        eprintln!("skipped: directory permissions are not enforced for this user");
        return;
    }
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import", "--dry-run"])
            .arg(&file)
            .assert()
            .success(),
    );
    assert!(out.contains("1 would attach, 0 skipped, 0 failed"), "{out}");
    assert_eq!(ledger(&repo), before);
    assert!(!paths.root.join("findings.jsonl.lock").exists());
}

#[test]
fn a_report_that_fails_validation_prints_each_problem() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    // Parses as a report, but two of its artifact paths are not acceptable.
    std::fs::write(
        &file,
        format!("{PLAIN}\nArtifacts\n- ../secrets.txt\n- /etc/hosts\n"),
    )
    .unwrap();
    let before = ledger(&repo);
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&file)
            .assert()
            .failure(),
    );
    assert!(
        out.contains("NB-001.md: 2 problem(s) in the report"),
        "{out}"
    );
    let detail: Vec<&str> = out.lines().filter(|l| l.starts_with(' ')).collect();
    assert_eq!(detail.len(), 2, "{out}");
    assert!(
        detail[0].trim() == "report.artifacts[0].path: must not contain `..`",
        "{out}"
    );
    assert!(
        detail[1].trim() == "report.artifacts[1].path: must be workspace-relative, not absolute",
        "{out}"
    );
    assert!(out.contains("0 attached, 0 skipped, 1 failed"), "{out}");
    assert_eq!(ledger(&repo), before);
}

#[test]
fn a_missing_artifact_fails_a_real_import_and_a_dry_run_alike() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(
        &file,
        format!("{PLAIN}\nArtifacts\n- evidence/absent.txt\n"),
    )
    .unwrap();
    let before = ledger(&repo);
    for dry in [true, false] {
        let mut cmd = rupu(home.path());
        cmd.args(["findings", "import"]);
        if dry {
            cmd.arg("--dry-run");
        }
        let out = stdout_of(cmd.arg(&file).assert().failure());
        assert!(
            out.contains("artifact `evidence/absent.txt` does not exist in the workspace"),
            "dry={dry}: {out}"
        );
        assert!(out.contains("0 skipped, 1 failed"), "dry={dry}: {out}");
        assert_eq!(ledger(&repo), before, "dry={dry}");
    }
    // Present, the same artifact goes in.
    std::fs::create_dir_all(repo.join("evidence")).unwrap();
    std::fs::write(repo.join("evidence/absent.txt"), "GET /api/notes/1").unwrap();
    rupu(home.path())
        .args(["findings", "import", "--dry-run"])
        .arg(&file)
        .assert()
        .success();
    assert_eq!(ledger(&repo), before);
    rupu(home.path())
        .args(["findings", "import"])
        .arg(&file)
        .assert()
        .success();
    let report = &line_of(&ledger(&repo), ID1)["report"];
    assert_eq!(report["artifacts"][0]["path"], "evidence/absent.txt");
}

#[test]
fn evidence_block_files_are_verified_and_stored() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    let blocks = "![The leaked note](shots/leak.png)\n\n\
                  _Packet capture: the request and its 200 — `shots/c.pcap`_\n\n";
    let md = PLAIN.replace("\nRemediation\n", &format!("\n{blocks}Remediation\n"));
    assert!(md.contains("shots/c.pcap"));
    std::fs::write(&file, md).unwrap();
    let before = ledger(&repo);
    // A block's file must be in the workspace, on a dry run as on a real one.
    for dry in [true, false] {
        let mut cmd = rupu(home.path());
        cmd.args(["findings", "import"]);
        if dry {
            cmd.arg("--dry-run");
        }
        let out = stdout_of(cmd.arg(&file).assert().failure());
        assert!(
            out.contains("report.blocks[0].artifact.path") && out.contains("shots/leak.png"),
            "dry={dry}: {out}"
        );
        assert_eq!(ledger(&repo), before, "dry={dry}");
    }
    std::fs::create_dir_all(repo.join("shots")).unwrap();
    std::fs::write(repo.join("shots/leak.png"), b"\x89PNG\r\n\x1a\nnote").unwrap();
    std::fs::write(repo.join("shots/c.pcap"), b"\xd4\xc3\xb2\xa1capture").unwrap();
    rupu(home.path())
        .args(["findings", "import"])
        .arg(&file)
        .assert()
        .success();
    let report = &line_of(&ledger(&repo), ID1)["report"];
    let blocks = report["blocks"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 2, "{report}");
    assert_eq!(blocks[0]["kind"], "image");
    assert_eq!(blocks[0]["caption"], "The leaked note");
    assert_eq!(blocks[1]["kind"], "pcap_ref");
    assert_eq!(blocks[1]["summary"], "the request and its 200");
    for (b, path) in blocks.iter().zip(["shots/leak.png", "shots/c.pcap"]) {
        let a = &b["artifact"];
        assert_eq!(a["path"], path);
        assert_eq!(a["sha256"].as_str().map(str::len), Some(64), "{a}");
        assert_eq!(a["stored"], "copied", "{a}");
    }
    // The original claim is still the evidence.
    assert_eq!(report["evidence"].as_array().map(Vec::len), Some(1));
}

#[test]
fn an_id_present_in_two_ledgers_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let second = write_ledger(&repo, "tgt2", &[summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let (before1, before2) = (
        ledger(&repo),
        std::fs::read_to_string(&second.findings).unwrap(),
    );
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&file)
            .assert()
            .failure(),
    );
    assert!(
        out.contains(&format!("{ID1} is in more than one ledger")),
        "{out}"
    );
    assert_eq!(ledger(&repo), before1);
    assert_eq!(std::fs::read_to_string(&second.findings).unwrap(), before2);
}

#[test]
fn an_id_on_two_lines_of_one_ledger_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let before = ledger(&repo);
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&file)
            .assert()
            .failure(),
    );
    assert!(
        out.contains(&format!("{ID1} appears more than once in its ledger")),
        "{out}"
    );
    assert_eq!(ledger(&repo), before);
}

#[cfg(unix)]
#[test]
fn an_unreadable_report_file_is_a_failed_line_and_the_rest_go_on() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("NB-001.md"), PLAIN).unwrap();
    let locked = dir.join("locked.md");
    std::fs::write(&locked, PLAIN).unwrap();
    let _restore = Restrict::new(&locked, 0o000);
    if std::fs::File::open(&locked).is_ok() {
        eprintln!("skipped: file permissions are not enforced for this user");
        return;
    }
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&dir)
            .assert()
            .failure(),
    );
    assert!(out.contains("locked.md: Permission denied"), "{out}");
    assert!(out.contains("1 attached, 0 skipped, 1 failed"), "{out}");
    assert_eq!(line_of(&ledger(&repo), ID1)["profile"], "full");
}

#[cfg(unix)]
#[test]
fn an_unreadable_subdirectory_is_a_failed_line_and_the_rest_go_on() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let dir = home.path().join("reports");
    let sub = dir.join("locked");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(dir.join("NB-001.md"), PLAIN).unwrap();
    let _restore = Restrict::new(&sub, 0o000);
    if std::fs::read_dir(&sub).is_ok() {
        eprintln!("skipped: directory permissions are not enforced for this user");
        return;
    }
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&dir)
            .assert()
            .failure(),
    );
    assert!(
        out.contains(&format!("{}: Permission denied", sub.display())),
        "{out}"
    );
    assert!(out.contains("1 attached, 0 skipped, 1 failed"), "{out}");
    assert_eq!(line_of(&ledger(&repo), ID1)["profile"], "full");
}

#[test]
fn a_named_path_that_does_not_exist_fails_the_command_naming_it() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), &[summary_record(ID1)]);
    let missing = home.path().join("no-such-dir");
    let assert = rupu(home.path())
        .args(["findings", "import"])
        .arg(&missing)
        .assert()
        .failure();
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(err.contains(&missing.display().to_string()), "{err}");
    assert!(err.contains("No such file or directory"), "{err}");
}

#[test]
fn the_same_file_named_two_ways_is_one_file() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    std::fs::write(home.path().join("NB-001.md"), PLAIN).unwrap();
    // Run from the directory holding the file, so both spellings are relative.
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import", "NB-001.md", "./NB-001.md"])
            .assert()
            .success(),
    );
    assert!(out.contains("1 attached, 0 skipped, 0 failed"), "{out}");
    assert!(!out.contains("reports cite"), "{out}");
    assert_eq!(line_of(&ledger(&repo), ID1)["profile"], "full");
}

#[test]
fn a_search_that_finds_no_markdown_is_an_error() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), &[summary_record(ID1)]);
    let dir = home.path().join("reports");
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::write(dir.join("notes.txt"), "not markdown").unwrap();
    std::fs::write(dir.join(".hidden/NB-001.md"), PLAIN).unwrap();
    let assert = rupu(home.path())
        .args(["findings", "import"])
        .arg(&dir)
        .assert()
        .failure();
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        err.contains(&format!(
            "no Markdown reports found under {}",
            dir.display()
        )),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_reports_are_not_found_while_searching_a_directory() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let real = home.path().join("real");
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(real.join("NB-001.md"), PLAIN).unwrap();
    std::os::unix::fs::symlink(real.join("NB-001.md"), dir.join("linked.md")).unwrap();
    std::os::unix::fs::symlink(&real, dir.join("linked_dir")).unwrap();
    let before = ledger(&repo);
    let assert = rupu(home.path())
        .args(["findings", "import"])
        .arg(&dir)
        .assert()
        .failure();
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(err.contains("no Markdown reports found under"), "{err}");
    assert_eq!(ledger(&repo), before);
    // Named outright, the link is followed.
    rupu(home.path())
        .args(["findings", "import"])
        .arg(dir.join("linked.md"))
        .assert()
        .success();
    assert_eq!(line_of(&ledger(&repo), ID1)["profile"], "full");
}

#[test]
fn a_torn_line_elsewhere_in_the_ledger_does_not_hide_the_finding() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let findings = CoveragePaths::new(&repo, "tgt1").findings;
    // A line that is not UTF-8, as a torn write can leave.
    let mut raw = std::fs::read(&findings).unwrap();
    raw.extend_from_slice(b"{\"id\":\"fnd_torn\xff\xfe\n");
    std::fs::write(&findings, &raw).unwrap();
    let report = home.path().join("NB-001.md");
    std::fs::write(&report, PLAIN).unwrap();

    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&report)
            .assert()
            .success(),
    );
    assert!(out.contains("1 attached, 0 skipped, 0 failed"), "{out}");
    let after = std::fs::read(&findings).unwrap();
    assert!(
        after.ends_with(b"{\"id\":\"fnd_torn\xff\xfe\n"),
        "the torn line is written back as it was"
    );
    let first = std::str::from_utf8(after.split(|b| *b == b'\n').next().unwrap()).unwrap();
    let first: serde_json::Value = serde_json::from_str(first).unwrap();
    assert_eq!(first["id"], ID1);
    assert_eq!(first["profile"], "full");
}

#[test]
fn a_file_named_with_id_that_is_not_a_report_fails() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let notes = home.path().join("notes.md");
    std::fs::write(&notes, "# Notes\n\nTo do: write the report.\n").unwrap();
    let before = ledger(&repo);

    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import", "--id", ID1])
            .arg(&notes)
            .assert()
            .failure(),
    );
    assert!(out.contains("notes.md: not a finding report"), "{out}");
    assert!(out.contains("0 attached, 0 skipped, 1 failed"), "{out}");
    assert_eq!(ledger(&repo), before);
}
