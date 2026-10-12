//! `rupu skill`: install, list, show, validate and update against an
//! isolated RUPU_HOME and project, with local-directory sources (no
//! network).

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn skill_src(dir: &Path, name: &str, description: &str) -> PathBuf {
    let root = dir.join(name);
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"),
    )
    .unwrap();
    fs::write(root.join("scripts/run.sh"), "echo ok\n").unwrap();
    root
}

fn rupu(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home)
        .env_remove("RUPU_LOG")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
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
fn install_list_show_validate() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join(".rupu")).unwrap();
    let src = tempfile::tempdir().unwrap();
    let pdf = skill_src(src.path(), "pdf-processing", "PDF things.");
    let review = skill_src(src.path(), "code-review", "Review a diff.");

    let out = ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "install", pdf.to_str().unwrap()],
    ));
    assert!(out.contains("installed pdf-processing (global)"), "{out}");
    ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "install", "--project", review.to_str().unwrap()],
    ));
    assert!(project
        .path()
        .join(".rupu/skills/code-review/SKILL.md")
        .is_file());

    let again = rupu(
        home.path(),
        project.path(),
        &["skill", "install", pdf.to_str().unwrap()],
    );
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force"));

    let json: Value = serde_json::from_str(&ok(&rupu(
        home.path(),
        project.path(),
        &["--format", "json", "skill", "list"],
    )))
    .unwrap();
    let rows: Vec<(String, String, String)> = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().into(),
                r["layer"].as_str().unwrap().into(),
                r["status"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("code-review".into(), "project".into(), "ok".into()),
            ("pdf-processing".into(), "global".into(), "ok".into()),
        ]
    );
    assert_eq!(
        json["rows"][1]["source"].as_str(),
        Some(pdf.to_str().unwrap())
    );

    let show = ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "show", "pdf-processing"],
    ));
    assert!(show.contains("scripts/run.sh"), "{show}");
    assert!(show.contains("PDF things."), "{show}");
    assert!(show.contains("source:"), "{show}");

    ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "validate", "--all"],
    ));
}

#[test]
fn a_collection_needs_a_choice() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    skill_src(&repo.path().join("skills"), "pdf", "PDF.");
    skill_src(&repo.path().join("skills"), "docx", "Word.");
    let repo_arg = repo.path().to_str().unwrap();

    let listed = ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "install", "--list", repo_arg],
    ));
    assert!(
        listed.contains("pdf") && listed.contains("docx"),
        "{listed}"
    );
    assert!(!home.path().join("skills/pdf").exists());

    let none = rupu(home.path(), cwd.path(), &["skill", "install", repo_arg]);
    assert!(!none.status.success());
    assert!(String::from_utf8_lossy(&none.stderr).contains("--skill"));

    ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "install", "--skill", "pdf", repo_arg],
    ));
    assert!(home.path().join("skills/pdf/SKILL.md").is_file());
    assert!(!home.path().join("skills/docx").exists());

    ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "install", "--all", "--force", repo_arg],
    ));
    assert!(home.path().join("skills/docx/SKILL.md").is_file());
}

#[test]
fn update_follows_a_local_source() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let notes = skill_src(src.path(), "notes", "v1");
    ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "install", notes.to_str().unwrap()],
    ));

    let same = ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "update", "--all"],
    ));
    assert!(same.contains("notes: up to date"), "{same}");

    skill_src(src.path(), "notes", "v2");
    let moved = ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "update", "notes"],
    ));
    assert!(moved.contains("notes: updated"), "{moved}");
    assert!(
        fs::read_to_string(home.path().join("skills/notes/SKILL.md"))
            .unwrap()
            .contains("v2")
    );
}

#[test]
fn validate_fails_on_an_invalid_skill() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let bad = home.path().join("skills/bad-one");
    fs::create_dir_all(&bad).unwrap();
    fs::write(
        bad.join("SKILL.md"),
        "---\nname: other\ndescription: d\n---\n",
    )
    .unwrap();
    let out = rupu(home.path(), cwd.path(), &["skill", "validate", "--all"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("does not match its directory"));
}

#[test]
fn install_project_outside_a_project_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let s = skill_src(src.path(), "s", "d");
    let out = rupu(
        home.path(),
        cwd.path(),
        &["skill", "install", "--project", s.to_str().unwrap()],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a rupu project"));
}

