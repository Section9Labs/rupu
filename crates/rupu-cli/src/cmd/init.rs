//! `rupu init [PATH] [--with-samples] [--force] [--git]` — bootstrap a
//! project's `.rupu/` directory.
//!
//! Spec: docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md

use clap::Args as ClapArgs;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::templates::{CONFIG_SKELETON, GITIGNORE_ENTRY};

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

    create_skeleton(root)?;
    ensure_gitignore_entry(root)?;
    Ok(())
}

fn create_skeleton(root: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(root.join(".rupu/agents"))?;
    fs::create_dir_all(root.join(".rupu/workflows"))?;

    let cfg_path = root.join(".rupu/config.toml");
    if !cfg_path.exists() {
        fs::write(&cfg_path, CONFIG_SKELETON)?;
        println!("CREATED {}", relpath(root, &cfg_path));
    } else {
        println!("SKIPPED {} (exists)", relpath(root, &cfg_path));
    }
    Ok(())
}

fn ensure_gitignore_entry(root: &Path) -> anyhow::Result<()> {
    let path = root.join(".gitignore");
    let needle = GITIGNORE_ENTRY;

    if !path.exists() {
        fs::write(&path, format!("{needle}\n"))?;
        println!("CREATED {}", relpath(root, &path));
        return Ok(());
    }

    let body = fs::read_to_string(&path)?;
    if body.lines().any(|l| l.trim() == needle) {
        return Ok(());
    }
    let mut new_body = body;
    if !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    new_body.push_str(needle);
    new_body.push('\n');
    fs::write(&path, new_body)?;
    println!("UPDATED {} (appended {needle})", relpath(root, &path));
    Ok(())
}

fn relpath(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}
