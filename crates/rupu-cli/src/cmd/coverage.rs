//! `rupu coverage` — inspect agentic coverage ledgers and concern catalogs.

use crate::output::formats::OutputFormat;
use anyhow::Result;
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List coverage targets recorded under .rupu/coverage/.
    List,
    /// List or show bundled concern templates.
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
    /// Print the effective catalog snapshot for a target.
    Catalog {
        /// Target id (from `coverage list`).
        target_id: String,
    },
    /// Show the derived ledger view (touched files + assertions + findings).
    Show {
        /// Target id (from `coverage list`).
        target_id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TemplatesAction {
    /// List bundled template names.
    List,
    /// Print a bundled template's concerns.
    Show { name: String },
}

fn workspace() -> Result<PathBuf> {
    Ok(std::env::current_dir()?)
}

pub async fn handle(action: Action, _format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List => workspace().and_then(|ws| run_list_in(&ws)),
        Action::Templates { action } => run_templates(action),
        Action::Catalog { target_id } => workspace().and_then(|ws| run_catalog_in(&ws, &target_id)),
        Action::Show { target_id } => workspace().and_then(|ws| run_show_in(&ws, &target_id)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("coverage error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_list_in(workspace: &Path) -> Result<()> {
    let targets = rupu_coverage::discover_targets(workspace)?;
    if targets.is_empty() {
        println!("no coverage targets under .rupu/coverage/");
        return Ok(());
    }
    for t in targets {
        println!(
            "{}  ·  {} assertions  ·  catalog: {}",
            t.target_id,
            t.assertion_lines,
            if t.has_catalog { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn run_templates(action: TemplatesAction) -> Result<()> {
    match action {
        TemplatesAction::List => {
            for name in rupu_coverage::builtin_names() {
                println!("{name}");
            }
            Ok(())
        }
        TemplatesAction::Show { name } => {
            let template = rupu_coverage::resolve_builtin(&name)
                .ok_or_else(|| anyhow::anyhow!("unknown template `{name}`"))?
                .map_err(|e| anyhow::anyhow!("template parse error: {e}"))?;
            for concern in &template.concerns {
                println!("{}  [{:?}]  {}", concern.id, concern.severity, concern.name);
            }
            Ok(())
        }
    }
}

fn run_catalog_in(workspace: &Path, target_id: &str) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    if !paths.catalog.exists() {
        anyhow::bail!("no catalog snapshot for target `{target_id}`");
    }
    let catalog = rupu_coverage::read_snapshot(&paths.catalog)?;
    println!("{} concerns in effective catalog", catalog.concerns.len());
    for c in &catalog.concerns {
        println!("  {}  [{:?}]  {}", c.id, c.severity, c.name);
    }
    Ok(())
}

fn run_show_in(workspace: &Path, target_id: &str) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let events = rupu_coverage::read_file_events(&paths)?;
    let views = rupu_coverage::file_views(&events);
    let assertions = rupu_coverage::read_concern_assertions(&paths)?;
    let findings = rupu_coverage::read_findings(&paths)?;

    println!("== files touched ({}) ==", views.len());
    for v in &views {
        println!("  {}  [{}]", v.path, format!("{:?}", v.strongest).to_lowercase());
    }
    println!("== concern assertions ({}) ==", assertions.len());
    for a in &assertions {
        println!(
            "  {} · {} · {:?} · {}",
            a.concern_id, a.file_path, a.status, a.declared_by.model
        );
    }
    println!("== findings ({}) ==", findings.len());
    for f in &findings {
        println!(
            "  {} · {:?} · {} · {}",
            f.id,
            f.severity,
            f.file_path.as_deref().unwrap_or("(repo)"),
            f.summary
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_in_handles_no_targets() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No .rupu/coverage → prints the empty message, returns Ok.
        assert!(run_list_in(tmp.path()).is_ok());
    }

    #[test]
    fn templates_list_runs() {
        assert!(run_templates(TemplatesAction::List).is_ok());
    }

    #[test]
    fn templates_show_unknown_errors() {
        assert!(run_templates(TemplatesAction::Show { name: "nope".into() }).is_err());
    }

    #[test]
    fn templates_show_known_runs() {
        assert!(run_templates(TemplatesAction::Show { name: "stride".into() }).is_ok());
    }

    #[test]
    fn catalog_missing_snapshot_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(run_catalog_in(tmp.path(), "missing").is_err());
    }

    #[test]
    fn show_empty_target_is_ok() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No ledger files → empty sections, no error.
        assert!(run_show_in(tmp.path(), "missing").is_ok());
    }

    #[test]
    fn catalog_prints_snapshot_concerns() {
        use rupu_coverage::{
            flatten, write_snapshot, CatalogMode, ConcernsBlock, ConcernsEntry, CoveragePaths,
            IncludeDirective,
        };
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();
        let cat = flatten(&ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            })],
        })
        .unwrap();
        write_snapshot(&cat, &paths.catalog).unwrap();
        assert!(run_catalog_in(tmp.path(), "tgt").is_ok());
    }
}
