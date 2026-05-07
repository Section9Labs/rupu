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
        Err(e) => crate::output::diag::fail(e)
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

    let mut tally = WriteTally::default();
    create_skeleton(root, &mut tally)?;
    ensure_gitignore_entry(root)?;

    if args.with_samples {
        write_manifest(root, args.force, &mut tally)?;
    }

    println!(
        "init: created {}, skipped {}, overwrote {}",
        tally.created, tally.skipped, tally.overwrote
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

#[derive(Default)]
struct WriteTally {
    created: usize,
    skipped: usize,
    overwrote: usize,
}

#[derive(Debug, Clone, Copy)]
enum FileAction {
    Created,
    Skipped,
    Overwrote,
}

fn write_file(path: &Path, content: &str, force: bool) -> anyhow::Result<FileAction> {
    if !path.exists() {
        fs::write(path, content)?;
        return Ok(FileAction::Created);
    }
    if force {
        fs::write(path, content)?;
        return Ok(FileAction::Overwrote);
    }
    Ok(FileAction::Skipped)
}

// config.toml does NOT honor --force — overwriting a customized config is
// a worse footgun than re-seeding agent templates.
fn create_skeleton(root: &Path, tally: &mut WriteTally) -> anyhow::Result<()> {
    fs::create_dir_all(root.join(".rupu/agents"))?;
    fs::create_dir_all(root.join(".rupu/workflows"))?;

    let cfg_path = root.join(".rupu/config.toml");
    let action = write_file(&cfg_path, CONFIG_SKELETON, false)?;
    match action {
        FileAction::Created => {
            println!("CREATED {}", relpath(root, &cfg_path));
            tally.created += 1;
        }
        FileAction::Skipped => {
            println!("SKIPPED {} (exists)", relpath(root, &cfg_path));
            tally.skipped += 1;
        }
        FileAction::Overwrote => {
            unreachable!("config.toml never gets force=true at this layer")
        }
    }
    Ok(())
}

fn write_manifest(root: &Path, force: bool, tally: &mut WriteTally) -> anyhow::Result<()> {
    use crate::templates::MANIFEST;
    for t in MANIFEST {
        let dest = root.join(t.target_relpath);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let action = write_file(&dest, t.content, force)?;
        match action {
            FileAction::Created => {
                println!("CREATED {}", relpath(root, &dest));
                tally.created += 1;
            }
            FileAction::Skipped => {
                println!("SKIPPED {} (exists)", relpath(root, &dest));
                tally.skipped += 1;
            }
            FileAction::Overwrote => {
                println!("OVERWROTE {}", relpath(root, &dest));
                tally.overwrote += 1;
            }
        }
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
