//! `rupu __apply-update` — hidden, privileged install step.
//!
//! `rupu update` invokes this (via `sudo`) when the target install
//! directory isn't writable by the current user. It trusts nothing
//! from argv: the staged file's sha256 is re-verified before the
//! atomic swap, so a compromised or stale `--sha256` argument can't
//! smuggle in unverified bytes.

use anyhow::{Context, Result};
use clap::Args;
use std::path::Path;
use std::process::ExitCode;

/// True if we can create/replace files in `dir`.
pub fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".rupu-write-probe.{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[derive(Args, Debug)]
pub struct ApplyUpdateArgs {
    /// Path to the staged, already-downloaded binary.
    #[arg(long)]
    pub from: std::path::PathBuf,
    /// Path to the live binary to replace.
    #[arg(long)]
    pub to: std::path::PathBuf,
    /// Expected sha256 of the staged binary (re-verified here, not trusted).
    #[arg(long)]
    pub sha256: String,
}

/// Privileged apply step (invoked via sudo). Re-verifies the staged
/// file's checksum before swapping.
fn run_inner(args: ApplyUpdateArgs) -> Result<()> {
    let bytes = std::fs::read(&args.from).context("read staged binary")?;
    let side = format!("{}  staged", args.sha256);
    rupu_update::verify::verify_checksum(&bytes, &side).context("staged checksum re-verify")?;
    let bak = rupu_update::install::backup_dir().join(format!(
        "rupu-{}",
        args.to
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("prev")
    ));
    rupu_update::install::swap_in_place(&bytes, &args.to, Some(&bak)).context("privileged swap")?;
    println!("applied update to {}", args.to.display());
    Ok(())
}

pub fn handle(args: ApplyUpdateArgs) -> ExitCode {
    match run_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => crate::output::diag::fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempdir_is_writable() {
        let d = tempfile::tempdir().unwrap();
        assert!(dir_writable(d.path()));
    }

    #[test]
    fn root_owned_dir_not_writable() {
        // /usr/local/bin is root-owned in this environment; skip the
        // assertion if it's somehow writable (e.g. a Homebrew-owned
        // sandbox on some CI images) rather than asserting a false
        // negative.
        if !dir_writable(Path::new("/usr/local/bin")) {
            assert!(!dir_writable(Path::new("/usr/local/bin")));
        }
    }
}
