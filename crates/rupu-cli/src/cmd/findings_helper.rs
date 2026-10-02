//! `rupu __findings` — hidden helper a coordinator runs over SSH to pull a
//! finding artifact from this host's store (spec
//! 2026-09-30-rupu-remote-findings-transport-design.md §B1).

use anyhow::Context as _;
use clap::Subcommand;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum FindingsHelperAction {
    /// Write the stored blob `<sha256>` to stdout; nonzero exit if absent.
    Artifact { sha256: String },
}

pub async fn handle(action: FindingsHelperAction) -> ExitCode {
    match handle_inner(action) {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

fn handle_inner(action: FindingsHelperAction) -> anyhow::Result<()> {
    match action {
        FindingsHelperAction::Artifact { sha256 } => {
            let global = crate::paths::global_dir()?;
            let mut out = std::io::stdout().lock();
            write_blob(&global, &sha256, &mut out)?;
            out.flush()?;
            Ok(())
        }
    }
}

/// `<global>/findings/artifacts/<aa>/<sha256>`, after validating the sha.
pub(crate) fn blob_path(global: &Path, sha256: &str) -> anyhow::Result<PathBuf> {
    rupu_coverage::report::ArtifactStore::new(global.join("findings").join("artifacts"))
        .blob_path_checked(sha256)
        .ok_or_else(|| anyhow::anyhow!("{sha256:?} is not a sha256 (64 lowercase hex characters)"))
}

/// Copy the stored blob to `out`; returns the byte count.
fn write_blob(global: &Path, sha256: &str, out: &mut impl Write) -> anyhow::Result<u64> {
    let path = blob_path(global, sha256)?;
    let mut f = std::fs::File::open(&path).with_context(|| {
        format!(
            "artifact {sha256} is not in this host's store ({})",
            path.display()
        )
    })?;
    Ok(std::io::copy(&mut f, out)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_blob_copies_the_stored_bytes() {
        let global = tempfile::tempdir().unwrap();
        let sha = "cd".repeat(32);
        let p = blob_path(global.path(), &sha).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"poc bytes").unwrap();
        let mut out = Vec::new();
        assert_eq!(write_blob(global.path(), &sha, &mut out).unwrap(), 9);
        assert_eq!(out, b"poc bytes");
    }

    #[test]
    fn write_blob_refuses_a_missing_blob_and_a_bad_sha() {
        let global = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        let err = write_blob(global.path(), &"ee".repeat(32), &mut out).unwrap_err();
        assert!(
            err.to_string().contains("not in this host's store"),
            "{err}"
        );
        let err = write_blob(global.path(), "../x", &mut out).unwrap_err();
        assert!(err.to_string().contains("is not a sha256"), "{err}");
        assert!(out.is_empty(), "nothing is written for a refused sha");
    }
}
