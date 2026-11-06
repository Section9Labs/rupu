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
//! re-seeds them. Source of truth: `rupu_templates::FLEET_MANIFEST`, installed
//! by `rupu_templates::fleet` (the CP's `/api/fleet` uses the same).

use clap::Subcommand;
use std::process::ExitCode;

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

    let report = rupu_templates::fleet::install_into(&root, args.force)?;
    for f in &report.files {
        println!("{}", f.line());
    }
    println!(
        "fleet install: created {}, skipped {}, overwrote {} under {}",
        report.created(),
        report.skipped(),
        report.overwrote(),
        root.display()
    );
    if report.skipped() > 0 && !args.force {
        println!("(re-run with --force to overwrite the skipped files)");
    }
    Ok(())
}

fn print_list() {
    use rupu_templates::fleet::EntryKind;
    let entries = rupu_templates::fleet::entries();
    let names = |kind| {
        entries
            .iter()
            .filter(move |e| e.kind == kind)
            .map(|e| e.name)
    };
    println!(
        "Stock fleet — {} agents, {} workflows",
        names(EntryKind::Agent).count(),
        names(EntryKind::Workflow).count()
    );
    println!("\nAgents:");
    for a in names(EntryKind::Agent) {
        println!("  {a}");
    }
    println!("\nWorkflows:");
    for w in names(EntryKind::Workflow) {
        println!("  {w}");
    }
    println!(
        "\nInstall with `rupu fleet install` (into ~/.rupu) or `rupu fleet install --project`."
    );
}
