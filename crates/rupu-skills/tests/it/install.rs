use crate::support::write_skill;
use rupu_skills::install::{
    choose, find_skills, install_one, install_selected, InstallError, InstallOutcome, Selection,
    Tracking,
};
use rupu_skills::{load_skill, Layer};
use std::fs;
use std::path::Path;

/// A repo shaped like the published skill collections: `skills/<name>/`,
/// a template at the root, skills under `.claude/skills/`, and noise.
fn collection(root: &Path) {
    write_skill(&root.join("skills"), "pdf", "pdf", "PDF things.");
    write_skill(&root.join("skills"), "docx", "docx", "Word things.");
    write_skill(
        &root.join("skills/pdf"),
        "nested",
        "nested",
        "never found: inside a skill",
    );
    write_skill(root, "template", "template-skill", "A starting point.");
    write_skill(
        &root.join(".claude/skills"),
        "hidden-one",
        "hidden-one",
        "From a plugin folder.",
    );
    write_skill(
        &root.join("node_modules"),
        "dep",
        "dep",
        "never found: node_modules",
    );
    write_skill(&root.join(".git"), "obj", "obj", "never found: .git");
    fs::write(root.join("README.md"), "# skills\n").unwrap();
}

#[test]
fn find_skills_searches_like_the_ecosystem() {
    let t = tempfile::tempdir().unwrap();
    collection(t.path());
    let found = find_skills(t.path()).unwrap();
    let got: Vec<(&str, &str)> = found
        .iter()
        .map(|f| (f.name.as_str(), f.rel.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("hidden-one", ".claude/skills/hidden-one"),
            ("docx", "skills/docx"),
            ("pdf", "skills/pdf"),
            ("template-skill", "template"),
        ]
    );
    assert_eq!(found[2].description.as_deref(), Some("PDF things."));
}

#[test]
fn a_root_skill_is_found_by_its_frontmatter_name() {
    let t = tempfile::tempdir().unwrap();
    let root = write_skill(t.path(), "repo-checkout", "report-style", "d");
    let found = find_skills(&root).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].name.as_str(), found[0].rel.as_str()),
        ("report-style", "")
    );
}

#[test]
fn search_depth_is_bounded() {
    let t = tempfile::tempdir().unwrap();
    write_skill(
        &t.path().join("a/b/c/d/e/f"),
        "deep",
        "deep",
        "seven levels down",
    );
    write_skill(&t.path().join("a/b/c/d/e"), "ok", "ok", "six levels down");
    let names: Vec<String> = find_skills(t.path())
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(names, vec!["ok"]);
}

#[test]
fn duplicate_names_in_one_source_are_an_error() {
    let t = tempfile::tempdir().unwrap();
    write_skill(&t.path().join("a"), "same", "same", "d");
    write_skill(&t.path().join("b"), "same", "same", "d");
    assert!(matches!(
        find_skills(t.path()),
        Err(InstallError::DuplicateName { name, paths }) if name == "same" && paths == ["a/same", "b/same"]
    ));
}

#[test]
fn choose_by_selection() {
    let t = tempfile::tempdir().unwrap();
    collection(t.path());
    let found = find_skills(t.path()).unwrap();
    assert!(
        matches!(choose(&found, &Selection::Default), Err(InstallError::Several(v)) if v.len() == 4)
    );
    let one = choose(&found, &Selection::Named(vec!["pdf".into()])).unwrap();
    assert_eq!(one[0].rel, "skills/pdf");
    assert_eq!(choose(&found, &Selection::All).unwrap().len(), 4);
    assert!(matches!(
        choose(&found, &Selection::Named(vec!["xlsx".into()])),
        Err(InstallError::NameNotFound { name, .. }) if name == "xlsx"
    ));
    let single = find_skills(&t.path().join("skills/pdf")).unwrap();
    assert_eq!(choose(&single, &Selection::Default).unwrap().len(), 1);
}

#[test]
fn install_one_names_the_skill_by_its_frontmatter() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let root = write_skill(src.path(), "repo-checkout", "report-style", "d");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".git/HEAD"), "x").unwrap();
    let got = install_one(&root, dst.path(), Layer::Global, false).unwrap();
    assert_eq!(got.outcome, InstallOutcome::Created);
    assert_eq!(got.path, dst.path().join("report-style"));
    let installed = load_skill(&got.path, Layer::Global).unwrap();
    assert_eq!(installed.files(), vec!["SKILL.md", "scripts/run.sh"]);
    assert_eq!(installed.content_hash().unwrap(), got.content_hash);
    let leftovers: Vec<_> = fs::read_dir(dst.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        vec![std::ffi::OsString::from("report-style")],
        "no staging dirs left"
    );
}

