//! `rupu init [PATH] [--with-samples] [--force] [--git]` — bootstrap a
//! project's `.rupu/` directory through `rupu_templates::init` (the CP's
//! `/api/projects/init` uses the same).
//!
//! Spec: docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md

use clap::Args as ClapArgs;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(ClapArgs, Debug)]
pub struct InitArgs {
    /// Target directory for the new project. Defaults to the current
    /// working directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Include the curated agent + workflow templates.
    #[arg(long)]
    pub with_samples: bool,

    /// Overwrite existing template files (still merges by default).
    #[arg(long)]
    pub force: bool,

    /// Run `git init` afterwards if the target is not already inside a git repo.
    #[arg(long)]
    pub git: bool,
}

pub async fn handle(args: InitArgs) -> ExitCode {
    match init_inner(args) {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

/// Test entry point. Same code path as `handle` but returns the error
/// instead of mapping to ExitCode, so integration tests can assert on
/// success/failure without spawning a binary.
pub fn init_for_test(args: InitArgs) -> anyhow::Result<()> {
    init_inner(args)
}

fn init_inner(args: InitArgs) -> anyhow::Result<()> {
    let root = &args.path;
    if !root.exists() {
        fs::create_dir_all(root)?;
    } else if !root.is_dir() {
        anyhow::bail!("PATH exists but is not a directory: {}", root.display());
    }

    let report = rupu_templates::init::init_project(root, args.with_samples, args.force)?;
    for f in &report.files {
        println!("{}", f.line());
    }
    println!(
        "init: created {}, skipped {}, overwrote {}",
        report.created(),
        report.skipped(),
        report.overwrote()
    );
    if args.git {
        maybe_git_init(root)?;
    }
    Ok(())
}

fn maybe_git_init(root: &Path) -> anyhow::Result<()> {
    if which::which("git").is_err() {
        eprintln!("init: --git requested but git not found on PATH; skipping");
        return Ok(());
    }
    let inside = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .map(|o| o.status.success() && o.stdout.starts_with(b"true"))
        .unwrap_or(false);
    if inside {
        return Ok(());
    }
    let status = std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .status()?;
    if !status.success() {
        eprintln!("init: git init exited with status {status}; continuing");
    }
    Ok(())
}
