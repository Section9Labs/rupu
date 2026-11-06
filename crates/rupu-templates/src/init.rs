//! Bootstrap a project's `.rupu/` directory: the skeleton (`agents/`,
//! `workflows/`, `config.toml`), rupu's `.gitignore` entries, the
//! project-local netflow directory and, on request, the curated samples.
//! Re-running it merges: nothing that exists is overwritten except a sample
//! under `force`, and `config.toml` never is.
//!
//! Spec: docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md

use crate::{
    relpath, write_file, write_templates, FileAction, FileWrite, WriteReport, CONFIG_SKELETON,
    GITIGNORE_ENTRIES, MANIFEST,
};
use std::fs;
use std::path::Path;

/// Whether `root` already has a rupu project (`.rupu/config.toml`).
pub fn is_initialised(root: &Path) -> bool {
    root.join(".rupu").join("config.toml").exists()
}

/// Initialise `root`, which must be an existing directory.
/// `with_samples` adds [`MANIFEST`]; `force` overwrites samples that exist.
pub fn init_project(root: &Path, with_samples: bool, force: bool) -> std::io::Result<WriteReport> {
    let mut report = WriteReport::default();
    create_skeleton(root, &mut report)?;
    ensure_gitignore_entry(root, &mut report)?;
    ensure_netflow_dir(root, &mut report)?;
    if with_samples {
        write_templates(root, MANIFEST, force, &mut report)?;
    }
    Ok(report)
}

/// The `created`, `skipped`, `overwrote` counts `rupu init`'s summary line
/// prints: as [`WriteReport`]'s, except that `.gitignore` is listed but never
/// counted (`rupu init` has always counted only the skeleton, the netflow
/// directory and the samples).
pub fn summary_counts(report: &WriteReport) -> (usize, usize, usize) {
    let counted = |action: FileAction| {
        report
            .files
            .iter()
            .filter(|f| f.action == action && f.path != ".gitignore")
            .count()
    };
    (
        counted(FileAction::Created),
        counted(FileAction::Skipped),
        counted(FileAction::Overwrote),
    )
}

// config.toml does NOT honor `force` — overwriting a customized config is a
// worse footgun than re-seeding agent templates.
fn create_skeleton(root: &Path, report: &mut WriteReport) -> std::io::Result<()> {
    fs::create_dir_all(root.join(".rupu/agents"))?;
    fs::create_dir_all(root.join(".rupu/workflows"))?;
    let cfg_path = root.join(".rupu/config.toml");
    let action = write_file(&cfg_path, CONFIG_SKELETON, false)?;
    report.push(root, &cfg_path, action);
    Ok(())
}

fn ensure_gitignore_entry(root: &Path, report: &mut WriteReport) -> std::io::Result<()> {
    let path = root.join(".gitignore");
    if !path.exists() {
        let mut content = String::new();
        for needle in GITIGNORE_ENTRIES {
            content.push_str(needle);
            content.push('\n');
        }
        fs::write(&path, content)?;
        report.push(root, &path, FileAction::Created);
        return Ok(());
    }

    let body = fs::read_to_string(&path)?;
    let mut new_body = body.clone();
    let mut appended: Vec<&str> = Vec::new();
    for needle in GITIGNORE_ENTRIES {
        if body.lines().any(|l| l.trim() == *needle) {
            continue;
        }
        if !new_body.is_empty() && !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        new_body.push_str(needle);
        new_body.push('\n');
        appended.push(needle);
    }
    if appended.is_empty() {
        return Ok(());
    }
    fs::write(&path, new_body)?;
    report.files.push(FileWrite {
        path: relpath(root, &path),
        action: FileAction::Updated,
        detail: Some(appended.join(", ")),
    });
    Ok(())
}

/// Opt this project's `.rupu/netflow/` into project-local netflow routing,
/// in the same breath as the `.gitignore` entry `ensure_gitignore_entry`
/// just wrote — `rupu_netflow::netflow_dir`'s existence gate resolves a
/// run's ledger to this directory ONLY WHEN IT ALREADY EXISTS (else every
/// run falls back to the global root, unattributable to any one project
/// from the read side — see `rupu-cp`'s `get_project_netflow`), and
/// nothing else ever creates it.
///
/// This is a one-time, EXPLICIT opt-in `init` performs deliberately, not
/// auto-creation-on-first-write: it does not reopen the git-leak class the
/// existence gate exists to close, because the ignore protection (both
/// this project's own `.gitignore` entry above, and the directory's own
/// self-ignoring `.gitignore` `ensure_netflow_dir` installs) lands in the
/// very same `init` invocation, before any ledger can ever be written
/// there.
fn ensure_netflow_dir(root: &Path, report: &mut WriteReport) -> std::io::Result<()> {
    let dir = rupu_netflow::project_local_netflow_dir(root);
    // Checked BEFORE the call (`create_dir_all` / `ensure_self_ignore` are
    // both idempotent) so re-running `init` on an already-opted-in project
    // reports it skipped rather than a misleading second creation. This
    // directory's existence is what makes the CP's project-scoped Network
    // tab work, so it earns its own entry the way `.rupu/config.toml` does.
    let already_existed = dir.is_dir();
    rupu_netflow::ensure_netflow_dir(&dir)?;
    let action = if already_existed {
        FileAction::Skipped
    } else {
        FileAction::Created
    };
    report.push(root, &dir, action);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_writes_the_skeleton_then_merges() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!is_initialised(root));
        let first = init_project(root, true, false).unwrap();
        assert!(is_initialised(root));
        assert!(root.join(".rupu/agents").is_dir());
        assert!(root.join(".rupu/netflow").is_dir());
        assert_eq!(first.skipped(), 0, "{first:?}");
        // The CLI's summary leaves the created `.gitignore` out, as it always has.
        assert_eq!(first.created(), MANIFEST.len() + 3);
        assert_eq!(summary_counts(&first), (MANIFEST.len() + 2, 0, 0));
        assert_eq!(
            first.files[0],
            FileWrite {
                path: ".rupu/config.toml".into(),
                action: FileAction::Created,
                detail: None
            }
        );
        let second = init_project(root, true, false).unwrap();
        assert_eq!(second.created(), 0, "{second:?}");
        // config.toml, netflow and every sample are kept; the gitignore
        // already has its entries, so it is not reported.
        assert_eq!(second.skipped(), MANIFEST.len() + 2);
    }

    #[test]
    fn a_gitignore_missing_an_entry_is_appended_to() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "target/").unwrap();
        let r = init_project(root, false, false).unwrap();
        let g = r.files.iter().find(|f| f.path == ".gitignore").unwrap();
        assert_eq!(g.action, FileAction::Updated);
        assert_eq!(
            g.line(),
            "UPDATED .gitignore (appended .rupu/transcripts/, .rupu/netflow/)"
        );
        assert_eq!(
            fs::read_to_string(root.join(".gitignore")).unwrap(),
            "target/\n.rupu/transcripts/\n.rupu/netflow/\n"
        );
    }
}
