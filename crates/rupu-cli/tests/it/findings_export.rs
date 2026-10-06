//! `rupu findings export` end to end: a temp `RUPU_HOME` with one registered
//! workspace whose coverage target holds a findings ledger, exported through
//! the real binary.

use assert_cmd::Command;
use rupu_coverage::{
    Attribution, CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingReport,
    FindingScope, Severity, Surface,
};
use std::path::{Path, PathBuf};

fn full_record(id: &str, severity: Severity, run: &str) -> FindingRecord {
    let report: FindingReport = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    FindingRecord {
        id: id.to_string(),
        file_path: Some("src/a.rs".to_string()),
        line_range: Some([1, 10]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: report.title.clone(),
        severity,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "why".to_string(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: run.to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: "2026-09-29T00:00:00Z".parse().unwrap(),
        profile: FindingProfile::Full,
        report: Some(report),
        tags: Vec::new(),
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
    let paths = CoveragePaths::new(&repo, "tgt1");
    paths.ensure_dir().unwrap();
    let jsonl: String = records
        .iter()
        .map(|r| serde_json::to_string(r).unwrap() + "\n")
        .collect();
    std::fs::write(&paths.findings, jsonl).unwrap();
    repo
}

/// Two full findings: `fnd_crit` is SEC-001 and `fnd_high` SEC-002.
fn seed_two(home: &Path) -> PathBuf {
    seed(
        home,
        &[
            full_record("fnd_crit", Severity::Critical, "run_crit"),
            full_record("fnd_high", Severity::High, "run_high"),
        ],
    )
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

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn project_report_as_markdown_carries_every_finding() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("out.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--to", "md", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("SEC-001"), "{text}");
    assert!(text.contains("SEC-002"), "{text}");
}

#[test]
fn markdown_is_the_default_format() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("default.out");
    let out = rupu(tmp.path())
        .args(["findings", "export", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.starts_with('#'), "not markdown: {text}");
}

#[test]
fn a_single_id_writes_one_finding_document() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("one.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--id", "fnd_high", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    // Numbered within the project, not renumbered for the selection.
    assert!(text.contains("SEC-002"), "{text}");
    assert!(!text.contains("SEC-001"), "{text}");
}

#[cfg(feature = "pdf")]
#[test]
fn a_single_id_as_pdf_writes_a_pdf() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("one.pdf");
    let out = rupu(tmp.path())
        .args([
            "findings", "export", "--id", "fnd_crit", "--to", "pdf", "-o",
        ])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let bytes = std::fs::read(&out_path).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "not a pdf");
}

#[cfg(not(feature = "pdf"))]
#[test]
fn pdf_without_pdf_support_is_refused_up_front() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("one.pdf");
    let out = rupu(tmp.path())
        .args([
            "findings", "export", "--id", "fnd_crit", "--to", "pdf", "-o",
        ])
        .arg(&out_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("without PDF support"),
        "{}",
        stderr(&out)
    );
    assert!(!out_path.exists());
}

#[test]
fn a_selection_that_matches_nothing_fails() {
    let tmp = tempfile::tempdir().unwrap();
    // Only a high finding: nothing is `critical`.
    seed(
        tmp.path(),
        &[full_record("fnd_high", Severity::High, "run_high")],
    );
    let out_path = tmp.path().join("none.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--severity", "critical", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no findings match"),
        "{}",
        stderr(&out)
    );
    assert!(!out_path.exists(), "nothing must be written");
}

#[test]
fn an_unknown_id_fails_naming_it() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "export", "--id", "fnd_nope", "-o"])
        .arg(tmp.path().join("x.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("fnd_nope"), "{}", stderr(&out));
}

#[test]
fn split_writes_a_zip_with_an_index_and_one_file_per_finding() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("out.zip");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--split", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let bytes = std::fs::read(&out_path).unwrap();
    assert!(bytes.starts_with(b"PK"), "not a zip");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    // The index plus one file per finding.
    assert_eq!(zip.len(), 3);
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.iter().any(|n| n.starts_with("SEC-001")), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("SEC-002")), "{names:?}");
}

#[test]
fn severity_narrows_the_report_and_keeps_numbers_stable() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("high.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--severity", "critical", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("SEC-001"), "{text}");
    assert!(!text.contains("SEC-002"), "{text}");
}

