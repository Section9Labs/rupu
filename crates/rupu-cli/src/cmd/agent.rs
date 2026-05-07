//! `rupu agent list | show <name>`.

use crate::cmd::completers::agent_names;
use crate::cmd::editor;
use crate::paths;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_agent::{load_agent, load_agents, AgentSpec};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all available agents (global + project).
    List,
    /// Print an agent's frontmatter and body.
    Show {
        /// Name of the agent.
        #[arg(add = ArgValueCompleter::new(agent_names))]
        name: String,
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
        Action::List => match list().await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu agent list: {e}");
                ExitCode::from(1)
            }
        },
        Action::Show { name } => match show(&name).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu agent show: {e}");
                ExitCode::from(1)
            }
        },
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

async fn list() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let agents = load_agents(&global, project_agents_parent.as_deref())?;

    println!("{:<24} {:<10} DESCRIPTION", "NAME", "SCOPE");
    for a in &agents {
        let scope = scope_for(&a.name, &global, project_agents_parent.as_deref());
        let desc = a.description.as_deref().unwrap_or("-");
        println!("{:<24} {:<10} {}", a.name, scope, desc);
    }
    Ok(())
}

async fn show(name: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let spec = load_agent(&global, project_agents_parent.as_deref(), name)?;
    println!("name:        {}", spec.name);
    if let Some(d) = &spec.description {
        println!("description: {d}");
    }
    if let Some(p) = &spec.provider {
        println!("provider:    {p}");
    }
    if let Some(m) = &spec.model {
        println!("model:       {m}");
    }
    if let Some(t) = &spec.tools {
        println!("tools:       {}", t.join(", "));
    }
    if let Some(mt) = spec.max_turns {
        println!("maxTurns:    {mt}");
    }
    if let Some(pm) = &spec.permission_mode {
        println!("mode:        {pm}");
    }
    println!("\n--- system prompt ---");
    print!("{}", spec.system_prompt);
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
