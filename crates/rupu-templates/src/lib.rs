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

/// Write `content` to `path` unless it exists; `force` overwrites it.
fn write_file(path: &Path, content: &str, force: bool) -> std::io::Result<FileAction> {
    if !path.exists() {
        std::fs::write(path, content)?;
        return Ok(FileAction::Created);
    }
    if force {
        std::fs::write(path, content)?;
        return Ok(FileAction::Overwrote);
    }
    Ok(FileAction::Skipped)
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