#[test]
fn project_accepts_a_workspace_id_or_its_path() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = seed_two(tmp.path());
    for (i, project) in ["ws1", repo.to_str().unwrap()].into_iter().enumerate() {
        let out_path = tmp.path().join(format!("p{i}.md"));
        let out = rupu(tmp.path())
            .args(["findings", "export", "--project", project, "-o"])
            .arg(&out_path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{project}: {}", stderr(&out));
        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(text.contains("SEC-001"), "{project}: {text}");
    }
}

#[test]
fn an_unknown_project_fails_naming_it() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "export", "--project", "no-such-project", "-o"])
        .arg(tmp.path().join("x.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no-such-project"), "{}", stderr(&out));
}

#[test]
fn run_scopes_to_the_findings_the_run_declared() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("run.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--run", "run_high", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("SEC-002"), "{text}");
    assert!(!text.contains("SEC-001"), "{text}");
}

#[test]
fn the_configured_prefix_replaces_sec() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    std::fs::write(
        tmp.path().join("config.toml"),
        "[findings]\nexport_id_prefix = \"ACME\"\n",
    )
    .unwrap();
    let out_path = tmp.path().join("acme.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("ACME-001"), "{text}");
    assert!(!text.contains("SEC-001"), "{text}");
}

#[test]
fn a_title_names_the_project_report() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("titled.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--title", "Q3 review", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("Q3 review"), "{text}");
}

#[test]
fn an_output_directory_receives_the_generated_file_name() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_dir = tmp.path().join("reports");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out = rupu(tmp.path())
        .args(["findings", "export", "--title", "Q3 review", "-o"])
        .arg(&out_dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out_dir.join("Q3 review.md").is_file());
}

