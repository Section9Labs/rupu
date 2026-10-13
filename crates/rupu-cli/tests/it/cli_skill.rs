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

fn project_with_skill(skill_md: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let dir = project.path().join(".rupu/skills/evil");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), skill_md).unwrap();
    (home, project)
}

#[test]
fn project_text_never_reaches_the_terminal_raw() {
    let (home, project) = project_with_skill(
        "---\nname: evil\ndescription: \"Hi \\e]0;pwned\\a there\"\n\
         license: \"MIT\\e[2J\"\nmetadata:\n  \"k\\e[31m\": \"v\\a\"\n---\n\nBody.\n",
    );
    fs::write(
        project.path().join(".rupu/skills/evil/run\x1b[1m.sh"),
        "echo\n",
    )
    .unwrap();
    for args in [
        &["skill", "list"][..],
        &["skill", "show", "evil"][..],
        &["--format", "csv", "skill", "list"][..],
    ] {
        let out = ok(&rupu(home.path(), project.path(), args));
        assert!(out.contains("pwned"), "{args:?}: {out}");
        assert!(
            !out.bytes().any(|b| b == 0x1b || b == 0x07),
            "{args:?}: {out:?}"
        );
    }
}

#[test]
fn show_survives_a_malformed_source_record() {
    let (home, project) = project_with_skill("---\nname: evil\ndescription: Fine.\n---\n\nBody.\n");
    let sources = project.path().join(".rupu/skills/.sources");
    fs::create_dir_all(&sources).unwrap();
    fs::write(sources.join("evil.json"), "{not json").unwrap();
    let out = ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "show", "evil"],
    ));
    assert!(out.contains("  source:        unreadable ("), "{out}");
    assert!(out.contains("  description:   Fine."), "{out}");
}

#[test]
fn a_qualified_show_still_notes_the_other_layer() {
    let (home, project) =
        project_with_skill("---\nname: evil\ndescription: Project copy.\n---\n\nBody.\n");
    let global = home.path().join("skills/evil");
    fs::create_dir_all(&global).unwrap();
    fs::write(
        global.join("SKILL.md"),
        "---\nname: evil\ndescription: Global copy.\n---\n\nBody.\n",
    )
    .unwrap();
    let out = ok(&rupu(
        home.path(),
        project.path(),
        &["skill", "show", "global:evil"],
    ));
    assert!(
        out.starts_with(
            "evil is installed in both layers; grant it as global:evil or project:evil.\n"
        ),
        "{out}"
    );
    assert!(out.contains("Global copy."), "{out}");
    assert!(!out.contains("Project copy."), "{out}");
}

#[test]
fn run_names_where_a_missing_skill_was_looked_for() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("agents/a.md"),
        "---\nname: a\nprovider: anthropic\nmodel: claude-sonnet-4-6\nskills: [nope]\n---\nhi\n",
    )
    .unwrap();
    let out = rupu(home.path(), cwd.path(), &["run", "a", "go"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "agent `a`: skills: unknown skill \"nope\" (looked in {}; install it with `rupu skill install`)",
            home.path().join("skills").display()
        )),
        "{stderr}"
    );
}

#[test]
fn run_skill_flag_names_where_a_missing_skill_was_looked_for() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("agents/a.md"),
        "---\nname: a\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nhi\n",
    )
    .unwrap();
    let out = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .env(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{"AssistantText":{"text":"ran","stop":"end_turn"}}]"#,
        )
        .env_remove("RUPU_LOG")
        .current_dir(cwd.path())
        .args([
            "run",
            "a",
            "--mode",
            "readonly",
            "--run-id",
            "run_skillflag",
            "--skill",
            "nope",
            "go",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let message = format!(
        "skills: unknown skill \"nope\" (looked in {}; install it with `rupu skill install`)",
        home.path().join("skills").display()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&format!("[error] {message}")), "{stderr}");
    let record: Value = serde_json::from_str(
        &fs::read_to_string(home.path().join("runs/run_skillflag/run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["status"], "failed");
    assert_eq!(record["error_message"], message.as_str());
}

#[test]
fn validate_names_an_oversized_skill_md() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let dir = home.path().join("skills/huge");
    fs::create_dir_all(&dir).unwrap();
    let text = format!(
        "---\nname: huge\ndescription: d\n---\n{}\n",
        "x".repeat(1 << 20)
    );
    fs::write(dir.join("SKILL.md"), &text).unwrap();
    let out = rupu(home.path(), cwd.path(), &["skill", "validate", "--all"]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!(
            "SKILL.md is {} bytes; the limit is 1048576 bytes",
            text.len()
        )),
        "{stdout}"
    );
}

fn home_with_agent_and_skills(skills: &[(&str, &str)]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("agents")).unwrap();
    fs::write(
        home.path().join("agents/a.md"),
        "---\nname: a\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nhi\n",
    )
    .unwrap();
    for (name, body) in skills {
        let dir = home.path().join("skills").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\n{body}\n"),
        )
        .unwrap();
    }
    home
}

