//! Bidirectional manifest sync:
//!   1. Every entry in MANIFEST / FLEET_MANIFEST exists on disk under
//!      crates/rupu-cli/templates/.
//!   2. Every file under crates/rupu-cli/templates/ appears in one of them.
//!
//! MANIFEST source files live directly under `templates/` (its
//! `target_relpath` is `.rupu/<...>`); FLEET_MANIFEST source files live
//! under `templates/fleet/` (its `target_relpath` is global-root-relative,
//! e.g. `agents/recon.md`).

use std::collections::HashSet;
use std::path::PathBuf;

use rupu_cli::templates::{FLEET_MANIFEST, MANIFEST};

fn templates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates")
}

#[test]
fn every_manifest_entry_exists_on_disk() {
    let dir = templates_dir();
    for t in MANIFEST {
        // target_relpath is `.rupu/agents/foo.md`; the on-disk source
        // is `templates/agents/foo.md` — drop the ".rupu/" prefix.
        let stripped = t
            .target_relpath
            .strip_prefix(".rupu/")
            .expect("manifest paths must start with .rupu/");
        let path = dir.join(stripped);
        assert!(
            path.exists(),
            "manifest entry {} has no source file at {}",
            t.target_relpath,
            path.display()
        );
    }
    for t in FLEET_MANIFEST {
        // target_relpath is global-root-relative (`agents/recon.md`); the
        // on-disk source is `templates/fleet/agents/recon.md`.
        let path = dir.join("fleet").join(t.target_relpath);
        assert!(
            path.exists(),
            "fleet manifest entry {} has no source file at {}",
            t.target_relpath,
            path.display()
        );
    }
}

#[test]
fn every_template_file_is_in_manifest() {
    let dir = templates_dir();
    let mut on_disk = HashSet::new();
    walk(&dir, &dir, &mut on_disk);

    let mut in_manifest: HashSet<String> = MANIFEST
        .iter()
        .map(|t| {
            t.target_relpath
                .strip_prefix(".rupu/")
                .expect("manifest paths must start with .rupu/")
                .to_string()
        })
        .collect();
    // FLEET_MANIFEST sources live under templates/fleet/.
    in_manifest.extend(
        FLEET_MANIFEST
            .iter()
            .map(|t| format!("fleet/{}", t.target_relpath)),
    );

    let missing: Vec<&String> = on_disk.difference(&in_manifest).collect();
    assert!(
        missing.is_empty(),
        "files exist under templates/ but are not in a manifest: {missing:?}"
    );
}

fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(base, &p, out);
        } else if p.is_file() {
            let rel = p.strip_prefix(base).unwrap().display().to_string();
            out.insert(rel);
        }
    }
}
