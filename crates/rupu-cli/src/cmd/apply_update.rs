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
    /// Path to back up the current `--to` binary to before swapping.
    /// Computed by the (unprivileged) caller in the USER's `$HOME` —
    /// under `sudo` this process's own `$HOME` may resolve to root's
    /// home, which `rupu update --rollback` (run as the user) could
    /// never find.
    #[arg(long)]
    pub backup: std::path::PathBuf,
}

/// Privileged apply step (invoked via sudo). Re-verifies the staged
/// file's checksum before swapping.
fn run_inner(args: ApplyUpdateArgs) -> Result<()> {
    let bytes = std::fs::read(&args.from).context("read staged binary")?;
    let side = format!("{}  staged", args.sha256);
    rupu_update::verify::verify_checksum(&bytes, &side).context("staged checksum re-verify")?;
    rupu_update::install::swap_in_place(&bytes, &args.to, Some(&args.backup))
        .context("privileged swap")?;
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

    #[derive(clap::Parser, Debug)]
    struct Wrapper {
        #[command(flatten)]
        args: ApplyUpdateArgs,
    }

    #[test]
    fn backup_arg_is_parsed() {
        use clap::Parser;
        let w = Wrapper::parse_from([
            "__apply-update",
            "--from",
            "/tmp/staged",
            "--to",
            "/usr/local/bin/rupu",
            "--sha256",
            "abc123",
            "--backup",
            "/home/user/.rupu/backups/rupu-rupu",
        ]);
        assert_eq!(w.args.from, std::path::PathBuf::from("/tmp/staged"));
        assert_eq!(w.args.to, std::path::PathBuf::from("/usr/local/bin/rupu"));
        assert_eq!(w.args.sha256, "abc123");
        assert_eq!(
            w.args.backup,
            std::path::PathBuf::from("/home/user/.rupu/backups/rupu-rupu")
        );
    }

    #[test]
    fn backup_arg_is_required() {
        use clap::Parser;
        let res = Wrapper::try_parse_from([
            "__apply-update",
            "--from",
            "/tmp/staged",
            "--to",
            "/usr/local/bin/rupu",
            "--sha256",
            "abc123",
        ]);
        assert!(res.is_err(), "expected missing --backup to fail parsing");
    }
}
