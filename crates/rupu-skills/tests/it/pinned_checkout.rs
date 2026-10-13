#![cfg(feature = "git")]
use rupu_skills::pinned::PinnedCheckout;
use rupu_skills::{Fetch, GitSource};
use std::path::Path;

use crate::support::write_skill;

fn commit_all(repo: &git2::Repository) -> String {
    let mut idx = repo.index().unwrap();
    idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
    let parents: Vec<git2::Commit> = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .into_iter()
        .collect();
    let refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &refs)
        .unwrap()
        .to_string()
}

fn checkout_with_one_skill(dir: &Path) -> (git2::Repository, String) {
    let repo = git2::Repository::init(dir).unwrap();
    write_skill(
        &dir.join("skills"),
        "work-units",
        "work-units",
        "split results",
    );
    let c = commit_all(&repo);
    (repo, c)
}

fn src(commit: &str) -> GitSource {
    GitSource {
        url: "https://github.com/Section9Labs/rupu-skills".into(),
        git_ref: Some(commit.into()),
        path: None,
    }
}

fn checkout(dir: &Path) -> PinnedCheckout {
    PinnedCheckout { dir: dir.into() }
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

#[test]
fn a_checkout_whose_head_is_the_pin_serves_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    let tree = checkout(tmp.path()).fetch(&src(&c)).unwrap();
    assert_eq!(tree.commit.as_deref(), Some(c.as_str()));
    assert!(tree.root.join("skills/work-units/SKILL.md").is_file());
    assert!(tree.root.join("skills/work-units/scripts/run.sh").is_file());
    assert_ne!(tree.root, tmp.path());
}

#[test]
fn the_export_lives_as_long_as_the_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    let tree = checkout(tmp.path()).fetch(&src(&c)).unwrap();
    let root = tree.root.clone();
    assert!(root.join("skills").is_dir());
    drop(tree);
    assert!(!root.exists());
}

#[test]
fn a_later_head_still_serves_the_pinned_content() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, a) = checkout_with_one_skill(tmp.path());
    let before = read(tmp.path(), "skills/work-units/SKILL.md");
    write_skill(
        &tmp.path().join("skills"),
        "work-units",
        "work-units",
        "NEWER",
    );
    let b = commit_all(&repo);
    assert_ne!(a, b);
    let tree = checkout(tmp.path()).fetch(&src(&a)).unwrap();
    assert_eq!(read(&tree.root, "skills/work-units/SKILL.md"), before);
    assert!(!read(&tree.root, "skills/work-units/SKILL.md").contains("NEWER"));
    let tree_b = checkout(tmp.path()).fetch(&src(&b)).unwrap();
    assert!(read(&tree_b.root, "skills/work-units/SKILL.md").contains("NEWER"));
}

#[test]
fn a_pin_absent_from_the_object_database_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, _c) = checkout_with_one_skill(tmp.path());
    let other = "0123456789abcdef0123456789abcdef01234567";
    let err = checkout(tmp.path())
        .fetch(&src(other))
        .unwrap_err()
        .to_string();
    assert!(err.contains(other) && err.contains("git fetch"), "{err}");
}

#[test]
fn a_source_without_a_pin_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, _c) = checkout_with_one_skill(tmp.path());
    let mut s = src("x");
    s.git_ref = None;
    assert!(checkout(tmp.path()).fetch(&s).is_err());
}

