//! `rupu fleet install [--force] [--project]` — materialize the stock
//! security-assessment fleet onto disk.
//!
//! The fleet is the generic agents + workflows the built-in engagement
//! profiles name in their `[bundle]` (a `network` profile's `recon` /
//! `service-analyst` / `exploit-verifier`, a `web` profile's `crawler` /
//! `appsec-tester`, and so on). By default it installs into the GLOBAL
//! rupu root (`~/.rupu`, or `$RUPU_HOME`), where every engagement — in
//! any project — reads it via `rupu_agent::load_agents` /
//! `rupu_orchestrator::list_workflow_summaries`. `--project` installs
//! into the current project's `.rupu/` instead.
//!
//! Files that already exist are KEPT (a user's own edits win); `--force`
//! re-seeds them. Source of truth: `crate::templates::FLEET_MANIFEST`.

use clap::Subcommand;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use crate::templates::FLEET_MANIFEST;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Install the stock fleet (agents + workflows) onto disk.
    Install(InstallArgs),
    /// List the agents and workflows the stock fleet ships.
    List,
}

#[derive(clap::Args, Debug)]
pub struct InstallArgs {
    /// Overwrite fleet files that already exist (default: keep them).
    #[arg(long)]
    pub force: bool,
    /// Install into the current project's `.rupu/` instead of the global
    /// `~/.rupu`.
    #[arg(long)]
    pub project: bool,
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::Install(args) => match install(args) {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
        Action::List => {
            print_list();
            ExitCode::from(0)
        }
    }
}

fn install(args: InstallArgs) -> anyhow::Result<()> {
    let root = if args.project {
        let pwd = std::env::current_dir()?;
        let project_root = crate::paths::project_root_for(&pwd)?.ok_or_else(|| {
            anyhow::anyhow!(
                "not inside a rupu project (no `.rupu/` found). Run `rupu init` first, \
                 or omit --project to install the fleet globally into ~/.rupu."
            )
        })?;
        project_root.join(".rupu")
    } else {
        crate::paths::global_dir()?
    };

    let tally = install_into(&root, args.force)?;
    println!(
        "fleet install: created {}, skipped {}, overwrote {} under {}",
        tally.created,
        tally.skipped,
        tally.overwrote,
        root.display()
    );
    if tally.skipped > 0 && !args.force {
        println!("(re-run with --force to overwrite the skipped files)");
    }
    Ok(())
}

/// Write every `FLEET_MANIFEST` entry under `root` (a rupu root — the
/// global `~/.rupu` or a project `.rupu/`). Each entry's `target_relpath`
/// is relative to that root (`agents/<name>.md`, `workflows/<id>.yaml`).
/// Existing files are skipped unless `force`. Prints one line per file.
pub fn install_into(root: &Path, force: bool) -> anyhow::Result<WriteTally> {
    let mut tally = WriteTally::default();
    for t in FLEET_MANIFEST {
        let dest = root.join(t.target_relpath);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        match write_file(&dest, t.content, force)? {
            FileAction::Created => {
                println!("CREATED {}", t.target_relpath);
                tally.created += 1;
            }
            FileAction::Skipped => {
                println!("SKIPPED {} (exists)", t.target_relpath);
                tally.skipped += 1;
            }
            FileAction::Overwrote => {
                println!("OVERWROTE {}", t.target_relpath);
                tally.overwrote += 1;
            }
        }
    }
    Ok(tally)
}

fn print_list() {
    let mut agents: Vec<&str> = Vec::new();
    let mut workflows: Vec<&str> = Vec::new();
    for t in FLEET_MANIFEST {
        if let Some(name) = t.target_relpath.strip_prefix("agents/") {
            agents.push(name.trim_end_matches(".md"));
        } else if let Some(id) = t.target_relpath.strip_prefix("workflows/") {
            workflows.push(id.trim_end_matches(".yaml"));
        }
    }
    println!(
        "Stock fleet — {} agents, {} workflows",
        agents.len(),
        workflows.len()
    );
    println!("\nAgents:");
    for a in agents {
        println!("  {a}");
    }
    println!("\nWorkflows:");
    for w in workflows {
        println!("  {w}");
    }
    println!(
        "\nInstall with `rupu fleet install` (into ~/.rupu) or `rupu fleet install --project`."
    );
}

/// A per-install tally of file outcomes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WriteTally {
    pub created: usize,
    pub skipped: usize,
    pub overwrote: usize,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_the_whole_fleet_then_skips_on_a_second_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // First pass: every fleet file is created.
        let t1 = install_into(root, false).unwrap();
        assert_eq!(t1.created, FLEET_MANIFEST.len());
        assert_eq!(t1.skipped, 0);
        assert_eq!(t1.overwrote, 0);

        // A representative agent and workflow landed where the loaders read.
        assert!(root.join("agents/recon.md").is_file());
        assert!(root.join("workflows/network-assessment.yaml").is_file());

        // Content matches the embedded template byte-for-byte.
        let recon = FLEET_MANIFEST
            .iter()
            .find(|t| t.target_relpath == "agents/recon.md")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("agents/recon.md")).unwrap(),
            recon.content
        );

        // Second pass without --force keeps everything (a user's edits win).
        let t2 = install_into(root, false).unwrap();
        assert_eq!(t2.created, 0);
        assert_eq!(t2.skipped, FLEET_MANIFEST.len());
        assert_eq!(t2.overwrote, 0);
    }

    #[test]
    fn force_overwrites_an_edited_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_into(root, false).unwrap();

        // Simulate a user edit, then re-seed with --force.
        let recon = root.join("agents/recon.md");
        std::fs::write(&recon, "edited").unwrap();
        let t = install_into(root, true).unwrap();
        assert_eq!(t.created, 0);
        assert_eq!(t.overwrote, FLEET_MANIFEST.len());
        assert_ne!(std::fs::read_to_string(&recon).unwrap(), "edited");
    }
}
