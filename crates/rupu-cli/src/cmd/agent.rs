//! `rupu agent list | show <name>`.

use crate::cmd::completers::agent_names;
use crate::paths;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_agent::{load_agent, load_agents};
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
