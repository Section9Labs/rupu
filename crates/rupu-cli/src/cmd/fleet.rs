//! `rupu fleet install [--force] [--project] [--skills-from DIR]` —
//! materialize the stock security-assessment fleet onto disk.
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
//! The fleet is also the skills the bundles name: each built-in profile's
//! `[bundle].skills` entry is pinned to a commit of a skills repository and
//! installed into the same layer as the agents (`rupu_skills::pinned`).
//! `--skills-from <dir>` serves those pins from a local checkout instead of
//! fetching them, for boxes without network access.
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
    /// Install the stock fleet (agents, workflows and bundled skills) onto disk.
    Install(InstallArgs),
    /// List the agents, workflows and skills the stock fleet ships.
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
    /// Install the fleet's skills from a local checkout of
    /// Section9Labs/rupu-skills at the pinned commit (offline boxes),
    /// instead of fetching them.
    #[arg(long, value_name = "DIR")]
    pub skills_from: Option<std::path::PathBuf>,
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

    let global = crate::paths::global_dir()?;
    let layer = if args.project && !same_dir(&root, &global) {
        rupu_skills::Layer::Project
    } else {
        rupu_skills::Layer::Global
    };
    let pins = fleet_pins();
    let fetcher: Box<dyn rupu_skills::Fetch> = match &args.skills_from {
        Some(dir) => Box::new(rupu_skills::pinned::PinnedCheckout { dir: dir.clone() }),
        None => Box::new(rupu_skills::git::GitFetcher),
    };
    let report = rupu_skills::pinned::install_pins(
        &pins,
        &root.join("skills"),
        layer,
        args.force,
        fetcher.as_ref(),
    );
    for (pin, outcome) in &report.results {
        println!("{}", pin_line(pin, outcome));
    }
    let missing = report.missing();
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|p| p.name.as_str()).collect();
        anyhow::bail!(
            "fleet install: {} skill(s) not installed: {}. Agents that grant them will not \
             launch. Re-run `rupu fleet install` when the network is back, or on an offline \
             box `rupu fleet install --skills-from <a checkout of {} containing commit {}>`",
            missing.len(),
            names.join(", "),
            missing[0].source,
            missing[0].commit,
        );
    }
    Ok(())
}

/// The skills the stock fleet installs: every built-in profile's bundled
/// skills, one per name (the lockstep tests keep the profiles' pins equal).
pub fn fleet_pins() -> Vec<rupu_skills::pinned::Pin> {
    let mut by_name = std::collections::BTreeMap::new();
    for (_, p) in rupu_coverage::builtin_profiles() {
        for s in p.bundle.skills {
            by_name
                .entry(s.name.clone())
                .or_insert(rupu_skills::pinned::Pin {
                    name: s.name,
                    source: s.source,
                    tag: s.tag,
                    commit: s.commit,
                });
        }
    }
    by_name.into_values().collect()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn short(c: &str) -> &str {
    &c[..c.len().min(12)]
}

fn pin_line(pin: &rupu_skills::pinned::Pin, o: &rupu_skills::pinned::PinOutcome) -> String {
    use rupu_skills::pinned::{KeptReason, PinOutcome};
    let n = &pin.name;
    match o {
        PinOutcome::Created => format!("CREATED skills/{n} ({} {})", pin.tag, short(&pin.commit)),
        PinOutcome::Skipped => format!("SKIPPED skills/{n} (pinned {})", pin.tag),
        PinOutcome::Updated { from } => format!(
            "UPDATED skills/{n} {} → {} ({})",
            from.as_deref().map(short).unwrap_or("unknown"),
            short(&pin.commit),
            pin.tag
        ),
        PinOutcome::Replaced => {
            format!("OVERWROTE skills/{n} ({} {})", pin.tag, short(&pin.commit))
        }
        PinOutcome::Kept(KeptReason::LocallyModified) => {
            format!("KEPT skills/{n} (locally modified; --force replaces it)")
        }
        PinOutcome::Kept(KeptReason::OtherSource) => {
            format!("KEPT skills/{n} (installed from elsewhere; --force replaces it)")
        }
        PinOutcome::Failed(e) => format!("FAILED skills/{n}: {e}"),
    }
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
    let pins = fleet_pins();
    println!(
        "Stock fleet — {} agents, {} workflows, {} skills",
        agents.len(),
        workflows.len(),
        pins.len()
    );
    println!("\nAgents:");
    for a in agents {
        println!("  {a}");
    }
    println!("\nWorkflows:");
    for w in workflows {
        println!("  {w}");
    }
    println!("\nSkills:");
    if pins.is_empty() {
        println!("  (none)");
    }
    for p in &pins {
        println!("  {}  {} {}", p.name, p.tag, short(&p.commit));
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
    fn fleet_pins_are_the_union_of_the_builtin_bundles_one_per_name() {
        let pins = fleet_pins();
        let mut names: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len, "one pin per skill name");
        let mut want: Vec<String> = rupu_coverage::builtin_profiles()
            .into_values()
            .flat_map(|p| p.bundle.skills.into_iter().map(|s| s.name))
            .collect();
        want.sort();
        want.dedup();
        assert_eq!(
            pins.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            want
        );
    }

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
