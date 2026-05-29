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
}