#[cfg(unix)]
#[test]
fn keeps_the_exec_bit() {
    use std::os::unix::fs::PermissionsExt;
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "d");
    fs::set_permissions(
        skill.join("scripts/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let got = install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    let mode = fs::metadata(got.path.join("scripts/run.sh"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755);
}

#[test]
fn refuses_existing_without_force_and_replaces_with_it() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "first");
    install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    write_skill(src.path(), "s", "s", "second");
    assert!(matches!(
        install_one(&skill, dst.path(), Layer::Global, false),
        Err(InstallError::Exists(_))
    ));
    assert_eq!(
        install_one(&skill, dst.path(), Layer::Global, true)
            .unwrap()
            .outcome,
        InstallOutcome::Replaced
    );
    assert_eq!(
        load_skill(&dst.path().join("s"), Layer::Global)
            .unwrap()
            .frontmatter
            .description,
        "second"
    );
    let leftovers: Vec<_> = fs::read_dir(dst.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        vec![std::ffi::OsString::from("s")],
        "no staging or backup dirs left"
    );
}

#[test]
fn a_broken_replacement_leaves_the_old_skill_intact() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "good");
    install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    fs::write(skill.join("SKILL.md"), "---\nname: s\n---\n").unwrap();
    assert!(matches!(
        install_one(&skill, dst.path(), Layer::Global, true),
        Err(InstallError::Invalid { .. })
    ));
    assert_eq!(
        load_skill(&dst.path().join("s"), Layer::Global)
            .unwrap()
            .frontmatter
            .description,
        "good"
    );
}

#[cfg(unix)]
#[test]
fn absolute_internal_symlinks_are_refused_relative_ones_copied() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "d");
    std::os::unix::fs::symlink("scripts/run.sh", skill.join("run")).unwrap();
    let got = install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    assert_eq!(
        fs::read_link(got.path.join("run")).unwrap(),
        Path::new("scripts/run.sh")
    );

    let skill2 = write_skill(src.path(), "t", "t", "d");
    std::os::unix::fs::symlink(
        skill2.join("scripts/run.sh").canonicalize().unwrap(),
        skill2.join("run"),
    )
    .unwrap();
    assert!(matches!(
        install_one(&skill2, dst.path(), Layer::Global, false),
        Err(InstallError::AbsoluteSymlink { skill, rel }) if skill == "t" && rel == "run"
    ));
}

#[test]
fn install_all_skips_invalid_named_invalid_fails_before_writing() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    write_skill(src.path(), "good", "good", "d");
    let bad = write_skill(src.path(), "bad", "bad", "d");
    fs::write(bad.join("SKILL.md"), "---\nname: bad\n---\n").unwrap();
    let found = find_skills(src.path()).unwrap();

    let err = install_selected(
        &found,
        &Selection::Named(vec!["good".into(), "bad".into()]),
        dst.path(),
        Layer::Global,
        false,
        &mut |_| Ok(Tracking::Recorded),
    );
    assert!(matches!(err, Err(InstallError::Invalid { name, .. }) if name == "bad"));
    assert!(
        !dst.path().join("good").exists(),
        "nothing is written when a named skill is invalid"
    );

    let report = install_selected(
        &found,
        &Selection::All,
        dst.path(),
        Layer::Global,
        false,
        &mut |_| Ok(Tracking::Recorded),
    )
    .unwrap();
    assert_eq!(
        report
            .installed
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        vec!["good"]
    );
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].0, "bad");
}

#[test]
fn install_selected_checks_every_destination_before_writing() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    write_skill(src.path(), "a-one", "a-one", "d");
    write_skill(src.path(), "b-two", "b-two", "d");
    write_skill(dst.path(), "b-two", "b-two", "already here");
    let found = find_skills(src.path()).unwrap();
    assert!(matches!(
        install_selected(
            &found,
            &Selection::All,
            dst.path(),
            Layer::Global,
            false,
            &mut |_| Ok(Tracking::Recorded)
        ),
        Err(InstallError::Exists(_))
    ));
    assert!(!dst.path().join("a-one").exists());
}

#[cfg(unix)]
#[test]
fn the_search_does_not_follow_symlinks_out_of_the_source() {
    let outside = tempfile::tempdir().unwrap();
    write_skill(outside.path(), "far", "far", "never found: behind a link");
    fs::write(
        outside.path().join("secret.md"),
        "---\nname: leak\ndescription: outside text\n---\n",
    )
    .unwrap();
    let src = tempfile::tempdir().unwrap();
    write_skill(src.path(), "near", "near", "d");
    std::os::unix::fs::symlink(outside.path(), src.path().join("linked")).unwrap();
    fs::create_dir_all(src.path().join("leak")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.md"),
        src.path().join("leak/SKILL.md"),
    )
    .unwrap();

    let found = find_skills(src.path()).unwrap();
    let got: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|f| (f.name.as_str(), f.description.as_deref()))
        .collect();
    assert_eq!(got, vec![("leak", None), ("near", Some("d"))]);
    assert_eq!(
        found[0].error.as_deref(),
        Some("SKILL.md is a symlink that leaves the source")
    );
}

