//! Persistent autoflow worktree allocation helpers.

use crate::worktree_layout::issue_worktree_path;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoflowWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub reused: bool,
}

#[derive(Debug, Error)]
pub enum AutoflowWorktreeError {
    #[error("io {action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("git {action} failed ({status}): {stderr}")]
    Git {
        action: String,
        status: i32,
        stderr: String,
    },
    #[error("path `{path}` exists but is not a git worktree")]
    InvalidExistingPath { path: String },
}

pub fn ensure_issue_worktree(
    base_checkout: &Path,
    worktree_root: &Path,
    repo_ref: &str,
    issue_ref: &str,
    branch: &str,
    start_point: Option<&str>,
) -> Result<AutoflowWorktree, AutoflowWorktreeError> {
    let base_checkout = base_checkout
        .canonicalize()
        .map_err(|e| AutoflowWorktreeError::Io {
            action: format!("canonicalize {}", base_checkout.display()),
            source: e,
        })?;
    let worktree_path = issue_worktree_path(worktree_root, repo_ref, issue_ref);

    if worktree_path.exists() {
        if is_git_worktree(&worktree_path)? {
            return Ok(AutoflowWorktree {
                path: worktree_path,
                branch: branch.to_string(),
                reused: true,
            });
        }
        return Err(AutoflowWorktreeError::InvalidExistingPath {
            path: worktree_path.display().to_string(),
        });
    }

    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AutoflowWorktreeError::Io {
            action: format!("create_dir_all {}", parent.display()),
            source: e,
        })?;
    }

    if branch_exists(&base_checkout, branch)? {
        run_git(
            &base_checkout,
            ["worktree", "add"]
                .into_iter()
                .chain([worktree_path.to_string_lossy().as_ref(), branch]),
        )?;
    } else {
        let start = start_point.unwrap_or("HEAD");
        run_git(
            &base_checkout,
            ["worktree", "add", "-b"].into_iter().chain([
                branch,
                worktree_path.to_string_lossy().as_ref(),
                start,
            ]),
        )?;
    }

    Ok(AutoflowWorktree {
        path: worktree_path,
        branch: branch.to_string(),
        reused: false,
    })
}

pub fn remove_issue_worktree(
    base_checkout: &Path,
    worktree_path: &Path,
) -> Result<bool, AutoflowWorktreeError> {
    let base_checkout = base_checkout
        .canonicalize()
        .map_err(|e| AutoflowWorktreeError::Io {
            action: format!("canonicalize {}", base_checkout.display()),
            source: e,
        })?;
    if !worktree_path.exists() {
        return Ok(false);
    }
    run_git(
        &base_checkout,
        [
            "worktree",
            "remove",
            "--force",
            worktree_path.to_string_lossy().as_ref(),
        ],
    )?;
    Ok(true)
}

fn branch_exists(base_checkout: &Path, branch: &str) -> Result<bool, AutoflowWorktreeError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(base_checkout)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .map_err(|e| AutoflowWorktreeError::Io {
            action: format!("git show-ref in {}", base_checkout.display()),
            source: e,
        })?;
    Ok(out.status.success())
}

fn is_git_worktree(path: &Path) -> Result<bool, AutoflowWorktreeError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|e| AutoflowWorktreeError::Io {
            action: format!("git rev-parse in {}", path.display()),
            source: e,
        })?;
    Ok(out.status.success())
}

fn run_git<'a>(
    base_checkout: &Path,
    args: impl IntoIterator<Item = &'a str>,
) -> Result<(), AutoflowWorktreeError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(base_checkout)
        .args(args)
        .output()
        .map_err(|e| AutoflowWorktreeError::Io {
            action: format!("git in {}", base_checkout.display()),
            source: e,
        })?;
    if out.status.success() {
        return Ok(());
    }
    Err(AutoflowWorktreeError::Git {
        action: format!("git -C {} worktree add", base_checkout.display()),
        status: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_git_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        assert!(Command::new("git")
            .arg("init")
            .arg(path)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test User"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "commit.gpgsign", "false"])
            .status()
            .unwrap()
            .success());
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-m", "init"])
            .status()
            .unwrap()
            .success());
    }

    #[test]
    fn creates_and_reuses_issue_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let root = tmp.path().join("worktrees");
        init_git_repo(&repo);

        let created = ensure_issue_worktree(
            &repo,
            &root,
            "github:Section9Labs/rupu",
            "github:Section9Labs/rupu/issues/42",
            "rupu/issue-42",
            Some("HEAD"),
        )
        .unwrap();
        assert!(!created.reused);
        assert!(created.path.join("README.md").is_file());

        let branch = Command::new("git")
            .arg("-C")
            .arg(&created.path)
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .unwrap();
        assert!(branch.status.success());
        assert_eq!(
            String::from_utf8_lossy(&branch.stdout).trim(),
            "rupu/issue-42"
        );

        let reused = ensure_issue_worktree(
            &repo,
            &root,
            "github:Section9Labs/rupu",
            "github:Section9Labs/rupu/issues/42",
            "rupu/issue-42",
            Some("HEAD"),
        )
        .unwrap();
        assert!(reused.reused);
        assert_eq!(reused.path, created.path);
    }

    #[test]
    fn removes_issue_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let root = tmp.path().join("worktrees");
        init_git_repo(&repo);

        let created = ensure_issue_worktree(
            &repo,
            &root,
            "github:Section9Labs/rupu",
            "github:Section9Labs/rupu/issues/42",
            "rupu/issue-42",
            Some("HEAD"),
        )
        .unwrap();
        assert!(created.path.exists());

        let removed = remove_issue_worktree(&repo, &created.path).unwrap();
        assert!(removed);
        assert!(!created.path.exists());
    }
}
