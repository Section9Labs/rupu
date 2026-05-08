//! Helpers for persistent autoflow worktree layout.

use crate::repo_store::sanitize_component;
use std::path::{Path, PathBuf};

pub fn repo_dir_name(repo_ref: &str) -> String {
    sanitize_component(repo_ref)
}

pub fn issue_dir_name(issue_ref: &str) -> String {
    if let Some(number) = issue_ref.rsplit_once("/issues/").map(|(_, n)| n) {
        if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
            return format!("issue-{number}");
        }
    }
    sanitize_component(issue_ref)
}

pub fn issue_worktree_path(root: &Path, repo_ref: &str, issue_ref: &str) -> PathBuf {
    root.join(repo_dir_name(repo_ref))
        .join(issue_dir_name(issue_ref))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_worktree_path_uses_repo_and_issue_layout() {
        let p = issue_worktree_path(
            Path::new("/tmp/worktrees"),
            "github:Section9Labs/rupu",
            "github:Section9Labs/rupu/issues/42",
        );
        assert!(p.ends_with("github--Section9Labs--rupu/issue-42"));
    }
}
