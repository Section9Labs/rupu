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

#[test]
fn a_clean_checkout_at_the_pin_is_served() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    let tree = checkout(tmp.path()).fetch(&src(&c)).unwrap();
    assert_eq!(tree.commit.as_deref(), Some(c.as_str()));
    assert!(tree.root.join("skills/work-units/SKILL.md").is_file());
}

#[test]
fn a_checkout_at_another_commit_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    let other = "0123456789abcdef0123456789abcdef01234567";
    let err = checkout(tmp.path())
        .fetch(&src(other))
        .unwrap_err()
        .to_string();
    assert!(err.contains(&c) && err.contains(other), "{err}");
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
fn a_modified_file_under_skills_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    std::fs::write(
        tmp.path().join("skills/work-units/scripts/run.sh"),
        "changed\n",
    )
    .unwrap();
    let err = checkout(tmp.path())
        .fetch(&src(&c))
        .unwrap_err()
        .to_string();
    assert!(err.contains("skills/work-units/scripts/run.sh"), "{err}");
}

#[test]
fn a_staged_change_under_skills_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, c) = checkout_with_one_skill(tmp.path());
    std::fs::write(
        tmp.path().join("skills/work-units/scripts/run.sh"),
        "staged\n",
    )
    .unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("skills/work-units/scripts/run.sh"))
        .unwrap();
    idx.write().unwrap();
    let err = checkout(tmp.path())
        .fetch(&src(&c))
        .unwrap_err()
        .to_string();
    assert!(err.contains("skills/work-units/scripts/run.sh"), "{err}");
}

#[test]
fn an_untracked_file_under_skills_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join("skills/work-units/extra.sh"), "x\n").unwrap();
    let err = checkout(tmp.path())
        .fetch(&src(&c))
        .unwrap_err()
        .to_string();
    assert!(err.contains("skills/work-units/extra.sh"), "{err}");
}

#[test]
fn an_untracked_or_ignored_file_under_skills_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join(".gitignore"), "__pycache__/\n").unwrap();
    let c = commit_all(&repo);
    let cache = tmp.path().join("skills/work-units/scripts/__pycache__");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("x.pyc"), "x").unwrap();
    let err = checkout(tmp.path())
        .fetch(&src(&c))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("__pycache__") && err.contains("git clean -fdx skills/"),
        "{err}"
    );
}

#[test]
fn a_changed_file_outside_skills_is_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let (_r, c) = checkout_with_one_skill(tmp.path());
    std::fs::write(tmp.path().join("NOTES.md"), "scratch\n").unwrap();
    checkout(tmp.path()).fetch(&src(&c)).unwrap();
}