#[test]
fn bad_arguments_are_usage_errors_and_write_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    for args in [
        ["--to", "docx"],
        ["--severity", "urgent"],
        ["--cwe", ""],
        ["--cwe", "xss"],
        ["--id", " "],
    ] {
        let out = rupu(tmp.path())
            .args(["findings", "export"])
            .args(args)
            .arg("-o")
            .arg(tmp.path().join("x.md"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert!(!tmp.path().join("x.md").exists(), "{args:?}");
    }
}

#[test]
fn the_global_format_flag_is_not_the_document_format() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    for (flag, value) in [("--format", "json"), ("--format", "md")] {
        let out = rupu(tmp.path())
            .args(["findings", "export", flag, value, "-o"])
            .arg(tmp.path().join("x.out"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{value}: {}", stderr(&out));
        assert!(!tmp.path().join("x.out").exists(), "{value}");
    }
    // Table is all `findings` takes of the global flag.
    let out = rupu(tmp.path())
        .args(["findings", "export", "--format", "json", "-o"])
        .arg(tmp.path().join("x.out"))
        .output()
        .unwrap();
    assert!(
        stderr(&out).contains("does not support"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn html_is_selected_with_to() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("out.html");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--to", "html", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.to_lowercase().contains("<html"), "{text}");
    assert!(text.contains("SEC-001"), "{text}");
    assert!(!stderr(&out).contains("[warn]"), "{}", stderr(&out));
}

#[test]
fn owner_narrows_to_the_findings_that_name_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = full_record("fnd_crit", Severity::Critical, "run_crit");
    a.report.as_mut().unwrap().ownership.owner = "team-a".to_string();
    let mut b = full_record("fnd_high", Severity::High, "run_high");
    b.report.as_mut().unwrap().ownership.owner = "team-b".to_string();
    seed(tmp.path(), &[a, b]);
    let out_path = tmp.path().join("owner.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--owner", "team-b", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("SEC-002"), "{text}");
    assert!(!text.contains("SEC-001"), "{text}");
}

#[test]
fn cwe_narrows_to_the_findings_that_carry_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = full_record("fnd_crit", Severity::Critical, "run_crit");
    a.report.as_mut().unwrap().cwe = vec!["CWE-79".to_string()];
    let mut b = full_record("fnd_high", Severity::High, "run_high");
    b.report.as_mut().unwrap().cwe = vec!["CWE-89".to_string()];
    seed(tmp.path(), &[a, b]);
    let out_path = tmp.path().join("cwe.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--cwe", "cwe-79", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("SEC-001"), "{text}");
    assert!(!text.contains("SEC-002"), "{text}");
}

#[test]
fn summary_findings_are_left_out_unless_asked_for() {
    let tmp = tempfile::tempdir().unwrap();
    let mut summary = full_record("fnd_low", Severity::Low, "run_low");
    summary.profile = FindingProfile::Summary;
    summary.report = None;
    summary.summary = "zzz-summary-only-marker".to_string();
    seed(
        tmp.path(),
        &[
            full_record("fnd_crit", Severity::Critical, "run_crit"),
            summary,
        ],
    );
    let plain = tmp.path().join("plain.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "-o"])
        .arg(&plain)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&plain).unwrap();
    assert!(text.contains("SEC-001"), "{text}");
    assert!(!text.contains("zzz-summary-only-marker"), "{text}");

    let with = tmp.path().join("with.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "--include-summaries", "-o"])
        .arg(&with)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&with).unwrap();
    assert!(text.contains("SEC-002"), "{text}");
    assert!(text.contains("zzz-summary-only-marker"), "{text}");
}

#[test]
fn the_prefix_comes_from_the_global_config_never_the_project() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    // A project (cwd) whose own config sets a prefix.
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join(".rupu")).unwrap();
    std::fs::write(
        project.join(".rupu/config.toml"),
        "[findings]\nexport_id_prefix = \"ACME\"\n",
    )
    .unwrap();

    // Project-level only: ignored, so the number is still SEC-…
    let out_path = tmp.path().join("project-only.md");
    let out = rupu_in(tmp.path(), &project)
        .args(["findings", "export", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("SEC-001"), "{text}");
    assert!(!text.contains("ACME"), "{text}");

    // Global sets one, project another: the global one wins outright.
    std::fs::write(
        tmp.path().join("config.toml"),
        "[findings]\nexport_id_prefix = \"GLOB\"\n",
    )
    .unwrap();
    let out_path = tmp.path().join("both.md");
    let out = rupu_in(tmp.path(), &project)
        .args(["findings", "export", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&out_path).unwrap();
    assert!(text.contains("GLOB-001"), "{text}");
    assert!(!text.contains("ACME"), "{text}");
    assert!(!text.contains("SEC-001"), "{text}");
}

#[test]
fn a_failed_write_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    let out_path = tmp.path().join("no-such-dir").join("out.md");
    let out = rupu(tmp.path())
        .args(["findings", "export", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("cannot write"), "{err}");
    assert!(err.contains("out.md"), "{err}");
    assert!(err.contains("No such file or directory"), "{err}");
}

#[test]
fn an_extension_that_does_not_match_the_content_warns_but_writes() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    for (args, name, warning) in [
        (vec!["--split"], "x.pdf", "writing a zip archive to"),
        (vec![], "x.txt", "writing a Markdown report to"),
        (vec!["--to", "html"], "x.md", "writing an HTML report to"),
    ] {
        let out_path = tmp.path().join(name);
        let out = rupu(tmp.path())
            .args(["findings", "export"])
            .args(&args)
            .arg("-o")
            .arg(&out_path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        let err = stderr(&out);
        assert!(err.contains("[warn]"), "{args:?}: {err}");
        assert!(err.contains(warning), "{args:?}: {err}");
        assert!(err.contains(name), "{args:?}: {err}");
        assert!(out_path.is_file(), "{args:?}");
    }
}

#[test]
fn a_matching_extension_does_not_warn() {
    let tmp = tempfile::tempdir().unwrap();
    seed_two(tmp.path());
    for (args, name) in [
        (vec![], "a.md"),
        (vec![], "b.MD"),
        (vec![], "c.markdown"),
        (vec!["--split"], "d.zip"),
        (vec!["--to", "html"], "e.html"),
    ] {
        let out = rupu(tmp.path())
            .args(["findings", "export"])
            .args(&args)
            .arg("-o")
            .arg(tmp.path().join(name))
            .output()
            .unwrap();
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        assert!(!stderr(&out).contains("[warn]"), "{name}: {}", stderr(&out));
    }
}
