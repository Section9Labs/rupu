//! `rupu agent list | show <name>`.

use crate::cmd::completers::agent_names;
use crate::cmd::editor;
use crate::cmd::ui::{self, UiPrefs};
use crate::paths;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_agent::{load_agents, AgentSpec};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all available agents (global + project).
    List {
        /// Disable colored output (also honored: `NO_COLOR` env,
        /// `[ui].color = "never"` in config).
        #[arg(long)]
        no_color: bool,
    },
    /// Print an agent's frontmatter and body.
    Show {
        /// Name of the agent.
        #[arg(add = ArgValueCompleter::new(agent_names))]
        name: String,
        /// Disable colored output (also honored: `NO_COLOR` env var).
        #[arg(long)]
        no_color: bool,
        /// syntect theme name. Default: `base16-ocean.dark`.
        #[arg(long)]
        theme: Option<String>,
        /// Force pager. Default: page when stdout is a tty.
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        /// Disable pager.
        #[arg(long)]
        no_pager: bool,
    },
    /// Open an agent file in `$VISUAL` / `$EDITOR`. Validates the
    /// frontmatter on save (warn-only).
    Edit {
        /// Name of the agent.
        name: String,
        /// Force the project shadow (`.rupu/agents/<name>.md`) or the
        /// global file (`<global>/agents/<name>.md`). Default: prefer
        /// project if it exists, else global.
        #[arg(long, value_parser = ["global", "project"])]
        scope: Option<String>,
        /// Override the editor (e.g. `--editor "code --wait"`).
        /// Default: `$VISUAL` then `$EDITOR` then `vi`.
        #[arg(long)]
        editor: Option<String>,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::List { no_color } => match list(no_color).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu agent list: {e}");
                ExitCode::from(1)
            }
        },
        Action::Show {
            name,
            no_color,
            theme,
            pager,
            no_pager,
        } => {
            let pager_flag = if pager {
                Some(true)
            } else if no_pager {
                Some(false)
            } else {
                None
            };
            match show(&name, no_color, theme.as_deref(), pager_flag).await {
                Ok(()) => ExitCode::from(0),
                Err(e) => {
                    eprintln!("rupu agent show: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Action::Edit {
            name,
            scope,
            editor,
        } => match edit(&name, scope.as_deref(), editor.as_deref()).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu agent edit: {e}");
                ExitCode::from(1)
            }
        },
    }
}

async fn list(no_color: bool) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let agents = load_agents(&global, project_agents_parent.as_deref())?;

    if agents.is_empty() {
        println!(
            "(no agents found)\n\nDrop a `<name>.md` under `.rupu/agents/` (project) or \
             `~/.rupu/agents/` (global). See `rupu init --with-samples` for a starter set."
        );
        return Ok(());
    }

    let cfg = layered_config(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None);

    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["NAME", "SCOPE", "DESCRIPTION"]);
    for a in &agents {
        let scope = scope_for(&a.name, &global, project_agents_parent.as_deref());
        let desc = a.description.as_deref().unwrap_or("-");
        table.add_row(vec![
            comfy_table::Cell::new(&a.name),
            crate::output::tables::status_cell(&scope, &prefs),
            comfy_table::Cell::new(desc),
        ]);
    }
    println!("{table}");
    Ok(())
}

async fn show(
    name: &str,
    no_color: bool,
    theme: Option<&str>,
    pager_flag: Option<bool>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));

    let path = locate_agent_file(name, &global, project_agents_parent.as_deref())?;
    let body = std::fs::read_to_string(&path)?;

    let cfg = layered_config(&global, project_root.as_deref());
    let prefs = UiPrefs::resolve(&cfg.ui, no_color, theme, pager_flag);

    let rendered = ui::highlight_agent_file(&body, &prefs);
    ui::paginate(&rendered, &prefs)?;
    Ok(())
}

async fn edit(name: &str, scope: Option<&str>, editor_override: Option<&str>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));

    let target = resolve_agent_path(
        name,
        scope,
        &global,
        project_agents_parent.as_deref(),
    )?;
    println!("editing {} ({})", target.display(), describe_scope(&target, &global));

    editor::open_for_edit(editor_override, &target)?;

    match AgentSpec::parse_file(&target) {
        Ok(_) => {
            println!("✓ {name}: frontmatter parses cleanly");
            Ok(())
        }
        Err(e) => {
            eprintln!("⚠ {name}: failed to re-parse after save:\n  {e}");
            Ok(())
        }
    }
}

fn locate_agent_file(
    name: &str,
    global: &std::path::Path,
    project_parent: Option<&std::path::Path>,
) -> anyhow::Result<std::path::PathBuf> {
    if let Some(p) = project_parent {
        let candidate = p.join("agents").join(format!("{name}.md"));
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    let candidate = global.join("agents").join(format!("{name}.md"));
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(anyhow::anyhow!(
        "agent `{name}` not found in project or global agents dir"
    ))
}

fn layered_config(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
) -> rupu_config::Config {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

/// Pick the on-disk file to edit. With `--scope` set we honor it
/// strictly; without it we prefer the project shadow if present and
/// fall back to global.
fn resolve_agent_path(
    name: &str,
    scope: Option<&str>,
    global: &std::path::Path,
    project_parent: Option<&std::path::Path>,
) -> anyhow::Result<PathBuf> {
    let project_path = project_parent.map(|p| p.join("agents").join(format!("{name}.md")));
    let global_path = global.join("agents").join(format!("{name}.md"));

    match scope {
        Some("project") => match project_path {
            Some(p) if p.exists() => Ok(p),
            Some(p) => Err(anyhow::anyhow!(
                "agent `{name}` not found at project scope ({})",
                p.display()
            )),
            None => Err(anyhow::anyhow!(
                "no project root detected; cannot use --scope project"
            )),
        },
        Some("global") => {
            if global_path.exists() {
                Ok(global_path)
            } else {
                Err(anyhow::anyhow!(
                    "agent `{name}` not found at global scope ({})",
                    global_path.display()
                ))
            }
        }
        Some(other) => Err(anyhow::anyhow!(
            "invalid --scope `{other}` (expected `global` or `project`)"
        )),
        None => {
            if let Some(p) = project_path {
                if p.exists() {
                    return Ok(p);
                }
            }
            if global_path.exists() {
                Ok(global_path)
            } else {
                Err(anyhow::anyhow!(
                    "agent `{name}` not found in project or global agents dir"
                ))
            }
        }
    }
}

fn describe_scope(path: &std::path::Path, global: &std::path::Path) -> &'static str {
    if path.starts_with(global) {
        "global"
    } else {
        "project"
    }
}

fn scope_for(name: &str, global: &std::path::Path, project: Option<&std::path::Path>) -> String {
    if let Some(p) = project {
        if p.join("agents").join(format!("{name}.md")).exists() {
            return "project".to_string();
        }
    }
    if global.join("agents").join(format!("{name}.md")).exists() {
        "global".to_string()
    } else {
        "?".to_string()
    }
}
