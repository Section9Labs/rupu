//! Hidden `rupu __workspace stage|collect` helper — the remote side of SSH
//! workspace sync. Reads/writes raw bytes over stdin/stdout and delegates to
//! the shared rupu-cp staging core so remote staging is byte-identical to the
//! Local transport. Confined to the rupu cache root.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;

use rupu_cp::host::workspace_stage::{collect_from_dir, discard_from_dir, stage_to_dir};

#[derive(Subcommand, Debug)]
pub enum WorkspaceHelperAction {
    /// Stage a packed workspace read from stdin; print the working dir.
    Stage,
    /// Collect the change-delta from a staged working dir; write it to stdout.
    Collect { working_dir: String },
    /// Best-effort discard of a staged working dir's scratch (no stdout).
    Discard { working_dir: String },
}

pub async fn handle(action: WorkspaceHelperAction) -> ExitCode {
    match handle_inner(action) {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

fn handle_inner(action: WorkspaceHelperAction) -> anyhow::Result<()> {
    let cache_root = cache_root()?;
    match action {
        WorkspaceHelperAction::Stage => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            let work = stage_bytes(&buf, &cache_root)?;
            println!("{work}");
        }
        WorkspaceHelperAction::Collect { working_dir } => {
            let delta = collect_bytes(&working_dir, &cache_root)?;
            std::io::stdout().write_all(&delta)?;
            std::io::stdout().flush()?;
        }
        WorkspaceHelperAction::Discard { working_dir } => {
            discard_bytes(&working_dir, &cache_root)?;
        }
    }
    Ok(())
}

fn stage_bytes(stdin: &[u8], cache_root: &Path) -> anyhow::Result<String> {
    stage_to_dir(stdin, cache_root).map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn collect_bytes(working_dir: &str, cache_root: &Path) -> anyhow::Result<Vec<u8>> {
    collect_from_dir(working_dir, cache_root).map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn discard_bytes(working_dir: &str, cache_root: &Path) -> anyhow::Result<()> {
    discard_from_dir(working_dir, cache_root).map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// The rupu global/cache dir — the SAME base the Local transport stages under
/// (`global_dir/workspace-sync/...`).
fn cache_root() -> anyhow::Result<PathBuf> {
    crate::paths::global_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_stage_then_collect_round_trips() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a.txt"), "orig").unwrap();
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        let encoded = rupu_cp::host::connector::encode_payload(&payload);

        let cache = tempfile::tempdir().unwrap();
        let work = stage_bytes(&encoded, cache.path()).unwrap();
        std::fs::write(std::path::Path::new(&work).join("a.txt"), "EDITED").unwrap();
        let delta = collect_bytes(&work, cache.path()).unwrap();
        assert!(!delta.is_empty());
        let d = rupu_cp::host::connector::decode_delta(&delta).unwrap();
        assert!(d.changed.iter().any(|p| p == "a.txt"));
    }

    #[test]
    fn helper_stage_rejects_oversize() {
        let cache = tempfile::tempdir().unwrap();
        let huge = vec![0u8; rupu_cp::host::connector::MAX_WORKSPACE_BYTES + 1];
        assert!(stage_bytes(&huge, cache.path()).is_err());
    }

    /// `stage` then `discard` (the remote-cleanup action, T4) removes the
    /// scratch dir entirely — mirrors the stage-then-collect round trip but
    /// exercises the discard action added for the dispatcher cleanup paths.
    #[test]
    fn helper_stage_then_discard_removes_scratch() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a.txt"), "orig").unwrap();
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        let encoded = rupu_cp::host::connector::encode_payload(&payload);

        let cache = tempfile::tempdir().unwrap();
        let work = stage_bytes(&encoded, cache.path()).unwrap();
        let base = std::path::Path::new(&work).parent().unwrap().to_path_buf();
        assert!(base.exists());

        discard_bytes(&work, cache.path()).unwrap();

        assert!(!base.exists(), "scratch base dir must be removed");
        assert!(!std::path::Path::new(&work).exists());
    }
}
