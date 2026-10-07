//! `rupu findings list|tag|tags` end to end, through the real binary, over a
//! temp `RUPU_HOME` with registered workspaces.

use assert_cmd::Command;
use rupu_coverage::{
    Attribution, CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope,
    Severity, Surface,
};
use std::path::{Path, PathBuf};

fn record(id: &str, severity: Severity) -> FindingRecord {
    FindingRecord {
        id: id.into(),
        file_path: Some("src/app.rs".into()),
        line_range: Some([5, 9]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: format!("{id} summary"),
        severity,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "why".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: format!("run_{id}"),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: "2026-10-01T00:00:00Z".parse().unwrap(),
        profile: FindingProfile::Summary,
        report: None,
        tags: vec![],
    }
}

/// Register workspace `ws_id` at `<home>/<dir>` with `records` in one target.
fn seed(home: &Path, ws_id: &str, dir: &str, records: &[FindingRecord]) -> PathBuf {
    let repo = home.join(dir);
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.canonicalize().unwrap();
    let ws = rupu_workspace::Workspace {
        id: ws_id.to_string(),
        path: repo.to_str().unwrap().to_string(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        last_run_at: None,
    };
    let wsdir = home.join("workspaces");
    std::fs::create_dir_all(&wsdir).unwrap();
    std::fs::write(
        wsdir.join(format!("{ws_id}.toml")),
        toml::to_string(&ws).unwrap(),
    )
    .unwrap();
    let paths = CoveragePaths::new(&repo, "tgt1");
    paths.ensure_dir().unwrap();
    let jsonl: String = records
        .iter()
        .map(|r| serde_json::to_string(r).unwrap() + "\n")
        .collect();
    std::fs::write(&paths.findings, jsonl).unwrap();
    repo
}

fn seed_one(home: &Path) -> PathBuf {
    seed(
        home,
        "ws1",
        "repo",
        &[
            record("fnd_a", Severity::High),
            record("fnd_b", Severity::Low),
        ],
    )
}

fn rupu(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.env("RUPU_HOME", home)
        .env("NO_COLOR", "1")
        .current_dir(home)
        .write_stdin("");
    cmd
}

fn ok(out: &std::process::Output) -> String {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn tag_then_filter_by_tag_and_untagged() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args([
            "findings",
            "tag",
            "fnd_a",
            "--add",
            "Class:SQLi",
            "--add",
            "needs-poc",
        ])
        .output()
        .unwrap();
    assert!(ok(&out).contains("fnd_a: (none) → class:sqli, needs-poc"));

    let out = rupu(tmp.path())
        .args(["findings", "list", "--ids-only", "tag:class:sqli"])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
    let out = rupu(tmp.path())
        .args(["findings", "list", "--ids-only", "-has:tags"])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_b\n");
}

#[test]
fn ids_can_be_piped_on_stdin() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "tag", "-", "--add", "triaged"])
        .write_stdin("fnd_a\n\nfnd_b\n")
        .output()
        .unwrap();
    ok(&out);
    let out = rupu(tmp.path())
        .args(["findings", "list", "--ids-only", "tag:triaged"])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_a\nfnd_b\n");
}

#[test]
fn an_unknown_id_fails_but_known_ones_are_tagged() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "fnd_nope", "--add", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("fnd_nope"));
    let out = rupu(tmp.path())
        .args(["findings", "list", "--ids-only", "tag:x"])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
}

#[test]
fn the_partial_success_note_only_appears_when_something_changed() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    // First call changes fnd_a, so the note is true.
    let out = rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "fnd_nope", "--add", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("the other findings were changed"));
    // Repeating it changes nothing, so the note must not claim otherwise.
    let out = rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "fnd_nope", "--add", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("fnd_nope"), "{stderr}");
    assert!(!stderr.contains("were changed"), "{stderr}");
}

#[test]
fn one_call_tags_findings_in_two_workspaces() {
    let tmp = tempfile::tempdir().unwrap();
    let repo1 = seed_one(tmp.path());
    let repo2 = seed(
        tmp.path(),
        "ws2",
        "other",
        &[record("fnd_c", Severity::Medium)],
    );
    ok(&rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "fnd_c", "--add", "x"])
        .output()
        .unwrap());
    for repo in [repo1, repo2] {
        let log = rupu_coverage::TagLog::for_workspace(&repo);
        assert_eq!(rupu_coverage::read_tag_events(&log).unwrap().len(), 1);
    }
}

#[test]
fn remove_and_tags_in_use_as_json() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    ok(&rupu(tmp.path())
        .args([
            "findings", "tag", "fnd_a", "fnd_b", "--add", "x", "--add", "y",
        ])
        .output()
        .unwrap());
    ok(&rupu(tmp.path())
        .args(["findings", "tag", "fnd_b", "--remove", "y"])
        .output()
        .unwrap());
    let out = rupu(tmp.path())
        .args(["--format", "json", "findings", "tags"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&ok(&out)).unwrap();
    assert_eq!(
        v,
        serde_json::json!([{"tag": "x", "count": 2}, {"tag": "y", "count": 1}])
    );
    let events = rupu_coverage::read_tag_events(&rupu_coverage::TagLog::for_workspace(
        &tmp.path().join("repo").canonicalize().unwrap(),
    ))
    .unwrap();
    assert!(events.iter().all(|e| matches!(
        &e.by,
        rupu_coverage::TagActor::Operator(o) if o.via == rupu_coverage::OperatorSurface::Cli
    )));
}

#[test]
fn list_as_json_carries_tags_and_project_and_limit_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    ok(&rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "--add", "x"])
        .output()
        .unwrap());
    let out = rupu(tmp.path())
        .args(["--format", "json", "findings", "list"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&ok(&out)).unwrap();
    assert_eq!(v[0]["id"], "fnd_a");
    assert_eq!(v[0]["tags"], serde_json::json!(["x"]));
    assert_eq!(v[0]["project"], "repo");
    let out = rupu(tmp.path())
        .args(["findings", "list", "--limit", "1", "--ids-only"])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("showing 1 of 2"));
}

#[test]
fn an_invalid_tag_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "--add", "bad tag"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid tag"));
}

#[test]
fn list_takes_a_query_with_severity_comparison_and_negation() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    ok(&rupu(tmp.path())
        .args(["findings", "tag", "fnd_b", "--add", "noise"])
        .output()
        .unwrap());
    let out = rupu(tmp.path())
        .args(["findings", "list", "--ids-only", "severity>=low -tag:noise"])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
    let out = rupu(tmp.path())
        .args([
            "findings",
            "list",
            "--ids-only",
            "project:repo",
            "severity:high",
        ])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
}

#[test]
fn a_bad_query_is_a_clear_error() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "list", "sevrity:high"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown key `sevrity`"));
}

#[test]
fn a_flag_after_the_query_words_is_refused_not_misparsed() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "list", "tag:x", "--ids-only"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("put flags"), "{stderr}");
    assert!(
        stderr.contains("`--ids-only` looks like a flag"),
        "{stderr}"
    );
}