fn mock_run(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home)
        .env(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{"AssistantText":{"text":"ran","stop":"end_turn"}}]"#,
        )
        .env_remove("RUPU_LOG")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn transcript_events(home: &Path, run_id: &str) -> Vec<Value> {
    fs::read_to_string(home.join("transcripts").join(format!("{run_id}.jsonl")))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// `(name, layer, preload, reason)` per `skill_index` row, and the system
/// prompt the run sent.
fn skill_index_and_prompt(events: &[Value]) -> (Vec<(String, String, bool, String)>, String) {
    let rows = events
        .iter()
        .find(|e| e["type"] == "skill_index")
        .map(|e| {
            e["data"]["skills"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    (
                        r["name"].as_str().unwrap().to_owned(),
                        r["layer"].as_str().unwrap().to_owned(),
                        r["preload"].as_bool().unwrap(),
                        r["reason"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let prompt = events
        .iter()
        .find(|e| e["type"] == "run_start")
        .and_then(|e| e["data"]["system_prompt"].as_str())
        .unwrap_or_default()
        .to_owned();
    (rows, prompt)
}

fn row(name: &str, preload: bool) -> (String, String, bool, String) {
    (name.into(), "global".into(), preload, "launch".into())
}

#[test]
fn run_skill_flags_give_the_run_launch_skills() {
    let home = home_with_agent_and_skills(&[("x", "X-PRELOADED-BODY"), ("y", "Y-BODY")]);
    let cwd = tempfile::tempdir().unwrap();
    let out = mock_run(
        home.path(),
        cwd.path(),
        &[
            "run",
            "a",
            "--mode",
            "readonly",
            "--run-id",
            "run_flags",
            "--preload-skill",
            "x",
            "--skill",
            "y",
            "go",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (rows, prompt) = skill_index_and_prompt(&transcript_events(home.path(), "run_flags"));
    assert_eq!(rows, vec![row("x", true), row("y", false)]);
    assert!(prompt.contains("## Skill: x"), "{prompt}");
    assert!(prompt.contains("X-PRELOADED-BODY"), "{prompt}");
    assert!(prompt.contains("- y: The y skill."), "{prompt}");
    assert!(!prompt.contains("Y-BODY"), "{prompt}");
}

/// Cut a transcript back to before its first turn: what a runner killed
/// while waiting on the model leaves behind.
fn interrupt(home: &Path, run_id: &str) {
    let path = home.join("transcripts").join(format!("{run_id}.jsonl"));
    let kept: String = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .take_while(|l| serde_json::from_str::<Value>(l).unwrap()["type"] != "turn_start")
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&path, kept).unwrap();
}

#[test]
fn a_continued_run_keeps_its_launch_skills_unless_the_flags_name_others() {
    let home = home_with_agent_and_skills(&[("x", "X-PRELOADED-BODY"), ("y", "Y-BODY")]);
    let cwd = tempfile::tempdir().unwrap();
    let first = mock_run(
        home.path(),
        cwd.path(),
        &[
            "run",
            "a",
            "--mode",
            "readonly",
            "--run-id",
            "run_first",
            "--preload-skill",
            "x",
            "go",
        ],
    );
    assert!(first.status.success());
    interrupt(home.path(), "run_first");

    let kept = mock_run(
        home.path(),
        cwd.path(),
        &[
            "run",
            "a",
            "--mode",
            "readonly",
            "--run-id",
            "run_kept",
            "--continue",
            "run_first",
        ],
    );
    assert!(
        kept.status.success(),
        "{}",
        String::from_utf8_lossy(&kept.stderr)
    );
    let (rows, prompt) = skill_index_and_prompt(&transcript_events(home.path(), "run_kept"));
    assert_eq!(rows, vec![row("x", true)]);
    assert!(prompt.contains("X-PRELOADED-BODY"), "{prompt}");

    interrupt(home.path(), "run_kept");
    let replaced = mock_run(
        home.path(),
        cwd.path(),
        &[
            "run",
            "a",
            "--mode",
            "readonly",
            "--run-id",
            "run_replaced",
            "--continue",
            "run_kept",
            "--skill",
            "y",
        ],
    );
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    let (rows, prompt) = skill_index_and_prompt(&transcript_events(home.path(), "run_replaced"));
    assert_eq!(rows, vec![row("y", false)]);
    assert!(!prompt.contains("X-PRELOADED-BODY"), "{prompt}");
}

fn add_skill(home: &Path, name: &str, body: &str) {
    let dir = home.join("skills").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\n{body}\n"),
    )
    .unwrap();
}

fn run_ok(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_continued_run_keeps_a_launch_preload_of_an_agent_skill() {
    let home = home_with_agent_and_skills(&[("x", "X-PRELOADED-BODY")]);
    fs::write(
        home.path().join("agents/b.md"),
        "---\nname: b\nprovider: anthropic\nmodel: claude-sonnet-4-6\nskills: [x]\n---\nhi\n",
    )
    .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let base = ["run", "b", "--mode", "readonly", "--run-id"];
    run_ok(&mock_run(
        home.path(),
        cwd.path(),
        &[&base[..], &["run_overlap", "--preload-skill", "x", "go"]].concat(),
    ));
    let (rows, prompt) = skill_index_and_prompt(&transcript_events(home.path(), "run_overlap"));
    assert_eq!(
        rows,
        vec![("x".into(), "global".into(), true, "agent".into())]
    );
    assert!(prompt.contains("X-PRELOADED-BODY"), "{prompt}");
    interrupt(home.path(), "run_overlap");

    run_ok(&mock_run(
        home.path(),
        cwd.path(),
        &[&base[..], &["run_overlap_2", "--continue", "run_overlap"]].concat(),
    ));
    let (rows, prompt) = skill_index_and_prompt(&transcript_events(home.path(), "run_overlap_2"));
    assert_eq!(
        rows,
        vec![("x".into(), "global".into(), true, "agent".into())]
    );
    assert!(prompt.contains("X-PRELOADED-BODY"), "{prompt}");
}

#[test]
fn a_continued_star_launch_grants_a_skill_installed_since() {
    let home = home_with_agent_and_skills(&[("x", "X-BODY")]);
    let cwd = tempfile::tempdir().unwrap();
    let base = ["run", "a", "--mode", "readonly", "--run-id"];
    run_ok(&mock_run(
        home.path(),
        cwd.path(),
        &[&base[..], &["run_star", "--skill", "*", "go"]].concat(),
    ));
    let events = transcript_events(home.path(), "run_star");
    let start = events.iter().find(|e| e["type"] == "run_start").unwrap();
    assert_eq!(start["data"]["launch_skills"], serde_json::json!(["*"]));
    assert_eq!(skill_index_and_prompt(&events).0, vec![row("x", false)]);
    interrupt(home.path(), "run_star");

    add_skill(home.path(), "y", "Y-BODY");
    run_ok(&mock_run(
        home.path(),
        cwd.path(),
        &[&base[..], &["run_star_2", "--continue", "run_star"]].concat(),
    ));
    let events = transcript_events(home.path(), "run_star_2");
    let start = events.iter().find(|e| e["type"] == "run_start").unwrap();
    assert_eq!(start["data"]["launch_skills"], serde_json::json!(["*"]));
    assert_eq!(
        skill_index_and_prompt(&events).0,
        vec![row("x", false), row("y", false)]
    );
}

#[test]
fn a_continued_launch_skill_that_no_longer_resolves_names_where_it_came_from() {
    let home = home_with_agent_and_skills(&[("x", "X-BODY")]);
    let cwd = tempfile::tempdir().unwrap();
    let base = ["run", "a", "--mode", "readonly", "--run-id"];
    run_ok(&mock_run(
        home.path(),
        cwd.path(),
        &[&base[..], &["run_gone", "--skill", "x", "go"]].concat(),
    ));
    interrupt(home.path(), "run_gone");
    fs::remove_dir_all(home.path().join("skills/x")).unwrap();

    let out = mock_run(
        home.path(),
        cwd.path(),
        &[&base[..], &["run_gone_2", "--continue", "run_gone"]].concat(),
    );
    assert!(!out.status.success());
    let message = format!(
        "skills: unknown skill \"x\" (looked in {}; install it with `rupu skill install`)\n  \
         The launch skills (--skill x) came from run run_gone, which this run continues; \
         pass --skill or --preload-skill to name others.",
        home.path().join("skills").display()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&message), "{stderr}");
    let record: Value = serde_json::from_str(
        &fs::read_to_string(home.path().join("runs/run_gone_2/run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["error_message"], message.as_str());
}

#[test]
fn a_bad_skill_flag_fails_before_the_target_is_cloned() {
    let home = home_with_agent_and_skills(&[]);
    let cwd = tempfile::tempdir().unwrap();
    let out = mock_run(
        home.path(),
        cwd.path(),
        &[
            "run",
            "a",
            "github:octo/r",
            "--mode",
            "readonly",
            "--run-id",
            "run_badclone",
            "--skill",
            "typo",
            "go",
        ],
    );
    assert!(!out.status.success());
    let message = format!(
        "skills: unknown skill \"typo\" (looked in {}; install it with `rupu skill install`)",
        home.path().join("skills").display()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&format!("[error] {message}")), "{stderr}");
    assert!(!cwd.path().join("r").exists());
    let record: Value = serde_json::from_str(
        &fs::read_to_string(home.path().join("runs/run_badclone/run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["status"], "failed");
    assert_eq!(record["error_message"], message.as_str());
    assert!(home.path().join("transcripts/run_badclone.jsonl").is_file());
}