#[test]
fn a_directory_that_is_not_a_checkout_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let err = checkout(tmp.path())
        .fetch(&src("0123456789abcdef0123456789abcdef01234567"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a git checkout"), "{err}");
}

#[test]
fn a_dirty_working_tree_does_not_leak_into_the_export() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = checkout_with_one_skill(tmp.path());
    // A second tracked script that will be hidden from status.
    std::fs::write(
        tmp.path().join("skills/work-units/scripts/hidden.sh"),
        "ok\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "__pycache__/\n").unwrap();
    let c = commit_all(&repo);
    let dir = tmp.path();

    // Modified, untracked, ignored, and skip-worktree / assume-unchanged edits.
    std::fs::write(dir.join("skills/work-units/scripts/run.sh"), "changed\n").unwrap();
    std::fs::write(dir.join("skills/work-units/extra.sh"), "x\n").unwrap();
    let cache = dir.join("skills/work-units/scripts/__pycache__");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("x.pyc"), "x").unwrap();
    git(
        dir,
        &[
            "update-index",
            "--skip-worktree",
            "skills/work-units/scripts/hidden.sh",
        ],
    );
    std::fs::write(dir.join("skills/work-units/scripts/hidden.sh"), "EVIL\n").unwrap();
    git(
        dir,
        &[
            "update-index",
            "--assume-unchanged",
            "skills/work-units/SKILL.md",
        ],
    );
    std::fs::write(dir.join("skills/work-units/SKILL.md"), "EVIL\n").unwrap();

    let tree = checkout(dir).fetch(&src(&c)).unwrap();
    let skill = tree.root.join("skills/work-units");
    assert_eq!(read(&skill, "scripts/run.sh"), "#!/bin/sh\necho ok\n");
    assert_eq!(read(&skill, "scripts/hidden.sh"), "ok\n");
    assert!(read(&skill, "SKILL.md").starts_with("---\nname: work-units"));
    assert!(!skill.join("extra.sh").exists());
    assert!(!skill.join("scripts/__pycache__").exists());

    // The user's checkout is untouched.
    assert_eq!(read(dir, "skills/work-units/scripts/run.sh"), "changed\n");
    assert_eq!(read(dir, "skills/work-units/scripts/hidden.sh"), "EVIL\n");
    assert_eq!(read(dir, "skills/work-units/SKILL.md"), "EVIL\n");
    assert!(dir.join("skills/work-units/extra.sh").is_file());
    assert!(cache.join("x.pyc").is_file());
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        c
    );
}

#[test]
fn the_users_head_and_index_are_untouched_when_the_pin_is_older() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, a) = checkout_with_one_skill(tmp.path());
    write_skill(
        &tmp.path().join("skills"),
        "work-units",
        "work-units",
        "NEWER",
    );
    let b = commit_all(&repo);
    let _ = checkout(tmp.path()).fetch(&src(&a)).unwrap();
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        b
    );
    assert!(read(tmp.path(), "skills/work-units/SKILL.md").contains("NEWER"));
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(tmp.path())
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
}

#[test]
fn a_changed_file_outside_skills_is_irrelevant() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join("NOTES.md"), "scratch\n").unwrap();
    let tree = checkout(tmp.path()).fetch(&src(&c)).unwrap();
    assert!(!tree.root.join("NOTES.md").exists());
}

#[cfg(unix)]
#[test]
fn an_outward_symlink_in_the_pinned_tree_fails_the_pin_and_writes_nothing() {
    use rupu_skills::pinned::{install_pins, Pin, PinOutcome};
    use rupu_skills::Layer;

    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join("outside.txt"), "not a skill file\n").unwrap();
    std::os::unix::fs::symlink(
        "../../../outside.txt",
        tmp.path().join("skills/work-units/scripts/escape"),
    )
    .unwrap();
    let c = commit_all(&repo);

    let dest = tempfile::tempdir().unwrap();
    let pin = Pin {
        name: "work-units".into(),
        source: "Section9Labs/rupu-skills".into(),
        tag: "v1.0.0".into(),
        commit: c,
    };
    let report = install_pins(
        &[pin],
        dest.path(),
        Layer::Global,
        false,
        &checkout(tmp.path()),
    );
    match &report.results[0].1 {
        PinOutcome::Failed(msg) => assert!(msg.contains("symlink"), "{msg}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    let left: Vec<_> = std::fs::read_dir(dest.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "skills dir should be untouched: {left:?}");
}
