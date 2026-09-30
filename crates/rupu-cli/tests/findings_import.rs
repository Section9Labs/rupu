//! `rupu findings import` end to end: a temp `RUPU_HOME` with one registered
//! workspace whose coverage target holds a findings ledger of summary
//! findings, and Markdown reports imported through the real binary.

use assert_cmd::Command;
use rupu_coverage::{
    Attribution, CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope,
    Severity, Surface,
};
use std::path::{Path, PathBuf};

const PLAIN: &str =
    include_str!("../../rupu-findings-report/tests/fixtures/import/notebin_plain.md");
const EXPORTED: &str =
    include_str!("../../rupu-findings-report/tests/fixtures/import/notebin_markdown.md");
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

#[test]
fn id_overrides_the_cited_id_for_a_single_file() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID9)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
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
fn a_report_citing_no_finding_id_or_several_needs_id() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), &[summary_record(ID1), summary_record(ID9)]);
    let none = home.path().join("none.md");
    std::fs::write(&none, PLAIN.replace(&format!("Finding ID: {ID1}\n"), "")).unwrap();
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&none)
            .assert()
            .failure(),
    );
    assert!(out.contains("cites no finding id"), "{out}");
    assert!(out.contains("--id"), "{out}");

    let several = home.path().join("several.md");
    std::fs::write(
        &several,
        PLAIN.replace(
            "Root Cause\n",
            &format!("Root Cause\nSee also {ID9} for the same store.\n"),
        ),
    )
    .unwrap();
    let out = stdout_of(
        rupu(home.path())
            .args(["findings", "import"])
            .arg(&several)
            .assert()
            .failure(),
    );
    assert!(out.contains("cites several finding ids"), "{out}");
    assert!(out.contains(ID1) && out.contains(ID9), "{out}");
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
        PLAIN.replace(
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

/// One ledger that cannot be updated fails its own files; the other ledgers
/// are still imported.
#[cfg(unix)]
#[test]
fn a_ledger_that_cannot_be_updated_fails_its_files_and_the_rest_go_on() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let locked = write_ledger(&repo, "tgt2", &[summary_record(ID9)]);
    let locked_before = std::fs::read_to_string(&locked.findings).unwrap();
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), PLAIN).unwrap();
    std::fs::write(dir.join("b.md"), PLAIN.replace(ID1, ID9)).unwrap();

    // A read-only target directory: the ledger lock cannot be created.
    std::fs::set_permissions(&locked.root, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = std::fs::write(locked.root.join("probe"), "x");
    if probe.is_ok() {
        // Running as a user the mode does not bind (root): nothing to test.
        std::fs::set_permissions(&locked.root, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let assert = rupu(home.path())
        .args(["findings", "import"])
        .arg(&dir)
        .assert()
        .failure();
    std::fs::set_permissions(&locked.root, std::fs::Permissions::from_mode(0o755)).unwrap();

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
