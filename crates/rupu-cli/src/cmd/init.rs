//! `rupu init [PATH] [--with-samples] [--force] [--git]` — bootstrap a
//! project's `.rupu/` directory.
//!
//! Spec: docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md

use clap::Args as ClapArgs;
use std::path::PathBuf;
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
        Err(e) => {
            eprintln!("rupu init: {e}");
            ExitCode::from(1)
        }
    }
}

fn init_inner(_args: InitArgs) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented (Task 4)")
}
