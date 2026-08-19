//! `rupu init [PATH] [--with-samples] [--force] [--git]` — bootstrap a
//! project's `.rupu/` directory.
//!
//! Spec: docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md

use clap::Args as ClapArgs;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::templates::{CONFIG_SKELETON, GITIGNORE_ENTRIES};

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

    let mut tally = WriteTally::default();
    create_skeleton(root, &mut tally)?;
    ensure_gitignore_entry(root)?;
    ensure_netflow_dir(root, &mut tally)?;

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
    let needles = GITIGNORE_ENTRIES;

    if !path.exists() {
        let mut content = String::new();
        for needle in needles {
            content.push_str(needle);
            content.push('\n');
        }
        fs::write(&path, content)?;
        println!("CREATED {}", relpath(root, &path));
        return Ok(());
    }

    let body = fs::read_to_string(&path)?;
    let mut new_body = body.clone();
    let mut appended: Vec<&str> = Vec::new();
    for needle in needles {
        if body.lines().any(|l| l.trim() == *needle) {
            continue;
        }
        if !new_body.is_empty() && !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        new_body.push_str(needle);
        new_body.push('\n');
        appended.push(needle);
    }
    if appended.is_empty() {
        return Ok(());
    }
    fs::write(&path, new_body)?;
    println!(
        "UPDATED {} (appended {})",
        relpath(root, &path),
        appended.join(", ")
    );
    Ok(())
}

/// Opt this project's `.rupu/netflow/` into project-local netflow routing,
/// in the same breath as the `.gitignore` entry `ensure_gitignore_entry`
/// just wrote — `rupu_netflow::netflow_dir`'s existence gate resolves a
/// run's ledger to this directory ONLY WHEN IT ALREADY EXISTS (else every
/// run falls back to the global root, unattributable to any one project
/// from the read side — see `rupu-cp`'s `get_project_netflow`), and
/// nothing else in this crate ever creates it.
///
/// This is a one-time, EXPLICIT opt-in `init` performs deliberately, not
/// auto-creation-on-first-write: it does not reopen the git-leak class the
/// existence gate exists to close, because the ignore protection (both
/// this project's own `.gitignore` entry above, and the directory's own
/// self-ignoring `.gitignore` `ensure_netflow_dir` installs) lands in the
/// very same `init` invocation, before any ledger can ever be written
/// there.
fn ensure_netflow_dir(root: &Path, tally: &mut WriteTally) -> anyhow::Result<()> {
    let dir = rupu_netflow::project_local_netflow_dir(root);
    // Checked BEFORE the call (not derived from its `Result`, which is
    // `Ok(())` either way — `create_dir_all`/`ensure_self_ignore` are both
    // idempotent) so re-running `init` on an already-opted-in project
    // reports SKIPPED rather than a misleading second CREATED. This is
    // the one visible line confirming the netflow opt-in actually
    // happened — unlike `.rupu/agents`/`.rupu/workflows` (silently
    // created, no tally line), this directory's existence is what makes
    // rupu-cp's project-scoped Network tab work, so it earns its own
    // confirmation the way `.rupu/config.toml` does.
    let already_existed = dir.is_dir();
    rupu_netflow::ensure_netflow_dir(&dir)?;
    if already_existed {
        println!("SKIPPED {} (exists)", relpath(root, &dir));
        tally.skipped += 1;
    } else {
        println!("CREATED {}", relpath(root, &dir));
        tally.created += 1;
    }
    Ok(())
}

fn relpath(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}
