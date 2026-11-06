//! The stock security-assessment fleet: the generic agents + workflows the
//! built-in engagement profiles name in their `[bundle]` (a `network`
//! profile's `recon` / `service-analyst` / `exploit-verifier`, a `web`
//! profile's `crawler` / `appsec-tester`, and so on).
//!
//! It installs into a rupu root — the global `~/.rupu` (or `$RUPU_HOME`),
//! where every engagement in any project reads it via
//! `rupu_agent::load_agents` / `rupu_orchestrator::list_workflow_summaries`,
//! or a project's `.rupu/`. Files that already exist are kept (a user's own
//! edits win); `force` re-seeds them.

use crate::{write_templates, WriteReport, FLEET_MANIFEST};
use serde::Serialize;
use std::path::Path;

/// Write every [`FLEET_MANIFEST`] entry under `root`. Each entry's
/// `target_relpath` is relative to that root (`agents/<name>.md`,
/// `workflows/<id>.yaml`). Existing files are skipped unless `force`.
pub fn install_into(root: &Path, force: bool) -> std::io::Result<WriteReport> {
    let mut report = WriteReport::default();
    write_templates(root, FLEET_MANIFEST, force, &mut report)?;
    Ok(report)
}

/// What a fleet entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Agent,
    Workflow,
}

/// One fleet file as a root has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Installed {
    Missing,
    /// On disk, byte-for-byte the shipped file.
    Current,
    /// On disk and different: edited, or shipped by another rupu version.
    Modified,
    /// The path exists but can't be read as the file.
    Unreadable,
}

/// One entry of the fleet: its kind, its name (an agent's name, a
/// workflow's id), and where it installs, relative to a rupu root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub kind: EntryKind,
    pub name: &'static str,
    pub path: &'static str,
}

/// Every fleet entry, in manifest order.
pub fn entries() -> Vec<Entry> {
    FLEET_MANIFEST
        .iter()
        .filter_map(|t| {
            let (kind, name) = if let Some(n) = t.target_relpath.strip_prefix("agents/") {
                (EntryKind::Agent, n.strip_suffix(".md")?)
            } else {
                let n = t.target_relpath.strip_prefix("workflows/")?;
                (EntryKind::Workflow, n.strip_suffix(".yaml")?)
            };
            Some(Entry {
                kind,
                name,
                path: t.target_relpath,
            })
        })
        .collect()
}

/// How `root` has each fleet entry, in [`entries`] order.
pub fn status(root: &Path) -> Vec<Installed> {
    FLEET_MANIFEST
        .iter()
        .map(|t| {
            let path = root.join(t.target_relpath);
            match std::fs::read(&path) {
                Ok(bytes) if bytes == t.content.as_bytes() => Installed::Current,
                Ok(_) => Installed::Modified,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Installed::Missing,
                Err(_) => Installed::Unreadable,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_the_whole_fleet_then_skips_on_a_second_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let t1 = install_into(root, false).unwrap();
        assert_eq!(t1.created(), FLEET_MANIFEST.len());
        assert_eq!(t1.skipped(), 0);
        assert_eq!(t1.overwrote(), 0);
        assert_eq!(
            t1.files[0].line(),
            format!("CREATED {}", FLEET_MANIFEST[0].target_relpath)
        );

        assert!(root.join("agents/recon.md").is_file());
        assert!(root.join("workflows/network-assessment.yaml").is_file());
        let recon = FLEET_MANIFEST
            .iter()
            .find(|t| t.target_relpath == "agents/recon.md")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("agents/recon.md")).unwrap(),
            recon.content
        );

        let t2 = install_into(root, false).unwrap();
        assert_eq!(t2.created(), 0);
        assert_eq!(t2.skipped(), FLEET_MANIFEST.len());
        assert_eq!(t2.overwrote(), 0);
    }

    #[test]
    fn force_overwrites_an_edited_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_into(root, false).unwrap();

        let recon = root.join("agents/recon.md");
        std::fs::write(&recon, "edited").unwrap();
        let t = install_into(root, true).unwrap();
        assert_eq!(t.created(), 0);
        assert_eq!(t.overwrote(), FLEET_MANIFEST.len());
        assert_ne!(std::fs::read_to_string(&recon).unwrap(), "edited");
    }

    #[test]
    fn status_tells_missing_current_and_modified_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(status(root).iter().all(|s| *s == Installed::Missing));
        install_into(root, false).unwrap();
        std::fs::write(root.join("agents/recon.md"), "edited").unwrap();
        let entries = entries();
        let st = status(root);
        assert_eq!(entries.len(), st.len());
        for (e, s) in entries.iter().zip(st) {
            let want = if e.path == "agents/recon.md" {
                Installed::Modified
            } else {
                Installed::Current
            };
            assert_eq!(s, want, "{}", e.path);
        }
    }

    #[test]
    fn entries_name_agents_and_workflows() {
        let entries = entries();
        assert_eq!(entries.len(), FLEET_MANIFEST.len());
        assert!(entries
            .iter()
            .any(|e| e.kind == EntryKind::Agent && e.name == "recon"));
        assert!(entries
            .iter()
            .any(|e| e.kind == EntryKind::Workflow && e.name == "network-assessment"));
    }
}