#[test]
fn an_over_limit_description_is_a_warning_not_a_failure() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let long = skill_src(src.path(), "wordy", &"x".repeat(1068));
    ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "install", long.to_str().unwrap()],
    ));

    let out = rupu(home.path(), cwd.path(), &["skill", "validate", "--all"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("ok       wordy (global)"), "{stdout}");
    assert!(
        stdout.contains(
            "warn     wordy (global): `description` is 1068 characters; the agentskills.io limit is 1024"
        ),
        "{stdout}"
    );

    let show = ok(&rupu(home.path(), cwd.path(), &["skill", "show", "wordy"]));
    assert!(
        show.contains("  warning:       `description` is 1068 characters"),
        "{show}"
    );
}

#[test]
fn install_list_marks_invalid_skills() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    skill_src(&repo.path().join("skills"), "good", "Fine.");
    let bad = repo.path().join("skills/bad");
    fs::create_dir_all(&bad).unwrap();
    fs::write(bad.join("SKILL.md"), "---\nname: bad\n---\n").unwrap();
    let odd = repo.path().join("skills/odd");
    fs::create_dir_all(&odd).unwrap();
    fs::write(
        odd.join("SKILL.md"),
        "---\nname: Not_A_Name\ndescription: Looks fine.\n---\n",
    )
    .unwrap();

    let listed = ok(&rupu(
        home.path(),
        cwd.path(),
        &["skill", "install", "--list", repo.path().to_str().unwrap()],
    ));
    let line = |name: &str| {
        listed
            .lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_else(|| panic!("no {name} row: {listed}"))
            .to_owned()
    };
    assert!(
        line("bad ").contains("INVALID: frontmatter has no `description`"),
        "{listed}"
    );
    assert!(line("Not_A_Name ").contains("INVALID: name"), "{listed}");
    assert!(line("good ").ends_with("Fine."), "{listed}");
}

#[test]
fn naming_a_skill_twice_installs_it_once() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    skill_src(&repo.path().join("skills"), "pdf", "PDF.");
    skill_src(&repo.path().join("skills"), "docx", "Word.");
    let out = ok(&rupu(
        home.path(),
        cwd.path(),
        &[
            "skill",
            "install",
            "--skill",
            "pdf",
            "--skill",
            "pdf",
            repo.path().to_str().unwrap(),
        ],
    ));
    assert_eq!(out.matches("installed pdf").count(), 1, "{out}");
    assert!(home.path().join("skills/.sources/pdf.json").is_file());
}

#[test]
fn a_project_install_from_outside_the_project_is_not_tracked() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join(".rupu")).unwrap();
    let src = tempfile::tempdir().unwrap();
    let notes = skill_src(src.path(), "notes", "v1");
    let out = ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "install", "--project", notes.to_str().unwrap()],
    ));
    assert!(out.contains("installed notes (project)"), "{out}");
    assert!(
        out.contains(
            "note: notes is not tracked for update (its source is outside the project); \
             reinstall it to update"
        ),
        "{out}"
    );
    assert!(project.path().join(".rupu/skills/notes/SKILL.md").is_file());
    assert!(!project
        .path()
        .join(".rupu/skills/.sources/notes.json")
        .exists());

    let upd = ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "update", "--project", "notes"],
    ));
    assert!(upd.contains("notes: no source record"), "{upd}");

    let vendored = skill_src(&project.path().join("vendor"), "inside", "v1");
    let out = ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "install", "--project", vendored.to_str().unwrap()],
    ));
    assert!(!out.contains("note:"), "{out}");
    assert!(project
        .path()
        .join(".rupu/skills/.sources/inside.json")
        .is_file());
}

#[cfg(unix)]
#[test]
fn a_batch_that_fails_part_way_reports_what_landed() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    skill_src(&repo.path().join("skills"), "pdf", "PDF.");
    let broken = repo.path().join("skills/b-dir");
    fs::create_dir_all(&broken).unwrap();
    fs::write(
        broken.join("SKILL.md"),
        "---\nname: b-two\ndescription: d\n---\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("../b-dir/SKILL.md", broken.join("link")).unwrap();
    let out = rupu(
        home.path(),
        cwd.path(),
        &[
            "skill",
            "install",
            "--skill",
            "pdf",
            "--skill",
            "b-two",
            repo.path().to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stdout}");
    assert!(stdout.contains("installed pdf (global)"), "{stdout}");
    assert!(stderr.contains("b-two"), "{stderr}");
    assert!(home.path().join("skills/.sources/pdf.json").is_file());
    assert!(!home.path().join("skills/b-two").exists());
}