fn leftovers(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A skill whose directory is not its name and whose symlink climbs back
/// through that directory: it validates in place, and breaks once copied
/// under its real name.
#[cfg(unix)]
pub fn breaks_when_staged(parent: &Path, dir: &str, name: &str) -> std::path::PathBuf {
    let root = write_skill(parent, dir, name, "breaks when staged");
    std::os::unix::fs::symlink(format!("../{dir}/SKILL.md"), root.join("link")).unwrap();
    root
}

#[cfg(unix)]
#[test]
fn a_replacement_that_fails_staged_validation_leaves_the_old_skill() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let good = write_skill(src.path(), "s", "s", "good");
    install_one(&good, dst.path(), Layer::Global, false).unwrap();
    let broken = breaks_when_staged(src.path(), "checkout", "s");
    assert!(load_skill(&broken, Layer::Global).is_err_and(|e| e.to_string().contains("match")));
    let got = install_one(&broken, dst.path(), Layer::Global, true);
    assert!(
        matches!(&got, Err(InstallError::Invalid { name, error }) if name == "s" && error.contains("link")),
        "{got:?}"
    );
    assert_eq!(
        load_skill(&dst.path().join("s"), Layer::Global)
            .unwrap()
            .frontmatter
            .description,
        "good"
    );
    assert_eq!(leftovers(dst.path()), vec!["s"]);
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_at_the_destination_counts_as_installed() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "d");
    std::os::unix::fs::symlink(dst.path().join("gone"), dst.path().join("s")).unwrap();
    assert!(matches!(
        install_one(&skill, dst.path(), Layer::Global, false),
        Err(InstallError::Exists(_))
    ));
    assert_eq!(leftovers(dst.path()), vec!["s"], "no staging dir left");
    let found = find_skills(src.path()).unwrap();
    assert!(matches!(
        install_selected(
            &found,
            &Selection::All,
            dst.path(),
            Layer::Global,
            false,
            &mut |_| Ok(Tracking::Recorded)
        ),
        Err(InstallError::Exists(_))
    ));

    let got = install_one(&skill, dst.path(), Layer::Global, true).unwrap();
    assert_eq!(got.outcome, InstallOutcome::Replaced);
    assert!(load_skill(&dst.path().join("s"), Layer::Global).is_ok());
    assert_eq!(leftovers(dst.path()), vec!["s"]);
}

#[test]
fn an_interrupted_replace_is_put_back_before_the_next_install() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let skill = write_skill(src.path(), "s", "s", "v1");
    install_one(&skill, dst.path(), Layer::Global, false).unwrap();
    write_skill(dst.path(), "s-tools", "s-tools", "another skill");
    fs::rename(
        dst.path().join("s-tools"),
        dst.path().join(".replaced-s-tools-7"),
    )
    .unwrap();
    fs::rename(dst.path().join("s"), dst.path().join(".replaced-s-4242")).unwrap();

    write_skill(src.path(), "s", "s", "v2");
    assert!(matches!(
        install_one(&skill, dst.path(), Layer::Global, false),
        Err(InstallError::Exists(_))
    ));
    assert_eq!(
        load_skill(&dst.path().join("s"), Layer::Global)
            .unwrap()
            .frontmatter
            .description,
        "v1"
    );
    assert_eq!(leftovers(dst.path()), vec![".replaced-s-tools-7", "s"]);

    fs::create_dir_all(dst.path().join(".replaced-s-9")).unwrap();
    install_one(&skill, dst.path(), Layer::Global, true).unwrap();
    assert_eq!(
        leftovers(dst.path()),
        vec![".replaced-s-tools-7", "s"],
        "a backup next to an installed skill is stale"
    );
}

#[test]
fn naming_a_skill_twice_chooses_it_once() {
    let t = tempfile::tempdir().unwrap();
    collection(t.path());
    let found = find_skills(t.path()).unwrap();
    let got: Vec<&str> = choose(
        &found,
        &Selection::Named(vec!["pdf".into(), "docx".into(), "pdf".into()]),
    )
    .unwrap()
    .iter()
    .map(|f| f.name.as_str())
    .collect();
    assert_eq!(got, vec!["pdf", "docx"]);
}
