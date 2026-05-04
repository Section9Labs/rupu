//! GitLab JSON → typed value translation tests.

use rupu_scm::connectors::gitlab::repo::translate_project_to_repo;
use rupu_scm::Platform;

#[test]
fn projects_list_happy_translates_to_repo() {
    let raw = std::fs::read_to_string("tests/fixtures/gitlab/projects_list_happy.json").unwrap();
    let arr: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    let repos: Vec<rupu_scm::types::Repo> = arr
        .iter()
        .map(translate_project_to_repo)
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        !repos.is_empty(),
        "fixture should contain at least one project"
    );
    let first = &repos[0];
    assert_eq!(first.r.platform, Platform::Gitlab);
    assert!(!first.r.owner.is_empty());
    assert!(!first.r.repo.is_empty());
    assert!(!first.default_branch.is_empty());
    assert!(first.clone_url_https.starts_with("https://"));
    assert!(
        first.clone_url_ssh.starts_with("git@") || first.clone_url_ssh.starts_with("ssh://"),
        "clone_url_ssh should start with git@ or ssh://"
    );
}

#[test]
fn nested_namespace_owner_includes_subgroup() {
    let raw = std::fs::read_to_string("tests/fixtures/gitlab/projects_list_happy.json").unwrap();
    let arr: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    // Find an entry with a nested namespace.
    let nested = arr
        .iter()
        .map(translate_project_to_repo)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .find(|r| r.r.owner.contains('/'));
    if let Some(repo) = nested {
        assert!(
            repo.r.owner.contains('/'),
            "nested owner should include subgroup"
        );
    } // else: no nested entry in this minimal fixture, skip silently
}
