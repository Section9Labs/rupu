//! The files rupu writes into a rupu root: the `rupu init` skeleton and its
//! samples ([`init`]), and the stock security-assessment fleet ([`fleet`]).
//! Both are embedded at build time ([`MANIFEST`], [`FLEET_MANIFEST`]); the
//! CLI (`rupu init`, `rupu fleet`) and the control plane (`/api/projects/init`,
//! `/api/fleet`) install them through the same functions.

pub mod fleet;
pub mod init;
mod manifest;

pub use manifest::{Template, CONFIG_SKELETON, FLEET_MANIFEST, GITIGNORE_ENTRIES, MANIFEST};

use serde::Serialize;
use std::io::Write;
use std::path::Path;

/// What an install did to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    Created,
    /// It existed and was kept.
    Skipped,
    Overwrote,
    /// It existed and lines were appended (`.gitignore`).
    Updated,
}

/// One file an install touched, `path` relative to the install root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileWrite {
    pub path: String,
    pub action: FileAction,
    /// For [`FileAction::Updated`]: what was appended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl FileWrite {
    /// The line `rupu init` / `rupu fleet install` prints for it.
    pub fn line(&self) -> String {
        match self.action {
            FileAction::Created => format!("CREATED {}", self.path),
            FileAction::Skipped => format!("SKIPPED {} (exists)", self.path),
            FileAction::Overwrote => format!("OVERWROTE {}", self.path),
            FileAction::Updated => format!(
                "UPDATED {} (appended {})",
                self.path,
                self.detail.as_deref().unwrap_or_default()
            ),
        }
    }
}

/// Every file an install touched, in the order it touched them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WriteReport {
    pub files: Vec<FileWrite>,
}

impl WriteReport {
    fn count(&self, action: FileAction) -> usize {
        self.files.iter().filter(|f| f.action == action).count()
    }

    pub fn created(&self) -> usize {
        self.count(FileAction::Created)
    }

    pub fn skipped(&self) -> usize {
        self.count(FileAction::Skipped)
    }

    pub fn overwrote(&self) -> usize {
        self.count(FileAction::Overwrote)
    }

    fn push(&mut self, root: &Path, path: &Path, action: FileAction) {
        self.files.push(FileWrite {
            path: relpath(root, path),
            action,
            detail: None,
        });
    }
}

/// Write `content` to a newly created `path`; `force` retains overwrite behavior.
/// Non-forced creation never follows or replaces an existing final symlink.
/// A failed write may leave a partial file and returns its I/O error, not Created.
/// This does not protect parent directories against symlinks or replacement.
fn write_file(path: &Path, content: &str, force: bool) -> std::io::Result<FileAction> {
    if force {
        let action = if path.exists() {
            FileAction::Overwrote
        } else {
            FileAction::Created
        };
        std::fs::write(path, content)?;
        return Ok(action);
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(FileAction::Skipped);
        }
        Err(error) => return Err(error),
    };
    file.write_all(content.as_bytes())?;
    Ok(FileAction::Created)
}

/// Write every template of `templates` under `root`, creating directories.
fn write_templates(
    root: &Path,
    templates: &[Template],
    force: bool,
    report: &mut WriteReport,
) -> std::io::Result<()> {
    for t in templates {
        let dest = root.join(t.target_relpath);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let action = write_file(&dest, t.content, force)?;
        report.push(root, &dest, action);
    }
    Ok(())
}

fn relpath(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

#[cfg(test)]
mod write_file_tests {
    use super::*;

    #[test]
    fn nonforced_existing_file_is_preserved_and_new_file_is_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.md");
        assert_eq!(
            write_file(&path, "original content", false).unwrap(),
            FileAction::Created
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original content");
        assert_eq!(
            write_file(&path, "replacement", false).unwrap(),
            FileAction::Skipped
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original content");
    }

    #[test]
    fn concurrent_creators_choose_one_complete_winner_without_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.md");
        let gate = std::sync::Arc::new(std::sync::Barrier::new(8));
        let results = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8)
                .map(|index| {
                    let path = path.clone();
                    let gate = std::sync::Arc::clone(&gate);
                    scope.spawn(move || {
                        let content = format!(
                            "writer-{index}:{}",
                            char::from(b'a' + index).to_string().repeat(16_384)
                        );
                        gate.wait();
                        (write_file(&path, &content, false).unwrap(), content)
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        let winners: Vec<_> = results
            .iter()
            .filter(|(action, _)| *action == FileAction::Created)
            .collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|(action, _)| *action == FileAction::Skipped)
                .count(),
            7
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), winners[0].1);
    }

    #[test]
    fn existing_directory_is_skipped_and_other_open_errors_propagate() {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("agent.md");
        std::fs::create_dir(&directory).unwrap();
        assert_eq!(
            write_file(&directory, "content", false).unwrap(),
            FileAction::Skipped
        );
        assert!(directory.is_dir());
        let missing_parent = tmp.path().join("missing/agent.md");
        assert!(write_file(&missing_parent, "content", false).is_err());
        assert!(!missing_parent.exists());
    }

    #[test]
    fn failed_batch_reports_no_success_for_failed_path_and_keeps_prior_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("blocked"), "existing blocker").unwrap();
        let templates = [
            Template {
                target_relpath: "first.md",
                content: "created before failure",
            },
            Template {
                target_relpath: "blocked/second.md",
                content: "unwritten",
            },
        ];
        let mut report = WriteReport::default();
        assert!(write_templates(root, &templates, false, &mut report).is_err());
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].path, "first.md");
        assert_eq!(report.files[0].action, FileAction::Created);
        assert_eq!(
            std::fs::read_to_string(root.join("first.md")).unwrap(),
            "created before failure"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("blocked")).unwrap(),
            "existing blocker"
        );
        assert!(!root.join("blocked/second.md").exists());
    }

    #[test]
    fn forced_creation_and_overwrite_keep_existing_actions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.md");
        assert_eq!(
            write_file(&path, "first", true).unwrap(),
            FileAction::Created
        );
        assert_eq!(
            write_file(&path, "second", true).unwrap(),
            FileAction::Overwrote
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "second");
    }

    #[cfg(unix)]
    #[test]
    fn nonforced_symlinks_including_dangling_links_are_preserved() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.md");
        let link = tmp.path().join("agent.md");
        symlink(&target, &link).unwrap();
        assert_eq!(
            write_file(&link, "must not create target", false).unwrap(),
            FileAction::Skipped
        );
        assert!(!target.exists());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        std::fs::write(&target, "keep target").unwrap();
        assert_eq!(
            write_file(&link, "must not replace target", false).unwrap(),
            FileAction::Skipped
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep target");
        assert_eq!(std::fs::read_link(link).unwrap(), target);
    }

    #[cfg(unix)]
    #[test]
    fn nonforced_unreadable_file_is_skipped_without_opening_content() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.md");
        std::fs::write(&path, "keep unreadable content").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
        let result = write_file(&path, "replacement", false);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(result.unwrap(), FileAction::Skipped);
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "keep unreadable content"
        );
    }

    #[cfg(unix)]
    #[test]
    fn forced_dangling_symlink_keeps_prior_follow_and_created_semantics() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.md");
        let link = tmp.path().join("agent.md");
        symlink(&target, &link).unwrap();
        assert_eq!(
            write_file(&link, "forced", true).unwrap(),
            FileAction::Created
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "forced");
        assert_eq!(
            write_file(&link, "forced again", true).unwrap(),
            FileAction::Overwrote
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "forced again");
        assert_eq!(std::fs::read_link(link).unwrap(), target);
    }
}
